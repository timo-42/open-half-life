//! Bounded simulation-owned break debris, independent of rendering.
//!
//! Material/custom-model and attack-relative selection follow the published
//! Sven `func_breakable` contract cited in `ohl_game::effects`. Counts, sizes,
//! launch speeds, bounce, drag and lifetime are project policies, not measured
//! original-build constants. TODO(black-box): physical/material fidelity.
//! World/mover contacts use nine swept point samples of an axis-aligned box;
//! narrow unsampled features, oriented collision, player/debris contacts and
//! contact damage are explicitly outside this approximation.

use std::collections::BTreeSet;

use glam::{EulerRot, Mat4, Quat, Vec3};
use ohl_game::effects::BreakCommand;
use ohl_game::hecs::Entity;
use ohl_physics::{CollisionModel, Hull};
use serde::{Deserialize, Serialize};

/// Project resource limits; excess break fragments are deterministically omitted.
pub(crate) const MAX_DEBRIS: usize = 128;
const DEBRIS_PER_BREAK: usize = 6;
const MAX_CONTACTS_PER_STEP: usize = 2;
const MAX_STEP_SECONDS: f32 = 0.25;
const MIN_LIFETIME: f32 = 4.0;
const MAX_LIFETIME: f32 = 8.0;
const MAX_SPEED: f32 = 2048.0;
const MAX_ANGULAR_SPEED: f32 = 40.0;
const MAX_HALF_EXTENT: f32 = 4.0;
const MAX_GRAVITY: f32 = 4096.0;
const SOURCE_POSITION_FRACTION: f32 = 0.7;
const RANDOM_LAUNCH_SPEED: f32 = 45.0;
const UPWARD_LAUNCH_SPEED: f32 = 65.0;
const DIRECTIONAL_LAUNCH_SPEED: f32 = 90.0;
const HEAVY_MATERIAL_SPEED_SCALE: f32 = 0.7;
const ANGULAR_LAUNCH_SPEED: f32 = 8.0;
const SOURCE_SIZE_FRACTION: f32 = 0.12;
const MIN_BASE_HALF_EXTENT: f32 = 0.5;
const MAX_BASE_HALF_EXTENT: f32 = 3.0;
const MIN_SIZE_SCALE: f32 = 0.75;
const SIZE_SCALE_RANGE: f32 = 0.5;
const BOUNCE: f32 = 0.35;
const TANGENTIAL_RETENTION: f32 = 0.72;
const REST_SPEED: f32 = 12.0;
const SUPPORT_DISTANCE: f32 = 0.25;
const SEPARATION: f32 = 0.031_25;
const INITIAL_RNG: u32 = 0xa341_316c;

/// Asset loading occurs only while attaching a level, never while advancing
/// fragments. Distinct references, including failed requests, have a hard cap.
#[derive(Default)]
pub(crate) struct DebrisModels {
    pub models: Vec<ohl_world::StudioModel>,
    pub poses: Vec<ohl_world::StudioPose>,
    by_path: std::collections::BTreeMap<String, Option<usize>>,
}

impl DebrisModels {
    pub(crate) fn preload(
        &mut self,
        registry: &ohl_game::Registry,
        source: &dyn crate::AssetSource,
    ) {
        const MAX_MODELS: usize = 16;
        for &entity in &registry.entities {
            let Ok(effects) = registry
                .world
                .get::<&ohl_game::effects::BreakEffects>(entity)
            else {
                continue;
            };
            let Some(path) = effects.gib_model.as_ref() else {
                continue;
            };
            let key = path.to_ascii_lowercase();
            if self.by_path.contains_key(&key) {
                continue;
            }
            if self.by_path.len() >= MAX_MODELS {
                break;
            }
            let model = key.strip_suffix(".mdl").and_then(|stem| {
                let bytes = source.read(&key)?;
                let texture = source.read(&format!("{stem}t.mdl"));
                ohl_world::StudioModel::parse_with_external_texture(
                    &bytes,
                    texture.as_deref(),
                    &ohl_world::StudioLimits::default(),
                )
                .ok()
            });
            let slot = model.and_then(|model| {
                if !has_default_geometry(&model) {
                    return None;
                }
                let pose = ohl_world::StudioPose::sample(&model, 0, 0.0).ok()?;
                self.models.push(model);
                self.poses.push(pose);
                Some(self.models.len() - 1)
            });
            self.by_path.insert(key, slot);
        }
    }

    /// First body/skin/sequence is a bounded appearance approximation. Normalize
    /// the selected model's published bounds to the physical fragment box; this
    /// scale/centering policy is project-authored, TODO(black-box).
    pub(crate) fn placement(&self, record: &DebrisRecord) -> Option<(usize, [f32; 16])> {
        let slot = self
            .by_path
            .get(&record.gib_model.as_ref()?.to_ascii_lowercase())
            .copied()
            .flatten()?;
        let model = self.models.get(slot)?;
        let min = Vec3::from_array(model.bounds_min);
        let max = Vec3::from_array(model.bounds_max);
        let size = max - min;
        if !min.is_finite()
            || !max.is_finite()
            || size.min_element() < 0.0
            || size.max_element() <= 0.0
        {
            return None;
        }
        let scale = (0..3)
            .filter(|&axis| size[axis] > 0.0)
            .map(|axis| record.half_extents[axis] * 2.0 / size[axis])
            .fold(f32::INFINITY, f32::min);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let transform = Mat4::from_cols_array(&record.transform())
            * Mat4::from_scale(Vec3::splat(scale))
            * Mat4::from_translation(-(min + max) * 0.5);
        transform
            .is_finite()
            .then_some((slot, transform.to_cols_array()))
    }
}

/// Whole-model buffers may contain only non-default body selections. The fixed
/// body/skin policy must actually submit a triangle before fallback is suppressed.
fn has_default_geometry(model: &ohl_world::StudioModel) -> bool {
    model.visible_meshes(&[]).into_iter().any(|index| {
        let Some(mesh) = model.meshes.get(index) else {
            return false;
        };
        let Some(end) = mesh.first_index.checked_add(mesh.index_count) else {
            return false;
        };
        let Some(indices) = usize::try_from(mesh.first_index)
            .ok()
            .zip(usize::try_from(end).ok())
            .and_then(|(start, end)| model.indices.get(start..end))
        else {
            return false;
        };
        indices.len() >= 3
            && indices
                .iter()
                .all(|&index| usize::try_from(index).is_ok_and(|i| i < model.vertices.len()))
            && model
                .textures
                .get(model.resolve_skin(0, mesh.skin_slot))
                .is_some()
    })
}

/// A valid project-authored model with blank first selection and a drawable
/// second selection. Geometry exists globally but not for the fixed default body.
#[cfg(test)]
pub(crate) fn synthetic_blank_first_gib_model() -> Vec<u8> {
    let (mut bytes, layout) = ohl_formats::test_support::build_minimal_mdl10();
    let record = bytes[layout.models_offset..layout.meshes_offset].to_vec();
    let mut blank = record.clone();
    blank[72..76].copy_from_slice(&0_u32.to_le_bytes());
    let offset = u32::try_from(bytes.len()).unwrap();
    bytes.extend_from_slice(&blank);
    bytes.extend_from_slice(&record);
    bytes[layout.body_parts_offset + 64..layout.body_parts_offset + 68]
        .copy_from_slice(&2_u32.to_le_bytes());
    bytes[layout.body_parts_offset + 72..layout.body_parts_offset + 76]
        .copy_from_slice(&offset.to_le_bytes());
    let length = u32::try_from(bytes.len()).unwrap();
    bytes[72..76].copy_from_slice(&length.to_le_bytes());
    for (offset, value) in [
        (112, 10.0_f32),
        (116, 0.0),
        (120, 0.0),
        (124, 11.0),
        (128, 1.0),
        (132, 0.0),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// A live fragment. The generic source supports a stable save reference while
/// leaving runtime ECS handles out of serialized sections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebrisRecord<E = Entity> {
    pub id: u64,
    pub source: E,
    pub material: u8,
    pub position: Vec3,
    pub velocity: Vec3,
    /// Euler radians, in XYZ order; rendering only samples this state.
    pub angles: Vec3,
    pub angular_velocity: Vec3,
    pub half_extents: Vec3,
    pub age: f32,
    pub lifetime: f32,
    pub resting: bool,
    /// Retained appearance reference. Missing assets use the material cuboid.
    #[serde(deserialize_with = "bounded_model")]
    pub gib_model: Option<String>,
}

impl<E> DebrisRecord<E> {
    pub(crate) fn map_source<T>(self, source: T) -> DebrisRecord<T> {
        DebrisRecord {
            id: self.id,
            source,
            material: self.material,
            position: self.position,
            velocity: self.velocity,
            angles: self.angles,
            angular_velocity: self.angular_velocity,
            half_extents: self.half_extents,
            age: self.age,
            lifetime: self.lifetime,
            resting: self.resting,
            gib_model: self.gib_model,
        }
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.id != 0
            && self.position.is_finite()
            && self.velocity.is_finite()
            && self.velocity.length() <= MAX_SPEED
            && self.angles.is_finite()
            && self.angular_velocity.is_finite()
            && self.angular_velocity.abs().max_element() <= MAX_ANGULAR_SPEED
            && self.half_extents.is_finite()
            && self.half_extents.min_element() > 0.0
            && self.half_extents.max_element() <= MAX_HALF_EXTENT
            && self.age.is_finite()
            && self.lifetime.is_finite()
            && self.age >= 0.0
            && self.age < self.lifetime
            && (0.0..=MAX_LIFETIME).contains(&self.lifetime)
            && self
                .gib_model
                .as_ref()
                .is_none_or(|model| valid_model(model))
    }

    pub(crate) fn transform(&self) -> [f32; 16] {
        Mat4::from_rotation_translation(
            Quat::from_euler(EulerRot::XYZ, self.angles.x, self.angles.y, self.angles.z),
            self.position,
        )
        .to_cols_array()
    }

    pub(crate) fn color(&self) -> [f32; 4] {
        // Project fallback palette, not extracted original material colors.
        match self.material {
            0 | 7 => [0.55, 0.78, 0.76, 1.0],
            1 => [0.44, 0.27, 0.12, 1.0],
            2 => [0.48, 0.51, 0.55, 1.0],
            3 => [0.55, 0.08, 0.06, 1.0],
            5 => [0.72, 0.70, 0.60, 1.0],
            6 => [0.19, 0.25, 0.28, 1.0],
            _ => [0.45, 0.43, 0.39, 1.0],
        }
    }
}

/// Collision seam for synthetic fixtures and the ordinary map collision model.
pub(crate) trait DebrisWorld {
    fn sweep(&self, start: Vec3, end: Vec3, half_extents: Vec3) -> DebrisTrace;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DebrisTrace {
    pub fraction: f32,
    pub end: Vec3,
    pub normal: Vec3,
    pub embedded: bool,
}

impl DebrisTrace {
    pub(crate) fn miss(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            end,
            normal: Vec3::ZERO,
            embedded: false,
        }
    }
}

impl DebrisWorld for Option<&CollisionModel> {
    fn sweep(&self, start: Vec3, end: Vec3, half_extents: Vec3) -> DebrisTrace {
        self.map_or_else(
            || DebrisTrace::miss(end),
            |world| world.sweep(start, end, half_extents),
        )
    }
}

impl DebrisWorld for CollisionModel {
    fn sweep(&self, start: Vec3, end: Vec3, half_extents: Vec3) -> DebrisTrace {
        let mut closest = DebrisTrace::miss(end);
        for sample in 0..9 {
            let offset = if sample == 8 {
                Vec3::ZERO
            } else {
                Vec3::new(
                    if sample & 1 == 0 {
                        -half_extents.x
                    } else {
                        half_extents.x
                    },
                    if sample & 2 == 0 {
                        -half_extents.y
                    } else {
                        half_extents.y
                    },
                    if sample & 4 == 0 {
                        -half_extents.z
                    } else {
                        half_extents.z
                    },
                )
            };
            let trace = self.trace(Hull::Point, start + offset, end + offset);
            if trace.start_solid || trace.all_solid {
                return DebrisTrace {
                    fraction: 0.0,
                    end: start,
                    normal: trace.plane_normal,
                    embedded: true,
                };
            }
            if trace.fraction < closest.fraction {
                closest = DebrisTrace {
                    fraction: trace.fraction,
                    end: trace.end_pos - offset,
                    normal: trace.plane_normal,
                    embedded: false,
                };
            }
        }
        closest
    }
}

#[derive(Debug)]
pub(crate) struct DebrisSystem {
    records: Vec<DebrisRecord>,
    next_id: u64,
    rng: u32,
}

impl Default for DebrisSystem {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            rng: INITIAL_RNG,
        }
    }
}

impl DebrisSystem {
    pub(crate) fn records(&self) -> &[DebrisRecord] {
        &self.records
    }
    pub(crate) fn next_id(&self) -> u64 {
        self.next_id
    }
    pub(crate) fn rng(&self) -> u32 {
        self.rng
    }

    /// The source command already captures the live posed center and bounds.
    pub(crate) fn spawn_break(&mut self, source: Entity, origin: Vec3, command: &BreakCommand) {
        if !origin.is_finite() || !command.half_extents.is_finite() {
            return;
        }
        let count = DEBRIS_PER_BREAK.min(MAX_DEBRIS.saturating_sub(self.records.len()));
        for _ in 0..count {
            let spread = self.random_vector();
            let position = origin + spread * command.half_extents * SOURCE_POSITION_FRACTION;
            let material_speed = if matches!(command.material, 2 | 4 | 8) {
                HEAVY_MATERIAL_SPEED_SCALE
            } else {
                1.0
            };
            let random_velocity =
                self.random_vector() * RANDOM_LAUNCH_SPEED + Vec3::Z * UPWARD_LAUNCH_SPEED;
            let velocity = (random_velocity
                + command.attack_direction.unwrap_or(Vec3::ZERO) * DIRECTIONAL_LAUNCH_SPEED)
                * material_speed;
            let angles = self.random_vector() * std::f32::consts::PI;
            let angular_velocity = self.random_vector() * ANGULAR_LAUNCH_SPEED;
            let size = MIN_SIZE_SCALE + self.random_unit() * SIZE_SCALE_RANGE;
            let half_extents = (command.half_extents * SOURCE_SIZE_FRACTION).clamp(
                Vec3::splat(MIN_BASE_HALF_EXTENT),
                Vec3::splat(MAX_BASE_HALF_EXTENT),
            ) * size;
            let lifetime = MIN_LIFETIME + self.random_unit() * (MAX_LIFETIME - MIN_LIFETIME);
            let id = self.allocate_id();
            self.records.push(DebrisRecord {
                id,
                source,
                material: command.material,
                position,
                velocity,
                angles,
                angular_velocity,
                half_extents,
                age: 0.0,
                lifetime,
                resting: false,
                gib_model: command.effects.gib_model.clone(),
            });
        }
    }

    /// One bounded physics update per fixed step. The host supplies its actual
    /// world gravity; drawing never calls this method.
    pub(crate) fn advance(&mut self, dt: f32, gravity: f32, world: &impl DebrisWorld) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let dt = dt.min(MAX_STEP_SECONDS);
        let gravity = if gravity.is_finite() {
            gravity.clamp(0.0, MAX_GRAVITY)
        } else {
            0.0
        };
        for record in &mut self.records {
            record.age += dt;
            if record.age >= record.lifetime {
                continue;
            }
            // A conservative orientation-independent collision box. Exact
            // oriented contacts and riding a mover remain TODO(black-box).
            let collision_size = Vec3::splat(record.half_extents.length());
            if record.resting {
                let support = world.sweep(
                    record.position,
                    record.position - Vec3::Z * SUPPORT_DISTANCE,
                    collision_size,
                );
                if support.embedded || (support.fraction < 1.0 && support.normal.z >= 0.5) {
                    continue;
                }
                record.resting = false;
            }
            record.velocity.z -= gravity * dt;
            record.velocity = record.velocity.clamp_length_max(MAX_SPEED);
            record.angles = (record.angles + record.angular_velocity * dt) % std::f32::consts::TAU;
            let mut remaining = dt;
            for _ in 0..MAX_CONTACTS_PER_STEP {
                let hit = world.sweep(
                    record.position,
                    record.position + record.velocity * remaining,
                    collision_size,
                );
                if hit.embedded {
                    record.velocity = Vec3::ZERO;
                    record.resting = true;
                    break;
                }
                record.position = hit.end;
                if hit.fraction >= 1.0 {
                    break;
                }
                let Some(normal) = hit.normal.try_normalize() else {
                    record.velocity = Vec3::ZERO;
                    break;
                };
                record.position += normal * SEPARATION;
                let normal_speed = record.velocity.dot(normal);
                if normal_speed < 0.0 {
                    let tangent = record.velocity - normal * normal_speed;
                    record.velocity =
                        tangent * TANGENTIAL_RETENTION - normal * normal_speed * BOUNCE;
                }
                if normal.z >= 0.5 && record.velocity.length() < REST_SPEED {
                    record.velocity = Vec3::ZERO;
                    record.angular_velocity = Vec3::ZERO;
                    record.resting = true;
                    break;
                }
                remaining *= 1.0 - hit.fraction.clamp(0.0, 1.0);
                if remaining <= f32::EPSILON {
                    break;
                }
            }
        }
        self.records.retain(DebrisRecord::is_valid);
    }

    /// Restores bounded, finite live records; duplicate ids use the first entry.
    /// An old save supplies no records and the authored default seed/counter.
    pub(crate) fn restore(
        &mut self,
        records: impl IntoIterator<Item = DebrisRecord>,
        next_id: u64,
        rng: u32,
    ) {
        let mut ids = BTreeSet::new();
        self.records = records
            .into_iter()
            .take(MAX_DEBRIS)
            .filter(|record| record.is_valid() && ids.insert(record.id))
            .collect();
        self.next_id = next_id.max(1);
        self.rng = if rng == 0 { INITIAL_RNG } else { rng };
    }

    fn allocate_id(&mut self) -> u64 {
        while self.records.iter().any(|record| record.id == self.next_id) {
            self.next_id = self.next_id.wrapping_add(1).max(1);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    #[allow(clippy::cast_precision_loss)]
    fn random_unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_215.0
    }

    fn random_vector(&mut self) -> Vec3 {
        Vec3::new(self.random_unit(), self.random_unit(), self.random_unit()) * 2.0 - Vec3::ONE
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 260
        && !model.contains(':')
        && !model.contains('\\')
        && !model.chars().any(char::is_control)
        && model
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn bounded_model<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    struct OptionalModel;
    struct Model;
    impl<'de> serde::de::Visitor<'de> for Model {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded relative model reference")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if valid_model(value) {
                Ok(value.to_owned())
            } else {
                Err(E::custom("invalid debris appearance"))
            }
        }
    }
    impl<'de> serde::de::Visitor<'de> for OptionalModel {
        type Value = Option<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("optional bounded model reference")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_str(Model).map(Some)
        }
    }
    deserializer.deserialize_option(OptionalModel)
}
