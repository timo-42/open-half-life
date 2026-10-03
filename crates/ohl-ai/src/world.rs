//! The fixed-tick AI world: components, the per-tick pipeline and a
//! determinism hash.
//!
//! [`AiWorld`] owns everything that is not per-entity — the brains, the
//! relationship table, the sound list, the damage queue, the squad roster
//! and one seeded [`Pcg32`] — and drives every entity carrying an [`Actor`]
//! and a [`MonsterAi`] component through one tick of sense, decide, schedule
//! and move.
//!
//! Determinism is a hard requirement: entities are processed in ascending
//! [`hecs::Entity::id`] order rather than query order, senses read a
//! snapshot taken before the first entity moves, and every random draw comes
//! from the one seeded generator. [`AiWorld::state_hash`] exists so a replay
//! test can assert bit-identical outcomes.

use glam::Vec3;
use hecs::{Entity, World};
use ohl_core::StreamingSha256;
use ohl_physics::{CollisionModel, Hull};
use std::collections::BTreeMap;

use crate::damage::{DamageEvent, DamageQueue, DamageSink, summarize};
use crate::monsters::NavBridge;
use crate::movement::{
    self, MoveResult, Route, StuckDetector, forward_from_yaw, move_toward, normalize_yaw,
    turn_toward, yaw_toward,
};
use crate::rng::Pcg32;
use crate::schedule::{
    Activity, Brain, RunOutcome, Schedule, ScheduleRunner, Task, TaskExecutor, TaskStatus,
};
use crate::senses::{
    Candidate, EnemyMemory, SightContext, SoundEvent, SoundKind, SoundList, Viewer, listen, look,
};
use crate::squad::{SquadCandidate, SquadRoster};
use crate::state::{Classification, Conditions, MonsterState, RelationshipTable};

/// How fast a monster turns, in degrees per second.
///
/// **Provisional**, to be black-box observed.
pub const TURN_RATE: f32 = 300.0;

/// How far a monster backs away when looking for cover, in world units.
///
/// **Provisional**, to be black-box observed.
pub const COVER_DISTANCE: f32 = 320.0;

/// How close counts as having faced a target, in degrees.
pub const FACING_TOLERANCE: f32 = 5.0;

/// The largest number of events one tick reports.
pub const MAX_EVENTS_PER_TICK: usize = 4_096;

/// Identifies a registered [`Brain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BrainId(pub usize);

/// The kinematic and faction state the AI reads and writes.
///
/// A component on every entity the AI can perceive, including the player and
/// entities with no [`MonsterAi`] of their own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Actor {
    /// The faction.
    pub classification: Classification,
    /// World-space origin.
    pub origin: Vec3,
    /// The eye offset above the origin.
    pub view_ofs: Vec3,
    /// The facing yaw, in degrees.
    pub yaw: f32,
    /// Current health.
    pub health: f32,
    /// Whether the entity is alive.
    pub alive: bool,
    /// Whether the entity is the player.
    pub is_client: bool,
    /// The collision hull this entity moves with.
    pub hull: Hull,
}

impl Actor {
    /// A living, standing-hull actor at `origin`.
    #[must_use]
    pub fn new(classification: Classification, origin: Vec3) -> Self {
        Self {
            classification,
            origin,
            view_ofs: Vec3::new(0.0, 0.0, 28.0),
            yaw: 0.0,
            health: 100.0,
            alive: true,
            is_client: false,
            hull: Hull::Standing,
        }
    }

    /// The same actor marked as the player.
    #[must_use]
    pub fn as_client(mut self) -> Self {
        self.is_client = true;
        self.classification = Classification::Player;
        self
    }

    /// The same actor facing `yaw` degrees.
    #[must_use]
    pub fn facing(mut self, yaw: f32) -> Self {
        self.yaw = normalize_yaw(yaw);
        self
    }

    /// The same actor with the given health.
    #[must_use]
    pub fn with_health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }

    /// The eye position sight originates from and is traced to.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.origin + self.view_ofs
    }

    /// The unit forward vector implied by [`Self::yaw`].
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        forward_from_yaw(self.yaw)
    }
}

/// Marks a monster spawned with the published `Prisoner` spawnflag
/// (`crate::spawn::SPAWNFLAG_PRISONER`).
///
/// Published (see `docs/FORMAT_SOURCES.md`, "Monster definitions"): TWHL's
/// "VERC: Common Monster Properties" — "When this is checked, normal AI is
/// disabled, so the monster won't attack the player. This can be useful
/// when you're using normally offensive monsters in a scripted_sequence."
/// — and the line every TWHL `monster_*` entity page carries for the same
/// bit: "Won't attack, or be attacked by, other monsters."
///
/// Modelled as the narrowest reading of both that a map can rely on: a
/// prisoner never acquires an enemy — not by sight, not from a squad mate,
/// not from being hurt — and no other monster's sight ever reads it as one
/// ([`crate::senses::sighting_relationship`], the one hostility rule, reads
/// a *hostile* relationship with a prisoner on either end as
/// [`crate::state::Relationship::NoRelationship`]). An enemy it remembers
/// from before — only a save made mid-fight can supply one — is dropped,
/// together with any attack schedule it was restored into. Never having an
/// enemy, it never reaches [`crate::state::MonsterState::Combat`], which is
/// what lets a `scripted_sequence` without `Override AI` take it over with
/// the player standing in front of it — the use the first page names.
/// Everything else about it is left alone: it still idles, hears, walks a
/// scripted route and plays a script's animations, and fear and alliance
/// are untouched both ways (a scientist still runs from an armed prisoner,
/// and a prisoner scientist still runs from a real hostile), because
/// neither is an attack.
///
/// Derived from the entity definition's own spawnflags at spawn
/// ([`crate::spawn::attach_monsters`]) and never changed afterwards, so it
/// needs no save-file field: a restored or carried monster is rebuilt from
/// the same definition (`ohl-engine`'s `tests/prisoner_monsters.rs` pins
/// both). That rests on a reading, not on a published fact: that the flag
/// never lifts. Whether a retail prisoner the player hurts turns on them is
/// `TODO(black-box)`; if it does, the flag becomes runtime state and needs
/// a save field of its own, with a save-format version bump. One older
/// save shape already loses it: a level change captured before carried
/// entities' keyvalues travelled re-creates a carried monster from its
/// classname, names and placement alone, with no spawnflags, so a carried
/// prisoner from such a save arrives an ordinary monster. A
/// `monstermaker`'s children carry no definition of their own and are
/// never prisoners; no page says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Prisoner;

/// The `netname` squad membership of a monster.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SquadTag {
    /// The `netname` keyvalue.
    pub name: String,
    /// Whether the `SquadLeader` spawnflag is set.
    pub leader: bool,
}

impl SquadTag {
    /// A plain member of `name`.
    #[must_use]
    pub fn member(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            leader: false,
        }
    }

    /// The leader of `name`.
    #[must_use]
    pub fn leader(name: impl Into<String>) -> Self {
        Self {
            leader: true,
            ..Self::member(name)
        }
    }
}

/// The per-monster AI state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonsterAi {
    /// Which registered brain decides for this monster.
    pub brain: BrainId,
    /// The current state.
    pub state: MonsterState,
    /// The conditions computed at the start of the last tick.
    pub conditions: Conditions,
    /// The running schedule.
    pub runner: ScheduleRunner,
    /// What is remembered about the acquired enemy.
    pub memory: Option<EnemyMemory>,
    /// The route currently being followed.
    pub route: Route,
    /// Where a move task was told to go.
    pub move_target: Option<Vec3>,
    /// The cover position [`Task::FindCover`] chose.
    pub cover: Option<Vec3>,
    /// The current animation intent.
    pub activity: Activity,
    /// The speed the path tasks selected, in units per second.
    pub move_speed: f32,
    /// The yaw a facing task is turning toward.
    pub ideal_yaw: f32,
    /// Consecutive ticks of no movement progress.
    pub stuck: StuckDetector,
    /// Conditions produced late in a tick and delivered on the next one.
    pub pending_conditions: Conditions,
}

impl MonsterAi {
    /// A fresh idle monster driven by `brain`.
    #[must_use]
    pub fn new(brain: BrainId) -> Self {
        Self {
            brain,
            state: MonsterState::Idle,
            ..Self::default()
        }
    }

    /// The running schedule's stable name, or `""`.
    #[must_use]
    pub fn schedule_name(&self) -> &'static str {
        self.runner.schedule_name()
    }

    /// The acquired enemy, if any.
    #[must_use]
    pub fn enemy(&self) -> Option<Entity> {
        self.memory.map(|memory| memory.entity)
    }

    /// Where the enemy was last seen, if anything is remembered.
    #[must_use]
    pub fn last_known_position(&self) -> Option<Vec3> {
        self.memory.map(|memory| memory.last_known_position)
    }
}

/// Which attack a monster performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackKind {
    /// Primary melee.
    Melee1,
    /// Secondary melee.
    Melee2,
    /// Primary ranged.
    Range1,
    /// Secondary ranged.
    Range2,
}

/// Something the AI did that the rest of the engine may care about.
#[derive(Debug, Clone, PartialEq)]
pub enum AiEventKind {
    /// The monster state changed.
    StateChanged {
        /// The state left behind.
        from: MonsterState,
        /// The state entered.
        to: MonsterState,
    },
    /// A schedule was selected and started.
    ScheduleStarted(&'static str),
    /// A schedule stopped.
    ScheduleEnded {
        /// The schedule's stable name.
        name: &'static str,
        /// Why it stopped.
        outcome: RunOutcome,
    },
    /// An enemy was acquired.
    EnemyAcquired(Entity),
    /// The acquired enemy was forgotten.
    EnemyLost,
    /// An attack was performed; combat resolves the damage.
    Attack {
        /// Which attack.
        kind: AttackKind,
        /// The enemy it was aimed at, if any.
        target: Option<Entity>,
    },
    /// The monster reloaded.
    Reloaded,
    /// A named animation sequence should play.
    PlaySequence(&'static str),
    /// The animation intent changed.
    ActivityChanged(Activity),
    /// A sound was emitted into the world's sound list.
    SoundEmitted(SoundKind),
    /// The monster died.
    Died,
}

/// One [`AiEventKind`] with the entity it happened to.
#[derive(Debug, Clone, PartialEq)]
pub struct AiEvent {
    /// The monster.
    pub entity: Entity,
    /// What happened.
    pub kind: AiEventKind,
}

/// The fixed-tick AI simulation.
pub struct AiWorld {
    brains: Vec<Box<dyn Brain>>,
    relationships: RelationshipTable,
    sounds: SoundList,
    damage: DamageQueue,
    squads: SquadRoster,
    rng: Pcg32,
    tick_count: u64,
    /// The real, `ohl-nav`-backed navigator, when one has been built for
    /// the current map; `None` uses the straight-line fallback instead
    /// (see [`advance_route`]).
    navigator: Option<NavBridge>,
}

impl core::fmt::Debug for AiWorld {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AiWorld")
            .field("brains", &self.brains.len())
            .field("sounds", &self.sounds.len())
            .field("damage", &self.damage.len())
            .field("squads", &self.squads.squads().len())
            .field("tick_count", &self.tick_count)
            .finish_non_exhaustive()
    }
}

impl AiWorld {
    /// A world seeded with `seed` and the provisional relationship table.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            brains: Vec::new(),
            relationships: RelationshipTable::provisional(),
            sounds: SoundList::new(),
            damage: DamageQueue::new(),
            squads: SquadRoster::new(),
            rng: Pcg32::new(seed),
            tick_count: 0,
            navigator: None,
        }
    }

    /// Registers a brain and returns the id to put in [`MonsterAi::brain`].
    pub fn register_brain(&mut self, brain: Box<dyn Brain>) -> BrainId {
        self.brains.push(brain);
        BrainId(self.brains.len() - 1)
    }

    /// Attaches the real, `ohl-nav`-backed navigator built for the current
    /// map, replacing the straight-line fallback every path task used until
    /// now.
    pub fn attach_navigator(&mut self, bridge: NavBridge) {
        self.navigator = Some(bridge);
    }

    /// Detaches the navigator, restoring the straight-line fallback (e.g.
    /// when leaving a map), and returns it.
    pub fn detach_navigator(&mut self) -> Option<NavBridge> {
        self.navigator.take()
    }

    /// The attached navigator, if any.
    #[must_use]
    pub fn navigator(&self) -> Option<&NavBridge> {
        self.navigator.as_ref()
    }

    /// The relationship table, for per-map overrides.
    #[must_use]
    pub fn relationships(&self) -> &RelationshipTable {
        &self.relationships
    }

    /// The relationship table, mutably.
    pub fn relationships_mut(&mut self) -> &mut RelationshipTable {
        &mut self.relationships
    }

    /// The live sound and scent list.
    #[must_use]
    pub fn sounds(&self) -> &SoundList {
        &self.sounds
    }

    /// The squad roster as of the last tick.
    #[must_use]
    pub fn squads(&self) -> &SquadRoster {
        &self.squads
    }

    /// Number of ticks run.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// This world's own random stream's raw state, for save files. Additive,
    /// for §6/§8 P4b of the M7.9 design plan (recorded in local design
    /// notes, not part of the repository): a caller that owns the seed
    /// this world was constructed with (`Systems::rng`, in `ohl-engine`)
    /// still needs this to continue the *same* stream across a save/load,
    /// since [`Self::new`] always reseeds from its constructor argument
    /// rather than resuming wherever a previous instance's stream had
    /// advanced to.
    #[must_use]
    pub fn rng_snapshot(&self) -> (u64, u64) {
        self.rng.snapshot()
    }

    /// Restores this world's random stream and tick counter from a
    /// previous [`Self::rng_snapshot`]/[`Self::tick_count`], so a
    /// determinism test (or a real save/load) that continues ticking after
    /// a restore reproduces the same sequence an uninterrupted run would.
    /// Additive, for save-file restore.
    pub fn restore_determinism_state(&mut self, rng: (u64, u64), tick_count: u64) {
        self.rng = Pcg32::from_snapshot(rng);
        self.tick_count = tick_count;
    }

    /// Adds a sound or scent for the next tick's [`listen`] pass.
    pub fn emit_sound(&mut self, event: SoundEvent) -> bool {
        self.sounds.push(event)
    }

    /// Queues a damage event for the next tick.
    pub fn apply_damage(&mut self, event: DamageEvent) -> bool {
        self.damage.push_damage(event)
    }

    /// Advances every AI entity in `world` by `dt` seconds.
    ///
    /// Returns the events produced, bounded by [`MAX_EVENTS_PER_TICK`].
    pub fn tick(&mut self, world: &mut World, context: &SightContext<'_>, dt: f32) -> Vec<AiEvent> {
        let mut events = Vec::new();
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };

        let candidates = snapshot_candidates(world);
        let by_entity: BTreeMap<Entity, Candidate> = candidates
            .iter()
            .map(|candidate| (candidate.entity, *candidate))
            .collect();
        self.rebuild_squads(world);

        let mut order: Vec<Entity> = world
            .query::<(Entity, &Actor, &MonsterAi)>()
            .iter()
            .map(|(entity, _, _)| entity)
            .collect();
        order.sort_unstable_by_key(|entity: &Entity| entity.id());

        if let Some(navigator) = self.navigator.as_mut() {
            navigator.begin_tick(&order);
        }

        for entity in order {
            self.tick_one(
                world,
                entity,
                &candidates,
                &by_entity,
                context,
                dt,
                &mut events,
            );
        }

        self.sounds.expire(dt);
        self.damage.clear();
        self.tick_count += 1;
        events.truncate(MAX_EVENTS_PER_TICK);
        events
    }

    fn rebuild_squads(&mut self, world: &World) {
        let mut candidates: Vec<(u32, SquadCandidate)> = world
            .query::<(Entity, &SquadTag, &MonsterAi)>()
            .iter()
            .map(|(entity, tag, _)| (entity, tag.clone()))
            .filter(|(_, tag)| !tag.name.is_empty())
            .map(|(entity, tag)| {
                (
                    entity.id(),
                    SquadCandidate {
                        entity,
                        squad_name: tag.name,
                        is_leader: tag.leader,
                    },
                )
            })
            .collect();
        candidates.sort_unstable_by_key(|(id, _)| *id);
        let ordered: Vec<SquadCandidate> = candidates
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect();

        let previous: BTreeMap<String, (Option<Entity>, Option<Vec3>)> = self
            .squads
            .squads()
            .iter()
            .map(|squad| (squad.name.clone(), (squad.enemy, squad.enemy_position)))
            .collect();
        let mut rebuilt = SquadRoster::build(&ordered);
        for squad in rebuilt.squads_mut() {
            if let Some((enemy, position)) = previous.get(&squad.name) {
                squad.enemy = *enemy;
                squad.enemy_position = *position;
            }
        }
        self.squads = rebuilt;
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn tick_one(
        &mut self,
        world: &mut World,
        entity: Entity,
        candidates: &[Candidate],
        by_entity: &BTreeMap<Entity, Candidate>,
        context: &SightContext<'_>,
        dt: f32,
        events: &mut Vec<AiEvent>,
    ) {
        let Ok(mut actor) = world.get::<&Actor>(entity).map(|actor| *actor) else {
            return;
        };
        let Ok(mut ai) = world
            .get::<&mut MonsterAi>(entity)
            .map(|mut ai| core::mem::take(&mut *ai))
        else {
            return;
        };
        let Some(brain) = self.brains.get(ai.brain.0) else {
            // No brain registered: leave the monster exactly as it was.
            if let Ok(mut slot) = world.get::<&mut MonsterAi>(entity) {
                *slot = ai;
            }
            return;
        };
        let senses = brain.senses();

        let mut conditions = ai.pending_conditions;
        ai.pending_conditions = Conditions::EMPTY;

        if !actor.alive || actor.health <= 0.0 {
            actor.alive = false;
            if ai.state != MonsterState::Dead {
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::StateChanged {
                        from: ai.state,
                        to: MonsterState::Dead,
                    },
                });
                ai.state = MonsterState::Dead;
            }
        }

        // --- Senses -------------------------------------------------------
        // The published `Prisoner` spawnflag (see [`Prisoner`]): such a
        // monster never acquires an enemy by any of the three routes below
        // — sight, a squad mate's shared enemy, or being hurt.
        let prisoner = world.get::<&Prisoner>(entity).is_ok();
        if prisoner {
            // Nothing below lets a prisoner acquire an enemy, but one can
            // arrive from outside this tick: a save made mid-fight, before
            // the flag was modelled, restores the memory and the attack
            // schedule the monster was running. `EnemyMemory::occlude`
            // would keep an unseen enemy within its range for good, and an
            // attack task with no enemy still fires straight ahead. Drop
            // both; neither can come back.
            if ai.memory.take().is_some() {
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyLost,
                });
            }
            if ai.runner.schedule().is_some_and(attacks) {
                ai.runner.clear();
            }
        }
        let viewer = Viewer {
            entity,
            origin: actor.origin,
            view_ofs: actor.view_ofs,
            forward: actor.forward(),
            classification: actor.classification,
            prisoner,
        };
        let sight = look(&viewer, &senses, candidates, &self.relationships, context);
        conditions |= sight.conditions;
        let heard = listen(viewer.eye(), &senses, &self.sounds);
        conditions |= heard.conditions;
        if let Some(sound) = heard.best {
            ai.move_target = Some(sound.position);
        }

        // --- Enemy acquisition and memory ---------------------------------
        if let Some(seen) = sight.enemy {
            let is_new = ai.memory.is_none_or(|memory| memory.entity != seen.entity);
            if is_new {
                conditions |= Conditions::NEW_ENEMY;
                ai.memory = Some(EnemyMemory::seen(&seen));
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyAcquired(seen.entity),
                });
            } else if let Some(memory) = ai.memory.as_mut() {
                memory.refresh(&seen);
            }
            if seen.facing_viewer {
                conditions |= Conditions::ENEMY_FACING_ME;
            }
            if brain.has_melee_attack()
                && brain.melee_in_reach(actor.origin, seen.origin, seen.distance)
            {
                conditions |= Conditions::CAN_MELEE_ATTACK1;
            }
            if brain.has_range_attack() && seen.distance <= brain.range_attack_range() {
                conditions |= Conditions::CAN_RANGE_ATTACK1;
            }
            self.squads.share_enemy(entity, seen.entity, seen.origin);
        } else if let Some(mut memory) = ai.memory {
            let known = by_entity.get(&memory.entity);
            let dead = known.is_none_or(|candidate| !candidate.alive);
            let distance = known.map(|candidate| (candidate.origin - actor.origin).length());
            if dead {
                conditions |= Conditions::ENEMY_DEAD;
                ai.memory = None;
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyLost,
                });
            } else {
                conditions |= Conditions::ENEMY_OCCLUDED;
                if distance.is_some_and(|distance| distance > senses.look_distance) {
                    conditions |= Conditions::ENEMY_TOOFAR;
                }
                if memory.occlude(dt, distance) {
                    ai.memory = Some(memory);
                } else {
                    ai.memory = None;
                    events.push(AiEvent {
                        entity,
                        kind: AiEventKind::EnemyLost,
                    });
                }
            }
        } else if let Some((shared, position)) =
            self.squads.shared_enemy(entity).filter(|_| !prisoner)
        {
            // No enemy of our own: take the squad's, remembered but unseen.
            if shared != entity && by_entity.get(&shared).is_some_and(|c| c.alive) {
                conditions |= Conditions::NEW_ENEMY | Conditions::ENEMY_OCCLUDED;
                ai.memory = Some(EnemyMemory {
                    entity: shared,
                    last_known_position: position,
                    time_since_seen: 0.0,
                    occluded: true,
                    last_known_distance: (position - actor.origin).length(),
                });
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyAcquired(shared),
                });
            }
        }

        // --- Damage -------------------------------------------------------
        if let Some((total, attacker, position, provokes)) = summarize(&self.damage, entity) {
            if total >= brain.heavy_damage_threshold() {
                conditions |= Conditions::HEAVY_DAMAGE;
            } else {
                conditions |= Conditions::LIGHT_DAMAGE;
            }
            if provokes {
                conditions |= Conditions::PROVOKED;
            }
            if ai.memory.is_none()
                && !prisoner
                && let Some(attacker) = attacker
                && attacker != entity
            {
                ai.memory = Some(EnemyMemory {
                    entity: attacker,
                    last_known_position: position,
                    time_since_seen: 0.0,
                    occluded: true,
                    last_known_distance: (position - actor.origin).length(),
                });
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyAcquired(attacker),
                });
            }
        }

        if ai.stuck.is_stuck() {
            conditions |= Conditions::BLOCKED;
        }
        ai.conditions = conditions;

        // --- Scripted possession ------------------------------------------
        // A monster a `scripted_sequence` has taken over keeps sensing and
        // remembering — that is what lets an interruptible script notice
        // damage or an enemy — but chooses no state and runs no schedule
        // while `crate::scripts::ScriptHold` is present. It still follows
        // whatever route the script set, through exactly the same
        // navigator seam every other route uses.
        if world.get::<&crate::scripts::ScriptHold>(entity).is_ok() {
            ai.runner.clear();
            let moved = advance_route(
                entity,
                &mut actor,
                &mut ai,
                context.collision,
                self.navigator.as_mut(),
                dt,
            );
            if ai.move_speed > 0.0 {
                if ai.stuck.record(moved) {
                    ai.pending_conditions |= Conditions::BLOCKED;
                }
            } else {
                ai.stuck.reset();
            }
            if let Ok(mut slot) = world.get::<&mut Actor>(entity) {
                *slot = actor;
            }
            if let Ok(mut slot) = world.get::<&mut MonsterAi>(entity) {
                *slot = ai;
            }
            return;
        }

        // --- State --------------------------------------------------------
        let next_state = brain.next_state(ai.state, conditions);
        if next_state != ai.state {
            events.push(AiEvent {
                entity,
                kind: AiEventKind::StateChanged {
                    from: ai.state,
                    to: next_state,
                },
            });
            ai.state = next_state;
        }

        // --- Schedule -----------------------------------------------------
        let enemy_position = ai
            .memory
            .and_then(|memory| by_entity.get(&memory.entity).map(|c| c.origin));
        let enemy_entity = ai.memory.map(|memory| memory.entity);
        let last_known = ai.last_known_position();

        let mut runner = core::mem::take(&mut ai.runner);
        if !runner.is_running() {
            let schedule = brain.select_schedule(ai.state, conditions);
            runner.start(schedule);
            events.push(AiEvent {
                entity,
                kind: AiEventKind::ScheduleStarted(schedule.name),
            });
        }
        let running_name = runner.schedule_name();

        let outcome = {
            let mut executor = MonsterExecutor {
                entity,
                actor: &mut actor,
                ai: &mut ai,
                brain: brain.as_ref(),
                collision: context.collision,
                enemy_entity,
                enemy_position,
                last_known,
                sounds: &mut self.sounds,
                events,
                dt,
                tick_count: self.tick_count,
            };
            runner.tick(dt, conditions, &mut self.rng, &mut executor)
        };
        if outcome.needs_new_schedule() {
            if outcome != RunOutcome::Idle {
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::ScheduleEnded {
                        name: running_name,
                        outcome,
                    },
                });
            }
            // Re-select immediately, from this tick's conditions plus why
            // the last schedule stopped, so a monster is never left without
            // a schedule between ticks and `TASK_FAILED`/`SCHEDULE_DONE`
            // cannot leak forward and interrupt their own replacement.
            let post = conditions | outcome.condition();
            let schedule = brain.select_schedule(ai.state, post);
            runner.start(schedule);
            events.push(AiEvent {
                entity,
                kind: AiEventKind::ScheduleStarted(schedule.name),
            });
        }
        ai.runner = runner;

        // --- Movement -----------------------------------------------------
        let moved = advance_route(
            entity,
            &mut actor,
            &mut ai,
            context.collision,
            self.navigator.as_mut(),
            dt,
        );
        if ai.move_speed > 0.0 {
            if ai.stuck.record(moved) {
                ai.pending_conditions |= Conditions::BLOCKED;
            }
        } else {
            ai.stuck.reset();
        }

        if let Ok(mut slot) = world.get::<&mut Actor>(entity) {
            *slot = actor;
        }
        if let Ok(mut slot) = world.get::<&mut MonsterAi>(entity) {
            *slot = ai;
        }
    }

    /// A digest of everything that must replay identically.
    ///
    /// Covers the tick counter, the generator state, the relationship table
    /// and, in ascending entity order, each actor's kinematics and each
    /// monster's state, conditions, schedule cursor, memory and route.
    #[must_use]
    pub fn state_hash(&self, world: &World) -> [u8; 32] {
        let mut hasher = StreamingSha256::new();
        hasher.update(&self.tick_count.to_le_bytes());
        let (state, increment) = self.rng.snapshot();
        hasher.update(&state.to_le_bytes());
        hasher.update(&increment.to_le_bytes());
        hasher.update(&self.relationships.to_tags());
        hasher.update(
            &u32::try_from(self.sounds.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );

        let mut rows: Vec<(u32, Vec<u8>)> = world
            .query::<(Entity, &Actor)>()
            .iter()
            .map(|(entity, actor)| (entity.id(), actor_bytes(actor)))
            .collect();
        rows.sort_unstable_by_key(|(id, _)| *id);
        for (id, bytes) in rows {
            hasher.update(&id.to_le_bytes());
            hasher.update(&bytes);
        }

        let mut ai_rows: Vec<(u32, Vec<u8>)> = world
            .query::<(Entity, &MonsterAi)>()
            .iter()
            .map(|(entity, ai)| (entity.id(), ai_bytes(ai)))
            .collect();
        ai_rows.sort_unstable_by_key(|(id, _)| *id);
        for (id, bytes) in ai_rows {
            hasher.update(&id.to_le_bytes());
            hasher.update(&bytes);
        }

        for squad in self.squads.squads() {
            hasher.update(squad.name.as_bytes());
            hasher.update(&squad.leader.id().to_le_bytes());
            for member in &squad.members {
                hasher.update(&member.id().to_le_bytes());
            }
            hasher.update(&[u8::from(squad.enemy.is_some())]);
        }

        hasher.finalize()
    }
}

fn actor_bytes(actor: &Actor) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(40);
    bytes.push(actor.classification.index().try_into().unwrap_or(u8::MAX));
    for value in [
        actor.origin.x,
        actor.origin.y,
        actor.origin.z,
        actor.view_ofs.x,
        actor.view_ofs.y,
        actor.view_ofs.z,
        actor.yaw,
        actor.health,
    ] {
        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    bytes.push(u8::from(actor.alive));
    bytes.push(u8::from(actor.is_client));
    bytes.push(u8::try_from(actor.hull.index()).unwrap_or(u8::MAX));
    bytes
}

fn ai_bytes(ai: &MonsterAi) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    bytes.push(ai.state.tag());
    bytes.extend_from_slice(&ai.conditions.bits().to_le_bytes());
    bytes.extend_from_slice(&ai.pending_conditions.bits().to_le_bytes());
    bytes.extend_from_slice(ai.runner.schedule_name().as_bytes());
    bytes.extend_from_slice(
        &u32::try_from(ai.runner.task_index())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&ai.runner.timer().to_bits().to_le_bytes());
    bytes.push(ai.activity.tag());
    bytes.extend_from_slice(&ai.move_speed.to_bits().to_le_bytes());
    bytes.extend_from_slice(&ai.ideal_yaw.to_bits().to_le_bytes());
    bytes.extend_from_slice(&ai.stuck.ticks().to_le_bytes());
    match ai.memory {
        Some(memory) => {
            bytes.push(1);
            bytes.extend_from_slice(&memory.entity.id().to_le_bytes());
            for value in memory.last_known_position.to_array() {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            bytes.push(u8::from(memory.occluded));
        }
        None => bytes.push(0),
    }
    bytes.extend_from_slice(
        &u32::try_from(ai.route.current)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for waypoint in &ai.route.waypoints {
        for value in waypoint.to_array() {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    bytes
}

fn snapshot_candidates(world: &World) -> Vec<Candidate> {
    let mut candidates: Vec<(u32, Candidate)> = world
        .query::<(Entity, &Actor, Option<&Prisoner>)>()
        .iter()
        .map(|(entity, actor, prisoner)| {
            (
                entity.id(),
                Candidate {
                    entity,
                    classification: actor.classification,
                    origin: actor.origin,
                    view_ofs: actor.view_ofs,
                    forward: actor.forward(),
                    alive: actor.alive,
                    is_client: actor.is_client,
                    prisoner: prisoner.is_some(),
                },
            )
        })
        .collect();
    candidates.sort_unstable_by_key(|(id, _)| *id);
    candidates.into_iter().map(|(_, c)| c).collect()
}

/// Follows the current route by one tick, returning the distance travelled.
///
/// `ai.route` still only ever carries the single ultimate goal a path task
/// set ([`start_route`](MonsterExecutor::start_route) never changed): the
/// node-graph routing a [`NavBridge`] does — multiple waypoints, per-hull
/// links, local steering around obstacles — lives entirely inside the
/// bridge's own per-actor cache, keyed off this same goal. `Route` stays
/// the high-level "am I still moving, has the goal drifted" bookkeeping
/// either way, so every other consumer (`WaitForMovement`, `StopMoving`,
/// the determinism hash) is unaffected by whether a navigator is attached.
fn advance_route(
    entity: Entity,
    actor: &mut Actor,
    ai: &mut MonsterAi,
    collision: Option<&CollisionModel>,
    navigator: Option<&mut NavBridge>,
    dt: f32,
) -> f32 {
    if ai.move_speed <= 0.0 || ai.route.is_finished() {
        return 0.0;
    }
    let Some(waypoint) = ai.route.waypoint() else {
        return 0.0;
    };
    let step = ai.move_speed * dt;
    let (position, distance) = match (navigator, collision) {
        (Some(navigator), Some(model)) => {
            let next = navigator.next_move(entity, actor.origin, waypoint, actor.hull, model, step);
            (next, (next - actor.origin).length())
        }
        (_, Some(model)) => {
            let result = move_toward(model, actor.hull, actor.origin, waypoint, ai.move_speed, dt);
            (result.position, result.distance)
        }
        (_, None) => {
            let result = straight_step(
                actor.origin,
                waypoint,
                ai.move_speed,
                dt,
                movement::flies(actor.hull),
            );
            (result.position, result.distance)
        }
    };
    actor.origin = position;
    if let Some(yaw) = yaw_toward(actor.origin, waypoint) {
        let (turned, _) = turn_toward(actor.yaw, yaw, TURN_RATE * dt);
        actor.yaw = turned;
    }
    ai.route.advance_if_reached(actor.origin);
    if ai.route.is_finished() {
        ai.move_speed = 0.0;
    }
    distance
}

/// The no-collision-data fallback: move straight toward the waypoint,
/// horizontally for a walker and along the full direction for a flier
/// (`crate::movement::flies`).
fn straight_step(from: Vec3, to: Vec3, speed: f32, dt: f32, flies: bool) -> MoveResult {
    let delta = if flies {
        to - from
    } else {
        Vec3::new(to.x - from.x, to.y - from.y, 0.0)
    };
    let length = delta.length();
    let step = speed * dt;
    if length <= f32::EPSILON || step <= 0.0 {
        return MoveResult {
            position: from,
            distance: 0.0,
            blocked: false,
            stepped_up: false,
        };
    }
    let travelled = step.min(length);
    MoveResult {
        position: from + delta / length * travelled,
        distance: travelled,
        blocked: false,
        stepped_up: false,
    }
}

/// Runs the tasks [`ScheduleRunner`] hands over.
struct MonsterExecutor<'a> {
    entity: Entity,
    actor: &'a mut Actor,
    ai: &'a mut MonsterAi,
    brain: &'a dyn Brain,
    collision: Option<&'a CollisionModel>,
    enemy_entity: Option<Entity>,
    enemy_position: Option<Vec3>,
    last_known: Option<Vec3>,
    sounds: &'a mut SoundList,
    events: &'a mut Vec<AiEvent>,
    dt: f32,
    /// The world's tick counter, which seeds a [`Task::Wander`] direction
    /// (see [`MonsterExecutor::wander`]).
    tick_count: u64,
}

impl MonsterExecutor<'_> {
    fn emit(&mut self, kind: AiEventKind) {
        if self.events.len() < MAX_EVENTS_PER_TICK {
            self.events.push(AiEvent {
                entity: self.entity,
                kind,
            });
        }
    }

    fn start_facing(&mut self, target: Option<Vec3>) -> TaskStatus {
        let Some(target) = target else {
            return TaskStatus::Failed;
        };
        let Some(yaw) = yaw_toward(self.actor.origin, target) else {
            return TaskStatus::Complete;
        };
        self.ai.ideal_yaw = yaw;
        self.turn()
    }

    fn turn(&mut self) -> TaskStatus {
        let (yaw, arrived) = turn_toward(self.actor.yaw, self.ai.ideal_yaw, TURN_RATE * self.dt);
        self.actor.yaw = yaw;
        let close = (movement::normalize_yaw(self.ai.ideal_yaw - yaw)).abs() <= FACING_TOLERANCE;
        if arrived || close {
            TaskStatus::Complete
        } else {
            TaskStatus::Running
        }
    }

    fn start_route(&mut self, goal: Option<Vec3>, within: f32) -> TaskStatus {
        let Some(goal) = goal else {
            return TaskStatus::Failed;
        };
        let from = self.actor.origin;
        let delta = Vec3::new(goal.x - from.x, goal.y - from.y, 0.0);
        let length = delta.length();
        let stop = if within > 0.0 && length > within {
            goal - delta / length * within
        } else {
            goal
        };
        self.ai.move_target = Some(stop);
        self.ai.route = Route::straight_line(stop);
        self.ai.stuck.reset();
        TaskStatus::Complete
    }

    fn find_cover(&mut self) -> TaskStatus {
        let Some(threat) = self
            .enemy_position
            .or(self.last_known)
            .or(self.ai.move_target)
        else {
            return TaskStatus::Failed;
        };
        let away = Vec3::new(
            self.actor.origin.x - threat.x,
            self.actor.origin.y - threat.y,
            0.0,
        );
        let direction = if away.length() > f32::EPSILON {
            away.normalize()
        } else {
            self.actor.forward()
        };
        let goal = self.actor.origin + direction * COVER_DISTANCE;
        let reachable = self.collision.map_or(goal, |model| {
            move_toward(
                model,
                self.actor.hull,
                self.actor.origin,
                goal,
                COVER_DISTANCE,
                1.0,
            )
            .position
        });
        self.ai.cover = Some(reachable);
        TaskStatus::Complete
    }

    /// Builds a route `distance` units away in a direction drawn from a
    /// generator seeded by this tick and this entity.
    ///
    /// Deterministic and replayable — the same tick and entity always
    /// wander the same way — without drawing from the world's shared
    /// stream, whose per-tick consumption every other monster's
    /// [`Task::WaitRandom`] outcome depends on: a critter that wanders
    /// must not reshuffle the waits of everything ticked after it.
    fn wander(&mut self, distance: f32) -> TaskStatus {
        if !distance.is_finite() || distance <= 0.0 {
            return TaskStatus::Failed;
        }
        let mut draw = Pcg32::with_stream(self.tick_count, u64::from(self.entity.id()));
        let yaw = draw.range_f32(-180.0, 180.0);
        let goal = self.actor.origin + movement::forward_from_yaw(yaw) * distance;
        self.ai.move_target = Some(goal);
        self.start_route(Some(goal), 0.0)
    }

    fn set_path_speed(&mut self, running: bool) -> TaskStatus {
        if self.ai.route.is_finished() {
            return TaskStatus::Failed;
        }
        let (walk, run) = self.brain.speeds();
        self.ai.move_speed = if running { run } else { walk };
        self.ai.stuck.reset();
        TaskStatus::Complete
    }
}

impl TaskExecutor for MonsterExecutor<'_> {
    #[allow(clippy::too_many_lines)]
    fn begin(&mut self, task: &Task) -> TaskStatus {
        match *task {
            Task::Wait(_) | Task::WaitRandom { .. } => TaskStatus::Complete,
            Task::WaitForMovement => {
                if self.ai.route.is_finished() {
                    TaskStatus::Complete
                } else if self.ai.stuck.is_stuck() {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Running
                }
            }
            Task::FaceEnemy => {
                let target = self.enemy_position.or(self.last_known);
                self.start_facing(target)
            }
            Task::FaceTarget => {
                let target = self.ai.move_target;
                self.start_facing(target)
            }
            Task::FaceLastKnownPosition => {
                let target = self.last_known.or(self.ai.move_target);
                self.start_facing(target)
            }
            Task::MoveToEnemy { within } => {
                let goal = self.enemy_position.or(self.last_known);
                self.start_route(goal, within)
            }
            Task::MoveToTarget { within } => {
                let goal = self.ai.move_target;
                self.start_route(goal, within)
            }
            Task::MoveToLastKnownPosition => {
                let goal = self.last_known;
                self.start_route(goal, 0.0)
            }
            Task::MoveToNode(_) => {
                // Package 7.6 supplies the node graph; until then this is
                // indistinguishable from having no route to build.
                TaskStatus::Failed
            }
            Task::RunPath => self.set_path_speed(true),
            Task::WalkPath => self.set_path_speed(false),
            Task::StopMoving => {
                self.ai.move_speed = 0.0;
                self.ai.route = Route::new();
                self.ai.stuck.reset();
                TaskStatus::Complete
            }
            Task::PlaySequence(name) => {
                self.emit(AiEventKind::PlaySequence(name));
                TaskStatus::Complete
            }
            Task::SetActivity(activity) => {
                if self.ai.activity != activity {
                    self.ai.activity = activity;
                    self.emit(AiEventKind::ActivityChanged(activity));
                }
                TaskStatus::Complete
            }
            Task::FindCover => self.find_cover(),
            Task::TakeCover => {
                let goal = self.ai.cover;
                self.start_route(goal, 0.0)
            }
            Task::MeleeAttack1 => self.attack(AttackKind::Melee1),
            Task::MeleeAttack2 => self.attack(AttackKind::Melee2),
            Task::RangeAttack1 => self.attack(AttackKind::Range1),
            Task::RangeAttack2 => self.attack(AttackKind::Range2),
            Task::Reload => {
                self.emit(AiEventKind::Reloaded);
                TaskStatus::Complete
            }
            Task::EmitSound(kind, radius) => {
                self.sounds
                    .push(SoundEvent::new(kind, self.actor.origin, radius).from(self.entity));
                self.emit(AiEventKind::SoundEmitted(kind));
                TaskStatus::Complete
            }
            Task::SetState(state) => {
                if self.ai.state != state {
                    let from = self.ai.state;
                    self.ai.state = state;
                    self.emit(AiEventKind::StateChanged { from, to: state });
                }
                TaskStatus::Complete
            }
            Task::ClearEnemy => {
                if self.ai.memory.take().is_some() {
                    self.emit(AiEventKind::EnemyLost);
                }
                TaskStatus::Complete
            }
            Task::Die => {
                self.actor.alive = false;
                self.actor.health = 0.0;
                self.ai.move_speed = 0.0;
                self.emit(AiEventKind::Died);
                TaskStatus::Complete
            }
            Task::Fail => TaskStatus::Failed,
            Task::Wander { distance } => self.wander(distance),
        }
    }

    fn resume(&mut self, task: &Task, _dt: f32) -> TaskStatus {
        match *task {
            Task::WaitForMovement => {
                if self.ai.route.is_finished() {
                    TaskStatus::Complete
                } else if self.ai.stuck.is_stuck() {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Running
                }
            }
            Task::FaceEnemy | Task::FaceTarget | Task::FaceLastKnownPosition => self.turn(),
            _ => TaskStatus::Complete,
        }
    }
}

impl MonsterExecutor<'_> {
    fn attack(&mut self, kind: AttackKind) -> TaskStatus {
        let target = self.enemy_entity;
        self.emit(AiEventKind::Attack { kind, target });
        TaskStatus::Complete
    }
}

/// Whether `schedule` holds a task that attacks — the only tasks that
/// emit [`AiEventKind::Attack`].
fn attacks(schedule: &Schedule) -> bool {
    schedule.tasks.iter().any(|task| {
        matches!(
            task,
            Task::MeleeAttack1 | Task::MeleeAttack2 | Task::RangeAttack1 | Task::RangeAttack2
        )
    })
}

/// Convenience: spawns a monster with the standard component set.
pub fn spawn_monster(world: &mut World, actor: Actor, brain: BrainId) -> Entity {
    world.spawn((actor, MonsterAi::new(brain)))
}

/// Convenience: spawns a squad member with the standard component set.
pub fn spawn_squad_monster(
    world: &mut World,
    actor: Actor,
    brain: BrainId,
    squad: SquadTag,
) -> Entity {
    world.spawn((actor, MonsterAi::new(brain), squad))
}

/// Convenience: an entity the AI can perceive but that has no AI of its own.
pub fn spawn_actor(world: &mut World, actor: Actor) -> Entity {
    world.spawn((actor,))
}

/// A [`Schedule`] the AI reports as running, resolved by name; used by save
/// restore, which stores schedule names rather than indices.
#[must_use]
pub fn resolve_schedule(name: &str) -> Option<&'static Schedule> {
    crate::brain::schedule_by_name(name)
}

#[cfg(test)]
mod tests {
    use super::{
        Actor, AiEventKind, AiWorld, MonsterAi, Prisoner, SquadTag, spawn_actor, spawn_monster,
        spawn_squad_monster,
    };
    use crate::brain::DefaultBrain;
    use crate::damage::DamageEvent;
    use crate::senses::{SightContext, SoundEvent, SoundKind};
    use crate::state::{Classification, Conditions, MonsterState};
    use glam::Vec3;
    use hecs::World;

    const DT: f32 = 0.01;

    fn setup() -> (AiWorld, World, super::BrainId) {
        let mut ai = AiWorld::new(0x5EED);
        let brain = ai.register_brain(Box::new(DefaultBrain::ranged(
            Classification::HumanMilitary,
        )));
        (ai, World::new(), brain)
    }

    #[test]
    fn seeing_a_hostile_flips_to_combat_in_one_tick() {
        let (mut ai, mut world, brain) = setup();
        let monster = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
        );
        spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(200.0, 0.0, 0.0)).as_client(),
        );

        let events = ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert_eq!(state.state, MonsterState::Combat);
        assert!(state.conditions.contains(Conditions::SEE_ENEMY));
        assert!(state.enemy().is_some());
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, AiEventKind::EnemyAcquired(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            AiEventKind::StateChanged {
                to: MonsterState::Combat,
                ..
            }
        )));
        assert_eq!(ai.tick_count(), 1);
    }

    #[test]
    fn damage_from_behind_provokes_and_acquires_the_attacker() {
        let (mut ai, mut world, brain) = setup();
        let monster = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
        );
        let sniper = spawn_actor(
            &mut world,
            Actor::new(Classification::AlienMilitary, Vec3::new(-4_000.0, 0.0, 0.0)),
        );
        ai.apply_damage(DamageEvent::new(
            monster,
            sniper,
            30.0,
            Vec3::new(-4_000.0, 0.0, 0.0),
        ));
        ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert!(state.conditions.contains(Conditions::HEAVY_DAMAGE));
        assert!(state.conditions.contains(Conditions::PROVOKED));
        assert_eq!(state.enemy(), Some(sniper));
    }

    #[test]
    fn a_dead_monster_stops_scheduling() {
        let (mut ai, mut world, brain) = setup();
        let monster = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(0.0),
            brain,
        );
        ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert_eq!(state.state, MonsterState::Dead);
        assert_eq!(state.schedule_name(), "ohl/inert");
    }

    #[test]
    fn a_danger_sound_is_heard_and_taken_cover_from() {
        let (mut ai, mut world, brain) = setup();
        let monster = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
        );
        assert!(ai.emit_sound(
            SoundEvent::new(SoundKind::Danger, Vec3::new(100.0, 0.0, 0.0), 400.0).lasting(1.0)
        ));
        ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert!(state.conditions.contains(Conditions::HEAR_DANGER));
        assert_eq!(state.schedule_name(), "ohl/take_cover_from_danger");
        assert_eq!(state.state, MonsterState::Alert);
    }

    #[test]
    fn an_entity_without_a_registered_brain_is_left_alone() {
        let (mut ai, mut world, _) = setup();
        let monster = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            super::BrainId(99),
        );
        ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert_eq!(state.state, MonsterState::Idle);
        assert_eq!(state.schedule_name(), "");
    }

    #[test]
    fn a_squad_shares_its_leaders_enemy() {
        let (mut ai, mut world, brain) = setup();
        let leader = spawn_squad_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
            SquadTag::leader("alpha"),
        );
        // Facing away, so it can only learn of the enemy from the leader.
        let follower = spawn_squad_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::new(0.0, 48.0, 0.0)).facing(180.0),
            brain,
            SquadTag::member("alpha"),
        );
        let player = spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(180.0, 0.0, 0.0)).as_client(),
        );

        ai.tick(&mut world, &SightContext::empty(), DT);
        assert_eq!(
            world.get::<&MonsterAi>(leader).expect("component").enemy(),
            Some(player)
        );
        // The follower adopts the shared enemy on the following tick.
        ai.tick(&mut world, &SightContext::empty(), DT);
        assert_eq!(
            world
                .get::<&MonsterAi>(follower)
                .expect("component")
                .enemy(),
            Some(player)
        );
        assert!(ai.squads().is_leader(leader));
    }

    /// The published `Prisoner` spawnflag, through the whole tick: the
    /// exact scene `seeing_a_hostile_flips_to_combat_in_one_tick` builds,
    /// with the monster marked a prisoner, sees the player and does
    /// nothing about it — no enemy, no combat state, no attack — for as
    /// long as the player stands there, and not even when hurt by them.
    #[test]
    fn a_prisoner_never_acquires_an_enemy_by_sight_or_damage() {
        let (mut ai, mut world, brain) = setup();
        let prisoner = world.spawn((
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            MonsterAi::new(brain),
            Prisoner,
        ));
        let player = spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(200.0, 0.0, 0.0)).as_client(),
        );

        let mut events = Vec::new();
        for _ in 0..200 {
            events.extend(ai.tick(&mut world, &SightContext::empty(), DT));
        }
        {
            let state = world.get::<&MonsterAi>(prisoner).expect("component");
            assert!(state.conditions.contains(Conditions::SEE_CLIENT), "seen");
            assert!(!state.conditions.contains(Conditions::SEE_ENEMY));
            assert_ne!(state.state, MonsterState::Combat);
            assert!(state.enemy().is_none());
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, AiEventKind::Attack { .. })),
            "a prisoner never attacks"
        );

        // Being hurt is not an exception: neither published sentence names
        // one, and "normal AI is disabled" is what a damage-provoked
        // enemy would undo. The hurt still registers as a condition.
        ai.apply_damage(DamageEvent::new(
            prisoner,
            player,
            30.0,
            Vec3::new(200.0, 0.0, 0.0),
        ));
        ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(prisoner).expect("component");
        assert!(state.conditions.contains(Conditions::HEAVY_DAMAGE));
        assert!(state.enemy().is_none(), "hurt, and still no enemy");
    }

    /// A prisoner restored from a save made mid-fight — before the flag
    /// was modelled — arrives remembering the player as its enemy, in the
    /// combat state, part-way into an attack schedule. None of that may
    /// survive its first tick: the memory is dropped (and reported lost),
    /// the attack schedule is abandoned before its attack task can fire
    /// straight ahead, and it never fights. The same restored state on an
    /// unflagged monster is the control: it shoots.
    #[test]
    fn a_prisoner_restored_mid_fight_forgets_its_enemy_and_stops_attacking() {
        use crate::brain::RANGE_ATTACK;
        use crate::schedule::{ScheduleRunner, Task};
        use crate::senses::EnemyMemory;

        let restored = |prisoner: bool| {
            let (mut ai, mut world, brain) = setup();
            let player = spawn_actor(
                &mut world,
                Actor::new(Classification::Player, Vec3::new(200.0, 0.0, 0.0)).as_client(),
            );
            let mut state = MonsterAi::new(brain);
            state.state = MonsterState::Combat;
            state.memory = Some(EnemyMemory {
                entity: player,
                last_known_position: Vec3::new(200.0, 0.0, 0.0),
                time_since_seen: 0.0,
                occluded: false,
                last_known_distance: 200.0,
            });
            // Restored the way `ohl-engine`'s save path restores it: by
            // name and task index, here sitting on the attack task itself,
            // past the face-the-enemy step that would otherwise fail
            // without an enemy and abandon the schedule first.
            let attack_task = RANGE_ATTACK
                .tasks
                .iter()
                .position(|task| matches!(task, Task::RangeAttack1))
                .expect("the schedule attacks");
            state.runner = ScheduleRunner::restore(RANGE_ATTACK.name, attack_task, false, 0.0);
            let monster =
                world.spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO), state));
            if prisoner {
                world.insert_one(monster, Prisoner).expect("spawned");
            }
            let mut events = Vec::new();
            for _ in 0..100 {
                events.extend(ai.tick(&mut world, &SightContext::empty(), DT));
            }
            (world, monster, events)
        };
        let attacked = |events: &[super::AiEvent]| {
            events
                .iter()
                .any(|event| matches!(event.kind, AiEventKind::Attack { .. }))
        };

        let (_, _, events) = restored(false);
        assert!(attacked(&events), "the control: the restored fight goes on");

        let (world, monster, events) = restored(true);
        assert!(!attacked(&events), "a restored prisoner never fires");
        assert!(
            events
                .iter()
                .any(|event| event.entity == monster && event.kind == AiEventKind::EnemyLost),
            "the remembered enemy is reported lost"
        );
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert!(state.enemy().is_none(), "the remembered enemy is gone");
        assert_ne!(state.state, MonsterState::Combat);
        assert_ne!(state.runner.schedule_name(), RANGE_ATTACK.name);
    }

    /// The other half of the same sentence: a prisoner is not attacked
    /// either. A hostile monster with a prisoner in plain view in front of
    /// it — the one thing in the world it could fight — stays out of
    /// combat.
    #[test]
    fn a_prisoner_is_not_chosen_as_an_enemy_by_a_hostile() {
        let (mut ai, mut world, brain) = setup();
        let hunter = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
        );
        world.spawn((
            Actor::new(Classification::AlienMilitary, Vec3::new(200.0, 0.0, 0.0)),
            MonsterAi::new(brain),
            Prisoner,
        ));
        let events = ai.tick(&mut world, &SightContext::empty(), DT);
        let state = world.get::<&MonsterAi>(hunter).expect("component");
        assert!(!state.conditions.contains(Conditions::SEE_ENEMY));
        assert_ne!(state.state, MonsterState::Combat);
        assert!(state.enemy().is_none());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, AiEventKind::EnemyAcquired(_)))
        );
    }

    /// A squad mate's shared enemy is the third way in, and it is shut
    /// too: the leader fights, the prisoner member beside it does not.
    #[test]
    fn a_prisoner_squad_member_does_not_take_the_leaders_enemy() {
        let (mut ai, mut world, brain) = setup();
        let leader = spawn_squad_monster(
            &mut world,
            Actor::new(Classification::HumanMilitary, Vec3::ZERO),
            brain,
            SquadTag::leader("alpha"),
        );
        let captive = world.spawn((
            Actor::new(Classification::HumanMilitary, Vec3::new(0.0, 48.0, 0.0)).facing(180.0),
            MonsterAi::new(brain),
            SquadTag::member("alpha"),
            Prisoner,
        ));
        let player = spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(180.0, 0.0, 0.0)).as_client(),
        );
        for _ in 0..3 {
            ai.tick(&mut world, &SightContext::empty(), DT);
        }
        assert_eq!(
            world.get::<&MonsterAi>(leader).expect("component").enemy(),
            Some(player)
        );
        assert!(
            world
                .get::<&MonsterAi>(captive)
                .expect("component")
                .enemy()
                .is_none()
        );
    }

    #[test]
    fn a_thousand_ticks_replay_identically() {
        let build = || {
            let (ai, mut world, brain) = setup();
            spawn_monster(
                &mut world,
                Actor::new(Classification::HumanMilitary, Vec3::new(-300.0, 0.0, 0.0)),
                brain,
            );
            spawn_monster(
                &mut world,
                Actor::new(Classification::AlienMilitary, Vec3::new(300.0, 0.0, 0.0)).facing(180.0),
                brain,
            );
            spawn_actor(
                &mut world,
                Actor::new(Classification::Player, Vec3::new(0.0, 600.0, 0.0)).as_client(),
            );
            (ai, world)
        };

        let run = || {
            let (mut ai, mut world) = build();
            for tick in 0..1_000 {
                if tick % 137 == 0 {
                    ai.emit_sound(SoundEvent::new(
                        SoundKind::Combat,
                        Vec3::new(0.0, 0.0, 0.0),
                        512.0,
                    ));
                }
                ai.tick(&mut world, &SightContext::empty(), DT);
            }
            ai.state_hash(&world)
        };

        assert_eq!(run(), run());
    }

    #[test]
    fn different_seeds_diverge_over_a_long_run() {
        let run = |seed| {
            let mut ai = AiWorld::new(seed);
            let brain =
                ai.register_brain(Box::new(DefaultBrain::melee(Classification::AlienMonster)));
            let mut world = World::new();
            spawn_monster(
                &mut world,
                Actor::new(Classification::AlienMonster, Vec3::ZERO),
                brain,
            );
            for _ in 0..500 {
                ai.tick(&mut world, &SightContext::empty(), DT);
            }
            ai.state_hash(&world)
        };
        assert_ne!(run(1), run(2));
    }

    /// Wave 1 batch A: the no-collision fallback (`straight_step`) honours
    /// the hull the same way `crate::movement::move_toward` does. Chasing
    /// an enemy above it with no collision model at all, a point-hull mover
    /// climbs along the full line and a standing-hull mover closes at its
    /// own height.
    #[test]
    fn the_no_collision_fallback_flies_a_point_hull_and_walks_a_box_hull() {
        for (hull, flies) in [
            (ohl_physics::Hull::Point, true),
            (ohl_physics::Hull::Standing, false),
        ] {
            let mut ai = AiWorld::new(0x5EED);
            let brain =
                ai.register_brain(Box::new(DefaultBrain::melee(Classification::AlienMilitary)));
            let mut world = World::new();
            let mut actor = Actor::new(Classification::AlienMilitary, Vec3::ZERO);
            actor.hull = hull;
            let monster = spawn_monster(&mut world, actor, brain);
            spawn_actor(
                &mut world,
                Actor::new(Classification::Player, Vec3::new(300.0, 0.0, 200.0)).as_client(),
            );
            for _ in 0..100 {
                ai.tick(&mut world, &SightContext::empty(), DT);
            }
            let origin = world.get::<&Actor>(monster).expect("actor").origin;
            assert!(origin.x > 16.0, "{hull:?} closed on its enemy: {origin:?}");
            if flies {
                assert!(origin.z > 16.0, "a flier climbs: {origin:?}");
            } else {
                assert!(
                    origin.z.abs() < 1e-3,
                    "a walker keeps its height: {origin:?}"
                );
            }
        }
    }

    /// Only ever wanders: one leg, then a fixed (not random) pause.
    static WANDER_ONLY: crate::schedule::Schedule = crate::schedule::Schedule::new(
        "test/wander_only",
        &[
            crate::schedule::Task::Wander { distance: 64.0 },
            crate::schedule::Task::WalkPath,
            crate::schedule::Task::WaitForMovement,
            crate::schedule::Task::Wait(0.1),
        ],
        Conditions::EMPTY,
    );

    struct Wanderer;

    impl crate::schedule::Brain for Wanderer {
        fn classification(&self) -> Classification {
            Classification::None
        }

        fn select_schedule(
            &self,
            _state: MonsterState,
            _conditions: Conditions,
        ) -> &'static crate::schedule::Schedule {
            &WANDER_ONLY
        }

        /// A walk fast enough at [`DT`] that the stuck check never gives a
        /// leg up, so every spell reaches its closing wait.
        fn speeds(&self) -> (f32, f32) {
            (100.0, 200.0)
        }
    }

    /// Wave 1 batch A: a [`crate::schedule::Task::Wander`] direction is
    /// drawn from a generator seeded by the tick and the entity, never from
    /// the world's shared stream. Two wanderers started on the same spot in
    /// the same tick go different ways, each one turns between legs, and
    /// after many complete legs the shared stream is exactly where a fresh
    /// world's is — nothing (no wait here is random) has drawn from it.
    #[test]
    fn a_wander_is_drawn_per_tick_and_entity_and_leaves_the_shared_stream_alone() {
        const SEED: u64 = 0x5EED;
        let mut ai = AiWorld::new(SEED);
        let brain = ai.register_brain(Box::new(Wanderer));
        let mut world = World::new();
        let wanderers = [
            spawn_monster(
                &mut world,
                Actor::new(Classification::None, Vec3::ZERO),
                brain,
            ),
            spawn_monster(
                &mut world,
                Actor::new(Classification::None, Vec3::ZERO),
                brain,
            ),
        ];
        let mut walked = [0.0_f32; 2];
        let mut previous = [Vec3::ZERO; 2];
        let mut spells = 0;
        for _ in 0..1_000 {
            for event in ai.tick(&mut world, &SightContext::empty(), DT) {
                if let AiEventKind::ScheduleEnded { outcome, .. } = event.kind {
                    assert_eq!(outcome, crate::schedule::RunOutcome::Done);
                    spells += 1;
                }
            }
            for (index, entity) in wanderers.iter().enumerate() {
                let origin = world.get::<&Actor>(*entity).expect("actor").origin;
                walked[index] += (origin - previous[index]).length();
                previous[index] = origin;
            }
        }
        assert!(
            previous[0].distance(previous[1]) > 1.0,
            "same spot, same tick, different entity: different ways {previous:?}"
        );
        assert!(spells >= 4, "every spell ran to its closing wait: {spells}");
        for (index, end) in previous.iter().enumerate() {
            assert!(walked[index] > 2.0 * 64.0, "it wandered more than one leg");
            assert!(
                end.length() + 1.0 < walked[index],
                "its legs did not all point the same way: {end:?} after {}",
                walked[index]
            );
        }
        assert_eq!(
            ai.rng_snapshot(),
            AiWorld::new(SEED).rng_snapshot(),
            "a wander drew from the world's shared stream"
        );
    }
}
