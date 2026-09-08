//! Save-file snapshot structs for M7.9 P4b: `SECTION_INVENTORY` (23),
//! `SECTION_ENTITY_COMBAT` (24), `SECTION_AI` (25), `SECTION_PROJECTILES`
//! (26) and `SECTION_RNG` (27); plus M9.5's `SECTION_MAKER_CHILDREN` (29).
//! See `crate::save`'s module doc for the full tag map and
//! `.plan/m79-design.md` §6 for the rules every section here follows:
//! additive only, entities referenced by spawn index (never a raw
//! `hecs::Entity`), a missing section loads as a default so a save written
//! before this package still opens, and every length is bounded.
//!
//! # Entity references
//!
//! [`spawn_index_of`]/[`entity_at_spawn_index`] are the one place this
//! module (and its callers in `crate::combat`, `crate::ai`,
//! `crate::projectiles`) turns a live `hecs::Entity` into a save-stable
//! number and back, by its position in `Registry::entities` — the same
//! spawn order every other save section already keys by.
//!
//! # Monstermaker children are now saved (`SECTION_MAKER_CHILDREN`, 29)
//!
//! The gap this heading used to describe — a `monstermaker`'s children were
//! spawned directly through `registry.world.spawn`
//! (`crate::ai::AiState::spawn_child`), bypassing `Registry::entities`, so
//! no spawn index existed to key them by — is closed: `crate::ai::AiState::
//! spawn_child` now also pushes the new entity onto `Registry::entities`
//! (see that method's own doc comment), so a maker child gets a spawn index
//! exactly like any map-declared monster the instant it exists, and every
//! other index-keyed section (`SECTION_ENTITY_REGISTRY` 18 for its
//! transform, `SECTION_ENTITY_COMBAT` 24 for its health,
//! `SECTION_AI` 25 for its AI state) already covers it for free.
//!
//! What is left is recreating the child *entity itself* on load, before
//! those index-keyed sections are zipped against `Registry::entities` — a
//! fresh `attach_level` only ever spawns the map's own declared entities,
//! never a maker's dynamically-created children, so `Registry::entities`
//! is shorter than the save recorded until [`snapshot_maker_children`]'s
//! own section fills the gap back in. [`MonsterMakerChildSnapshot`]
//! records just enough per child (which maker made it, and its spawn
//! classname) for `crate::ai::AiState::restore_maker_children` to recreate
//! a placeholder monster of the right kind at the right spawn index, ahead
//! of every other tag's own restore; `crate::ai::AiState::
//! finalize_maker_children` links the survivors back onto their maker's
//! own live-child list once health/AI state has actually landed on them,
//! so [`ohl_ai::Spawner::live_children`]/`has_room` keep counting correctly
//! after a load exactly as they do mid-session.
//!
//! A child already gone by save time — gibbed outright, or a faded corpse
//! `crate::ai::AiState::age_corpses` already despawned — has no classname
//! left to record (its components are gone with it), so its slot in
//! [`snapshot_maker_children`]'s section is `None`; restoring a `None` tail
//! slot recreates an inert, component-less placeholder purely to keep
//! every *later* child's index aligned, and nothing ever queries it again.
//! This mirrors the pre-existing rule `SECTION_ENTITY_COMBAT` already
//! applies to a gibbed map-declared monster: the record is gone, so the
//! entity simply stays gone.
//!
//! # A restored projectile draws as nothing
//!
//! `TODO(P4b-followup)`: a model-backed projectile (a flying rocket, a
//! placed tripmine) is drawn through a real `hecs` entity carrying
//! [`crate::components::StudioAnim`], owned by
//! `crate::projectiles::ProjectileSystem`'s own `models` map
//! (`ProjectileId` -> `Entity`). This package restores the physical
//! projectile/deployable state exactly, but does not re-create that
//! drawing entity, so a projectile in flight when a save is loaded
//! continues to move, damage and detonate identically, but renders
//! invisibly until it resolves. Fixing this needs `ProjectileSystem` to
//! remember which model slot each kind used, which is `crate::projectiles`'
//! own runtime configuration (`set_model_for`), not save data.

use glam::Vec3;
use ohl_combat::{EntityId as CombatEntityId, ProjectileKind};
use ohl_game::hecs::Entity;
use ohl_game::registry::{
    AutoTrigger, Breakable, ClassName, MakerActivation, MomentaryDoor, MomentaryRotButton,
    MoverState, Pendulum, Pushable, RotButton, Rotator,
};
use ohl_game::{TrackTrainState, TriggerCameraState};
use serde::{Deserialize, Serialize};

use crate::components::{MonsterMaker, Owner};
use crate::ids::{entity_id, entity_of};
use crate::level::Level;

/// The most entities one `SECTION_ENTITY_COMBAT`/`SECTION_AI` section
/// records. Both sections hold exactly one entry per
/// `Registry::entities` slot (`None` for an entity with nothing to say), so
/// this is also the largest registry a level may have for those sections to
/// stay representable; a larger registry truncates the tail, matching the
/// rest of this crate's "bound every list" policy.
pub const MAX_SNAPSHOT_ENTITIES: usize = 65_536;

/// The most live projectiles one `SECTION_PROJECTILES` records, matching
/// `ohl_combat::ProjectileLimits::default`'s own cap.
pub const MAX_SNAPSHOT_PROJECTILES: usize = 128;

/// The most placed satchels/tripmines one `SECTION_PROJECTILES` records,
/// matching `ohl_combat::deployables::{MAX_SATCHELS, MAX_TRIPMINES}`.
pub const MAX_SNAPSHOT_DEPLOYABLES: usize = 5;

/// `entity`'s position in `level.registry.entities`, the save-stable
/// reference every section in this module uses instead of a raw
/// `hecs::Entity`. `None` for an entity the registry never indexed (the
/// player, or a `monstermaker` child — see the module doc).
#[must_use]
pub(crate) fn spawn_index_of(level: &Level, entity: Entity) -> Option<u32> {
    level
        .registry
        .entities
        .iter()
        .position(|candidate| *candidate == entity)
        .and_then(|index| u32::try_from(index).ok())
}

/// The entity a previous [`spawn_index_of`] named, or `None` when the index
/// is out of range (a corrupt or stale save, or an entity that no longer
/// exists in the reloaded level).
#[must_use]
pub(crate) fn entity_at_spawn_index(level: &Level, index: u32) -> Option<Entity> {
    usize::try_from(index)
        .ok()
        .and_then(|index| level.registry.entities.get(index))
        .copied()
}

/// A combat entity id, as a save-stable spawn index.
#[must_use]
pub(crate) fn spawn_index_of_combat_id(level: &Level, id: CombatEntityId) -> Option<u32> {
    entity_of(id).and_then(|entity| spawn_index_of(level, entity))
}

/// A save-stable spawn index, as a combat entity id.
#[must_use]
pub(crate) fn combat_id_at_spawn_index(level: &Level, index: u32) -> Option<CombatEntityId> {
    entity_at_spawn_index(level, index).map(entity_id)
}

/// One owned weapon's persisted state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WeaponSnapshot {
    /// Whether the weapon is owned at all.
    pub owned: bool,
    /// Rounds currently loaded (meaningless when `owned` is `false`).
    pub clip: u32,
}

/// The currently-drawn weapon's firing state-machine summary: see
/// `ohl_combat::FiringState::state_tag_and_timer`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FiringSnapshot {
    /// The weapon this firing state belongs to, as a position in
    /// `ohl_combat::WeaponId::ALL`.
    pub weapon: u8,
    /// `ohl_combat::FiringState::state_tag_and_timer`'s discriminant.
    pub state_tag: u8,
    /// That state's own remaining timer.
    pub timer: f32,
}

/// `SECTION_INVENTORY` (23): owned weapons, per-weapon clips, ammo
/// reserves, selection and the drawn weapon's firing summary.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InventorySnapshot {
    /// One entry per `ohl_combat::WeaponId::ALL`, in that order.
    pub weapons: Vec<WeaponSnapshot>,
    /// One entry per `ohl_combat::AmmoType::ALL`, in that order.
    pub ammo: Vec<u32>,
    /// The selected weapon, as a position in `WeaponId::ALL`.
    pub selected: Option<u8>,
    /// Whether the HEV suit is owned.
    pub has_suit: bool,
    /// Whether the long-jump module is owned.
    pub has_long_jump: bool,
    /// The drawn weapon's firing state, when one is selected.
    pub firing: Option<FiringSnapshot>,
    /// `crate::combat::CombatState::capture_carry`'s opaque P1 blob, kept
    /// only so an old save (written before this typed section existed, and
    /// so missing tag 23 entirely) still restores through
    /// `crate::combat::CombatState::restore_carry` exactly as it did
    /// before this package landed. A save written by this package populates
    /// it too (it costs nothing and keeps `SECTION_PLAYER_CARRY`, tag 17,
    /// meaningful on its own), but restore prefers the typed fields above
    /// whenever tag 23 itself is present.
    pub legacy_carry: Vec<u8>,
}

/// One registry entity's persisted health/armor, in `SECTION_ENTITY_COMBAT`
/// (24). `None` for an entity with neither.
///
/// Health is read from `ohl_ai::Actor::health` when the entity carries one
/// (the authority `ohl_ai::apply_monster_damage` moves), falling back to
/// the bare `ohl_combat::Health` component otherwise; restoring writes both
/// when both are present, so a monster's think phase sees the same health
/// next tick that the AI simulation left it with before the save.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct EntityCombatSnapshot {
    /// `(current, max)`.
    pub health: Option<(f32, f32)>,
    /// `(current, max)`.
    pub armor: Option<(f32, f32)>,
}

/// One acquired enemy's remembered state, part of [`AiSnapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EnemyMemorySnapshot {
    /// The remembered enemy, as a spawn index.
    pub enemy: u32,
    /// Where it was last actually seen.
    pub last_known_position: [f32; 3],
    /// Seconds since it was last seen.
    pub time_since_seen: f32,
    /// Whether line of sight is currently blocked.
    pub occluded: bool,
    /// The distance at the last update.
    pub last_known_distance: f32,
}

/// One monster's persisted AI state, part of `SECTION_AI` (25).
///
/// Schedule identity is the running schedule's stable name (`""` for none),
/// resolved back to the schedule table by
/// `ohl_ai::ScheduleRunner::restore` — never an index, so adding a schedule
/// can never invalidate a save (`.plan/m79-design.md` §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiSnapshot {
    /// `ohl_ai::MonsterState::tag`.
    pub state_tag: u8,
    /// `ohl_ai::Conditions::bits`.
    pub conditions: u32,
    /// `ohl_ai::MonsterAi::pending_conditions`, as raw bits.
    pub pending_conditions: u32,
    /// The running schedule's stable name, or `""`.
    pub schedule_name: String,
    /// `ohl_ai::ScheduleRunner::task_index`.
    pub task_index: u32,
    /// `ohl_ai::ScheduleRunner::started`.
    pub schedule_started: bool,
    /// `ohl_ai::ScheduleRunner::timer`.
    pub schedule_timer: f32,
    /// The acquired enemy, as a spawn index.
    pub enemy: Option<EnemyMemorySnapshot>,
    /// The route's waypoints.
    pub route_waypoints: Vec<[f32; 3]>,
    /// The route's current waypoint index.
    pub route_current: u32,
    /// The route's own goal (`ohl_ai::Route::goal`).
    pub route_goal: [f32; 3],
    /// `ohl_ai::MonsterAi::move_target`.
    pub move_target: Option<[f32; 3]>,
    /// `ohl_ai::MonsterAi::cover`.
    pub cover: Option<[f32; 3]>,
    /// `ohl_ai::Activity::tag`.
    pub activity_tag: u8,
    /// `ohl_ai::MonsterAi::move_speed`.
    pub move_speed: f32,
    /// `ohl_ai::MonsterAi::ideal_yaw`.
    pub ideal_yaw: f32,
    /// `ohl_ai::StuckDetector`'s own tick counter.
    pub stuck_ticks: u32,
    /// The `ohl_ai::SquadTag` this entity carries, when it has one.
    pub squad: Option<SquadSnapshot>,
}

/// An entity's squad membership, part of [`AiSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SquadSnapshot {
    /// The squad's `netname`.
    pub name: String,
    /// Whether this entity is the squad leader.
    pub leader: bool,
}

/// One live projectile, part of `SECTION_PROJECTILES` (26).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectileSnapshot {
    /// The projectile's own handle, so ids are stable across a save/load.
    pub id: u32,
    /// The kind, as a fixed tag (see [`projectile_kind_tag`]).
    pub kind_tag: u8,
    /// Who fired it, as a spawn index.
    pub owner: Option<u32>,
    /// World-space position.
    pub position: [f32; 3],
    /// World-space velocity.
    pub velocity: [f32; 3],
    /// Seconds since spawned.
    pub age: f32,
    /// Seconds left on the fuse, when the kind has one.
    pub fuse: Option<f32>,
    /// A guided rocket's steering point.
    pub guide_point: Option<[f32; 3]>,
    /// A homing hornet's or hopping snark's target, as a spawn index.
    pub target: Option<u32>,
    /// Seconds until a snark may bite again.
    pub attack_cooldown: f32,
    /// Seconds until a snark hops again.
    pub hop_cooldown: f32,
    /// Whether the projectile has settled on a surface.
    pub resting: bool,
}

/// One placed satchel charge, part of `SECTION_PROJECTILES` (26).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SatchelSnapshot {
    /// The charge's own handle.
    pub id: u32,
    /// Who placed it, as a spawn index.
    pub owner: Option<u32>,
    /// Where it sits.
    pub position: [f32; 3],
    /// Seconds since placed.
    pub age: f32,
}

/// One placed tripmine, part of `SECTION_PROJECTILES` (26).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TripmineSnapshot {
    /// The mine's own handle.
    pub id: u32,
    /// Who placed it, as a spawn index.
    pub owner: Option<u32>,
    /// Where it is stuck.
    pub position: [f32; 3],
    /// The surface normal its beam runs along.
    pub normal: [f32; 3],
    /// Seconds since placed.
    pub age: f32,
    /// Whether the arming delay has elapsed.
    pub armed: bool,
}

/// `SECTION_PROJECTILES` (26): live projectiles and placed deployables,
/// plus each set's own id/rng restart bookkeeping so ids and (for
/// projectiles) the snark-hop random stream continue exactly where the
/// save left them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ProjectilesSnapshot {
    /// Live projectiles, in spawn order.
    pub projectiles: Vec<ProjectileSnapshot>,
    /// `ohl_combat::ProjectileSet::next_id_and_rng_state`'s first element.
    pub projectile_next_id: u32,
    /// `ohl_combat::ProjectileSet::next_id_and_rng_state`'s second element.
    pub projectile_rng_state: u64,
    /// Placed satchels, in placement order.
    pub satchels: Vec<SatchelSnapshot>,
    /// Placed tripmines, in placement order.
    pub tripmines: Vec<TripmineSnapshot>,
    /// `ohl_combat::DeployableSet::next_id`.
    pub deployable_next_id: u32,
}

/// `SECTION_RNG` (27): `Systems::rng`'s own PCG state and the substep
/// counter, so a fixed-seed scripted run continued after a load produces
/// the same `ai_state_hash` as the uninterrupted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RngSnapshot {
    /// `ohl_ai::Pcg32::snapshot`'s first element.
    pub state: u64,
    /// `ohl_ai::Pcg32::snapshot`'s second element.
    pub increment: u64,
    /// How many fixed steps have run since the level was attached.
    pub substep_counter: u64,
    /// `ohl_ai::AiWorld::rng_snapshot`'s first element. The AI world's own
    /// random stream is a distinct generator seeded from a draw off
    /// [`Self::state`]/[`Self::increment`] at construction time
    /// (`crate::systems::Systems::new`), so it needs its own save entry —
    /// without it, a save/load continued by more ticks would not
    /// reproduce the same `ai_state_hash` an uninterrupted run does
    /// whenever anything in `ohl-ai` draws randomness (a snark's hop, a
    /// schedule's `WaitRandom`).
    pub ai_state: u64,
    /// `ohl_ai::AiWorld::rng_snapshot`'s second element.
    pub ai_increment: u64,
    /// `ohl_ai::AiWorld::tick_count`, mixed into `state_hash` itself, so a
    /// save omitting it would desync the very digest a determinism check
    /// compares.
    pub ai_tick_count: u64,
}

/// The fixed tag [`ProjectileKind`] is stored as. Values never change once
/// assigned, matching every other save-file tag in this crate.
#[must_use]
pub(crate) const fn projectile_kind_tag(kind: ProjectileKind) -> u8 {
    match kind {
        ProjectileKind::CrossbowBolt => 0,
        ProjectileKind::Rocket => 1,
        ProjectileKind::Mp5Grenade => 2,
        ProjectileKind::HandGrenade => 3,
        ProjectileKind::Hornet => 4,
        ProjectileKind::Snark => 5,
    }
}

/// The [`ProjectileKind`] a previous [`projectile_kind_tag`] named, or
/// `None` for a tag this build does not recognise.
#[must_use]
pub(crate) const fn projectile_kind_from_tag(tag: u8) -> Option<ProjectileKind> {
    Some(match tag {
        0 => ProjectileKind::CrossbowBolt,
        1 => ProjectileKind::Rocket,
        2 => ProjectileKind::Mp5Grenade,
        3 => ProjectileKind::HandGrenade,
        4 => ProjectileKind::Hornet,
        5 => ProjectileKind::Snark,
        _ => return None,
    })
}

/// Captures one entity's `SECTION_ENTITY_COMBAT` (24) entry, or `None` when
/// the entity no longer exists in the world at all (a gibbed monster,
/// `world.despawn`ed outright by `crate::ai::AiState::retire`) — restoring
/// a `None` entry despawns the entity again, since a fresh level load
/// always re-spawns every map-declared monster first (`crate::ai::AiState::
/// attach_level`'s own `attach_monsters` call knows nothing about what a
/// save recorded).
///
/// Health is read from `ohl_ai::Actor::health` when the entity carries one
/// — the value `ohl_ai::apply_monster_damage` actually moves — falling back
/// to the bare `ohl_combat::Health` component's own `current`/`max`
/// otherwise; a monster carries both, so `Health::max` still supplies the
/// maximum an `Actor` alone does not record.
#[must_use]
pub(crate) fn snapshot_entity_combat(
    level: &Level,
    entity: Entity,
) -> Option<EntityCombatSnapshot> {
    if !level.registry.world.contains(entity) {
        return None;
    }
    let bare_health = level
        .registry
        .world
        .get::<&ohl_combat::Health>(entity)
        .ok()
        .map(|health| (health.current, health.max));
    let health = level
        .registry
        .world
        .get::<&ohl_ai::Actor>(entity)
        .ok()
        .map(|actor| {
            (
                actor.health,
                bare_health.map_or(actor.health, |(_, max)| max),
            )
        })
        .or(bare_health);
    let armor = level
        .registry
        .world
        .get::<&ohl_combat::Armor>(entity)
        .ok()
        .map(|armor| (armor.current, armor.max));
    Some(EntityCombatSnapshot { health, armor })
}

/// Restores one entity's `SECTION_ENTITY_COMBAT` (24) entry. `snapshot`
/// being `None` despawns `entity` when the (freshly attach-level-spawned)
/// level still has it live, so a monster the save recorded as gone stays
/// gone after the load; `Some` writes every component this entity still
/// has that the snapshot describes, leaving alone any component the live
/// entity does not carry (a save taken against a different registry
/// shape).
pub(crate) fn restore_entity_combat(
    level: &mut Level,
    entity: Entity,
    snapshot: Option<&EntityCombatSnapshot>,
) {
    let Some(snapshot) = snapshot else {
        let _ = level.registry.world.despawn(entity);
        return;
    };
    // Non-finite health/armor values are sanitized to `0.0` the same way
    // `array_vec3` sanitizes a position, so a corrupt or adversarial save
    // cannot hand the simulation a `NaN` health.
    if let Some((current, _)) = snapshot.health
        && let Ok(mut actor) = level.registry.world.get::<&mut ohl_ai::Actor>(entity)
    {
        let current = sanitize_f32(current, 0.0);
        actor.health = current;
        actor.alive = current > 0.0;
    }
    if let Some((current, max)) = snapshot.health
        && let Ok(mut health) = level.registry.world.get::<&mut ohl_combat::Health>(entity)
    {
        health.current = sanitize_f32(current, 0.0);
        health.max = sanitize_f32(max, 0.0);
    }
    if let Some((current, max)) = snapshot.armor
        && let Ok(mut armor) = level.registry.world.get::<&mut ohl_combat::Armor>(entity)
    {
        armor.current = sanitize_f32(current, 0.0);
        armor.max = sanitize_f32(max, 0.0);
    }
}

/// `value`, as a plain `[f32; 3]`.
#[must_use]
pub(crate) fn vec3_array(value: Vec3) -> [f32; 3] {
    value.to_array()
}

/// A previously stored `[f32; 3]`, as a `Vec3`. Non-finite components are
/// zeroed rather than propagated, so a corrupt save cannot hand the
/// simulation a `NaN` position.
#[must_use]
pub(crate) fn array_vec3(value: [f32; 3]) -> Vec3 {
    let sanitize = |component: f32| {
        if component.is_finite() {
            component
        } else {
            0.0
        }
    };
    Vec3::new(sanitize(value[0]), sanitize(value[1]), sanitize(value[2]))
}

/// A previously stored scalar, sanitized the same way [`array_vec3`]
/// sanitizes each of its components: a non-finite value (a corrupt or
/// adversarial save) falls back to `default` rather than propagating a
/// `NaN`/`inf` into the simulation.
#[must_use]
pub(crate) fn sanitize_f32(value: f32, default: f32) -> f32 {
    if value.is_finite() { value } else { default }
}

/// The `ohl_ai::Conditions` a previous [`ohl_ai::Conditions::bits`] named,
/// masked down to the bits this build actually defines
/// (`ohl_ai::Conditions::NAMED`), so an out-of-range bit in a corrupt or
/// forward-written save cannot set a condition this build has no name for.
#[must_use]
pub(crate) fn masked_conditions(bits: u32) -> ohl_ai::Conditions {
    let mask = ohl_ai::Conditions::NAMED
        .iter()
        .fold(0u32, |mask, (_, bit)| mask | bit.bits());
    ohl_ai::Conditions::from_bits(bits & mask)
}

// --- `SECTION_MOVER_STATE` (28) -------------------------------------------
//
// A `func_train`/`func_tracktrain`'s mid-route position, a `trigger_camera`
// sequence's active/hold/path progress, a `scripted_sequence`'s possession
// state, a `monstermaker`'s spawn counters, and a `trigger_auto`'s one-shot
// `fired` flag were, before this section, each a documented gap:
// `ohl_game::track_train::TrackTrainState` and
// `ohl_game::camera::TriggerCameraState` are hecs components with no save
// entry at all (see `crate::camera`'s "Save/load" module section for why
// widening `SECTION_SIMULATION` was rejected — postcard is not
// self-describing, so an added field fails a save written before it
// existed rather than filling in a default), a running script's state
// lived only in `crate::ai::AiState::scripts`/`sentences`, a
// `monstermaker`'s counters were reset by every reload, and
// `ohl_game::registry::AutoTrigger::fired` (see that field's own "Save/load
// note" doc comment) being unsaved meant every `trigger_auto` replayed on
// load — silently re-toggling, and so stopping, any train/camera this very
// section had just finished restoring to an active state. This section
// fixes all five at once, additively, with its own new tag rather than
// touching any of tags 16-27.
//
// # A `trigger_auto` removed on fire — verified, not just asserted
//
// `AutoTrigger::fired` above is only ever read off a *live* entity: one
// with the published `Remove On fire` spawnflag set
// (`ohl_game::registry::SPAWNFLAG_TRIGGER_AUTO_REMOVE_ON_FIRE`) is
// `world.despawn`ed by `ohl_game::logic::Simulation::fire_auto_triggers`
// the moment it fires, so `snapshot_movers` below finds no `AutoTrigger`
// component at that spawn index (`.get::<&AutoTrigger>` fails) and records
// `auto_trigger_fired: None` for it. This does **not** leave a replay gap,
// though, and this was checked against the actual restore-order code
// rather than assumed: `crate::save_state::restore_entity_combat`
// (`SECTION_ENTITY_COMBAT`, tag 24, already existing, unrelated to this
// section) despawns *any* spawn-index entity — not only a monster — whose
// slot's `EntityCombatSnapshot` is `None`, and `snapshot_entity_combat`
// records exactly that `None` for any entity `level.registry.world.
// contains` reports gone at save time, a `Remove On fire`-despawned
// `trigger_auto` included. So although `attach_level` does unconditionally
// recreate that entity fresh on load (with a new `AutoTrigger { fired:
// false, .. }`), tag 24's restore (applied earlier in `Game::restore`,
// before this section's own) despawns it right back before this section's
// `auto_trigger_fired: None` is even consulted — a `Remove On fire`
// `trigger_auto` that had already fired before the save stays gone, and
// does not refire, exactly like every other despawned entity a save
// records. Confirmed with a dedicated integration test
// (`crates/ohl-engine/tests/save_sections.rs`,
// `a_remove_on_fire_trigger_auto_that_already_fired_does_not_refire_after_a_load`).
//
// # `MonsterMakerSnapshot::pending_activation`
//
// A `monstermaker`'s own `MakerActivation::pending` counter (bumped by
// `ohl_game::logic::Simulation::activate` in phase 12, drained by
// `crate::ai::AiState::tick_makers` in phase 10 the *following* tick) is
// carried too, for the same reason `auto_trigger_fired` is: a save taken
// in that one-tick window would otherwise lose a trigger that had already
// fired but not yet reached the `Spawner`.

/// The most entities one `SECTION_MOVER_STATE` section records, matching
/// [`MAX_SNAPSHOT_ENTITIES`].
pub const MAX_SNAPSHOT_MOVERS: usize = MAX_SNAPSHOT_ENTITIES;

/// A `func_train`/`func_tracktrain`'s runtime position, part of
/// [`MoverSnapshot`]. Mirrors
/// [`ohl_game::TrackTrainState::dynamic_state`]'s tuple fields one for one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackTrainSnapshot {
    /// The node the train last departed from (or rests at).
    pub node_index: u32,
    /// Progress toward the other end of the active segment, in `0..=1`.
    pub t: f32,
    /// `1.0` toward the chain's next node, `-1.0` toward its previous one.
    pub direction: f32,
    /// Current speed magnitude, units/second.
    pub speed: f32,
    /// Whether the train is currently moving.
    pub moving: bool,
    /// Seconds remaining in a `path_track`'s `wait` pause.
    pub wait_timer: f32,
}

/// A `trigger_camera` sequence's runtime progress, part of
/// [`MoverSnapshot`]. Mirrors
/// [`ohl_game::TriggerCameraState::dynamic_state`]'s tuple fields one for
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TriggerCameraSnapshot {
    /// The node this sequence last departed from (or rests at), meaningless
    /// for a stationary camera with no `moveto` chain.
    pub node_index: u32,
    /// Progress toward the next node, in `0..=1`.
    pub t: f32,
    /// Current travel speed, units/second.
    pub speed: f32,
    /// Seconds remaining in a `path_corner`'s own `wait` pause.
    pub wait_timer: f32,
    /// Whether this sequence is currently overriding the player's view.
    pub active: bool,
    /// Seconds remaining before this sequence reverts the player's view.
    pub hold_remaining: f32,
}

/// A `scripted_sequence`/`aiscripted_sequence`'s possession state, part of
/// [`MoverSnapshot`]. Mirrors `ohl_ai::scripts::ScriptRunner`'s own dynamic
/// fields, plus the engine-side `ActiveScript` bookkeeping
/// (`crate::ai::AiState::scripts`) that decides which monster it holds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScriptRunnerSnapshot {
    /// `ohl_ai::scripts::ScriptPhase::tag`.
    pub phase_tag: u8,
    /// `ohl_ai::scripts::ScriptRunner::timer`.
    pub timer: f32,
    /// `ohl_ai::scripts::ScriptRunner::completions`.
    pub completions: u32,
    /// `ohl_ai::scripts::ScriptRunner::warped`.
    pub warped: bool,
    /// `ohl_ai::scripts::ScriptRunner::moving_elapsed`.
    pub moving_elapsed: f32,
    /// The monster this script currently possesses, as a spawn index.
    /// `None` when the script has not (yet) bound one — the common case
    /// for a `monstermaker`-spawned target, which carries no spawn index
    /// at all (see this module's own "Monstermaker children are not
    /// saved" note); such a script simply searches again after the load,
    /// exactly as it does after any other spawn.
    pub actor: Option<u32>,
    /// `ActiveScript::pending_trigger`.
    pub pending_trigger: bool,
    /// `ActiveScript::was_active`.
    pub was_active: bool,
    /// `ActiveScript::played`.
    pub played: f32,
    /// `ActiveScript::play_origin`.
    pub play_origin: [f32; 3],
}

/// A `monstermaker`'s spawn counters, part of [`MoverSnapshot`]. Mirrors
/// `ohl_ai::Spawner::restore_counters`'s parameters, plus
/// [`ohl_ai::Spawner::live_children`] for information (see that method's
/// own doc comment for why the live-child *entity list* itself is not, and
/// cannot yet be, restored) and `pending_activation`, the maker's own
/// `MakerActivation::pending` counter.
///
/// `pending_activation` closes a one-tick window a review of this section
/// found: `ohl_game::logic::Simulation::activate` (phase 12, the last
/// phase of a tick) bumps `MakerActivation::pending`, but
/// `crate::ai::AiState::tick_makers` (phase 10) only drains it the
/// *following* tick — so a save taken in between (after a trigger fired
/// this tick, before the next tick's phase 10 has run) would otherwise
/// lose that trigger on load, exactly like `SECTION_MOVER_STATE`'s own
/// `auto_trigger_fired` fixes the analogous `trigger_auto` gap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MonsterMakerSnapshot {
    /// `ohl_ai::Spawner::spawned_total`.
    pub spawned_total: u32,
    /// `ohl_ai::Spawner::live_children`, informational only — see this
    /// struct's own doc comment.
    pub live_children: u32,
    /// `ohl_ai::Spawner::is_active`.
    pub active: bool,
    /// `ohl_ai::Spawner::timer`.
    pub timer: f32,
    /// `ohl_game::registry::MakerActivation::pending` — see this struct's
    /// own doc comment.
    pub pending_activation: u32,
}

/// A `func_rotating`'s mutable spin state: `ohl_game::registry::Rotator::
/// spinning`/`angle_deg`. `axis`/`speed` are fixed at spawn from keyvalues
/// and spawnflags (`attach_level` always rebuilds them identically), so
/// only the state a player's `use`/trigger can flip, or that accumulates
/// while spinning, needs to round-trip. Without this a `func_rotating`
/// toggled on reverts to its spawnflag default (`spinning: start_on`,
/// `angle_deg: 0.0`) across a save/load or level transition, exactly the
/// gap noted alongside `Rotator`'s introduction in `docs/FORMAT_SOURCES.md`
/// `TODO(black-box)` item 24.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RotatorSnapshot {
    /// `ohl_game::registry::Rotator::spinning`.
    pub spinning: bool,
    /// `ohl_game::registry::Rotator::angle_deg`.
    pub angle_deg: f32,
}

/// `SECTION_MOVER_STATE` (28): one optional entry per `Registry::entities`
/// slot, in spawn order, covering everything a fresh `attach_level` cannot
/// otherwise reconstruct about a mover, camera sequence, running script,
/// `monstermaker`, or `trigger_auto`. `None` for an entity with none of the
/// five.
///
/// **This struct's shape is frozen.** Tag 28 is written by every save this
/// crate produces and decoded as one fixed, non-self-describing `postcard`
/// shape, so any field added to it — or to one of the five snapshot types
/// it holds — makes every previously written save whose tag 28 is
/// non-empty (a map with one `trigger_auto` is enough) fail to decode.
/// [`Self::rotator`] is the cautionary case: it was added here after tag 28
/// already shipped, and it duplicates state `SECTION_ENTITY_REGISTRY` (tag
/// 18) already persists for the same entity in the same spawn order,
/// through `crate::transition::EntitySnapshot::rotator`. It stays anyway,
/// because removing it would move the wire shape a second time and misread
/// every save written since — see [`crate::save`]'s "Frozen section shapes"
/// doc and `docs/FORMAT_SOURCES.md` `TODO(black-box)` item 28. The two
/// copies cannot disagree in practice (both are captured from the same
/// component in the same snapshot); if they ever did, this one is applied
/// last and would win. `crates/ohl-engine/tests/save_format_frozen.rs`
/// pins this shape with golden bytes.
///
/// `func_rot_button`/`momentary_rot_button`/`func_pendulum` deliberately do
/// **not** live here, even though they are movers in the same sense: this
/// is a *required* section (`crate::save::GameSave::mover_state` is written
/// unconditionally whenever it is `Some`, and every save this crate writes
/// sets it), so a `postcard` field added to [`MoverSnapshot`] fails every
/// save written before that field existed, the same non-self-describing-
/// wire-format hazard [`crate::save`]'s own module doc records for the
/// truly required tags (16-22). Those three entities' state instead rides
/// `SECTION_ROTATING_MOVER_STATE` (tag 30, see [`RotatingMoverSnapshot`]),
/// an *optional* section that costs an absent-tag `None` rather than a
/// decode failure when it is missing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MoverSnapshot {
    /// This entity's `func_train`/`func_tracktrain` state, when it has one.
    pub track_train: Option<TrackTrainSnapshot>,
    /// This entity's `trigger_camera` state, when it has one.
    pub camera: Option<TriggerCameraSnapshot>,
    /// This entity's running-script state, when it is a
    /// `scripted_sequence`/`aiscripted_sequence`.
    pub script: Option<ScriptRunnerSnapshot>,
    /// This entity's `monstermaker` counters, when it is one.
    pub maker: Option<MonsterMakerSnapshot>,
    /// This entity's `func_rotating` spin state, when it has one.
    /// Redundant with `crate::transition::EntitySnapshot::rotator` (tag
    /// 18), and kept only because tag 28's wire shape is frozen with it in
    /// place: see this struct's own doc comment.
    pub rotator: Option<RotatorSnapshot>,
    /// `ohl_game::registry::AutoTrigger::fired`, when this entity is a
    /// `trigger_auto`. Not itself a mover/camera/script/maker, but carried
    /// in the same section on that struct's own explicit invitation (see
    /// its "Save/load note for M7.9 P4b" doc comment): without this, every
    /// `trigger_auto` on the map replays on every load, since
    /// `attach_level` always rebuilds it fresh with `fired: false`, which
    /// would otherwise re-toggle (and so silently stop) any camera/train
    /// this section just finished restoring to an active state.
    pub auto_trigger_fired: Option<bool>,
}

impl MoverSnapshot {
    /// Whether every field is `None`, so a caller can collapse an entry to
    /// `None` instead of storing an empty struct.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.track_train.is_none()
            && self.camera.is_none()
            && self.script.is_none()
            && self.maker.is_none()
            && self.rotator.is_none()
            && self.auto_trigger_fired.is_none()
    }
}

/// A `func_rot_button`'s press/return state:
/// `ohl_game::registry::RotButton::state`/`timer`. `axis`/`speed`/`distance`
/// etc. are fixed at spawn (`attach_level` always rebuilds them identically),
/// so only the state a player's `use`/touch can flip needs to round-trip —
/// the same reasoning already recorded for [`RotatorSnapshot`]. Part of
/// [`RotatingMoverSnapshot`] (`SECTION_ROTATING_MOVER_STATE`, tag 30).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RotButtonSnapshot {
    /// `ohl_game::registry::RotButton::state`.
    pub state: MoverState,
    /// `ohl_game::registry::RotButton::timer`.
    pub timer: f32,
}

/// A `momentary_rot_button`'s turn state:
/// `ohl_game::registry::MomentaryRotButton::fraction`/`moving_forward`/
/// `returning`. Part of [`RotatingMoverSnapshot`]
/// (`SECTION_ROTATING_MOVER_STATE`, tag 30).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MomentaryRotButtonSnapshot {
    /// `ohl_game::registry::MomentaryRotButton::fraction`.
    pub fraction: f32,
    /// `ohl_game::registry::MomentaryRotButton::moving_forward`.
    pub moving_forward: bool,
    /// `ohl_game::registry::MomentaryRotButton::returning`.
    pub returning: bool,
}

/// A `func_pendulum`'s swing state:
/// `ohl_game::registry::Pendulum::swinging`/`elapsed`/`returning`/
/// `angle_deg`. Part of [`RotatingMoverSnapshot`]
/// (`SECTION_ROTATING_MOVER_STATE`, tag 30).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PendulumSnapshot {
    /// `ohl_game::registry::Pendulum::swinging`.
    pub swinging: bool,
    /// `ohl_game::registry::Pendulum::elapsed`.
    pub elapsed: f32,
    /// `ohl_game::registry::Pendulum::returning`.
    pub returning: bool,
    /// `ohl_game::registry::Pendulum::angle_deg`.
    pub angle_deg: f32,
}

/// `SECTION_ROTATING_MOVER_STATE` (30): one optional entry per
/// `Registry::entities` slot, in spawn order, covering
/// `func_rot_button`/`momentary_rot_button`/`func_pendulum` runtime state.
/// `None` for an entity with none of the three.
///
/// A new tag, not a `MoverSnapshot` (tag 28) addition: see that struct's
/// own doc comment for why. `crate::save::RotatingMoverStateSnapshot` pairs
/// a `Vec<Option<Self>>` (this) with the separate
/// `ohl_game::logic::Simulation::rot_button_touch_snapshot` bookkeeping
/// under the same tag.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RotatingMoverSnapshot {
    /// This entity's `func_rot_button` press state, when it has one.
    pub rot_button: Option<RotButtonSnapshot>,
    /// This entity's `momentary_rot_button` turn state, when it has one.
    pub momentary_rot_button: Option<MomentaryRotButtonSnapshot>,
    /// This entity's `func_pendulum` swing state, when it has one.
    pub pendulum: Option<PendulumSnapshot>,
}

impl RotatingMoverSnapshot {
    /// Whether every field is `None`, so a caller can collapse an entry to
    /// `None` instead of storing an empty struct.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.rot_button.is_none() && self.momentary_rot_button.is_none() && self.pendulum.is_none()
    }
}

/// `SECTION_MOVER_STATE` (28)'s track-train/camera/`monstermaker`/
/// `trigger_auto` portion — everything readable straight off a `hecs`
/// component without needing
/// `crate::ai::AiState`'s own script bookkeeping. `crate::systems::Systems`
/// merges [`crate::ai::AiState::snapshot_scripts`] into the result (the
/// `script` field of each entry) to build the section this crate actually
/// writes.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn snapshot_movers(level: &Level) -> Vec<Option<MoverSnapshot>> {
    level
        .registry
        .entities
        .iter()
        .take(MAX_SNAPSHOT_MOVERS)
        .map(|entity| {
            let track_train = level
                .registry
                .world
                .get::<&TrackTrainState>(*entity)
                .ok()
                .map(|state| {
                    let (node_index, t, direction, speed, moving, wait_timer) =
                        state.dynamic_state();
                    TrackTrainSnapshot {
                        node_index: u32::try_from(node_index).unwrap_or(u32::MAX),
                        t,
                        direction,
                        speed,
                        moving,
                        wait_timer,
                    }
                });
            let camera = level
                .registry
                .world
                .get::<&TriggerCameraState>(*entity)
                .ok()
                .map(|state| {
                    let (node_index, t, speed, wait_timer, active, hold_remaining) =
                        state.dynamic_state();
                    TriggerCameraSnapshot {
                        node_index: u32::try_from(node_index).unwrap_or(u32::MAX),
                        t,
                        speed,
                        wait_timer,
                        active,
                        hold_remaining,
                    }
                });
            let maker = level
                .registry
                .world
                .get::<&MonsterMaker>(*entity)
                .ok()
                .map(|maker| {
                    let pending_activation = level
                        .registry
                        .world
                        .get::<&MakerActivation>(*entity)
                        .map_or(0, |activation| activation.pending);
                    MonsterMakerSnapshot {
                        spawned_total: maker.0.spawned_total(),
                        live_children: u32::try_from(maker.0.live_children()).unwrap_or(u32::MAX),
                        active: maker.0.is_active(),
                        timer: maker.0.timer(),
                        pending_activation,
                    }
                });
            let rotator = level
                .registry
                .world
                .get::<&Rotator>(*entity)
                .ok()
                .map(|rotator| RotatorSnapshot {
                    spinning: rotator.spinning,
                    angle_deg: rotator.angle_deg,
                });
            let auto_trigger_fired = level
                .registry
                .world
                .get::<&AutoTrigger>(*entity)
                .ok()
                .map(|auto| auto.fired);
            let snapshot = MoverSnapshot {
                track_train,
                camera,
                script: None,
                maker,
                rotator,
                auto_trigger_fired,
            };
            (!snapshot.is_empty()).then_some(snapshot)
        })
        .collect()
}

/// Restores [`snapshot_movers`]'s track-train/camera/`monstermaker`/`trigger_auto` portion,
/// zipped against `level.registry.entities` in spawn order. The `script`
/// field of each entry is restored separately, by
/// `crate::ai::AiState::restore_scripts`.
pub(crate) fn restore_movers(level: &mut Level, snapshots: &[Option<MoverSnapshot>]) {
    let entities = level.registry.entities.clone();
    for (entity, snapshot) in entities.iter().zip(snapshots) {
        let Some(snapshot) = snapshot else { continue };
        if let Some(train) = &snapshot.track_train
            && let Ok(mut state) = level.registry.world.get::<&mut TrackTrainState>(*entity)
        {
            state.restore_dynamic_state(
                usize::try_from(train.node_index).unwrap_or(usize::MAX),
                train.t,
                train.direction,
                train.speed,
                train.moving,
                train.wait_timer,
            );
        }
        if let Some(camera) = &snapshot.camera
            && let Ok(mut state) = level.registry.world.get::<&mut TriggerCameraState>(*entity)
        {
            state.restore_dynamic_state(
                usize::try_from(camera.node_index).unwrap_or(usize::MAX),
                camera.t,
                camera.speed,
                camera.wait_timer,
                camera.active,
                camera.hold_remaining,
            );
        }
        if let Some(maker) = &snapshot.maker {
            if let Ok(mut component) = level.registry.world.get::<&mut MonsterMaker>(*entity) {
                component
                    .0
                    .restore_counters(maker.spawned_total, maker.active, maker.timer);
            }
            if let Ok(mut activation) = level.registry.world.get::<&mut MakerActivation>(*entity) {
                activation.pending = maker.pending_activation.min(MakerActivation::MAX_PENDING);
            }
        }
        if let Some(rotator) = &snapshot.rotator
            && let Ok(mut component) = level.registry.world.get::<&mut Rotator>(*entity)
        {
            component.spinning = rotator.spinning;
            component.angle_deg = rotator.angle_deg;
        }
        if let Some(fired) = snapshot.auto_trigger_fired
            && let Ok(mut auto) = level.registry.world.get::<&mut AutoTrigger>(*entity)
        {
            auto.fired = fired;
        }
    }
}

// --- `SECTION_ROTATING_MOVER_STATE` (30) ----------------------------------

/// The most entities one `SECTION_ROTATING_MOVER_STATE` section records,
/// matching [`MAX_SNAPSHOT_ENTITIES`] — the same per-registry-slot cap
/// every other index-keyed section already uses.
pub const MAX_SNAPSHOT_ROTATING_MOVERS: usize = MAX_SNAPSHOT_ENTITIES;

/// `SECTION_ROTATING_MOVER_STATE` (30)'s entity-indexed portion: one
/// optional [`RotatingMoverSnapshot`] per `Registry::entities` slot, in
/// spawn order. `crate::save::RotatingMoverStateSnapshot` (the whole
/// section's payload) pairs this with the separate
/// `ohl_game::logic::Simulation::rot_button_touch_snapshot` bookkeeping,
/// which has no entity-indexed shape to share this `Vec`'s slots with.
#[must_use]
pub(crate) fn snapshot_rotating_movers(level: &Level) -> Vec<Option<RotatingMoverSnapshot>> {
    level
        .registry
        .entities
        .iter()
        .take(MAX_SNAPSHOT_ROTATING_MOVERS)
        .map(|entity| {
            let rot_button = level
                .registry
                .world
                .get::<&RotButton>(*entity)
                .ok()
                .map(|button| RotButtonSnapshot {
                    state: button.state,
                    timer: button.timer,
                });
            let momentary_rot_button = level
                .registry
                .world
                .get::<&MomentaryRotButton>(*entity)
                .ok()
                .map(|button| MomentaryRotButtonSnapshot {
                    fraction: button.fraction,
                    moving_forward: button.moving_forward,
                    returning: button.returning,
                });
            let pendulum = level
                .registry
                .world
                .get::<&Pendulum>(*entity)
                .ok()
                .map(|pendulum| PendulumSnapshot {
                    swinging: pendulum.swinging,
                    elapsed: pendulum.elapsed,
                    returning: pendulum.returning,
                    angle_deg: pendulum.angle_deg,
                });
            let snapshot = RotatingMoverSnapshot {
                rot_button,
                momentary_rot_button,
                pendulum,
            };
            (!snapshot.is_empty()).then_some(snapshot)
        })
        .collect()
}

/// Restores [`snapshot_rotating_movers`], zipped against
/// `level.registry.entities` in spawn order.
pub(crate) fn restore_rotating_movers(
    level: &mut Level,
    snapshots: &[Option<RotatingMoverSnapshot>],
) {
    let entities = level.registry.entities.clone();
    for (entity, snapshot) in entities.iter().zip(snapshots) {
        let Some(snapshot) = snapshot else { continue };
        if let Some(button) = &snapshot.rot_button
            && let Ok(mut component) = level.registry.world.get::<&mut RotButton>(*entity)
        {
            component.state = button.state;
            component.timer = button.timer;
        }
        if let Some(button) = &snapshot.momentary_rot_button
            && let Ok(mut component) = level.registry.world.get::<&mut MomentaryRotButton>(*entity)
        {
            component.fraction = button.fraction;
            component.moving_forward = button.moving_forward;
            component.returning = button.returning;
        }
        if let Some(pendulum) = &snapshot.pendulum
            && let Ok(mut component) = level.registry.world.get::<&mut Pendulum>(*entity)
        {
            component.swinging = pendulum.swinging;
            component.elapsed = pendulum.elapsed;
            component.returning = pendulum.returning;
            component.angle_deg = pendulum.angle_deg;
        }
    }
}

// --- `SECTION_MOMENTARY_DOOR_STATE` (31) ----------------------------------

/// A `momentary_door`'s own position: `ohl_game::registry::MomentaryDoor::
/// fraction`. `speed`/`lip`/`movedir`/`travel_distance` are fixed at spawn
/// (`attach_level` always rebuilds them identically), so only the fraction a
/// driving `momentary_rot_button` can move needs to round-trip — the same
/// reasoning already recorded for [`RotatorSnapshot`]/
/// [`MomentaryRotButtonSnapshot`]. Part of `SECTION_MOMENTARY_DOOR_STATE`
/// (tag 31, see `crate::save::SECTION_MOMENTARY_DOOR_STATE`'s own doc
/// comment for why this is a **new** tag rather than an addition to tag 30).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MomentaryDoorSnapshot {
    /// `ohl_game::registry::MomentaryDoor::fraction`.
    pub fraction: f32,
}

/// The most entities one `SECTION_MOMENTARY_DOOR_STATE` section records,
/// matching [`MAX_SNAPSHOT_ENTITIES`] — the same per-registry-slot cap every
/// other index-keyed section already uses.
pub const MAX_SNAPSHOT_MOMENTARY_DOORS: usize = MAX_SNAPSHOT_ENTITIES;

/// `SECTION_MOMENTARY_DOOR_STATE` (31)'s whole payload: one optional
/// [`MomentaryDoorSnapshot`] per `Registry::entities` slot, in spawn order.
/// `None` for an entity with no [`MomentaryDoor`].
#[must_use]
pub(crate) fn snapshot_momentary_doors(level: &Level) -> Vec<Option<MomentaryDoorSnapshot>> {
    level
        .registry
        .entities
        .iter()
        .take(MAX_SNAPSHOT_MOMENTARY_DOORS)
        .map(|entity| {
            level
                .registry
                .world
                .get::<&MomentaryDoor>(*entity)
                .ok()
                .map(|door| MomentaryDoorSnapshot {
                    fraction: door.fraction,
                })
        })
        .collect()
}

/// Restores [`snapshot_momentary_doors`], zipped against
/// `level.registry.entities` in spawn order.
pub(crate) fn restore_momentary_doors(
    level: &mut Level,
    snapshots: &[Option<MomentaryDoorSnapshot>],
) {
    let entities = level.registry.entities.clone();
    for (entity, snapshot) in entities.iter().zip(snapshots) {
        let Some(snapshot) = snapshot else { continue };
        if let Ok(mut component) = level.registry.world.get::<&mut MomentaryDoor>(*entity) {
            component.fraction = snapshot.fraction;
        }
    }
}

// --- `SECTION_BREAKABLE_STATE` (33) ---------------------------------------

/// A `func_breakable`/`func_pushable`'s own runtime state: remaining hit
/// points, whether it has broken, and how far a `func_pushable` has been
/// pushed. Everything else about either entity (`material`, the spawnflag-
/// derived break rules, `friction`) is fixed at spawn and rebuilt
/// identically by `attach_level` every load, so only these three need to
/// round-trip — the same reasoning already recorded for
/// [`MomentaryDoorSnapshot`]. Part of `SECTION_BREAKABLE_STATE` (tag 33;
/// see `crate::save::SECTION_BREAKABLE_STATE` for why it is a new tag).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BreakableSnapshot {
    /// `ohl_game::registry::Breakable::health`.
    pub health: f32,
    /// `ohl_game::registry::Breakable::broken`.
    pub broken: bool,
    /// `ohl_game::registry::Pushable::offset`, `[0.0; 3]` for an entity
    /// with no `Pushable` (an ordinary `func_breakable` never moves).
    pub push_offset: [f32; 3],
}

/// The most entities one `SECTION_BREAKABLE_STATE` section records,
/// matching [`MAX_SNAPSHOT_ENTITIES`] — the same per-registry-slot cap
/// every other index-keyed section already uses.
pub const MAX_SNAPSHOT_BREAKABLES: usize = MAX_SNAPSHOT_ENTITIES;

/// `SECTION_BREAKABLE_STATE` (33)'s whole payload: one optional
/// [`BreakableSnapshot`] per `Registry::entities` slot, in spawn order.
/// `None` for an entity with no [`Breakable`].
#[must_use]
pub(crate) fn snapshot_breakables(level: &Level) -> Vec<Option<BreakableSnapshot>> {
    level
        .registry
        .entities
        .iter()
        .take(MAX_SNAPSHOT_BREAKABLES)
        .map(|entity| {
            level
                .registry
                .world
                .get::<&Breakable>(*entity)
                .ok()
                .map(|breakable| BreakableSnapshot {
                    health: breakable.health,
                    broken: breakable.broken,
                    push_offset: level
                        .registry
                        .world
                        .get::<&Pushable>(*entity)
                        .map_or([0.0; 3], |pushable| pushable.offset.to_array()),
                })
        })
        .collect()
}

/// Restores [`snapshot_breakables`], zipped against
/// `level.registry.entities` in spawn order. A restored *broken* brush stays
/// broken: the next `Level::sync_brush_collision` detaches its hull again,
/// and `ohl_game::brush`'s instance lists leave it out of both the drawn and
/// the solid set, exactly as they did before the save.
pub(crate) fn restore_breakables(level: &mut Level, snapshots: &[Option<BreakableSnapshot>]) {
    let entities = level.registry.entities.clone();
    for (entity, snapshot) in entities.iter().zip(snapshots) {
        let Some(snapshot) = snapshot else { continue };
        if let Ok(mut breakable) = level.registry.world.get::<&mut Breakable>(*entity) {
            breakable.health = snapshot.health;
            breakable.broken = snapshot.broken;
        }
        if let Ok(mut pushable) = level.registry.world.get::<&mut Pushable>(*entity) {
            pushable.offset = Vec3::from_array(snapshot.push_offset);
        }
    }
}

// --- `SECTION_MAKER_CHILDREN` (29) ----------------------------------------

/// The most maker children one `SECTION_MAKER_CHILDREN` section records,
/// matching [`MAX_SNAPSHOT_ENTITIES`] — the same per-registry-slot cap
/// every other index-keyed section already uses.
pub const MAX_SNAPSHOT_MAKER_CHILDREN: usize = MAX_SNAPSHOT_ENTITIES;

/// One `monstermaker` child, part of `SECTION_MAKER_CHILDREN` (29): just
/// enough to recreate the entity itself (`crate::ai::AiState::
/// restore_maker_children`) before the index-keyed sections (18/24/25) that
/// already carry its transform/health/AI state are applied. See this
/// module's own "Monstermaker children are now saved" doc section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonsterMakerChildSnapshot {
    /// The `monstermaker` that spawned this child, as a spawn index.
    pub maker: u32,
    /// The classname `crate::ai::AiState::spawn_child` looked its brain up
    /// by (a `monstertype` keyvalue value, e.g. `monster_headcrab` —
    /// map-declared data, not a literal this project invented; see
    /// `docs/CLEAN_ROOM.md`).
    pub classname: String,
}

/// `SECTION_MAKER_CHILDREN` (29): one optional entry per `Registry::
/// entities` slot, in spawn order, `None` for a slot that is not (or, for
/// a child already gone by save time, is no longer identifiably) a
/// `monstermaker` child.
#[must_use]
pub(crate) fn snapshot_maker_children(level: &Level) -> Vec<Option<MonsterMakerChildSnapshot>> {
    level
        .registry
        .entities
        .iter()
        .take(MAX_SNAPSHOT_MAKER_CHILDREN)
        .map(|entity| -> Option<MonsterMakerChildSnapshot> {
            let owner = level.registry.world.get::<&Owner>(*entity).ok()?;
            // Only an `Owner` pointing at a live `MonsterMaker` counts: a
            // projectile/deployable also carries `Owner` (attributing a
            // hit back to whoever fired it), but is not a maker child and
            // has its own section (`SECTION_PROJECTILES`, 26).
            level.registry.world.get::<&MonsterMaker>(owner.0).ok()?;
            let maker = spawn_index_of(level, owner.0)?;
            let classname = level.registry.world.get::<&ClassName>(*entity).ok()?;
            Some(MonsterMakerChildSnapshot {
                maker,
                classname: classname.0.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        array_vec3, masked_conditions, projectile_kind_from_tag, projectile_kind_tag, sanitize_f32,
        vec3_array,
    };
    use ohl_combat::ProjectileKind;

    const ALL_KINDS: [ProjectileKind; 6] = [
        ProjectileKind::CrossbowBolt,
        ProjectileKind::Rocket,
        ProjectileKind::Mp5Grenade,
        ProjectileKind::HandGrenade,
        ProjectileKind::Hornet,
        ProjectileKind::Snark,
    ];

    #[test]
    fn every_projectile_kind_round_trips_through_its_tag() {
        for kind in ALL_KINDS {
            assert_eq!(
                projectile_kind_from_tag(projectile_kind_tag(kind)),
                Some(kind)
            );
        }
    }

    #[test]
    fn an_unrecognised_tag_is_rejected() {
        assert_eq!(projectile_kind_from_tag(255), None);
    }

    #[test]
    fn a_vec3_round_trips_through_its_array() {
        let value = glam::Vec3::new(1.0, -2.0, 3.5);
        assert_eq!(array_vec3(vec3_array(value)), value);
    }

    #[test]
    fn a_non_finite_component_is_sanitized_to_zero() {
        let restored = array_vec3([f32::NAN, f32::INFINITY, 4.0]);
        assert_eq!(restored, glam::Vec3::new(0.0, 0.0, 4.0));
    }

    #[test]
    fn sanitize_f32_passes_through_a_finite_value() {
        assert!((sanitize_f32(3.5, 0.0) - 3.5).abs() < f32::EPSILON);
    }

    #[test]
    fn sanitize_f32_falls_back_on_non_finite_input() {
        assert!((sanitize_f32(f32::NAN, 7.0) - 7.0).abs() < f32::EPSILON);
        assert!((sanitize_f32(f32::INFINITY, 7.0) - 7.0).abs() < f32::EPSILON);
        assert!((sanitize_f32(f32::NEG_INFINITY, 7.0) - 7.0).abs() < f32::EPSILON);
    }

    #[test]
    fn masked_conditions_drops_bits_this_build_does_not_name() {
        let all_named = ohl_ai::Conditions::NAMED
            .iter()
            .fold(0u32, |mask, (_, bit)| mask | bit.bits());
        assert_eq!(masked_conditions(u32::MAX).bits(), all_named);
        assert_eq!(masked_conditions(0).bits(), 0);
    }

    #[test]
    fn masked_conditions_keeps_every_named_bit() {
        for (_, bit) in ohl_ai::Conditions::NAMED {
            assert_eq!(masked_conditions(bit.bits()), *bit);
        }
    }
}

#[cfg(test)]
mod decode_proptests {
    use proptest::prelude::*;

    use super::{
        AiSnapshot, EntityCombatSnapshot, InventorySnapshot, MonsterMakerChildSnapshot,
        ProjectilesSnapshot, RngSnapshot,
    };

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Decoding arbitrary bytes as any M7.9 P4b (or, for
        /// `MonsterMakerChildSnapshot`, M9.5) snapshot type never panics,
        /// matching `ohl_save`'s and `ohl_engine::GameSave`'s own "never
        /// panic on adversarial section bytes" guarantee.
        #[test]
        fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _: Result<InventorySnapshot, _> = postcard::from_bytes(&bytes);
            let _: Result<EntityCombatSnapshot, _> = postcard::from_bytes(&bytes);
            let _: Result<Vec<Option<EntityCombatSnapshot>>, _> = postcard::from_bytes(&bytes);
            let _: Result<AiSnapshot, _> = postcard::from_bytes(&bytes);
            let _: Result<Vec<Option<AiSnapshot>>, _> = postcard::from_bytes(&bytes);
            let _: Result<MonsterMakerChildSnapshot, _> = postcard::from_bytes(&bytes);
            let _: Result<Vec<Option<MonsterMakerChildSnapshot>>, _> = postcard::from_bytes(&bytes);
            let _: Result<ProjectilesSnapshot, _> = postcard::from_bytes(&bytes);
            let _: Result<RngSnapshot, _> = postcard::from_bytes(&bytes);
        }
    }
}
