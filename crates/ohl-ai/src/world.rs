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
use crate::monsters::{Fallback, NavBridge};
use crate::movement::{
    self, MoveResult, Route, StuckDetector, forward_from_yaw, move_toward, normalize_yaw,
    turn_toward, yaw_toward,
};
use crate::rng::Pcg32;
use crate::schedule::{
    Activity, Brain, RunOutcome, Schedule, ScheduleRunner, Task, TaskExecutor, TaskStatus,
};
use crate::senses::{
    Candidate, EnemyMemory, SightContext, Sighting, SoundEvent, SoundKind, SoundList, Viewer,
    listen_with_danger_bounds, look, select_enemy,
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

/// The shortest [`Task::Wander`] leg worth walking once its goal has been
/// clamped to what is reachable, in world units: two arrival tolerances
/// ([`movement::WAYPOINT_TOLERANCE`]), so a leg that would count as
/// "arrived" almost where it started is skipped instead. A project choice.
pub const MIN_WANDER_LEG: f32 = 2.0 * movement::WAYPOINT_TOLERANCE;

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
    /// Authored model anchor for monsters; controller center for clients.
    pub origin: Vec3,
    /// Model-local eye offset for monsters; world offset for clients.
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
    /// Derived collision frame; never written into an existing save record.
    pub body_frame: crate::BodyFrame,
}

impl Actor {
    /// A living, standing-hull actor at `origin`.
    #[must_use]
    pub fn new(classification: Classification, origin: Vec3) -> Self {
        Self {
            classification,
            origin,
            view_ofs: crate::BodyFrame::Feet.eye_offset(Hull::Standing, None),
            yaw: 0.0,
            health: 100.0,
            alive: true,
            is_client: false,
            hull: Hull::Standing,
            body_frame: crate::BodyFrame::Feet,
        }
    }

    /// The same actor marked as the player.
    #[must_use]
    pub fn as_client(mut self) -> Self {
        self.is_client = true;
        self.classification = Classification::Player;
        self.body_frame = crate::BodyFrame::Centered;
        self.view_ofs = Vec3::new(0.0, 0.0, 28.0);
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
        if self.is_client {
            return self.origin + self.view_ofs;
        }
        let (sin, cos) = self.yaw.to_radians().sin_cos();
        self.origin
            + Vec3::new(
                cos * self.view_ofs.x - sin * self.view_ofs.y,
                sin * self.view_ofs.x + cos * self.view_ofs.y,
                self.view_ofs.z,
            )
    }

    /// Centered query point for this actor's selected compiled BSP hull.
    #[must_use]
    pub fn query_origin(&self) -> Vec3 {
        self.body_frame.anchor_to_query(self.hull, self.origin)
    }

    /// Last-resort anchor-relative, axis-aligned damage box, when no usable posed
    /// or clipping bounds exist. A point hull still needs nonzero damage geometry.
    #[must_use]
    pub fn fallback_damage_bounds(&self) -> (Vec3, Vec3) {
        if self.hull == Hull::Point {
            (Vec3::splat(-24.0), Vec3::splat(24.0))
        } else {
            self.body_frame.local_bounds(self.hull)
        }
    }

    /// The navigation target supplied to another actor. Player stance
    /// changes keep the same floor anchor; monster model anchors stay exact.
    #[must_use]
    pub fn navigation_anchor(&self) -> Vec3 {
        if self.is_client {
            self.origin - Vec3::Z * self.hull.foot_offset()
        } else {
            self.origin
        }
    }

    /// Reconstructs model-dependent body and eye policy without moving it.
    pub fn configure_model(
        &mut self,
        kind: &crate::MonsterKind,
        model: Option<&ohl_world::StudioModel>,
    ) {
        if self.is_client {
            return;
        }
        self.body_frame = crate::BodyFrame::for_model(kind, self.hull, model);
        self.view_ofs = self.body_frame.eye_offset(self.hull, model);
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

/// Marks an actor no damage can hurt: in this project, a `monster_generic`
/// spawned with its published `Not solid` spawnflag, which the cited page
/// calls "impervious to any damage" (`docs/FORMAT_SOURCES.md`, "Monster
/// definitions"; the engine drops every hit queued at it).
///
/// Sight still sees it, and it still sees, hears and is scripted like any
/// other monster; but no looker ever reads it as an enemy
/// ([`crate::senses::look`] reads it through the same one hostility
/// rule a prisoner goes through, [`crate::senses::sighting_relationship`]). An enemy that
/// cannot be hurt is one a monster would fire at forever: a soldier that
/// saw such a prop nearer than the player — both hated alike — chose the
/// prop and ignored the player for as long as the prop stood there.
///
/// Derived from the entity definition's own spawnflags at spawn and never
/// changed afterwards, so it needs no save-file field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Impervious;

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
    /// The route in absolute query coordinates (also the tag-25 wire domain).
    pub route: Route,
    /// Absolute query/world destination; sounds may overwrite it independently of the route.
    pub move_target: Option<Vec3>,
    /// Absolute query position [`Task::FindCover`] chose; saved without translation.
    pub cover: Option<Vec3>,
    /// Fresh follow authority, consumed even while scripts or death own the actor.
    pub follow_input: crate::follow::FollowInput,
    /// Accepted historical intent; persisted separately from the frozen AI payload.
    pub follow_attempt: Option<crate::follow::FollowAttempt>,
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
    /// The named map entity should be fired (a Gonarch trail node's
    /// `reachtarget`; see `crate::monsters::bigmomma`). Dispatching a fire
    /// by name into the map logic is the host's job, exactly as for
    /// `crate::monsters::MonsterTrigger`.
    FireTarget(String),
    /// The named map entity should be removed (a trail node's
    /// `killtarget`).
    KillTarget(String),
    /// The named `scripted_sequence` should run on this monster (a trail
    /// node's `reachsequence`).
    ScriptRequested(String),
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
    script_navigation: crate::NavigationStats,
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
            script_navigation: crate::NavigationStats::default(),
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

    /// Aggregate script navigation counters for development inspection.
    /// They never contain entity names, positions or asset information.
    #[must_use]
    pub fn script_navigation_stats(&self) -> crate::NavigationStats {
        self.script_navigation
    }

    /// Drops transient navigation state when restoring a live world.
    pub fn invalidate_navigation(&mut self) {
        if let Some(navigator) = self.navigator.as_mut() {
            navigator.invalidate();
        }
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

    /// Adds a sound or scent for the next tick's [`crate::senses::listen`] pass.
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
            // A boss or aircraft component (a Gonarch's trail, a
            // Nihilanth's shield, an aircraft's flight plan) is driven just
            // before the ordinary step, through the same route/condition
            // handles a script or a follower uses; a monster carrying none
            // is untouched. See `crate::monsters::bosses`.
            let speeds = world
                .get::<&MonsterAi>(entity)
                .ok()
                .and_then(|ai| self.brains.get(ai.brain.0).map(|brain| brain.speeds()))
                .unwrap_or((40.0, 160.0));
            crate::monsters::bosses::pre_think(world, entity, dt, speeds, &mut events);
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
        let Ok(mut ai) = world
            .get::<&mut MonsterAi>(entity)
            .map(|mut ai| core::mem::take(&mut *ai))
        else {
            return;
        };
        let follow_input = core::mem::take(&mut ai.follow_input);
        let Ok(mut actor) = world.get::<&Actor>(entity).map(|actor| *actor) else {
            if let Ok(mut slot) = world.get::<&mut MonsterAi>(entity) {
                *slot = ai;
            }
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
        let walking_attachment = permits_ground_attachment(world, entity, &actor);

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
            origin: actor.navigation_anchor(),
            view_ofs: actor.eye() - actor.navigation_anchor(),
            forward: actor.forward(),
            classification: actor.classification,
            prisoner,
        };
        let sight = look(&viewer, &senses, candidates, &self.relationships, context);
        conditions |= sight.conditions;
        let (min, max) = actor.fallback_damage_bounds();
        let heard = listen_with_danger_bounds(
            viewer.eye(),
            &senses,
            &self.sounds,
            Some((actor.origin + min, actor.origin + max)),
        );
        conditions |= heard.conditions;
        if let Some(sound) = heard.best {
            ai.move_target = Some(sound.position);
        }

        // --- Enemy acquisition and memory ---------------------------------
        // A monster that can only attack one place takes what is there as
        // its enemy ahead of sight's own choice (`Brain::
        // chooses_enemy_in_reach`).
        let enemy = if brain.has_melee_attack() && brain.chooses_enemy_in_reach() {
            let in_reach: Vec<Sighting> = sight
                .visible
                .iter()
                .filter(|seen| brain.melee_in_reach(actor.origin, seen.origin, seen.distance))
                .copied()
                .collect();
            select_enemy(&in_reach).or(sight.enemy)
        } else {
            sight.enemy
        };
        if let Some(seen) = enemy {
            let is_new = ai.memory.is_none_or(|memory| memory.entity != seen.entity);
            if is_new {
                conditions |= Conditions::NEW_ENEMY;
                let mut memory = EnemyMemory::seen(&seen);
                memory.last_known_position =
                    actor.body_frame.anchor_to_query(actor.hull, seen.origin);
                ai.memory = Some(memory);
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::EnemyAcquired(seen.entity),
                });
            } else if let Some(memory) = ai.memory.as_mut() {
                memory.refresh(&seen);
                memory.last_known_position =
                    actor.body_frame.anchor_to_query(actor.hull, seen.origin);
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
                    last_known_position: actor.body_frame.anchor_to_query(actor.hull, position),
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
                let known_anchor = by_entity.get(&attacker).map(|candidate| candidate.origin);
                let remembered = known_anchor.map_or(position, |anchor| {
                    actor.body_frame.anchor_to_query(actor.hull, anchor)
                });
                // Unknown damage positions are literal world points, including on wire.
                let distance_from = known_anchor.unwrap_or(position);
                ai.memory = Some(EnemyMemory {
                    entity: attacker,
                    last_known_position: remembered,
                    time_since_seen: 0.0,
                    occluded: true,
                    last_known_distance: (distance_from - actor.origin).length(),
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
            ai.follow_attempt = None;
            ai.runner.clear();
            let step = ai.move_speed * dt;
            let before = self
                .navigator
                .as_ref()
                .map(NavBridge::stats)
                .unwrap_or_default();
            if self.navigator.is_none() && step > 0.0 && !ai.route.is_finished() {
                if let Some(collision) = context.collision {
                    self.script_navigation.traced_steps =
                        self.script_navigation.traced_steps.saturating_add(1);
                    if collision
                        .trace(actor.hull, actor.query_origin(), actor.query_origin())
                        .start_solid
                    {
                        self.script_navigation.start_solid =
                            self.script_navigation.start_solid.saturating_add(1);
                    }
                } else {
                    self.script_navigation.untraced_steps =
                        self.script_navigation.untraced_steps.saturating_add(1);
                }
            }
            let moved = advance_route(
                entity,
                &mut actor,
                &mut ai,
                context.collision,
                self.navigator.as_mut(),
                Fallback::StraightLine,
                walking_attachment,
                dt,
            );
            if let Some(navigator) = self.navigator.as_ref() {
                self.script_navigation.add_delta(before, navigator.stats());
            }
            if ai.move_speed > 0.0 {
                if ai.stuck.record_step(moved, step) {
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
            .and_then(|memory| by_entity.get(&memory.entity))
            .map(|candidate| {
                actor
                    .body_frame
                    .anchor_to_query(actor.hull, candidate.origin)
            });
        let enemy_entity = ai.memory.map(|memory| memory.entity);
        let last_known = ai.last_known_position();

        let mut runner = core::mem::take(&mut ai.runner);
        // Project-authored current-intent ownership; TODO(black-box).
        if (!actor.alive
            || actor.health <= 0.0
            || matches!(follow_input, crate::follow::FollowInput::Inactive))
            && ai.follow_attempt.take().is_some()
            && runner.schedule().is_some_and(is_follow_schedule)
        {
            runner.clear();
            retire_follow_motion(&mut ai, &mut conditions);
        }
        if runner.is_running() && !runner.schedule().is_some_and(is_follow_schedule) {
            ai.follow_attempt = None;
        }
        let mut replaced_follow = false;
        if runner.schedule().is_some_and(is_follow_schedule) {
            let selected = brain.select_schedule(ai.state, conditions);
            if admit_follow_intent(
                entity,
                &actor,
                &mut ai,
                follow_input,
                selected,
                &mut conditions,
                dt,
                false,
                self.navigator.as_mut(),
            ) {
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::ScheduleEnded {
                        name: runner.schedule_name(),
                        outcome: RunOutcome::Interrupted,
                    },
                });
                runner.start(&crate::monsters::brains::FOLLOW_PLAYER);
                events.push(AiEvent {
                    entity,
                    kind: AiEventKind::ScheduleStarted(runner.schedule_name()),
                });
                replaced_follow = true;
            }
        }
        if !runner.is_running() {
            let schedule = brain.select_schedule(ai.state, conditions);
            admit_follow_intent(
                entity,
                &actor,
                &mut ai,
                follow_input,
                schedule,
                &mut conditions,
                dt,
                false,
                self.navigator.as_mut(),
            );
            prepare_danger_cover_recovery(
                entity,
                &actor,
                &mut ai,
                schedule,
                &mut conditions,
                dt,
                self.navigator.as_mut(),
            );
            if !is_follow_schedule(schedule) {
                ai.follow_attempt = None;
            }
            runner.start(schedule);
            events.push(AiEvent {
                entity,
                kind: AiEventKind::ScheduleStarted(schedule.name),
            });
        }
        let running_name = runner.schedule_name();
        // TODO(black-box): project-authored danger cover uses the current qualified sound.
        // The local runner owns the schedule; ai.runner is temporarily empty.
        let danger_cover = runner.schedule().is_some_and(|schedule| {
            std::ptr::eq(schedule, &raw const crate::brain::TAKE_COVER_FROM_DANGER)
        });
        let cover_threat = if danger_cover {
            heard
                .best
                .filter(|sound| sound.kind == SoundKind::Danger)
                .map(|sound| sound.position)
        } else {
            enemy_position.or(last_known).or(ai.move_target)
        };

        let following = runner.schedule().is_some_and(is_follow_schedule);
        let paused_follow = following
            && ai.follow_attempt.is_some()
            && (!dt.is_finite()
                || dt <= 0.0
                || !matches!(follow_input, crate::follow::FollowInput::Target(goal) if goal.is_finite()));
        let outcome = if replaced_follow
            || (paused_follow
                && !runner
                    .schedule()
                    .is_some_and(|s| s.is_interrupted_by(conditions)))
        {
            RunOutcome::Running
        } else {
            let mut executor = MonsterExecutor {
                entity,
                actor: &mut actor,
                ai: &mut ai,
                brain: brain.as_ref(),
                collision: context.collision,
                enemy_entity,
                enemy_position,
                last_known,
                cover_threat,
                danger_cover,
                following,
                follow_input,
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
            let mut post = conditions | outcome.condition();
            let schedule = brain.select_schedule(ai.state, post);
            if admit_follow_intent(
                entity,
                &actor,
                &mut ai,
                follow_input,
                schedule,
                &mut post,
                dt,
                outcome == RunOutcome::Done,
                self.navigator.as_mut(),
            ) {
                conditions.remove(Conditions::BLOCKED);
            }
            if prepare_danger_cover_recovery(
                entity,
                &actor,
                &mut ai,
                schedule,
                &mut post,
                dt,
                self.navigator.as_mut(),
            ) {
                conditions.remove(Conditions::BLOCKED);
            }
            if !is_follow_schedule(schedule) {
                ai.follow_attempt = None;
            }
            runner.start(schedule);
            events.push(AiEvent {
                entity,
                kind: AiEventKind::ScheduleStarted(schedule.name),
            });
        }
        ai.runner = runner;

        // --- Movement -----------------------------------------------------
        let owned_follow = ai.follow_attempt.is_some();
        let pause_motion = replaced_follow || (owned_follow && paused_follow);
        let step = ai.move_speed * dt;
        let attempted = step.is_finite() && step > 0.0 && !ai.route.is_finished();
        if !pause_motion {
            let moved = advance_route(
                entity,
                &mut actor,
                &mut ai,
                context.collision,
                self.navigator.as_mut(),
                Fallback::Traced,
                walking_attachment,
                dt,
            );
            if owned_follow {
                if attempted && ai.route.is_finished() {
                    if let Some(attempt) = &mut ai.follow_attempt {
                        attempt.phase = crate::follow::FollowPhase::Arrived;
                    }
                    ai.stuck.reset();
                    ai.pending_conditions.remove(Conditions::BLOCKED);
                    conditions.remove(Conditions::BLOCKED);
                } else if attempted && ai.stuck.record_step(moved, step) {
                    ai.pending_conditions |= Conditions::BLOCKED;
                }
            } else if ai.move_speed > 0.0 {
                if ai.stuck.record_step(moved, step) {
                    ai.pending_conditions |= Conditions::BLOCKED;
                }
            } else {
                ai.stuck.reset();
            }
        }
        ai.conditions = conditions;

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
    let (tag, bottom) = match actor.body_frame {
        crate::BodyFrame::Feet => (0, 0.0_f32),
        crate::BodyFrame::Centered => (1, 0.0),
        crate::BodyFrame::Ceiling => (2, 0.0),
        crate::BodyFrame::ModelBottom(bottom) => (3, bottom),
        crate::BodyFrame::FixedModelAnchor(None) => (4, 0.0),
        crate::BodyFrame::FixedModelAnchor(Some(bottom)) => (5, bottom),
    };
    bytes.push(tag);
    bytes.extend_from_slice(&bottom.to_bits().to_le_bytes());
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
    match ai.follow_attempt {
        None => bytes.push(0),
        Some(attempt) => {
            bytes.push(1);
            bytes.push(attempt.phase as u8);
            for value in attempt.accepted_player_anchor.to_array() {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
        }
    }
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

fn is_follow_schedule(schedule: &Schedule) -> bool {
    std::ptr::eq(schedule, &raw const crate::monsters::brains::FOLLOW_PLAYER)
}

// TODO(black-box): a newly selected danger response owns a fresh escape attempt.
// Physical failure of the retired route must not interrupt its setup task.
#[allow(clippy::too_many_arguments)]
fn prepare_danger_cover_recovery(
    entity: Entity,
    actor: &Actor,
    ai: &mut MonsterAi,
    schedule: &Schedule,
    conditions: &mut Conditions,
    dt: f32,
    navigator: Option<&mut NavBridge>,
) -> bool {
    if !actor.alive
        || !actor.health.is_finite()
        || actor.health <= 0.0
        || !dt.is_finite()
        || dt <= 0.0
        || !conditions.contains(Conditions::HEAR_DANGER)
        || !conditions.contains(Conditions::BLOCKED)
        || !std::ptr::eq(schedule, &raw const crate::brain::TAKE_COVER_FROM_DANGER)
    {
        return false;
    }
    // This existing operation retires only route/speed/physical stuck/BLOCKED;
    // it does not alter roster, enemy, sound, damage, or other interrupt state.
    retire_follow_motion(ai, conditions);
    if let Some(navigator) = navigator {
        navigator.invalidate_actor(entity);
    }
    true
}

fn retire_follow_motion(ai: &mut MonsterAi, conditions: &mut Conditions) {
    ai.route = Route::new();
    ai.move_speed = 0.0;
    ai.stuck.reset();
    conditions.remove(Conditions::BLOCKED);
    ai.conditions.remove(Conditions::BLOCKED);
    ai.pending_conditions.remove(Conditions::BLOCKED);
}

// Project-authored admission transaction; TODO(black-box). Caller establishes
// an actual follow runner or ordinary selection boundary, never a speculative takeover.
#[allow(clippy::too_many_arguments)]
fn admit_follow_intent(
    entity: Entity,
    actor: &Actor,
    ai: &mut MonsterAi,
    input: crate::follow::FollowInput,
    selected: &Schedule,
    conditions: &mut Conditions,
    dt: f32,
    completed: bool,
    navigator: Option<&mut NavBridge>,
) -> bool {
    use crate::follow::{FOLLOW_DISTANCE, FollowAttempt, FollowInput, FollowPhase};
    let FollowInput::Target(goal) = input else {
        return false;
    };
    let mut competing = *conditions;
    competing.remove(Conditions::BLOCKED);
    if !actor.alive
        || actor.health <= 0.0
        || !actor.health.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
        || !goal.is_finite()
        || !is_follow_schedule(selected)
        || selected.is_interrupted_by(competing)
    {
        return false;
    }
    let query = actor.body_frame.anchor_to_query(actor.hull, goal);
    let delta = query - actor.query_origin();
    let distance = Vec3::new(delta.x, delta.y, 0.0).length();
    if !query.is_finite() || !delta.is_finite() || !distance.is_finite() {
        return false;
    }
    let holding = match ai.follow_attempt {
        None => distance <= FOLLOW_DISTANCE,
        Some(attempt) => {
            let drift = (goal - attempt.accepted_player_anchor).length();
            if !drift.is_finite() {
                return false;
            }
            match attempt.phase {
                FollowPhase::Holding => {
                    if distance <= FOLLOW_DISTANCE + movement::WAYPOINT_TOLERANCE {
                        return false;
                    }
                    false
                }
                FollowPhase::Moving if distance <= FOLLOW_DISTANCE => true,
                FollowPhase::Arrived
                    if distance <= FOLLOW_DISTANCE + movement::WAYPOINT_TOLERANCE =>
                {
                    true
                }
                _ => {
                    if drift <= movement::ROUTE_REFRESH_DISTANCE
                        && !(completed && attempt.phase == FollowPhase::Arrived)
                    {
                        return false;
                    }
                    distance <= FOLLOW_DISTANCE
                }
            }
        }
    };
    retire_follow_motion(ai, conditions);
    if let Some(navigator) = navigator {
        navigator.invalidate_actor(entity);
    }
    ai.follow_attempt = Some(FollowAttempt {
        accepted_player_anchor: goal,
        phase: if holding {
            FollowPhase::Holding
        } else {
            FollowPhase::Preparing
        },
    });
    true
}

fn snapshot_candidates(world: &World) -> Vec<Candidate> {
    let mut candidates: Vec<(u32, Candidate)> = world
        .query::<(Entity, &Actor, Option<&Prisoner>, Option<&Impervious>)>()
        .iter()
        .map(|(entity, actor, prisoner, impervious)| {
            (
                entity.id(),
                Candidate {
                    entity,
                    classification: actor.classification,
                    origin: actor.navigation_anchor(),
                    view_ofs: actor.eye() - actor.navigation_anchor(),
                    forward: actor.forward(),
                    alive: actor.alive,
                    is_client: actor.is_client,
                    prisoner: prisoner.is_some(),
                    impervious: impervious.is_some(),
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
#[allow(clippy::too_many_arguments)]
fn advance_route(
    entity: Entity,
    actor: &mut Actor,
    ai: &mut MonsterAi,
    collision: Option<&CollisionModel>,
    navigator: Option<&mut NavBridge>,
    fallback: Fallback,
    walking_attachment: bool,
    dt: f32,
) -> f32 {
    if ai.move_speed <= 0.0 || ai.route.is_finished() {
        return 0.0;
    }
    let Some(waypoint) = ai.route.waypoint() else {
        return 0.0;
    };
    let step = ai.move_speed * dt;
    let query = actor.query_origin();
    let query_goal = waypoint;
    let (position, distance) = match (navigator, collision) {
        (Some(navigator), Some(model)) => {
            let next = if walking_attachment {
                navigator.next_move_with_walking_attachment(
                    entity, query, query_goal, actor.hull, model, step, fallback,
                )
            } else {
                navigator
                    .next_move_with(entity, query, query_goal, actor.hull, model, step, fallback)
            };
            (
                actor.body_frame.query_to_anchor(actor.hull, next),
                (next - query).length(),
            )
        }
        (_, Some(model)) => {
            let result = move_toward(model, actor.hull, query, query_goal, ai.move_speed, dt);
            (
                actor
                    .body_frame
                    .query_to_anchor(actor.hull, result.position),
                result.distance,
            )
        }
        (_, None) => {
            let result = straight_step(
                query,
                query_goal,
                ai.move_speed,
                dt,
                movement::flies(actor.hull),
            );
            (
                actor
                    .body_frame
                    .query_to_anchor(actor.hull, result.position),
                result.distance,
            )
        }
    };
    actor.origin = position;
    if let Some(yaw) = yaw_toward(actor.origin, waypoint) {
        let (turned, _) = turn_toward(actor.yaw, yaw, TURN_RATE * dt);
        actor.yaw = turned;
    }
    ai.route.advance_if_reached(actor.query_origin());
    if ai.route.is_finished() {
        ai.move_speed = 0.0;
    }
    distance
}

/// Actor permission for initial attachment and terminal flat graph approach.
/// Policy stays here, above the context-free centered navigation API.
fn permits_ground_attachment(world: &World, entity: Entity, actor: &Actor) -> bool {
    use crate::monsters::{MonsterFlags, MonsterKind, spec_for};
    if !actor.alive
        || actor.health <= 0.0
        || actor.is_client
        || movement::flies(actor.hull)
        || !matches!(
            actor.body_frame,
            crate::BodyFrame::Feet | crate::BodyFrame::ModelBottom(_)
        )
        || world.get::<&Impervious>(entity).is_ok()
    {
        return false;
    }
    let Ok(class) = world.get::<&ohl_game::registry::ClassName>(entity) else {
        return false;
    };
    let kind = MonsterKind::from_classname(&class.0);
    spec_for(&kind).is_some_and(|spec| {
        !spec.flags.contains(MonsterFlags::ROOTED) && !movement::flies(spec.hull)
    }) && !matches!(
        kind,
        MonsterKind::Turret
            | MonsterKind::MiniTurret
            | MonsterKind::Sentry
            | MonsterKind::Tentacle
            | MonsterKind::Nihilanth
            | MonsterKind::Furniture
            | MonsterKind::Ichthyosaur
            | MonsterKind::Leech
            | MonsterKind::Apache
            | MonsterKind::Osprey
            | MonsterKind::AlienController
    )
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

// TODO(black-box): project-authored danger-only local alternatives, not general routing.
// Full flat legs retain preference; shorter prefixes use the same support checks.
fn danger_cover_goal(
    collision: Option<&CollisionModel>,
    hull: Hull,
    origin: Vec3,
    threat: Option<Vec3>,
    forward: Vec3,
) -> Option<Vec3> {
    let collision = collision?;
    let threat = threat?;
    if !origin.is_finite() || !threat.is_finite() {
        return None;
    }
    let away = Vec3::new(origin.x - threat.x, origin.y - threat.y, 0.0);
    let direction = if away.length() > f32::EPSILON {
        away.normalize()
    } else {
        Vec3::new(forward.x, forward.y, 0.0).normalize_or_zero()
    };
    if !direction.is_finite() || direction.length_squared() <= f32::EPSILON {
        return None;
    }
    let directions = [
        direction,
        Vec3::new(-direction.y, direction.x, 0.0),
        Vec3::new(direction.y, -direction.x, 0.0),
    ];
    if let Some(goal) = directions.into_iter().find_map(|direction| {
        let goal = origin + direction * COVER_DISTANCE;
        danger_cover_leg(collision, hull, origin, goal).then_some(goal)
    }) {
        return Some(goal);
    }

    // TODO(black-box): best-effort local escape when no full leg fits. Keep
    // the full-leg policy above, then compare only prefixes of those same
    // three rays. This predicts neither future blast position nor safety.
    let current_separation = origin.truncate().distance_squared(threat.truncate());
    if !current_separation.is_finite() {
        return None;
    }
    let mut best = None;
    let mut best_separation = current_separation;
    for direction in directions {
        let trace = collision.trace(hull, origin, origin + direction * COVER_DISTANCE);
        if trace.start_solid
            || trace.all_solid
            || !trace.fraction.is_finite()
            || trace.fraction <= 0.0
            || trace.fraction >= 1.0
        {
            continue;
        }
        let distance = COVER_DISTANCE * trace.fraction - ohl_physics::DIST_EPSILON;
        let mut goal = origin + direction * distance;
        goal.z = origin.z;
        if !danger_cover_leg_with_length(collision, hull, origin, goal, origin.distance(goal)) {
            continue;
        }
        let separation = goal.truncate().distance_squared(threat.truncate());
        // Strict improvement also preserves direction order for exact ties.
        if separation.is_finite() && separation > best_separation {
            best = Some(goal);
            best_separation = separation;
        }
    }
    best
}

fn danger_cover_leg(collision: &CollisionModel, hull: Hull, origin: Vec3, goal: Vec3) -> bool {
    // Full candidates retain their original nominal-length checks, including
    // when forming the endpoint rounds at large finite world coordinates.
    danger_cover_leg_with_length(collision, hull, origin, goal, COVER_DISTANCE)
}

fn danger_cover_leg_with_length(
    collision: &CollisionModel,
    hull: Hull,
    origin: Vec3,
    goal: Vec3,
    expected_distance: f32,
) -> bool {
    let epsilon = ohl_physics::DIST_EPSILON;
    let distance = origin.distance(goal);
    if !origin.is_finite()
        || !goal.is_finite()
        || !distance.is_finite()
        || !expected_distance.is_finite()
        || expected_distance <= movement::WAYPOINT_TOLERANCE
        || expected_distance - COVER_DISTANCE > epsilon
        || (distance - expected_distance).abs() > epsilon
        || goal.z.to_bits() != origin.z.to_bits()
    {
        return false;
    }
    let moved = move_toward(collision, hull, origin, goal, COVER_DISTANCE, 1.0);
    let chord = collision.trace(hull, origin, goal);
    if moved.blocked
        || !moved.position.is_finite()
        || !moved.distance.is_finite()
        || (moved.distance - expected_distance).abs() > epsilon
        || !moved.position.abs_diff_eq(goal, epsilon)
        || chord.start_solid
        || chord.all_solid
        || chord.fraction < 1.0
    {
        return false;
    }
    if movement::flies(hull) {
        return true;
    }
    // 320 / 48 rounds up to seven: at most 21 existing short-leg validations.
    let Some(slices) =
        (1_u8..=7).find(|count| distance <= movement::TERMINAL_GROUND_SPAN * f32::from(*count))
    else {
        return false;
    };
    let mut from = origin;
    for index in 1..=slices {
        let mut end = origin.lerp(goal, f32::from(index) / f32::from(slices));
        end.z = origin.z;
        let next =
            movement::terminal_ground_approach(collision, hull, from, end, end, from.distance(end));
        if !next.is_finite() || !next.abs_diff_eq(end, epsilon) {
            return false;
        }
        from = end;
    }
    true
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
    /// Tick-local choice; never saved or inferred from an already-written route.
    cover_threat: Option<Vec3>,
    danger_cover: bool,
    following: bool,
    follow_input: crate::follow::FollowInput,
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
        let from = self.actor.query_origin();
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
        if self.danger_cover {
            self.ai.cover = danger_cover_goal(
                self.collision,
                self.actor.hull,
                self.actor.query_origin(),
                self.cover_threat,
                self.actor.forward(),
            );
            return if self.ai.cover.is_some() {
                TaskStatus::Complete
            } else {
                TaskStatus::Failed
            };
        }
        let Some(threat) = self.cover_threat else {
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
        let goal = self.actor.query_origin() + direction * COVER_DISTANCE;
        let reachable = self.collision.map_or(goal, |model| {
            move_toward(
                model,
                self.actor.hull,
                self.actor.query_origin(),
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
    ///
    /// With a collision model the drawn point is clamped to where the
    /// monster can actually walk to ([`movement::walkable_reach`]: never
    /// into or past a wall, never out over a drop), the same way
    /// [`Self::find_cover`] clamps its cover point. A goal it cannot reach
    /// would otherwise fail every straight-line check and every graph
    /// search a [`NavBridge`] makes for it, every tick, and leave it to a
    /// fallback mover. A leg shorter than [`MIN_WANDER_LEG`] after clamping
    /// is not worth walking: the task fails and the critter picks another
    /// direction next spell.
    fn wander(&mut self, distance: f32) -> TaskStatus {
        if !distance.is_finite() || distance <= 0.0 {
            return TaskStatus::Failed;
        }
        let mut draw = Pcg32::with_stream(self.tick_count, u64::from(self.entity.id()));
        let yaw = draw.range_f32(-180.0, 180.0);
        let origin = self.actor.query_origin();
        let drawn = origin + movement::forward_from_yaw(yaw) * distance;
        let goal = self.collision.map_or(drawn, |model| {
            movement::walkable_reach(model, self.actor.hull, self.actor.query_origin(), drawn)
        });
        if Vec3::new(goal.x - origin.x, goal.y - origin.y, 0.0).length() < MIN_WANDER_LEG {
            return TaskStatus::Failed;
        }
        self.ai.move_target = Some(goal);
        self.start_route(Some(goal), 0.0)
    }

    fn start_follow_route(&mut self, within: f32) -> TaskStatus {
        use crate::follow::FollowPhase;
        let Some(attempt) = self.ai.follow_attempt else {
            return TaskStatus::Failed;
        };
        if attempt.phase != FollowPhase::Preparing {
            return TaskStatus::Complete;
        }
        let goal = self
            .actor
            .body_frame
            .anchor_to_query(self.actor.hull, attempt.accepted_player_anchor);
        let from = self.actor.query_origin();
        let delta = Vec3::new(goal.x - from.x, goal.y - from.y, 0.0);
        let length = delta.length();
        if !goal.is_finite() || !length.is_finite() {
            return TaskStatus::Failed;
        }
        let stop = if length > within {
            goal - delta / length * within
        } else {
            goal
        };
        self.ai.route = Route::straight_line(stop);
        TaskStatus::Complete
    }

    fn set_path_speed(&mut self, running: bool) -> TaskStatus {
        if self.following {
            use crate::follow::FollowPhase;
            let Some(attempt) = self.ai.follow_attempt.as_mut() else {
                return TaskStatus::Failed;
            };
            match attempt.phase {
                FollowPhase::Holding | FollowPhase::Arrived => {
                    self.ai.move_speed = 0.0;
                    return TaskStatus::Complete;
                }
                FollowPhase::Moving => return TaskStatus::Complete,
                FollowPhase::Preparing => {
                    if self.ai.route.is_finished() {
                        return TaskStatus::Failed;
                    }
                    attempt.phase = FollowPhase::Moving;
                    let (walk, run) = self.brain.speeds();
                    self.ai.move_speed = if running { run } else { walk };
                    return TaskStatus::Complete;
                }
            }
        }
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
                let target = if self.following {
                    match self.follow_input {
                        crate::follow::FollowInput::Target(goal) => {
                            Some(self.actor.body_frame.anchor_to_query(self.actor.hull, goal))
                        }
                        _ => None,
                    }
                } else {
                    self.ai.move_target
                };
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
                if self.following {
                    self.start_follow_route(within)
                } else {
                    let goal = self.ai.move_target;
                    self.start_route(goal, within)
                }
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
                let activity = if self.following
                    && self.ai.follow_attempt.is_some_and(|a| {
                        matches!(
                            a.phase,
                            crate::follow::FollowPhase::Holding
                                | crate::follow::FollowPhase::Arrived
                        )
                    }) {
                    Activity::Idle
                } else {
                    activity
                };
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
    fn terminal_ground_and_initial_attachment_require_a_known_living_solid_walking_policy() {
        use crate::BodyFrame;
        use ohl_game::registry::ClassName;
        let mut world = World::new();
        let entity = world.spawn((ClassName("monster_barney".into()),));
        let baseline = Actor::new(Classification::PlayerAlly, Vec3::ZERO);
        assert!(super::permits_ground_attachment(&world, entity, &baseline));
        let mut custom = baseline;
        custom.body_frame = BodyFrame::ModelBottom(-8.0);
        assert!(super::permits_ground_attachment(&world, entity, &custom));
        for frame in [
            BodyFrame::Centered,
            BodyFrame::Ceiling,
            BodyFrame::FixedModelAnchor(Some(-8.0)),
        ] {
            let mut actor = baseline;
            actor.body_frame = frame;
            assert!(!super::permits_ground_attachment(&world, entity, &actor));
        }
        for variant in 0..4 {
            let mut actor = baseline;
            match variant {
                0 => actor.is_client = true,
                1 => actor.alive = false,
                2 => actor.health = 0.0,
                _ => actor.hull = ohl_physics::Hull::Point,
            }
            assert!(!super::permits_ground_attachment(&world, entity, &actor));
        }
        world
            .insert_one(entity, super::Impervious)
            .expect("impervious");
        assert!(!super::permits_ground_attachment(&world, entity, &baseline));
        world
            .remove_one::<super::Impervious>(entity)
            .expect("remove guard");
        for classname in [
            "monster_barnacle",
            "monster_turret",
            "monster_miniturret",
            "monster_sentry",
            "monster_tentacle",
            "monster_nihilanth",
            "monster_furniture",
            "monster_ichthyosaur",
            "monster_leech",
            "monster_apache",
            "monster_osprey",
            "monster_alien_controller",
            "ohl_unknown",
        ] {
            world.get::<&mut ClassName>(entity).expect("class").0 = classname.into();
            assert!(
                !super::permits_ground_attachment(&world, entity, &baseline),
                "{classname}"
            );
        }
        world.remove_one::<ClassName>(entity).expect("remove class");
        assert!(!super::permits_ground_attachment(&world, entity, &baseline));
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

    fn danger_cover_scene() -> (
        AiWorld,
        World,
        ohl_physics::CollisionModel,
        hecs::Entity,
        hecs::Entity,
    ) {
        use ohl_formats::test_support::CollisionBrush;
        let collision = collision_from(
            &[
                CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
                CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -1_024.0),
                CollisionBrush::half_space([1.0, 0.0, 0.0], -1_024.0),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -1_024.0),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -1_024.0),
            ],
            [-1_024.0, -1_024.0, 0.0],
            [1_024.0, 1_024.0, 256.0],
        );
        let mut ai = AiWorld::new(0x5EED);
        let brain = ai.register_brain(Box::new(
            crate::monsters::MonsterBrain::for_kind(crate::monsters::MonsterKind::HumanAssassin)
                .expect("authored ordinary brain"),
        ));
        let mut world = World::new();
        let actor = Actor::new(Classification::HumanMilitary, Vec3::new(0.0, 0.0, 1.0));
        let enemy = Actor::new(Classification::Player, Vec3::new(384.0, 0.0, 1.0));
        for body in [actor, enemy] {
            let trace = collision.trace(body.hull, body.query_origin(), body.query_origin());
            assert!(!trace.start_solid && !trace.all_solid);
        }
        let sight = collision.trace(ohl_physics::Hull::Point, actor.eye(), enemy.eye());
        assert!(!sight.start_solid && !sight.all_solid && sight.fraction >= 1.0);
        assert!(
            ai.relationships()
                .get(actor.classification, enemy.classification)
                .is_hostile()
        );
        let listener = spawn_monster(&mut world, actor, brain);
        let target = spawn_actor(&mut world, enemy);
        (ai, world, collision, listener, target)
    }

    #[test]
    fn danger_cover_uses_current_sound_with_an_opposed_enemy() {
        let (mut ai, mut world, collision, listener, enemy) = danger_cover_scene();
        let danger = Vec3::new(-128.0, 0.0, 37.0);
        let start = world.get::<&Actor>(listener).unwrap().origin;
        assert!(ai.emit_sound(SoundEvent::new(SoundKind::Danger, danger, 512.0).lasting(5.0)));
        let context = SightContext::tracing(&collision);
        ai.tick(&mut world, &context, DT);
        {
            let state = world.get::<&MonsterAi>(listener).unwrap();
            assert!(
                state
                    .conditions
                    .contains(Conditions::SEE_ENEMY | Conditions::HEAR_DANGER)
            );
            assert_eq!(state.memory.unwrap().entity, enemy);
            assert_eq!(state.move_target, Some(danger));
            assert_eq!(
                state.schedule_name(),
                crate::brain::TAKE_COVER_FROM_DANGER.name
            );
            assert_eq!(state.runner.task(), Some(crate::Task::FindCover));
            assert!(!state.runner.started());
            assert!(state.cover.is_none());
        }
        ai.tick(&mut world, &context, DT);
        {
            let state = world.get::<&MonsterAi>(listener).unwrap();
            assert!(
                state
                    .conditions
                    .contains(Conditions::SEE_ENEMY | Conditions::HEAR_DANGER)
            );
            assert_eq!(state.memory.unwrap().entity, enemy);
            let cover = state.cover.expect("ordinary FindCover completed");
            assert!(
                cover.x > start.x,
                "danger cover must escape away from the heard danger"
            );
            assert_eq!(state.runner.task(), Some(crate::Task::TakeCover));
        }
        for _ in 0..8 {
            ai.tick(&mut world, &context, DT);
        }
        let actor = world.get::<&Actor>(listener).unwrap();
        assert!(
            actor.origin.x > start.x,
            "ordinary route movement follows danger cover"
        );
        assert!((actor.origin - danger).truncate().length() > (start - danger).truncate().length());
    }

    #[test]
    fn danger_cover_preserves_enemy_cover_for_other_schedules() {
        let (mut ai, mut world, collision, listener, enemy) = danger_cover_scene();
        let context = SightContext::tracing(&collision);
        let start = world.get::<&Actor>(listener).unwrap().origin;
        let mut covered = false;
        for _ in 0..100 {
            ai.tick(&mut world, &context, DT);
            let state = world.get::<&MonsterAi>(listener).unwrap();
            assert!(state.conditions.contains(Conditions::SEE_ENEMY));
            assert!(!state.conditions.contains(Conditions::HEAR_DANGER));
            assert_eq!(state.memory.unwrap().entity, enemy);
            assert_eq!(
                state.schedule_name(),
                crate::monsters::brains::ASSASSIN_HIT_AND_RUN.name
            );
            if let Some(cover) = state.cover {
                assert!(
                    cover.x < start.x,
                    "ordinary combat cover still escapes from the enemy"
                );
                assert_eq!(state.runner.task(), Some(crate::Task::TakeCover));
                covered = true;
                break;
            }
        }
        assert!(covered, "ordinary burst must reach its own FindCover task");
        for _ in 0..8 {
            ai.tick(&mut world, &context, DT);
        }
        assert!(world.get::<&Actor>(listener).unwrap().origin.x < start.x);
    }

    #[test]
    fn danger_cover_rejects_expired_or_unrelated_sound() {
        for unrelated in [false, true] {
            let (mut ai, mut world, collision, listener, enemy) = danger_cover_scene();
            let context = SightContext::tracing(&collision);
            let danger = Vec3::new(-128.0, 0.0, 37.0);
            let start = world.get::<&Actor>(listener).unwrap().origin;
            assert!(ai.emit_sound(SoundEvent::new(SoundKind::Danger, danger, 512.0).lasting(DT)));
            ai.tick(&mut world, &context, DT);
            {
                let state = world.get::<&MonsterAi>(listener).unwrap();
                assert!(
                    state
                        .conditions
                        .contains(Conditions::SEE_ENEMY | Conditions::HEAR_DANGER)
                );
                assert_eq!(state.memory.unwrap().entity, enemy);
                assert_eq!(
                    state.schedule_name(),
                    crate::brain::TAKE_COVER_FROM_DANGER.name
                );
                assert_eq!(state.runner.task(), Some(crate::Task::FindCover));
                assert_eq!(state.move_target, Some(danger));
                assert!(state.cover.is_none());
            }
            assert!(ai.sounds().is_empty(), "the one-tick danger has expired");
            if unrelated {
                assert!(ai.emit_sound(SoundEvent::new(
                    SoundKind::World,
                    Vec3::new(0.0, 128.0, 37.0),
                    512.0,
                )));
            }
            let events = ai.tick(&mut world, &context, DT);
            assert!(
                events.iter().any(|event| event.entity == listener
                    && matches!(
                        event.kind,
                        AiEventKind::ScheduleEnded { name, outcome: crate::RunOutcome::Failed }
                            if name == crate::brain::TAKE_COVER_FROM_DANGER.name
                    )),
                "missing qualified danger must fail the cover task"
            );
            let state = world.get::<&MonsterAi>(listener).unwrap();
            assert!(state.conditions.contains(Conditions::SEE_ENEMY));
            assert!(!state.conditions.contains(Conditions::HEAR_DANGER));
            assert_eq!(state.memory.unwrap().entity, enemy);
            assert!(
                state.cover.is_none(),
                "neither an enemy nor a stale/unrelated point substitutes"
            );
            assert_eq!(world.get::<&Actor>(listener).unwrap().origin, start);
        }
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

    /// A closed collision model from `brushes`, bounded by `mins..maxs`.
    fn collision_from(
        brushes: &[ohl_formats::test_support::CollisionBrush],
        mins: [f32; 3],
        maxs: [f32; 3],
    ) -> ohl_physics::CollisionModel {
        use ohl_formats::bsp30::{Bsp, Limits};
        let mut builder = ohl_formats::test_support::Bsp30Builder::new();
        builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
        let heads = builder.push_collision_hulls(brushes);
        builder.push_model(mins, maxs, [0.0, 0.0, 0.0], heads, 2, 0, 0);
        let bytes = builder.build();
        let limits = Limits::default();
        let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
        ohl_physics::CollisionModel::from_bsp(&bsp, &limits).expect("fixture has collision hulls")
    }

    /// A rat on the crouched hull at `origin`, ticked at the engine's own
    /// rate against `collision` for `ticks` ticks. Returns every wander
    /// goal it set, every origin it stood at, and how many
    /// `critter_wander` spells ended `Done`.
    fn wander_in(
        collision: &ohl_physics::CollisionModel,
        origin: Vec3,
        navigator: bool,
        ticks: usize,
    ) -> (Vec<Vec3>, Vec<Vec3>, usize) {
        let mut ai = AiWorld::new(0x5EED);
        let brain = ai.register_brain(Box::new(
            crate::monsters::MonsterBrain::for_kind(crate::monsters::MonsterKind::Rat)
                .expect("defined"),
        ));
        if navigator {
            ai.attach_navigator(crate::monsters::NavBridge::build(
                &[],
                collision,
                &ohl_nav::BuildLimits::default(),
                crate::monsters::NavBridgeLimits::default(),
            ));
        }
        let mut world = World::new();
        let mut actor = Actor::new(Classification::None, origin);
        actor.hull = ohl_physics::Hull::Crouched;
        let rat = spawn_monster(&mut world, actor, brain);
        let context = SightContext::tracing(collision);
        let (mut goals, mut origins, mut finished) = (Vec::new(), Vec::new(), 0);
        for _ in 0..ticks {
            for event in ai.tick(&mut world, &context, ohl_physics::controller::TICK_SECONDS) {
                if let AiEventKind::ScheduleEnded { name, outcome } = event.kind
                    && name == crate::monsters::brains::CRITTER_WANDER.name
                    && outcome == crate::schedule::RunOutcome::Done
                {
                    finished += 1;
                }
            }
            let state = world.get::<&MonsterAi>(rat).expect("ai");
            if let Some(goal) = state.move_target {
                goals.push(goal);
            }
            origins.push(world.get::<&Actor>(rat).expect("actor").origin);
        }
        (goals, origins, finished)
    }

    /// Wave 1 batch A review: a wander goal is clamped to what the critter
    /// can reach. In a closed room narrower than a wander leg, with a
    /// `NavBridge` attached (whose fallback, for a goal no straight line
    /// or graph route reaches, is what once carried a monster through a
    /// wall), every goal the rat sets and every spot it stands on stays
    /// inside the walls, and it still finishes legs.
    #[test]
    fn a_wander_goal_is_clamped_inside_the_walls() {
        use ohl_formats::test_support::CollisionBrush;
        const HALF: f32 = 96.0;
        let room = collision_from(
            &[
                CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
                CollisionBrush::half_space([0.0, 0.0, -1.0], -128.0),
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
                CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
            ],
            [-HALF, -HALF, 0.0],
            [HALF, HALF, 128.0],
        );
        // The actor stores its feet, one unit above this synthetic floor.
        let (goals, origins, finished) = wander_in(&room, Vec3::new(0.0, 0.0, 1.0), true, 3_000);
        // The crouched hull is 32 wide: its origin can get no closer to a
        // wall than 16.
        let inside = |point: &Vec3| point.x.abs() <= HALF - 15.0 && point.y.abs() <= HALF - 15.0;
        assert!(!goals.is_empty(), "the rat wandered at all");
        for goal in &goals {
            assert!(inside(goal), "a wander goal beyond the walls: {goal:?}");
        }
        for origin in &origins {
            assert!(inside(origin), "the rat left the room: {origin:?}");
            assert!((0.0..=1.01).contains(&origin.z), "feet remain at the floor");
        }
        assert!(finished >= 2, "it still finishes legs: {finished}");
    }

    /// Wave 1 batch A review: monsters have no gravity, so a leg aimed off
    /// a ledge would float the critter out over the drop. On a platform
    /// smaller than one wander leg, the floor probe ends every leg at the
    /// edge: the rat never stands anywhere its hull has no platform under
    /// it.
    #[test]
    fn a_wander_never_walks_off_a_ledge() {
        use ohl_formats::test_support::CollisionBrush;
        const PLATFORM: f32 = 48.0;
        const TOP: f32 = 64.0;
        let room = collision_from(
            &[
                CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
                CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
                CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
                CollisionBrush::box_brush([-PLATFORM, -PLATFORM, 0.0], [PLATFORM, PLATFORM, TOP]),
            ],
            [-512.0, -512.0, 0.0],
            [512.0, 512.0, 256.0],
        );
        let (_, origins, finished) = wander_in(&room, Vec3::new(0.0, 0.0, TOP + 1.0), false, 3_000);
        // Half the crouched hull's width past the edge is the farthest the
        // hull can stand with some of the platform still under it.
        for origin in &origins {
            assert!(
                origin.x.abs() <= PLATFORM + 16.0 && origin.y.abs() <= PLATFORM + 16.0,
                "the rat walked off the platform: {origin:?}"
            );
            assert!(
                (TOP..=TOP + 1.01).contains(&origin.z),
                "feet stay on top of it: {origin:?}"
            );
        }
        assert!(finished >= 2, "it still finishes legs: {finished}");
    }

    /// Wave 1 batch A review: a controller flies through the whole tick,
    /// not just in `move_toward`'s own unit test. Against a real collision
    /// model, with an enemy above it and beyond its volley's range, it
    /// chases and climbs toward it; a walker on the same chase would keep
    /// its height.
    #[test]
    fn a_controller_climbs_to_chase_an_enemy_above_it() {
        use ohl_formats::test_support::CollisionBrush;
        let room = collision_from(
            &[
                CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
                CollisionBrush::half_space([0.0, 0.0, -1.0], -1_024.0),
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -2_048.0),
                CollisionBrush::half_space([1.0, 0.0, 0.0], -2_048.0),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -2_048.0),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -2_048.0),
            ],
            [-2_048.0, -2_048.0, 0.0],
            [2_048.0, 2_048.0, 1_024.0],
        );
        let kind = crate::monsters::MonsterKind::AlienController;
        let spec = crate::monsters::spec_for(&kind).expect("defined");
        let mut ai = AiWorld::new(0x5EED);
        let brain = ai.register_brain(Box::new(
            crate::monsters::MonsterBrain::for_kind(kind).expect("defined"),
        ));
        let mut world = World::new();
        let mut actor = Actor::new(spec.classification, Vec3::new(0.0, 0.0, 64.0));
        actor.hull = spec.hull;
        let controller = spawn_monster(&mut world, actor, brain);
        spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(1_500.0, 0.0, 600.0)).as_client(),
        );
        let context = SightContext::tracing(&room);
        for _ in 0..200 {
            ai.tick(&mut world, &context, ohl_physics::controller::TICK_SECONDS);
        }
        let origin = world.get::<&Actor>(controller).expect("actor").origin;
        assert!(origin.x > 100.0, "it chased: {origin:?}");
        assert!(
            origin.z > 64.0 + 50.0,
            "and climbed while it did: {origin:?}"
        );
    }

    /// Walks to the move target it was given, and nothing else.
    static WALK_TO_TARGET: crate::schedule::Schedule = crate::schedule::Schedule::new(
        "test/walk_to_target",
        &[
            crate::schedule::Task::MoveToTarget { within: 0.0 },
            crate::schedule::Task::RunPath,
            crate::schedule::Task::WaitForMovement,
        ],
        Conditions::EMPTY,
    );

    struct Walker;

    impl crate::schedule::Brain for Walker {
        fn classification(&self) -> Classification {
            Classification::None
        }

        fn select_schedule(
            &self,
            _state: MonsterState,
            _conditions: Conditions,
        ) -> &'static crate::schedule::Schedule {
            &WALK_TO_TARGET
        }
    }

    /// Wave 1 batch A review: with a `NavBridge` attached and no route
    /// around a wall, a monster on its own brain is stopped by the wall
    /// (the bridge's traced fallback), while a monster a script is walking
    /// to its mark still crosses it (`Fallback::StraightLine`, kept because
    /// a scripted walk that never arrives stalls the map).
    #[test]
    fn only_a_scripted_walk_keeps_the_straight_line_fallback() {
        use ohl_formats::test_support::CollisionBrush;
        let room = collision_from(
            &[
                CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
                CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
                CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
                CollisionBrush::box_brush([-16.0, -128.0, -16.0], [16.0, 128.0, 256.0]),
            ],
            [-512.0, -512.0, 0.0],
            [512.0, 512.0, 256.0],
        );
        let goal = Vec3::new(100.0, 0.0, 1.0);
        let run = |scripted: bool| {
            let mut ai = AiWorld::new(1);
            let brain = ai.register_brain(Box::new(Walker));
            ai.attach_navigator(crate::monsters::NavBridge::build(
                &[],
                &room,
                &ohl_nav::BuildLimits::default(),
                crate::monsters::NavBridgeLimits::default(),
            ));
            let mut world = World::new();
            let monster = spawn_monster(
                &mut world,
                Actor::new(Classification::None, Vec3::new(-100.0, 0.0, 1.0)),
                brain,
            );
            {
                let mut state = world.get::<&mut MonsterAi>(monster).expect("ai");
                state.move_target = Some(goal);
                if scripted {
                    state.route = crate::movement::Route::straight_line(goal);
                    state.move_speed = 160.0;
                }
            }
            if scripted {
                world
                    .insert_one(monster, crate::scripts::ScriptHold)
                    .expect("alive");
            }
            let context = SightContext::tracing(&room);
            for _ in 0..300 {
                ai.tick(&mut world, &context, ohl_physics::controller::TICK_SECONDS);
            }
            world.get::<&Actor>(monster).expect("actor").origin
        };
        let own = run(false);
        assert!(
            own.x < -16.0,
            "a monster on its own brain crossed the wall: {own:?}"
        );
        let scripted = run(true);
        assert!(
            scripted.x > 16.0,
            "a scripted walk was stopped short of its mark: {scripted:?}"
        );
    }

    #[test]
    fn danger_local_cover_rejects_a_broad_unsupported_middle() {
        use ohl_formats::test_support::CollisionBrush;
        use ohl_physics::Hull;
        let floor = collision_from(
            &[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)],
            [-512.0, -512.0, -64.0],
            [512.0, 512.0, 256.0],
        );
        let gap = collision_from(
            &[
                CollisionBrush::box_brush([-512.0, -512.0, -64.0], [64.0, 512.0, 0.0]),
                CollisionBrush::box_brush([256.0, -512.0, -64.0], [512.0, 512.0, 0.0]),
            ],
            [-512.0, -512.0, -64.0],
            [512.0, 512.0, 256.0],
        );
        let start = Vec3::new(0.0, 0.0, 37.0);
        let end = start + Vec3::X * super::COVER_DISTANCE;
        let middle = start.lerp(end, 0.5);
        for point in [start, end] {
            let support = gap.trace(Hull::Standing, point, point - Vec3::Z * 2.0);
            assert!(!support.start_solid && support.fraction < 1.0);
        }
        let unsupported = gap.trace(Hull::Standing, middle, middle - Vec3::Z * 2.0);
        assert!(!unsupported.start_solid && unsupported.fraction >= 1.0);
        let chord = gap.trace(Hull::Standing, start, end);
        assert!(!chord.start_solid && chord.fraction >= 1.0);
        let moved = crate::movement::move_toward(
            &gap,
            Hull::Standing,
            start,
            end,
            super::COVER_DISTANCE,
            1.0,
        );
        assert!(!moved.blocked && moved.position.abs_diff_eq(end, ohl_physics::DIST_EPSILON));
        assert!(super::danger_cover_leg(&floor, Hull::Standing, start, end));
        assert!(super::danger_cover_leg(&gap, Hull::Point, start, end));
        assert!(
            !super::danger_cover_leg(&gap, Hull::Standing, start, end),
            "supported endpoints cannot certify the unsupported middle"
        );
    }

    #[test]
    fn danger_local_cover_point_omits_only_ground_validation() {
        use ohl_formats::test_support::CollisionBrush;
        use ohl_physics::Hull;
        let empty = collision_from(&[], [-512.0; 3], [512.0; 3]);
        let walls = collision_from(
            &[
                CollisionBrush::half_space([-1.0, 0.0, 0.0], -32.0),
                CollisionBrush::half_space([0.0, -1.0, 0.0], -32.0),
                CollisionBrush::half_space([0.0, 1.0, 0.0], -32.0),
            ],
            [-512.0; 3],
            [512.0; 3],
        );
        let start = Vec3::new(0.0, 0.0, 64.0);
        let threat_position = start - Vec3::X * 96.0;
        let threat = Some(threat_position);
        let goal = super::danger_cover_goal(Some(&empty), Hull::Point, start, threat, Vec3::X);
        assert_eq!(goal, Some(start + Vec3::X * super::COVER_DISTANCE));
        assert!(
            super::danger_cover_goal(Some(&empty), Hull::Standing, start, threat, Vec3::X)
                .is_none()
        );
        assert!(super::danger_cover_goal(None, Hull::Point, start, threat, Vec3::X).is_none());
        // Project-authored short fallback: a point needs a clear improving
        // prefix, but still has no ground-support requirement.
        let short = super::danger_cover_goal(Some(&walls), Hull::Point, start, threat, Vec3::X)
            .expect("the bounded wall prefix is usable");
        assert!(short.is_finite());
        assert!(short.distance(start) > crate::movement::WAYPOINT_TOLERANCE);
        assert!(short.distance(start) < super::COVER_DISTANCE);
        assert!(
            short
                .truncate()
                .distance_squared(threat_position.truncate())
                > start
                    .truncate()
                    .distance_squared(threat_position.truncate())
        );
        let trace = walls.trace(Hull::Point, start, short);
        assert!(!trace.start_solid && !trace.all_solid && trace.fraction >= 1.0);
        assert!(
            super::danger_cover_goal(Some(&empty), Hull::Point, start, None, Vec3::X).is_none()
        );
    }
}

#[cfg(test)]
mod follow_intent_tests {
    use super::*;
    use crate::follow::{FollowInput, FollowPhase};
    use crate::monsters::{MonsterBrain, MonsterKind};

    const DT: f32 = 0.01;

    fn setup() -> (AiWorld, World, Entity) {
        let mut ai = AiWorld::new(17);
        let brain = ai.register_brain(Box::new(
            MonsterBrain::for_kind(MonsterKind::Scientist).expect("brain"),
        ));
        let mut world = World::new();
        let entity = spawn_monster(
            &mut world,
            Actor::new(Classification::HumanPassive, Vec3::ZERO),
            brain,
        );
        (ai, world, entity)
    }

    fn step(ai: &mut AiWorld, world: &mut World, entity: Entity, input: FollowInput, dt: f32) {
        {
            let mut state = world.get::<&mut MonsterAi>(entity).expect("AI");
            state.follow_input = input;
            if !matches!(input, FollowInput::Inactive) {
                state.pending_conditions |= Conditions::SPECIAL2;
            }
        }
        ai.tick(world, &SightContext::empty(), dt);
    }

    fn state(world: &World, entity: Entity) -> MonsterAi {
        (*world.get::<&MonsterAi>(entity).expect("AI")).clone()
    }

    fn position(world: &World, entity: Entity) -> Vec3 {
        world.get::<&Actor>(entity).expect("actor").origin
    }

    #[test]
    fn follow_intent_admission_invalidates_the_real_warm_close_endpoint() {
        use ohl_formats::bsp30::{Bsp, Limits};
        use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
        let mut builder = Bsp30Builder::new();
        let heads =
            builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
        builder.push_model(
            [-512.0, -512.0, 0.0],
            [512.0, 512.0, 256.0],
            [0.0; 3],
            heads,
            2,
            0,
            0,
        );
        let bytes = builder.build();
        let bsp = Bsp::parse(&bytes, &Limits::default()).expect("generated BSP");
        let collision = CollisionModel::from_bsp(&bsp, &Limits::default()).expect("collision");
        let (mut ai, mut world, entity) = setup();
        ai.attach_navigator(NavBridge::build(
            &[],
            &collision,
            &ohl_nav::BuildLimits::default(),
            crate::monsters::NavBridgeLimits::default(),
        ));
        let old = Vec3::X * 136.0;
        let new = Vec3::Y * 136.0;
        let tick = |ai: &mut AiWorld, world: &mut World, goal| {
            let mut state = world.get::<&mut MonsterAi>(entity).expect("AI");
            state.follow_input = FollowInput::Target(goal);
            state.pending_conditions |= Conditions::SPECIAL2;
            drop(state);
            ai.tick(world, &SightContext::tracing(&collision), DT);
        };
        for _ in 0..3 {
            tick(&mut ai, &mut world, old);
        }
        let warmed = state(&world, entity);
        let before = position(&world, entity);
        assert_eq!(
            warmed.follow_attempt.expect("moving").phase,
            FollowPhase::Moving
        );
        assert!(before.x > 0.0 && before.x < 1.0 && before.y == 0.0);
        assert!(!warmed.route.is_finished());
        assert_eq!(ai.navigator().expect("navigator").stats().direct_steps, 1);
        assert!((old - new).length() > movement::ROUTE_REFRESH_DISTANCE);
        let actor = *world.get::<&Actor>(entity).expect("actor");
        let query_goal = actor.body_frame.anchor_to_query(actor.hull, new);
        let delta = query_goal - actor.query_origin();
        let expected_stop = query_goal - delta.normalize() * crate::follow::FOLLOW_DISTANCE;
        assert!((expected_stop - warmed.route.goal).length() < movement::ROUTE_REFRESH_DISTANCE);
        tick(&mut ai, &mut world, new);
        let admitted = state(&world, entity);
        assert_eq!(admitted.runner.task_index(), 0);
        assert!(!admitted.runner.started());
        assert_eq!(position(&world, entity), before);
        assert_eq!(
            admitted
                .follow_attempt
                .expect("admitted")
                .accepted_player_anchor,
            new
        );
        for index in [1, 2] {
            tick(&mut ai, &mut world, new);
            assert_eq!(state(&world, entity).runner.task_index(), index);
            assert_eq!(position(&world, entity), before);
        }
        tick(&mut ai, &mut world, new);
        let after = position(&world, entity);
        assert!(
            after.y > before.y && after.x < before.x,
            "actual admitted close-endpoint retarget must turn the next physical step"
        );
    }

    #[test]
    fn follow_intent_raw_goal_and_replacement_obey_one_task_cadence() {
        let (mut ai, mut world, entity) = setup();
        let old = Vec3::X * 300.0;
        step(&mut ai, &mut world, entity, FollowInput::Target(old), DT);
        assert_eq!(state(&world, entity).runner.task_index(), 1);
        assert_eq!(position(&world, entity), Vec3::ZERO);
        // A real non-danger sound may write the unrelated move_target.
        ai.emit_sound(SoundEvent::new(SoundKind::Combat, Vec3::Y * 200.0, 512.0).lasting(DT));
        step(&mut ai, &mut world, entity, FollowInput::Target(old), DT);
        let route = state(&world, entity);
        assert_eq!(route.runner.task_index(), 2);
        assert!(
            route
                .route
                .goal
                .abs_diff_eq(Vec3::new(204.0, 0.0, 36.0), 0.001)
        );
        for goal in [old, old + Vec3::Y * 40.0, old + Vec3::Y * 80.0] {
            step(&mut ai, &mut world, entity, FollowInput::Target(goal), DT);
            assert_eq!(
                state(&world, entity)
                    .follow_attempt
                    .expect("attempt")
                    .accepted_player_anchor,
                old
            );
        }
        let before = position(&world, entity);
        let next = old + Vec3::Y * 81.0;
        step(&mut ai, &mut world, entity, FollowInput::Target(next), DT);
        let admitted = state(&world, entity);
        assert_eq!(admitted.runner.task_index(), 0);
        assert!(!admitted.runner.started() && admitted.route.is_finished());
        assert_eq!(
            admitted
                .follow_attempt
                .expect("attempt")
                .accepted_player_anchor,
            next
        );
        assert_eq!(position(&world, entity), before);
        for expected_task in [1, 2, 3] {
            step(&mut ai, &mut world, entity, FollowInput::Target(next), DT);
            assert_eq!(state(&world, entity).runner.task_index(), expected_task);
            if expected_task < 3 {
                assert_eq!(position(&world, entity), before);
            }
        }
        let moved = position(&world, entity) - before;
        assert!(moved.x > 0.0 && moved.y > 0.0 && moved.length() <= 0.401);
        assert_eq!(
            state(&world, entity)
                .follow_attempt
                .expect("attempt")
                .accepted_player_anchor,
            next
        );
    }

    #[test]
    fn follow_intent_pause_priority_and_actual_ownership_are_distinct() {
        static BUSY: Schedule = Schedule::new(
            "authored/follow_busy",
            &[Task::Wait(1.0)],
            Conditions::EMPTY,
        );
        let (mut ai, mut world, entity) = setup();
        let old = Vec3::X * 300.0;
        for _ in 0..3 {
            step(&mut ai, &mut world, entity, FollowInput::Target(old), DT);
        }
        // A lower-level authored partially accumulated history, not the real-input primary.
        world.get::<&mut MonsterAi>(entity).expect("AI").stuck = StuckDetector::from_ticks(12);
        let before = state(&world, entity);
        let origin = position(&world, entity);
        step(&mut ai, &mut world, entity, FollowInput::Unavailable, DT);
        assert_eq!(state(&world, entity).stuck, before.stuck);
        assert_eq!(state(&world, entity).route, before.route);
        assert_eq!(position(&world, entity), origin);
        step(
            &mut ai,
            &mut world,
            entity,
            FollowInput::Target(Vec3::Y * 300.0),
            0.0,
        );
        assert_eq!(state(&world, entity).follow_attempt, before.follow_attempt);
        assert_eq!(state(&world, entity).stuck, before.stuck);
        world
            .get::<&mut MonsterAi>(entity)
            .expect("AI")
            .pending_conditions |= Conditions::TASK_FAILED;
        step(
            &mut ai,
            &mut world,
            entity,
            FollowInput::Target(Vec3::Y * 300.0),
            DT,
        );
        assert_eq!(state(&world, entity).follow_attempt, before.follow_attempt);
        assert!(
            state(&world, entity)
                .conditions
                .contains(Conditions::TASK_FAILED)
        );
        ai.emit_sound(SoundEvent::new(SoundKind::Danger, Vec3::X * 20.0, 512.0).lasting(DT));
        step(
            &mut ai,
            &mut world,
            entity,
            FollowInput::Target(Vec3::Y * 300.0),
            DT,
        );
        assert!(
            !state(&world, entity)
                .runner
                .schedule()
                .is_some_and(is_follow_schedule)
        );
        assert!(state(&world, entity).follow_attempt.is_none());
        // A still-running unrelated schedule is not preempted by an eligible candidate.
        world
            .get::<&mut MonsterAi>(entity)
            .expect("AI")
            .runner
            .start(&BUSY);
        step(
            &mut ai,
            &mut world,
            entity,
            FollowInput::Target(Vec3::Y * 300.0),
            DT,
        );
        assert_eq!(state(&world, entity).runner.schedule_name(), BUSY.name);
        assert!(state(&world, entity).follow_attempt.is_none());
        assert_active_follow_lifecycle_retirement(old);
    }

    fn assert_active_follow_lifecycle_retirement(old: Vec3) {
        // Each lifecycle action starts from its own ordinarily entered Moving
        // attempt, so an earlier takeover cannot make retirement vacuous.
        for action in 0..3 {
            let (mut ai, mut world, entity) = setup();
            for _ in 0..3 {
                step(&mut ai, &mut world, entity, FollowInput::Target(old), DT);
            }
            let moving = state(&world, entity);
            let before = position(&world, entity);
            assert_eq!(
                moving.follow_attempt.expect("active").phase,
                FollowPhase::Moving
            );
            assert!(!moving.route.is_finished() && moving.move_speed > 0.0);
            assert!(before.x > 0.0);
            match action {
                0 => {
                    world.insert_one(entity, crate::ScriptHold).expect("hold");
                }
                1 => {}
                _ => {
                    world.get::<&mut Actor>(entity).expect("actor").health = 0.0;
                }
            }
            let input = if action == 1 {
                FollowInput::Inactive
            } else {
                FollowInput::Target(old)
            };
            step(&mut ai, &mut world, entity, input, DT);
            let retired = state(&world, entity);
            assert_eq!(retired.follow_input, FollowInput::Unavailable);
            assert!(retired.follow_attempt.is_none());
            if action != 2 {
                assert!(!retired.runner.schedule().is_some_and(is_follow_schedule));
            }
            // Existing brain selection may name FOLLOW on a dead SPECIAL2 actor;
            // the ownership contract below still prohibits an attempt or motion.
            if action == 0 {
                // Script possession retains its existing route/movement policy.
                assert_eq!(retired.route, moving.route);
                assert!(position(&world, entity).x > before.x);
                world
                    .remove_one::<crate::ScriptHold>(entity)
                    .expect("release");
                step(&mut ai, &mut world, entity, FollowInput::Target(old), DT);
                assert!(state(&world, entity).follow_attempt.is_some());
            } else {
                assert_eq!(position(&world, entity), before);
                assert!(retired.route.is_finished() && retired.move_speed == 0.0);
                assert_eq!(retired.stuck.ticks(), 0);
                if action == 2 {
                    assert_eq!(retired.state, MonsterState::Dead);
                }
            }
        }
    }

    #[test]
    fn follow_intent_holds_without_repeated_admission_then_restarts() {
        let (mut ai, mut world, entity) = setup();
        let initial = Vec3::X * 60.0;
        step(
            &mut ai,
            &mut world,
            entity,
            FollowInput::Target(initial),
            DT,
        );
        let accepted = state(&world, entity).follow_attempt;
        assert_eq!(accepted.expect("held").phase, FollowPhase::Holding);
        for x in [100.0, 104.0, 60.0, 90.0, 100.0, 104.0] {
            step(
                &mut ai,
                &mut world,
                entity,
                FollowInput::Target(Vec3::X * x),
                DT,
            );
            assert_eq!(state(&world, entity).follow_attempt, accepted);
            assert_eq!(position(&world, entity), Vec3::ZERO);
            assert_eq!(state(&world, entity).stuck.ticks(), 0);
            assert!(state(&world, entity).route.waypoints.is_empty());
        }
        let goal = Vec3::X * 120.0;
        step(&mut ai, &mut world, entity, FollowInput::Target(goal), DT);
        assert_eq!(
            state(&world, entity).follow_attempt.expect("attempt").phase,
            FollowPhase::Preparing
        );
        for _ in 0..150 {
            step(&mut ai, &mut world, entity, FollowInput::Target(goal), DT);
        }
        assert!(position(&world, entity).x > 8.0);
        assert_eq!(
            state(&world, entity)
                .follow_attempt
                .expect("arrived hold")
                .phase,
            FollowPhase::Holding
        );
    }
}
