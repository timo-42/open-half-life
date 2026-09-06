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
use ohl_ai::follow::{FollowChange, FollowRoster, Follower};
use ohl_ai::monsters::spec_for;
use ohl_ai::monsters::table::Difficulty as AiDifficulty;
use ohl_ai::scripts::{ScriptAction, ScriptHold, ScriptRunner, ScriptSense};
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
use ohl_game::scripts::{ScriptActivation, ScriptDef, SentenceDef};

use crate::components::{Corpse, MonsterMaker, Owner, StudioAnim};
use crate::ids::entity_id;
use crate::level::Level;
use crate::nav;
use crate::systems::QueuedDamage;
use crate::text::SentenceLookup;

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
    /// How many monster damage events have been applied since this level
    /// was attached. Counts events, not monsters: a monster hit twice in
    /// one step counts twice. Used only by the scripted-input smoke's
    /// "A monster took damage." milestone line (`ohl-app`); never logged
    /// from here, per this crate's logging policy.
    damage_events: u64,
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
    /// This level's `scripted_sequence`/`aiscripted_sequence` entities, in
    /// spawn order.
    scripts: Vec<ActiveScript>,
    /// This level's `scripted_sentence` entities, in spawn order.
    sentences: Vec<ActiveSentence>,
    /// Who is following the player, oldest first.
    followers: FollowRoster,
    /// The `sentences.txt` lookup a `scripted_sentence` resolves against.
    /// Never logged; see [`AiState::speak`].
    sentence_lookup: SentenceLookup,
    /// A player `use` this step's phase 8 has not consumed yet, at the
    /// position it was pressed from.
    pending_use: Option<Vec3>,
    /// How many scripted sequences have started since this level was
    /// attached. Data, never a log line from this crate.
    script_starts: u64,
    /// How many scripted sequences have completed their action animation
    /// since this level was attached.
    script_completions: u64,
    /// How many sentence word slots have been resolved. The words
    /// themselves name assets and never leave this function; see
    /// [`AiState::speak`].
    sentence_words: u64,
    /// Sound cues produced by `scripted_sentence`, drained by
    /// [`crate::Game::tick`].
    sound_cues: Vec<ohl_gameplay::SoundCue>,
    /// Bumped whenever this level's set of monsters changes (a level
    /// attach, a `monstermaker` child). An unbound script only re-runs its
    /// classname search when this moves, so a map full of scripts that name
    /// a monster it never spawns costs one world sweep per spawn rather
    /// than one per step.
    spawn_generation: u64,
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
            damage_events: 0,
            maker_children: 0,
            difficulty: AiDifficulty::default(),
            hitboxes: HitboxIndex::new(HitboxLimits::default()),
            projectiles: Box::new(NoProjectiles),
            scripts: Vec::new(),
            sentences: Vec::new(),
            followers: FollowRoster::new(),
            sentence_lookup: SentenceLookup::default(),
            pending_use: None,
            script_starts: 0,
            script_completions: 0,
            sentence_words: 0,
            sound_cues: Vec::new(),
            spawn_generation: 0,
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

    /// How many monster damage events have been applied since this level
    /// was attached. Media-derived: data, never a log line.
    #[must_use]
    pub fn damage_event_count(&self) -> u64 {
        self.damage_events
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
        self.damage_events = 0;
        self.maker_children = 0;
        self.scripts.clear();
        self.sentences.clear();
        self.followers.clear();
        self.pending_use = None;
        self.script_starts = 0;
        self.script_completions = 0;
        self.sentence_words = 0;
        self.sound_cues.clear();
        self.spawn_generation = self.spawn_generation.wrapping_add(1);
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
        self.attach_scripts(level);
        Self::attach_followers(level, &spawned);
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
        // Scripts and followers decide before the brains do: both work by
        // setting up the very same route, move target and pending
        // conditions `AiWorld::tick` is about to read, so a scripted or
        // following monster is driven by the existing think step rather
        // than beside it.
        self.update_scripts(level, dt);
        self.update_followers(level);
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
        self.damage_events += hurt.len() as u64;
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
        self.retire_followers(&died);
        self.speak(level, dt);
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
                // TODO(P1): a hit aimed at a non-monster target (the player
                // included) is discarded here rather than applied, because
                // phase 9 (damage resolution, `Systems::resolve_damage`) is
                // still an empty hook. Until M7.9 P1 lands that phase, "the
                // player took damage" is not an observable engine event, so
                // the scripted-input smoke's milestone line for it is not
                // wired; see `ohl-app/src/script_log.rs`.
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
            // A new monster exists, so every unbound script gets one more
            // chance to find its `m_iszEntity` target.
            self.spawn_generation = self.spawn_generation.wrapping_add(1);
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

// --- M7.11: scripted sequences, talk monsters and sentences -----------------

/// How close to a script's mark counts as having arrived, in world units.
///
/// **`TODO(black-box)`**: no public page states an arrival tolerance.
pub const SCRIPT_ARRIVE_RADIUS: f32 = 24.0;

/// How close a monster's yaw must be to a script's before "Turn to Face"
/// is satisfied, in degrees.
///
/// **`TODO(black-box)`**: project-authored, like every other AI tolerance.
pub const SCRIPT_FACING_TOLERANCE_DEGREES: f32 = 5.0;

/// How fast a scripted monster turns toward the script's facing, in degrees
/// per second.
///
/// **`TODO(black-box)`**: project-authored.
pub const SCRIPT_TURN_RATE_DEGREES: f32 = 360.0;

/// How long an action animation this project cannot resolve is assumed to
/// last, in seconds.
///
/// A script whose `m_iszPlay` names no sequence the monster's model
/// publishes — or whose monster has no model loaded at all — still has to
/// finish, or its `target` would never fire. **`TODO(black-box)`**:
/// project-authored, and only ever used when the real duration is unknown.
pub const SCRIPT_FALLBACK_ACTION_SECONDS: f32 = 1.0;

/// How far from the player a talk monster may be and still be brought into
/// the player's group with `use`, in world units.
///
/// **`TODO(black-box)`**: no public page states a use range for a talk
/// monster; this matches the engine's own [`crate::systems::USE_RADIUS`]
/// for doors and buttons so one press cannot mean two different reaches.
pub const TALK_USE_RADIUS: f32 = crate::USE_RADIUS;

/// The published talk-monster classnames that can be asked to follow.
pub const TALK_MONSTER_CLASSNAMES: [&str; 2] = ["monster_barney", "monster_scientist"];

/// One `scripted_sequence`/`aiscripted_sequence` in the current level.
struct ActiveScript {
    /// The script entity itself.
    entity: Entity,
    /// Its state machine.
    runner: ScriptRunner,
    /// The monster it has chosen, once one exists.
    actor: Option<Entity>,
    /// The spawn generation the last unsuccessful target search ran
    /// against, so an unbound script does not sweep the world every step.
    /// A search only repeats once something new has spawned — or, once the
    /// script has been triggered, on every step until it finds a monster,
    /// which is the published "the first monster to enter the radius will
    /// follow the sequence".
    searched_generation: Option<u64>,
    /// An activation received while no monster was bound yet.
    pending_trigger: bool,
    /// Whether the runner was holding its monster on the previous step, so
    /// the engine can act on the *transition* out of possession rather
    /// than reaching into an actor it does not own on every step.
    was_active: bool,
    /// Seconds the action animation has been playing.
    played: f32,
    /// Where the action animation started, for `No Script Movement`.
    play_origin: Vec3,
}

/// One `scripted_sentence` in the current level.
struct ActiveSentence {
    /// The sentence entity.
    entity: Entity,
    /// Its published keyvalues.
    def: SentenceDef,
    /// Seconds before the speaker may be asked again.
    cooldown: f32,
    /// Whether a `Fire Once` sentence has already played.
    spent: bool,
}

impl AiState {
    /// Installs the `sentences.txt` lookup a `scripted_sentence` resolves
    /// against. Called by [`crate::Game`], which owns the loaded file.
    pub fn set_sentence_lookup(&mut self, lookup: SentenceLookup) {
        self.sentence_lookup = lookup;
    }

    /// How many scripted sequences currently possess a monster.
    #[must_use]
    pub fn active_script_count(&self) -> usize {
        self.scripts
            .iter()
            .filter(|script| script.runner.is_active())
            .count()
    }

    /// How many scripted sequences have started since this level was
    /// attached. Data, never a log line.
    #[must_use]
    pub fn script_start_count(&self) -> u64 {
        self.script_starts
    }

    /// How many scripted sequences have finished their action animation
    /// since this level was attached. Data, never a log line.
    #[must_use]
    pub fn script_completion_count(&self) -> u64 {
        self.script_completions
    }

    /// How many `sentences.txt` word slots a `scripted_sentence` has
    /// resolved. A count, never the words: they name assets.
    #[must_use]
    pub fn spoken_word_count(&self) -> u64 {
        self.sentence_words
    }

    /// Who is following the player, oldest first.
    #[must_use]
    pub fn followers(&self) -> &FollowRoster {
        &self.followers
    }

    /// Records a player `use` pressed from `position`, for the next phase 8
    /// to offer to a nearby talk monster.
    pub fn queue_use(&mut self, position: Vec3) {
        self.pending_use = Some(position);
    }

    /// Takes the sound cues `scripted_sentence` produced since the last
    /// call.
    pub fn drain_sound_cues(&mut self) -> Vec<ohl_gameplay::SoundCue> {
        std::mem::take(&mut self.sound_cues)
    }

    /// Reads every `scripted_sequence`, `aiscripted_sequence` and
    /// `scripted_sentence` this map declared and gives each the
    /// [`ScriptActivation`] counter the map-logic simulation bumps.
    fn attach_scripts(&mut self, level: &mut Level) {
        for (index, def) in level.defs.iter().enumerate() {
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            if let Some(script) = ScriptDef::from_def(def) {
                level
                    .registry
                    .world
                    .insert_one(entity, ScriptActivation::default())
                    .ok();
                self.scripts.push(ActiveScript {
                    entity,
                    runner: ScriptRunner::new(script),
                    actor: None,
                    searched_generation: None,
                    pending_trigger: false,
                    was_active: false,
                    played: 0.0,
                    play_origin: Vec3::ZERO,
                });
            } else if let Some(sentence) = SentenceDef::from_def(def) {
                level
                    .registry
                    .world
                    .insert_one(entity, ScriptActivation::default())
                    .ok();
                self.sentences.push(ActiveSentence {
                    entity,
                    def: sentence,
                    cooldown: 0.0,
                    spent: false,
                });
            }
        }
    }

    /// Gives every talk monster this map spawned its [`Follower`] state,
    /// reading the published `Pre-Disaster` spawnflag out of its definition.
    fn attach_followers(level: &mut Level, spawned: &[Entity]) {
        for (index, def) in level.defs.iter().enumerate() {
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            if !spawned.contains(&entity)
                || !TALK_MONSTER_CLASSNAMES.contains(&def.classname.as_str())
            {
                continue;
            }
            level
                .registry
                .world
                .insert_one(entity, Follower::from_spawnflags(def.spawnflags))
                .ok();
        }
    }

    /// Advances every script by one step, part of phase 8.
    fn update_scripts(&mut self, level: &mut Level, dt: f32) {
        let mut scripts = std::mem::take(&mut self.scripts);
        for script in &mut scripts {
            self.update_one_script(level, script, dt);
        }
        self.scripts = scripts;
    }

    fn update_one_script(&mut self, level: &mut Level, script: &mut ActiveScript, dt: f32) {
        let mut activated = false;
        if let Ok(activation) = level
            .registry
            .world
            .query_one_mut::<&mut ScriptActivation>(script.entity)
        {
            while activation.take() {
                activated = true;
            }
        }
        script.pending_trigger |= activated;

        if script.actor.is_none()
            && (script.pending_trigger || script.searched_generation != Some(self.spawn_generation))
        {
            script.actor = find_script_actor(level, script.runner.def());
            script.searched_generation = Some(self.spawn_generation);
        }
        let Some(actor) = script
            .actor
            .filter(|actor| level.registry.world.contains(*actor))
        else {
            return;
        };

        if script.pending_trigger {
            if Self::may_possess(level, script, actor) && script.runner.trigger() {
                self.script_starts += 1;
                script.played = 0.0;
            }
            // Spent either way: an activation a monster refuses (it is in
            // combat and this script does not `Override AI`) is dropped
            // rather than queued. Only "no monster bound yet" banks it, so
            // a script can still wait for `m_iszEntity` to spawn.
            script.pending_trigger = false;
        }

        // A script that is not holding its monster still advances — a
        // `m_flRepeat` wait is its own business — but it is told nothing
        // about the actor and, in `apply_script_step`, allowed to touch
        // nothing of it. Only a holding script reads the world.
        let sense = if script.runner.is_active() {
            Self::sense_script(level, script, actor, dt)
        } else {
            ScriptSense {
                dt,
                ..ScriptSense::default()
            }
        };
        let step = script.runner.update(&sense);
        self.apply_script_step(level, script, actor, step, dt);
    }

    /// Whether `script` may take `actor` over right now.
    ///
    /// Published: `Override AI` (and every `aiscripted_sequence`) "will
    /// possess its target even when the monster is in the combat state at
    /// the moment of the call". Without it, a monster already in combat is
    /// left alone.
    fn may_possess(level: &Level, script: &ActiveScript, actor: Entity) -> bool {
        if script.runner.def().overrides_ai() {
            return true;
        }
        level
            .registry
            .world
            .get::<&MonsterAi>(actor)
            .is_ok_and(|ai| ai.state != ohl_ai::MonsterState::Combat)
            || level.registry.world.get::<&MonsterAi>(actor).is_err()
    }

    /// Reads what the script's state machine needs to know about `actor`.
    fn sense_script(level: &Level, script: &ActiveScript, actor: Entity, dt: f32) -> ScriptSense {
        let def = script.runner.def();
        let (origin, yaw) = level
            .registry
            .world
            .get::<&Actor>(actor)
            .map_or((Vec3::ZERO, 0.0), |a| (a.origin, a.yaw));
        let flat = Vec3::new(origin.x - def.origin.x, origin.y - def.origin.y, 0.0);
        let disturbed = level
            .registry
            .world
            .get::<&MonsterAi>(actor)
            .is_ok_and(|ai| {
                ai.conditions
                    .intersects(Conditions::ALL_DAMAGE.union(Conditions::NEW_ENEMY))
            });
        ScriptSense {
            dt,
            at_mark: flat.length() <= SCRIPT_ARRIVE_RADIUS,
            facing_mark: ohl_ai::movement::normalize_yaw(def.yaw - yaw).abs()
                <= SCRIPT_FACING_TOLERANCE_DEGREES,
            sequence_finished: script.played >= Self::action_seconds(level, script, actor),
            disturbed,
        }
    }

    /// How long `script`'s action animation lasts for `actor`.
    fn action_seconds(level: &Level, script: &ActiveScript, actor: Entity) -> f32 {
        let Some(name) = script.runner.def().play_sequence() else {
            // "the Action Animation is not specified": the target fires as
            // soon as the monster has moved to the script.
            return 0.0;
        };
        let resolved = level
            .registry
            .world
            .get::<&StudioAnim>(actor)
            .ok()
            .and_then(|anim| {
                let model = level.studio_models.get(anim.model)?;
                let index = model.sequence_by_name(name)?;
                model
                    .sequences
                    .get(index)
                    .map(ohl_world::StudioSequence::duration)
            });
        match resolved {
            Some(duration) if duration > 0.0 => duration,
            _ => SCRIPT_FALLBACK_ACTION_SECONDS,
        }
    }

    /// Carries out what the script decided.
    fn apply_script_step(
        &mut self,
        level: &mut Level,
        script: &mut ActiveScript,
        actor: Entity,
        step: ohl_ai::ScriptStep,
        dt: f32,
    ) {
        if script.runner.is_active() {
            level.registry.world.insert_one(actor, ScriptHold).ok();
            // Driving the actor is gated on the script actually holding
            // it. `ScriptAction::None` — a dormant, waiting, finished or
            // just-released script — reaches into nothing.
            Self::drive_actor(level, script, actor, step.action, dt);
        }
        self.finish_script_step(level, script, actor, step);
    }

    /// Carries out the one thing a *holding* script asked for this step.
    fn drive_actor(
        level: &mut Level,
        script: &mut ActiveScript,
        actor: Entity,
        action: ScriptAction,
        dt: f32,
    ) {
        // The mark and the facing come from the definition; nothing here
        // needs to own a copy of it.
        let (mark, yaw) = {
            let def = script.runner.def();
            (def.origin, def.yaw)
        };
        match action {
            ScriptAction::None => {}
            ScriptAction::Idle => {
                stop_scripted_movement(level, actor);
                Self::play_idle(level, script, actor);
            }
            ScriptAction::Approach { run } => {
                let speed = if run {
                    SCRIPT_RUN_SPEED
                } else {
                    SCRIPT_WALK_SPEED
                };
                if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(actor) {
                    if ai.route.is_finished() || ai.route.needs_refresh(mark) {
                        ai.route = ohl_ai::Route::straight_line(mark);
                        ai.stuck.reset();
                    }
                    ai.move_target = Some(mark);
                    ai.move_speed = speed;
                }
                Self::play_idle(level, script, actor);
            }
            ScriptAction::Teleport => {
                stop_scripted_movement(level, actor);
                place(level, actor, mark, yaw);
                Self::play_idle(level, script, actor);
            }
            ScriptAction::Face => {
                stop_scripted_movement(level, actor);
                if let Ok(mut a) = level.registry.world.get::<&mut Actor>(actor) {
                    let (turned, _) =
                        ohl_ai::movement::turn_toward(a.yaw, yaw, SCRIPT_TURN_RATE_DEGREES * dt);
                    a.yaw = turned;
                }
                Self::play_idle(level, script, actor);
            }
            ScriptAction::Play => {
                stop_scripted_movement(level, actor);
                if script.played <= 0.0 {
                    script.play_origin = level
                        .registry
                        .world
                        .get::<&Actor>(actor)
                        .map_or(Vec3::ZERO, |a| a.origin);
                    let name = script
                        .runner
                        .def()
                        .play_sequence()
                        .map(std::string::ToString::to_string);
                    if let Some(name) = name {
                        select_sequence(level, actor, &name);
                    }
                }
                script.played += dt.max(0.0);
            }
        }
    }

    /// Fires `target`/`killtarget` on completion and hands the monster
    /// back on the transition out of possession.
    fn finish_script_step(
        &mut self,
        level: &mut Level,
        script: &mut ActiveScript,
        actor: Entity,
        step: ohl_ai::ScriptStep,
    ) {
        if step.completed {
            self.script_completions += 1;
            let (no_script_movement, yaw, target, delay, kill_target) = {
                let def = script.runner.def();
                (
                    def.no_script_movement(),
                    def.yaw,
                    def.target.clone(),
                    def.delay,
                    def.kill_target.clone(),
                )
            };
            if no_script_movement {
                let origin = script.play_origin;
                place(level, actor, origin, yaw);
            }
            if !target.is_empty() {
                level.simulation.fire(target, Some(actor), delay);
            }
            if !kill_target.is_empty() {
                let doomed: Vec<Entity> = level.registry.find(&kill_target).to_vec();
                for entity in doomed {
                    level.registry.world.despawn(entity).ok();
                }
            }
        }

        // The one place the script gives the monster back: exactly once, on
        // the transition out of possession, never on the steps in between.
        if script.was_active && !script.runner.is_active() {
            script.played = 0.0;
            level.registry.world.remove_one::<ScriptHold>(actor).ok();
            stop_scripted_movement(level, actor);
            if script.runner.def().leaves_corpse()
                && let Ok(mut corpse) = level.registry.world.get::<&mut Corpse>(actor)
            {
                corpse.seconds_left = f32::INFINITY;
            }
        }
        script.was_active = script.runner.is_active();
    }

    /// Points `actor` at this script's idle animation, when the map named a
    /// working one. Only called while the script holds `actor`.
    fn play_idle(level: &mut Level, script: &ActiveScript, actor: Entity) {
        let idle = script
            .runner
            .def()
            .idle_sequence()
            .map(std::string::ToString::to_string);
        if let Some(idle) = idle {
            select_sequence(level, actor, &idle);
        }
    }

    /// Offers a queued player `use` to the nearest talk monster and keeps
    /// every follower pointed at the player. Part of phase 8.
    fn update_followers(&mut self, level: &mut Level) {
        if let Some(position) = self.pending_use.take()
            && let Some(entity) = nearest_follower(level, position)
        {
            let mut follower = level
                .registry
                .world
                .get::<&Follower>(entity)
                .map(|f| *f)
                .unwrap_or_default();
            let change = self.followers.toggle(entity, &mut follower);
            if let Ok(mut slot) = level.registry.world.get::<&mut Follower>(entity) {
                *slot = follower;
            }
            if let FollowChange::Started {
                evicted: Some(evicted),
            } = change
                && let Ok(mut slot) = level.registry.world.get::<&mut Follower>(evicted)
            {
                slot.following = false;
            }
        }

        let player = level.player;
        let Ok(origin) = level
            .registry
            .world
            .get::<&Actor>(player)
            .map(|actor| actor.origin)
        else {
            return;
        };
        for entity in self.followers.members().to_vec() {
            if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(entity) {
                // `SPECIAL2` plus a move target is exactly what
                // `ohl_ai::monsters::brains::FOLLOW_PLAYER` — the schedule
                // Barney and the scientist already select — reads.
                ai.pending_conditions |= Conditions::SPECIAL2;
                ai.move_target = Some(origin);
            }
        }
    }

    /// Drops dead or despawned allies out of the player's group. Part of
    /// phase 10.
    fn retire_followers(&mut self, died: &[Entity]) {
        for entity in died {
            self.followers.remove(*entity);
        }
    }

    /// Advances every `scripted_sentence`. Part of phase 10.
    ///
    /// The resolved word list names sound assets; per `docs/CLEAN_ROOM.md`
    /// rule 7 no such path may enter this project's source, a cue or a
    /// diagnostic, so only its *length* is kept and the cue's path is
    /// always `None` — the same policy `ohl_gameplay::sounds` already
    /// applies to every other sound this engine asks for. An empty group
    /// and a resolved one therefore produce the same cue.
    fn speak(&mut self, level: &mut Level, dt: f32) {
        let mut sentences = std::mem::take(&mut self.sentences);
        for sentence in &mut sentences {
            sentence.cooldown = (sentence.cooldown - dt).max(0.0);
            let mut activated = false;
            if let Ok(activation) = level
                .registry
                .world
                .query_one_mut::<&mut ScriptActivation>(sentence.entity)
            {
                while activation.take() {
                    activated = true;
                }
            }
            if !activated || sentence.spent || sentence.cooldown > 0.0 {
                continue;
            }
            let origin = level
                .registry
                .world
                .get::<&Transform>(sentence.entity)
                .map_or(Vec3::ZERO, |transform| transform.origin);
            let Some(speaker) = find_speaker(level, &sentence.def, origin) else {
                // Published: `refire` is the delay before trying to find
                // the speaker again.
                sentence.cooldown = sentence.def.refire;
                continue;
            };
            if sentence.def.followers_only() && !self.followers.is_following(speaker) {
                sentence.cooldown = sentence.def.refire;
                continue;
            }
            let words = self.sentence_lookup.words(&sentence.def.sentence);
            self.sentence_words += words.len() as u64;
            #[allow(clippy::cast_possible_truncation)]
            self.sound_cues.push(ohl_gameplay::SoundCue {
                entity: entity_id(speaker).0 as u32,
                class: ohl_gameplay::ChannelClass::Voice,
                path: None,
            });
            sentence.cooldown = sentence.def.duration;
            sentence.spent = sentence.def.fire_once();
            if !sentence.def.target.is_empty() {
                level.simulation.fire(
                    sentence.def.target.clone(),
                    Some(speaker),
                    sentence.def.delay,
                );
            }
        }
        self.sentences = sentences;
    }
}

/// How fast a scripted monster walks and runs to its mark, in units per
/// second.
///
/// **`TODO(black-box)`**: the same provisional pair `ohl_ai::Brain::speeds`
/// publishes, reused so a scripted walk and an unscripted one move alike.
pub const SCRIPT_WALK_SPEED: f32 = 40.0;

/// See [`SCRIPT_WALK_SPEED`].
pub const SCRIPT_RUN_SPEED: f32 = 160.0;

/// Stops whatever route a scripted monster was following.
fn stop_scripted_movement(level: &mut Level, actor: Entity) {
    if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(actor) {
        ai.move_speed = 0.0;
        ai.route = ohl_ai::Route::new();
        ai.stuck.reset();
    }
}

/// Puts `actor` at `origin`, facing `yaw`, in both the transform the
/// renderer reads and the actor the AI reads.
fn place(level: &mut Level, actor: Entity, origin: Vec3, yaw: f32) {
    if let Ok(mut transform) = level.registry.world.get::<&mut Transform>(actor) {
        transform.origin = origin;
        transform.angles.y = yaw;
    }
    if let Ok(mut a) = level.registry.world.get::<&mut Actor>(actor) {
        a.origin = origin;
        a.yaw = yaw;
    }
}

/// The monster a script's `m_iszEntity` names: a `targetname` first, then
/// the nearest live actor of that classname inside `m_flRadius`.
///
/// Ties are broken by entity id, so which monster a script picks never
/// depends on iteration order.
fn find_script_actor(level: &Level, def: &ScriptDef) -> Option<Entity> {
    if def.target_monster.is_empty() {
        return None;
    }
    if let Some(named) = level
        .registry
        .find(&def.target_monster)
        .iter()
        .copied()
        .find(|entity| level.registry.world.get::<&Actor>(*entity).is_ok())
    {
        return Some(named);
    }
    nearest_by_classname(level, &def.target_monster, def.origin, def.radius)
}

/// The nearest live actor whose classname is `classname` and that is within
/// `radius` of `origin` (any distance when `radius` is zero).
fn nearest_by_classname(
    level: &Level,
    classname: &str,
    origin: Vec3,
    radius: f32,
) -> Option<Entity> {
    let mut candidates: Vec<(f32, u32, Entity)> = Vec::new();
    for (entity, name, actor) in &mut level.registry.world.query::<(Entity, &ClassName, &Actor)>() {
        if name.0 != classname || !actor.alive {
            continue;
        }
        let distance = actor.origin.distance(origin);
        if !distance.is_finite() || (radius > 0.0 && distance > radius) {
            continue;
        }
        candidates.push((distance, entity.id(), entity));
    }
    closest(&mut candidates)
}

/// The nearest of `candidates`, ties broken by entity id so the choice
/// never depends on iteration order.
fn closest(candidates: &mut [(f32, u32, Entity)]) -> Option<Entity> {
    candidates
        .sort_unstable_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
    candidates.first().map(|(_, _, entity)| *entity)
}

/// The entity a `scripted_sentence`'s `entity` keyvalue names.
///
/// Published: a `targetname` matches at any distance, a classname only
/// inside `radius`, measured from the `scripted_sentence` itself.
fn find_speaker(level: &Level, def: &SentenceDef, origin: Vec3) -> Option<Entity> {
    if def.speaker.is_empty() {
        return None;
    }
    if let Some(named) = level
        .registry
        .find(&def.speaker)
        .iter()
        .copied()
        .find(|entity| level.registry.world.get::<&Actor>(*entity).is_ok())
    {
        return Some(named);
    }
    nearest_by_classname(level, &def.speaker, origin, def.radius)
}

/// The nearest talk monster to `position` that is close enough to `use`.
fn nearest_follower(level: &Level, position: Vec3) -> Option<Entity> {
    let mut candidates: Vec<(f32, u32, Entity)> = Vec::new();
    for (entity, actor, _) in &mut level.registry.world.query::<(Entity, &Actor, &Follower)>() {
        if !actor.alive {
            continue;
        }
        let distance = actor.origin.distance(position);
        if !distance.is_finite() || distance > TALK_USE_RADIUS {
            continue;
        }
        candidates.push((distance, entity.id(), entity));
    }
    closest(&mut candidates)
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
