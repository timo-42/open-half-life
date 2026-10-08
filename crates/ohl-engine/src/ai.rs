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
    Activity, Actor, AiEvent, AiEventKind, AiWorld, AttackKind, BrainId, Classification,
    Conditions, CorpseDecision, DamageEvent, DamageKinds, DamageQueue, DamageResponse, DamageSink,
    EnemyMemory, MonsterAi, MonsterBrain, MonsterKind, MonsterSpawn, MonsterSpawnRules,
    MonsterSpec, MonsterState, MonsterTrigger, Prisoner, Route, ScheduleRunner, SightContext,
    SquadTag, StuckDetector, TriggerCondition, TriggerContext, attach_monsters,
    sighting_relationship,
};
use ohl_combat::{DamageType, HitboxIndex, HitboxLimits, TraceFilter, TraceMask};
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::EntityDef;
use ohl_game::registry::{ClassName, MakerActivation, Transform};
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

/// The published `monster_generic` `Not solid` spawnflag bit: the prop
/// is "impervious to any damage" (`docs/FORMAT_SOURCES.md`, "Monster
/// definitions"; see [`ohl_ai::Impervious`] for what that does to sight).
/// Only the cited immunity is modeled: the prop keeps its hitbox, so a
/// shot still stops at it and reports a hit — whether a trace should pass
/// through such a prop is not something the cited page states, and is
/// recorded as a gap in `docs/MILESTONES.md` rather than guessed. Read only for
/// `monster_generic`, the one classname the cited page documents it on;
/// on every other monster bit 4 is the unrelated `MonsterClip` flag.
pub const SPAWNFLAG_GENERIC_NOT_SOLID: u32 = 4;

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

/// How far below its placed origin a spawning monster looks for a floor
/// to stand on, in world units.
///
/// The Quake-family `droptofloor` page: "Tries to move an entity to the
/// nearest floor beneath it, up to 256 map units", "mostly for snapping
/// item and monsters to the ground at map start" (`docs/FORMAT_SOURCES.md`,
/// "Monsters stand on their floor").
pub const MONSTER_DROP_DISTANCE: f32 = 256.0;

/// The two `monster_generic` models whose origin is the model's centre
/// rather than its feet. The cited `monster_generic` page: "The model's
/// origin should be on the ground", with the "Sole exceptions" of these
/// two, "which have their centered origins (like deathmatch player models)
/// adjusted automatically" (`docs/FORMAT_SOURCES.md`, "Monsters stand on
/// their floor"). A prop with one of them is neither lifted nor dropped.
pub const CENTRED_ORIGIN_MODELS: [&str; 2] = ["models/player.mdl", "models/holo.mdl"];

/// How far above its placed feet a spawning monster's floor search
/// starts, in world units, so a monster placed exactly on (or a hair
/// inside) its floor is not read as starting in solid.
///
/// **Project-authored**, the same one unit `ohl_nav`'s ground clearance
/// uses for the same reason.
pub const MONSTER_DROP_CLEARANCE: f32 = 1.0;

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

/// Physical attack classification, independent of its resolved damage profile.
#[must_use]
pub fn attack_shape(kind: &MonsterKind, attack: AttackKind) -> AttackShape {
    use ohl_combat::ProjectileKind as P;
    if matches!(attack, AttackKind::Melee1 | AttackKind::Melee2) {
        return AttackShape::Melee;
    }
    let projectile = match kind {
        MonsterKind::AlienGrunt => P::Hornet,
        MonsterKind::HumanGrunt | MonsterKind::HumanAssassin if attack == AttackKind::Range2 => {
            P::HandGrenade
        }
        MonsterKind::Bullsquid => P::BullsquidSpit,
        MonsterKind::AlienController if attack == AttackKind::Range2 => P::ControllerHomingBall,
        MonsterKind::AlienController => P::ControllerBall,
        MonsterKind::Apache if attack == AttackKind::Range2 => P::Rocket,
        MonsterKind::BigMomma => P::GonarchMortar,
        _ => return AttackShape::Hitscan,
    };
    AttackShape::Projectile(projectile)
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
    /// Damage resolved at launch, independent of subsequent shooter changes.
    pub damage: f32,
    /// Project-authored damage classification.
    pub damage_type: DamageType,
    /// Radius for explosive attacks; `None` means a direct impact.
    pub blast_radius: Option<f32>,
    /// Homing target, if the physical kind supports one.
    pub target: Option<Entity>,
}

/// Borrowed actual phase-5 geometry and projectile policy; never retained in AI/save state.
pub(crate) struct GrenadeSafetyContext<'a> {
    pub(crate) projectiles: &'a crate::projectiles::ProjectileSystem,
    pub(crate) hitboxes: &'a ohl_combat::HitboxIndex,
}

/// Creates the projectiles monster attacks ask for.
///
/// The normal engine sink queues bounded value requests for Systems to drain
/// after phase 8. Custom sinks can implement their own delivery; the explicit
/// [`NoProjectiles`] sink discards requests.
pub trait ProjectileSpawner {
    /// Creates one projectile. Returning is the whole contract: a spawner
    /// that cannot honour a request drops it.
    fn spawn_projectile(&mut self, request: &ProjectileRequest);
    /// Drains value requests for the engine-owned simulation. Custom sinks may retain delivery.
    fn take_requests(&mut self) -> Vec<ProjectileRequest> {
        Vec::new()
    }
}

/// An explicit [`ProjectileSpawner`] that drops every request.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProjectiles;

impl ProjectileSpawner for NoProjectiles {
    fn spawn_projectile(&mut self, _request: &ProjectileRequest) {}
}

/// A bounded value queue; never owns another projectile simulation.
#[derive(Default)]
struct QueuedProjectiles {
    requests: Vec<ProjectileRequest>,
}

impl ProjectileSpawner for QueuedProjectiles {
    fn spawn_projectile(&mut self, request: &ProjectileRequest) {
        if self.requests.len() < ohl_combat::ProjectileLimits::default().max_projectiles
            && request.origin.is_finite()
            && request.velocity.is_finite()
            && request.damage.is_finite()
            && request.damage >= 0.0
            && request
                .blast_radius
                .is_none_or(|radius| radius.is_finite() && radius > 0.0)
        {
            self.requests.push(*request);
        }
    }
    fn take_requests(&mut self) -> Vec<ProjectileRequest> {
        std::mem::take(&mut self.requests)
    }
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
        // The species' own hull and eye offset: the hull is what decides
        // whether it walks or flies (`ohl_ai::movement::flies`), and the
        // eye is where its sight and attack traces start (a barnacle's
        // points down, since its origin is at the ceiling).
        Some(
            MonsterSpawn::new(spec.classification, brain)
                .with_health(self.health_of(&kind, spec))
                .with_hull(spec.hull)
                .with_view_ofs(kind.view_offset())
                .with_difficulty(self.difficulty),
        )
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
    replan_restored_routes: bool,
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
    secondary_cooldowns: BTreeMap<Entity, f32>,
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
    /// How many scripted sequences have given up on ever satisfying
    /// `ScriptPhase::Moving`'s mark condition and released their monster
    /// (`ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS`), since this level
    /// was attached. A counted reason, never a log line: see that
    /// constant's doc comment for why the bound exists.
    script_timeouts: u64,
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
    pub(crate) fn snapshot_secondary_cooldowns(&self, level: &Level) -> Vec<(u32, f32)> {
        let mut entries: Vec<_> = self
            .secondary_cooldowns
            .iter()
            .filter(|(_, left)| **left > 0.0 && left.is_finite())
            .filter_map(|(entity, left)| {
                crate::save_state::spawn_index_of(level, *entity).map(|index| (index, *left))
            })
            .take(crate::save_state::MAX_SNAPSHOT_ENTITIES)
            .collect();
        entries.sort_by_key(|entry| entry.0);
        entries
    }

    pub(crate) fn restore_secondary_cooldowns(&mut self, level: &Level, entries: &[(u32, f32)]) {
        self.secondary_cooldowns.clear();
        for &(index, left) in entries
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_ENTITIES)
        {
            if let Some(entity) = crate::save_state::entity_at_spawn_index(level, index)
                && self.spec_of(level, entity).is_some()
            {
                // Invalid entries suppress immediate attack until the normal project cooldown passes.
                let left = if left.is_finite() {
                    left.clamp(0.0, 6.0)
                } else {
                    6.0
                };
                self.secondary_cooldowns.entry(entity).or_insert(left);
            }
        }
    }

    pub(crate) fn take_projectile_requests(&mut self) -> Vec<ProjectileRequest> {
        self.projectiles.take_requests()
    }

    /// An AI world seeded with `seed` and no brains registered yet.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            world: AiWorld::new(seed),
            brains: BTreeMap::new(),
            brain_kinds: Vec::new(),
            replan_restored_routes: false,
            damage: DamageQueue::new(),
            triggers: Vec::new(),
            deaths: 0,
            damage_events: 0,
            maker_children: 0,
            difficulty: AiDifficulty::default(),
            hitboxes: HitboxIndex::new(HitboxLimits::default()),
            projectiles: Box::<QueuedProjectiles>::default(),
            secondary_cooldowns: BTreeMap::new(),
            scripts: Vec::new(),
            sentences: Vec::new(),
            followers: FollowRoster::new(),
            sentence_lookup: SentenceLookup::default(),
            pending_use: None,
            script_starts: 0,
            script_completions: 0,
            script_timeouts: 0,
            sentence_words: 0,
            sound_cues: Vec::new(),
            spawn_generation: 0,
        }
    }

    /// Installs the sole sink monster projectile attacks go through,
    /// replacing the engine's bounded value queue.
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
    /// was attached: hits that cost their target something, not one its
    /// species shrugs off or its shield takes. Media-derived: data, never a
    /// log line.
    #[must_use]
    pub fn damage_event_count(&self) -> u64 {
        self.damage_events
    }

    /// The AI world, for a caller that needs to read its state.
    #[must_use]
    pub fn world(&self) -> &AiWorld {
        &self.world
    }

    /// `SECTION_RNG` (27, M7.9 P4b): this world's own random stream and
    /// tick counter, which `Systems::new`'s deterministic reseeding
    /// (`AiState::new`'s constructor argument, itself a draw off
    /// `Systems::rng`) does not otherwise let a save continue past — see
    /// `ohl_ai::AiWorld::rng_snapshot`'s own doc comment.
    #[must_use]
    pub(crate) fn rng_snapshot(&self) -> (u64, u64) {
        self.world.rng_snapshot()
    }

    /// The tick counter half of the same snapshot; `ohl_ai::AiWorld::
    /// state_hash` mixes it in, so a save omitting it would desync the
    /// very digest a determinism test compares.
    #[must_use]
    pub(crate) fn tick_count(&self) -> u64 {
        self.world.tick_count()
    }

    /// Restores what [`Self::rng_snapshot`]/[`Self::tick_count`] captured.
    pub(crate) fn restore_rng(&mut self, rng: (u64, u64), tick_count: u64) {
        self.world.restore_determinism_state(rng, tick_count);
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
        self.secondary_cooldowns.clear();
        self.projectiles.take_requests();
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
        self.script_timeouts = 0;
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
        Self::configure_actor_models(level);
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
        for entity in &spawned {
            let centred = level
                .registry
                .entities
                .iter()
                .position(|spawned| spawned == entity)
                .and_then(|index| level.defs.get(index))
                .is_some_and(has_centred_origin);
            let kind = level
                .registry
                .world
                .get::<&ClassName>(*entity)
                .map(|name| MonsterKind::from_classname(&name.0));
            if let (false, Ok(kind)) = (centred, kind) {
                stand_on_floor(level, *entity, &kind);
            }
        }

        self.collect_triggers(level, &spawned);
        Self::mark_not_solid(level, &spawned);
        self.attach_scripts(level);
        Self::attach_followers(level, &spawned);
        Self::attach_makers(level);
        Self::attach_player_actor(level);

        if let Some(bridge) = nav::build(&level.defs, level.monster_collision.as_ref()) {
            self.world.attach_navigator(bridge);
        }
    }

    /// Rebuilds model-local eye and collision-frame metadata. This is derived
    /// runtime state, shared by map attachment and save reconstruction.
    pub(crate) fn configure_actor_models(level: &mut Level) {
        let centred: Vec<Entity> = level
            .defs
            .iter()
            .zip(&level.registry.entities)
            .filter(|(def, _)| has_centred_origin(def))
            .map(|(_, entity)| *entity)
            .collect();
        for (entity, actor, classname, anim) in
            &mut level
                .registry
                .world
                .query::<(Entity, &mut Actor, &ClassName, Option<&StudioAnim>)>()
        {
            let kind = MonsterKind::from_classname(&classname.0);
            let model = anim.and_then(|anim| level.studio_models.get(anim.model));
            actor.configure_model(&kind, model);
            // Project-authored: the named centered convention also governs missing
            // metadata; valid model eye precedence stays intact. TODO(black-box).
            if kind == MonsterKind::Generic && centred.contains(&entity) {
                actor.body_frame = ohl_ai::BodyFrame::Centered;
                actor.view_ofs = actor.body_frame.eye_offset(actor.hull, model);
            }
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
    /// that declares one, in spawn order — except the kinds whose own page
    /// says the pair does not work on them (`ohl_ai::MonsterKind::
    /// honours_trigger_condition`: the Nihilanth, the Apache, the Osprey).
    fn collect_triggers(&mut self, level: &Level, spawned: &[Entity]) {
        for (index, def) in level.defs.iter().enumerate() {
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            if !spawned.contains(&entity)
                || !MonsterKind::from_classname(&def.classname).honours_trigger_condition()
            {
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

    /// Marks every `monster_generic` spawned with its `Not solid`
    /// spawnflag ([`SPAWNFLAG_GENERIC_NOT_SOLID`]) with
    /// [`ohl_ai::Impervious`], so the damage drain drops hits at it and no
    /// monster's sight takes it for an enemy.
    fn mark_not_solid(level: &mut Level, spawned: &[Entity]) {
        for (index, def) in level.defs.iter().enumerate() {
            let Some(entity) = level.registry.entities.get(index).copied() else {
                break;
            };
            if !spawned.contains(&entity)
                || MonsterKind::from_classname(&def.classname) != MonsterKind::Generic
                || def.spawnflags & SPAWNFLAG_GENERIC_NOT_SOLID == 0
            {
                continue;
            }
            level
                .registry
                .world
                .insert_one(entity, ohl_ai::Impervious)
                .ok();
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
            // `monstermaker` rides the same `target`-firing path every
            // other logic entity uses; `ohl-game`'s `Simulation::activate`
            // bumps this counter, and `Self::tick_makers` below drains it.
            level
                .registry
                .world
                .insert_one(entity, MakerActivation::default())
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

    /// The eye position of whichever spawned, living monster (an entity
    /// carrying both [`MonsterAi`] and [`Actor`]) sits closest to `from`,
    /// or `None` when this level has none.
    ///
    /// Additive and data-only: like [`Self::monster_count`], this returns a
    /// position for a caller to act on and never logs a classname, entity
    /// id or coordinate itself. Ties (equal distance) are broken by
    /// ascending [`hecs::Entity::id`], matching this module's usual
    /// deterministic entity ordering, so the result never depends on
    /// query iteration order.
    #[must_use]
    pub fn nearest_monster_position(&self, level: &Level, from: Vec3) -> Option<Vec3> {
        level
            .registry
            .world
            .query::<(Entity, &Actor, &MonsterAi)>()
            .iter()
            .filter(|(_, actor, _)| actor.alive)
            .map(|(entity, actor, _)| (entity, actor.eye()))
            .min_by(|(entity_a, position_a), (entity_b, position_b)| {
                let distance_a = position_a.distance_squared(from);
                let distance_b = position_b.distance_squared(from);
                distance_a
                    .partial_cmp(&distance_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| entity_a.id().cmp(&entity_b.id()))
            })
            .map(|(_, position)| position)
    }

    /// Every living monster this level holds that regards the player as an
    /// enemy, as (entity, eye position), in ascending [`hecs::Entity::id`]
    /// order.
    ///
    /// "Hostile" is read through the same rule a monster's own enemy
    /// acquisition reads: [`ohl_ai::sighting_relationship`] from the
    /// monster's classification to [`Classification::Player`], hostile per
    /// [`ohl_ai::Relationship::is_hostile`]. Nothing here is a second
    /// hostility rule, so a monster carrying the published `Prisoner`
    /// spawnflag ([`ohl_ai::Prisoner`]) — one that never takes the player
    /// as its enemy — is never listed, however its class regards the
    /// player.
    ///
    /// Additive and data-only, like [`Self::nearest_monster_position`]: a
    /// caller may aim at what this returns, and this method never logs a
    /// classname, entity id or coordinate.
    #[must_use]
    pub fn hostile_monster_eyes(&self, level: &Level) -> Vec<(Entity, Vec3)> {
        let relationships = self.world.relationships();
        let mut hostile: Vec<(Entity, Vec3)> = level
            .registry
            .world
            .query::<(Entity, &Actor, &MonsterAi, Option<&Prisoner>)>()
            .iter()
            .filter(|(_, actor, _, _)| actor.alive)
            .filter(|(_, actor, _, prisoner)| {
                sighting_relationship(
                    relationships,
                    actor.classification,
                    Classification::Player,
                    prisoner.is_some(),
                )
                .is_hostile()
            })
            .map(|(entity, actor, _, _)| (entity, actor.eye()))
            .collect();
        hostile.sort_by_key(|(entity, _)| entity.id());
        hostile
    }

    /// A digest of the whole AI simulation, for determinism tests.
    #[must_use]
    pub fn state_hash(&self, level: &Level) -> [u8; 32] {
        let mut hash = ohl_core::StreamingSha256::new();
        hash.update(&self.world.state_hash(&level.registry.world));
        // FollowRoster's private storage contains at most MAX_FOLLOWERS (two) members.
        let follower_count: u32 = self.followers.members().iter().map(|_| 1_u32).sum();
        hash.update(&follower_count.to_le_bytes());
        for entity in self.followers.members() {
            hash.update(&entity.to_bits().get().to_le_bytes());
        }
        hash.finalize()
    }

    /// Separate optional continuation; frozen AI snapshot encoding is unchanged.
    pub(crate) fn snapshot_follow_navigation(
        &self,
        level: &Level,
    ) -> crate::save::FollowNavigationSnapshot {
        use crate::save::{FollowAttemptSnapshot, FollowMemberSnapshot, FollowNavigationSnapshot};
        FollowNavigationSnapshot {
            version: 1,
            members: self
                .followers
                .members()
                .iter()
                .map(|entity| {
                    FollowMemberSnapshot {
                        // A stale unindexed membership cannot silently become another actor.
                        spawn_index: crate::save_state::spawn_index_of(level, *entity)
                            .unwrap_or(u32::MAX),
                        attempt: level
                            .registry
                            .world
                            .get::<&MonsterAi>(*entity)
                            .ok()
                            .and_then(|ai| ai.follow_attempt)
                            .map(|attempt| FollowAttemptSnapshot {
                                accepted_player_anchor: attempt.accepted_player_anchor.to_array(),
                                phase: attempt.phase as u8,
                            }),
                    }
                })
                .collect(),
        }
    }

    /// Validate the complete optional section before changing roster, flags or attempts.
    pub(crate) fn restore_follow_navigation(
        &mut self,
        level: &mut Level,
        save: &crate::GameSave,
    ) -> crate::Result<()> {
        use ohl_ai::follow::{FollowAttempt, FollowPhase};
        let Some(state) = &save.follow_navigation else {
            return Ok(());
        };
        if !state.within_limits() {
            return Err(crate::EngineError::SaveUnreadable);
        }
        let mut staged = Vec::with_capacity(state.members.len());
        for member in &state.members {
            let entity = crate::save_state::entity_at_spawn_index(level, member.spawn_index)
                .ok_or(crate::EngineError::SaveUnreadable)?;
            let follower = level
                .registry
                .world
                .get::<&Follower>(entity)
                .map_err(|_| crate::EngineError::SaveUnreadable)?;
            if !follower.can_follow {
                return Err(crate::EngineError::SaveUnreadable);
            }
            let (attempt, runner) = if let Some(saved) = member.attempt {
                let snapshot = save
                    .ai
                    .as_ref()
                    .and_then(|rows| rows.get(member.spawn_index as usize))
                    .and_then(Option::as_ref)
                    .ok_or(crate::EngineError::SaveUnreadable)?;
                let schedule = &ohl_ai::monsters::brains::FOLLOW_PLAYER;
                if snapshot.schedule_name != schedule.name
                    || snapshot.task_index as usize >= schedule.tasks.len()
                    || !snapshot.schedule_timer.is_finite()
                    || snapshot.schedule_timer < 0.0
                {
                    return Err(crate::EngineError::SaveUnreadable);
                }
                let runner = ScheduleRunner::restore_resolved(
                    schedule,
                    snapshot.task_index as usize,
                    snapshot.schedule_started,
                    snapshot.schedule_timer,
                );
                let ai = level
                    .registry
                    .world
                    .get::<&MonsterAi>(entity)
                    .map_err(|_| crate::EngineError::SaveUnreadable)?;
                let actor = level
                    .registry
                    .world
                    .get::<&Actor>(entity)
                    .map_err(|_| crate::EngineError::SaveUnreadable)?;
                let anchor = Vec3::from_array(saved.accepted_player_anchor);
                let phase =
                    FollowPhase::from_tag(saved.phase).ok_or(crate::EngineError::SaveUnreadable)?;
                let delta =
                    actor.body_frame.anchor_to_query(actor.hull, anchor) - actor.query_origin();
                let shape_valid = match phase {
                    FollowPhase::Preparing => ai.move_speed == 0.0,
                    FollowPhase::Moving => !ai.route.is_finished() && ai.move_speed > 0.0,
                    FollowPhase::Arrived => ai.route.is_finished() && ai.move_speed == 0.0,
                    FollowPhase::Holding => {
                        ai.route.waypoints.is_empty()
                            && ai.move_speed == 0.0
                            && ai.stuck.ticks() == 0
                    }
                };
                if !shape_valid
                    || !delta.is_finite()
                    || !delta.length().is_finite()
                    || !snapshot.move_speed.is_finite()
                    || snapshot.move_speed < 0.0
                    || snapshot.route_current as usize > snapshot.route_waypoints.len()
                    || !snapshot.route_goal.iter().all(|v| v.is_finite())
                    || !snapshot
                        .route_waypoints
                        .iter()
                        .flatten()
                        .all(|v| v.is_finite())
                    || level.registry.world.get::<&ScriptHold>(entity).is_ok()
                {
                    return Err(crate::EngineError::SaveUnreadable);
                }
                (
                    Some(FollowAttempt {
                        accepted_player_anchor: anchor,
                        phase,
                    }),
                    Some(runner),
                )
            } else {
                (None, None)
            };
            staged.push((entity, attempt, runner));
        }
        self.apply_follow_navigation(level, staged);
        Ok(())
    }

    fn apply_follow_navigation(
        &mut self,
        level: &mut Level,
        staged: Vec<(
            Entity,
            Option<ohl_ai::follow::FollowAttempt>,
            Option<ScheduleRunner>,
        )>,
    ) {
        use ohl_ai::follow::FollowInput;
        // Commit only after every member and associated AI state validated.
        self.followers.clear();
        for follower in &mut level.registry.world.query::<&mut Follower>() {
            follower.following = false;
        }
        for ai in &mut level.registry.world.query::<&mut MonsterAi>() {
            ai.follow_input = FollowInput::Unavailable;
            ai.follow_attempt = None;
        }
        for (entity, attempt, runner) in staged {
            if let Ok(mut follower) = level.registry.world.get::<&mut Follower>(entity) {
                self.followers.toggle(entity, &mut follower);
            }
            if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(entity) {
                ai.follow_attempt = attempt;
                if let Some(runner) = runner {
                    ai.runner = runner;
                }
            }
        }
    }

    /// Captures `entity`'s `SECTION_AI` (25) entry, or `None` when it
    /// carries no [`MonsterAi`].
    #[must_use]
    pub(crate) fn snapshot_entity(
        level: &Level,
        entity: Entity,
    ) -> Option<crate::save_state::AiSnapshot> {
        let ai = level.registry.world.get::<&MonsterAi>(entity).ok()?;
        let enemy = ai.memory.and_then(|memory| {
            crate::save_state::spawn_index_of(level, memory.entity).map(|index| {
                crate::save_state::EnemyMemorySnapshot {
                    enemy: index,
                    last_known_position: crate::save_state::vec3_array(memory.last_known_position),
                    time_since_seen: memory.time_since_seen,
                    occluded: memory.occluded,
                    last_known_distance: memory.last_known_distance,
                }
            })
        });
        let squad = level
            .registry
            .world
            .get::<&SquadTag>(entity)
            .ok()
            .map(|tag| crate::save_state::SquadSnapshot {
                name: tag.name.clone(),
                leader: tag.leader,
            });
        Some(crate::save_state::AiSnapshot {
            state_tag: ai.state.tag(),
            conditions: ai.conditions.bits(),
            pending_conditions: ai.pending_conditions.bits(),
            schedule_name: ai.runner.schedule_name().to_string(),
            task_index: u32::try_from(ai.runner.task_index()).unwrap_or(u32::MAX),
            schedule_started: ai.runner.started(),
            schedule_timer: ai.runner.timer(),
            enemy,
            route_waypoints: ai
                .route
                .waypoints
                .iter()
                .map(|waypoint| crate::save_state::vec3_array(*waypoint))
                .collect(),
            route_current: u32::try_from(ai.route.current).unwrap_or(u32::MAX),
            route_goal: crate::save_state::vec3_array(ai.route.goal),
            move_target: ai.move_target.map(crate::save_state::vec3_array),
            cover: ai.cover.map(crate::save_state::vec3_array),
            activity_tag: ai.activity.tag(),
            move_speed: ai.move_speed,
            ideal_yaw: ai.ideal_yaw,
            stuck_ticks: ai.stuck.ticks(),
            squad,
        })
    }

    /// Restores `entity`'s `SECTION_AI` (25) entry. A no-op when `entity`
    /// carries no [`MonsterAi`] (the save named an entity this level no
    /// longer spawns a monster at — the registry shape changed, or the
    /// entry was `None` to begin with).
    pub(crate) fn restore_entity(
        level: &mut Level,
        entity: Entity,
        snapshot: &crate::save_state::AiSnapshot,
    ) {
        if level.registry.world.get::<&MonsterAi>(entity).is_err() {
            return;
        }
        let enemy = snapshot.enemy.as_ref().and_then(|memory| {
            crate::save_state::entity_at_spawn_index(level, memory.enemy)
                .map(|entity| (entity, memory))
        });
        let runner = ScheduleRunner::restore(
            &snapshot.schedule_name,
            snapshot.task_index as usize,
            snapshot.schedule_started,
            snapshot.schedule_timer,
        );
        let waypoints: Vec<Vec3> = snapshot
            .route_waypoints
            .iter()
            .take(ohl_ai::movement::MAX_WAYPOINTS)
            .copied()
            .map(crate::save_state::array_vec3)
            .collect();
        // Bounded by the restored (already-truncated) waypoint list itself,
        // so a corrupt or stale `route_current` cannot point past its end.
        // `current == waypoints.len()` is `Route`'s own valid "finished"
        // state (`Route::is_finished`), not an out-of-range value, so the
        // clamp allows exactly that index rather than rewinding a
        // completed route onto its last waypoint.
        let route_current = (snapshot.route_current as usize).min(waypoints.len());
        let route = Route {
            waypoints,
            current: route_current,
            goal: crate::save_state::array_vec3(snapshot.route_goal),
        };
        if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(entity) {
            ai.state = MonsterState::from_tag(snapshot.state_tag).unwrap_or_default();
            ai.conditions = crate::save_state::masked_conditions(snapshot.conditions);
            ai.pending_conditions =
                crate::save_state::masked_conditions(snapshot.pending_conditions);
            ai.runner = runner;
            ai.memory = enemy.map(|(entity, memory)| EnemyMemory {
                entity,
                last_known_position: crate::save_state::array_vec3(memory.last_known_position),
                time_since_seen: crate::save_state::sanitize_f32(memory.time_since_seen, 0.0)
                    .max(0.0),
                occluded: memory.occluded,
                last_known_distance: crate::save_state::sanitize_f32(
                    memory.last_known_distance,
                    0.0,
                )
                .max(0.0),
            });
            ai.route = route;
            ai.move_target = snapshot.move_target.map(crate::save_state::array_vec3);
            ai.cover = snapshot.cover.map(crate::save_state::array_vec3);
            ai.activity = Activity::from_tag(snapshot.activity_tag).unwrap_or_default();
            ai.move_speed = crate::save_state::sanitize_f32(snapshot.move_speed, 0.0).max(0.0);
            ai.ideal_yaw = crate::save_state::sanitize_f32(snapshot.ideal_yaw, 0.0);
            ai.stuck = StuckDetector::from_ticks(snapshot.stuck_ticks);
        }
        if let Some(squad) = &snapshot.squad {
            let _ = level.registry.world.insert_one(
                entity,
                SquadTag {
                    name: squad.name.clone(),
                    leader: squad.leader,
                },
            );
        } else {
            let _ = level.registry.world.remove_one::<SquadTag>(entity);
        }
    }

    /// `SECTION_MOVER_STATE` (28)'s script portion: one optional
    /// [`crate::save_state::ScriptRunnerSnapshot`] per
    /// `level.registry.entities` slot, in spawn order, `None` for an entity
    /// that is not a `scripted_sequence`/`aiscripted_sequence`.
    /// `crate::systems::Systems::snapshot_mover_state` folds this into
    /// [`crate::save_state::snapshot_movers`]'s own result.
    #[must_use]
    pub(crate) fn snapshot_scripts(
        &self,
        level: &Level,
    ) -> Vec<Option<crate::save_state::ScriptRunnerSnapshot>> {
        level
            .registry
            .entities
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_MOVERS)
            .map(|entity| {
                self.scripts
                    .iter()
                    .find(|script| script.entity == *entity)
                    .map(|script| crate::save_state::ScriptRunnerSnapshot {
                        phase_tag: script.runner.phase().tag(),
                        timer: script.runner.timer(),
                        completions: script.runner.completions(),
                        warped: script.runner.warped(),
                        moving_elapsed: script.runner.moving_elapsed(),
                        actor: script
                            .actor
                            .and_then(|actor| crate::save_state::spawn_index_of(level, actor)),
                        pending_trigger: script.pending_trigger,
                        was_active: script.was_active,
                        played: script.played,
                        play_origin: crate::save_state::vec3_array(script.play_origin),
                    })
            })
            .collect()
    }

    /// Restores [`Self::snapshot_scripts`]'s entries, zipped against
    /// `level.registry.entities` in spawn order. Takes the whole
    /// [`crate::save_state::MoverSnapshot`] slice `crate::systems::Systems::
    /// restore_mover_state` already has (rather than a separately
    /// collected `Vec` of just the script fields), so restoring tag 28
    /// allocates one `Vec` of that shape, not two.
    pub(crate) fn restore_scripts(
        &mut self,
        level: &Level,
        snapshots: &[Option<crate::save_state::MoverSnapshot>],
    ) {
        let entities = level.registry.entities.clone();
        for (entity, snapshot) in entities.iter().zip(snapshots) {
            let Some(snapshot) = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.script.as_ref())
            else {
                continue;
            };
            let Some(script) = self
                .scripts
                .iter_mut()
                .find(|script| script.entity == *entity)
            else {
                continue;
            };
            script.runner.restore(
                snapshot.phase_tag,
                snapshot.timer,
                snapshot.completions,
                snapshot.warped,
                snapshot.moving_elapsed,
            );
            script.actor = snapshot
                .actor
                .and_then(|index| crate::save_state::entity_at_spawn_index(level, index));
            script.pending_trigger = snapshot.pending_trigger;
            script.was_active = snapshot.was_active;
            script.played = crate::save_state::sanitize_f32(snapshot.played, 0.0).max(0.0);
            script.play_origin = crate::save_state::array_vec3(snapshot.play_origin);
        }
    }

    /// Phase 8 — one AI think step, and the attacks it produced.
    ///
    /// The navigator, when this map has one, is already attached, so path
    /// following happens inside `AiWorld::tick`.
    // Only engine orchestration calls this; human emission requires actual projectile context.
    pub(crate) fn think(
        &mut self,
        level: &mut Level,
        dt: f32,
        damage: &mut Vec<QueuedDamage>,
        grenade: &GrenadeSafetyContext<'_>,
    ) {
        // Scripts and followers decide before the brains do: both work by
        // setting up the very same route, move target and pending
        // conditions `AiWorld::tick` is about to read, so a scripted or
        // following monster is driven by the existing think step rather
        // than beside it.
        self.update_scripts(level, dt);
        self.update_followers(level);
        if std::mem::take(&mut self.replan_restored_routes) {
            Self::replan_target_routes(level);
        }
        self.update_secondary_opportunities(level, dt);
        self.qualify_human_secondary_emission(level, grenade);
        grenade.projectiles.emit_grenade_danger(&mut self.world, dt);
        let events = {
            let context = SightContext {
                collision: level.monster_collision.as_ref(),
                world: Some(&level.world),
            };
            self.world.tick(&mut level.registry.world, &context, dt)
        };
        self.consume_events(level, &events, damage, grenade);
        // Last, so it wins the precedence tie `docs/FORMAT_SOURCES.md`'s
        // `TODO(black-box)` item 21 documents: `consume_events` just applied
        // whatever `ActivityChanged` the AI's own tick produced for this
        // step (an `Idle` monster among them, looked up under this crate's
        // own "idle" vocabulary name, not the map's `m_iszIdle`), and a
        // dormant script whose monster is idle now overwrites that specific
        // case — and only that case; see `apply_pretrigger_idles`.
        self.apply_pretrigger_idles(level);
    }

    /// Existing saves do not encode a route coordinate convention. Replan
    /// recognized live-target pursuit on the next tick, after player stance
    /// synchronization. All persisted route/cover/memory/destination vectors
    /// remain absolute query/world points; save/restore never translates them.
    pub(crate) fn restore_navigation(&mut self) {
        self.world.invalidate_navigation();
        self.replan_restored_routes = true;
    }

    fn replan_target_routes(level: &mut Level) {
        use ohl_ai::Task;
        let targets: BTreeMap<_, _> = level
            .registry
            .world
            .query::<(Entity, &Actor)>()
            .iter()
            .map(|(entity, actor)| (entity, actor.navigation_anchor()))
            .collect();
        for (actor, ai, hold) in
            &mut level
                .registry
                .world
                .query::<(&Actor, &mut MonsterAi, Option<&ohl_ai::ScriptHold>)>()
        {
            if hold.is_some() || ai.route.is_finished() {
                continue;
            }
            let task = ai.runner.schedule().and_then(|schedule| {
                schedule
                    .tasks
                    .iter()
                    .take(ai.runner.task_index().saturating_add(1))
                    .rev()
                    .find(|task| {
                        matches!(
                            task,
                            Task::MoveToEnemy { .. }
                                | Task::MoveToLastKnownPosition
                                | Task::MoveToTarget { .. }
                                | Task::MoveToNode(_)
                                | Task::TakeCover
                                | Task::Wander { .. }
                        )
                    })
            });
            let pursuing_visible = matches!(task, Some(Task::MoveToEnemy { .. }))
                && ai.memory.is_some_and(|memory| !memory.occluded);
            let goal = if pursuing_visible {
                ai.memory
                    .and_then(|memory| targets.get(&memory.entity).copied())
            } else {
                None
            };
            if let Some(goal) = goal {
                let goal = actor.body_frame.anchor_to_query(actor.hull, goal);
                if pursuing_visible && let Some(memory) = ai.memory.as_mut() {
                    memory.last_known_position = goal;
                }
                ai.move_target = Some(goal);
                ai.route = Route::new();
                ai.move_speed = 0.0;
                ai.runner.clear();
                ai.stuck.reset();
            }
        }
    }

    /// Project-authored readiness persists until a secondary request actually emits.
    fn update_secondary_opportunities(&mut self, level: &mut Level, dt: f32) {
        self.secondary_cooldowns.retain(|entity, remaining| {
            *remaining = (*remaining - dt.max(0.0)).max(0.0);
            level.registry.world.contains(*entity)
        });
        for (entity, actor, ai) in &mut level
            .registry
            .world
            .query::<(Entity, &Actor, &mut MonsterAi)>()
        {
            let Some(kind) = self.brain_kinds.get(ai.brain.0) else {
                continue;
            };
            if !matches!(
                kind,
                MonsterKind::HumanGrunt
                    | MonsterKind::HumanAssassin
                    | MonsterKind::AlienController
                    | MonsterKind::Apache
            ) {
                continue;
            }
            // A previously ready opportunity must not survive a new script/prisoner/death guard.
            ai.pending_conditions.remove(Conditions::CAN_RANGE_ATTACK2);
            if !actor.alive
                || actor.health <= 0.0
                || level.registry.world.get::<&Prisoner>(entity).is_ok()
                || level.registry.world.get::<&ScriptHold>(entity).is_ok()
                || self
                    .secondary_cooldowns
                    .get(&entity)
                    .is_some_and(|left| *left > 0.0)
                || level
                    .registry
                    .world
                    .get::<&ohl_ai::monsters::FlightPlan>(entity)
                    .is_ok_and(|plan| !plan.is_active())
            {
                continue;
            }
            let Some(memory) = ai.memory.filter(|memory| !memory.occluded) else {
                continue;
            };
            let Ok(enemy) = level.registry.world.get::<&Actor>(memory.entity) else {
                continue;
            };
            if !secondary_target_clear(
                kind,
                self.difficulty,
                level.collision.as_ref(),
                actor.eye(),
                &enemy,
            ) {
                continue;
            }
            ai.pending_conditions |= Conditions::CAN_RANGE_ATTACK2;
        }
    }

    /// TODO(black-box): project-authored selection qualification, not a future guarantee.
    /// A refused frozen throw must not repeatedly outrank an ordinary legal attack.
    /// This adds up to 502 scratch steps per ready human/tick; emission still rechecks.
    fn qualify_human_secondary_emission(
        &self,
        level: &mut Level,
        grenade: &GrenadeSafetyContext<'_>,
    ) {
        let candidates: Vec<_> = level
            .registry
            .world
            .query::<(Entity, &Actor, &MonsterAi)>()
            .iter()
            .filter_map(|(entity, actor, ai)| {
                if !ai
                    .pending_conditions
                    .contains(Conditions::CAN_RANGE_ATTACK2)
                    || !matches!(
                        self.brain_kinds.get(ai.brain.0),
                        Some(MonsterKind::HumanGrunt | MonsterKind::HumanAssassin)
                    )
                {
                    return None;
                }
                let request = ai
                    .memory
                    .filter(|memory| !memory.occluded)
                    .and_then(|memory| {
                        let enemy = level.registry.world.get::<&Actor>(memory.entity).ok()?;
                        (enemy.alive && enemy.health > 0.0).then(|| {
                            monster_projectile_request(
                                ohl_combat::ProjectileKind::HandGrenade,
                                self.difficulty,
                                entity,
                                actor.eye(),
                                enemy.eye(),
                                Some(memory.entity),
                            )
                        })
                    });
                Some((entity, request))
            })
            .collect();
        for (entity, request) in candidates {
            if request.as_ref().is_none_or(|request| {
                !grenade.projectiles.human_grenade_safe(
                    level,
                    request,
                    grenade.hitboxes,
                    self.world.relationships(),
                )
            }) && let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(entity)
            {
                ai.pending_conditions.remove(Conditions::CAN_RANGE_ATTACK2);
            }
        }
    }

    /// Turns this step's [`AiEvent`]s into animation, damage and projectile
    /// requests.
    fn consume_events(
        &mut self,
        level: &mut Level,
        events: &[AiEvent],
        damage: &mut Vec<QueuedDamage>,
        grenade: &GrenadeSafetyContext<'_>,
    ) {
        for event in events {
            match &event.kind {
                AiEventKind::ActivityChanged(activity) => {
                    // The two prop kinds whose model the map names
                    // (`MonsterKind::model_from_map`) keep the pose the map
                    // gave them, its `sequence` keyvalue: their own brain's
                    // stand-and-look activities are not a reason to drop a
                    // set piece to sequence 0. A script still drives them
                    // (`PlaySequence`, and the script runner's own calls).
                    if self
                        .spec_of(level, event.entity)
                        .is_some_and(|(kind, _)| kind.model_from_map())
                    {
                        continue;
                    }
                    select_sequence(level, event.entity, &activity_name(*activity));
                }
                AiEventKind::PlaySequence(name) => {
                    select_sequence(level, event.entity, name);
                }
                AiEventKind::Attack { kind, target } => {
                    self.resolve_attack(level, event.entity, *kind, *target, damage, grenade);
                }
                // A Gonarch reaching an `info_bigmomma` node: its
                // `reachtarget` and `reachsequence` fire by name through
                // the map logic, exactly as a finished script's `target`
                // does (`finish_script_step`) — a `scripted_sequence`
                // fired by name starts and binds its own monster — and its
                // `killtarget` removes what it names the same way a
                // script's does.
                AiEventKind::FireTarget(name) | AiEventKind::ScriptRequested(name) => {
                    level.simulation.fire(name.clone(), Some(event.entity), 0.0);
                }
                AiEventKind::KillTarget(name) => {
                    let doomed: Vec<Entity> = level.registry.find(name).to_vec();
                    for entity in doomed {
                        level.registry.world.despawn(entity).ok();
                    }
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
        grenade: &GrenadeSafetyContext<'_>,
    ) {
        let Some((kind, spec)) = self.spec_of(level, attacker) else {
            return;
        };
        let Ok(actor) = level.registry.world.get::<&Actor>(attacker).map(|a| *a) else {
            return;
        };
        if !actor.alive || actor.health <= 0.0 {
            return;
        }
        let shape = attack_shape(&kind, attack);
        let Some((amount, range)) = attack_amount_range(shape, spec, self.difficulty) else {
            return;
        };

        let muzzle = actor.eye();
        let resolved_target = target.and_then(|entity| {
            level
                .registry
                .world
                .get::<&Actor>(entity)
                .ok()
                .map(|actor| *actor)
        });
        // TODO(black-box): a ready schedule may retarget before its attack task.
        // Recheck that event's actual target; task/activity/Wait stay unchanged.
        if secondary_checks_target(&kind, attack)
            && resolved_target.as_ref().is_none_or(|enemy| {
                !secondary_target_clear(
                    &kind,
                    self.difficulty,
                    level.collision.as_ref(),
                    muzzle,
                    enemy,
                )
            })
        {
            return;
        }
        let aim =
            resolved_target.map_or_else(|| muzzle + actor.forward() * range, |enemy| enemy.eye());

        if let AttackShape::Projectile(projectile) = shape {
            let request = monster_projectile_request(
                projectile,
                self.difficulty,
                attacker,
                muzzle,
                aim,
                target,
            );
            // TODO(black-box): attack task/activity occurred; trailing Wait is unchanged.
            // Unsafe frozen-world exposure refuses only emission and its cooldown.
            if attack == AttackKind::Range2
                && matches!(kind, MonsterKind::HumanGrunt | MonsterKind::HumanAssassin)
                && !grenade.projectiles.human_grenade_safe(
                    level,
                    &request,
                    grenade.hitboxes,
                    self.world.relationships(),
                )
            {
                return;
            }
            self.projectiles.spawn_projectile(&request);
            if attack == AttackKind::Range2 {
                // TODO(black-box): project-authored six-second opportunity cadence.
                self.secondary_cooldowns.insert(attacker, 6.0);
            }
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
        if let Some(collision) = level.monster_collision.as_ref() {
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
        // How each hit target's species answers each damage type (the
        // gargantua's published immunity, the Apache's doubled blast,
        // face value for everything else), resolved here where the species
        // table is reachable and handed to the one place a monster's
        // health moves. Looked up per target once, not per event: the
        // queue is small and bounded. (A `Not solid` prop never gets
        // here: `drain_engine_damage` dropped every hit at it.)
        let responses: BTreeMap<Entity, DamageResponse> = self
            .damage
            .events()
            .iter()
            .map(|event| event.target)
            .map(|target| {
                let response = self
                    .spec_of(level, target)
                    .map_or(DamageResponse::ORDINARY, |(kind, _)| {
                        ohl_ai::damage_response_for(&kind)
                    });
                (target, response)
            })
            .collect();
        let response_of = |target: Entity| {
            responses
                .get(&target)
                .copied()
                .unwrap_or(DamageResponse::ORDINARY)
        };
        let outcome = ohl_ai::monsters::lifecycle::apply_damage_effective(
            &mut level.registry.world,
            &self.damage,
            ohl_ai::monsters::lifecycle::DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &response_of,
        );
        let (events, corpses) = (outcome.events, outcome.corpses);
        // Every monster a hit reached, for its `TriggerCondition`'s "took
        // damage": unchanged by M9.45's damage types, so a species that
        // shrugs a hit off still counts as hit there. Which hits cost it
        // anything is `outcome.hurt`.
        let hurt: Vec<Entity> = self
            .damage
            .events()
            .iter()
            .map(|event| event.target)
            .collect();
        // Only the hits that cost their target something count as damage
        // applied: one its species ignores, or that its shield or reserve
        // took, does not (M9.45).
        self.damage_events += self
            .damage
            .events()
            .iter()
            .filter(|event| {
                outcome.hurt.contains(&event.target) && response_of(event.target).scale(event) > 0.0
            })
            .count() as u64;
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
                .get::<&ohl_ai::Impervious>(queued.target)
                .is_ok()
            {
                // A `Not solid` prop is "impervious to any damage".
                continue;
            }
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
                kinds: damage_kinds_of(queued.info.kind),
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
                // Drain this maker's pending `target`-firing activations
                // (routed here by `ohl-game`'s `Simulation::activate`,
                // exactly like a script's `ScriptActivation`) into
                // `Spawner::trigger` calls before ticking it, so a save's
                // worth of queued triggers and this frame's tick see the
                // same, up-to-date `active` state. `MakerActivation::pending`
                // itself is also part of `SECTION_MOVER_STATE` (tag 28,
                // `crate::save_state::MonsterMakerSnapshot::pending_activation`),
                // so a save taken between this drain (phase 10) and the
                // next `Simulation::activate` bump (phase 12) does not lose
                // a trigger that landed in that one-tick gap.
                let pending = level
                    .registry
                    .world
                    .get::<&mut MakerActivation>(maker)
                    .ok()
                    .map_or(0, |mut activation| std::mem::take(&mut activation.pending));
                let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(maker) else {
                    continue;
                };
                for _ in 0..pending {
                    component.0.trigger();
                }
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
    ///
    /// Pushes the new entity onto `Registry::entities` too, so it gets a
    /// spawn index exactly like a map-declared monster and is covered by
    /// every index-keyed save section (`SECTION_ENTITY_REGISTRY` 18,
    /// `SECTION_ENTITY_COMBAT` 24, `SECTION_AI` 25) for free — this is what
    /// closes the "monstermaker children are not saved" gap
    /// `crate::save_state`'s module doc used to record; see that doc's
    /// current "Monstermaker children are now saved" section, and
    /// [`Self::restore_maker_children`] for the load-side half (recreating
    /// the entity itself, which a fresh `attach_level` never does for a
    /// maker's own children).
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
        let model = kind.default_model_path().and_then(|path| {
            level
                .studio_model_paths
                .iter()
                .position(|loaded| loaded.eq_ignore_ascii_case(path))
        });
        actor.configure_model(
            &kind,
            model.and_then(|index| level.studio_models.get(index)),
        );
        let entity = level.registry.world.spawn((
            ClassName(classname.to_string()),
            Transform {
                origin,
                angles: Vec3::new(0.0, yaw, 0.0),
            },
            actor,
            MonsterAi::new(brain),
            ohl_combat::Health::new(health),
            Owner(maker),
        ));
        level.registry.entities.push(entity);
        if let Some(model) = model {
            let _ = level.registry.world.insert_one(
                entity,
                StudioAnim {
                    model,
                    sequence: 0,
                    cycle: 0.0,
                    frame_rate: 1.0,
                    body: 0,
                    skin: 0,
                },
            );
        }
        stand_on_floor(level, entity, &kind);
        // TODO(black-box): project-authored child defaults, not the maker's flags.
        Self::attach_follower(&mut level.registry.world, entity, classname, 0);
        Some(entity)
    }

    /// Recreates every `monstermaker` child `snapshots` recorded
    /// (`SECTION_MAKER_CHILDREN`, 29), so their spawn index lines up with
    /// `SECTION_ENTITY_REGISTRY`/`SECTION_ENTITY_COMBAT`/`SECTION_AI`
    /// (18/24/25)'s own zip-against-`Registry::entities` restore.
    ///
    /// **Must run before those sections' restore** (`crate::game::Game::
    /// restore`, right at the top): `level.registry.entities` still holds
    /// only the entities this load's `attach_level` just spawned fresh —
    /// never a maker's own dynamically-created children, which a map does
    /// not declare — so every slot this call appends is exactly the gap
    /// those later, index-keyed restores would otherwise silently skip
    /// (their own `.zip(entities)` simply stops at the shorter list).
    ///
    /// A `Some` slot recreates a fresh placeholder monster of the recorded
    /// classname (via [`Self::spawn_child`]), at the maker's own current
    /// transform — a throwaway starting pose, since tag 18's restore
    /// overlays the save's exact transform onto it moments later, same as
    /// it already does for every attach_level-spawned monster. A slot this
    /// build cannot recreate — no brain registered for the recorded
    /// classname (a save from a build with a different monster table), or
    /// `None` because no record survived the child's own death before the
    /// save (see `crate::save_state`'s module doc) — gets an inert,
    /// component-less placeholder instead, purely to keep every later
    /// slot's index aligned; nothing ever queries it again. Recreation
    /// stops (falling back to placeholders for the remaining tail) once
    /// [`MAX_MAKER_CHILDREN_PER_LEVEL`] real children exist, the same cap
    /// [`Self::tick_makers`] enforces at runtime, so a corrupt or
    /// adversarial save cannot use this path to spawn unbounded monsters.
    ///
    /// Returns the `(maker, child)` pairs for every real (non-placeholder)
    /// child recreated, for [`Self::finalize_maker_children`] to link back
    /// onto each maker's own live-child list once health/AI state has
    /// actually been restored onto them.
    pub(crate) fn restore_maker_children(
        &mut self,
        level: &mut Level,
        snapshots: Option<&[Option<crate::save_state::MonsterMakerChildSnapshot>]>,
    ) -> Vec<(Entity, Entity)> {
        let Some(snapshots) = snapshots else {
            return Vec::new();
        };
        let base = level.registry.entities.len();
        let mut pairs = Vec::new();
        for snapshot in snapshots.iter().skip(base) {
            let recreated = snapshot.as_ref().and_then(|child| {
                if self.maker_children >= MAX_MAKER_CHILDREN_PER_LEVEL {
                    return None;
                }
                let maker = crate::save_state::entity_at_spawn_index(level, child.maker)?;
                let (origin, yaw) = level
                    .registry
                    .world
                    .get::<&Transform>(maker)
                    .map_or((Vec3::ZERO, 0.0), |transform| {
                        (transform.origin, transform.angles.y)
                    });
                let spawned = self.spawn_child(level, maker, &child.classname, origin, yaw)?;
                self.maker_children += 1;
                Some((maker, spawned))
            });
            if let Some((maker, spawned)) = recreated {
                pairs.push((maker, spawned));
            } else {
                let placeholder = level.registry.world.spawn(());
                level.registry.entities.push(placeholder);
            }
        }
        pairs
    }

    /// Finishes [`Self::restore_maker_children`]: links each recreated
    /// child back onto its maker's own [`ohl_ai::Spawner`] live-child list
    /// (`ohl_ai::Spawner::restore_child`), then prunes any that are
    /// already dead — the same `is_alive` check [`Self::tick_makers`]'s own
    /// pruning uses — so `live_children`/`has_room` count correctly right
    /// after a load, matching what they would report mid-session.
    ///
    /// Run this **after** `SECTION_ENTITY_COMBAT`/`SECTION_AI` (24/25) and
    /// `Systems::sync_actor_from_transforms` have all applied: only then
    /// does each recreated child's `Actor::alive` reflect the save's own
    /// health, rather than [`Self::spawn_child`]'s full-health placeholder
    /// default.
    pub(crate) fn finalize_maker_children(level: &mut Level, pairs: &[(Entity, Entity)]) {
        // A child the save recorded as already dead (a corpse still
        // carrying `Owner`/`ClassName` — `Self::retire` only ever removes
        // `MonsterAi`, never the whole entity, for a `CorpseDecision::
        // Corpse`) still gets a fresh `MonsterAi` from `Self::spawn_child`
        // above, since that is the only way this method knows to recreate
        // *any* child. Strip it back off once `Actor::alive` (just set by
        // `SECTION_ENTITY_COMBAT`'s restore, above this call) says the
        // child did not survive, so `Game::monster_count` (which counts
        // `MonsterAi`) does not resurrect a corpse into a thinking monster.
        for &(_, child) in pairs {
            let alive = level
                .registry
                .world
                .get::<&Actor>(child)
                .is_ok_and(|actor| actor.alive);
            if !alive {
                level.registry.world.remove_one::<MonsterAi>(child).ok();
            }
        }
        for &(maker, child) in pairs {
            if let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(maker) {
                component.0.restore_child(child);
            }
        }
        let mut makers: Vec<Entity> = level
            .registry
            .world
            .query::<(Entity, &MonsterMaker)>()
            .iter()
            .map(|(entity, _)| entity)
            .collect();
        makers.sort_unstable_by_key(|entity: &Entity| entity.id());
        for maker in makers {
            let is_alive = |entity: Entity| {
                level
                    .registry
                    .world
                    .get::<&Actor>(entity)
                    .is_ok_and(|actor| actor.alive)
            };
            if let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(maker) {
                component.0.prune_dead(&is_alive);
            }
        }
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

/// The existing secondary attacks that recheck their resolved target.
fn secondary_checks_target(kind: &MonsterKind, attack: AttackKind) -> bool {
    attack == AttackKind::Range2
        && matches!(
            kind,
            MonsterKind::HumanGrunt
                | MonsterKind::HumanAssassin
                | MonsterKind::AlienController
                | MonsterKind::Apache
        )
}

/// The existing species table's damage and range for this attack shape.
fn attack_amount_range(
    shape: AttackShape,
    spec: &MonsterSpec,
    difficulty: AiDifficulty,
) -> Option<(f32, f32)> {
    match shape {
        AttackShape::Melee => spec
            .melee
            .map(|melee| (melee.damage[difficulty.index()], melee.range)),
        AttackShape::Hitscan | AttackShape::Projectile(_) => spec.ranged.map(|ranged| {
            (
                ranged.damage[difficulty.index()],
                if ranged.range.is_finite() && ranged.range > 0.0 {
                    ranged.range
                } else {
                    DEFAULT_ATTACK_RANGE
                },
            )
        }),
    }
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

/// `ohl-combat`'s damage type as `ohl-ai`'s damage kinds.
///
/// The two are the same published vocabulary in the same declaration
/// order with the same dense bit assignment (`ohl_ai::damage`'s module doc
/// comment records that as a contract), so the conversion is by bits;
/// `damage_kinds_match_combat_damage_types_bit_for_bit` below holds it.
#[must_use]
pub fn damage_kinds_of(kind: DamageType) -> DamageKinds {
    DamageKinds::from_bits_truncate(kind.bits())
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

    /// The installed `sentences.txt` lookup, so the presentation phase can
    /// resolve an `ambient_generic`'s published `!SENTENCENAME` `message`
    /// against the same table a `scripted_sentence` speaks from.
    #[must_use]
    pub fn sentence_lookup(&self) -> &SentenceLookup {
        &self.sentence_lookup
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

    /// How many scripted sequences have given up on ever satisfying
    /// `ScriptPhase::Moving`'s mark condition and released their monster,
    /// since this level was attached. See
    /// `ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS`'s doc comment: a
    /// counted reason, never a log line.
    #[must_use]
    pub fn script_timeout_count(&self) -> u64 {
        self.script_timeouts
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
            if spawned.contains(&entity) {
                Self::attach_follower(
                    &mut level.registry.world,
                    entity,
                    &def.classname,
                    def.spawnflags,
                );
            }
        }
    }

    /// Shared derived component initialization for declared and maker talk monsters.
    fn attach_follower(
        world: &mut ohl_game::hecs::World,
        entity: Entity,
        classname: &str,
        spawnflags: u32,
    ) {
        if TALK_MONSTER_CLASSNAMES.contains(&classname) {
            world
                .insert_one(entity, Follower::from_spawnflags(spawnflags))
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
        // Script placement stays in the authored anchor domain.
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
                let goal = level
                    .registry
                    .world
                    .get::<&Actor>(actor)
                    .map_or(mark, |actor| {
                        actor.body_frame.anchor_to_query(actor.hull, mark)
                    });
                if let Ok(mut ai) = level.registry.world.get::<&mut MonsterAi>(actor) {
                    if ai.route.is_finished() || ai.route.needs_refresh(goal) {
                        ai.route = ohl_ai::Route::straight_line(goal);
                        ai.stuck.reset();
                    }
                    ai.move_target = Some(goal);
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
        if step.timed_out {
            self.script_timeouts += 1;
        }
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

    /// Applies every dormant script's pre-trigger idle animation
    /// (`ScriptRunner::pretrigger_idle_sequence`) to its named monster,
    /// without possessing it.
    ///
    /// Called once per [`Self::think`], after the monster's own AI has
    /// already ticked and applied whatever sequence its own activity change
    /// asked for. This is deliberately the *last* write to
    /// [`StudioAnim::sequence`] this step, and deliberately narrow: it only
    /// overwrites the case `docs/FORMAT_SOURCES.md`'s `TODO(black-box)`
    /// item 21 resolves — the monster's own [`MonsterAi`] reporting
    /// [`MonsterState::Idle`] and [`Activity::Idle`], i.e. "nothing of
    /// interest perceived" and "standing around" (this project's project-
    /// authored idle-eligibility rule, since no page states one). Anything
    /// else the monster's own brain is doing — walking, alert, fighting,
    /// following — is left completely alone, matching the module-level
    /// contract that a dormant script never touches the actor it names.
    ///
    /// A monster with no [`MonsterAi`] at all (so no state or activity to
    /// read) is treated as **not** eligible: this method can only apply the
    /// documented idle *while the monster's own AI is idle*, and an inert
    /// monster has no AI to be idle. `ScriptAction::Idle` — produced only
    /// while a script *holds* an active monster, e.g. approaching a
    /// `MoveTo::No` mark — remains the published idle for a monster with no
    /// brain of its own to fight over the sequence slot.
    fn apply_pretrigger_idles(&self, level: &mut Level) {
        for script in &self.scripts {
            let Some(actor) = script.actor else { continue };
            if !level.registry.world.contains(actor) {
                continue;
            }
            let Some(idle) = script.runner.pretrigger_idle_sequence() else {
                continue;
            };
            let eligible = level
                .registry
                .world
                .get::<&MonsterAi>(actor)
                .is_ok_and(|ai| ai.state == MonsterState::Idle && ai.activity == Activity::Idle);
            if eligible {
                select_sequence(level, actor, idle);
            }
        }
    }

    /// Offers a queued player `use` to the nearest talk monster and keeps
    /// every follower pointed at the player. Part of phase 8.
    fn update_followers(&mut self, level: &mut Level) {
        use ohl_ai::follow::FollowInput;
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

        let player_goal = level
            .registry
            .world
            .get::<&Actor>(level.player)
            .ok()
            .filter(|actor| actor.alive && actor.health > 0.0)
            .map(|actor| actor.navigation_anchor())
            .filter(|goal| goal.is_finite());
        for (entity, ai, follower) in
            &mut level
                .registry
                .world
                .query::<(Entity, &mut MonsterAi, Option<&Follower>)>()
        {
            let authorized = follower.is_some_and(|f| f.can_follow && f.following)
                && self.followers.is_following(entity);
            if follower.is_some() || ai.follow_attempt.is_some() {
                ai.pending_conditions.remove(Conditions::SPECIAL2);
            }
            ai.follow_input = if authorized {
                ai.pending_conditions |= Conditions::SPECIAL2;
                player_goal.map_or(FollowInput::Unavailable, FollowInput::Target)
            } else {
                FollowInput::Inactive
            };
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
    /// A sentence that fires emits one voice cue carrying the words the
    /// payload's own `sentences.txt` names for its group, in speaking order
    /// (`ohl_gameplay::SoundAsset::Sentence`). Those words are asset paths
    /// read from the user's installation at run time: they travel in
    /// memory to the host, which decodes and plays them, and they never
    /// enter this project's source, a log line or any other diagnostic
    /// (`docs/CLEAN_ROOM.md` rule 7). A group the payload does not define
    /// yields no words, and the cue then names nothing playable. Only the
    /// word *count* is kept here, as a bounded aggregate.
    ///
    /// The cue is placed where the speaker stands when the line starts,
    /// and stays there: a speaker that walks while talking is still heard
    /// from where it began. That is a known limit (see
    /// `docs/FORMAT_SOURCES.md`, "`scripted_sentence` and sentences").
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
            // A speaker is a live actor. A dead one says nothing, and the
            // sentence waits `refire` and looks again, exactly as when no
            // speaker is found at all.
            let Some((speaker, speaker_origin)) = find_speaker(level, &sentence.def, origin) else {
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
            // Heard at the speaker's own position, not the
            // `scripted_sentence`'s: the entity is a director, and what the
            // player hears is the monster it directed. At the published
            // `volume` (`SentenceDef::gain`) and "Sound Radius"
            // (`SentenceDef::radius`, read through the same `ATTN_*`
            // mapping as an `ambient_generic`'s radius spawnflags).
            #[allow(clippy::cast_possible_truncation)]
            self.sound_cues.push(
                ohl_gameplay::SoundCue::new(
                    entity_id(speaker).0 as u32,
                    ohl_gameplay::ChannelClass::Voice,
                    // Resolved through the payload's own `sentences.txt`,
                    // so no asset path literal is involved; an unknown
                    // group simply yields no words and nothing to play.
                    ohl_gameplay::SoundAsset::sentence(words.into_iter().map(|word| word.0)),
                )
                .at(
                    speaker_origin.to_array(),
                    crate::presentation::attenuation_of(sentence.def.radius()),
                )
                .with_gain(sentence.def.gain(), 1.0),
            );
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

/// Puts the same authored anchor and yaw in Actor and Transform.
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

/// Whether `def` is a `monster_generic` drawing one of the
/// [`CENTRED_ORIGIN_MODELS`], whose map origin is its middle, not its feet.
fn has_centred_origin(def: &EntityDef) -> bool {
    let Some(ohl_game::keyvalues::ModelRef::Asset(path)) = &def.model else {
        return false;
    };
    let path = path.trim().replace('\\', "/");
    def.classname == "monster_generic"
        && CENTRED_ORIGIN_MODELS
            .iter()
            .any(|model| path.eq_ignore_ascii_case(model))
}

/// Spawn-floor eligibility is distinct from the derived query offset. A custom
/// model bottom can give a zero or negative offset and still be a walker.
fn stands_on_floor(kind: &MonsterKind, actor: &Actor) -> bool {
    matches!(
        actor.body_frame,
        ohl_ai::BodyFrame::Feet | ohl_ai::BodyFrame::ModelBottom(_)
    ) && !ohl_ai::movement::flies(actor.hull)
        && !matches!(
            kind,
            MonsterKind::Barnacle
                | MonsterKind::Ichthyosaur
                | MonsterKind::Leech
                | MonsterKind::Nihilanth
                | MonsterKind::Tentacle
        )
}

/// Applies the bounded spawn-floor search once, before any saved Transform is
/// restored. Actor and Transform remain the same model anchor; only the trace
/// crosses the BodyFrame boundary. Missing floor, solid starts and exclusions
/// retain the authored placement without introducing a second lift component.
fn stand_on_floor(level: &mut Level, entity: Entity, kind: &MonsterKind) {
    let Ok(actor) = level
        .registry
        .world
        .get::<&Actor>(entity)
        .map(|actor| *actor)
    else {
        return;
    };
    if !stands_on_floor(kind, &actor) || !actor.origin.is_finite() {
        return;
    }
    let Some(collision) = level.monster_collision.as_ref() else {
        return;
    };
    let start = actor.query_origin() + Vec3::Z * MONSTER_DROP_CLEARANCE;
    let end = start - Vec3::Z * (MONSTER_DROP_DISTANCE + MONSTER_DROP_CLEARANCE);
    if !start.is_finite() || !end.is_finite() {
        return;
    }
    let fall = collision.trace(actor.hull, start, end);
    if fall.start_solid || fall.all_solid || fall.fraction >= 1.0 || !fall.end_pos.is_finite() {
        return;
    }
    let anchor = actor.body_frame.query_to_anchor(actor.hull, fall.end_pos);
    // A trace contact epsilon must not lift an already grounded authored anchor.
    if !anchor.is_finite() || anchor.z >= actor.origin.z {
        return;
    }
    if let Ok(mut transform) = level.registry.world.get::<&mut Transform>(entity) {
        transform.origin = anchor;
    }
    if let Ok(mut actor) = level.registry.world.get::<&mut Actor>(entity) {
        actor.origin = anchor;
    }
}

#[cfg(test)]
mod anchor_domain_floor_tests {
    use super::*;
    use crate::test_support::{AI_MAP, ai_room_bsp, entity_block, entity_of_classname};

    #[test]
    // Keep generated model setup and its floor/retention discriminators together.
    #[allow(clippy::too_many_lines)]
    fn anchor_domain_floor_uses_one_offset_even_for_custom_nonpositive_offsets() {
        for (bottom, authored_z, distinguishes_start) in [
            (0.0_f32, 17.0, true),
            (-36.0, 80.0, false),
            (-52.0, 80.0, false),
        ] {
            let block = format!(
                "{{\"classname\" \"worldspawn\"}}{}{}",
                entity_block("info_player_start", [-200.0, -200.0, 36.0], 0.0, &[]),
                entity_block(
                    "monster_generic",
                    [100.0, 0.0, authored_z],
                    0.0,
                    &[("model", "models/ohl-anchor.mdl")]
                )
            );
            let bytes = ai_room_bsp(&block, false);
            let (mut mdl, _) = ohl_formats::test_support::build_minimal_mdl10();
            for (base, values) in [
                (88, [-16.0, -16.0, bottom]),
                (100, [16.0, 16.0, bottom + 72.0]),
            ] {
                for (axis, value) in values.into_iter().enumerate() {
                    mdl[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            let mut assets = crate::MemoryAssets::new();
            assets.insert("models/ohl-anchor.mdl", mdl);
            let mut game =
                crate::Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("generated floor");
            let entity = entity_of_classname(&game, "monster_generic").expect("custom walker");
            let (level, _) = game.level_and_systems_mut();
            let actor = *level.registry.world.get::<&Actor>(entity).unwrap();
            assert_eq!(actor.body_frame, ohl_ai::BodyFrame::ModelBottom(bottom));
            if distinguishes_start {
                // The model's raised feet are clear, while treating that anchor
                // as a centered Standing query starts inside the generated floor.
                let anchor = Vec3::new(100.0, 0.0, authored_z);
                let model = level.monster_collision.as_ref().unwrap();
                let query_start = actor.body_frame.anchor_to_query(actor.hull, anchor)
                    + Vec3::Z * MONSTER_DROP_CLEARANCE;
                let raw_start = anchor + Vec3::Z * MONSTER_DROP_CLEARANCE;
                assert!(
                    !model
                        .trace(actor.hull, query_start, query_start)
                        .start_solid,
                    "raised query start is clear"
                );
                assert!(
                    model.trace(actor.hull, raw_start, raw_start).start_solid,
                    "unconverted anchor start is solid"
                );
            }
            assert_eq!(
                actor.origin,
                level
                    .registry
                    .world
                    .get::<&Transform>(entity)
                    .unwrap()
                    .origin
            );
            assert!(
                (actor.origin.z + bottom).abs() < 0.05,
                "single-offset floor landing"
            );
            assert!((actor.query_origin().z - 36.0).abs() < 0.05);

            // Missing collision and a genuinely embedded start retain both anchors.
            let saved_collision = level.monster_collision.take();
            let placed = Vec3::new(100.0, 0.0, 80.0);
            place(level, entity, placed, 0.0);
            stand_on_floor(level, entity, &MonsterKind::Generic);
            assert_eq!(
                level.registry.world.get::<&Actor>(entity).unwrap().origin,
                placed
            );
            level.monster_collision = saved_collision;
            let embedded = Vec3::new(100.0, 0.0, -100.0 - bottom);
            place(level, entity, embedded, 0.0);
            stand_on_floor(level, entity, &MonsterKind::Generic);
            assert_eq!(
                level.registry.world.get::<&Actor>(entity).unwrap().origin,
                embedded
            );
            assert_eq!(
                level
                    .registry
                    .world
                    .get::<&Transform>(entity)
                    .unwrap()
                    .origin,
                embedded
            );

            // A separate generated open model puts its only floor out of reach.
            let mut builder = ohl_formats::test_support::Bsp30Builder::new();
            builder.set_entities_text("{\"classname\" \"worldspawn\"}");
            let heads = builder.push_collision_hulls(&[
                ohl_formats::test_support::CollisionBrush::half_space([0.0, 0.0, 1.0], -1024.0),
            ]);
            builder.push_model([-2048.0; 3], [2048.0; 3], [0.0; 3], heads, 2, 0, 0);
            let bytes = builder.build();
            let limits = ohl_formats::bsp30::Limits::default();
            let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &limits).unwrap();
            level.monster_collision =
                Some(ohl_physics::CollisionModel::from_bsp(&bsp, &limits).unwrap());
            place(level, entity, placed, 0.0);
            let actor = *level.registry.world.get::<&Actor>(entity).unwrap();
            let start = actor.query_origin() + Vec3::Z * MONSTER_DROP_CLEARANCE;
            let fall = level.monster_collision.as_ref().unwrap().trace(
                actor.hull,
                start,
                start - Vec3::Z * (MONSTER_DROP_DISTANCE + MONSTER_DROP_CLEARANCE),
            );
            assert!(
                !fall.start_solid && !fall.all_solid && fall.fraction >= 1.0,
                "actual no-floor prerequisite"
            );
            stand_on_floor(level, entity, &MonsterKind::Generic);
            assert_eq!(
                level.registry.world.get::<&Actor>(entity).unwrap().origin,
                placed
            );
        }
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
        // Feet to the placed point entity's origin, as the map measured it.
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

/// The live actor a `scripted_sentence`'s `entity` keyvalue names, and
/// where it stands.
///
/// Published: a `targetname` matches at any distance, a classname only
/// inside `radius`, measured from the `scripted_sentence` itself. Either
/// way the speaker must be alive: a corpse keeps its `Actor` (a death only
/// retires its brain), and a corpse does not talk.
fn find_speaker(level: &Level, def: &SentenceDef, origin: Vec3) -> Option<(Entity, Vec3)> {
    if def.speaker.is_empty() {
        return None;
    }
    let speaker = level
        .registry
        .find(&def.speaker)
        .iter()
        .copied()
        .find(|entity| {
            level
                .registry
                .world
                .get::<&Actor>(*entity)
                .is_ok_and(|actor| actor.alive)
        })
        .or_else(|| nearest_by_classname(level, &def.speaker, origin, def.radius))?;
    let at = level.registry.world.get::<&Actor>(speaker).ok()?.origin;
    Some((speaker, at))
}

/// The nearest talk monster to `position` that is close enough to `use`.
fn nearest_follower(level: &Level, position: Vec3) -> Option<Entity> {
    let mut candidates: Vec<(f32, u32, Entity)> = Vec::new();
    for (entity, actor, _) in &mut level.registry.world.query::<(Entity, &Actor, &Follower)>() {
        if !actor.alive {
            continue;
        }
        // The use ray starts at the player eye; compare the physical body,
        // not a newly feet-relative model pivot.
        let distance = actor.query_origin().distance(position);
        if !distance.is_finite() || distance > TALK_USE_RADIUS {
            continue;
        }
        candidates.push((distance, entity.id(), entity));
    }
    closest(&mut candidates)
}

#[cfg(test)]
mod tests {
    use super::{AttackShape, activity_name, attack_shape, damage_kinds_of, trigger_condition_of};
    use ohl_ai::{Activity, AttackKind, DamageKinds, MonsterKind, TriggerCondition};
    use ohl_combat::DamageType;

    /// The contract `damage_kinds_of` converts by: `ohl-combat`'s damage
    /// types and `ohl-ai`'s damage kinds are the same published names in
    /// the same order on the same bits, so a hit typed in one crate means
    /// the same thing in the other.
    #[test]
    fn damage_kinds_match_combat_damage_types_bit_for_bit() {
        assert_eq!(DamageType::NAMED.len(), DamageKinds::NAMED.len());
        for ((combat, combat_label), (ai, ai_label)) in
            DamageType::NAMED.iter().zip(DamageKinds::NAMED.iter())
        {
            assert_eq!(combat_label, ai_label);
            assert_eq!(combat.bits(), ai.bits(), "{combat_label}");
            assert_eq!(damage_kinds_of(*combat), *ai);
        }
        assert_eq!(damage_kinds_of(DamageType::ALL), DamageKinds::ALL);
        assert_eq!(damage_kinds_of(DamageType::NONE), DamageKinds::NONE);
        assert_eq!(
            damage_kinds_of(DamageType::BLAST | DamageType::BURN),
            DamageKinds::BLAST | DamageKinds::BURN
        );
    }

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
    fn published_projectile_attacks_map_to_physical_kinds() {
        let projectiles = MonsterKind::defined()
            .iter()
            .filter(|kind| {
                matches!(
                    attack_shape(kind, AttackKind::Range1),
                    AttackShape::Projectile(_)
                )
            })
            .count();
        assert_eq!(
            projectiles, 4,
            "hornet, spit, hand ball and mortar primaries"
        );
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

/// TODO(black-box): project-authored secondary engagement/geometry policy,
/// shared by readiness and emission to check the current target consistently.
fn secondary_target_clear(
    kind: &MonsterKind,
    difficulty: AiDifficulty,
    collision: Option<&ohl_physics::CollisionModel>,
    muzzle: Vec3,
    enemy: &Actor,
) -> bool {
    let aim = enemy.eye();
    if !enemy.alive || enemy.health <= 0.0 || !(96.0..=1024.0).contains(&muzzle.distance(aim)) {
        return false;
    }
    // Held NPC brushes may be absent only from the monster collision model.
    let Some(collision) = collision else {
        return false;
    };
    if collision
        .trace(ohl_physics::Hull::Point, muzzle, aim)
        .fraction
        < 1.0
    {
        return false;
    }
    !matches!(kind, MonsterKind::HumanGrunt | MonsterKind::HumanAssassin)
        || grenade_lob_clear(collision, difficulty, muzzle, aim)
}

/// One ordinary request construction shared by selection preview and emission.
fn monster_projectile_request(
    kind: ohl_combat::ProjectileKind,
    difficulty: AiDifficulty,
    owner: Entity,
    origin: Vec3,
    aim: Vec3,
    target: Option<Entity>,
) -> ProjectileRequest {
    let (damage, damage_type, blast_radius) = monster_projectile_profile(kind, difficulty);
    let velocity = monster_projectile_velocity(kind, difficulty, origin, aim);
    ProjectileRequest {
        kind,
        owner,
        origin,
        velocity,
        damage,
        damage_type,
        blast_radius,
        target,
    }
}

/// Published head-ball speed; other speeds and bounded ballistic aim are TODO(black-box).
fn monster_projectile_velocity(
    kind: ohl_combat::ProjectileKind,
    difficulty: AiDifficulty,
    muzzle: Vec3,
    aim: Vec3,
) -> Vec3 {
    use ohl_combat::ProjectileKind as P;
    if matches!(kind, P::HandGrenade | P::GonarchMortar) {
        let flight = monster_lob_flight_seconds(muzzle, aim);
        (aim - muzzle) / flight
            + Vec3::Z * (ohl_physics::MoveConfig::default().gravity * flight * 0.5)
    } else {
        let speed = if kind == P::ControllerHomingBall {
            [650.0, 800.0, 1000.0][difficulty.index()]
        } else {
            DEFAULT_PROJECTILE_SPEED
        };
        (aim - muzzle).normalize_or_zero() * speed
    }
}

fn monster_lob_flight_seconds(muzzle: Vec3, aim: Vec3) -> f32 {
    (aim.distance(muzzle) / 400.0).clamp(0.35, 1.5)
}

/// TODO(black-box): project-authored current-world approach clearance, not
/// eventual splash safety. The last whole fixed step conservatively covers
/// the nominal flight time; entity impacts and later bounces are not predicted.
fn grenade_lob_clear(
    collision: &ohl_physics::CollisionModel,
    difficulty: AiDifficulty,
    muzzle: Vec3,
    aim: Vec3,
) -> bool {
    if !muzzle.is_finite() || !aim.is_finite() {
        return false;
    }
    let flight = monster_lob_flight_seconds(muzzle, aim);
    let mut velocity = monster_projectile_velocity(
        ohl_combat::ProjectileKind::HandGrenade,
        difficulty,
        muzzle,
        aim,
    );
    if !flight.is_finite() || !velocity.is_finite() {
        return false;
    }
    let gravity = ohl_physics::MoveConfig::default().gravity
        * ohl_combat::ProjectileTuning::default().gravity_scale.value;
    let mut position = muzzle;
    // Flight is capped at 1.5 seconds: at most 150 current engine steps.
    let mut steps = (flight / crate::TICK_SECONDS).ceil();
    while steps > 0.0 {
        velocity.z -= gravity * crate::TICK_SECONDS;
        let end = position + velocity * crate::TICK_SECONDS;
        if !end.is_finite() {
            return false;
        }
        let trace = collision.trace(ohl_physics::Hull::Point, position, end);
        if trace.start_solid || trace.all_solid || trace.fraction < 1.0 {
            return false;
        }
        position = end;
        steps -= 1.0;
    }
    true
}

/// Published attack tables: TWHL pages for each named monster, cited in FORMAT_SOURCES.
/// Radii other than Gonarch's and damage classifications are project-authored.
fn monster_projectile_profile(
    kind: ohl_combat::ProjectileKind,
    difficulty: AiDifficulty,
) -> (f32, DamageType, Option<f32>) {
    use ohl_combat::ProjectileKind as P;
    let skill = difficulty.index();
    match kind {
        P::HandGrenade => (100.0, DamageType::BLAST, Some(200.0)),
        P::Rocket => (150.0, DamageType::BLAST, Some(250.0)),
        P::Hornet => ([4.0, 5.0, 8.0][skill], DamageType::BULLET, None),
        P::BullsquidSpit => ([10.0, 10.0, 15.0][skill], DamageType::ACID, None),
        P::ControllerBall => ([3.0, 4.0, 5.0][skill], DamageType::SHOCK, None),
        P::ControllerHomingBall => ([15.0, 25.0, 35.0][skill], DamageType::SHOCK, None),
        P::GonarchMortar => (
            [100.0, 120.0, 160.0][skill],
            DamageType::BLAST | DamageType::ACID,
            Some([250.0, 250.0, 275.0][skill]),
        ),
        _ => (0.0, DamageType::GENERIC, None),
    }
}

#[cfg(test)]
mod origin_frame_tests {
    use super::*;
    use crate::test_support::{AI_MAP, ai_room_bsp, entity_block, entity_of_classname};

    #[test]
    fn projectile_launch_and_aim_use_the_same_rotated_metadata_eyes_as_sight() {
        let entities = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}",
            entity_block("monster_alien_grunt", [0.0, 0.0, 0.0], 90.0, &[]),
            entity_block("monster_barney", [100.0, 100.0, 0.0], 180.0, &[])
        );
        let bytes = ai_room_bsp(&entities, false);
        let (mut mdl, _) = ohl_formats::test_support::build_minimal_mdl10();
        let eye = [12.0_f32, 3.0, 20.0];
        for (axis, value) in eye.into_iter().enumerate() {
            mdl[76 + axis * 4..80 + axis * 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut assets = crate::MemoryAssets::new();
        assets.insert(
            MonsterKind::AlienGrunt
                .default_model_path()
                .expect("default"),
            mdl.clone(),
        );
        assets.insert(
            MonsterKind::Barney.default_model_path().expect("default"),
            mdl,
        );
        let mut game = crate::Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("fixture");
        let attacker = entity_of_classname(&game, "monster_alien_grunt").expect("attacker");
        let target = entity_of_classname(&game, "monster_barney").expect("target");
        let (level, systems) = game.level_and_systems_mut();
        let (projectiles, hitboxes) = super::grenade_emission_tests::context_parts(level);
        let grenade = GrenadeSafetyContext {
            projectiles: &projectiles,
            hitboxes: &hitboxes,
        };
        let ai = systems.ai_mut();
        ai.resolve_attack(
            level,
            attacker,
            AttackKind::Range1,
            Some(target),
            &mut Vec::new(),
            &grenade,
        );
        let requests = ai.take_projectile_requests();
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        let muzzle = Vec3::new(-3.0, 12.0, 20.0);
        let aim = Vec3::new(88.0, 97.0, 20.0);
        assert!(request.origin.abs_diff_eq(muzzle, 0.001));
        assert!(
            request
                .velocity
                .normalize()
                .abs_diff_eq((aim - muzzle).normalize(), 0.001)
        );
        assert_eq!(request.owner, attacker);
        assert_eq!(request.target, Some(target));
    }

    fn script_fixture(
        wall: bool,
        step: bool,
        move_to: &str,
        navigation: Option<usize>,
    ) -> crate::Game {
        let entities = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}{}{}{}",
            entity_block("info_player_start", [-200.0, -200.0, 36.0], 0.0, &[]),
            entity_block(
                "monster_barney",
                [-100.0, 0.0, 0.0],
                0.0,
                &[("targetname", "ohl_actor"), ("spawnflags", "16")]
            ),
            entity_block(
                "scripted_sequence",
                [100.0, 0.0, if step { 12.0 } else { 0.0 }],
                0.0,
                &[
                    ("targetname", "ohl_script"),
                    ("m_iszEntity", "ohl_actor"),
                    ("m_fMoveTo", move_to),
                    ("target", "ohl_done")
                ]
            ),
            entity_block("trigger_auto", [0.0; 3], 0.0, &[("target", "ohl_script")]),
            entity_block(
                "trigger_changelevel",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "ohl_done"),
                    ("map", "ohlelsewhere"),
                    ("landmark", "ohl_landmark")
                ]
            )
        );
        // A low step makes the straight segment fail while traced local
        // walking without a navigator remains reachable.
        let mut builder = ohl_formats::test_support::Bsp30Builder::new();
        builder.set_entities_text(&entities);
        let mut brushes = vec![ohl_formats::test_support::CollisionBrush::half_space(
            [0.0, 0.0, 1.0],
            0.0,
        )];
        if wall || step {
            brushes.push(ohl_formats::test_support::CollisionBrush::box_brush(
                [0.0, -256.0, 0.0],
                [
                    if wall { 16.0 } else { 200.0 },
                    256.0,
                    if wall { 256.0 } else { 12.0 },
                ],
            ));
        } else {
            // Flat graph case: a short wall forces a real detour, so merely
            // attaching a graph while taking a direct segment cannot pass.
            brushes.push(ohl_formats::test_support::CollisionBrush::box_brush(
                [-8.0, -48.0, 0.0],
                [8.0, 48.0, 128.0],
            ));
        }
        let heads = builder.push_collision_hulls(&brushes);
        builder.push_model(
            [-512.0, -512.0, -256.0],
            [512.0; 3],
            [0.0; 3],
            heads,
            2,
            0,
            0,
        );
        let bytes = builder.build();
        let mut game = crate::Game::from_map_bytes(&crate::MemoryAssets::new(), AI_MAP, &bytes)
            .expect("fixture");
        if let Some(searches) = navigation {
            let (level, systems) = game.level_and_systems_mut();
            let mut seeds = vec![
                ohl_nav::NodeSeed::new(Vec3::new(-100.0, 0.0, 8.0), ohl_nav::NodeKind::Ground),
                ohl_nav::NodeSeed::new(Vec3::new(100.0, 0.0, 20.0), ohl_nav::NodeKind::Ground),
            ];
            if !wall && !step {
                seeds = [-100.0, 0.0, 100.0]
                    .into_iter()
                    .flat_map(|x| {
                        [-96.0, 0.0, 96.0].into_iter().map(move |y| {
                            ohl_nav::NodeSeed::new(Vec3::new(x, y, 8.0), ohl_nav::NodeKind::Ground)
                        })
                    })
                    .collect();
            }
            let bridge = ohl_ai::NavBridge::build(
                &seeds,
                level.monster_collision.as_ref().expect("collision"),
                &ohl_nav::BuildLimits::default(),
                ohl_ai::NavBridgeLimits {
                    max_searches_per_tick: searches,
                    ..ohl_ai::NavBridgeLimits::default()
                },
            );
            systems.ai_mut().world.attach_navigator(bridge);
        }
        game
    }

    #[test]
    fn script_steps_reach_a_floor_mark_with_graph_without_graph_and_legacy_fallback() {
        // Graph steering over a step is independently reproduced in
        // ohl-ai/tests/nav_bridge.rs and reserved for P5. Here the graph
        // success case is flat; no-navigator movement still climbs the step.
        // With a navigator but no search budget, M9.42's explicit legacy
        // fallback crosses the flat wall until P5 replaces that policy.
        for (navigation, step) in [(None, true), (Some(8), false), (Some(0), false)] {
            let mut game = script_fixture(false, step, "1", navigation);
            let mut fired = 0;
            for _ in 0..1_200 {
                fired += game
                    .tick(crate::TICK_SECONDS, &crate::Input::default())
                    .iter()
                    .filter(|event| matches!(event, crate::GameEvent::LevelChange { .. }))
                    .count();
            }
            let entity = entity_of_classname(&game, "monster_barney").expect("actor");
            let pose = game
                .registry()
                .world
                .get::<&Actor>(entity)
                .expect("actor")
                .origin;
            let stats = game.script_navigation_stats();
            assert_eq!(
                fired, 1,
                "completion target fires once: {navigation:?}, pose={pose:?}, stats={stats:?}"
            );
            assert_eq!(game.script_completion_count(), 1);
            assert_eq!(game.script_timeout_count(), 0);
            let actor = entity_of_classname(&game, "monster_barney").expect("actor");
            let actor = game.registry().world.get::<&Actor>(actor).expect("actor");
            assert!(actor.origin.x > 65.0);
            assert!((actor.origin.z - if step { 12.0 } else { 0.0 }).abs() < 0.1);
            let stats = game.script_navigation_stats();
            if navigation == Some(0) {
                assert!(stats.untraced_steps > 0, "explicit legacy fallback used");
                assert!(stats.start_solid > 0, "legacy path crossed the flat wall");
                assert_eq!(stats.graph_steps, 0);
            } else {
                assert_eq!(stats.untraced_steps, 0);
                assert_eq!(stats.start_solid, 0);
                if navigation == Some(8) {
                    assert!(stats.graph_steps > 0, "completion used the graph detour");
                } else {
                    assert!(stats.traced_steps > 0);
                }
            }
        }
    }

    #[test]
    fn a_blocked_script_without_a_navigator_cannot_warp_but_explicit_teleport_can() {
        for (move_to, completions) in [("1", 0), ("4", 1)] {
            let mut game = script_fixture(true, false, move_to, None);
            let mut fired = 0;
            for _ in 0..4_000 {
                fired += game
                    .tick(crate::TICK_SECONDS, &crate::Input::default())
                    .iter()
                    .filter(|event| matches!(event, crate::GameEvent::LevelChange { .. }))
                    .count();
            }
            assert_eq!(fired, completions);
            assert_eq!(game.script_completion_count(), completions as u64);
            let actor = entity_of_classname(&game, "monster_barney").expect("actor");
            let actor = game.registry().world.get::<&Actor>(actor).expect("actor");
            if completions == 0 {
                assert!(actor.origin.x < 0.0);
            } else {
                assert_eq!(actor.origin, Vec3::X * 100.0);
            }
            assert_eq!(game.script_navigation_stats().untraced_steps, 0);
        }
    }
}

#[cfg(test)]
mod projectile_queue_tests {
    use super::*;

    #[test]
    fn normal_sink_is_bounded_rejects_non_finite_requests_and_drains_once() {
        let mut world = ohl_game::hecs::World::new();
        let owner = world.spawn(());
        let request = ProjectileRequest {
            kind: ohl_combat::ProjectileKind::Rocket,
            owner,
            origin: Vec3::ZERO,
            velocity: Vec3::X,
            damage: 100.0,
            damage_type: DamageType::BLAST,
            blast_radius: Some(200.0),
            target: None,
        };
        let mut ai = AiState::new(0);
        ai.projectiles.spawn_projectile(&ProjectileRequest {
            damage: f32::NAN,
            ..request
        });
        assert!(ai.take_projectile_requests().is_empty());
        for _ in 0..150 {
            ai.projectiles.spawn_projectile(&request);
        }
        assert_eq!(ai.take_projectile_requests().len(), 128);
        assert!(ai.take_projectile_requests().is_empty());
        ai.set_projectile_spawner(Box::new(NoProjectiles));
        ai.projectiles.spawn_projectile(&request);
        assert!(
            ai.take_projectile_requests().is_empty(),
            "custom sink is the only route"
        );
    }

    fn live_fixture(classname: &str) -> (crate::Game, Entity) {
        let text = format!(
            "{{\"classname\" \"worldspawn\"}}\n{{\"classname\" \"info_player_start\" \"origin\" \"0 0 36\"}}\n{{\"classname\" \"{classname}\" \"origin\" \"128 0 36\" \"angle\" \"180\"}}\n"
        );
        let bytes = crate::test_support::ai_room_bsp(&text, false);
        let mut game =
            crate::Game::from_map_bytes(&crate::MemoryAssets::new(), "ohlaisynth", &bytes)
                .expect("fixture");
        let actor = crate::test_support::monster_entities(&game)[0];
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        (game, actor)
    }

    #[test]
    fn live_secondary_readiness_excludes_prisoners_scripts_dormant_aircraft_and_dead_actors() {
        for guard in 0..4 {
            let (mut game, actor) = live_fixture(if guard == 2 {
                "monster_apache"
            } else {
                "monster_human_grunt"
            });
            let (level, systems) = game.level_and_systems_mut();
            let ai = systems.ai_mut();
            ai.update_secondary_opportunities(level, crate::TICK_SECONDS);
            assert!(
                level
                    .registry
                    .world
                    .get::<&MonsterAi>(actor)
                    .expect("ai")
                    .pending_conditions
                    .contains(Conditions::CAN_RANGE_ATTACK2),
                "live opportunity stays ready"
            );
            match guard {
                0 => {
                    level
                        .registry
                        .world
                        .insert_one(actor, Prisoner)
                        .expect("prisoner");
                }
                1 => {
                    level
                        .registry
                        .world
                        .insert_one(actor, ScriptHold)
                        .expect("script hold");
                }
                2 => {
                    level
                        .registry
                        .world
                        .insert_one(
                            actor,
                            ohl_ai::monsters::FlightPlan::new(Vec::new(), true, 100.0)
                                .starting_inactive(),
                        )
                        .expect("dormant flight");
                }
                _ => {
                    level
                        .registry
                        .world
                        .get::<&mut Actor>(actor)
                        .expect("actor")
                        .alive = false;
                }
            }
            ai.update_secondary_opportunities(level, crate::TICK_SECONDS);
            assert!(
                !level
                    .registry
                    .world
                    .get::<&MonsterAi>(actor)
                    .expect("ai")
                    .pending_conditions
                    .contains(Conditions::CAN_RANGE_ATTACK2)
            );
        }
    }

    #[test]
    fn stale_attack_tasks_cannot_launch_from_a_dead_actor() {
        let (mut game, actor) = live_fixture("monster_human_grunt");
        let (level, systems) = game.level_and_systems_mut();
        let (projectiles, hitboxes) = super::grenade_emission_tests::context_parts(level);
        let grenade = GrenadeSafetyContext {
            projectiles: &projectiles,
            hitboxes: &hitboxes,
        };
        let ai = systems.ai_mut();
        assert!(
            ai.take_projectile_requests().is_empty(),
            "public tick drained all requests"
        );
        level
            .registry
            .world
            .get::<&mut Actor>(actor)
            .expect("actor")
            .alive = false;
        ai.resolve_attack(
            level,
            actor,
            AttackKind::Range2,
            Some(level.player),
            &mut Vec::new(),
            &grenade,
        );
        assert!(ai.take_projectile_requests().is_empty());
    }

    const GRENADE_WALL_MIN: Vec3 = Vec3::new(-8.0, -128.0, 0.0);
    const GRENADE_WALL_MAX: Vec3 = Vec3::new(8.0, 128.0, 256.0);

    fn grenade_wall_fixture(
        classname: &str,
        wall_class: &str,
        activate: bool,
    ) -> (crate::Game, Entity) {
        grenade_wall_fixture_with_player_prisoner(classname, wall_class, activate, false)
    }

    fn grenade_wall_fixture_with_player_prisoner(
        classname: &str,
        wall_class: &str,
        activate: bool,
        player_prisoner: bool,
    ) -> (crate::Game, Entity) {
        use crate::test_support::{AI_MAP, entity_block};
        use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
        let entities = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}{}{}{}",
            entity_block("info_player_start", [160.0, 0.0, 36.0], 180.0, &[]),
            entity_block(
                classname,
                [-160.0, 0.0, 0.0],
                0.0,
                &[("targetname", "ohl_thrower")]
            ),
            entity_block(
                "monster_human_grunt",
                [0.0, 96.0, 0.0],
                0.0,
                &[("targetname", "ohl_holder")]
            ),
            entity_block(
                wall_class,
                [0.0; 3],
                0.0,
                &[
                    ("model", "*1"),
                    ("targetname", "ohl_grenade_wall"),
                    ("spawnflags", "1")
                ]
            ),
            if activate {
                "{\"classname\" \"trigger_auto\" \"target\" \"ohl_grenade_wall\"}"
            } else {
                ""
            },
        );
        let mut bsp = Bsp30Builder::new();
        bsp.set_entities_text(&entities);
        let floor = bsp.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
        bsp.push_model([-512.0; 3], [512.0; 3], [0.0; 3], floor, 2, 0, 0);
        let wall = bsp.push_collision_hulls(&[CollisionBrush::box_brush(
            GRENADE_WALL_MIN.to_array(),
            GRENADE_WALL_MAX.to_array(),
        )]);
        bsp.push_model(
            GRENADE_WALL_MIN.to_array(),
            GRENADE_WALL_MAX.to_array(),
            [0.0; 3],
            wall,
            2,
            0,
            0,
        );
        let mut game =
            crate::Game::from_map_bytes(&crate::MemoryAssets::new(), AI_MAP, &bsp.build())
                .expect("generated grenade wall");
        let shooter = game.registry().find("ohl_thrower")[0];
        let holder = game.registry().find("ohl_holder")[0];
        if player_prisoner {
            // Held actors still sense: hide B before the first ordinary tick.
            let player = game.player_entity();
            game.registry_mut()
                .world
                .insert_one(player, Prisoner)
                .unwrap();
        }
        // Test-only possession keeps both actors stationary while real Game
        // ticks run wall activation and the normal phase-2a occupancy policy.
        for entity in [shooter, holder] {
            game.registry_mut()
                .world
                .insert_one(entity, ScriptHold)
                .expect("fixture hold");
        }
        for _ in 0..3 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
        }
        assert_eq!(game.projectile_count(), 0);
        (game, shooter)
    }

    fn seed_grenade_memory(game: &mut crate::Game, shooter: Entity) {
        let (level, systems) = game.level_and_systems_mut();
        let _ = level.registry.world.remove_one::<ScriptHold>(shooter);
        let enemy = *level
            .registry
            .world
            .get::<&Actor>(level.player)
            .expect("player actor");
        let actor = *level
            .registry
            .world
            .get::<&Actor>(shooter)
            .expect("shooter actor");
        let mut brain = level
            .registry
            .world
            .get::<&mut MonsterAi>(shooter)
            .expect("shooter brain");
        // Explicit seeded-memory control, not a claim of naturally acquired
        // sight through the physically blocked world.
        brain.memory = Some(EnemyMemory {
            entity: level.player,
            last_known_position: enemy.navigation_anchor(),
            time_since_seen: 0.0,
            occluded: false,
            last_known_distance: actor.eye().distance(enemy.eye()),
        });
        brain.state = MonsterState::Combat;
        brain.runner.clear();
        brain.pending_conditions = Conditions::EMPTY;
        assert!(systems.ai_mut().take_projectile_requests().is_empty());
    }

    fn grenade_world_traces(game: &mut crate::Game, shooter: Entity) -> (bool, bool) {
        let (level, _) = game.level_and_systems_mut();
        let shooter = *level.registry.world.get::<&Actor>(shooter).unwrap();
        let enemy = *level.registry.world.get::<&Actor>(level.player).unwrap();
        assert!(shooter.alive && enemy.alive && shooter.health > 0.0 && enemy.health > 0.0);
        assert!((96.0..=1024.0).contains(&shooter.eye().distance(enemy.eye())));
        let world = level.collision.as_ref().expect("actual projectile world");
        let monster = level.monster_collision.as_ref().expect("monster world");
        for actor in [shooter, enemy] {
            for model in [world, monster] {
                assert!(
                    !model
                        .trace(actor.hull, actor.query_origin(), actor.query_origin())
                        .start_solid,
                    "shooter and enemy are not embedded"
                );
            }
        }
        let trace = |model: &ohl_physics::CollisionModel| {
            let hit = model.trace(ohl_physics::Hull::Point, shooter.eye(), enemy.eye());
            assert!(!hit.start_solid && !hit.all_solid);
            hit.fraction < 1.0
        };
        (trace(world), trace(monster))
    }

    fn grenade_ready(game: &mut crate::Game, shooter: Entity) -> bool {
        let (level, systems) = game.level_and_systems_mut();
        systems
            .ai_mut()
            .update_secondary_opportunities(level, crate::TICK_SECONDS);
        level
            .registry
            .world
            .get::<&MonsterAi>(shooter)
            .unwrap()
            .pending_conditions
            .contains(Conditions::CAN_RANGE_ATTACK2)
    }

    fn think_for_grenade(game: &mut crate::Game) -> Vec<ProjectileRequest> {
        let (level, systems) = game.level_and_systems_mut();
        let (projectiles, hitboxes) = super::grenade_emission_tests::context_parts(level);
        let grenade = GrenadeSafetyContext {
            projectiles: &projectiles,
            hitboxes: &hitboxes,
        };
        // The real AI schedule/adapter runs; only memory was explicitly seeded.
        // No forced Range2 task is used by this admission/emission arm.
        for _ in 0..16 {
            systems
                .ai_mut()
                .think(level, crate::TICK_SECONDS, &mut Vec::new(), &grenade);
            let requests = systems.ai_mut().take_projectile_requests();
            if !requests.is_empty() {
                return requests;
            }
        }
        Vec::new()
    }

    fn forced_grenade_hits_actual_wall(game: &mut crate::Game, shooter: Entity) {
        let (level, systems) = game.level_and_systems_mut();
        // Explicit physics characterization bypasses emission safety only in this test.
        let requests = [super::grenade_emission_tests::forced_request(
            level,
            shooter,
            systems.ai_mut().difficulty,
        )];
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!(request.kind, ohl_combat::ProjectileKind::HandGrenade);
        assert_eq!(request.owner, shooter);
        assert_eq!(request.target, Some(level.player));
        assert!(request.origin.x < GRENADE_WALL_MIN.x - 64.0 && request.velocity.x > 0.0);
        let mut projectiles = crate::projectiles::ProjectileSystem::new(0);
        projectiles
            .spawn_request(level, &request)
            .expect("real engine adapter launch");
        let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut hitboxes, level);
        let mut damage = Vec::new();
        let mut sprites = crate::sprites::TransientSprites::default();
        let mut hit_wall = false;
        for _ in 0..80 {
            let before = projectiles.snapshot(level).projectiles[0];
            let position = Vec3::from_array(before.position);
            let velocity = Vec3::from_array(before.velocity)
                - Vec3::Z * ohl_physics::MoveConfig::default().gravity * crate::TICK_SECONDS;
            let predicted = level.collision.as_ref().unwrap().trace(
                ohl_physics::Hull::Point,
                position,
                position + velocity * crate::TICK_SECONDS,
            );
            projectiles.tick(
                level,
                &hitboxes,
                crate::TICK_SECONDS,
                &mut damage,
                &mut sprites,
            );
            let snapshot = projectiles.snapshot(level);
            assert_eq!(snapshot.projectiles.len(), 1, "impact precedes fuse expiry");
            let after = snapshot.projectiles[0];
            assert!(
                after.position[0] <= GRENADE_WALL_MIN.x,
                "the actual grenade never crosses the wall plane"
            );
            if after.velocity[0] < 0.0 {
                assert!(predicted.fraction < 1.0 && !predicted.start_solid);
                assert_eq!(predicted.plane_normal, Vec3::NEG_X);
                assert!((predicted.end_pos.x - GRENADE_WALL_MIN.x).abs() < 0.1);
                assert!(after.position[2] > 1.0 && after.position[2] < GRENADE_WALL_MAX.z);
                assert!(
                    after.position[1].abs() < 0.01,
                    "off-lane holder cannot intercept the grenade"
                );
                hit_wall = true;
                break;
            }
        }
        assert!(
            hit_wall,
            "forced adapter grenade reflects from the actual wall before crossing"
        );
        assert!(
            damage.is_empty(),
            "no blast/damage claim is made by the early-impact control"
        );
    }

    #[test]
    fn grenade_readiness_rejects_actual_wall_despite_npc_only_suspension() {
        for classname in ["monster_human_grunt", "monster_human_assassin"] {
            let (mut game, shooter) = grenade_wall_fixture(classname, "func_wall_toggle", true);
            let wall = game.registry().find("ohl_grenade_wall")[0];
            assert!(
                game.registry()
                    .world
                    .get::<&ohl_game::registry::WallToggle>(wall)
                    .unwrap()
                    .visible
            );
            let holder = game.registry().find("ohl_holder")[0];
            {
                let actor = game.registry().world.get::<&Actor>(holder).unwrap();
                assert!(actor.alive && actor.health > 0.0 && !actor.is_client);
                assert!(
                    game.registry()
                        .world
                        .get::<&ohl_ai::Impervious>(holder)
                        .is_err()
                );
                let (min, max) = actor.body_frame.world_bounds(actor.hull, actor.origin);
                assert!(min.cmplt(GRENADE_WALL_MAX).all() && max.cmpgt(GRENADE_WALL_MIN).all());
                assert!(min.y > 64.0, "holder is outside the launch corridor");
            }
            assert_eq!(grenade_world_traces(&mut game, shooter), (true, false));
            seed_grenade_memory(&mut game, shooter);
            assert!(
                !grenade_ready(&mut game, shooter),
                "grenade readiness rejects the actual projectile wall"
            );
            assert!(
                think_for_grenade(&mut game).is_empty(),
                "natural AI schedules enqueue no grenade through the held wall"
            );
            // A separate held-wall instance keeps forced emission/cooldown and
            // any natural hitscan movement out of the vacancy control below.
            let (mut forced, thrower) = grenade_wall_fixture(classname, "func_wall_toggle", true);
            assert_eq!(grenade_world_traces(&mut forced, thrower), (true, false));
            seed_grenade_memory(&mut forced, thrower);
            forced_grenade_hits_actual_wall(&mut forced, thrower);

            // Vacating the holder restores the monster model on a real tick.
            game.registry_mut()
                .world
                .insert_one(shooter, ScriptHold)
                .unwrap();
            game.registry()
                .world
                .get::<&mut Actor>(holder)
                .unwrap()
                .origin = Vec3::new(64.0, 96.0, 0.0);
            game.registry()
                .world
                .get::<&mut Transform>(holder)
                .unwrap()
                .origin = Vec3::new(64.0, 96.0, 0.0);
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            assert_eq!(grenade_world_traces(&mut game, shooter), (true, true));
            seed_grenade_memory(&mut game, shooter);
            assert!(!grenade_ready(&mut game, shooter));

            let (mut ordinary, shooter) = grenade_wall_fixture(classname, "func_wall", false);
            assert_eq!(grenade_world_traces(&mut ordinary, shooter), (true, true));
            seed_grenade_memory(&mut ordinary, shooter);
            assert!(!grenade_ready(&mut ordinary, shooter));
            assert!(think_for_grenade(&mut ordinary).is_empty());

            let (mut missing, shooter) = grenade_wall_fixture(classname, "func_wall_toggle", false);
            assert_eq!(grenade_world_traces(&mut missing, shooter), (false, false));
            seed_grenade_memory(&mut missing, shooter);
            missing.level_and_systems_mut().0.collision = None;
            assert!(
                !grenade_ready(&mut missing, shooter),
                "missing actual collision refuses grenade readiness"
            );
            assert!(think_for_grenade(&mut missing).is_empty());
        }
    }

    #[test]
    fn grenade_readiness_wall_off_emits_for_both_species() {
        for classname in ["monster_human_grunt", "monster_human_assassin"] {
            let (mut game, shooter) = grenade_wall_fixture(classname, "func_wall_toggle", false);
            // Same wall-off policy; emission safety also requires a clear owner margin.
            super::grenade_emission_tests::place(&game, shooter, Vec3::new(-640.0, 0.0, 0.0));
            assert_eq!(grenade_world_traces(&mut game, shooter), (false, false));
            seed_grenade_memory(&mut game, shooter);
            assert!(
                grenade_ready(&mut game, shooter),
                "clear actual world permits grenade readiness"
            );
            let requests = think_for_grenade(&mut game);
            assert_eq!(
                requests.len(),
                1,
                "natural secondary schedule emits one request"
            );
            assert_eq!(requests[0].kind, ohl_combat::ProjectileKind::HandGrenade);
            assert_eq!(requests[0].owner, shooter);
            assert_eq!(requests[0].target, Some(game.player_entity()));
        }
    }

    fn apache_wall_fixture(activate: bool) -> (crate::Game, Entity) {
        apache_wall_fixture_with_player_prisoner(activate, false)
    }

    fn apache_wall_fixture_with_player_prisoner(
        activate: bool,
        player_prisoner: bool,
    ) -> (crate::Game, Entity) {
        let (mut game, shooter) = grenade_wall_fixture_with_player_prisoner(
            "monster_apache",
            "func_wall_toggle",
            activate,
            player_prisoner,
        );
        // Reuse the real held-toggle activation, but put the aircraft above
        // the floor and give its ordinary flight controller an active route.
        let origin = Vec3::new(-160.0, 0.0, 96.0);
        game.registry()
            .world
            .get::<&mut Actor>(shooter)
            .unwrap()
            .origin = origin;
        game.registry()
            .world
            .get::<&mut Transform>(shooter)
            .unwrap()
            .origin = origin;
        game.registry_mut()
            .world
            .insert_one(
                shooter,
                ohl_ai::monsters::FlightPlan::new(
                    vec![origin + Vec3::Y * 32.0, origin - Vec3::Y * 32.0],
                    true,
                    100.0,
                ),
            )
            .unwrap();
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert!(
            game.registry()
                .world
                .get::<&ohl_ai::monsters::FlightPlan>(shooter)
                .unwrap()
                .is_active()
        );
        assert_eq!(game.projectile_count(), 0);
        (game, shooter)
    }

    fn characterize_forced_rocket_wall(game: &mut crate::Game, shooter: Entity) {
        let (level, systems) = game.level_and_systems_mut();
        // Explicit forced physics setup, not a production admission bypass.
        let kind = ohl_combat::ProjectileKind::Rocket;
        let origin = level.registry.world.get::<&Actor>(shooter).unwrap().eye();
        let aim = level
            .registry
            .world
            .get::<&Actor>(level.player)
            .unwrap()
            .eye();
        let difficulty = systems.ai_mut().difficulty;
        let (damage, damage_type, blast_radius) = monster_projectile_profile(kind, difficulty);
        let requests = [ProjectileRequest {
            kind,
            owner: shooter,
            origin,
            velocity: monster_projectile_velocity(kind, difficulty, origin, aim),
            damage,
            damage_type,
            blast_radius,
            target: Some(level.player),
        }];
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!(request.kind, ohl_combat::ProjectileKind::Rocket);
        assert_eq!(request.owner, shooter);
        let mut projectiles = crate::projectiles::ProjectileSystem::new(0);
        projectiles.spawn_request(level, &request).unwrap();
        let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut hitboxes, level);
        let mut damage = Vec::new();
        let mut sprites = crate::sprites::TransientSprites::default();
        for _ in 0..100 {
            let before = projectiles.snapshot(level).projectiles[0];
            let start = Vec3::from_array(before.position);
            let end = start + Vec3::from_array(before.velocity) * crate::TICK_SECONDS;
            let trace =
                level
                    .collision
                    .as_ref()
                    .unwrap()
                    .trace(ohl_physics::Hull::Point, start, end);
            projectiles.tick(
                level,
                &hitboxes,
                crate::TICK_SECONDS,
                &mut damage,
                &mut sprites,
            );
            if projectiles.snapshot(level).projectiles.is_empty() {
                assert!(trace.fraction < 1.0 && !trace.start_solid && !trace.all_solid);
                assert_eq!(trace.plane_normal, Vec3::NEG_X);
                assert!((trace.end_pos.x - GRENADE_WALL_MIN.x).abs() < 0.1);
                return;
            }
        }
        panic!("forced rocket must detonate against the actual held wall");
    }

    #[test]
    fn apache_rocket_readiness_rejects_actual_wall_despite_npc_only_suspension() {
        let (mut forced, shooter) = apache_wall_fixture(true);
        assert_eq!(grenade_world_traces(&mut forced, shooter), (true, false));
        characterize_forced_rocket_wall(&mut forced, shooter);

        let (mut game, shooter) = apache_wall_fixture(true);
        let wall = game.registry().find("ohl_grenade_wall")[0];
        assert!(
            game.registry()
                .world
                .get::<&ohl_game::registry::WallToggle>(wall)
                .unwrap()
                .visible
        );
        let holder = game.registry().find("ohl_holder")[0];
        {
            let actor = game.registry().world.get::<&Actor>(holder).unwrap();
            assert!(actor.alive && actor.health > 0.0 && !actor.is_client);
            assert!(
                game.registry()
                    .world
                    .get::<&ohl_ai::Impervious>(holder)
                    .is_err()
            );
            let (min, max) = actor.body_frame.world_bounds(actor.hull, actor.origin);
            assert!(min.cmplt(GRENADE_WALL_MAX).all() && max.cmpgt(GRENADE_WALL_MIN).all());
            assert!(min.y > 64.0, "holder is outside the rocket corridor");
        }
        assert_eq!(grenade_world_traces(&mut game, shooter), (true, false));
        seed_grenade_memory(&mut game, shooter);
        assert!(
            !grenade_ready(&mut game, shooter),
            "Apache secondary admission rejects the actual projectile wall"
        );
        assert!(
            think_for_grenade(&mut game).is_empty(),
            "natural Apache schedule emits no rocket through the held wall"
        );
        // No no-damage oracle: Apache primary hitscan is a separate path.
    }

    #[test]
    // Exact equality separates rocket launch from subsequent flight damage.
    #[allow(clippy::float_cmp)]
    fn apache_rocket_readiness_wall_off_emits_and_hurts_player() {
        let (mut game, shooter) = apache_wall_fixture(false);
        assert_eq!(grenade_world_traces(&mut game, shooter), (false, false));
        seed_grenade_memory(&mut game, shooter);
        assert!(grenade_ready(&mut game, shooter));
        let requests = think_for_grenade(&mut game);
        assert_eq!(requests.len(), 1, "natural Apache schedule emits a rocket");
        assert_eq!(requests[0].kind, ohl_combat::ProjectileKind::Rocket);
        assert_eq!(requests[0].owner, shooter);
        assert_eq!(requests[0].target, Some(game.player_entity()));
        // Feed the naturally emitted request back through the real phase-8
        // drain. Hold only its owner so later hitscan cannot satisfy damage.
        game.registry_mut()
            .world
            .insert_one(shooter, ScriptHold)
            .unwrap();
        game.level_and_systems_mut()
            .1
            .ai_mut()
            .projectiles
            .spawn_projectile(&requests[0]);
        let before = game.player_health();
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 1);
        assert_eq!(game.player_health(), before);
        for _ in 0..100 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
        }
        assert_eq!(
            game.projectile_count(),
            0,
            "real rocket detonated on contact"
        );
        assert!(
            game.player_health() < before,
            "admitted rocket hurts the player"
        );
    }

    // Ordinary Game ticks acquire A and finish/reselect into the rocket schedule.
    // No memory, condition or task cursor is assigned by this fixture.
    #[allow(clippy::too_many_lines)]
    fn secondary_retarget_fixture(wall_on: bool) -> (crate::Game, Entity, Entity) {
        let (mut game, shooter) = apache_wall_fixture_with_player_prisoner(wall_on, true);
        let player = game.player_entity();
        assert!(game.registry().world.get::<&Prisoner>(player).is_ok());
        {
            let ai = game.registry().world.get::<&MonsterAi>(shooter).unwrap();
            assert!(ai.memory.is_none(), "B was never sensed during held setup");
            assert!(ai.runner.schedule().is_none());
        }
        let mut first = *game.registry().world.get::<&Actor>(player).unwrap();
        first.origin = Vec3::new(320.0, 440.0, 64.0);
        first.is_client = false;
        first.health = 1_000.0;
        let first_target = game.registry_mut().world.spawn((
            first,
            Transform {
                origin: first.origin,
                angles: Vec3::ZERO,
            },
            ohl_combat::Health::new(first.health),
        ));
        let origin = game.registry().world.get::<&Actor>(shooter).unwrap().origin;
        // A real, active one-node route holds the aircraft at its authored point.
        game.registry_mut()
            .world
            .insert_one(
                shooter,
                ohl_ai::monsters::FlightPlan::new(vec![origin], true, 100.0),
            )
            .unwrap();
        game.registry_mut()
            .world
            .remove_one::<ScriptHold>(shooter)
            .unwrap();
        for _ in 0..600 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            assert_eq!(
                game.projectile_count(),
                0,
                "no secondary has run during setup"
            );
            let selected = {
                let ai = game.registry().world.get::<&MonsterAi>(shooter).unwrap();
                ai.runner.schedule_name() == "ohl/monsters/apache_rocket"
                    && ai.runner.task_index() == 0
                    && !ai.runner.started()
            };
            if selected {
                let ai = game.registry().world.get::<&MonsterAi>(shooter).unwrap();
                assert_eq!(ai.memory.unwrap().entity, first_target);
                drop(ai);
                let (level, systems) = game.level_and_systems_mut();
                let owner = *level.registry.world.get::<&Actor>(shooter).unwrap();
                let a = *level.registry.world.get::<&Actor>(first_target).unwrap();
                let b = *level.registry.world.get::<&Actor>(player).unwrap();
                let holder = level.registry.find("ohl_holder")[0];
                let holder_actor = *level.registry.world.get::<&Actor>(holder).unwrap();
                let relationships = systems.ai_mut().world.relationships();
                let to_a = relationships.get(owner.classification, a.classification);
                assert!(to_a.is_hostile());
                assert_eq!(
                    to_a,
                    relationships.get(owner.classification, b.classification)
                );
                assert!(
                    !relationships
                        .get(owner.classification, holder_actor.classification)
                        .is_hostile()
                );
                assert!(holder_actor.alive && holder_actor.health > 0.0);
                let (min, max) = holder_actor
                    .body_frame
                    .world_bounds(holder_actor.hull, holder_actor.origin);
                assert!(min.cmplt(GRENADE_WALL_MAX).all() && max.cmpgt(GRENADE_WALL_MIN).all());
                assert!(min.y > 64.0);
                for actor in [owner, a, b] {
                    assert!(actor.alive && actor.health > 0.0);
                    let trace = level.collision.as_ref().unwrap().trace(
                        actor.hull,
                        actor.query_origin(),
                        actor.query_origin(),
                    );
                    assert!(!trace.start_solid && !trace.all_solid);
                }
                assert!(owner.eye().distance(b.eye()) < owner.eye().distance(a.eye()));
                assert!(secondary_target_clear(
                    &MonsterKind::Apache,
                    systems.ai_mut().difficulty,
                    level.collision.as_ref(),
                    owner.eye(),
                    &a
                ));
                assert!(
                    level
                        .registry
                        .world
                        .get::<&ohl_ai::monsters::FlightPlan>(shooter)
                        .unwrap()
                        .is_active()
                );
                return (game, shooter, first_target);
            }
        }
        panic!("ordinary tasks must finish and reselect an unstarted secondary");
    }

    fn secondary_retarget_boundary(wall_on: bool) -> (crate::Game, Entity) {
        let (mut game, shooter, first_target) = secondary_retarget_fixture(wall_on);
        let player = game.player_entity();
        game.registry_mut()
            .world
            .remove_one::<Prisoner>(player)
            .unwrap();
        for _ in 0..100 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            assert_eq!(
                game.projectile_count(),
                0,
                "retarget/turn precedes the attack task"
            );
            let ready = {
                let ai = game.registry().world.get::<&MonsterAi>(shooter).unwrap();
                assert_eq!(
                    ai.memory.unwrap().entity,
                    player,
                    "ordinary senses prefer B"
                );
                assert_ne!(ai.memory.unwrap().entity, first_target);
                assert_eq!(ai.runner.schedule_name(), "ohl/monsters/apache_rocket");
                ai.runner.task() == Some(ohl_ai::Task::RangeAttack2) && !ai.runner.started()
            };
            if ready {
                let (level, systems) = game.level_and_systems_mut();
                let owner = *level.registry.world.get::<&Actor>(shooter).unwrap();
                let b = *level.registry.world.get::<&Actor>(player).unwrap();
                let actual = level.collision.as_ref().unwrap().trace(
                    ohl_physics::Hull::Point,
                    owner.eye(),
                    b.eye(),
                );
                let visible = level.monster_collision.as_ref().unwrap().trace(
                    ohl_physics::Hull::Point,
                    owner.eye(),
                    b.eye(),
                );
                assert!(!actual.start_solid && !actual.all_solid);
                assert!(!visible.start_solid && !visible.all_solid && visible.fraction >= 1.0);
                assert_eq!(actual.fraction < 1.0, wall_on);
                assert_eq!(
                    secondary_target_clear(
                        &MonsterKind::Apache,
                        systems.ai_mut().difficulty,
                        level.collision.as_ref(),
                        owner.eye(),
                        &b
                    ),
                    !wall_on
                );
                assert!(!systems.ai_mut().secondary_cooldowns.contains_key(&shooter));
                return (game, shooter);
            }
        }
        panic!("ordinary turning must reach the pending Range2 task");
    }

    fn assert_secondary_finished_without_request(game: &mut crate::Game, shooter: Entity) {
        let ai = game.registry().world.get::<&MonsterAi>(shooter).unwrap();
        assert_eq!(
            ai.runner.task(),
            Some(ohl_ai::Task::Wait(1.0)),
            "ordinary attack task completed"
        );
        drop(ai);
        assert_eq!(
            game.projectile_count(),
            0,
            "blocked retarget must not emit a secondary"
        );
        let ai = game.level_and_systems_mut().1.ai_mut();
        assert!(ai.take_projectile_requests().is_empty());
        assert!(!ai.secondary_cooldowns.contains_key(&shooter));
    }

    #[test]
    fn secondary_retargets_blocked_current_enemy_without_emitting() {
        let (mut game, shooter) = secondary_retarget_boundary(true);
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_secondary_finished_without_request(&mut game, shooter);
    }

    #[test]
    fn secondary_retargets_missing_actual_world_without_emitting() {
        let (mut game, shooter) = secondary_retarget_boundary(false);
        {
            let (level, systems) = game.level_and_systems_mut();
            let owner = *level.registry.world.get::<&Actor>(shooter).unwrap();
            let enemy = *level.registry.world.get::<&Actor>(level.player).unwrap();
            level.collision = None;
            assert!(!secondary_target_clear(
                &MonsterKind::Apache,
                systems.ai_mut().difficulty,
                level.collision.as_ref(),
                owner.eye(),
                &enemy
            ));
        }
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_secondary_finished_without_request(&mut game, shooter);
    }

    #[test]
    // The unadvanced launch and its ordinary cooldown/profile are exact copies.
    #[allow(clippy::float_cmp)]
    fn secondary_retargets_clear_current_enemy_and_rocket_hurts_player() {
        let (mut game, shooter) = secondary_retarget_boundary(false);
        let before = game.player_health();
        let (origin, velocity, profile) = {
            let (level, systems) = game.level_and_systems_mut();
            let origin = level.registry.world.get::<&Actor>(shooter).unwrap().eye();
            let aim = level
                .registry
                .world
                .get::<&Actor>(level.player)
                .unwrap()
                .eye();
            let kind = ohl_combat::ProjectileKind::Rocket;
            let difficulty = systems.ai_mut().difficulty;
            (
                origin,
                monster_projectile_velocity(kind, difficulty, origin, aim),
                monster_projectile_profile(kind, difficulty),
            )
        };
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 1);
        assert_eq!(
            game.player_health(),
            before,
            "launch precedes flight damage"
        );
        {
            let (level, systems) = game.level_and_systems_mut();
            let physical = systems.snapshot_projectiles(level);
            assert_eq!(physical.projectiles.len(), 1);
            let rocket = physical.projectiles[0];
            assert_eq!(
                rocket.kind_tag,
                crate::save_state::projectile_kind_tag(ohl_combat::ProjectileKind::Rocket)
            );
            assert_eq!(rocket.position, origin.to_array());
            assert_eq!(rocket.velocity, velocity.to_array());
            assert_eq!(rocket.age, 0.0);
            let runtime = systems.snapshot_projectile_runtime(level).unwrap();
            assert_eq!(runtime.attacks.len(), 1);
            let attack = runtime.attacks[0];
            assert_eq!(attack.id, rocket.id);
            assert_eq!(
                attack.owner,
                crate::save_state::projectile_entity_ref(level, entity_id(shooter))
            );
            assert!(attack.owner.is_some());
            assert_eq!(
                attack.target,
                Some(crate::save_state::ProjectileEntityRef::Player)
            );
            assert_eq!(
                (attack.damage, attack.damage_bits, attack.blast_radius),
                (profile.0, profile.1.bits(), profile.2)
            );
            assert_eq!(
                systems.ai_mut().secondary_cooldowns.get(&shooter),
                Some(&6.0)
            );
        }
        // Hold only after the natural launch; later primary fire cannot satisfy hurt.
        game.registry_mut()
            .world
            .insert_one(shooter, ScriptHold)
            .unwrap();
        for _ in 0..100 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
        }
        assert_eq!(
            game.projectile_count(),
            0,
            "ordinary rocket contact detonates"
        );
        assert!(
            game.player_health() < before,
            "naturally retargeted rocket hurts B"
        );
    }

    fn controller_wall_fixture(activate: bool) -> (crate::Game, Entity) {
        let (mut game, shooter) =
            grenade_wall_fixture("monster_alien_controller", "func_wall_toggle", activate);
        // Same real held-toggle fixture as Apache, with the controller's own
        // ordinary hovering brain rather than an invented aircraft flight plan.
        super::grenade_emission_tests::place(&game, shooter, Vec3::new(-160.0, 0.0, 96.0));
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 0);
        (game, shooter)
    }

    fn characterize_forced_head_ball_wall(game: &mut crate::Game, shooter: Entity) {
        use ohl_combat::{
            ProjectileEvent, ProjectileKind, ProjectileLimits, ProjectileSet, ProjectileTuning,
            ProjectileWorld,
        };
        let (level, systems) = game.level_and_systems_mut();
        let (_, hitboxes) = super::grenade_emission_tests::context_parts(level);
        // Explicit forced physics setup, not a production admission bypass.
        let kind = ProjectileKind::ControllerHomingBall;
        let origin = level.registry.world.get::<&Actor>(shooter).unwrap().eye();
        let aim = level
            .registry
            .world
            .get::<&Actor>(level.player)
            .unwrap()
            .eye();
        let difficulty = systems.ai_mut().difficulty;
        let (damage, damage_type, blast_radius) = monster_projectile_profile(kind, difficulty);
        let requests = [ProjectileRequest {
            kind,
            owner: shooter,
            origin,
            velocity: monster_projectile_velocity(kind, difficulty, origin, aim),
            damage,
            damage_type,
            blast_radius,
            target: Some(level.player),
        }];
        assert_eq!(requests.len(), 1);
        let request = requests[0];
        assert_eq!(request.kind, ProjectileKind::ControllerHomingBall);
        assert_eq!(request.owner, shooter);
        let tuning = ProjectileTuning::default();
        let movement = ohl_physics::MoveConfig::default();
        let world = ProjectileWorld {
            collision: level.collision.as_ref().unwrap(),
            entities: &hitboxes,
            movement: &movement,
            tuning: &tuning,
        };
        let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
        let id = set
            .spawn(
                request.kind,
                Some(entity_id(shooter)),
                request.origin,
                request.velocity,
                &tuning,
            )
            .unwrap();
        set.get_mut(id).unwrap().target = request.target.map(entity_id);
        for _ in 0..100 {
            let mut events = Vec::new();
            set.tick(crate::TICK_SECONDS, &world, &mut events);
            for event in events {
                if let ProjectileEvent::Impact {
                    kind,
                    entity,
                    normal,
                    position,
                    ..
                } = event
                {
                    assert_eq!(kind, ProjectileKind::ControllerHomingBall);
                    assert_eq!(entity, None, "real integrator contact is with the world");
                    assert_eq!(normal, Vec3::NEG_X);
                    assert!((position.x - GRENADE_WALL_MIN.x).abs() < 0.1);
                    assert!(
                        set.get(id).is_none(),
                        "head ball stops at its real brush impact"
                    );
                    return;
                }
            }
        }
        panic!("forced head ball must hit the actual held wall");
    }

    #[test]
    fn controller_head_ball_readiness_rejects_actual_wall_and_missing_world() {
        let (mut forced, shooter) = controller_wall_fixture(true);
        assert_eq!(grenade_world_traces(&mut forced, shooter), (true, false));
        characterize_forced_head_ball_wall(&mut forced, shooter);
        let (mut game, shooter) = controller_wall_fixture(true);
        let wall = game.registry().find("ohl_grenade_wall")[0];
        assert!(
            game.registry()
                .world
                .get::<&ohl_game::registry::WallToggle>(wall)
                .unwrap()
                .visible
        );
        let holder = game.registry().find("ohl_holder")[0];
        {
            let actor = game.registry().world.get::<&Actor>(holder).unwrap();
            assert!(actor.alive && actor.health > 0.0 && !actor.is_client);
            assert!(
                game.registry()
                    .world
                    .get::<&ohl_ai::Impervious>(holder)
                    .is_err()
            );
            let (min, max) = actor.body_frame.world_bounds(actor.hull, actor.origin);
            assert!(min.cmplt(GRENADE_WALL_MAX).all() && max.cmpgt(GRENADE_WALL_MIN).all());
            assert!(min.y > 64.0, "holder is outside the head-ball corridor");
        }
        assert_eq!(grenade_world_traces(&mut game, shooter), (true, false));
        seed_grenade_memory(&mut game, shooter);
        assert!(
            !grenade_ready(&mut game, shooter),
            "controller secondary admission rejects the actual projectile wall"
        );
        assert!(
            think_for_grenade(&mut game)
                .iter()
                .all(|r| r.kind != ohl_combat::ProjectileKind::ControllerHomingBall),
            "natural blocked schedule emits no secondary head ball"
        );
        // The primary volley remains independent; do not assert no other attack.
        let (mut missing, shooter) = controller_wall_fixture(false);
        seed_grenade_memory(&mut missing, shooter);
        missing.level_and_systems_mut().0.collision = None;
        assert!(
            !grenade_ready(&mut missing, shooter),
            "missing actual world refuses secondary"
        );
        assert!(
            think_for_grenade(&mut missing)
                .iter()
                .all(|r| r.kind != ohl_combat::ProjectileKind::ControllerHomingBall)
        );
    }

    #[test]
    // Exact health equality separates launch from actual homing-ball flight damage.
    #[allow(clippy::float_cmp)]
    fn controller_head_ball_readiness_wall_off_emits_and_hurts_player() {
        let (mut game, shooter) = controller_wall_fixture(false);
        // This wall-off arm needs no occupancy holder. A live human holder
        // competes with the player under the controller's ordinary senses.
        let holder = game.registry().find("ohl_holder")[0];
        game.registry_mut()
            .world
            .despawn(holder)
            .expect("remove the positive arm's competing target");
        assert_eq!(crate::test_support::monster_entities(&game), vec![shooter]);
        assert_eq!(grenade_world_traces(&mut game, shooter), (false, false));
        seed_grenade_memory(&mut game, shooter);
        assert!(grenade_ready(&mut game, shooter));
        let requests = think_for_grenade(&mut game);
        assert_eq!(
            requests.len(),
            1,
            "natural controller schedule emits one head ball"
        );
        assert_eq!(
            requests[0].kind,
            ohl_combat::ProjectileKind::ControllerHomingBall
        );
        assert_eq!(requests[0].owner, shooter);
        assert_eq!(requests[0].target, Some(game.player_entity()));
        assert_eq!(requests[0].damage_type, DamageType::SHOCK);
        assert_eq!(requests[0].blast_radius, None);
        game.registry_mut()
            .world
            .insert_one(shooter, ScriptHold)
            .unwrap();
        game.level_and_systems_mut()
            .1
            .ai_mut()
            .projectiles
            .spawn_projectile(&requests[0]);
        let before = game.player_health();
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 1);
        assert_eq!(game.player_health(), before);
        for _ in 0..100 {
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
        }
        assert_eq!(
            game.projectile_count(),
            0,
            "actual head ball stops on contact"
        );
        assert!(
            game.player_health() < before,
            "admitted head ball hurts the player"
        );
    }

    fn grenade_ceiling_room(classname: &str, ceiling: f32) -> (crate::Game, Entity) {
        use crate::test_support::{AI_MAP, entity_block};
        use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
        let text = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}",
            entity_block("info_player_start", [0.0, 0.0, 36.0], 180.0, &[]),
            entity_block(
                classname,
                [-160.0, 0.0, 0.0],
                0.0,
                &[("targetname", "ohl_lobber")]
            )
        );
        let mut bsp = Bsp30Builder::new();
        bsp.set_entities_text(&text);
        let heads = bsp.push_collision_hulls(&[
            CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
            CollisionBrush::half_space([0.0, 0.0, -1.0], -ceiling),
            CollisionBrush::half_space([1.0, 0.0, 0.0], -184.0),
            CollisionBrush::half_space([-1.0, 0.0, 0.0], -64.0),
            CollisionBrush::half_space([0.0, 1.0, 0.0], -32.0),
            CollisionBrush::half_space([0.0, -1.0, 0.0], -32.0),
        ]);
        bsp.push_model(
            [-184.0, -32.0, 0.0],
            [64.0, 32.0, ceiling],
            [0.0; 3],
            heads,
            1,
            0,
            0,
        );
        let mut game =
            crate::Game::from_map_bytes(&crate::MemoryAssets::new(), AI_MAP, &bsp.build())
                .expect("generated convex ceiling room");
        let shooter = game.registry().find("ohl_lobber")[0];
        game.registry_mut()
            .world
            .insert_one(shooter, ScriptHold)
            .unwrap();
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
        assert_eq!(game.projectile_count(), 0);
        (game, shooter)
    }

    // Exact defaults bind this generated control to the real engine integrator.
    #[allow(clippy::float_cmp)]
    fn grenade_ceiling_fixture(classname: &str, low: bool) -> (crate::Game, Entity, f32) {
        const _: () = assert!(crate::TICK_SECONDS <= ohl_combat::projectile::MAX_SUBSTEP_SECONDS);
        assert_eq!(
            ohl_combat::ProjectileTuning::default().gravity_scale.value,
            1.0
        );
        let (open, shooter) = grenade_ceiling_room(classname, 256.0);
        let actor = *open.registry().world.get::<&Actor>(shooter).unwrap();
        let enemy = *open
            .registry()
            .world
            .get::<&Actor>(open.player_entity())
            .unwrap();
        let mut position = actor.eye();
        let mut velocity = monster_projectile_velocity(
            ohl_combat::ProjectileKind::HandGrenade,
            AiDifficulty::Medium,
            position,
            enemy.eye(),
        );
        assert!(velocity.is_finite() && velocity.z > 0.0);
        let mut steps =
            (monster_lob_flight_seconds(position, enemy.eye()) / crate::TICK_SECONDS).ceil();
        let mut apex = position.z;
        while steps > 0.0 {
            velocity.z -= ohl_physics::MoveConfig::default().gravity * crate::TICK_SECONDS;
            position += velocity * crate::TICK_SECONDS;
            apex = apex.max(position.z);
            steps -= 1.0;
        }
        let top = [actor, enemy]
            .into_iter()
            .map(|a| {
                a.body_frame
                    .world_bounds(a.hull, a.origin)
                    .1
                    .z
                    .max(a.eye().z)
            })
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            apex > top + 2.0,
            "real launch rises above both occupied hulls"
        );
        let ceiling = if low { top.midpoint(apex) } else { apex + 16.0 };
        let (mut game, shooter) = grenade_ceiling_room(classname, ceiling);
        assert_eq!(
            game.registry().world.get::<&Actor>(shooter).unwrap().origin,
            actor.origin
        );
        assert_eq!(
            game.registry()
                .world
                .get::<&Actor>(game.player_entity())
                .unwrap()
                .origin,
            enemy.origin
        );
        assert_eq!(grenade_world_traces(&mut game, shooter), (false, false));
        let collision = game.level_and_systems_mut().0.collision.as_ref().unwrap();
        for a in [actor, enemy] {
            let (min, max) = a.body_frame.world_bounds(a.hull, a.origin);
            assert!(min.x > -184.0 && max.x < 64.0 && min.y > -32.0 && max.y < 32.0);
            assert!(min.z >= 0.0 && max.z < ceiling);
            let support = collision.trace(
                a.hull,
                a.query_origin() + Vec3::Z,
                a.query_origin() - Vec3::Z * 4.0,
            );
            assert!(!support.start_solid && support.fraction < 1.0 && support.plane_normal.z > 0.9);
        }
        (game, shooter, ceiling)
    }

    fn characterize_forced_ceiling_impact(game: &mut crate::Game, shooter: Entity, ceiling: f32) {
        let (level, systems) = game.level_and_systems_mut();
        // Explicit physics characterization bypasses emission safety only in this test.
        let requests = [super::grenade_emission_tests::forced_request(
            level,
            shooter,
            systems.ai_mut().difficulty,
        )];
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].kind, ohl_combat::ProjectileKind::HandGrenade);
        assert_eq!(requests[0].owner, shooter);
        let mut projectiles = crate::projectiles::ProjectileSystem::new(0);
        projectiles.spawn_request(level, &requests[0]).unwrap();
        let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut hitboxes, level);
        let mut damage = Vec::new();
        let mut sprites = crate::sprites::TransientSprites::default();
        let mut reflected = false;
        for _ in 0..150 {
            let before = projectiles.snapshot(level).projectiles[0];
            let velocity = Vec3::from_array(before.velocity)
                - Vec3::Z * ohl_physics::MoveConfig::default().gravity * crate::TICK_SECONDS;
            let position = Vec3::from_array(before.position);
            let trace = level.collision.as_ref().unwrap().trace(
                ohl_physics::Hull::Point,
                position,
                position + velocity * crate::TICK_SECONDS,
            );
            projectiles.tick(
                level,
                &hitboxes,
                crate::TICK_SECONDS,
                &mut damage,
                &mut sprites,
            );
            let state = projectiles.snapshot(level);
            assert_eq!(
                state.projectiles.len(),
                1,
                "ceiling impact precedes live fuse expiry"
            );
            let after = state.projectiles[0];
            if velocity.z > 0.0 && after.velocity[2] < 0.0 {
                assert!(trace.fraction < 1.0 && !trace.start_solid && !trace.all_solid);
                assert_eq!(trace.plane_normal, Vec3::NEG_Z);
                assert!((trace.end_pos.z - ceiling).abs() < 0.1);
                assert!(
                    after.position[0] < -16.0,
                    "ceiling impact precedes target arrival"
                );
                assert!(after.fuse.is_some_and(|fuse| fuse > 0.0));
                reflected = true;
                break;
            }
        }
        assert!(
            reflected,
            "the forced actual grenade reflects from the low ceiling"
        );
        assert!(
            damage.is_empty(),
            "early contact is not a blast or damage claim"
        );
    }

    #[test]
    fn grenade_lob_low_ceiling_rejects_after_real_impact_prerequisites() {
        for classname in ["monster_human_grunt", "monster_human_assassin"] {
            let (mut forced, shooter, ceiling) = grenade_ceiling_fixture(classname, true);
            characterize_forced_ceiling_impact(&mut forced, shooter, ceiling);
            let (mut game, shooter, _) = grenade_ceiling_fixture(classname, true);
            seed_grenade_memory(&mut game, shooter);
            assert!(
                !grenade_ready(&mut game, shooter),
                "clear eye chord cannot admit a blocked grenade lob"
            );
            assert!(
                think_for_grenade(&mut game).is_empty(),
                "natural schedule emits no low-ceiling grenade"
            );
        }
    }

    // Exact health equality distinguishes launch from the later timed blast.
    #[test]
    #[allow(clippy::float_cmp)]
    fn grenade_lob_high_ceiling_emits_and_hurts_player() {
        for classname in ["monster_human_grunt", "monster_human_assassin"] {
            let (mut game, shooter) = super::grenade_emission_tests::safe_scene(classname);
            super::grenade_emission_tests::assert_safe_exposure(&mut game, shooter);
            seed_grenade_memory(&mut game, shooter);
            assert!(
                grenade_ready(&mut game, shooter),
                "clear lob remains available"
            );
            let requests = think_for_grenade(&mut game);
            assert_eq!(requests.len(), 1, "real schedule emits a grenade request");
            assert_eq!(requests[0].kind, ohl_combat::ProjectileKind::HandGrenade);
            // Return the actually emitted request to the normal phase-8 drain;
            // hold only its owner to exclude subsequent hitscan/AI damage.
            game.registry_mut()
                .world
                .insert_one(shooter, ScriptHold)
                .unwrap();
            game.level_and_systems_mut()
                .1
                .ai_mut()
                .projectiles
                .spawn_projectile(&requests[0]);
            let before = game.player_health();
            game.tick(crate::TICK_SECONDS, &crate::Input::default());
            assert_eq!(game.projectile_count(), 1);
            assert_eq!(
                game.player_health(),
                before,
                "launch does not apply blast damage"
            );
            for _ in 0..530 {
                game.tick(crate::TICK_SECONDS, &crate::Input::default());
            }
            assert_eq!(
                game.projectile_count(),
                0,
                "the actual timed grenade is removed"
            );
            assert!(
                game.player_health() < before,
                "clear admitted grenade still damages the player"
            );
        }
    }
}

#[cfg(test)]
#[path = "grenade_emission_tests.rs"]
mod grenade_emission_tests;

#[cfg(test)]
mod grenade_danger_tests {
    use super::*;
    use crate::save_state::{ProjectileAttackSnapshot, ProjectileEntityRef, ProjectileSnapshot};
    use crate::test_support::{AI_MAP, entity_block, plan_scripted_monster_model_bytes};
    use ohl_ai::schedule::{Brain, Task};
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};

    fn actor(game: &crate::Game, entity: Entity) -> Actor {
        *game.registry().world.get::<&Actor>(entity).unwrap()
    }

    fn projectile(game: &crate::Game, id: u32) -> Option<ProjectileSnapshot> {
        game.to_save(0)
            .projectiles
            .unwrap()
            .projectiles
            .into_iter()
            .find(|p| p.id == id)
    }

    fn step(game: &mut crate::Game) {
        game.tick(crate::TICK_SECONDS, &crate::Input::default());
    }

    // Keep the authored scene and exact full-pose prerequisites together.
    #[allow(clippy::float_cmp, clippy::too_many_lines)]
    fn scene(with_ally: bool) -> (crate::Game, Entity, Option<Entity>) {
        let mut text = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}",
            entity_block("info_player_start", [800.0, 0.0, 36.0], 180.0, &[]),
            entity_block(
                "monster_human_grunt",
                [0.0; 3],
                0.0,
                &[("targetname", "thrower")]
            ),
        );
        if with_ally {
            text.push_str(&entity_block(
                "monster_barney",
                [0.0, 128.0, 0.0],
                90.0,
                &[("targetname", "listener"), ("spawnflags", "16")],
            ));
        }
        let mut bsp = Bsp30Builder::new();
        bsp.set_entities_text(&text);
        let heads = bsp.push_collision_hulls(&[
            CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
            CollisionBrush::half_space([0.0, 0.0, -1.0], -512.0),
            CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
            CollisionBrush::half_space([-1.0, 0.0, 0.0], -960.0),
            CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
            CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
        ]);
        bsp.push_model(
            [-512.0, -512.0, 0.0],
            [960.0, 512.0, 512.0],
            [0.0; 3],
            heads,
            1,
            0,
            0,
        );
        let model_bytes = plan_scripted_monster_model_bytes();
        let mut assets = crate::MemoryAssets::new();
        for kind in [MonsterKind::HumanGrunt, MonsterKind::Barney] {
            assets.insert(kind.default_model_path().unwrap(), model_bytes.clone());
        }
        let mut game = crate::Game::from_map_bytes(&assets, AI_MAP, &bsp.build()).unwrap();
        let owner = game.registry().find("thrower")[0];
        let ally = with_ally.then(|| game.registry().find("listener")[0]);
        if let Some(ally) = ally {
            assert!(game.registry().world.get::<&Prisoner>(ally).is_ok());
            let source = actor(&game, owner).classification;
            let target = actor(&game, ally).classification;
            assert_ne!(source, target);
            let table = game
                .level_and_systems_mut()
                .1
                .ai_mut()
                .world
                .relationships_mut();
            table.set(source, target, ohl_ai::Relationship::Ally);
            assert_eq!(table.get(source, target), ohl_ai::Relationship::Ally);
        }
        let player = actor(&game, game.player_entity());
        let model =
            ohl_world::StudioModel::parse(&model_bytes, &ohl_formats::mdl10::Limits::default())
                .unwrap();
        assert_eq!(model.hitboxes.len(), 1);
        assert_eq!(model.hitboxes[0].bone, 0);
        assert_eq!(model.bones[0].parent, None);
        assert_eq!(model.bones[0].value, [0.0; 6]);
        assert_eq!(model.sequences.len(), 1);
        assert_eq!(model.sequences[0].frame_count, 2);
        assert_eq!(model.sequences[0].fps, 10.0);
        let player_min = player.body_frame.world_bounds(player.hull, player.origin).0;
        for entity in [Some(owner), ally].into_iter().flatten() {
            let a = actor(&game, entity);
            let transform = *game.registry().world.get::<&Transform>(entity).unwrap();
            assert_eq!(a.origin, transform.origin);
            assert!(game.registry().world.get::<&ScriptHold>(entity).is_err());
            let rotation = glam::Quat::from_rotation_z(transform.angles.y.to_radians());
            for (time, x) in [(0.0, 10.0), (0.1, 20.0)] {
                let pose = ohl_world::StudioPose::sample(&model, 0, time).unwrap();
                let (min, max) = pose.hitbox_bounds(&model.hitboxes[0]).unwrap();
                assert_eq!(min, [x - 24.0, -24.0, 0.0]);
                assert_eq!(max, [x + 24.0, 24.0, 72.0]);
                for x in [min[0], max[0]] {
                    for y in [min[1], max[1]] {
                        for z in [min[2], max[2]] {
                            let world = a.origin + rotation * Vec3::new(x, y, z);
                            assert!(
                                player_min.x - world.x > 400.0,
                                "full-pose blast regions are disjoint"
                            );
                        }
                    }
                }
            }
        }
        for entity in [Some(owner), ally, Some(game.player_entity())]
            .into_iter()
            .flatten()
        {
            clear_body(&game, entity);
        }
        let source = actor(&game, owner);
        assert!(secondary_target_clear(
            &MonsterKind::HumanGrunt,
            AiDifficulty::Medium,
            game.collision(),
            source.eye(),
            &player
        ));
        assert!(game.systems_mut().ai().world.sounds().is_empty());
        (game, owner, ally)
    }

    fn clear_body(game: &crate::Game, entity: Entity) {
        let a = actor(game, entity);
        assert!(a.alive && a.health > 0.0);
        let world = game.collision().unwrap();
        let body = world.trace(a.hull, a.query_origin(), a.query_origin());
        assert!(!body.start_solid && !body.all_solid);
        let floor = world.trace(a.hull, a.query_origin(), a.query_origin() - Vec3::Z);
        assert!(!floor.start_solid && floor.fraction < 1.0, "floor support");
    }

    // Exact spawn/profile/task/cooldown assertions deliberately preserve the natural launch.
    #[allow(clippy::float_cmp)]
    fn natural_launch(
        game: &mut crate::Game,
        owner: Entity,
        ally: Option<Entity>,
    ) -> (u32, ProjectileAttackSnapshot) {
        for _ in 0..600 {
            let before = game.player_health();
            step(game);
            let save = game.to_save(0);
            if let Some(p) = save
                .projectiles
                .as_ref()
                .unwrap()
                .projectiles
                .iter()
                .find(|p| p.kind_tag == 3)
            {
                assert_eq!(
                    game.player_health(),
                    before,
                    "throw causes no immediate grenade damage"
                );
                assert_eq!(p.age, 0.0, "phase8 launch waits for phase7");
                assert_eq!(p.fuse, Some(5.0));
                let profile = *save
                    .projectile_runtime
                    .as_ref()
                    .unwrap()
                    .attacks
                    .iter()
                    .find(|a| a.id == p.id)
                    .unwrap();
                let (level, systems) = game.level_and_systems_mut();
                let owner_ref = crate::save_state::spawn_index_of(level, owner).unwrap();
                assert_eq!(p.owner, Some(owner_ref));
                assert_eq!(
                    profile.owner,
                    Some(ProjectileEntityRef::Registry(owner_ref))
                );
                assert_eq!(profile.target, Some(ProjectileEntityRef::Player));
                assert_eq!(profile.damage, 100.0);
                assert_eq!(profile.damage_bits, DamageType::BLAST.bits());
                assert_eq!(profile.blast_radius, Some(200.0));
                assert_eq!(systems.ai().secondary_cooldowns.get(&owner), Some(&6.0));
                {
                    let ai = level.registry.world.get::<&MonsterAi>(owner).unwrap();
                    assert_eq!(ai.memory.unwrap().entity, level.player);
                    assert_eq!(
                        ai.runner.schedule_name(),
                        ohl_ai::monsters::brains::GRUNT_GRENADE.name
                    );
                    assert_eq!(ai.runner.task(), Some(Task::Wait(1.0)));
                    assert!(!ai.runner.started());
                    assert!(!ai.conditions.contains(Conditions::HEAR_DANGER));
                }
                if let Some(ally) = ally {
                    let ai = level.registry.world.get::<&MonsterAi>(ally).unwrap();
                    assert!(ai.memory.is_none());
                    assert_eq!(ai.runner.schedule_name(), ohl_ai::brain::IDLE_STAND.name);
                    assert!(!ai.conditions.contains(Conditions::HEAR_DANGER));
                    assert!(ai.route.is_finished());
                }
                return (p.id, profile);
            }
        }
        panic!("ordinary senses and schedules must emit a safe grenade");
    }

    fn audible(game: &crate::Game, listener: Entity, kind: MonsterKind, id: u32) -> Vec3 {
        let p = projectile(game, id).expect("same live grenade");
        assert!(p.age > 0.0 && p.fuse.is_some_and(|left| left.is_finite() && left > 0.0));
        let position = Vec3::from_array(p.position);
        let sensitivity = MonsterBrain::for_kind(kind)
            .unwrap()
            .senses()
            .hearing_sensitivity;
        let a = actor(game, listener);
        assert!(
            position.is_finite() && a.eye().distance(position) < 200.0 * sensitivity,
            "actual phase7 position is audible before the response oracle"
        );
        clear_body(game, listener);
        let away =
            Vec3::new(a.origin.x - position.x, a.origin.y - position.y, 0.0).normalize_or_zero();
        assert!(away.length_squared() > 0.0);
        let end = a.query_origin() + away * ohl_ai::world::COVER_DISTANCE;
        let collision = game.collision().unwrap();
        let path = collision.trace(a.hull, a.query_origin(), end);
        assert!(!path.start_solid && !path.all_solid && path.fraction >= 1.0);
        let floor = collision.trace(a.hull, end, end - Vec3::Z);
        assert!(
            !floor.start_solid && floor.fraction < 1.0,
            "clear supported escape leg"
        );
        position
    }

    // The unblocked hull fraction is an exact collision prerequisite.
    #[allow(clippy::float_cmp)]
    fn cover_and_move(game: &mut crate::Game, listener: Entity, kind: &MonsterKind, id: u32) {
        let start = actor(game, listener).origin;
        let mut direction = None;
        for _ in 0..6 {
            step(game);
            let danger = audible(game, listener, kind.clone(), id);
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert!(ai.conditions.contains(Conditions::HEAR_DANGER));
            assert_eq!(
                ai.runner.schedule_name(),
                ohl_ai::brain::TAKE_COVER_FROM_DANGER.name
            );
            if let Some(cover) = ai.cover {
                let a = actor(game, listener);
                let away = Vec3::new(a.origin.x - danger.x, a.origin.y - danger.y, 0.0).normalize();
                assert!(
                    (cover - a.query_origin()).dot(away) > 300.0,
                    "current danger selects the clear cover leg"
                );
                let trace = game
                    .collision()
                    .unwrap()
                    .trace(a.hull, a.query_origin(), cover);
                assert!(!trace.start_solid && !trace.all_solid && trace.fraction == 1.0);
                assert_eq!(ai.runner.task(), Some(Task::TakeCover));
                direction = Some(away);
                break;
            }
        }
        let direction =
            direction.expect("ordinary FindCover runs while the warning remains audible");
        for _ in 0..20 {
            step(game);
        }
        clear_body(game, listener);
        assert!(
            (actor(game, listener).origin - start).dot(direction) > 1.0,
            "real AI route movement escapes the heard point"
        );
    }

    #[test]
    fn timed_grenade_danger_ally_hears_and_moves_without_a_new_mask() {
        let (mut game, owner, ally) = scene(true);
        let ally = ally.unwrap();
        let (id, profile) = natural_launch(&mut game, owner, Some(ally));
        step(&mut game);
        audible(&game, ally, MonsterKind::Barney, id);
        {
            let ai = game.registry().world.get::<&MonsterAi>(ally).unwrap();
            assert!(
                ai.conditions.contains(Conditions::HEAR_DANGER),
                "live grenade reaches ordinary ally hearing"
            );
            assert_eq!(
                ai.runner.schedule_name(),
                ohl_ai::brain::TAKE_COVER_FROM_DANGER.name
            );
        }
        cover_and_move(&mut game, ally, &MonsterKind::Barney, id);
        let mut removed = false;
        for _ in 0..510 {
            let Some(p) = projectile(&game, id) else {
                removed = true;
                break;
            };
            let saved = game.to_save(0).projectile_runtime.unwrap();
            assert_eq!(*saved.attacks.iter().find(|a| a.id == id).unwrap(), profile);
            step(&mut game);
            if projectile(&game, id).is_none() {
                assert!(
                    p.fuse.unwrap() <= crate::TICK_SECONDS,
                    "ordinary fuse, not contact, removes the grenade"
                );
                removed = true;
                break;
            }
        }
        assert!(removed, "ordinary finite fuse completes");
        assert_eq!(
            game.projectile_count(),
            0,
            "six-second cooldown prevents a replacement before this fuse"
        );
        assert!(
            !game
                .registry()
                .world
                .get::<&MonsterAi>(ally)
                .unwrap()
                .conditions
                .contains(Conditions::HEAR_DANGER)
        );
        assert!(
            game.systems_mut().ai().world.sounds().is_empty(),
            "no stale danger after removal"
        );
    }

    #[test]
    fn timed_grenade_danger_owner_interrupts_only_the_natural_grenade_wait() {
        let (mut game, owner, _) = scene(false);
        let (id, _) = natural_launch(&mut game, owner, None);
        step(&mut game);
        audible(&game, owner, MonsterKind::HumanGrunt, id);
        {
            let ai = game.registry().world.get::<&MonsterAi>(owner).unwrap();
            assert!(
                ai.conditions.contains(Conditions::HEAR_DANGER),
                "bridge remains active before the mask oracle"
            );
            assert_eq!(
                ai.runner.schedule_name(),
                ohl_ai::brain::TAKE_COVER_FROM_DANGER.name,
                "heard danger interrupts the natural grunt grenade wait"
            );
        }
        cover_and_move(&mut game, owner, &MonsterKind::HumanGrunt, id);
        assert!(actor(&game, owner).origin.x < 0.0);
        let remaining = game.systems_mut().ai().secondary_cooldowns[&owner];
        assert!(
            remaining > 5.0 && remaining < 6.0,
            "escape does not reset or shorten the emission cooldown"
        );
    }

    #[derive(Clone, Copy)]
    enum LocalCoverScene {
        Clear,
        PrimaryBlocked,
        Enclosed,
    }

    // Keep generated geometry and exact profile prerequisites together; AI uses ordinary ticks.
    #[allow(clippy::too_many_lines, clippy::float_cmp)]
    fn local_cover_scene(kind: LocalCoverScene) -> (crate::Game, Entity, u32) {
        let text = format!(
            "{{\"classname\" \"worldspawn\"}}{}{}",
            entity_block("info_player_start", [-384.0, -384.0, 36.0], 0.0, &[]),
            entity_block(
                "monster_barney",
                [0.0; 3],
                0.0,
                &[("targetname", "listener"), ("spawnflags", "16")],
            ),
        );
        let mut brushes = vec![
            CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
            CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
            CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
            CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
            CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
            CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
        ];
        match kind {
            LocalCoverScene::Clear => {}
            LocalCoverScene::PrimaryBlocked => brushes.push(CollisionBrush::box_brush(
                [32.0, -512.0, 0.0],
                [64.0, 512.0, 256.0],
            )),
            LocalCoverScene::Enclosed => brushes.extend([
                CollisionBrush::box_brush([21.0, -64.0, 0.0], [64.0, 64.0, 256.0]),
                CollisionBrush::box_brush([-64.0, -64.0, 0.0], [-21.0, 64.0, 256.0]),
                CollisionBrush::box_brush([-64.0, 21.0, 0.0], [64.0, 64.0, 256.0]),
                CollisionBrush::box_brush([-64.0, -64.0, 0.0], [64.0, -21.0, 256.0]),
            ]),
        }
        let mut bsp = Bsp30Builder::new();
        bsp.set_entities_text(&text);
        let heads = bsp.push_collision_hulls(&brushes);
        bsp.push_model(
            [-512.0, -512.0, 0.0],
            [512.0, 512.0, 256.0],
            [0.0; 3],
            heads,
            1,
            0,
            0,
        );
        let mut game =
            crate::Game::from_map_bytes(&crate::MemoryAssets::new(), AI_MAP, &bsp.build()).unwrap();
        let listener = game.registry().find("listener")[0];
        let a = actor(&game, listener);
        assert_eq!(a.hull, ohl_physics::Hull::Standing);
        assert_eq!(
            a.origin,
            game.registry()
                .world
                .get::<&Transform>(listener)
                .unwrap()
                .origin
        );
        assert!(game.registry().world.get::<&Prisoner>(listener).is_ok());
        assert!(game.registry().world.get::<&ScriptHold>(listener).is_err());
        assert!(
            game.level_and_systems_mut()
                .1
                .ai()
                .world
                .navigator()
                .is_none()
        );
        clear_body(&game, listener);
        {
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert!(ai.memory.is_none() && ai.cover.is_none() && ai.route.is_finished());
        }
        let x = if matches!(kind, LocalCoverScene::Enclosed) {
            -18.0
        } else {
            -96.0
        };
        let at = Vec3::new(x, 0.0, 128.0);
        let occupancy = game
            .collision()
            .unwrap()
            .trace(ohl_physics::Hull::Point, at, at);
        assert!(!occupancy.start_solid && !occupancy.all_solid);
        let id = game
            .debug_spawn_projectile(
                ohl_combat::ProjectileKind::HandGrenade,
                at.to_array(),
                [0.0; 3],
            )
            .unwrap();
        let saved = game.to_save(0);
        let profile = saved
            .projectile_runtime
            .unwrap()
            .attacks
            .into_iter()
            .find(|p| p.id == id.0)
            .unwrap();
        assert_eq!(profile.damage_bits, DamageType::BLAST.bits());
        assert!(profile.damage > 0.0 && profile.blast_radius == Some(200.0));
        assert!(profile.owner.is_none());
        (game, listener, id.0)
    }

    fn local_cover_warning(game: &crate::Game, listener: Entity, id: u32) -> Vec3 {
        let p = projectile(game, id).expect("same real timed grenade remains live");
        assert_eq!(p.kind_tag, 3);
        assert!(p.age > 0.0 && p.fuse.is_some_and(|left| left.is_finite() && left > 0.0));
        let at = Vec3::from_array(p.position);
        let a = actor(game, listener);
        let sensitivity = MonsterBrain::for_kind(MonsterKind::Barney)
            .unwrap()
            .senses()
            .hearing_sensitivity;
        assert!(at.is_finite() && a.eye().distance(at) < 200.0 * sensitivity);
        clear_body(game, listener);
        let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
        assert!(ai.memory.is_none());
        assert!(ai.conditions.contains(Conditions::HEAR_DANGER));
        assert_eq!(
            ai.runner.schedule_name(),
            ohl_ai::brain::TAKE_COVER_FROM_DANGER.name
        );
        at
    }

    fn local_cover_before_find(game: &mut crate::Game, listener: Entity, id: u32) -> Vec3 {
        for _ in 0..8 {
            step(game);
            let at = local_cover_warning(game, listener, id);
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert!(ai.cover.is_none() && ai.route.is_finished());
            if ai.runner.task() == Some(Task::FindCover) {
                assert!(!ai.runner.started());
                return at;
            }
        }
        panic!("ordinary danger schedule must reach unstarted FindCover");
    }

    fn assert_local_leg(game: &crate::Game, listener: Entity, direction: Vec3) {
        let a = actor(game, listener);
        let from = a.query_origin();
        let to = from + direction * ohl_ai::world::COVER_DISTANCE;
        let collision = game.collision().unwrap();
        let chord = collision.trace(a.hull, from, to);
        assert!(!chord.start_solid && !chord.all_solid && chord.fraction >= 1.0);
        for index in 0_u8..=20 {
            let mut at = from.lerp(to, f32::from(index) / 20.0);
            at.z = from.z;
            let support = collision.trace(a.hull, at, at - Vec3::Z);
            assert!(
                !support.start_solid && support.fraction < 1.0,
                "independently supported leg"
            );
        }
    }

    fn local_cover_move(game: &mut crate::Game, listener: Entity, expected: Vec3) {
        let start = actor(game, listener);
        {
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert_eq!(ai.runner.task(), Some(Task::TakeCover));
            assert!(!ai.runner.started());
        }
        step(game);
        {
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert_eq!(ai.runner.task(), Some(Task::RunPath));
            assert!(
                ai.route
                    .waypoint()
                    .unwrap()
                    .abs_diff_eq(expected, ohl_physics::DIST_EPSILON)
            );
        }
        for _ in 0..20 {
            step(game);
        }
        let now = actor(game, listener);
        let direction = (expected - start.query_origin()).normalize();
        assert!(
            (now.origin - start.origin).dot(direction) > 1.0,
            "ordinary route moves along the chosen full leg"
        );
        assert!((now.origin - start.origin).cross(direction).length() < ohl_physics::DIST_EPSILON);
        clear_body(game, listener);
    }

    #[test]
    fn danger_local_cover_uses_lateral_when_primary_is_blocked() {
        let (mut game, listener, id) = local_cover_scene(LocalCoverScene::PrimaryBlocked);
        let at = local_cover_before_find(&mut game, listener, id);
        let a = actor(&game, listener);
        assert!(at.x < a.origin.x && at.y.abs() <= ohl_physics::DIST_EPSILON);
        let primary = a.query_origin() + Vec3::X * ohl_ai::world::COVER_DISTANCE;
        let blocked = game
            .collision()
            .unwrap()
            .trace(a.hull, a.query_origin(), primary);
        assert!(!blocked.start_solid && blocked.fraction < 1.0);
        let moved = ohl_ai::movement::move_toward(
            game.collision().unwrap(),
            a.hull,
            a.query_origin(),
            primary,
            ohl_ai::world::COVER_DISTANCE,
            1.0,
        );
        assert!(moved.blocked && moved.distance < ohl_ai::world::COVER_DISTANCE);
        assert_local_leg(&game, listener, Vec3::Y);
        assert_local_leg(&game, listener, -Vec3::Y);
        step(&mut game);
        local_cover_warning(&game, listener, id);
        let expected = a.query_origin() + Vec3::Y * ohl_ai::world::COVER_DISTANCE;
        let cover = game
            .registry()
            .world
            .get::<&MonsterAi>(listener)
            .unwrap()
            .cover;
        assert!(
            cover.is_some_and(|point| point.abs_diff_eq(expected, ohl_physics::DIST_EPSILON)),
            "blocked primary chooses the first clear supported lateral cover"
        );
        local_cover_move(&mut game, listener, expected);
    }

    #[test]
    fn danger_local_cover_keeps_the_clear_primary() {
        let (mut game, listener, id) = local_cover_scene(LocalCoverScene::Clear);
        local_cover_before_find(&mut game, listener, id);
        let a = actor(&game, listener);
        assert_local_leg(&game, listener, Vec3::X);
        assert_local_leg(&game, listener, Vec3::Y);
        assert_local_leg(&game, listener, -Vec3::Y);
        step(&mut game);
        local_cover_warning(&game, listener, id);
        let expected = a.query_origin() + Vec3::X * ohl_ai::world::COVER_DISTANCE;
        assert!(
            game.registry()
                .world
                .get::<&MonsterAi>(listener)
                .unwrap()
                .cover
                .unwrap()
                .abs_diff_eq(expected, ohl_physics::DIST_EPSILON)
        );
        local_cover_move(&mut game, listener, expected);
    }

    #[test]
    fn danger_local_cover_enclosed_fails_without_a_false_goal() {
        let (mut game, listener, id) = local_cover_scene(LocalCoverScene::Enclosed);
        local_cover_before_find(&mut game, listener, id);
        let a = actor(&game, listener);
        for direction in [Vec3::X, Vec3::Y, -Vec3::Y] {
            let end = a.query_origin() + direction * ohl_ai::world::COVER_DISTANCE;
            let blocked = game
                .collision()
                .unwrap()
                .trace(a.hull, a.query_origin(), end);
            assert!(!blocked.start_solid && blocked.fraction < 1.0);
            let moved = ohl_ai::movement::move_toward(
                game.collision().unwrap(),
                a.hull,
                a.query_origin(),
                end,
                ohl_ai::world::COVER_DISTANCE,
                1.0,
            );
            assert!(moved.blocked && moved.distance < ohl_ai::movement::WAYPOINT_TOLERANCE);
        }
        step(&mut game);
        local_cover_warning(&game, listener, id);
        {
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert!(
                ai.cover.is_none(),
                "enclosed danger must not manufacture completed cover"
            );
            assert!(ai.route.is_finished());
            assert_eq!(
                ai.runner.task(),
                Some(Task::SetActivity(ohl_ai::Activity::Cover))
            );
            assert!(
                !ai.runner.started(),
                "failed FindCover reselects before executing the replacement"
            );
        }
        for _ in 0..8 {
            step(&mut game);
            local_cover_warning(&game, listener, id);
            let ai = game.registry().world.get::<&MonsterAi>(listener).unwrap();
            assert!(ai.cover.is_none() && ai.route.is_finished());
            assert!(matches!(
                ai.runner.task(),
                Some(Task::FindCover | Task::SetActivity(ohl_ai::Activity::Cover))
            ));
        }
        assert_eq!(actor(&game, listener).origin, a.origin);
    }
}
