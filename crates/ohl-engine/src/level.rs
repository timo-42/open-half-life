//! One loaded map: geometry, entities, collision and the assets they name.

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_formats::bsp30::{Bsp, Limits as BspLimits};
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::{self, EntityDef, Limits as KeyvalueLimits, ModelRef};
use ohl_game::registry::{ClassName, Landmark, TargetName, Transform};
use ohl_game::{Registry, Simulation};
use ohl_physics::{BrushId, CollisionModel, ContentsKind};
use ohl_world::{
    LightRamp, PlayerSpawn, SKY_FACE_SUFFIXES, SkyboxAsset, StudioLimits, StudioModel,
    WorldBuildOptions, WorldModel,
};

use crate::assets::AssetSource;
use crate::components::{PlayerTag, StudioAnim};
use crate::error::{EngineError, Result};

/// The largest number of distinct studio models one level loads, so a map
/// full of props cannot make level loading unbounded.
const MAX_STUDIO_MODELS: usize = 96;

/// Classnames that name a sprite (`.spr`), not a studio model, even though
/// they sit alongside studio-model-carrying entities in the entity list.
/// These are collected as [`SpritePlacement`]s instead (see
/// [`collect_sprites`]), so they are excluded from studio-model loading even
/// if an entity happened to carry a `.mdl` `model` keyvalue.
const SPRITE_ONLY_CLASSES: [&str; 3] = ["env_sprite", "env_glow", "cycler_sprite"];

/// Whether `classname`'s `model` keyvalue (when it names a `.mdl` asset)
/// should be loaded and placed as a studio model.
///
/// Earlier this milestone only matched a four-prefix allowlist
/// (`monster_`, `cycler`, `env_model`, `prop_`), which missed most of
/// GoldSrc's documented model-carrying classes: the full `monster_*` family
/// (including `monster_generic` and `monster_furniture`, both of which
/// carry an explicit `model` keyvalue rather than a hardcoded one), and the
/// `item_*` / `weapon_*` / `ammo_*` pickup families, which all resolve
/// their world model from their own `model` keyvalue (see the HL1 entity
/// list on the Valve Developer Community / TWHL wikis). Rather than
/// enumerate every one of those prefixes, any classname is accepted as
/// long as it actually carries a `.mdl` `model` keyvalue and is not one of
/// the sprite-only classes above — that is a strict superset of the
/// documented list and cannot mis-place a brush or sprite entity, since
/// [`ohl_game::keyvalues::ModelRef::Brush`] and non-`.mdl` asset paths are
/// filtered out separately.
fn wants_studio_model(classname: &str) -> bool {
    !SPRITE_ONLY_CLASSES.contains(&classname)
}

/// One placed studio model: which loaded model to draw, and where.
#[derive(Debug, Clone, Copy)]
pub struct PropPlacement {
    /// Index into [`Level::studio_models`].
    pub model: usize,
    /// World-space origin.
    pub origin: [f32; 3],
    /// Yaw in degrees.
    pub yaw: f32,
    /// The `sequence` keyvalue; `0` (the model's first sequence) when
    /// absent or unparsable.
    pub sequence: usize,
    /// The `body` keyvalue; `0` when absent or unparsable.
    pub body: u32,
    /// The `skin` keyvalue; `0` when absent or unparsable.
    pub skin: usize,
    /// How far into the sequence this instance stands, in seconds.
    pub cycle: f32,
}

/// The largest number of distinct sprite assets one level loads, mirroring
/// [`MAX_STUDIO_MODELS`]'s bound for the same reason.
const MAX_SPRITE_ASSETS: usize = 96;

/// One `env_sprite` / `env_glow` / `cycler_sprite` entity's placement.
#[derive(Debug, Clone, Copy)]
pub struct SpritePlacement {
    /// Index into [`Level::sprite_assets`].
    pub sprite: usize,
    /// World-space origin.
    pub origin: [f32; 3],
    /// The `scale` keyvalue; `1.0` when absent or unparsable, matching
    /// GoldSrc's own default sprite scale.
    pub scale: f32,
    /// `rendermode`/`renderamt`/`rendercolor`, as GoldSrc's `env_sprite`
    /// resolves brightness and additive/glow blending from them.
    pub render: keyvalues::RenderProps,
}

/// Loads the sprite assets this map's `env_sprite`/`env_glow`/
/// `cycler_sprite` entities reference, skipping (and counting) the ones the
/// payload does not publish or that fail to decode.
fn load_sprites(
    source: &dyn AssetSource,
    defs: &[EntityDef],
) -> (Vec<ohl_world::SpriteAsset>, Vec<SpritePlacement>, usize) {
    let sprite_limits = ohl_world::SprLimits::default();
    let mut by_path: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut assets = Vec::new();
    let mut placements = Vec::new();
    let mut missing = 0usize;

    for def in defs {
        if !SPRITE_ONLY_CLASSES.contains(&def.classname.as_str()) {
            continue;
        }
        let Some(ModelRef::Asset(path)) = def.model.as_ref() else {
            continue;
        };
        let key = path.to_ascii_lowercase();
        if !std::path::Path::new(&key)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("spr"))
        {
            continue;
        }
        let slot = if let Some(slot) = by_path.get(&key) {
            *slot
        } else {
            let slot = if assets.len() >= MAX_SPRITE_ASSETS {
                None
            } else {
                source
                    .read(&key)
                    .and_then(|bytes| ohl_world::SpriteAsset::build(&bytes, &sprite_limits).ok())
                    .map(|asset| {
                        assets.push(asset);
                        assets.len() - 1
                    })
            };
            if slot.is_none() {
                missing += 1;
            }
            by_path.insert(key, slot);
            slot
        };
        if let Some(sprite) = slot {
            placements.push(SpritePlacement {
                sprite,
                origin: def.origin,
                scale: def
                    .keyvalues
                    .get("scale")
                    .and_then(|value| value.trim().parse::<f32>().ok())
                    .filter(|scale| scale.is_finite() && *scale > 0.0)
                    .unwrap_or(1.0),
                render: def.render,
            });
        }
    }

    (assets, placements, missing)
}

/// The angular velocity (radians per second about the *signed* `axis`) a
/// brush posed at `previous` degrees last step and `current` degrees this
/// step is turning at.
///
/// The difference is taken as the shortest signed arc, so a
/// `func_rotating`'s own angle wrapping from just under 360 back to just
/// over 0 (`ohl_game::logic::Simulation::advance_rotators` wraps it into
/// `0.0..360.0` so it cannot grow without bound) reports the small positive
/// rate it actually turned at rather than a full backwards revolution. A
/// brush seen for the first time, a non-positive or non-finite `dt`, and a
/// zero axis all report no rotation, matching how `Level::brush_velocity`
/// treats the same cases for a translating mover.
fn angular_velocity(axis: Vec3, previous: Option<f32>, current: f32, dt: f32) -> Vec3 {
    let Some(previous) = previous else {
        return Vec3::ZERO;
    };
    if !dt.is_finite() || dt <= 0.0 || !previous.is_finite() || !current.is_finite() {
        return Vec3::ZERO;
    }
    let delta = (current - previous + 180.0).rem_euclid(360.0) - 180.0;
    let rate = delta.to_radians() / dt;
    if rate.is_finite() {
        axis.normalize_or_zero() * rate
    } else {
        Vec3::ZERO
    }
}

/// One attached brush entity's live rotation, recorded by
/// [`Level::sync_brush_collision`] for [`Level::brush_rotation`].
///
/// The pose itself is set on the collision model by
/// [`ohl_physics::CollisionModel::set_brush_pose`]; this is the *rate* that
/// pose is changing at, which is what a rider needs and what a pose alone
/// cannot answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushRotation {
    /// The world-space point the brush rotates about — the same pivot
    /// `set_brush_pose` was given, in world space (a rotating mover's
    /// submodel geometry is compiled relative to its origin brush, so that
    /// is its `origin` keyvalue; see `docs/FORMAT_SOURCES.md` item 24).
    pub pivot: Vec3,
    /// Radians per second about the signed rotation axis: the angle this
    /// brush turned through since the previous step, divided by that
    /// step's `dt`. Zero for a brush whose angle did not change.
    pub angular_velocity: Vec3,
    /// The angle (degrees, about the signed axis) this brush was posed at
    /// when the entry was written, so the next step can difference against
    /// it. Kept here rather than in a second map so the two can never fall
    /// out of step.
    pub angle_degrees: f32,
}

/// Everything one map contributes to the running game.
pub struct Level {
    /// The map's own name (`c0a0`), as the host asked for it.
    pub name: String,
    /// The worldspawn geometry.
    pub world: WorldModel,
    /// Brush-entity submodels, keyed by their `*N` index.
    pub submodels: BTreeMap<u32, WorldModel>,
    /// The entity registry.
    pub registry: Registry,
    /// The map-logic simulation driving this level's entities.
    pub simulation: Simulation,
    /// Collision hulls, when the map has usable ones. Carries the world
    /// hulls *and* every solid brush entity's, kept at its current position
    /// by [`Self::sync_brush_collision`].
    pub collision: Option<CollisionModel>,
    /// Which attached brush hull belongs to which brush entity, so a mover
    /// can be followed as the map logic advances it. Empty when the map has
    /// no usable collision hulls.
    pub brush_collision: Vec<(Entity, BrushId)>,
    /// A second collision model, built and kept in step alongside
    /// [`Self::collision`], for monster navigation/sensing/movement
    /// instead of the player: identical except `func_monsterclip` is
    /// attached as solid here (see `ohl_game::brush::
    /// monster_solid_model_instances`, `docs/FORMAT_SOURCES.md` item 33).
    /// `ohl-engine`'s own player-move phase never reads this field, only
    /// [`Self::collision`]; `ai.rs` reads this one for every AI-side trace
    /// (sight, movement/steering, the attack hit-trace, and the static
    /// navigation graph build) instead of the player's.
    pub monster_collision: Option<CollisionModel>,
    /// As [`Self::brush_collision`], but for [`Self::monster_collision`].
    /// A superset of `brush_collision`'s entities (every `func_monsterclip`
    /// entity is attached here and not there), kept in step by the same
    /// [`Self::sync_brush_collision`] call.
    pub monster_brush_collision: Vec<(Entity, BrushId)>,
    /// Each attached brush's velocity as of the last [`Self::sync_brush_collision`]
    /// call: this step's displacement (its new origin minus its previous
    /// one) divided by that step's `dt`. Read by the player-move phase and
    /// fed back in as [`ohl_physics::PlayerController::base_velocity`] when
    /// the player's ground is that brush, so standing on a moving
    /// `func_train`/`func_tracktrain`/`func_plat`/lift `func_door` carries
    /// the player along with it (see "Riding movers",
    /// `docs/FORMAT_SOURCES.md`). Missing an entry (or holding zero) means
    /// that brush did not move this step.
    pub brush_velocity: BTreeMap<BrushId, Vec3>,
    /// Each attached *rotating* brush's pivot and angular velocity as of
    /// the last [`Self::sync_brush_collision`] call. A rotating mover's
    /// own [`Self::brush_velocity`] entry is (and stays) zero — it never
    /// translates — so this is the only thing that can carry a player
    /// standing on a `func_rotating` disc or a swinging
    /// `func_door_rotating`: the player-move phase turns it into a
    /// per-point ride velocity with
    /// [`ohl_physics::rotational_ride_velocity`] and adds that to the same
    /// `ohl_physics::PlayerController::base_velocity` a translating mover
    /// already feeds. Missing an entry means that brush is not rotating.
    pub brush_rotation: BTreeMap<BrushId, BrushRotation>,
    /// Which attached brushes the player-move phase could not fully push
    /// the player clear of this step (a mover whose leading face is moving
    /// into the player faster than the bounded push trace can carry them
    /// out of its way). Refreshed every step; empty when nothing is
    /// blocked. Published as a signal for a mover's own state machine to
    /// react to; nothing yet consumes it to halt, reverse, or apply a
    /// door's `dmg` keyvalue to the player — see the `TODO(black-box)` on
    /// `ohl_physics::push_from_mover` — so today a blocked mover still
    /// finishes its planned move on schedule, just with the player pushed
    /// as far out of its way as the bounded trace allowed.
    pub movers_blocked: Vec<BrushId>,
    /// The `skyname` skybox, when the payload publishes its six faces.
    pub skybox: Option<SkyboxAsset>,
    /// Studio models referenced by this map's entities, in load order.
    pub studio_models: Vec<StudioModel>,
    /// Each loaded studio model's asset path (lower-cased), index-aligned
    /// with [`Self::studio_models`]. Lets `crate::projectiles` name an
    /// already-loaded model by its published GoldSrc filename (a rocket's,
    /// a bolt's, ...) without this crate loading any new asset.
    pub studio_model_paths: Vec<String>,
    /// Where each loaded studio model stands.
    pub props: Vec<PropPlacement>,
    /// How many referenced studio models were not published in the payload.
    pub missing_models: usize,
    /// Sprite assets referenced by this map's entities, in load order.
    pub sprite_assets: Vec<ohl_world::SpriteAsset>,
    /// Sprite entities this map places; see [`SpritePlacement`].
    pub sprites: Vec<SpritePlacement>,
    /// How many referenced sprites were not published in the payload.
    pub missing_sprites: usize,
    /// How many brush-entity submodels this map references but could not be
    /// built. Published, rather than silently dropped, so an entity that
    /// should be visible cannot disappear without a number to point at.
    pub unbuildable_submodels: usize,
    /// How many individual faces, summed across [`Self::world`] and every
    /// model in [`Self::submodels`], were dropped while building (see
    /// [`ohl_world::WorldModel::dropped_faces`]) rather than causing their
    /// whole model to fail. A non-zero count here is exactly the kind of
    /// occluder loss a fidelity pass could previously only spot by noticing
    /// a hole in a screenshot.
    pub dropped_faces: usize,
    /// The `info_player_start` this map spawns the player at.
    pub spawn: Option<PlayerSpawn>,
    /// How many `info_player_start` entities this map declares (see
    /// [`ohl_world::WorldModel::player_start_count`]). Greater than `1`
    /// means [`Self::spawn`] was chosen by a tie-break this project cannot
    /// currently justify against any public source beyond "first in
    /// entity-lump order" (see `ohl_world::spawn::find_player_start`'s doc
    /// comment) — published as a diagnostic so a wrong-framing report has a
    /// number to point at instead of only a screenshot.
    pub player_start_count: usize,
    /// How many of this map's quoted entity-lump strings were not valid
    /// UTF-8 and were therefore decoded byte-per-byte (see
    /// [`ohl_formats::bsp30::EntityLumpReport`]). An aggregate count only —
    /// safe to report, unlike the strings themselves. Non-zero means the
    /// map was authored on a legacy single-byte codepage; it is not an
    /// error, and the map's structure and ASCII vocabulary are unaffected.
    pub entity_lump_relaxed_strings: usize,
    /// The parsed entity lump, kept so the systems that spawn monsters,
    /// pickups and navigation seeds can read the same definitions the
    /// registry was built from.
    pub defs: Vec<EntityDef>,
    /// How many of [`Self::defs`] the *map itself* declared.
    ///
    /// Always the whole list on a fresh load. A level change materialises
    /// every carried entity the destination declares no counterpart for as
    /// one of this level's own entities, appending its definition here (see
    /// `crate::transition::materialise_carried`), so anything past this
    /// index arrived with the player rather than out of the entity lump —
    /// which is exactly the set a save has to write down, because reloading
    /// the map alone rebuilds only the lump's own entities.
    pub map_defs: usize,
    /// The single client entity, carrying [`PlayerTag`]. It is deliberately
    /// *not* in [`Registry::entities`]: that list is index-aligned with
    /// `defs`, and a save references entities by their index in it.
    pub player: Entity,
}

/// Attaches every solid brush entity's collision hulls to `model`, plus
/// every `func_ladder`/`func_water` entity's as a non-solid contents
/// volume, and reports which attached hull belongs to which entity.
///
/// The worldspawn hulls alone are not what a player walks on: the compiler
/// moves every brush entity into its own submodel, so a `func_wall` floor
/// slab or a closed `func_door` is missing from model 0 entirely. Without
/// this the player falls through any floor a mapper built as an entity.
/// Likewise a `func_ladder`/`func_water` submodel needs its *own* attach
/// call (`ohl_physics::CollisionModel::attach_contents_brush`, distinct
/// from the solid one) or a player standing inside one finds only whatever
/// the world tree alone reports there — see `docs/FORMAT_SOURCES.md`,
/// "Player systems", `func_ladder`/`func_water`.
///
/// Both kinds share one `Vec<(Entity, BrushId)>` and one
/// [`Level::sync_brush_collision`] pass: `BrushId` and
/// `CollisionModel::set_brush_origin` do not care which attach call
/// produced an id, so a `func_water` mover (documented as sharing
/// `func_door`'s move/trigger behaviour) is kept at its current origin
/// exactly as any other brush entity already is, with no separate code
/// path.
///
/// Either kind is attached at the placement its map logic *already* puts
/// it at, not at its raw `origin` keyvalue: the same
/// `crate::render::brush_offset` [`Level::sync_brush_collision`] applies
/// every step is applied once here too, symmetrically for both loops. A
/// `func_train`/`func_tracktrain` is placed on the first node of its path
/// at spawn (see `ohl_game::pose::track_train_transform`), so without this
/// its hull would spend the level's very first tick — the tick the
/// player's own spawn position is resolved against — back wherever the map
/// compiled it, leaving nothing under a player the map authored standing
/// inside it.
fn attach_brush_collision(
    model: &mut CollisionModel,
    bsp: &Bsp<'_>,
    limits: &BspLimits,
    registry: &Registry,
) -> Vec<(Entity, BrushId)> {
    attach_brush_collision_with(
        model,
        bsp,
        limits,
        registry,
        ohl_game::brush::solid_model_instances,
    )
}

/// As [`attach_brush_collision`], but for [`Level::monster_collision`]:
/// identical except which brush entities count as *solid* is decided by
/// `ohl_game::brush::monster_solid_model_instances` instead of
/// `solid_model_instances` — the only classname the two disagree about is
/// `func_monsterclip` (`docs/FORMAT_SOURCES.md` item 33). Contents volumes
/// (`func_ladder`/`func_water`) are attached identically either way.
fn attach_monster_brush_collision(
    model: &mut CollisionModel,
    bsp: &Bsp<'_>,
    limits: &BspLimits,
    registry: &Registry,
) -> Vec<(Entity, BrushId)> {
    attach_brush_collision_with(
        model,
        bsp,
        limits,
        registry,
        ohl_game::brush::monster_solid_model_instances,
    )
}

fn attach_brush_collision_with(
    model: &mut CollisionModel,
    bsp: &Bsp<'_>,
    limits: &BspLimits,
    registry: &Registry,
    solid_instances: fn(&Registry) -> Vec<ohl_game::brush::ModelInstance>,
) -> Vec<(Entity, BrushId)> {
    let mut attached = Vec::new();
    for instance in solid_instances(registry) {
        let Ok(index) = usize::try_from(instance.model_index) else {
            continue;
        };
        if index == 0 {
            // `"*0"` is the worldspawn model, which `model` already holds.
            continue;
        }
        let origin = instance.origin + crate::render::brush_offset(registry, instance.entity);
        if let Ok(id) = model.attach_brush(bsp, limits, index, origin) {
            let (axis, angle_degrees, pivot) =
                crate::render::brush_pose_rotation(registry, instance.entity);
            if axis != Vec3::ZERO {
                // A rotating mover's compiled geometry is stored relative
                // to its own origin brush, exactly like a `func_train`'s
                // (`ohl_game::pose::track_train_transform`'s own doc
                // comment: "the compiler writes that origin brush's
                // position into the entity's `origin` keyvalue and stores
                // the submodel's geometry relative to it") — so the
                // pivot to rotate about is the *local* origin the geometry
                // is already centred on, `Vec3::ZERO`, not the world-space
                // `origin` keyvalue `attach_brush` already translated by
                // above. A world-baked `func_tracktrain` is the one case
                // that is not centred on its own turning point and reports
                // a non-zero compiled-frame pivot instead
                // (`ohl_game::pose::track_train_pivot`).
                model.set_brush_pose(id, origin, pivot, axis, angle_degrees);
            }
            attached.push((instance.entity, id));
        }
    }
    for (instance, kind) in ohl_game::brush::contents_model_instances(registry) {
        let Ok(index) = usize::try_from(instance.model_index) else {
            continue;
        };
        if index == 0 {
            continue;
        }
        let kind = match kind {
            ohl_game::brush::ContentsVolumeKind::Ladder => ContentsKind::Ladder,
            ohl_game::brush::ContentsVolumeKind::Water(liquid) => match liquid {
                ohl_game::Liquid::Water => ContentsKind::Water,
                ohl_game::Liquid::Slime => ContentsKind::Slime,
                ohl_game::Liquid::Lava => ContentsKind::Lava,
            },
        };
        let origin = instance.origin + crate::render::brush_offset(registry, instance.entity);
        if let Ok(id) = model.attach_contents_brush(bsp, limits, index, origin, kind) {
            attached.push((instance.entity, id));
        }
    }
    attached
}

/// The classname the engine's own player entity carries. Project-authored:
/// the entity has no definition in any map's entity lump.
pub const PLAYER_CLASSNAME: &str = "player";

/// The player's published maximum health (`docs/FORMAT_SOURCES.md`,
/// "Combat and damage").
pub const PLAYER_MAX_HEALTH: f32 = 100.0;

/// The player's published maximum armour, i.e. a full HEV suit.
pub const PLAYER_MAX_ARMOR: f32 = 100.0;

/// Spawns the client entity, so monsters can see, target and shoot the
/// player through the same components they use for each other, and so the
/// player has an id an attack trace can be told to ignore.
///
/// The suit starts empty: armour is picked up, not spawned with.
fn spawn_player(registry: &mut Registry, spawn: Option<PlayerSpawn>) -> Entity {
    let (origin, yaw, pitch) = spawn.map_or((Vec3::ZERO, 0.0, 0.0), |spawn| {
        (Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
    });
    registry.world.spawn((
        PlayerTag,
        ClassName(PLAYER_CLASSNAME.to_string()),
        Transform {
            origin,
            angles: Vec3::new(pitch, yaw, 0.0),
        },
        ohl_combat::Health::new(PLAYER_MAX_HEALTH),
        ohl_combat::Armor::empty(PLAYER_MAX_ARMOR),
    ))
}

impl Level {
    /// Loads `map` (a bare name such as `c0a0`) through `source`, applying
    /// the documented [`LightRamp`] defaults.
    ///
    /// # Errors
    /// [`EngineError::MapNotFound`] when the payload has no such map,
    /// [`EngineError::MapUnreadable`] when its bytes do not parse, and
    /// [`EngineError::WorldUnbuildable`] when the parsed map cannot be
    /// turned into renderable geometry.
    pub fn load(source: &dyn AssetSource, map: &str) -> Result<Self> {
        Self::load_with_ramp(source, map, LightRamp::default())
    }

    /// As [`Self::load`], with a caller-chosen [`LightRamp`] (for example
    /// one with a non-default `overbright`; see `--overbright` in
    /// `ohl-app`).
    ///
    /// # Errors
    /// As [`Self::load`].
    pub fn load_with_ramp(source: &dyn AssetSource, map: &str, ramp: LightRamp) -> Result<Self> {
        let bytes = source
            .read(&format!("maps/{map}.bsp"))
            .ok_or(EngineError::MapNotFound)?;
        Self::from_bytes_with_ramp(source, map, &bytes, ramp)
    }

    /// Builds a level from map bytes the caller already holds, applying the
    /// documented [`LightRamp`] defaults.
    ///
    /// # Errors
    /// As [`Self::load`], minus [`EngineError::MapNotFound`].
    pub fn from_bytes(source: &dyn AssetSource, map: &str, bytes: &[u8]) -> Result<Self> {
        Self::from_bytes_with_ramp(source, map, bytes, LightRamp::default())
    }

    /// As [`Self::from_bytes`], with a caller-chosen [`LightRamp`].
    ///
    /// # Errors
    /// As [`Self::from_bytes`].
    #[allow(
        clippy::too_many_lines,
        reason = "one field per Self{} literal member; splitting the constructor \
                  would only add indirection"
    )]
    pub fn from_bytes_with_ramp(
        source: &dyn AssetSource,
        map: &str,
        bytes: &[u8],
        ramp: LightRamp,
    ) -> Result<Self> {
        let limits = BspLimits::default();
        let bsp = Bsp::parse(bytes, &limits).map_err(|_| EngineError::MapUnreadable)?;
        // Never `unwrap_or_default()`: an entities lump that fails to
        // parse used to leave the level with an empty entity list, which
        // loads as a silent empty room — no player start, no monsters, no
        // triggers, and a player spawned at the origin, usually inside
        // solid geometry — while every caller still saw a successful load.
        // A lump this build cannot read is a load failure.
        let (entities, entity_lump) = bsp
            .entities_with_report(&limits)
            .map_err(|_| EngineError::EntityLumpUnreadable)?;
        let kv_limits = KeyvalueLimits::default();
        let defs = keyvalues::parse_entities(&entities, &kv_limits);

        let wad_value = defs
            .first()
            .and_then(|worldspawn| worldspawn.keyvalues.get("wad"))
            .cloned()
            .unwrap_or_default();
        let wad_bytes = source.resolve_wads(&wad_value);
        let wad_slices: Vec<&[u8]> = wad_bytes.iter().map(Vec::as_slice).collect();
        let options = WorldBuildOptions {
            wads: &wad_slices,
            limits,
            ramp,
        };

        let world = WorldModel::build(&bsp, &options).map_err(|_| EngineError::WorldUnbuildable)?;

        let model_bounds = submodel_bounds(&bsp, &limits);
        let mut registry = Registry::build(&defs, &model_bounds, &kv_limits);

        // Only the submodels an entity actually references are built: a map
        // publishes one per brush entity and nothing else draws them.
        let mut submodels = BTreeMap::new();
        // A submodel that will not build used to be dropped silently, so a
        // brush entity that should be on screen could vanish with no
        // diagnostic at all. Count them instead and publish the count.
        let mut unbuildable_submodels = 0usize;
        for instance in ohl_game::brush::model_instances(&registry) {
            let index = instance.model_index;
            if index == 0 || submodels.contains_key(&index) {
                continue;
            }
            let Ok(index_usize) = usize::try_from(index) else {
                unbuildable_submodels += 1;
                continue;
            };
            match WorldModel::build_submodel(&bsp, &options, index_usize) {
                Ok(model) => {
                    submodels.insert(index, model);
                }
                Err(_) => unbuildable_submodels += 1,
            }
        }

        let mut collision = CollisionModel::from_bsp(&bsp, &limits).ok();
        let brush_collision = collision
            .as_mut()
            .map(|model| attach_brush_collision(model, &bsp, &limits, &registry))
            .unwrap_or_default();
        let mut monster_collision = CollisionModel::from_bsp(&bsp, &limits).ok();
        let monster_brush_collision = monster_collision
            .as_mut()
            .map(|model| attach_monster_brush_collision(model, &bsp, &limits, &registry))
            .unwrap_or_default();
        let skybox = registry
            .worldspawn
            .as_ref()
            .map(|worldspawn| worldspawn.skyname.as_str())
            .filter(|name| !name.is_empty())
            .and_then(|name| load_skybox(source, name));
        let studio = load_studio_models(source, &defs);
        let (sprite_assets, sprites, missing_sprites) = load_sprites(source, &defs);

        // Every prop placement becomes a drawable entity: the renderer
        // sources its studio instances from the registry, so a monster the
        // AI moves and a static prop the map placed take the same path.
        // `Registry::entities` is index-aligned with `defs`, which is what
        // lets a placement find the entity its definition produced.
        for (def_index, prop) in studio.def_indices.iter().zip(&studio.props) {
            let Some(entity) = registry.entities.get(*def_index) else {
                continue;
            };
            registry
                .world
                .insert_one(
                    *entity,
                    StudioAnim {
                        model: prop.model,
                        sequence: prop.sequence,
                        cycle: prop.cycle,
                        frame_rate: 1.0,
                        body: prop.body,
                        skin: prop.skin,
                    },
                )
                .ok();
        }
        let player = spawn_player(&mut registry, world.spawn);
        let dropped_faces = world.dropped_faces
            + submodels
                .values()
                .map(|model| model.dropped_faces)
                .sum::<usize>();

        Ok(Self {
            name: map.to_string(),
            spawn: world.spawn,
            player_start_count: world.player_start_count,
            entity_lump_relaxed_strings: entity_lump.relaxed_strings,
            world,
            submodels,
            registry,
            simulation: Simulation::new(),
            collision,
            brush_collision,
            monster_collision,
            monster_brush_collision,
            brush_velocity: BTreeMap::new(),
            brush_rotation: BTreeMap::new(),
            movers_blocked: Vec::new(),
            skybox,
            studio_models: studio.models,
            studio_model_paths: studio.paths,
            props: studio.props,
            missing_models: studio.missing,
            sprite_assets,
            sprites,
            missing_sprites,
            unbuildable_submodels,
            dropped_faces,
            map_defs: defs.len(),
            defs,
            player,
        })
    }

    /// Loads the studio models `self.defs[start..]` reference and attaches
    /// a [`StudioAnim`] to the registry entity at each matching def index —
    /// the same load-and-attach pass [`Self::from_bytes_with_ramp`] already
    /// ran for this map's own defs (`load_studio_models`, plus the loop
    /// that zips its `def_indices` against `props`), run again for
    /// whatever a level change or a save load appended *after* that pass
    /// already finished.
    ///
    /// A carried entity gets an `Actor`, a brain and its scripts from
    /// `Systems::attach_level` because that stage re-runs over the whole,
    /// now-extended `self.defs`/`Registry::entities` every time. The
    /// studio-model pass does not: it only ever ran once, during the
    /// initial `Level::load_with_ramp`/`from_bytes_with_ramp`, strictly
    /// *before* `crate::transition::TransitionState::apply` ->
    /// `crate::transition::materialise_carried` (a level change) or
    /// `crate::save_state::restore_carried_entities` (a tag-36 save
    /// restore) can append anything past `self.map_defs`. Without this
    /// second pass a carried monster is simulated (walking, scripted,
    /// alive) but never drawn: `render.rs`'s `collect_studio_instances`
    /// only enumerates entities carrying a [`StudioAnim`], and nothing
    /// after the initial load ever attached one to an entity created past
    /// that point.
    ///
    /// Callers pass `self.map_defs` as `start`: that field is set once, at
    /// construction, to the map's own def count, and `materialise_carried`
    /// never changes it — so it always names exactly the boundary between
    /// the destination's own defs (already covered by the initial pass)
    /// and whatever arrived with the player (not yet covered). Reuses
    /// whatever [`Self::studio_models`] the initial pass already loaded
    /// (by lower-cased asset path), so a carried monster that shares its
    /// species' model with something the destination map already places
    /// does not load a second copy.
    pub(crate) fn attach_studio_models(&mut self, source: &dyn AssetSource, start: usize) {
        if start >= self.defs.len() {
            return;
        }
        let mut by_path: BTreeMap<String, Option<usize>> = self
            .studio_model_paths
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, path)| (path, Some(index)))
            .collect();
        let (props, def_indices, missing) = load_studio_models_into(
            source,
            &self.defs[start..],
            start,
            &mut by_path,
            &mut self.studio_models,
            &mut self.studio_model_paths,
        );
        self.missing_models += missing;
        for (def_index, prop) in def_indices.iter().zip(&props) {
            let Some(entity) = self.registry.entities.get(*def_index) else {
                continue;
            };
            self.registry
                .world
                .insert_one(
                    *entity,
                    StudioAnim {
                        model: prop.model,
                        sequence: prop.sequence,
                        cycle: prop.cycle,
                        frame_rate: 1.0,
                        body: prop.body,
                        skin: prop.skin,
                    },
                )
                .ok();
        }
        self.props.extend(props);
    }

    /// Moves every attached brush hull to where its entity currently is,
    /// and records each one's velocity for [`Self::brush_velocity`].
    ///
    /// Call once per simulation step *before* the player moves, so a door
    /// or train blocks (and carries) at the position it is drawn at rather
    /// than at the position it was compiled at. `dt` is the step length
    /// used to turn this step's displacement into a velocity; a
    /// non-positive or non-finite `dt` reports zero velocity for every
    /// brush rather than dividing by it.
    ///
    /// An entry whose entity has since despawned (a `func_wall` removed by
    /// a scripted `killtarget`, for example — see `ai.rs`'s
    /// `finish_script_step`) is detached from the collision model instead
    /// of skipped: without that, a brush the map logic removed keeps
    /// blocking the player forever, since the collision model has no other
    /// way to learn an attached brush is gone. The stale entry is then
    /// dropped from `brush_collision` (and its velocity entry, if any) so
    /// later calls do not pay to look it up again. This is a single pass
    /// over `brush_collision` with no per-call allocation beyond the
    /// velocity map update: each entry already names its own `BrushId`
    /// (recorded once, at attach time), so there is no per-step name or
    /// entity search to do.
    pub fn sync_brush_collision(&mut self, dt: f32) {
        self.sync_monster_brush_collision();
        if self.brush_collision.is_empty() {
            return;
        }
        let Self {
            registry,
            collision,
            brush_collision,
            brush_velocity,
            brush_rotation,
            ..
        } = self;
        let Some(model) = collision.as_mut() else {
            return;
        };
        brush_collision.retain(|(entity, brush)| {
            // A despawned entity (a `killtarget`ed `func_wall`) *and* a
            // broken `func_breakable`/`func_pushable` are both gone from
            // the world as far as collision is concerned: a broken brush
            // keeps its entity (its state has to persist, see save tag 33)
            // but must stop blocking the player, exactly as
            // `ohl_game::brush::solid_model_instances` already stops
            // offering it for attachment (`docs/FORMAT_SOURCES.md`, item
            // 30).
            let broken = registry
                .world
                .get::<&ohl_game::registry::Breakable>(*entity)
                .is_ok_and(|breakable| breakable.broken);
            let transform = registry.world.get::<&Transform>(*entity);
            let (Ok(transform), false) = (transform, broken) else {
                model.detach_brush(*brush);
                brush_velocity.remove(brush);
                brush_rotation.remove(brush);
                return false;
            };
            let offset = crate::render::brush_offset(registry, *entity);
            let new_origin = transform.origin + offset;
            let displacement = new_origin - model.brush_origin(*brush);
            let velocity = if dt.is_finite() && dt > 0.0 && displacement.is_finite() {
                displacement / dt
            } else {
                Vec3::ZERO
            };
            // A rotating mover's own *translation* — zero for every
            // rotating mover except a `func_platrot`, which translates and
            // turns over the same trip — reports a velocity above like any
            // other brush; the rotational half of its motion, the only half
            // that can carry a rider standing on a `func_rotating`/
            // `func_door_rotating`, is recorded separately in
            // `brush_rotation` just below. A `func_platrot` rider is
            // carried by both halves of its trip, which is why
            // `Self::rotational_carry` takes this step's translation back
            // off the pivot before it turns them — that correction is what
            // lets the rigid step and the translation compose at all,
            // though for a platform turning a fraction of a degree per tick
            // the tangential velocity in `Self::brush_ride_velocity` would
            // land the rider in the same place on its own. The rigid step
            // earns its keep on a mover that turns through a large angle in
            // one tick; see `crates/ohl-engine/tests/platrot_rider.rs`,
            // which measures the composed result rather than which of the
            // two delivered it.
            brush_velocity.insert(*brush, velocity);
            let (axis, angle_degrees, pivot) =
                crate::render::brush_pose_rotation(registry, *entity);
            if axis == Vec3::ZERO {
                brush_rotation.remove(brush);
                model.set_brush_origin(*brush, new_origin);
            } else {
                let previous = brush_rotation
                    .get(brush)
                    .map(|rotation| rotation.angle_degrees);
                let angular_velocity = angular_velocity(axis, previous, angle_degrees, dt);
                brush_rotation.insert(
                    *brush,
                    BrushRotation {
                        // The pivot handed to `set_brush_pose` below is in
                        // the submodel's own compiled frame, which
                        // `new_origin` then translates: in world space it
                        // is `new_origin + pivot` (and so `new_origin`
                        // itself for every mover compiled around its own
                        // turning point).
                        pivot: new_origin + pivot,
                        angular_velocity,
                        angle_degrees,
                    },
                );
                // See `attach_brush_collision`'s matching branch: the pivot
                // to rotate about is in the compiled frame the geometry is
                // stored in, not the world-space translation `new_origin`
                // already carries. This is the same axis/angle/pivot triple
                // `crate::render::draw_brush_entities` draws the entity
                // with, so a `func_tracktrain` through a bend collides
                // where it is drawn, and the whole per-step yaw change
                // lands as a single pose rather than as a sequence the
                // player could be scraped along.
                model.set_brush_pose(*brush, new_origin, pivot, axis, angle_degrees);
            }
            true
        });
    }

    /// [`Self::monster_collision`]'s own position/pose sync, called from
    /// [`Self::sync_brush_collision`]. Position-only, unlike the player
    /// pass above: nothing reads a velocity or rotation off
    /// [`Self::monster_collision`] today (AI does not ride movers), so
    /// there is no `brush_velocity`/`brush_rotation`-equivalent map to
    /// maintain for it.
    fn sync_monster_brush_collision(&mut self) {
        if self.monster_brush_collision.is_empty() {
            return;
        }
        let Self {
            registry,
            monster_collision,
            monster_brush_collision,
            ..
        } = self;
        let Some(model) = monster_collision.as_mut() else {
            return;
        };
        monster_brush_collision.retain(|(entity, brush)| {
            let Ok(transform) = registry.world.get::<&Transform>(*entity) else {
                model.detach_brush(*brush);
                return false;
            };
            let new_origin = transform.origin + crate::render::brush_offset(registry, *entity);
            let (axis, angle_degrees, pivot) =
                crate::render::brush_pose_rotation(registry, *entity);
            if axis == Vec3::ZERO {
                model.set_brush_origin(*brush, new_origin);
            } else {
                model.set_brush_pose(*brush, new_origin, pivot, axis, angle_degrees);
            }
            true
        });
    }

    /// How fast an attached brush entity is moving *at the world-space
    /// point* `point`: its whole-body translation
    /// ([`Self::brush_velocity`]) plus, for a rotating mover, the
    /// tangential velocity its current spin gives that particular point
    /// ([`Self::brush_rotation`], through
    /// [`ohl_physics::rotational_ride_velocity`]).
    ///
    /// This is what "riding a mover" means for a rotating brush: every
    /// point of a translating `func_train`/`func_plat` moves alike, so its
    /// velocity alone is the ride, but a `func_rotating` disc carries a
    /// player standing near its rim far faster than one standing on its
    /// axis, and a swinging `func_door_rotating` sweeps its outer edge
    /// fastest of all. Zero for a brush this level has never synced, one
    /// that is not moving, and one whose id has been detached.
    #[must_use]
    pub fn brush_ride_velocity(&self, brush: BrushId, point: Vec3) -> Vec3 {
        let translation = self
            .brush_velocity
            .get(&brush)
            .copied()
            .unwrap_or(Vec3::ZERO);
        let rotation = self
            .brush_rotation
            .get(&brush)
            .map_or(Vec3::ZERO, |rotation| {
                ohl_physics::rotational_ride_velocity(
                    rotation.pivot,
                    rotation.angular_velocity,
                    point,
                )
            });
        translation + rotation
    }

    /// Where a rider standing at `point` on the attached brush `brush` is
    /// carried to by whatever *rotation* that brush went through in the
    /// step [`Self::sync_brush_collision`] just applied, or `None` when
    /// `brush` did not turn at all (every translating mover, and a
    /// rotating one that is currently still).
    ///
    /// This is the rotational half of "riding a mover" applied as a finite
    /// rigid step rather than as a velocity: a brush whose heading is the
    /// direction of the path segment it is on — a `func_tracktrain` — turns
    /// through the entire angle between two segments in the single step it
    /// changes segment on, and no velocity integrated over that step can
    /// follow the arc the rider's seat travels (see
    /// [`ohl_physics::rotational_ride_step`]). The caller moves the rider
    /// here only after checking the destination is free, and then feeds
    /// only the brush's *translation* back in as `base_velocity`, so the
    /// ride is applied exactly once.
    ///
    /// The pivot used is the brush's pivot as it was *before* this step's
    /// translation (`brush_rotation`'s pivot less this step's own
    /// displacement), because that is the pose the rider's offset was
    /// measured against; the translation itself is then added by
    /// `base_velocity` in the ordinary way.
    #[must_use]
    pub fn rotational_carry(&self, brush: BrushId, point: Vec3, dt: f32) -> Option<Vec3> {
        let rotation = self.brush_rotation.get(&brush)?;
        let translation = self
            .brush_velocity
            .get(&brush)
            .copied()
            .unwrap_or(Vec3::ZERO)
            * dt;
        let pivot = rotation.pivot - translation;
        let carried =
            ohl_physics::rotational_ride_step(pivot, rotation.angular_velocity, dt, point);
        (carried != point).then_some(carried)
    }

    /// Whether this level declares any `info_landmark` at all.
    ///
    /// A map reached only through a `trigger_changelevel`/`info_landmark`
    /// pair legitimately declares no `info_player_start`: the player
    /// arrives relative to the landmark it came through, not at a spawn
    /// point (`docs/FORMAT_SOURCES.md`, the `trigger_changelevel`/
    /// `info_landmark` item). So "no player start" alone does not mean a
    /// map failed to load its entity world — "no player start *and* no
    /// landmark" is the combination that does.
    #[must_use]
    pub fn has_landmark(&self) -> bool {
        self.registry
            .world
            .query::<&Landmark>()
            .into_iter()
            .next()
            .is_some()
    }

    /// The world-space origin of the landmark named `landmark`, when this
    /// level declares one.
    #[must_use]
    pub fn landmark_origin(&self, landmark: &str) -> Option<Vec3> {
        for (_, name, transform) in &mut self
            .registry
            .world
            .query::<(&Landmark, &TargetName, &Transform)>()
        {
            if name.0 == landmark {
                return Some(transform.origin);
            }
        }
        None
    }
}

/// Reads every brush submodel's bounding box, which the registry needs to
/// derive door and platform travel distances.
fn submodel_bounds(bsp: &Bsp<'_>, limits: &BspLimits) -> BTreeMap<u32, ([f32; 3], [f32; 3])> {
    let mut bounds = BTreeMap::new();
    let Ok(models) = bsp.models(limits) else {
        return bounds;
    };
    for (index, submodel) in models.iter().enumerate() {
        let Ok(index) = u32::try_from(index) else {
            continue;
        };
        bounds.insert(
            index,
            (
                [
                    submodel.mins[0].get(),
                    submodel.mins[1].get(),
                    submodel.mins[2].get(),
                ],
                [
                    submodel.maxs[0].get(),
                    submodel.maxs[1].get(),
                    submodel.maxs[2].get(),
                ],
            ),
        );
    }
    bounds
}

/// Loads the six `gfx/env/<name><suffix>.tga` skybox faces, or `None` when
/// any of them is missing or does not decode.
fn load_skybox(source: &dyn AssetSource, skyname: &str) -> Option<SkyboxAsset> {
    let mut faces = Vec::with_capacity(SKY_FACE_SUFFIXES.len());
    for suffix in SKY_FACE_SUFFIXES {
        faces.push(source.read(&format!("gfx/env/{skyname}{suffix}.tga"))?);
    }
    let borrowed: [&[u8]; 6] = [
        faces[0].as_slice(),
        faces[1].as_slice(),
        faces[2].as_slice(),
        faces[3].as_slice(),
        faces[4].as_slice(),
        faces[5].as_slice(),
    ];
    SkyboxAsset::build(borrowed).ok()
}

/// What [`load_studio_models`] resolved out of one map's entity lump.
struct StudioLoad {
    /// The distinct models that loaded, in load order.
    models: Vec<StudioModel>,
    /// Each loaded model's asset path (lower-cased), index-aligned with
    /// `models`. Kept alongside the models themselves so a later lookup
    /// (`crate::projectiles`' per-`ProjectileKind` default model paths) can
    /// name an already-loaded model by its published GoldSrc filename
    /// instead of duplicating the load.
    paths: Vec<String>,
    /// One placement per model-carrying entity definition.
    props: Vec<PropPlacement>,
    /// The index in `defs` each placement came from, so the placement can
    /// be attached to the registry entity that definition produced.
    def_indices: Vec<usize>,
    /// How many referenced models the payload does not publish.
    missing: usize,
}

/// The companion external-texture asset path for a `.mdl` asset path
/// (`models/zombie.mdl` -> `models/zombiet.mdl`), or `None` when `key`
/// does not end in `.mdl`.
///
/// GoldSrc studio models whose textures are stored separately (to let
/// several skins of the same body share one texture file) publish zero
/// embedded textures in the main file and instead ship a second file with
/// the same base name plus a trailing `t` (see `docs/FORMAT_SOURCES.md`,
/// "GoldSrc MDL v10 and SPR"). `key` is already lower-cased by the caller,
/// so the result is too.
fn external_texture_path(key: &str) -> Option<String> {
    let stem = key.strip_suffix(".mdl")?;
    Some(format!("{stem}t.mdl"))
}

/// Loads the studio models this map's monster and prop entities reference,
/// skipping (and counting) the ones the payload does not publish.
fn load_studio_models(source: &dyn AssetSource, defs: &[EntityDef]) -> StudioLoad {
    let mut by_path: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut models = Vec::new();
    let mut paths = Vec::new();
    let (props, def_indices, missing) =
        load_studio_models_into(source, defs, 0, &mut by_path, &mut models, &mut paths);
    StudioLoad {
        models,
        paths,
        props,
        def_indices,
        missing,
    }
}

/// The shared per-def resolution [`load_studio_models`] and
/// [`Level::attach_studio_models`] both run: `defs` is scanned starting at
/// `base_index` in the level's own `defs` list (so the returned
/// `def_indices` are always absolute, ready to zip against
/// `Registry::entities`), and any model not already cached in `by_path`
/// (keyed by its lower-cased asset path) is loaded and appended to
/// `models`/`paths`.
///
/// Splitting this out is what lets [`Level::attach_studio_models`] extend
/// the *same* `models`/`paths`/`by_path` cache a fresh map load already
/// built, instead of duplicating the resolution rules (the default-model
/// fallback, the `.mdl` extension check, [`MAX_STUDIO_MODELS`]) a second
/// time for whatever a level change or a tag-36 save restore appends past
/// the map's own defs.
fn load_studio_models_into(
    source: &dyn AssetSource,
    defs: &[EntityDef],
    base_index: usize,
    by_path: &mut BTreeMap<String, Option<usize>>,
    models: &mut Vec<StudioModel>,
    paths: &mut Vec<String>,
) -> (Vec<PropPlacement>, Vec<usize>, usize) {
    let studio_limits = StudioLimits::default();
    let mut props = Vec::new();
    let mut def_indices = Vec::new();
    let mut missing = 0usize;

    for (offset, def) in defs.iter().enumerate() {
        let def_index = base_index + offset;
        if !wants_studio_model(&def.classname) {
            continue;
        }
        // Most `monster_*` classnames carry no `model` keyvalue at all in
        // the map's entity lump: GoldSrc's own monster class hardcodes its
        // model in `Spawn`/`Precache`, not in map data (see
        // `wants_studio_model`'s doc comment and
        // `ohl_ai::MonsterKind::default_model_path`'s). A monster whose
        // entity has no explicit `model` keyvalue therefore falls back to
        // that table instead of being skipped outright, which is what lets
        // it draw at all rather than simulate invisibly.
        let path: std::borrow::Cow<'_, str> = match def.model.as_ref() {
            Some(ModelRef::Asset(path)) => std::borrow::Cow::Borrowed(path.as_str()),
            None if def.classname.starts_with("monster_") => {
                match ohl_ai::MonsterKind::from_classname(&def.classname).default_model_path() {
                    Some(default_path) => std::borrow::Cow::Borrowed(default_path),
                    None => continue,
                }
            }
            _ => continue,
        };
        let key = path.to_ascii_lowercase();
        if !std::path::Path::new(&key)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mdl"))
        {
            continue;
        }
        let slot = if let Some(slot) = by_path.get(&key) {
            *slot
        } else {
            let slot = if models.len() >= MAX_STUDIO_MODELS {
                None
            } else {
                source
                    .read(&key)
                    .and_then(|bytes| {
                        let texture_bytes =
                            external_texture_path(&key).and_then(|path| source.read(&path));
                        StudioModel::parse_with_external_texture(
                            &bytes,
                            texture_bytes.as_deref(),
                            &studio_limits,
                        )
                        .ok()
                    })
                    .map(|model| {
                        models.push(model);
                        paths.push(key.clone());
                        models.len() - 1
                    })
            };
            if slot.is_none() {
                missing += 1;
            }
            by_path.insert(key, slot);
            slot
        };
        if let Some(model) = slot {
            def_indices.push(def_index);
            props.push(PropPlacement {
                model,
                origin: def.origin,
                yaw: def.angles[1],
                sequence: def
                    .keyvalues
                    .get("sequence")
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0),
                body: def
                    .keyvalues
                    .get("body")
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .unwrap_or(0),
                skin: def
                    .keyvalues
                    .get("skin")
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0),
                cycle: 0.0,
            });
        }
    }

    (props, def_indices, missing)
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use ohl_formats::bsp30::{Bsp, Limits as BspLimits};
    use ohl_formats::test_support::{build_minimal_mdl10, build_minimal_spr};
    use ohl_game::keyvalues::{self, Limits as KeyvalueLimits};
    use ohl_game::registry::Registry;
    use ohl_physics::test_support::{build_ladder_entity_room_bsp, build_water_entity_room_bsp};
    use ohl_physics::{CollisionModel, contents};

    use super::{Level, angular_velocity, attach_brush_collision, attach_monster_brush_collision};
    use crate::assets::MemoryAssets;
    use crate::test_support::synthetic_map_bsp_with_extra_entity;

    /// Overwrites the single byte of `marker` inside `bytes` with `byte`,
    /// keeping every lump offset and length exactly as compiled (the
    /// entities lump's length is recorded in the BSP header, so a fixture
    /// must not grow or shrink it). Panics if the marker is not unique.
    fn patch_marker(bytes: &mut [u8], marker: &[u8], offset_in_marker: usize, byte: u8) {
        let positions: Vec<usize> = bytes
            .windows(marker.len())
            .enumerate()
            .filter(|(_, window)| *window == marker)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            positions.len(),
            1,
            "the marker must be unique in the fixture"
        );
        bytes[positions[0] + offset_in_marker] = byte;
    }

    /// The failure class this milestone found in a real campaign map: an
    /// otherwise well-formed entities lump carrying one byte in
    /// `0x80..=0xFF` inside a quoted value (a legacy single-byte codepage's
    /// punctuation). The whole map used to load as an empty room because
    /// the lump failed UTF-8 validation and `Level::load` swallowed the
    /// error; it must now load its entities, keep its player start, and
    /// merely report the relaxed string as a count.
    #[test]
    fn a_non_utf8_byte_in_a_quoted_value_still_loads_the_whole_entity_world() {
        let mut map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"monster_zombie\"\n\
             \"message\" \"aZb\"\n\"origin\" \"11 22 33\"\n}\n",
        );
        patch_marker(&mut map, b"\"aZb\"", 2, 0x92);

        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map)
            .expect("a non-UTF-8 byte inside a quoted value is not a load failure");

        assert_eq!(level.entity_lump_relaxed_strings, 1);
        assert!(
            level.defs.len() > 1,
            "the map's real entities loaded, not an empty room"
        );
        assert!(
            level.spawn.is_some(),
            "the player start survived the relaxed decode"
        );
        assert!(
            level
                .defs
                .iter()
                .any(|def| def.classname == "monster_zombie"),
            "the entity carrying the relaxed value is still present"
        );
    }

    /// A structurally broken entities lump must be a *load error*, not a
    /// silently empty world: before this, `bsp.entities(..).unwrap_or_default()`
    /// turned it into a map with no player start, no monsters and no
    /// triggers that every caller still counted as loaded.
    #[test]
    fn a_structurally_broken_entity_lump_is_a_load_error_not_an_empty_room() {
        let mut map = synthetic_map_bsp_with_extra_entity("next", "");
        // Break the grammar without changing the lump's length: the very
        // first `{` of the first block becomes an ordinary character.
        patch_marker(&mut map, b"{\n\"classname\" \"worldspawn\"", 0, b'x');

        let assets = MemoryAssets::new();
        let error = Level::from_bytes(&assets, "ohlsynth", &map)
            .err()
            .expect("an unreadable entities lump is a load failure");
        assert_eq!(error, crate::EngineError::EntityLumpUnreadable);
    }

    /// A map reached only through a `trigger_changelevel`/`info_landmark`
    /// pair legitimately declares no `info_player_start` of its own. It is
    /// still a fully loaded entity world, and `has_landmark` is what lets a
    /// caller tell it apart from a map that loaded nothing at all.
    #[test]
    fn a_map_with_a_landmark_and_no_player_start_still_reports_its_landmark() {
        let map = crate::test_support::synthetic_map_bsp_with_entities(
            "{\n\"classname\" \"worldspawn\"\n}\n\
             {\n\"classname\" \"info_landmark\"\n\
             \"targetname\" \"ohl_landmark\"\n\"origin\" \"0 0 8\"\n}\n",
        );
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");

        assert!(
            level.spawn.is_none(),
            "the fixture declares no player start"
        );
        assert!(level.has_landmark());
        assert!(level.defs.len() > 1);
    }

    /// The opposite case: a map with neither a player start nor a landmark
    /// has nothing for a caller to arrive at.
    #[test]
    fn a_map_with_neither_a_player_start_nor_a_landmark_reports_no_landmark() {
        let map = crate::test_support::synthetic_map_bsp_with_entities(
            "{\n\"classname\" \"worldspawn\"\n}\n",
        );
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");

        assert!(level.spawn.is_none());
        assert!(!level.has_landmark());
    }

    /// A healthy fixture reports no relaxed strings at all, so the count
    /// cannot quietly rise for every map.
    #[test]
    fn an_ordinary_ascii_entity_lump_reports_no_relaxed_strings() {
        let map = synthetic_map_bsp_with_extra_entity("next", "");
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");
        assert_eq!(level.entity_lump_relaxed_strings, 0);
        assert!(level.spawn.is_some());
    }

    /// A rotator advancing across the 359 degree -> 1 degree wrap
    /// (`Simulation::advance_rotators` keeps `angle_degrees` in `0.0..360.0`,
    /// so a `func_rotating` turning forward crosses this boundary exactly
    /// the way a real ride does) must report the small *positive* rate it
    /// actually turned at, not a near-full backwards revolution. Plain
    /// `current - previous` gives `1.0 - 359.0 == -358.0` degrees, i.e. a
    /// large negative rate turning the wrong way; the shortest-signed-arc
    /// differencing in `angular_velocity` must instead see this as `+2.0`
    /// degrees over the step. This is the gap PR #110's review flagged as
    /// "load-bearing but untested" (only a 12 s manual ride caught it,
    /// where the committed suite's 2.2 s engine test and the physics
    /// proptest's constant-`omega` fixture both stayed clear of the wrap).
    #[test]
    fn angular_velocity_reports_the_short_way_across_the_360_degree_wrap() {
        let axis = Vec3::Z;
        let dt = 1.0 / 30.0;
        let velocity = angular_velocity(axis, Some(359.0), 1.0, dt);

        // +2 degrees/step forward, not -358: a plain `current - previous`
        // would instead report a large *negative* rate (turning backwards).
        let expected_rate = 2.0_f32.to_radians() / dt;
        assert!(
            velocity.z > 0.0,
            "the wrap must report a small positive rate, got {velocity:?}"
        );
        assert!(
            (velocity.z - expected_rate).abs() < 1e-3,
            "expected the shortest +2 degree arc ({expected_rate} rad/s), got {velocity:?}"
        );

        // A plain difference would report roughly -358 degrees of rotation
        // over the step; make sure we are nowhere near that magnitude or
        // sign, in case some other bug produced a coincidentally-small
        // positive number.
        let plain_difference_rate = (1.0_f32 - 359.0).to_radians() / dt;
        assert!(
            (velocity.z - plain_difference_rate).abs() > 1.0,
            "must not match the plain-difference (non-wrapped) rate"
        );
    }

    /// `attach_brush_collision` is what turns a `func_ladder`/`func_water`
    /// entity in the registry into an attached, non-solid contents volume
    /// on the collision model — the glue this milestone adds between
    /// `ohl-game`'s classification (`Registry`/`ohl_game::brush::
    /// contents_model_instances`) and `ohl-physics`'s `ContentsKind`. This
    /// exercises it directly against `ohl-physics`'s own
    /// `build_ladder_entity_room_bsp`/`build_water_entity_room_bsp`
    /// fixtures (a submodel compiled the way a real brush entity actually
    /// compiles), reusing their entity text rather than `Level::from_bytes`
    /// so the test does not also depend on the fixture having renderable
    /// faces.
    fn attached_contents_model(bytes: &[u8]) -> (CollisionModel, usize) {
        let bsp_limits = BspLimits::default();
        let bsp = Bsp::parse(bytes, &bsp_limits).expect("fixture parses as BSP v30");
        let raw_entities = bsp.entities(&bsp_limits).unwrap_or_default();
        let kv_limits = KeyvalueLimits::default();
        let defs = keyvalues::parse_entities(&raw_entities, &kv_limits);
        let registry = Registry::build(&defs, &std::collections::BTreeMap::new(), &kv_limits);

        let mut model =
            CollisionModel::from_bsp(&bsp, &bsp_limits).expect("fixture has usable hulls");
        let attached = attach_brush_collision(&mut model, &bsp, &bsp_limits, &registry);
        (model, attached.len())
    }

    #[test]
    fn a_func_ladder_entity_is_attached_as_a_ladder_contents_volume() {
        let (model, count) = attached_contents_model(&build_ladder_entity_room_bsp());
        assert_eq!(count, 1, "the fixture's single func_ladder was attached");
        assert_eq!(
            model.point_contents(ohl_physics::Vec3::new(72.0, 0.0, 36.0)),
            contents::LADDER
        );
    }

    #[test]
    fn a_func_water_entity_is_attached_with_its_skin_keyvalue_liquid() {
        let (model, count) = attached_contents_model(&build_water_entity_room_bsp());
        assert_eq!(count, 1, "the fixture's single func_water was attached");
        assert_eq!(
            model.point_contents(ohl_physics::Vec3::new(0.0, 0.0, 100.0)),
            contents::WATER
        );
    }

    /// `attach_brush_collision`/`attach_monster_brush_collision` are the
    /// two functions `Level::load` uses to build [`Level::collision`] (the
    /// player's) and [`Level::monster_collision`] respectively (M9.10,
    /// `docs/FORMAT_SOURCES.md` item 33): a `func_monsterclip` submodel
    /// must be skipped by the former and attached by the latter, so the
    /// same brush is solid on one model and absent from the other.
    #[test]
    fn func_monsterclip_is_attached_only_to_the_monster_collision_model() {
        let bsp_limits = BspLimits::default();
        let bytes = ohl_formats::test_support::build_brush_entity_floor_bsp("func_monsterclip");
        let bsp = Bsp::parse(&bytes, &bsp_limits).expect("fixture parses as BSP v30");
        let raw_entities = bsp.entities(&bsp_limits).unwrap_or_default();
        let kv_limits = KeyvalueLimits::default();
        let defs = keyvalues::parse_entities(&raw_entities, &kv_limits);
        let registry = Registry::build(&defs, &std::collections::BTreeMap::new(), &kv_limits);

        let mut player_model =
            CollisionModel::from_bsp(&bsp, &bsp_limits).expect("fixture has usable hulls");
        let player_attached =
            attach_brush_collision(&mut player_model, &bsp, &bsp_limits, &registry);
        assert!(
            player_attached.is_empty(),
            "func_monsterclip must not be attached to the player's own collision model"
        );

        let mut monster_model =
            CollisionModel::from_bsp(&bsp, &bsp_limits).expect("fixture has usable hulls");
        let monster_attached =
            attach_monster_brush_collision(&mut monster_model, &bsp, &bsp_limits, &registry);
        assert_eq!(
            monster_attached.len(),
            1,
            "func_monsterclip must be attached to the monster collision model"
        );
    }

    /// A `monster_generic`-style entity (outside the old four-prefix
    /// allowlist) whose `model` keyvalue names a `.mdl` asset, plus
    /// `sequence`/`body`/`skin` keyvalues, must resolve to a placed studio
    /// instance carrying those exact values, and must not be counted as
    /// missing.
    #[test]
    fn prop_outside_legacy_prefixes_resolves_with_keyvalues() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"monster_generic\"\n\
             \"model\" \"models/ohl_prop.mdl\"\n\
             \"origin\" \"10 20 30\"\n\"angle\" \"45\"\n\
             \"sequence\" \"2\"\n\"body\" \"5\"\n\"skin\" \"1\"\n}\n",
        );

        let (mdl_bytes, _layout) = build_minimal_mdl10();
        let mut assets = MemoryAssets::new();
        assets.insert("maps/ohlsynth.bsp", map.clone());
        assets.insert("models/ohl_prop.mdl", mdl_bytes);

        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");

        assert_eq!(level.missing_models, 0);
        assert_eq!(level.studio_models.len(), 1);
        let prop = level
            .props
            .iter()
            .find(|prop| (prop.origin[0] - 10.0).abs() < f32::EPSILON)
            .expect("monster_generic prop placed");
        assert_eq!(prop.sequence, 2);
        assert_eq!(prop.body, 5);
        assert_eq!(prop.skin, 1);
        assert!((prop.yaw - 45.0).abs() < f32::EPSILON);
    }

    /// A `monster_*` entity with no `model` keyvalue at all (how GoldSrc's
    /// own map compiler emits every standard monster, since the model is
    /// hardcoded in the monster's own class rather than authored per map)
    /// must still resolve a studio prop, via
    /// `ohl_ai::MonsterKind::default_model_path`'s classname table, as long
    /// as the payload publishes that default path (fidelity F3).
    #[test]
    fn monster_with_no_model_keyvalue_resolves_via_the_default_model_table() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"monster_zombie\"\n\
             \"origin\" \"11 22 33\"\n\"angle\" \"90\"\n}\n",
        );
        let (mdl_bytes, _layout) = build_minimal_mdl10();
        let mut assets = MemoryAssets::new();
        assets.insert("maps/ohlsynth.bsp", map.clone());
        assets.insert("models/zombie.mdl", mdl_bytes);

        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");

        assert_eq!(level.missing_models, 0);
        assert_eq!(level.studio_models.len(), 1);
        let prop = level
            .props
            .iter()
            .find(|prop| (prop.origin[0] - 11.0).abs() < f32::EPSILON)
            .expect("monster_zombie prop placed from its default model");
        assert!((prop.yaw - 90.0).abs() < f32::EPSILON);
    }

    /// The same missing-model-keyvalue monster, when the payload does not
    /// publish its default model path, must be counted missing rather than
    /// silently placed with no model.
    #[test]
    fn monster_with_no_model_keyvalue_and_no_payload_asset_counts_as_missing() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"monster_zombie\"\n\"origin\" \"0 0 0\"\n}\n",
        );
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");
        assert_eq!(level.studio_models.len(), 0);
        assert_eq!(level.props.len(), 0);
        assert_eq!(level.missing_models, 1);
    }

    /// A `monster_*` classname this project's table does not define at all
    /// (`ohl_ai::MonsterKind::Unknown`) has no default path to guess, so it
    /// is skipped exactly like before this fix — not counted missing,
    /// since there was never a candidate asset path to fail to resolve.
    #[test]
    fn monster_of_an_undefined_kind_with_no_model_keyvalue_is_not_counted_missing() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"monster_not_in_the_table\"\n\"origin\" \"0 0 0\"\n}\n",
        );
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");
        assert_eq!(level.studio_models.len(), 0);
        assert_eq!(level.props.len(), 0);
        assert_eq!(level.missing_models, 0);
    }

    /// A sprite-only classname (`env_sprite`) must not be loaded as a
    /// studio model even if it happened to carry a `.mdl` `model`
    /// keyvalue: sprites are placed through a separate path.
    #[test]
    fn sprite_only_classnames_are_not_studio_props() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"env_sprite\"\n\
             \"model\" \"sprites/ohl_glow.mdl\"\n\
             \"origin\" \"1 2 3\"\n}\n",
        );
        let assets = MemoryAssets::new();
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");
        assert_eq!(level.studio_models.len(), 0);
        assert_eq!(level.props.len(), 0);
        assert_eq!(level.missing_models, 0);

        // The keyvalue names a `.mdl`, not a `.spr`, so it is not a
        // resolvable sprite either: nothing is placed and nothing is
        // counted missing, since GoldSrc itself would not resolve this.
        assert_eq!(level.sprites.len(), 0);
        assert_eq!(level.missing_sprites, 0);
    }

    /// `env_sprite`/`env_glow`/`cycler_sprite` entities whose `model`
    /// keyvalue resolves to a published `.spr` asset are collected with
    /// their render props and an explicit `scale`, regardless of whether a
    /// studio prop is also present in the map.
    #[test]
    fn sprite_entities_collected_with_render_props_and_scale() {
        let map = synthetic_map_bsp_with_extra_entity(
            "next",
            "{\n\"classname\" \"env_glow\"\n\
             \"model\" \"sprites/ohl_glow.spr\"\n\
             \"origin\" \"4 5 6\"\n\"scale\" \"2.5\"\n\
             \"rendermode\" \"5\"\n\"renderamt\" \"200\"\n\
             \"rendercolor\" \"10 20 30\"\n}\n\
             {\n\"classname\" \"cycler_sprite\"\n\
             \"model\" \"sprites/ohl_flare.spr\"\n\
             \"origin\" \"7 8 9\"\n}\n",
        );
        let mut assets = MemoryAssets::new();
        assets.insert("sprites/ohl_glow.spr", build_minimal_spr());
        assets.insert("sprites/ohl_flare.spr", build_minimal_spr());
        let level = Level::from_bytes(&assets, "ohlsynth", &map).expect("level loads");

        assert_eq!(level.missing_sprites, 0);
        assert_eq!(level.sprite_assets.len(), 2);
        assert_eq!(level.sprites.len(), 2);
        let glow = &level.sprites[0];
        assert!((glow.origin[0] - 4.0).abs() < f32::EPSILON);
        assert!((glow.origin[1] - 5.0).abs() < f32::EPSILON);
        assert!((glow.origin[2] - 6.0).abs() < f32::EPSILON);
        assert!((glow.scale - 2.5).abs() < f32::EPSILON);
        assert_eq!(glow.render.mode, 5);
        assert_eq!(glow.render.amt, 200);
        assert_eq!(glow.render.color, [10, 20, 30]);

        let cycler = &level.sprites[1];
        assert!((cycler.origin[0] - 7.0).abs() < f32::EPSILON);
        assert!((cycler.origin[1] - 8.0).abs() < f32::EPSILON);
        assert!((cycler.origin[2] - 9.0).abs() < f32::EPSILON);
        assert!((cycler.scale - 1.0).abs() < f32::EPSILON);
    }

    /// Applies a column-major [`crate::render`] placement matrix to a
    /// point, exactly as the renderer's own vertex shader would (see
    /// `ohl_render::math`'s module doc comment for the `m[column * 4 +
    /// row]` layout).
    fn apply_placement(matrix: &[f32; 16], point: ohl_physics::Vec3) -> ohl_physics::Vec3 {
        ohl_physics::Vec3::new(
            matrix[0] * point.x + matrix[4] * point.y + matrix[8] * point.z + matrix[12],
            matrix[1] * point.x + matrix[5] * point.y + matrix[9] * point.z + matrix[13],
            matrix[2] * point.x + matrix[6] * point.y + matrix[10] * point.z + matrix[14],
        )
    }

    /// Render and collision must agree on where a rotating brush's pose
    /// actually puts its geometry: `crate::render::rotated_placement`
    /// builds the matrix `draw_brush_entities` draws the submodel with,
    /// and `attach_brush_collision`/`Level::sync_brush_collision` set the
    /// exact same `(pivot, axis, angle_degrees)` on the attached collision
    /// brush via `ohl_physics::CollisionModel::set_brush_pose` — this
    /// pins that shared expression down concretely rather than trusting
    /// the two call sites to keep using the same three numbers by
    /// construction alone. A point taken from just inside the door leaf's
    /// own local shape, carried through `rotated_placement` into world
    /// space exactly as the renderer would place that vertex, must be
    /// reported solid by the collision model that
    /// `Level::sync_brush_collision` posed with the identical
    /// `crate::render::brush_pose_rotation` triple.
    /// Runs the render/collision pose-agreement check for one
    /// [`ohl_game::registry::MoverState`], forced directly (no ticking a
    /// whole `Simulation`) and compared against the angle
    /// `render::door_rotation_degrees` reports for that same state:
    /// `MoverState::Closed` (`fraction == 0.0`, angle `0.0`) is the spawn
    /// state of every `func_door_rotating` that does not start open, and
    /// is exactly the zero-angle path the blocking bug in
    /// `render::rotated_placement` used to drop the `origin` translation
    /// on; `MoverState::Open` (`fraction == 1.0`, the door's configured
    /// `distance`) is the terminal pose after a full open swing.
    fn assert_render_and_collision_agree_on_door_pose(
        state: ohl_game::registry::MoverState,
        angle_degrees: f32,
        leaf_should_be_solid_at_centerline: bool,
    ) {
        let bytes =
            crate::test_support::rotating_door_bsp(&crate::test_support::rotating_door_entities());
        let assets = MemoryAssets::new();
        let mut level = Level::from_bytes(&assets, "ohlrotdoorsynth", &bytes)
            .expect("the rotating-door fixture loads");

        let entity = *level
            .registry
            .find(crate::test_support::ROTATING_DOOR_NAME)
            .first()
            .expect("the fixture declares one named rotating door");
        {
            let mut door = level
                .registry
                .world
                .get::<&mut ohl_game::registry::Door>(entity)
                .expect("the named entity is a door");
            door.state = state;
            door.timer = 0.0;
        }
        level.sync_brush_collision(1.0 / 60.0);

        let pivot = ohl_physics::Vec3::new(
            crate::test_support::ROTATING_DOOR_PIVOT[0],
            crate::test_support::ROTATING_DOOR_PIVOT[1],
            crate::test_support::ROTATING_DOOR_PIVOT[2],
        );
        let axis = ohl_physics::Vec3::Z;
        // `origin` here (in the sense `rotated_placement` and
        // `set_brush_pose` both use it): the compiled submodel's own
        // local `(0, 0, 0)` sits at this world point (see
        // `rotating_door_bsp`'s doc comment), so it is what render's
        // rotate-then-translate composes with, unchanged from a plain
        // translating mover's `origin` keyvalue.
        let transform =
            crate::render::rotated_placement(pivot, ohl_physics::Vec3::ZERO, axis, angle_degrees);

        // A point just inside the door leaf's own compiled (local, pivot-
        // relative) shape — offset 2 units in from its pivot edge, well
        // inside every other face — carried into world space through the
        // render matrix exactly as the renderer would place that vertex.
        let local_point_inside_the_leaf = ohl_physics::Vec3::new(
            f32::midpoint(
                crate::test_support::ROTATING_DOOR_MINS[0],
                crate::test_support::ROTATING_DOOR_MAXS[0],
            ) - pivot.x,
            crate::test_support::ROTATING_DOOR_MINS[1] + 2.0 - pivot.y,
            f32::midpoint(
                crate::test_support::ROTATING_DOOR_MINS[2],
                crate::test_support::ROTATING_DOOR_MAXS[2],
            ) - pivot.z,
        );
        let world_point = apply_placement(&transform, local_point_inside_the_leaf);

        let model = level.collision.as_ref().expect("the fixture has collision");
        assert_eq!(
            model.point_contents(world_point),
            contents::SOLID,
            "collision does not agree the door's render pose at {angle_degrees} \
             degrees puts solid geometry at {world_point:?}"
        );

        // The corridor's own centreline: solid while the closed door still
        // blocks it (angle 0), empty once a full open swing has rotated
        // the leaf out of the way (angle 90).
        let centerline = ohl_physics::Vec3::new(pivot.x, 0.0, 40.0);
        let expected = if leaf_should_be_solid_at_centerline {
            contents::SOLID
        } else {
            contents::EMPTY
        };
        assert_eq!(model.point_contents(centerline), expected);
    }

    #[test]
    fn render_and_collision_agree_on_a_rotated_door_pose() {
        assert_render_and_collision_agree_on_door_pose(
            ohl_game::registry::MoverState::Open,
            90.0,
            false,
        );
    }

    /// The zero-angle counterpart of the above: this is the state every
    /// `func_door_rotating` spawns in unless it starts open, and it is
    /// exactly the path the blocking `rotated_placement` bug hit — an
    /// early return for `angle_degrees == 0.0` dropped the `origin`
    /// translation and drew/collided the closed door at world `(0, 0,
    /// 0)`. See the PR #107 review comment.
    #[test]
    fn render_and_collision_agree_on_a_closed_rotated_door_pose() {
        assert_render_and_collision_agree_on_door_pose(
            ohl_game::registry::MoverState::Closed,
            0.0,
            true,
        );
    }

    /// The same agreement, for the mover this milestone posed for the
    /// first time: a `func_tracktrain` turning to face its track.
    ///
    /// Checked at several progress values along a chain that turns a
    /// square corner — before the corner (drawn yaw 0), and after it
    /// (drawn yaw 90) — because a train is the one brush entity whose
    /// rotation *and* translation both change every step, so agreeing at
    /// one pose says nothing about agreeing at the next. Each check maps a
    /// point from the car's own compiled frame into world space through
    /// exactly the matrix `draw_brush_entities` draws the submodel with
    /// (`crate::render::rotated_placement`, fed from the same
    /// `ohl_game::pose::brush_pose_rotation`/`brush_offset` pair
    /// `sync_brush_collision` poses the hull from) and asserts the
    /// collision model agrees there is solid geometry at that world point.
    /// Before this milestone the collision hull was translated only, so
    /// every point taken from a *rotated* render pose landed outside it.
    #[test]
    fn render_and_collision_agree_on_a_turning_track_train_pose() {
        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{}.bsp", crate::test_support::BEND_TRAIN_MAP),
            crate::test_support::bending_track_train_bsp(),
        );
        let mut level = Level::load(&assets, crate::test_support::BEND_TRAIN_MAP)
            .expect("the bending-train fixture loads");

        let entity = level.registry.find(crate::test_support::BEND_TRAIN_NAME)[0];
        let step = 1.0 / 60.0;
        // The corner is 300 units out at 100 units/second, so three
        // seconds of stepping crosses it; `TrackTrainState::yaw_degrees`
        // then blends the heading change over
        // `ohl_game::track_train::DEFAULT_YAW_BLEND_DISTANCE` (256 units)
        // of the second segment, another 2.56 seconds, before it reads as
        // the second segment's own heading exactly — sample well past
        // that so the run really does see the fully-turned pose, not just
        // partway through the blend.
        let mut seen_yaws: Vec<f32> = Vec::new();
        for tick in 0..400 {
            level.simulation.tick(&mut level.registry, step);
            level.sync_brush_collision(step);
            if tick % 20 != 0 {
                continue;
            }

            let (axis, angle_degrees, pivot) =
                crate::render::brush_pose_rotation(&level.registry, entity);
            assert_eq!(
                axis,
                ohl_physics::Vec3::Z,
                "a `func_tracktrain` on a horizontal segment must report a yaw"
            );
            seen_yaws.push(angle_degrees);

            let authored = level
                .registry
                .world
                .get::<&ohl_game::registry::Transform>(entity)
                .expect("the fixture train has a transform")
                .origin;
            let origin = authored + crate::render::brush_offset(&level.registry, entity);
            let transform = crate::render::rotated_placement(origin, pivot, axis, angle_degrees);

            // The car's own compiled centre, and a point most of the way
            // along its length — the seat a passenger stands on, and the
            // point that moves furthest when the car turns.
            let model = level.collision.as_ref().expect("the fixture has collision");
            for local_x in [0.0, crate::test_support::BEND_SEAT_OFFSET_X] {
                let local = ohl_physics::Vec3::new(local_x, 0.0, 0.0);
                let world = apply_placement(&transform, local);
                assert_eq!(
                    model.point_contents(world),
                    contents::SOLID,
                    "collision does not agree the car's render pose at yaw \
                     {angle_degrees} puts solid geometry at {world:?}"
                );
            }
        }

        // The sampled run really did cross the corner: both the first
        // segment's pose and the second's were drawn. A car is posed a
        // `ohl_game::track_train::COMPILED_FACING_OFFSET_DEGREES` half
        // turn from the direction it travels, so the `+X` segment draws at
        // 180 and the `+Y` one at -90.
        assert!(
            seen_yaws.iter().any(|yaw| (yaw - 180.0).abs() < 1e-3),
            "the run never sampled the first (+X) segment: {seen_yaws:?}"
        );
        assert!(
            seen_yaws.iter().any(|yaw| (yaw + 90.0).abs() < 1e-3),
            "the run never sampled the second (+Y) segment: {seen_yaws:?}"
        );
    }
}
