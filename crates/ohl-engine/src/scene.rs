//! Owned brush/studio presentation, independent of simulation and network code.
//!
//! A catalog association belongs to one resource load/revision. Even identical
//! independent loads have different associations; handles, entity generations
//! and epochs are local identities, never original-protocol resource/entity IDs.
//! A future wire adapter must map those identities explicitly. Frames contain
//! no names, paths, ECS borrows, AI, random state, inventory or collision state.
//!
//! [`SceneFrame`] is an immutable owned candidate. [`SceneReplica::apply`]
//! validates the complete candidate before replacing its previous frame. Each
//! accepted frame replaces all supported records, so absent actors, held models,
//! hidden pickups and despawned entities disappear. Applying or drawing one
//! advances no simulation clock and invokes no authority systems.
//!
//! The shared CPU collectors use the engine's existing effective brush placement,
//! visibility/render properties and studio sampler. They never inspect GPU
//! availability. Sampled poses include current gait and held-model merging;
//! finite out-of-range sequence, body and skin selections retain the existing
//! sampler/renderer fallback. Presentation never resamples or advances them.
//!
//! This first slice excludes sprites, lightstyles, audio, transient effects,
//! recipient HUD/inventory, dynamic collision, resource/delta wire mapping and
//! prediction/reconciliation. It defines no serialization or gameplay protocol.
//! Automatic formatting and fixed errors redact identities, resources and poses;
//! explicit typed access to frame data is deliberate host-owned access.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use ohl_combat::EntityId;
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_render::{RenderProps, placement};
use ohl_world::StudioPose;

use crate::components::{HeldWeapon, StudioAnim, StudioGait};
use crate::ids::{entity_id, entity_of};
use crate::level::Level;

/// Project budgets, not original-engine or network limits.
pub const MAX_SCENE_INSTANCES: usize = 4096;
pub const MAX_SCENE_BONES: usize = 262_144;
pub const MAX_SCENE_BODY_SELECTIONS: usize = 128;

/// A host-selected lifetime for local presentation identities.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct SceneEpoch(u64);

impl SceneEpoch {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Generation-aware local identity, qualified by its host-owned epoch.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct SceneIdentity {
    epoch: SceneEpoch,
    local: EntityId,
}

impl SceneIdentity {
    pub fn from_entity(epoch: SceneEpoch, entity: Entity) -> Self {
        Self::from_local_id(epoch, entity_id(entity))
    }

    /// This is a local generational ID, never a directly decoded wire ID.
    pub const fn from_local_id(epoch: SceneEpoch, local: EntityId) -> Self {
        Self { epoch, local }
    }

    pub const fn epoch(self) -> SceneEpoch {
        self.epoch
    }

    pub const fn local_id(self) -> EntityId {
        self.local
    }
}

struct CatalogData {
    brushes: BTreeSet<u32>,
    studio_bones: Vec<usize>,
}

/// Immutable metadata and an opaque association to one loaded resource catalog.
#[derive(Clone)]
pub struct SceneCatalog(Arc<CatalogData>);

impl PartialEq for SceneCatalog {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SceneCatalog {}

impl SceneCatalog {
    pub(crate) fn from_level(level: &Level) -> Self {
        Self(Arc::new(CatalogData {
            brushes: level.submodels.keys().copied().collect(),
            studio_bones: level
                .studio_models
                .iter()
                .map(|model| model.bones.len())
                .collect(),
        }))
    }

    pub fn brush_handle(&self, local_index: u32) -> Result<BrushHandle, SceneError> {
        if !self.0.brushes.contains(&local_index) {
            return Err(SceneError::UnknownResource);
        }
        Ok(BrushHandle {
            catalog: self.clone(),
            local_index,
        })
    }

    pub fn studio_handle(&self, local_index: usize) -> Result<StudioHandle, SceneError> {
        if local_index >= self.0.studio_bones.len() {
            return Err(SceneError::UnknownResource);
        }
        Ok(StudioHandle {
            catalog: self.clone(),
            local_index,
        })
    }

    pub fn brush_handles(&self) -> impl Iterator<Item = BrushHandle> + '_ {
        self.0.brushes.iter().map(|&local_index| BrushHandle {
            catalog: self.clone(),
            local_index,
        })
    }

    pub fn studio_handles(&self) -> impl Iterator<Item = StudioHandle> + '_ {
        (0..self.0.studio_bones.len()).map(|local_index| StudioHandle {
            catalog: self.clone(),
            local_index,
        })
    }

    pub fn studio_bone_count(&self, handle: &StudioHandle) -> Result<usize, SceneError> {
        if self != &handle.catalog {
            return Err(SceneError::UnknownResource);
        }
        self.0
            .studio_bones
            .get(handle.local_index)
            .copied()
            .ok_or(SceneError::UnknownResource)
    }
}

/// Typed local resource handles include their exact catalog association.
#[derive(Clone, Eq, PartialEq)]
pub struct BrushHandle {
    catalog: SceneCatalog,
    local_index: u32,
}

impl BrushHandle {
    pub const fn local_index(&self) -> u32 {
        self.local_index
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct StudioHandle {
    catalog: SceneCatalog,
    local_index: usize,
}

impl StudioHandle {
    pub const fn local_index(&self) -> usize {
        self.local_index
    }
}

#[derive(Clone, PartialEq)]
pub struct BrushRecord {
    pub identity: SceneIdentity,
    pub model: BrushHandle,
    /// Effective column-major compiled-brush placement, including live movers.
    pub transform: [f32; 16],
    pub render: RenderProps,
}

/// Playback metadata accompanies the already resolved pose; it is not stepped.
#[derive(Clone, Copy, PartialEq)]
pub struct SceneAnimation {
    pub sequence: usize,
    pub cycle: f32,
    pub frame_rate: f32,
    pub gait: Option<SceneGait>,
}

#[derive(Clone, Copy, PartialEq)]
pub struct SceneGait {
    pub sequence: Option<usize>,
    pub cycle: f32,
    pub frame_rate: f32,
}

/// Held geometry inherits its owner's identity, transform, lighting and props.
#[derive(Clone, PartialEq)]
pub struct HeldModelRecord {
    pub model: StudioHandle,
    pub pose: StudioPose,
    pub body: Vec<u32>,
    pub skin: usize,
}

#[derive(Clone, PartialEq)]
pub struct StudioRecord {
    pub identity: SceneIdentity,
    pub model: StudioHandle,
    pub transform: [f32; 16],
    pub animation: SceneAnimation,
    pub pose: StudioPose,
    pub body: Vec<u32>,
    pub skin: usize,
    pub ambient: [f32; 3],
    pub render: RenderProps,
    pub held: Option<HeldModelRecord>,
}

/// Owned candidate records. Only immutable slices are exposed after creation.
#[derive(Clone, PartialEq)]
pub struct SceneFrame {
    catalog: SceneCatalog,
    epoch: SceneEpoch,
    tick: u64,
    time: f32,
    brushes: Vec<BrushRecord>,
    studios: Vec<StudioRecord>,
}

impl SceneFrame {
    /// Takes ownership of candidate records; validation happens at replica apply.
    pub fn new(
        catalog: SceneCatalog,
        epoch: SceneEpoch,
        tick: u64,
        time: f32,
        brushes: Vec<BrushRecord>,
        studios: Vec<StudioRecord>,
    ) -> Self {
        Self {
            catalog,
            epoch,
            tick,
            time,
            brushes,
            studios,
        }
    }

    pub fn catalog(&self) -> &SceneCatalog {
        &self.catalog
    }
    pub const fn epoch(&self) -> SceneEpoch {
        self.epoch
    }
    pub const fn tick(&self) -> u64 {
        self.tick
    }
    pub const fn time(&self) -> f32 {
        self.time
    }
    pub fn brushes(&self) -> &[BrushRecord] {
        &self.brushes
    }
    pub fn studios(&self) -> &[StudioRecord] {
        &self.studios
    }
}

/// A host may reduce, but never enlarge, these hard project budgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneLimits {
    pub max_instances: usize,
    pub max_bones: usize,
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self {
            max_instances: MAX_SCENE_INSTANCES,
            max_bones: MAX_SCENE_BONES,
        }
    }
}

/// Presentation-only full-frame replacement; this owns no authority systems.
pub struct SceneReplica {
    catalog: SceneCatalog,
    epoch: SceneEpoch,
    limits: SceneLimits,
    frame: Option<SceneFrame>,
}

impl SceneReplica {
    pub fn new(catalog: SceneCatalog, epoch: SceneEpoch) -> Self {
        Self {
            catalog,
            epoch,
            limits: SceneLimits::default(),
            frame: None,
        }
    }

    pub fn with_limits(
        catalog: SceneCatalog,
        epoch: SceneEpoch,
        limits: SceneLimits,
    ) -> Result<Self, SceneError> {
        if limits.max_instances > MAX_SCENE_INSTANCES || limits.max_bones > MAX_SCENE_BONES {
            return Err(SceneError::InvalidLimits);
        }
        Ok(Self {
            catalog,
            epoch,
            limits,
            frame: None,
        })
    }

    /// No state changes on any rejection. No tick wrapping or epoch fallback.
    pub fn apply(&mut self, frame: SceneFrame) -> Result<(), SceneError> {
        validate_frame(&frame, &self.catalog, self.epoch, self.limits)?;
        if let Some(previous) = &self.frame {
            if frame.tick <= previous.tick {
                return Err(SceneError::StaleTick);
            }
            if frame.time < previous.time {
                return Err(SceneError::StaleTime);
            }
        }
        self.frame = Some(frame);
        Ok(())
    }

    pub fn frame(&self) -> Option<&SceneFrame> {
        self.frame.as_ref()
    }
}

/// Fixed diagnostics never retain or format rejected IDs, handles or poses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneError {
    WrongCatalog,
    WrongEpoch,
    StaleTick,
    StaleTime,
    InvalidTime,
    InvalidIdentity,
    DuplicateIdentity,
    UnknownResource,
    NonFiniteState,
    PoseLayout,
    InstanceLimit,
    BoneLimit,
    BodyLimit,
    InvalidLimits,
    NoFrame,
    Renderer,
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WrongCatalog => "scene catalog association mismatch",
            Self::WrongEpoch => "scene epoch mismatch",
            Self::StaleTick => "scene tick is not newer",
            Self::StaleTime => "scene time moved backwards",
            Self::InvalidTime => "invalid scene time",
            Self::InvalidIdentity => "invalid local scene identity",
            Self::DuplicateIdentity => "duplicate local scene identity",
            Self::UnknownResource => "unknown local scene resource",
            Self::NonFiniteState => "non-finite scene state",
            Self::PoseLayout => "scene pose does not fit its loaded resource",
            Self::InstanceLimit => "scene instance budget exceeded",
            Self::BoneLimit => "scene bone budget exceeded",
            Self::BodyLimit => "scene body-selection budget exceeded",
            Self::InvalidLimits => "invalid scene budgets",
            Self::NoFrame => "no accepted scene frame",
            Self::Renderer => "scene renderer could not be created",
        })
    }
}

impl std::error::Error for SceneError {}

macro_rules! redacted_debug {
    ($($type:ty => $label:literal),+ $(,)?) => {$ (
        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!($label, " { .. }"))
            }
        }
    )+ };
}

redacted_debug! {
    SceneEpoch => "SceneEpoch", SceneIdentity => "SceneIdentity",
    SceneCatalog => "SceneCatalog", BrushHandle => "BrushHandle",
    StudioHandle => "StudioHandle", BrushRecord => "BrushRecord",
    SceneAnimation => "SceneAnimation", SceneGait => "SceneGait",
    HeldModelRecord => "HeldModelRecord", StudioRecord => "StudioRecord",
    SceneFrame => "SceneFrame", SceneReplica => "SceneReplica",
}

fn validate_frame(
    frame: &SceneFrame,
    catalog: &SceneCatalog,
    epoch: SceneEpoch,
    limits: SceneLimits,
) -> Result<(), SceneError> {
    if &frame.catalog != catalog {
        return Err(SceneError::WrongCatalog);
    }
    if frame.epoch != epoch {
        return Err(SceneError::WrongEpoch);
    }
    if !frame.time.is_finite() || frame.time < 0.0 {
        return Err(SceneError::InvalidTime);
    }
    if frame.brushes.len().saturating_add(frame.studios.len()) > limits.max_instances {
        return Err(SceneError::InstanceLimit);
    }
    let mut identities = BTreeSet::new();
    let mut budget = SceneBudget::new(limits);
    for brush in &frame.brushes {
        validate_identity(brush.identity, epoch, &mut identities)?;
        if catalog != &brush.model.catalog || !catalog.0.brushes.contains(&brush.model.local_index)
        {
            return Err(SceneError::UnknownResource);
        }
        finite(&brush.transform)?;
        budget.add(1, 0)?;
    }
    for studio in &frame.studios {
        validate_identity(studio.identity, epoch, &mut identities)?;
        finite(&studio.transform)?;
        finite(&studio.ambient)?;
        finite(&[studio.animation.cycle, studio.animation.frame_rate])?;
        if let Some(gait) = studio.animation.gait {
            finite(&[gait.cycle, gait.frame_rate])?;
        }
        validate_pose(catalog, &studio.model, &studio.pose, &studio.body)?;
        budget.add(1, studio.pose.matrices.len())?;
        if let Some(held) = &studio.held {
            validate_pose(catalog, &held.model, &held.pose, &held.body)?;
            budget.add(1, held.pose.matrices.len())?;
        }
    }
    Ok(())
}

fn validate_identity(
    identity: SceneIdentity,
    epoch: SceneEpoch,
    identities: &mut BTreeSet<SceneIdentity>,
) -> Result<(), SceneError> {
    if identity.epoch != epoch {
        return Err(SceneError::WrongEpoch);
    }
    if entity_of(identity.local).is_none() {
        return Err(SceneError::InvalidIdentity);
    }
    if !identities.insert(identity) {
        return Err(SceneError::DuplicateIdentity);
    }
    Ok(())
}

fn finite(values: &[f32]) -> Result<(), SceneError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(SceneError::NonFiniteState)
    }
}

fn validate_pose(
    catalog: &SceneCatalog,
    handle: &StudioHandle,
    pose: &StudioPose,
    body: &[u32],
) -> Result<(), SceneError> {
    let bones = catalog.studio_bone_count(handle)?;
    if pose.matrices.len() != bones || bones > ohl_world::MAX_BONES {
        return Err(SceneError::PoseLayout);
    }
    if body.len() > MAX_SCENE_BODY_SELECTIONS {
        return Err(SceneError::BodyLimit);
    }
    for matrix in &pose.matrices {
        finite(matrix)?;
    }
    Ok(())
}

pub(crate) struct SceneBudget {
    limits: SceneLimits,
    instances: usize,
    bones: usize,
}

impl SceneBudget {
    fn new(limits: SceneLimits) -> Self {
        Self {
            limits,
            instances: 0,
            bones: 0,
        }
    }

    pub(crate) fn unrestricted() -> Self {
        Self::new(SceneLimits {
            max_instances: usize::MAX,
            max_bones: usize::MAX,
        })
    }

    fn add(&mut self, instances: usize, bones: usize) -> Result<(), SceneError> {
        let instances = self
            .instances
            .checked_add(instances)
            .filter(|count| *count <= self.limits.max_instances)
            .ok_or(SceneError::InstanceLimit)?;
        let bones = self
            .bones
            .checked_add(bones)
            .filter(|count| *count <= self.limits.max_bones)
            .ok_or(SceneError::BoneLimit)?;
        self.instances = instances;
        self.bones = bones;
        Ok(())
    }
}

pub(crate) fn capture(
    level: &Level,
    catalog: &SceneCatalog,
    epoch: SceneEpoch,
    tick: u64,
    time: f32,
) -> Result<SceneFrame, SceneError> {
    let mut budget = SceneBudget::new(SceneLimits::default());
    let brushes = collect_brushes(level, catalog, epoch, &mut budget)?;
    let studios = collect_studios(level, catalog, epoch, &mut budget)?;
    let frame = SceneFrame::new(catalog.clone(), epoch, tick, time, brushes, studios);
    validate_frame(&frame, catalog, epoch, SceneLimits::default())?;
    Ok(frame)
}

pub(crate) fn collect_brushes(
    level: &Level,
    catalog: &SceneCatalog,
    epoch: SceneEpoch,
    budget: &mut SceneBudget,
) -> Result<Vec<BrushRecord>, SceneError> {
    let mut records = Vec::new();
    for instance in ohl_game::brush::model_instances(&level.registry) {
        let Ok(model) = catalog.brush_handle(instance.model_index) else {
            continue;
        };
        budget.add(1, 0)?;
        records.push(BrushRecord {
            identity: SceneIdentity::from_entity(epoch, instance.entity),
            model,
            transform: crate::render::brush_placement(
                &level.registry,
                instance.entity,
                instance.origin,
            ),
            render: crate::render::live_render_props(level, instance.entity, instance.render),
        });
    }
    Ok(records)
}

pub(crate) fn collect_studios(
    level: &Level,
    catalog: &SceneCatalog,
    epoch: SceneEpoch,
    budget: &mut SceneBudget,
) -> Result<Vec<StudioRecord>, SceneError> {
    let mut records = Vec::new();
    for (entity, anim, gait, transform) in
        &mut level
            .registry
            .world
            .query::<(Entity, &StudioAnim, Option<&StudioGait>, &Transform)>()
    {
        if crate::skirmish::studio_hidden(&level.registry.world, entity) {
            continue;
        }
        let (Ok(handle), Some(model)) = (
            catalog.studio_handle(anim.model),
            level.studio_models.get(anim.model),
        ) else {
            continue;
        };
        let Ok(pose) = anim.sample(model, gait) else {
            continue;
        };
        let held = level
            .registry
            .world
            .get::<&HeldWeapon>(entity)
            .ok()
            .and_then(|held| held.model)
            .and_then(|slot| {
                let weapon = level.studio_models.get(slot)?;
                Some(HeldModelRecord {
                    model: catalog.studio_handle(slot).ok()?,
                    pose: StudioPose::merge(weapon, model, &pose)?,
                    body: vec![0],
                    skin: 0,
                })
            });
        budget.add(1, pose.matrices.len())?;
        if let Some(held) = &held {
            budget.add(1, held.pose.matrices.len())?;
        }
        records.push(StudioRecord {
            identity: SceneIdentity::from_entity(epoch, entity),
            model: handle,
            transform: placement(transform.origin.to_array(), transform.angles.y),
            animation: SceneAnimation {
                sequence: anim.sequence,
                cycle: anim.cycle,
                frame_rate: anim.frame_rate,
                gait: gait.map(|gait| SceneGait {
                    sequence: gait.sequence,
                    cycle: gait.cycle,
                    frame_rate: gait.frame_rate,
                }),
            },
            pose,
            body: vec![anim.body],
            skin: anim.skin,
            ambient: crate::render::ambient_at(level, transform.origin.to_array()),
            render: crate::render::live_render_props(
                level,
                entity,
                ohl_game::keyvalues::RenderProps::default(),
            ),
            held,
        });
    }
    Ok(records)
}
