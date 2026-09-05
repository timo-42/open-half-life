//! Attack traces: world geometry first, then posed studio hitboxes.
//!
//! [`trace_attack`] answers "what does this shot hit". It traces the segment
//! through the map's point hull with `ohl_physics`, then refines the result
//! against the entities in a [`HitboxIndex`] — a bounded, flat list the
//! caller rebuilds each tick from its own entity storage. Nothing here
//! touches an ECS world, so hit resolution is a pure function of its inputs
//! and can be property-tested and fuzzed on its own.
//!
//! The hit group names are Half-Life's published vocabulary (see
//! `docs/FORMAT_SOURCES.md`, "Combat and damage"); the multipliers attached
//! to them are **to be black-box observed** and therefore live in a
//! caller-supplied [`HitGroupScale`] whose `Default` scales nothing.
//!
//! A projectile or melee swing should not hit its own owner. Rather than
//! asking the caller to omit the owner from the [`HitboxIndex`] — which
//! would make that index unusable for anyone else's trace against the same
//! tick — use [`trace_attack_filtered`] with a [`TraceFilter`] naming the
//! owner (and, if relevant, the weapon entity) in its `ignore` slots; those
//! entities are skipped during hitbox refinement but the index itself is
//! left untouched.

use glam::{EulerRot, Quat, Vec3};
use ohl_physics::{CollisionModel, Hull};
use ohl_world::{StudioHitbox, StudioPose};

/// An opaque handle to whatever the caller calls an entity.
///
/// `ohl-combat` never dereferences it. Callers driving a `hecs` world pass
/// `hecs::Entity::to_bits().get()`; other callers can use any stable
/// per-entity number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId(pub u64);

/// Half-Life's published hit groups.
///
/// The names and their numbering are the values a studio model's hitboxes
/// carry in their `group` field, as documented by the public model-viewer and
/// mapping references cited in `docs/FORMAT_SOURCES.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HitGroup {
    /// No specific group; the whole-body default.
    #[default]
    Generic,
    /// Head.
    Head,
    /// Chest.
    Chest,
    /// Stomach.
    Stomach,
    /// Left arm.
    LeftArm,
    /// Right arm.
    RightArm,
    /// Left leg.
    LeftLeg,
    /// Right leg.
    RightLeg,
}

impl HitGroup {
    /// Maps a [`StudioHitbox::group`] value; anything outside the published
    /// range falls back to [`HitGroup::Generic`] rather than being rejected,
    /// since a hitbox group is untrusted model data.
    #[must_use]
    pub const fn from_index(group: i32) -> Self {
        match group {
            1 => Self::Head,
            2 => Self::Chest,
            3 => Self::Stomach,
            4 => Self::LeftArm,
            5 => Self::RightArm,
            6 => Self::LeftLeg,
            7 => Self::RightLeg,
            _ => Self::Generic,
        }
    }

    /// The group's numeric value.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Generic => 0,
            Self::Head => 1,
            Self::Chest => 2,
            Self::Stomach => 3,
            Self::LeftArm => 4,
            Self::RightArm => 5,
            Self::LeftLeg => 6,
            Self::RightLeg => 7,
        }
    }
}

/// Per-hit-group damage multipliers.
///
/// **To be black-box observed.** Half-Life multiplies damage by a per-group
/// factor (a head shot hurts more than a leg shot), but the factors are not
/// published on any source this project may use, so [`Default`] is `1.0`
/// everywhere and every caller and test supplies its own values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitGroupScale {
    /// Multiplier for [`HitGroup::Generic`].
    pub generic: f32,
    /// Multiplier for [`HitGroup::Head`].
    pub head: f32,
    /// Multiplier for [`HitGroup::Chest`].
    pub chest: f32,
    /// Multiplier for [`HitGroup::Stomach`].
    pub stomach: f32,
    /// Multiplier for both arms.
    pub arms: f32,
    /// Multiplier for both legs.
    pub legs: f32,
}

impl Default for HitGroupScale {
    fn default() -> Self {
        Self {
            generic: 1.0,
            head: 1.0,
            chest: 1.0,
            stomach: 1.0,
            arms: 1.0,
            legs: 1.0,
        }
    }
}

impl HitGroupScale {
    /// The multiplier for `group`; non-finite or negative entries are
    /// treated as `1.0`.
    #[must_use]
    pub fn factor(self, group: HitGroup) -> f32 {
        let raw = match group {
            HitGroup::Generic => self.generic,
            HitGroup::Head => self.head,
            HitGroup::Chest => self.chest,
            HitGroup::Stomach => self.stomach,
            HitGroup::LeftArm | HitGroup::RightArm => self.arms,
            HitGroup::LeftLeg | HitGroup::RightLeg => self.legs,
        };
        if raw.is_finite() && raw >= 0.0 {
            raw
        } else {
            1.0
        }
    }
}

/// What an attack trace is allowed to hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceMask {
    /// Whether solid world geometry stops the trace.
    pub world: bool,
    /// Whether the entities in the [`HitboxIndex`] stop the trace.
    pub entities: bool,
}

impl TraceMask {
    /// The usual mask for a shot: world geometry and entities both block.
    pub const SHOT: Self = Self {
        world: true,
        entities: true,
    };
    /// World geometry only (line-of-sight against the map itself).
    pub const WORLD_ONLY: Self = Self {
        world: true,
        entities: false,
    };
    /// Entities only, ignoring the map.
    pub const ENTITIES_ONLY: Self = Self {
        world: false,
        entities: true,
    };
}

impl Default for TraceMask {
    fn default() -> Self {
        Self::SHOT
    }
}

/// What an attack trace is allowed to hit, plus entities it must never hit
/// regardless of the mask.
///
/// The `ignore` slots hold up to two [`EntityId`]s — typically an attack's
/// owner and, for a thrown or fired weapon entity, the weapon itself — that
/// are skipped during hitbox refinement so an attack cannot hit its own
/// source. `Default` ignores nothing and uses [`TraceMask::SHOT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TraceFilter {
    /// Entities skipped when refining against the [`HitboxIndex`].
    pub ignore: [Option<EntityId>; 2],
    /// What the trace is allowed to hit.
    pub mask: TraceMask,
}

impl TraceFilter {
    /// A filter that ignores nothing, using `mask`.
    #[must_use]
    pub const fn new(mask: TraceMask) -> Self {
        Self {
            ignore: [None, None],
            mask,
        }
    }

    /// A filter using `mask` that also skips `owner` during hitbox
    /// refinement.
    #[must_use]
    pub const fn ignoring(mask: TraceMask, owner: EntityId) -> Self {
        Self {
            ignore: [Some(owner), None],
            mask,
        }
    }

    /// Whether `id` is one of this filter's ignored entities.
    #[must_use]
    fn ignores(&self, id: EntityId) -> bool {
        self.ignore[0] == Some(id) || self.ignore[1] == Some(id)
    }
}

/// One posed hitbox, as an axis-aligned box in its entity's local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitboxVolume {
    /// The hitbox's index in its model's hitbox list, reported back in
    /// [`AttackTrace::hitbox`].
    pub index: usize,
    /// Minimum corner in entity-local space.
    pub min: Vec3,
    /// Maximum corner in entity-local space.
    pub max: Vec3,
    /// The hit group the box belongs to.
    pub group: HitGroup,
}

/// One entity's posed hitboxes, oriented into the world.
///
/// The boxes are axis aligned in the entity's own space; together with the
/// entity's origin and rotation each one is an oriented box in the world,
/// which is what [`trace_attack`] intersects the segment against.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityHitboxes {
    /// The caller's handle for this entity.
    pub id: EntityId,
    /// World-space origin of the entity.
    pub origin: Vec3,
    /// Entity-to-world rotation.
    pub rotation: Quat,
    /// The entity's hitboxes.
    pub boxes: Vec<HitboxVolume>,
}

impl EntityHitboxes {
    /// An entry at `origin` with no rotation and no boxes yet.
    #[must_use]
    pub fn new(id: EntityId, origin: Vec3) -> Self {
        Self {
            id,
            origin,
            rotation: Quat::IDENTITY,
            boxes: Vec::new(),
        }
    }

    /// An entry positioned by an `ohl-game` [`Transform`].
    ///
    /// The keyvalue angles are `pitch yaw roll` in degrees; this crate reads
    /// them as intrinsic rotations about `+Z`, `+Y` and `+X` in that order,
    /// the convention `ohl_game::registry::movedir_from_angles` already uses
    /// for yaw.
    ///
    /// [`Transform`]: ohl_game::registry::Transform
    #[must_use]
    pub fn from_transform(id: EntityId, transform: &ohl_game::registry::Transform) -> Self {
        let angles = transform.angles;
        Self {
            id,
            origin: transform.origin,
            rotation: Quat::from_euler(
                EulerRot::ZYX,
                angles.y.to_radians(),
                angles.x.to_radians(),
                angles.z.to_radians(),
            ),
            boxes: Vec::new(),
        }
    }

    /// Overrides the entity-to-world rotation.
    #[must_use]
    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    /// Appends one hitbox in entity-local space, normalising the corners.
    pub fn push_box(&mut self, index: usize, min: Vec3, max: Vec3, group: HitGroup) {
        self.boxes.push(HitboxVolume {
            index,
            min: min.min(max),
            max: min.max(max),
            group,
        });
    }

    /// Appends every hitbox of a posed studio model, in model order.
    ///
    /// Uses [`StudioPose::hitbox_bounds`](ohl_world::StudioPose::hitbox_bounds), so a hitbox whose bone is missing
    /// from the pose is skipped rather than mispositioned; boxes with a
    /// non-finite or empty extent are skipped too. Returns how many were
    /// added.
    pub fn push_studio_hitboxes(&mut self, pose: &StudioPose, hitboxes: &[StudioHitbox]) -> usize {
        let before = self.boxes.len();
        for (index, hitbox) in hitboxes.iter().enumerate() {
            let Some((min, max)) = pose.hitbox_bounds(hitbox) else {
                continue;
            };
            let min = Vec3::from_array(min);
            let max = Vec3::from_array(max);
            if !min.is_finite() || !max.is_finite() || (max - min).min_element() <= 0.0 {
                continue;
            }
            self.push_box(index, min, max, HitGroup::from_index(hitbox.group));
        }
        self.boxes.len() - before
    }
}

/// Caps on how much work one [`HitboxIndex`] may describe.
///
/// Attack traces run per shot, per pellet, per tick, so the index is bounded
/// at construction instead of trusting whatever the caller assembles from
/// model data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitboxLimits {
    /// Maximum number of entities in the index.
    pub max_entities: usize,
    /// Maximum number of hitboxes per entity.
    pub max_boxes_per_entity: usize,
}

impl Default for HitboxLimits {
    /// Project-chosen caps, comfortably above any plausible tick: 1024
    /// entities of 64 hitboxes each.
    fn default() -> Self {
        Self {
            max_entities: 1024,
            max_boxes_per_entity: 64,
        }
    }
}

/// The candidate entities one [`trace_attack`] call may hit.
///
/// A flat list, rebuilt by the caller each tick from whatever spatial
/// structure it keeps; `trace_attack` tests every entry, so callers should
/// cull to the shot's neighbourhood before filling it.
#[derive(Debug, Clone, Default)]
pub struct HitboxIndex {
    entries: Vec<EntityHitboxes>,
    limits: HitboxLimits,
    rejected: usize,
}

impl HitboxIndex {
    /// An empty index with the given limits.
    #[must_use]
    pub fn new(limits: HitboxLimits) -> Self {
        Self {
            entries: Vec::new(),
            limits,
            rejected: 0,
        }
    }

    /// Empties the index, keeping its allocation and limits, ready for the
    /// next tick.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.rejected = 0;
    }

    /// Adds one entity, truncating its hitbox list to
    /// [`HitboxLimits::max_boxes_per_entity`].
    ///
    /// Returns `false` when the index is already full or the entry has no
    /// usable hitbox; either way the rejection is counted by
    /// [`rejected`](Self::rejected) rather than reported as an error, so a
    /// crowded tick degrades instead of failing.
    pub fn push(&mut self, mut entity: EntityHitboxes) -> bool {
        if self.entries.len() >= self.limits.max_entities || entity.boxes.is_empty() {
            self.rejected += 1;
            return false;
        }
        if entity.boxes.len() > self.limits.max_boxes_per_entity {
            entity.boxes.truncate(self.limits.max_boxes_per_entity);
            self.rejected += 1;
        }
        self.entries.push(entity);
        true
    }

    /// The entities in the index.
    #[must_use]
    pub fn entries(&self) -> &[EntityHitboxes] {
        &self.entries
    }

    /// How many entities the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entities or hitbox lists were dropped by the limits.
    #[must_use]
    pub fn rejected(&self) -> usize {
        self.rejected
    }
}

/// Where an attack trace stopped.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackTrace {
    /// How far along `start -> end` the trace got, always in `0.0..=1.0`;
    /// `1.0` means nothing was hit.
    pub fraction: f32,
    /// The impact point, always `start + fraction * (end - start)`.
    pub end: Vec3,
    /// The entity hit, when an entity hitbox was nearer than the world.
    pub entity: Option<EntityId>,
    /// The index of the hitbox hit within its model's hitbox list.
    pub hitbox: Option<usize>,
    /// The hit group of that hitbox, or [`HitGroup::Generic`] for a world
    /// hit or a miss.
    pub hitgroup: HitGroup,
    /// The unit normal of the surface hit, pointing back towards the
    /// shooter. Zero when nothing was hit.
    pub surface_normal: Vec3,
}

impl AttackTrace {
    /// A trace that hit nothing, ending at `end`.
    #[must_use]
    pub fn miss(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            end,
            entity: None,
            hitbox: None,
            hitgroup: HitGroup::Generic,
            surface_normal: Vec3::ZERO,
        }
    }

    /// Whether the trace stopped short of `end`.
    #[must_use]
    pub fn hit(&self) -> bool {
        self.fraction < 1.0
    }

    /// Whether an entity hitbox, rather than the world, stopped the trace.
    #[must_use]
    pub fn hit_entity(&self) -> bool {
        self.entity.is_some()
    }
}

/// Traces an attack from `start` to `end`.
///
/// World geometry is traced through hull 0 (the point hull, since a bullet
/// has no size); if the mask allows entities, every entry of `entities` is
/// then intersected as a set of oriented boxes and the nearest impact — world
/// or entity — wins. A wall in front of a monster therefore blocks the shot,
/// and a monster in front of a wall takes it.
///
/// Total: a degenerate segment, a non-finite endpoint or an empty index
/// yields a miss rather than a panic.
///
/// A thin wrapper over [`trace_attack_filtered`] with an empty ignore list;
/// use that function instead when an attack must not hit its own owner.
#[must_use]
pub fn trace_attack(
    world: &CollisionModel,
    entities: &HitboxIndex,
    start: Vec3,
    end: Vec3,
    mask: TraceMask,
) -> AttackTrace {
    trace_attack_filtered(world, entities, start, end, TraceFilter::new(mask))
}

/// Traces an attack from `start` to `end`, as [`trace_attack`], but skipping
/// every entity named in `filter.ignore` during hitbox refinement.
///
/// An ignored entity is invisible to the entity pass entirely: it cannot
/// stop the trace and cannot be passed through to reach something behind
/// it — the trace behaves exactly as if that entity were absent from
/// `entities`. World geometry is unaffected by the ignore list.
#[must_use]
pub fn trace_attack_filtered(
    world: &CollisionModel,
    entities: &HitboxIndex,
    start: Vec3,
    end: Vec3,
    filter: TraceFilter,
) -> AttackTrace {
    if !start.is_finite() || !end.is_finite() {
        return AttackTrace::miss(if end.is_finite() { end } else { start });
    }

    let mask = filter.mask;
    let mut best = AttackTrace::miss(end);
    if mask.world {
        let trace = world.trace(Hull::Point, start, end);
        let fraction = clamp_fraction(trace.fraction);
        if fraction < 1.0 || trace.start_solid || trace.all_solid {
            best = AttackTrace {
                fraction,
                end: start + (end - start) * fraction,
                entity: None,
                hitbox: None,
                hitgroup: HitGroup::Generic,
                surface_normal: trace.plane_normal,
            };
        }
    }

    if !mask.entities {
        return best;
    }

    let delta = end - start;
    for entity in entities.entries() {
        if filter.ignores(entity.id) {
            continue;
        }
        // Into the entity's own space, where its hitboxes are axis aligned.
        let inverse = entity.rotation.conjugate();
        let local_start = inverse * (start - entity.origin);
        let local_delta = inverse * delta;
        for volume in &entity.boxes {
            let Some((fraction, local_normal)) =
                ray_box(local_start, local_delta, volume.min, volume.max)
            else {
                continue;
            };
            if fraction >= best.fraction {
                continue;
            }
            best = AttackTrace {
                fraction,
                end: start + delta * fraction,
                entity: Some(entity.id),
                hitbox: Some(volume.index),
                hitgroup: volume.group,
                surface_normal: (entity.rotation * local_normal).normalize_or_zero(),
            };
        }
    }

    best
}

/// Clamps a trace fraction into `0.0..=1.0`, mapping a non-finite one to a
/// full-length miss.
fn clamp_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Slab test of the segment `start + t * delta`, `t` in `0.0..=1.0`, against
/// the axis-aligned box `min..max`.
///
/// Returns the entry fraction and the outward unit normal of the face
/// entered. A segment that starts inside the box returns fraction `0.0` and
/// the normal of the face it would leave backwards through, so a point-blank
/// shot still registers.
fn ray_box(start: Vec3, delta: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let mut near = 0.0f32;
    let mut far = 1.0f32;
    let mut axis = 0usize;
    let mut sign = -1.0f32;

    for index in 0..3 {
        let origin = start[index];
        let direction = delta[index];
        let (low, high) = (min[index], max[index]);
        if direction.abs() < f32::EPSILON {
            if origin < low || origin > high {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction;
        let mut t_low = (low - origin) * inverse;
        let mut t_high = (high - origin) * inverse;
        let mut entering = -1.0f32;
        if t_low > t_high {
            core::mem::swap(&mut t_low, &mut t_high);
            entering = 1.0;
        }
        if t_low > near {
            near = t_low;
            axis = index;
            sign = entering;
        }
        far = far.min(t_high);
        if near > far {
            return None;
        }
    }

    if !near.is_finite() || near > 1.0 {
        return None;
    }
    let mut normal = Vec3::ZERO;
    normal[axis] = sign;
    Some((near.clamp(0.0, 1.0), normal))
}
