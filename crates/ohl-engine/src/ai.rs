//! Monster AI and navigation, wired into the one entity world.
//!
//! [`AiState`] owns the [`ohl_ai::AiWorld`] and everything around it that is
//! not per-entity: which brain each `monster_*` classname gets, the
//! per-entity `TriggerCondition`/`TriggerTarget` pairs a map declared, the
//! damage aimed at monsters, and the corpse bookkeeping. The monsters
//! themselves are ordinary entities in [`ohl_game::Registry::world`], so a
//! monster is a [`ohl_game::registry::Transform`], an [`ohl_ai::Actor`], an
//! [`ohl_ai::MonsterAi`] and (when its model loaded) a
//! [`crate::components::StudioAnim`] — the same components everything else
//! in the engine already reads.
//!
//! Two seams keep this module from growing into the rest of M7.9:
//!
//! - **Attacks are mapped, not resolved.** [`ohl_ai::AiWorld`] reports
//!   [`ohl_ai::AiEventKind::Attack`]; this module turns that into either a
//!   trace and a [`ohl_combat::DamageInfo`] pushed onto the engine's damage
//!   queue, or a [`ProjectileRequest`] handed to a [`ProjectileSpawner`].
//!   The default spawner does nothing, so ranged monsters whose attack is a
//!   projectile simply do not hurt anything until M7.9 P3 installs a real
//!   one.
//! - **Health arrives as data.** Damage aimed at a monster is queued through
//!   [`AiState::queue_damage`] and applied once per step, in phase 10, by
//!   [`ohl_ai::apply_monster_damage`], which is the only place a monster's
//!   health moves and the only thing that can emit
//!   [`ohl_ai::AiEventKind::Died`] for it.
//!
//! # Determinism
//!
//! Brains are registered in the sorted order of the classnames a map
//! declares, so two loads of the same map hand out the same
//! [`ohl_ai::BrainId`]s. Every per-entity list here is kept in spawn order,
//! never in `HashMap` order, and the only random stream is
//! [`ohl_ai::AiWorld`]'s own, seeded from [`crate::SystemsConfig::rng_seed`].
//!
//! # Clean room
//!
//! Every behavioural number consumed here belongs to `ohl-ai` and is cited
//! (or marked black-box) there; see `docs/FORMAT_SOURCES.md`, "Monster AI
//! behaviour", "Monster definitions" and "Navigation". The two tables this
//! module adds — which classname gets which brain, and which monster
//! attack is a projectile rather than a trace — are **project-authored**:
//! they name this project's own `ohl-ai`/`ohl-combat` vocabulary and encode
//! no published number.
//!
//! # Logging
//!
//! Nothing here logs. Classnames, monster kinds and counts are all
//! media-derived; [`ohl_ai::MonsterKind::Unknown`] in particular carries a
//! map-authored classname and never reaches a diagnostic.

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_ai::monsters::spec_for;
use ohl_ai::monsters::table::Difficulty as AiDifficulty;
use ohl_ai::{
    Actor, AiEvent, AiEventKind, AiWorld, AttackKind, BrainId, Classification, Conditions,
    CorpseDecision, DamageEvent, DamageQueue, DamageSink, MonsterAi, MonsterBrain, MonsterKind,
    MonsterSpawn, MonsterSpawnRules, MonsterSpec, MonsterTrigger, SightContext, TriggerCondition,
    TriggerContext, attach_monsters,
};
use ohl_combat::{DamageType, HitboxIndex, HitboxLimits, TraceFilter, TraceMask};
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::EntityDef;
use ohl_game::registry::{ClassName, Transform};

use crate::components::{Corpse, MonsterMaker, Owner, StudioAnim};
use crate::ids::entity_id;
use crate::level::Level;
use crate::nav;
use crate::systems::QueuedDamage;

/// How long a corpse whose species fades stays in the world, in seconds.
///
/// **`TODO(black-box)`**: that some corpses fade is published
/// (`monster_generic`'s `Fade Corpse` spawnflag, modelled per species by
/// `ohl_ai::monsters::MonsterFlags::FADES_CORPSE`); how long the fade waits
/// is not, so this is a project placeholder to be observed.
pub const CORPSE_FADE_SECONDS: f32 = 5.0;

/// The published `monstermaker` `Start On` spawnflag bit.
pub const SPAWNFLAG_MONSTERMAKER_START_ON: u32 = 1;

/// The published `monstermaker` `Cyclic` spawnflag bit.
pub const SPAWNFLAG_MONSTERMAKER_CYCLIC: u32 = 4;

/// The most children every `monstermaker` in one level may create between
/// them, however many of them declare an unlimited `monstercount`.
///
/// **Project-owned, not a published number.** A map may legitimately ask a
/// maker for an unlimited supply; the engine still has to keep one level's
/// entity count bounded, so the whole level shares one ceiling. Reaching it
/// stops further spawns rather than failing, in the same "degrade, don't
/// break" style as the rest of this project's limits.
pub const MAX_MAKER_CHILDREN_PER_LEVEL: u32 = 256;

/// The `monstermaker` classname, whose keyvalues become an
/// [`ohl_ai::Spawner`].
pub const MONSTERMAKER_CLASSNAME: &str = "monstermaker";

/// The published `monster_generic` keyvalue naming the condition that fires
/// [`TRIGGER_TARGET_KEY`].
pub const TRIGGER_CONDITION_KEY: &str = "TriggerCondition";

/// The published `monster_generic` keyvalue naming the entity a fired
/// trigger condition activates.
pub const TRIGGER_TARGET_KEY: &str = "TriggerTarget";

/// How far a monster's hitscan attack reaches when its species' table
/// publishes no range of its own, in world units.
///
/// **`TODO(black-box)`**: a project placeholder, like every other reach in
/// `ohl_ai::monsters::table`.
pub const DEFAULT_ATTACK_RANGE: f32 = 1024.0;

/// How fast a monster's projectile leaves the muzzle, in units per second.
///
/// **`TODO(black-box)`**: project-authored, and only ever handed to a
/// [`ProjectileSpawner`], which M7.9 P3 replaces together with this number.
pub const DEFAULT_PROJECTILE_SPEED: f32 = 1_000.0;

/// What one monster attack resolves to.
///
/// **Project-authored.** Each arm names this project's own `ohl-ai` attack
/// vocabulary and (for the projectile arm) `ohl-combat`'s own
/// [`ohl_combat::ProjectileKind`]; no published table is reproduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackShape {
    /// A short trace from the attacker's eye along its facing.
    Melee,
    /// A trace at the species' published range.
    Hitscan,
    /// A projectile the [`ProjectileSpawner`] creates.
    Projectile(ohl_combat::ProjectileKind),
}

/// How `kind`'s `attack` resolves.
///
/// Melee attacks are melee; ranged attacks are hitscan except for the two
/// kinds whose ranged attack is a visible flying object in the published
/// game (the alien grunt's hornet and the human grunt's thrown grenade).
/// Anything this project has no `ohl-combat` projectile for stays hitscan,
/// which is the conservative choice: it resolves through a trace that
/// already exists rather than silently doing nothing.
#[must_use]
pub fn attack_shape(kind: &MonsterKind, attack: AttackKind) -> AttackShape {
    match attack {
        AttackKind::Melee1 | AttackKind::Melee2 => AttackShape::Melee,
        AttackKind::Range1 | AttackKind::Range2 => match kind {
            MonsterKind::AlienGrunt => AttackShape::Projectile(ohl_combat::ProjectileKind::Hornet),
            MonsterKind::HumanGrunt if attack == AttackKind::Range2 => {
                AttackShape::Projectile(ohl_combat::ProjectileKind::HandGrenade)
            }
            _ => AttackShape::Hitscan,
        },
    }
}

/// One projectile a monster attack asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileRequest {
    /// Which projectile.
    pub kind: ohl_combat::ProjectileKind,
    /// The monster that fired it, so the projectile can ignore its owner.
    pub owner: Entity,
    /// Where it starts, in world units.
    pub origin: Vec3,
    /// Its initial velocity, in units per second.
    pub velocity: Vec3,
}

/// Creates the projectiles monster attacks ask for.
///
/// The seam M7.9 P3 fills: it owns the `ohl_combat::ProjectileSet` and the
/// tuning every kind needs, neither of which this package has. The default
/// [`NoProjectiles`] drops every request, so a ranged monster whose attack
/// is a projectile is harmless rather than broken.
pub trait ProjectileSpawner {
    /// Creates one projectile. Returning is the whole contract: a spawner
    /// that cannot honour a request drops it.
    fn spawn_projectile(&mut self, request: &ProjectileRequest);
}

/// The default [`ProjectileSpawner`]: drops every request.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProjectiles;

impl ProjectileSpawner for NoProjectiles {
    fn spawn_projectile(&mut self, _request: &ProjectileRequest) {}
}

/// Turns one map entity definition into a monster, using the brains
/// [`AiState`] registered for the classnames this map declares.
///
/// A classname with no registered brain — `ohl_ai::MonsterKind::Unknown`,
/// or a kind whose spec the table does not carry — still becomes an
/// [`Actor`], so it is visible, perceivable and shootable, but gets no
/// [`MonsterAi`] and therefore never thinks. That is the documented
/// "inert actor" state, not an error.
struct EngineSpawnRules<'a> {
    brains: &'a BTreeMap<String, BrainId>,
    difficulty: AiDifficulty,
    skill: &'a ohl_campaign::SkillTable,
    campaign_difficulty: ohl_campaign::Difficulty,
}

impl EngineSpawnRules<'_> {
    /// The health `kind` spawns with: its species table's value at this
    /// difficulty, overridden by the map's `skill.cfg` when it publishes
    /// the matching `sk_<subject>_health<N>` cvar.
    fn health_of(&self, kind: &MonsterKind, spec: &MonsterSpec) -> f32 {
        let lookup = |cvar: &str| -> Option<f32> {
            let stem = cvar.trim_end_matches(|byte: char| byte.is_ascii_digit());
            self.skill
                .lookup(stem, self.campaign_difficulty)
                .and_then(|value| value.trim().parse::<f32>().ok())
        };
        spec.resolve_health(kind, self.difficulty, Some(&lookup))
    }
}

impl MonsterSpawnRules for EngineSpawnRules<'_> {
    fn spawn_for(&self, def: &EntityDef) -> Option<MonsterSpawn> {
        if !def.classname.starts_with("monster_") {
            return None;
        }
        let kind = MonsterKind::from_classname(&def.classname);
        // A classname this project has no table row (and so no brain) for
        // is left alone entirely: it keeps whatever the registry built for
        // it and never thinks, which is the documented inert state.
        let spec = spec_for(&kind)?;
        let brain = self.brains.get(&def.classname).copied()?;
        Some(MonsterSpawn::new(spec.classification, brain).with_health(self.health_of(&kind, spec)))
    }
}

/// One monster's declared `TriggerCondition`/`TriggerTarget` pair, kept in
/// spawn order so evaluation never depends on a hash.
struct DeclaredTrigger {
    entity: Entity,
    trigger: MonsterTrigger,
}

/// The AI half of the step list: the world, its brains, and everything that
/// crosses between `ohl-ai` and the rest of the engine.
pub struct AiState {
    world: AiWorld,
    /// `monster_*` classname to the brain registered for it, in sorted
    /// classname order so registration is reproducible.
    brains: BTreeMap<String, BrainId>,
    /// Which [`MonsterKind`] each registered brain drives, indexed by
    /// [`BrainId`], so an attack event can find its species' table without
    /// a second per-entity component.
    brain_kinds: Vec<MonsterKind>,
    /// Damage aimed at monsters, applied once per step in phase 10.
    damage: DamageQueue,
    /// The `TriggerCondition`/`TriggerTarget` pairs this map declared.
    triggers: Vec<DeclaredTrigger>,
    /// How many monsters have died since this level was attached.
    deaths: u64,
    /// How many children every `monstermaker` in this level has created
    /// between them, against [`MAX_MAKER_CHILDREN_PER_LEVEL`].
    maker_children: u32,
    /// The difficulty this level's damage tables are read at.
    difficulty: AiDifficulty,
    /// Empty, and deliberately so: a monster attack resolves against the
    /// enemy `ohl-ai` already chose, and uses this only for the
    /// world-occlusion trace. M7.9 P1 owns the populated index.
    hitboxes: HitboxIndex,
    projectiles: Box<dyn ProjectileSpawner>,
}

impl core::fmt::Debug for AiState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AiState")
            .field("world", &self.world)
            .field("brains", &self.brains.len())
            .finish_non_exhaustive()
    }
}

impl AiState {
    /// An AI world seeded with `seed` and no brains registered yet.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            world: AiWorld::new(seed),
            brains: BTreeMap::new(),
            brain_kinds: Vec::new(),
            damage: DamageQueue::new(),
            triggers: Vec::new(),
            deaths: 0,
            maker_children: 0,
            difficulty: AiDifficulty::default(),
            hitboxes: HitboxIndex::new(HitboxLimits::default()),
            projectiles: Box::new(NoProjectiles),
        }
    }

    /// Installs the spawner monster projectile attacks go through,
    /// replacing [`NoProjectiles`].
    pub fn set_projectile_spawner(&mut self, spawner: Box<dyn ProjectileSpawner>) {
        self.projectiles = spawner;
    }

    /// How many monsters have died since this level was attached.
    ///
    /// `ohl_ai::apply_monster_damage` reports each death exactly once, so
    /// this counts deaths, not killing blows. Media-derived: data, never a
    /// log line.
    #[must_use]
    pub fn death_count(&self) -> u64 {
        self.deaths
    }

    /// The AI world, for a caller that needs to read its state.
    #[must_use]
    pub fn world(&self) -> &AiWorld {
        &self.world
    }

    /// Queues damage against a monster, consumed by the next phase 10.
    ///
    /// This is the only way a monster loses health: the queue is applied by
    /// [`ohl_ai::apply_monster_damage`], which is what guarantees exactly
    /// one death event per monster.
    pub fn queue_damage(&mut self, event: DamageEvent) {
        if !event.is_usable() {
            return;
        }
        self.damage.push_damage(event);
        // The same hit becomes a *condition* on the next tick, which is how
        // a monster reacts to being shot.
        self.world.apply_damage(event);
    }

    /// Builds this level's monsters, `monstermaker`s, declared triggers and
    /// navigation graph, replacing whatever the previous level left.
    pub fn attach_level(
        &mut self,
        level: &mut Level,
        difficulty: ohl_campaign::Difficulty,
        skill: &ohl_campaign::SkillTable,
    ) {
        self.brains.clear();
        self.brain_kinds.clear();
        self.triggers.clear();
        self.damage.clear();
        self.deaths = 0;
        self.maker_children = 0;
        self.world.detach_navigator();
        self.difficulty = ai_difficulty(difficulty);

        self.register_brains(&level.defs);
        let rules = EngineSpawnRules {
            brains: &self.brains,
            difficulty: self.difficulty,
            skill,
            campaign_difficulty: difficulty,
        };
        let defs = std::mem::take(&mut level.defs);
        let spawned = attach_monsters(&mut level.registry, &defs, &rules);
        level.defs = defs;
        // Record the health each monster spawned with, so a later
        // `health_fraction` (and anything M7.9 P1 resolves damage against)
        // reads the value the skill table actually produced rather than
        // re-deriving it from the species table.
        for entity in &spawned {
            let Ok(health) = level
                .registry
                .world
                .get::<&Actor>(*entity)
                .map(|actor| actor.health)
            else {
                continue;
            };
            level
                .registry
                .world
                .insert_one(*entity, ohl_combat::Health::new(health))
                .ok();
        }

        self.collect_triggers(level, &spawned);
        Self::attach_makers(level);
        Self::attach_player_actor(level);

        if let Some(bridge) = nav::build(&level.defs, level.collision.as_ref()) {
            self.world.attach_navigator(bridge);
        }
    }

    /// Registers one brain per distinct `monster_*` classname this map
    /// declares, in sorted classname order.
    fn register_brains(&mut self, defs: &[EntityDef]) {
        // Both the monsters a map places directly and the ones its
        // `monstermaker`s will create later: a maker's `monstertype` needs
        // a brain the moment it fires, and registering it up front keeps
        // brain ids a function of the map alone.
        let mut classnames: Vec<&str> = defs
            .iter()
            .flat_map(|def| {
                [
                    Some(def.classname.as_str()),
                    (def.classname == MONSTERMAKER_CLASSNAME)
                        .then(|| def.keyvalues.get("monstertype").map(|value| value.trim()))
                        .flatten(),
                ]
            })
            .flatten()
            .filter(|classname| classname.starts_with("monster_"))
            .collect();
        classnames.sort_unstable();
        classnames.dedup();
        for classname in classnames {
            let kind = MonsterKind::from_classname(classname);
            let Some(brain) = MonsterBrain::for_kind(kind.clone()) else {
                continue;
            };
            let id = self.world.register_brain(Box::new(brain));
            debug_assert_eq!(id.0, self.brain_kinds.len());
            self.brain_kinds.push(kind);
            self.brains.insert(classname.to_string(), id);
        }
    }

    /// Reads the `TriggerCondition`/`TriggerTarget` pair off every monster
    /// that declares one, in spawn order.
    fn collect_triggers(&mut self, level: &Level, spawned: &[Entity]) {
        for (index, def) in level.defs.iter().enumerate() {
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            if !spawned.contains(&entity) {
                continue;
            }
            let Some(condition) = def
                .keyvalues
                .get(TRIGGER_CONDITION_KEY)
                .and_then(|value| value.trim().parse::<u8>().ok())
                .and_then(trigger_condition_of)
            else {
                continue;
            };
            let Some(target) = def
                .keyvalues
                .get(TRIGGER_TARGET_KEY)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            self.triggers.push(DeclaredTrigger {
                entity,
                trigger: MonsterTrigger::new(condition, target),
            });
        }
    }

    /// Turns every `monstermaker` definition into a [`MonsterMaker`]
    /// component on the entity the registry already spawned for it.
    fn attach_makers(level: &mut Level) {
        for (index, def) in level.defs.iter().enumerate() {
            if def.classname != MONSTERMAKER_CLASSNAME {
                continue;
            }
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            let monstertype = def
                .keyvalues
                .get("monstertype")
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            if monstertype.is_empty() {
                continue;
            }
            let monstercount = def
                .keyvalues
                .get("monstercount")
                .and_then(|value| value.trim().parse::<i32>().ok())
                .unwrap_or(ohl_ai::spawner::UNLIMITED);
            let delay = def
                .keyvalues
                .get("delay")
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|delay| delay.is_finite())
                .unwrap_or(0.0);
            let max_live = def
                .keyvalues
                .get("m_imaxlivechildren")
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(0);
            let spawner = ohl_ai::Spawner::new(
                monstertype,
                monstercount,
                delay,
                max_live,
                def.spawnflags & SPAWNFLAG_MONSTERMAKER_START_ON != 0,
                def.spawnflags & SPAWNFLAG_MONSTERMAKER_CYCLIC != 0,
            );
            level
                .registry
                .world
                .insert_one(entity, MonsterMaker(spawner))
                .ok();
        }
    }

    /// Gives the client entity its [`Actor`], so monsters perceive, target
    /// and shoot the player through the components they use for each other.
    fn attach_player_actor(level: &mut Level) {
        let player = level.player;
        let origin = level
            .registry
            .world
            .get::<&Transform>(player)
            .map(|transform| transform.origin)
            .unwrap_or_default();
        let health = level
            .registry
            .world
            .get::<&ohl_combat::Health>(player)
            .map_or(crate::level::PLAYER_MAX_HEALTH, |health| health.current);
        let actor = Actor::new(Classification::Player, origin)
            .as_client()
            .with_health(health);
        level.registry.world.insert_one(player, actor).ok();
    }

    /// How many entities in this level currently carry a thinking
    /// [`MonsterAi`]. Media-derived: data, never a log line.
    #[must_use]
    pub fn monster_count(&self, level: &Level) -> usize {
        level.registry.world.query::<&MonsterAi>().iter().count()
    }

    /// A digest of the whole AI simulation, for determinism tests.
    #[must_use]
    pub fn state_hash(&self, level: &Level) -> [u8; 32] {
        self.world.state_hash(&level.registry.world)
    }

    /// Phase 8 — one AI think step, and the attacks it produced.
    ///
    /// The navigator, when this map has one, is already attached, so path
    /// following happens inside `AiWorld::tick`.
    pub fn think(&mut self, level: &mut Level, dt: f32, damage: &mut Vec<QueuedDamage>) {
        let events = {
            let context = SightContext {
                collision: level.collision.as_ref(),
                world: Some(&level.world),
            };
            self.world.tick(&mut level.registry.world, &context, dt)
        };
        self.consume_events(level, &events, damage);
    }

    /// Turns this step's [`AiEvent`]s into animation, damage and projectile
    /// requests.
    fn consume_events(
        &mut self,
        level: &mut Level,
        events: &[AiEvent],
        damage: &mut Vec<QueuedDamage>,
    ) {
        for event in events {
            match &event.kind {
                AiEventKind::ActivityChanged(activity) => {
                    select_sequence(level, event.entity, &activity_name(*activity));
                }
                AiEventKind::PlaySequence(name) => {
                    select_sequence(level, event.entity, name);
                }
                AiEventKind::Attack { kind, target } => {
                    self.resolve_attack(level, event.entity, *kind, *target, damage);
                }
                _ => {}
            }
        }
    }

    /// The species table row driving `entity`, when it has a brain.
    fn spec_of(
        &self,
        level: &Level,
        entity: Entity,
    ) -> Option<(MonsterKind, &'static MonsterSpec)> {
        let brain = level
            .registry
            .world
            .get::<&MonsterAi>(entity)
            .ok()
            .map(|ai| ai.brain)?;
        let kind = self.brain_kinds.get(brain.0)?.clone();
        let spec = spec_for(&kind)?;
        Some((kind, spec))
    }

    /// Maps one [`AiEventKind::Attack`] onto a trace or a projectile.
    fn resolve_attack(
        &mut self,
        level: &mut Level,
        attacker: Entity,
        attack: AttackKind,
        target: Option<Entity>,
        damage: &mut Vec<QueuedDamage>,
    ) {
        let Some((kind, spec)) = self.spec_of(level, attacker) else {
            return;
        };
        let Ok(actor) = level.registry.world.get::<&Actor>(attacker).map(|a| *a) else {
            return;
        };
        let shape = attack_shape(&kind, attack);
        let (amount, range) = match shape {
            AttackShape::Melee => match spec.melee {
                Some(melee) => (melee.damage[self.difficulty.index()], melee.range),
                None => return,
            },
            AttackShape::Hitscan | AttackShape::Projectile(_) => match spec.ranged {
                Some(ranged) => (
                    ranged.damage[self.difficulty.index()],
                    if ranged.range > 0.0 {
                        ranged.range
                    } else {
                        DEFAULT_ATTACK_RANGE
                    },
                ),
                None => return,
            },
        };

        let muzzle = actor.eye();
        let aim = target
            .and_then(|entity| {
                level
                    .registry
                    .world
                    .get::<&Actor>(entity)
                    .ok()
                    .map(|a| a.eye())
            })
            .unwrap_or_else(|| muzzle + actor.forward() * range);

        if let AttackShape::Projectile(projectile) = shape {
            let direction = (aim - muzzle).normalize_or_zero();
            self.projectiles.spawn_projectile(&ProjectileRequest {
                kind: projectile,
                owner: attacker,
                origin: muzzle,
                velocity: direction * DEFAULT_PROJECTILE_SPEED,
            });
            return;
        }

        let Some(target) = target else {
            return;
        };
        let to_target = aim - muzzle;
        if to_target.length() > range.max(0.0) {
            return;
        }
        // Only the world blocks: the enemy is the one `ohl-ai` already
        // chose by relationship and line of sight, so there is nothing for
        // a hitbox refinement to decide. M7.9 P1's populated index makes
        // this a per-hitbox trace without changing the call.
        if let Some(collision) = level.collision.as_ref() {
            let trace = ohl_combat::trace_attack_filtered(
                collision,
                &self.hitboxes,
                muzzle,
                aim,
                TraceFilter::ignoring(TraceMask::WORLD_ONLY, entity_id(attacker)),
            );
            if trace.fraction < 1.0 {
                return;
            }
        }
        damage.push(QueuedDamage {
            target,
            info: ohl_combat::DamageInfo {
                attacker: Some(entity_id(attacker)),
                inflictor: Some(entity_id(attacker)),
                amount,
                kind: attack_damage_type(shape),
                origin: muzzle,
                direction: to_target.normalize_or_zero(),
            },
        });
    }

    /// Phase 10 — deaths, corpses, gibs, declared triggers and
    /// `monstermaker`s.
    pub fn lifecycle(&mut self, level: &mut Level, dt: f32, damage: &mut Vec<QueuedDamage>) {
        self.drain_engine_damage(level, damage);
        let (events, corpses) = ohl_ai::monsters::lifecycle::apply_damage_with_corpses(
            &mut level.registry.world,
            &self.damage,
            ohl_ai::monsters::lifecycle::DEFAULT_GIB_OVERKILL_MULTIPLIER,
        );
        let hurt: Vec<Entity> = self
            .damage
            .events()
            .iter()
            .map(|event| event.target)
            .collect();
        self.damage.clear();

        self.deaths += events
            .iter()
            .filter(|event| event.kind == AiEventKind::Died)
            .count() as u64;
        let died: Vec<Entity> = events
            .iter()
            .filter(|event| event.kind == AiEventKind::Died)
            .map(|event| event.entity)
            .collect();

        // Triggers fire *before* the remains are dealt with: a gibbed
        // monster is despawned outright, and a despawned entity has no
        // state left for `TriggerCondition::Death` to read.
        self.fire_triggers(level, &hurt, &died);
        for (entity, decision) in &corpses {
            self.retire(level, *entity, *decision);
        }
        Self::age_corpses(level, dt);
        self.tick_makers(level, dt);
    }

    /// Moves whatever the damage phase left in the engine's queue into the
    /// AI's own, so a monster loses health however the hit was produced.
    ///
    /// M7.9 P1 owns phase 9 and forwards the hits it resolves through
    /// [`Self::queue_damage`]; until it lands, the queue still holds them
    /// here, and draining it is what keeps this package testable on its
    /// own. Either way each hit is applied exactly once.
    fn drain_engine_damage(&mut self, level: &Level, damage: &mut Vec<QueuedDamage>) {
        if damage.is_empty() {
            return;
        }
        let pending = std::mem::take(damage);
        for queued in pending {
            if level
                .registry
                .world
                .get::<&MonsterAi>(queued.target)
                .is_err()
            {
                continue;
            }
            let attacker = queued.info.attacker.and_then(crate::ids::entity_of);
            self.queue_damage(DamageEvent {
                target: queued.target,
                attacker,
                amount: queued.info.amount,
                source_position: queued.info.origin,
                provokes: attacker.is_some(),
            });
        }
    }

    /// Applies one death's corpse decision: a gib leaves nothing, a corpse
    /// stays (and fades when its species does).
    fn retire(&self, level: &mut Level, entity: Entity, decision: CorpseDecision) {
        let fades = self
            .spec_of(level, entity)
            .is_some_and(|(_, spec)| ohl_ai::monsters::lifecycle::should_fade_corpse(spec));
        level.registry.world.remove_one::<MonsterAi>(entity).ok();
        match decision {
            CorpseDecision::Gib => {
                level.registry.world.despawn(entity).ok();
            }
            CorpseDecision::Corpse => {
                let seconds_left = if fades {
                    CORPSE_FADE_SECONDS
                } else {
                    f32::INFINITY
                };
                level
                    .registry
                    .world
                    .insert_one(entity, Corpse { seconds_left })
                    .ok();
            }
        }
    }

    /// Evaluates every declared `TriggerCondition` and fires the ones that
    /// came true through the map-logic simulation.
    fn fire_triggers(&mut self, level: &mut Level, hurt: &[Entity], died: &[Entity]) {
        let mut triggers = std::mem::take(&mut self.triggers);
        for declared in &mut triggers {
            let entity = declared.entity;
            let Ok(actor) = level.registry.world.get::<&Actor>(entity).map(|a| *a) else {
                continue;
            };
            let conditions = level
                .registry
                .world
                .get::<&MonsterAi>(entity)
                .map_or(Conditions::EMPTY, |ai| ai.conditions);
            // The health this monster actually spawned with, skill-table
            // override included; the species table is only a fallback for
            // an entity that somehow has no `Health` of its own.
            let max_health = level
                .registry
                .world
                .get::<&ohl_combat::Health>(entity)
                .map(|health| health.max)
                .ok()
                .or_else(|| {
                    self.spec_of(level, entity)
                        .map(|(_, spec)| spec.health[self.difficulty.index()])
                })
                .unwrap_or(actor.health);
            let context = TriggerContext {
                sees_player: conditions.contains(Conditions::SEE_CLIENT),
                hostile_to_player: sees_player(level, entity),
                took_damage: hurt.contains(&entity),
                health_fraction: health_fraction(actor.health, max_health),
                died: died.contains(&entity),
                heard_world: conditions.contains(Conditions::HEAR_SOUND),
                // `ohl-ai` classifies sounds as world, danger or combat and
                // does not model "heard the player" separately, so this
                // published condition has no input yet.
                // `TODO(black-box)`: what marks a sound as the player's.
                heard_player: false,
                heard_combat: conditions.contains(Conditions::HEAR_COMBAT),
            };
            if declared.trigger.check(context) {
                level
                    .simulation
                    .fire(declared.trigger.target.clone(), Some(entity), 0.0);
            }
        }
        self.triggers = triggers;
    }

    /// Ages every fading corpse and removes the ones whose time is up.
    fn age_corpses(level: &mut Level, dt: f32) {
        let mut expired = Vec::new();
        for (entity, corpse) in &mut level.registry.world.query::<(Entity, &mut Corpse)>() {
            if !corpse.seconds_left.is_finite() {
                continue;
            }
            corpse.seconds_left -= dt;
            if corpse.seconds_left <= 0.0 {
                expired.push(entity);
            }
        }
        expired.sort_unstable_by_key(|entity: &Entity| entity.id());
        for entity in expired {
            level.registry.world.despawn(entity).ok();
        }
    }

    /// Advances every `monstermaker` and spawns the children it asks for.
    fn tick_makers(&mut self, level: &mut Level, dt: f32) {
        let mut makers: Vec<Entity> = level
            .registry
            .world
            .query::<(Entity, &MonsterMaker)>()
            .iter()
            .map(|(entity, _)| entity)
            .collect();
        makers.sort_unstable_by_key(|entity: &Entity| entity.id());

        for maker in makers {
            if self.maker_children >= MAX_MAKER_CHILDREN_PER_LEVEL {
                return;
            }
            let (wants_spawn, classname, origin, yaw) = {
                let alive = |entity: Entity| {
                    level
                        .registry
                        .world
                        .get::<&Actor>(entity)
                        .is_ok_and(|actor| actor.alive)
                };
                let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(maker) else {
                    continue;
                };
                let wants = component.0.tick(dt, &alive);
                let transform = level
                    .registry
                    .world
                    .get::<&Transform>(maker)
                    .map_or((Vec3::ZERO, 0.0), |transform| {
                        (transform.origin, transform.angles.y)
                    });
                (
                    wants,
                    component.0.monster_classname.clone(),
                    transform.0,
                    transform.1,
                )
            };
            if !wants_spawn {
                continue;
            }
            let Some(child) = self.spawn_child(level, maker, &classname, origin, yaw) else {
                continue;
            };
            self.maker_children += 1;
            if let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(maker) {
                component.0.note_spawned(child);
            }
        }
    }

    /// Spawns one `monstermaker` child, or `None` when this map registered
    /// no brain for its `monstertype`.
    fn spawn_child(
        &self,
        level: &mut Level,
        maker: Entity,
        classname: &str,
        origin: Vec3,
        yaw: f32,
    ) -> Option<Entity> {
        let brain = *self.brains.get(classname)?;
        let kind = self.brain_kinds.get(brain.0)?.clone();
        let spec = spec_for(&kind)?;
        let health = spec.health[self.difficulty.index()];
        let mut actor = Actor::new(spec.classification, origin).with_health(health);
        actor.yaw = yaw;
        actor.hull = spec.hull;
        Some(level.registry.world.spawn((
            ClassName(classname.to_string()),
            Transform {
                origin,
                angles: Vec3::new(0.0, yaw, 0.0),
            },
            actor,
            MonsterAi::new(brain),
            ohl_combat::Health::new(health),
            Owner(maker),
        )))
    }
}

/// Whether `entity`'s acquired enemy is the client.
fn sees_player(level: &Level, entity: Entity) -> bool {
    level
        .registry
        .world
        .get::<&MonsterAi>(entity)
        .ok()
        .and_then(|ai| ai.enemy())
        .is_some_and(|enemy| enemy == level.player)
}

/// `health` as a fraction of `max`, clamped, and `0.0` for corrupt state.
fn health_fraction(health: f32, max: f32) -> f32 {
    if !health.is_finite() || !max.is_finite() || max <= 0.0 {
        return 0.0;
    }
    (health / max).clamp(0.0, 1.0)
}

/// The damage type a monster attack deals.
///
/// **Project-authored**, and deliberately coarse: `ohl-combat`'s damage
/// vocabulary is a bitmask over published names, and which bit each
/// monster's attack sets is a black-box observation nobody has made yet.
/// Melee is a slash, everything else a bullet.
fn attack_damage_type(shape: AttackShape) -> DamageType {
    match shape {
        AttackShape::Melee => DamageType::SLASH,
        AttackShape::Hitscan | AttackShape::Projectile(_) => DamageType::BULLET,
    }
}

/// The `ohl-ai` difficulty matching the campaign's.
fn ai_difficulty(difficulty: ohl_campaign::Difficulty) -> AiDifficulty {
    match difficulty {
        ohl_campaign::Difficulty::Easy => AiDifficulty::Easy,
        ohl_campaign::Difficulty::Medium => AiDifficulty::Medium,
        ohl_campaign::Difficulty::Hard => AiDifficulty::Hard,
    }
}

/// The published `TriggerCondition` value `raw` names, or `None` when the
/// map declared a number outside the documented set.
fn trigger_condition_of(raw: u8) -> Option<TriggerCondition> {
    Some(match raw {
        0 => TriggerCondition::None,
        1 => TriggerCondition::SeePlayerMadAtPlayer,
        2 => TriggerCondition::TakeDamage,
        3 => TriggerCondition::HalfHealthRemaining,
        4 => TriggerCondition::Death,
        5 => TriggerCondition::Unconfirmed5,
        6 => TriggerCondition::Unconfirmed6,
        7 => TriggerCondition::HearWorld,
        8 => TriggerCondition::HearPlayer,
        9 => TriggerCondition::HearCombat,
        10 => TriggerCondition::SeePlayerUnconditional,
        _ => return None,
    })
}

/// The name an [`ohl_ai::Activity`] is looked up under in a model's own
/// sequence table.
///
/// The string is `ohl-ai`'s own variant name, lower-cased — this project's
/// animation vocabulary, not a game's. No sequence name is written into
/// this crate: the *match* happens against whatever the loaded model
/// publishes, and a model that publishes nothing by that name simply keeps
/// sequence 0.
fn activity_name(activity: ohl_ai::Activity) -> String {
    format!("{activity:?}").to_ascii_lowercase()
}

/// Points `entity`'s [`StudioAnim`] at the sequence its model publishes
/// under `name`, or leaves it on sequence 0.
fn select_sequence(level: &mut Level, entity: Entity, name: &str) {
    let Ok(mut anim) = level.registry.world.get::<&mut StudioAnim>(entity) else {
        return;
    };
    let sequence = level
        .studio_models
        .get(anim.model)
        .and_then(|model| model.sequence_by_name(name))
        .unwrap_or(0);
    anim.play(sequence);
}

#[cfg(test)]
mod tests {
    use super::{AttackShape, activity_name, attack_shape, trigger_condition_of};
    use ohl_ai::{Activity, AttackKind, MonsterKind, TriggerCondition};

    #[test]
    fn melee_attacks_are_always_traces() {
        for kind in MonsterKind::defined() {
            assert_eq!(
                attack_shape(kind, AttackKind::Melee1),
                AttackShape::Melee,
                "every melee attack resolves as a trace"
            );
        }
    }

    #[test]
    fn only_the_two_project_chosen_kinds_fire_projectiles() {
        let projectiles = MonsterKind::defined()
            .iter()
            .filter(|kind| {
                matches!(
                    attack_shape(kind, AttackKind::Range1),
                    AttackShape::Projectile(_)
                )
            })
            .count();
        assert_eq!(projectiles, 1, "only the alien grunt's primary is a hornet");
        assert!(matches!(
            attack_shape(&MonsterKind::HumanGrunt, AttackKind::Range2),
            AttackShape::Projectile(_)
        ));
        assert_eq!(
            attack_shape(&MonsterKind::HumanGrunt, AttackKind::Range1),
            AttackShape::Hitscan
        );
    }

    #[test]
    fn an_unknown_classname_still_maps_to_a_shape() {
        let unknown = MonsterKind::from_classname("monster_not_in_the_table");
        assert_eq!(
            attack_shape(&unknown, AttackKind::Range1),
            AttackShape::Hitscan
        );
        assert_eq!(
            attack_shape(&unknown, AttackKind::Melee2),
            AttackShape::Melee
        );
    }

    #[test]
    fn activity_names_come_from_the_ai_crates_own_vocabulary() {
        assert_eq!(activity_name(Activity::Walk), "walk");
        assert_eq!(activity_name(Activity::Idle), "idle");
    }

    #[test]
    fn trigger_conditions_outside_the_documented_set_are_rejected() {
        assert_eq!(trigger_condition_of(4), Some(TriggerCondition::Death));
        assert_eq!(trigger_condition_of(11), None);
        assert_eq!(trigger_condition_of(255), None);
    }
}
