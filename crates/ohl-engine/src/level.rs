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

/// Classname prefixes whose `model` keyvalue names a studio model worth
/// placing in the world at this milestone.
const STUDIO_CLASS_PREFIXES: [&str; 4] = ["monster_", "cycler", "env_model", "prop_"];

/// One placed studio model: which loaded model to draw, and where.
#[derive(Debug, Clone, Copy)]
pub struct PropPlacement {
    /// Index into [`Level::studio_models`].
    pub model: usize,
    /// World-space origin.
    pub origin: [f32; 3],
    /// Yaw in degrees.
    pub yaw: f32,
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
        if !STUDIO_CLASS_PREFIXES
            .iter()
            .any(|prefix| def.classname.starts_with(prefix))
        {
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
            });
        }
    }

    (models, props, missing)
}
