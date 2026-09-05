//! One loaded map: geometry, entities, collision and the assets they name.

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_formats::bsp30::{Bsp, Limits as BspLimits};
use ohl_game::keyvalues::{self, EntityDef, Limits as KeyvalueLimits, ModelRef};
use ohl_game::registry::{Landmark, TargetName, Transform};
use ohl_game::{Registry, Simulation};
use ohl_physics::CollisionModel;
use ohl_world::{
    PlayerSpawn, SKY_FACE_SUFFIXES, SkyboxAsset, StudioLimits, StudioModel, WorldBuildOptions,
    WorldModel,
};

use crate::assets::AssetSource;
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
    /// Collision hulls, when the map has usable ones.
    pub collision: Option<CollisionModel>,
    /// The `skyname` skybox, when the payload publishes its six faces.
    pub skybox: Option<SkyboxAsset>,
    /// Studio models referenced by this map's entities, in load order.
    pub studio_models: Vec<StudioModel>,
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
    /// The `info_player_start` this map spawns the player at.
    pub spawn: Option<PlayerSpawn>,
}

impl Level {
    /// Loads `map` (a bare name such as `c0a0`) through `source`.
    ///
    /// # Errors
    /// [`EngineError::MapNotFound`] when the payload has no such map,
    /// [`EngineError::MapUnreadable`] when its bytes do not parse, and
    /// [`EngineError::WorldUnbuildable`] when the parsed map cannot be
    /// turned into renderable geometry.
    pub fn load(source: &dyn AssetSource, map: &str) -> Result<Self> {
        let bytes = source
            .read(&format!("maps/{map}.bsp"))
            .ok_or(EngineError::MapNotFound)?;
        Self::from_bytes(source, map, &bytes)
    }

    /// Builds a level from map bytes the caller already holds.
    ///
    /// # Errors
    /// As [`Self::load`], minus [`EngineError::MapNotFound`].
    pub fn from_bytes(source: &dyn AssetSource, map: &str, bytes: &[u8]) -> Result<Self> {
        let limits = BspLimits::default();
        let bsp = Bsp::parse(bytes, &limits).map_err(|_| EngineError::MapUnreadable)?;
        let entities = bsp.entities(&limits).unwrap_or_default();
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
        };

        let world = WorldModel::build(&bsp, &options).map_err(|_| EngineError::WorldUnbuildable)?;

        let model_bounds = submodel_bounds(&bsp, &limits);
        let registry = Registry::build(&defs, &model_bounds, &kv_limits);

        // Only the submodels an entity actually references are built: a map
        // publishes one per brush entity and nothing else draws them.
        let mut submodels = BTreeMap::new();
        for instance in ohl_game::brush::model_instances(&registry) {
            let index = instance.model_index;
            if index == 0 || submodels.contains_key(&index) {
                continue;
            }
            let Ok(index_usize) = usize::try_from(index) else {
                continue;
            };
            if let Ok(model) = WorldModel::build_submodel(&bsp, &options, index_usize) {
                submodels.insert(index, model);
            }
        }

        let collision = CollisionModel::from_bsp(&bsp, &limits).ok();
        let skybox = registry
            .worldspawn
            .as_ref()
            .map(|worldspawn| worldspawn.skyname.as_str())
            .filter(|name| !name.is_empty())
            .and_then(|name| load_skybox(source, name));
        let (studio_models, props, missing_models) = load_studio_models(source, &defs);
        let (sprite_assets, sprites, missing_sprites) = load_sprites(source, &defs);

        Ok(Self {
            name: map.to_string(),
            spawn: world.spawn,
            world,
            submodels,
            registry,
            simulation: Simulation::new(),
            collision,
            skybox,
            studio_models,
            props,
            missing_models,
            sprite_assets,
            sprites,
            missing_sprites,
        })
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

/// Loads the studio models this map's monster and prop entities reference,
/// skipping (and counting) the ones the payload does not publish.
fn load_studio_models(
    source: &dyn AssetSource,
    defs: &[EntityDef],
) -> (Vec<StudioModel>, Vec<PropPlacement>, usize) {
    let studio_limits = StudioLimits::default();
    let mut by_path: BTreeMap<String, Option<usize>> = BTreeMap::new();
    let mut models = Vec::new();
    let mut props = Vec::new();
    let mut missing = 0usize;

    for def in defs {
        if !wants_studio_model(&def.classname) {
            continue;
        }
        let Some(ModelRef::Asset(path)) = def.model.as_ref() else {
            continue;
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
                    .and_then(|bytes| StudioModel::parse(&bytes, &studio_limits).ok())
                    .map(|model| {
                        models.push(model);
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
            });
        }
    }

    (models, props, missing)
}

#[cfg(test)]
mod tests {
    use ohl_formats::test_support::{build_minimal_mdl10, build_minimal_spr};

    use super::Level;
    use crate::assets::MemoryAssets;
    use crate::test_support::synthetic_map_bsp_with_extra_entity;

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
}
