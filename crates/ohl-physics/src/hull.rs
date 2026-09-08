//! Clip-hull tracing.
//!
//! A GoldSrc map is compiled with four collision trees. Hull 0 is the BSP
//! node tree itself and answers point queries; hulls 1-3 are separate
//! `BSPCLIPNODE` trees whose planes were pushed outward by the compiler by
//! the size of a player bounding box, so sweeping a box through the world
//! becomes a *point* trace through the matching pre-expanded hull. The four
//! documented box sizes are in [`HULL_SIZES`].
//!
//! [`CollisionModel`] owns a validated copy of those trees:
//! [`CollisionModel::from_bsp`] checks every plane, child, leaf and head
//! index once, so [`CollisionModel::trace`] afterwards cannot fail, cannot
//! panic, and — being depth-limited — cannot be made to recurse without
//! bound by a malformed or hostile map.
//!
//! Implemented from the public documentation recorded in
//! `docs/FORMAT_SOURCES.md` under "Collision hulls and player movement": the
//! Unofficial Quake Specs' description of `clipnode`/hull structure and the
//! Valve Developer Community's BSP and hull articles.

use alloc::vec::Vec;

use glam::{Quat, Vec3};
use ohl_core::SanitizedError;
use ohl_formats::bsp30::{Bsp, Limits};

/// Contents values stored in a leaf or encoded in a negative hull child
/// link. These are the documented Quake/GoldSrc values, unchanged in BSP
/// v30.
pub mod contents {
    /// Ordinary open space.
    pub const EMPTY: i32 = -1;
    /// Impassable world geometry.
    pub const SOLID: i32 = -2;
    /// Water.
    pub const WATER: i32 = -3;
    /// Slime.
    pub const SLIME: i32 = -4;
    /// Lava.
    pub const LAVA: i32 = -5;
    /// Sky.
    pub const SKY: i32 = -6;
    /// A brush that only marked a model origin at compile time.
    pub const ORIGIN: i32 = -7;
    /// Solid to players but invisible (`CLIP` brushes).
    pub const CLIP: i32 = -8;
    /// Push volumes, one per direction.
    pub const CURRENT_0: i32 = -9;
    /// Push volume, +90 degrees.
    pub const CURRENT_90: i32 = -10;
    /// Push volume, 180 degrees.
    pub const CURRENT_180: i32 = -11;
    /// Push volume, 270 degrees.
    pub const CURRENT_270: i32 = -12;
    /// Push volume, up.
    pub const CURRENT_UP: i32 = -13;
    /// Push volume, down.
    pub const CURRENT_DOWN: i32 = -14;
    /// GoldSrc addition: see-through but non-solid.
    pub const TRANSLUCENT: i32 = -15;
    /// GoldSrc addition: climbable.
    pub const LADDER: i32 = -16;

    /// The most negative contents value this crate accepts from a map.
    pub const MIN: i32 = LADDER;

    /// Whether `value` is anything other than open space. Used to decide
    /// whether a point falls *inside* a non-solid contents-volume brush
    /// (`func_ladder`/`func_water`; see [`super::ContentsKind`]): such a
    /// brush's own compiled hull tree has no special contents of its own
    /// (only [`SOLID`] "inside the brush shape" and [`EMPTY`] "outside
    /// it", the same as any ordinary brush), so a query into its tree is
    /// remapped from "solid" to the volume's declared kind rather than
    /// read literally.
    #[must_use]
    pub const fn is_present(value: i32) -> bool {
        value != EMPTY
    }

    /// Whether `value` blocks player movement. `CLIP` blocks players even
    /// though it is invisible; everything else that is not `SOLID` does not.
    #[must_use]
    pub const fn is_solid(value: i32) -> bool {
        value == SOLID || value == CLIP
    }

    /// Whether `value` is one of the swimmable liquids.
    #[must_use]
    pub const fn is_liquid(value: i32) -> bool {
        value == WATER || value == SLIME || value == LAVA
    }
}

/// The offset, in world units, by which a trace stops short of the plane it
/// hit, so the resulting position is never exactly *on* the plane (where
/// floating-point rounding could classify it as solid). This is the
/// long-documented `DIST_EPSILON` value, 1/32 of a unit.
pub const DIST_EPSILON: f32 = 0.031_25;

/// The largest number of hull nodes one trace will visit along a single
/// path before giving up and reporting the move as blocked. Real hull trees
/// are far shallower; the bound exists so a cyclic child link in a
/// malformed map cannot recurse without end.
pub const MAX_TRACE_DEPTH: u32 = 256;

/// The largest number of solid brush entities one [`CollisionModel`] will
/// attach. `CollisionModel::attach_brush` refuses anything past this rather
/// than growing `brushes` without bound: every attached brush costs one
/// extra tree walk per trace (see [`CollisionModel::trace`]), so an
/// unbounded count is an unbounded per-trace cost. No published GoldSrc map
/// comes remotely close to this many solid brush entities; a map that
/// somehow did would simply have its excess brush entities fall back to
/// having no collision, rather than the engine's per-frame cost growing
/// without limit.
pub const MAX_ATTACHED_BRUSHES: usize = 512;

/// The four documented GoldSrc hulls, in the order the compiler writes them
/// into `BSPMODEL::headnodes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hull {
    /// Hull 0: a point (the BSP node tree itself).
    Point,
    /// Hull 1: the standing player, 32x32x72.
    Standing,
    /// Hull 2: the large hull, 64x64x64.
    Large,
    /// Hull 3: the crouched player, 32x32x36.
    Crouched,
}

/// Each hull's bounding box relative to the entity origin, in hull order.
/// The origin of a standing player therefore sits 36 units above the floor
/// it stands on, and a crouched one 18 units above it.
pub const HULL_SIZES: [([f32; 3], [f32; 3]); 4] = [
    ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
    ([-16.0, -16.0, -36.0], [16.0, 16.0, 36.0]),
    ([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]),
    ([-16.0, -16.0, -18.0], [16.0, 16.0, 18.0]),
];

impl Hull {
    /// This hull's index into `BSPMODEL::headnodes` and [`HULL_SIZES`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Point => 0,
            Self::Standing => 1,
            Self::Large => 2,
            Self::Crouched => 3,
        }
    }

    /// The hull at `index`, or `None` when `index > 3`.
    #[must_use]
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Point),
            1 => Some(Self::Standing),
            2 => Some(Self::Large),
            3 => Some(Self::Crouched),
            _ => None,
        }
    }

    /// This hull's bounding box relative to the entity origin.
    #[must_use]
    pub fn bounds(self) -> (Vec3, Vec3) {
        let (mins, maxs) = HULL_SIZES[self.index()];
        (Vec3::from_array(mins), Vec3::from_array(maxs))
    }

    /// The offset from an entity origin down to the bottom of this hull
    /// (36 standing, 18 crouched, 0 for a point).
    #[must_use]
    pub fn foot_offset(self) -> f32 {
        -HULL_SIZES[self.index()].0[2]
    }

    /// Picks the hull whose box best fits an entity of size `maxs - mins`,
    /// using the documented selection rule: boxes no wider than 8 units use
    /// the point hull, boxes no wider than 36 units use the human-sized
    /// hulls (crouched when they are also short), and anything wider uses
    /// the large hull.
    #[must_use]
    pub fn for_size(mins: Vec3, maxs: Vec3) -> Self {
        let size = maxs - mins;
        if size.x <= 8.0 {
            Self::Point
        } else if size.x <= 36.0 {
            if size.z <= 36.0 {
                Self::Crouched
            } else {
                Self::Standing
            }
        } else {
            Self::Large
        }
    }
}

/// The result of tracing a segment through one hull.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Trace {
    /// How far along `start -> end` the move got, in `0.0..=1.0`. `1.0`
    /// means nothing was hit.
    pub fraction: f32,
    /// Where the move ended, always `start + fraction * (end - start)`.
    pub end_pos: Vec3,
    /// The unit normal of the plane that stopped the move, pointing out of
    /// the solid. Zero when nothing was hit.
    pub plane_normal: Vec3,
    /// The distance of that plane from the origin along its normal.
    pub plane_dist: f32,
    /// The whole segment was inside solid.
    pub all_solid: bool,
    /// The segment started inside solid.
    pub start_solid: bool,
    /// Some part of the segment was in open (empty) space, *as seen by the
    /// world tree alone*: an attached solid brush entity's own hull tree
    /// treats everywhere outside its own small footprint as empty, so
    /// folding a brush's `in_open` into this field would routinely report
    /// "open" for a segment that, once the brush is accounted for
    /// (`CollisionModel::contents_at`'s solid-wins-over-world rule), never
    /// left world solid at all. See [`combine`].
    pub in_open: bool,
    /// Some part of the segment was in a liquid or other non-empty,
    /// non-solid volume.
    pub in_water: bool,
    /// The contents value at [`Self::end_pos`], as seen by the traced hull.
    pub contents: i32,
    /// Which attached brush entity's hull tree produced the nearest hit
    /// ([`combine`] keeps the smaller fraction), so a caller can tell a
    /// world surface from a `func_train`/`func_plat`/`func_door` the player
    /// is standing on or was stopped by. Also set when the segment started
    /// embedded in an attached brush's solid, even if the world tree is
    /// *also* `start_solid` (a fraction tie [`combine`] cannot otherwise
    /// break) — see [`combine`]'s own doc for why that tie is resolved in
    /// the brush's favour. `None` when the move ended on the world tree
    /// alone, or hit nothing at all.
    pub brush_index: Option<BrushId>,
}

impl Trace {
    /// A trace that hit nothing, ending at `end`.
    #[must_use]
    pub fn miss(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            end_pos: end,
            plane_normal: Vec3::ZERO,
            plane_dist: 0.0,
            all_solid: false,
            start_solid: false,
            in_open: false,
            in_water: false,
            contents: contents::EMPTY,
            brush_index: None,
        }
    }

    /// Whether the move was stopped before reaching its destination.
    #[must_use]
    pub fn blocked(&self) -> bool {
        self.fraction < 1.0 || self.start_solid || self.all_solid
    }
}

/// One hull node: a plane and two children. A non-negative child is another
/// node in the same array; a negative child is a contents value.
#[derive(Debug, Clone, Copy)]
struct HullNode {
    plane: u32,
    children: [i32; 2],
}

#[derive(Debug, Clone, Copy)]
struct HullPlane {
    normal: Vec3,
    dist: f32,
}

/// Identifies one brush entity (solid or a non-solid contents volume)
/// attached to a [`CollisionModel`] with [`CollisionModel::attach_brush`] or
/// [`CollisionModel::attach_contents_brush`], so its origin can be updated
/// as the map logic moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrushId(usize);

/// The non-solid contents a brush entity attached with
/// [`CollisionModel::attach_contents_brush`] reports wherever a query lands
/// inside its submodel's shape.
///
/// TWHL wiki `func_ladder` (already cited in `docs/FORMAT_SOURCES.md` under
/// "Player systems"): a brush entity's `skin` keyvalue set to `-16`
/// (`CONTENTS_LADDER`) makes it climbable the same way a world ladder
/// volume is. TWHL wiki `func_water` (consulted via a search-engine result
/// summary of the page, same HTTP 403 caveat already recorded for other
/// TWHL citations; reviewed 2026-09-07): its "Contents (skin)" keyvalue
/// selects which of the three documented liquids the volume is, using the
/// raw `CONTENTS_*` enum values directly as the keyvalue's choices (`-3`
/// water, `-4` slime, `-5` lava) — the same values [`contents::WATER`],
/// [`contents::SLIME`] and [`contents::LAVA`] already name. `func_water`
/// shares its move/trigger behaviour with `func_door` (same search-summary
/// source), which is why this crate does not special-case its origin:
/// [`CollisionModel::set_brush_origin`] already moves any attached brush,
/// solid or contents volume alike, to wherever the caller's map-logic step
/// currently has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentsKind {
    /// `func_ladder`, or any brush entity documented as climbable.
    Ladder,
    /// `func_water` with `skin` `-3` (or absent, the documented default).
    Water,
    /// `func_water` with `skin` `-4`.
    Slime,
    /// `func_water` with `skin` `-5`.
    Lava,
}

impl ContentsKind {
    /// The contents value this kind reports.
    #[must_use]
    const fn contents_value(self) -> i32 {
        match self {
            Self::Ladder => contents::LADDER,
            Self::Water => contents::WATER,
            Self::Slime => contents::SLIME,
            Self::Lava => contents::LAVA,
        }
    }
}

/// What kind of brush a [`BrushPart`] is, deciding how
/// [`CollisionModel::contents_at`] and [`CollisionModel::trace`] treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrushKind {
    /// Blocks movement; [`CollisionModel::attach_brush`]. Reads its
    /// contents straight from the submodel's own compiled tree, exactly as
    /// the world does.
    Solid,
    /// Never blocks movement; [`CollisionModel::attach_contents_brush`].
    /// The submodel's own tree only ever distinguishes "inside the brush
    /// shape" from "outside" (it was compiled as an ordinary, solid-shaped
    /// brush; nothing in the BSP format lets a brush *entity* declare
    /// special leaf contents at compile time) — this crate remaps "inside"
    /// to the carried [`ContentsKind`] and never treats it as solid,
    /// implementing the documented `skin`-keyvalue override at query time
    /// instead of at compile time.
    Contents(ContentsKind),
}

/// One brush entity's hulls: the head links of its `BSPMODEL` plus the
/// world-space offset the map logic has moved it by since the map was
/// compiled.
#[derive(Debug, Clone, Copy)]
struct BrushPart {
    heads: [i32; 4],
    /// World-space translation offset from where the submodel was compiled,
    /// as moved by [`CollisionModel::set_brush_origin`]/
    /// [`CollisionModel::set_brush_pose`]. A translating mover
    /// (`func_door`/`func_plat`/`func_train`) only ever sets this; a purely
    /// rotating one ([`Self::has_rotation`]) leaves it at `Vec3::ZERO`.
    origin: Vec3,
    /// World-space point a rotation happens about. TWHL wiki
    /// `func_door_rotating` (see `docs/FORMAT_SOURCES.md`, "Entity
    /// keyvalues and map logic"): the entity "requires an origin brush ...
    /// which gives it the axis to rotate on", i.e. the compiled `origin`
    /// keyvalue is the pivot, not a translation. Unused (any value is
    /// equivalent) while [`Self::has_rotation`] is false.
    pivot: Vec3,
    /// The rotation axis, or `Vec3::ZERO` for no rotation. Need not be
    /// normalized; [`Self::has_rotation`] and [`Self::rotation`] treat a
    /// zero axis as "no rotation" regardless of `angle_deg`.
    axis: Vec3,
    /// The current rotation angle about [`Self::axis`], in degrees.
    angle_deg: f32,
    kind: BrushKind,
    /// The submodel's own compiled bounding box (`BSPMODEL::mins/maxs`),
    /// relative to the same frame [`Self::heads`]'s trees are in (i.e.
    /// before [`Self::origin`]/[`Self::pivot`]/[`Self::axis`] are applied).
    /// Used only for the broad-phase check in [`CollisionModel::trace`] and
    /// [`CollisionModel::contents_at`]; it plays no part in what is
    /// actually solid, which is decided by walking the hull tree.
    mins: Vec3,
    maxs: Vec3,
}

impl BrushPart {
    /// Whether every hull's head link is already a bare contents value
    /// rather than a tree: such a brush has no boundary and cannot bound a
    /// volume (see the no-op case documented on [`CollisionModel::trace`]),
    /// so it is not a "brush" for [`CollisionModel::brush_count`]'s
    /// purposes. [`CollisionModel::detach_brush`] reduces a brush to this
    /// same state, so a despawned brush entity is excluded exactly the same
    /// way a submodel that was always bare contents is.
    fn is_bare(&self) -> bool {
        self.heads.iter().all(|&head| head < 0)
    }

    /// Whether this brush currently carries a live rotation: a zero axis or
    /// a zero angle both mean "no rotation", the same translation-only
    /// pose every brush had before [`CollisionModel::set_brush_pose`]
    /// existed.
    fn has_rotation(&self) -> bool {
        self.angle_deg != 0.0 && self.axis != Vec3::ZERO
    }

    /// The unit quaternion this brush's current pose rotates by. Only
    /// meaningful when [`Self::has_rotation`] is true.
    fn rotation(&self) -> Quat {
        Quat::from_axis_angle(self.axis.normalize(), self.angle_deg.to_radians())
    }

    /// Converts a world-space point into this brush's compiled (local)
    /// frame: subtract the translation offset, then undo the rotation about
    /// [`Self::pivot`]. Bit-identical to the pre-rotation `point -
    /// self.origin` whenever [`Self::has_rotation`] is false, so every
    /// existing translation-only trace is unaffected.
    fn local_point(&self, world: Vec3) -> Vec3 {
        if self.has_rotation() {
            self.rotation().inverse() * (world - self.origin - self.pivot) + self.pivot
        } else {
            world - self.origin
        }
    }

    /// The inverse of [`Self::local_point`].
    fn world_point(&self, local: Vec3) -> Vec3 {
        if self.has_rotation() {
            self.rotation() * (local - self.pivot) + self.pivot + self.origin
        } else {
            local + self.origin
        }
    }

    /// Rotates a direction (a plane normal) from the compiled frame into
    /// world space; a translation does not change a normal, so this is the
    /// identity whenever [`Self::has_rotation`] is false.
    fn normal_to_world(&self, local_normal: Vec3) -> Vec3 {
        if self.has_rotation() {
            self.rotation() * local_normal
        } else {
            local_normal
        }
    }

    /// This brush's world-space bounding box, expanded by `hull`'s box, so
    /// a segment or point that falls entirely outside it cannot reach the
    /// brush's hull tree at all (the Minkowski sum of the hull box with the
    /// brush's own bounds).
    ///
    /// A rotated brush's own box is expanded by `hull` *before* rotating —
    /// matching the order the hull's own clip-tree planes were already
    /// expanded in at compile time, in the submodel's local frame, before
    /// this crate's rotation is ever applied to them — and only then
    /// re-derived as the axis-aligned box enclosing its (rotated) corners.
    /// Rotating first and adding an axis-aligned world-space hull box
    /// afterwards, tried initially, is *not* equivalent: a proptest with a
    /// non-`Z` rotation axis and the (uniformly ±32) large hull caught it
    /// under-covering a point the unrestricted hull tree still reported
    /// solid, which the broad phase must never do.
    fn broad_bounds(&self, hull: Hull) -> (Vec3, Vec3) {
        let (hull_mins, hull_maxs) = hull.bounds();
        if self.has_rotation() {
            self.rotated_world_bounds(self.mins + hull_mins, self.maxs + hull_maxs)
        } else {
            (
                self.origin + self.mins + hull_mins,
                self.origin + self.maxs + hull_maxs,
            )
        }
    }

    /// The axis-aligned box enclosing every corner of the local box
    /// `(local_mins, local_maxs)` after this brush's current rotation and
    /// translation are applied.
    fn rotated_world_bounds(&self, local_mins: Vec3, local_maxs: Vec3) -> (Vec3, Vec3) {
        if !local_mins.is_finite() || !local_maxs.is_finite() {
            // `CollisionModel::widen_brush_bounds_for_test` sets `mins`/
            // `maxs` to +/- infinity so a test can force the broad phase
            // to always pass; rotating an infinite corner produces NaN
            // (infinity times a near-zero sine/cosine component), which
            // would make every `boxes_overlap` comparison false — the
            // opposite of "always overlap". Skip the rotation and hand the
            // already all-covering box straight through.
            return (local_mins, local_maxs);
        }
        let corners = [
            Vec3::new(local_mins.x, local_mins.y, local_mins.z),
            Vec3::new(local_mins.x, local_mins.y, local_maxs.z),
            Vec3::new(local_mins.x, local_maxs.y, local_mins.z),
            Vec3::new(local_mins.x, local_maxs.y, local_maxs.z),
            Vec3::new(local_maxs.x, local_mins.y, local_mins.z),
            Vec3::new(local_maxs.x, local_mins.y, local_maxs.z),
            Vec3::new(local_maxs.x, local_maxs.y, local_mins.z),
            Vec3::new(local_maxs.x, local_maxs.y, local_maxs.z),
        ];
        let mut out_min = Vec3::splat(f32::INFINITY);
        let mut out_max = Vec3::splat(f32::NEG_INFINITY);
        for corner in corners {
            let world = self.world_point(corner);
            out_min = out_min.min(world);
            out_max = out_max.max(world);
        }
        (out_min, out_max)
    }
}

/// Whether the axis-aligned box `(a_mins, a_maxs)` overlaps `(b_mins,
/// b_maxs)` on every axis. Touching boxes (equal on an axis) count as
/// overlapping, matching the hull trees' own closed-on-the-plane
/// convention.
fn boxes_overlap(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
    a_mins.x <= b_maxs.x
        && a_maxs.x >= b_mins.x
        && a_mins.y <= b_maxs.y
        && a_maxs.y >= b_mins.y
        && a_mins.z <= b_maxs.z
        && a_maxs.z >= b_mins.z
}

/// A map's collision hulls: the validated planes plus the four hull trees.
///
/// A map's *worldspawn* hulls are only part of what a player collides with:
/// the compiler moves every brush entity (`func_wall`, `func_door`,
/// `func_plat`, `func_breakable`, ...) out of model 0 and into its own
/// `BSPMODEL`, referenced from the entity lump as `"*N"` (Valve Developer
/// Community, "BSP (GoldSrc)"; see `docs/FORMAT_SOURCES.md`). Each of those
/// submodels carries its own four head nodes into the same shared plane and
/// clipnode arrays, so a player trace has to visit the world tree *and*
/// every solid brush entity's tree and keep the nearest hit. Attach those
/// with [`Self::attach_brush`]; without them a floor built as a `func_wall`
/// is simply not there and the player falls through it.
#[derive(Debug, Clone)]
pub struct CollisionModel {
    planes: Vec<HullPlane>,
    /// Hull 0, derived from the BSP node tree with leaf references replaced
    /// by the leaves' contents values, so all four hulls traverse alike.
    point_nodes: Vec<HullNode>,
    /// Hulls 1-3, taken from the clipnodes lump.
    clip_nodes: Vec<HullNode>,
    /// Per-hull head links, in `BSPMODEL::headnodes` order.
    heads: [i32; 4],
    /// The solid brush entities that move with the map logic.
    brushes: Vec<BrushPart>,
}

fn valid_contents(value: i32) -> Result<i32, SanitizedError> {
    if (contents::MIN..=contents::EMPTY).contains(&value) {
        Ok(value)
    } else {
        Err(SanitizedError::InvalidInput)
    }
}

impl CollisionModel {
    /// Builds the collision hulls of the map's worldspawn model.
    pub fn from_bsp(bsp: &Bsp<'_>, limits: &Limits) -> Result<Self, SanitizedError> {
        Self::from_bsp_model(bsp, limits, 0)
    }

    /// Builds the collision hulls of submodel `model_index` (`0` is the
    /// worldspawn model; 1.. are the brush entities).
    pub fn from_bsp_model(
        bsp: &Bsp<'_>,
        limits: &Limits,
        model_index: usize,
    ) -> Result<Self, SanitizedError> {
        let invalid = |_| SanitizedError::InvalidInput;
        let raw_planes = bsp.planes(limits).map_err(invalid)?;
        let raw_nodes = bsp.nodes(limits).map_err(invalid)?;
        let raw_leaves = bsp.leaves(limits).map_err(invalid)?;
        let raw_clipnodes = bsp.clipnodes(limits).map_err(invalid)?;
        let raw_models = bsp.models(limits).map_err(invalid)?;
        let model = raw_models
            .get(model_index)
            .ok_or(SanitizedError::NotFound)?;

        let mut planes = Vec::with_capacity(raw_planes.len());
        for plane in raw_planes {
            let normal = Vec3::new(
                plane.normal[0].get(),
                plane.normal[1].get(),
                plane.normal[2].get(),
            );
            let dist = plane.dist.get();
            if !normal.is_finite() || !dist.is_finite() || normal.length_squared() <= 0.0 {
                return Err(SanitizedError::InvalidInput);
            }
            planes.push(HullPlane { normal, dist });
        }

        // Hull 0: the BSP node tree, with each leaf child replaced by that
        // leaf's contents so the same traversal serves every hull.
        let mut point_nodes = Vec::with_capacity(raw_nodes.len());
        for node in raw_nodes {
            let plane = node.plane.get();
            if plane as usize >= planes.len() {
                return Err(SanitizedError::InvalidInput);
            }
            let mut children = [0i32; 2];
            for (slot, raw) in children.iter_mut().zip(node.children) {
                let child = i32::from(raw.get());
                *slot = if child >= 0 {
                    if usize::try_from(child).map_err(|_| SanitizedError::InvalidInput)?
                        >= raw_nodes.len()
                    {
                        return Err(SanitizedError::InvalidInput);
                    }
                    child
                } else {
                    // Leaf indices are stored as the bitwise complement of a
                    // negative child link.
                    let leaf = raw_leaves
                        .get(usize::try_from(!child).map_err(|_| SanitizedError::InvalidInput)?)
                        .ok_or(SanitizedError::InvalidInput)?;
                    valid_contents(leaf.contents.get())?
                };
            }
            point_nodes.push(HullNode { plane, children });
        }

        let mut clip_nodes = Vec::with_capacity(raw_clipnodes.len());
        for node in raw_clipnodes {
            let plane =
                u32::try_from(node.plane.get()).map_err(|_| SanitizedError::InvalidInput)?;
            if plane as usize >= planes.len() {
                return Err(SanitizedError::InvalidInput);
            }
            let mut children = [0i32; 2];
            for (slot, raw) in children.iter_mut().zip(node.children) {
                let child = i32::from(raw.get());
                *slot = if child >= 0 {
                    if usize::try_from(child).map_err(|_| SanitizedError::InvalidInput)?
                        >= raw_clipnodes.len()
                    {
                        return Err(SanitizedError::InvalidInput);
                    }
                    child
                } else {
                    valid_contents(child)?
                };
            }
            clip_nodes.push(HullNode { plane, children });
        }

        let mut heads = [contents::EMPTY; 4];
        for (hull, head) in heads.iter_mut().enumerate() {
            let raw = model.headnodes[hull].get();
            let node_count = if hull == 0 {
                point_nodes.len()
            } else {
                clip_nodes.len()
            };
            *head = if raw >= 0 {
                if usize::try_from(raw).map_err(|_| SanitizedError::InvalidInput)? >= node_count {
                    return Err(SanitizedError::InvalidInput);
                }
                raw
            } else if hull == 0 {
                // A model whose tree is a single leaf.
                let leaf = raw_leaves
                    .get(usize::try_from(!raw).map_err(|_| SanitizedError::InvalidInput)?)
                    .ok_or(SanitizedError::InvalidInput)?;
                valid_contents(leaf.contents.get())?
            } else {
                valid_contents(raw)?
            };
        }

        Ok(Self {
            planes,
            point_nodes,
            clip_nodes,
            heads,
            brushes: Vec::new(),
        })
    }

    /// Attaches brush-entity submodel `model_index` as a solid the player
    /// collides with, standing `origin` away from where it was compiled.
    ///
    /// Only the submodel's four head links are taken: every brush entity
    /// indexes the same shared plane and clipnode arrays this model already
    /// validated, so an attached brush costs four integers and adds one
    /// extra tree walk per trace. `model_index` must name a real submodel
    /// (`1..`); `0` is the worldspawn model this type already holds and is
    /// rejected so a caller cannot double-count the world.
    ///
    /// The caller decides *which* entities are solid: `trigger_*` volumes
    /// and `func_illusionary` are documented as non-solid and must not be
    /// attached at all (see `ohl_game::brush::is_solid_brush`); `func_ladder`
    /// and `func_water` are documented as non-solid too but still mark
    /// space with their own contents (climbable, or a swimmable liquid) and
    /// should be attached with [`Self::attach_contents_brush`] instead.
    ///
    /// Refuses once [`MAX_ATTACHED_BRUSHES`] are already attached, so a
    /// pathological map cannot make every later trace arbitrarily
    /// expensive; the caller already tolerates an attach failure (a brush
    /// that could not attach simply has no collision), so this is not a
    /// new failure mode for it to handle.
    pub fn attach_brush(
        &mut self,
        bsp: &Bsp<'_>,
        limits: &Limits,
        model_index: usize,
        origin: Vec3,
    ) -> Result<BrushId, SanitizedError> {
        self.attach_brush_kind(bsp, limits, model_index, origin, BrushKind::Solid)
    }

    /// Attaches brush-entity submodel `model_index` as a non-solid contents
    /// volume reporting `kind` wherever a query lands inside its shape —
    /// `func_ladder` (`kind = `[`ContentsKind::Ladder`]) or `func_water`
    /// (`kind` from its `skin` keyvalue; see [`ContentsKind`]).
    ///
    /// Unlike [`Self::attach_brush`] this never blocks a trace: the
    /// submodel's boundary only changes what [`Self::contents_at`] (and,
    /// through it, [`Self::point_contents`]) reports at a point inside it,
    /// the same way a world-compiled water/slime/lava/ladder volume already
    /// does. A player therefore climbs or swims through the volume exactly
    /// as through a world one, while a solid brush entity or the world
    /// itself still blocks movement as before. See [`BrushKind::Contents`]
    /// for why this is implemented as a query-time remap rather than by
    /// reading the submodel's compiled contents literally.
    pub fn attach_contents_brush(
        &mut self,
        bsp: &Bsp<'_>,
        limits: &Limits,
        model_index: usize,
        origin: Vec3,
        kind: ContentsKind,
    ) -> Result<BrushId, SanitizedError> {
        self.attach_brush_kind(bsp, limits, model_index, origin, BrushKind::Contents(kind))
    }

    fn attach_brush_kind(
        &mut self,
        bsp: &Bsp<'_>,
        limits: &Limits,
        model_index: usize,
        origin: Vec3,
        kind: BrushKind,
    ) -> Result<BrushId, SanitizedError> {
        if model_index == 0 {
            return Err(SanitizedError::InvalidInput);
        }
        if !origin.is_finite() {
            return Err(SanitizedError::InvalidInput);
        }
        if self.brushes.len() >= MAX_ATTACHED_BRUSHES {
            return Err(SanitizedError::Unsupported);
        }
        let raw_models = bsp
            .models(limits)
            .map_err(|_| SanitizedError::InvalidInput)?;
        let raw_leaves = bsp
            .leaves(limits)
            .map_err(|_| SanitizedError::InvalidInput)?;
        let model = raw_models
            .get(model_index)
            .ok_or(SanitizedError::NotFound)?;

        let mins = Vec3::new(
            model.mins[0].get(),
            model.mins[1].get(),
            model.mins[2].get(),
        );
        let maxs = Vec3::new(
            model.maxs[0].get(),
            model.maxs[1].get(),
            model.maxs[2].get(),
        );
        if !mins.is_finite() || !maxs.is_finite() {
            return Err(SanitizedError::InvalidInput);
        }

        let mut heads = [contents::EMPTY; 4];
        for (hull, head) in heads.iter_mut().enumerate() {
            let raw = model.headnodes[hull].get();
            let node_count = if hull == 0 {
                self.point_nodes.len()
            } else {
                self.clip_nodes.len()
            };
            *head = if raw >= 0 {
                if usize::try_from(raw).map_err(|_| SanitizedError::InvalidInput)? >= node_count {
                    return Err(SanitizedError::InvalidInput);
                }
                raw
            } else if hull == 0 {
                let leaf = raw_leaves
                    .get(usize::try_from(!raw).map_err(|_| SanitizedError::InvalidInput)?)
                    .ok_or(SanitizedError::InvalidInput)?;
                valid_contents(leaf.contents.get())?
            } else {
                valid_contents(raw)?
            };
        }

        self.brushes.push(BrushPart {
            heads,
            origin,
            pivot: Vec3::ZERO,
            axis: Vec3::ZERO,
            angle_deg: 0.0,
            kind,
            mins,
            maxs,
        });
        Ok(BrushId(self.brushes.len() - 1))
    }

    /// Moves an attached brush entity to `origin` (its offset from where it
    /// was compiled), so a door or platform collides where it currently is.
    /// A non-finite `origin` is ignored rather than poisoning every later
    /// trace. Does not touch any rotation set by [`Self::set_brush_pose`];
    /// a brush entity that only ever translates (every mover except
    /// `func_door_rotating`/`func_rotating`) never has one to touch.
    pub fn set_brush_origin(&mut self, brush: BrushId, origin: Vec3) {
        if !origin.is_finite() {
            return;
        }
        if let Some(part) = self.brushes.get_mut(brush.0) {
            part.origin = origin;
        }
    }

    /// Moves and/or rotates an attached brush entity: `origin` is a
    /// translation offset exactly as [`Self::set_brush_origin`]'s, and
    /// `pivot`/`axis`/`angle_degrees` describe a rotation about a fixed
    /// world point — a `func_door_rotating`/`func_rotating` entity's origin
    /// keyvalue, which TWHL's wiki documents as coming from a required
    /// "origin brush" giving "the axis to rotate on" (see
    /// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic"). `axis`
    /// need not be normalized; a zero `axis` or a zero `angle_degrees`
    /// means no rotation, identical to a plain [`Self::set_brush_origin`]
    /// call. A non-finite argument leaves the brush's pose unchanged rather
    /// than poisoning every later trace, exactly as
    /// [`Self::set_brush_origin`] already does for `origin`.
    pub fn set_brush_pose(
        &mut self,
        brush: BrushId,
        origin: Vec3,
        pivot: Vec3,
        axis: Vec3,
        angle_degrees: f32,
    ) {
        if !origin.is_finite()
            || !pivot.is_finite()
            || !axis.is_finite()
            || !angle_degrees.is_finite()
        {
            return;
        }
        if let Some(part) = self.brushes.get_mut(brush.0) {
            part.origin = origin;
            part.pivot = pivot;
            part.axis = axis;
            part.angle_deg = angle_degrees;
        }
    }

    /// The world-space origin `brush` currently sits at (its offset from
    /// where it was compiled), or `Vec3::ZERO` for an out-of-range
    /// `BrushId`. Lets a caller compute a mover's per-step displacement by
    /// reading this before calling [`Self::set_brush_origin`] with the
    /// entity's new position.
    #[must_use]
    pub fn brush_origin(&self, brush: BrushId) -> Vec3 {
        self.brushes
            .get(brush.0)
            .map_or(Vec3::ZERO, |part| part.origin)
    }

    /// Whether `brush` currently carries a live rotation set by
    /// [`Self::set_brush_pose`] ([`BrushPart::has_rotation`]) rather than a
    /// plain translation (or no pose at all, for an out-of-range
    /// `BrushId`). Lets a caller distinguish a rider on a spinning
    /// `func_rotating`/`func_door_rotating` from one on a static floor or a
    /// translating `func_train`/`func_plat`/lift `func_door`, which is
    /// exactly the case [`crate::movement::ground_probe`]'s one-unit retry
    /// exists for.
    ///
    /// `has_rotation()` is not a proxy for that ambiguity, it is exactly
    /// the condition that creates it: it is the same predicate
    /// [`BrushPart::local_point`] tests to choose its inverse-rotation
    /// branch (`self.rotation().inverse() * (world - self.origin -
    /// self.pivot) + self.pivot`) over the bit-identical, rounding-free
    /// `world - self.origin` translation. So this is true exactly when a
    /// trace
    /// against `brush` goes through that rotation arithmetic — a
    /// `func_rotating` parked at a nonzero angle still has it (its planes
    /// are inverse-rotated on every trace even while not turning, so it
    /// still needs the retry), while a spin that happens to wrap through
    /// exactly `0.0` degrees, or a closed `func_door_rotating`, does not
    /// (its pose is the identity for that one tick, with no rounding to
    /// recover from, so excluding it costs nothing).
    #[must_use]
    pub fn brush_is_rotating(&self, brush: BrushId) -> bool {
        self.brushes
            .get(brush.0)
            .is_some_and(BrushPart::has_rotation)
    }

    /// Detaches an attached brush entity, so a map-logic despawn (a
    /// `func_wall` floor removed by a scripted `killtarget`, for example)
    /// stops blocking the player instead of leaving a solid the collision
    /// model has no other way to learn is gone.
    ///
    /// This does not remove the brush's slot — every other attached
    /// brush's [`BrushId`] is that brush's index into `brushes`, and
    /// shifting the array would silently repoint them at the wrong brush.
    /// Instead its head links are reduced to the same "bare contents, no
    /// tree" state a genuinely empty submodel starts in, which
    /// [`Self::trace`] and [`Self::contents_at`] already treat as a no-op,
    /// and which [`Self::brush_count`] already excludes. Detaching an
    /// already-detached (or never-attached, out-of-range) `brush` is a
    /// harmless no-op.
    pub fn detach_brush(&mut self, brush: BrushId) {
        if let Some(part) = self.brushes.get_mut(brush.0) {
            part.heads = [contents::EMPTY; 4];
        }
    }

    /// Test-only: widens `brush`'s broad-phase bounds to cover the whole
    /// coordinate space, so a test can compare [`Self::trace`] and
    /// [`Self::contents_at`] with and without the broad-phase skip against
    /// the exact same attached brush, rather than only asserting on
    /// specific hand-picked positions. Does not touch the brush's actual
    /// hull tree, so whether this was called never changes what a trace
    /// reports — only whether the broad phase gets a chance to skip the
    /// walk that finds it.
    #[cfg(feature = "test-support")]
    pub fn widen_brush_bounds_for_test(&mut self, brush: BrushId) {
        if let Some(part) = self.brushes.get_mut(brush.0) {
            part.mins = Vec3::splat(f32::NEG_INFINITY);
            part.maxs = Vec3::splat(f32::INFINITY);
        }
    }

    /// How many solid brush entities are attached and still contribute a
    /// real collision tree.
    ///
    /// A submodel whose four heads were already bare contents values at
    /// attach time (an entity compiled with no actual brush geometry, which
    /// [`Self::trace`] and [`Self::contents_at`] both treat as a no-op) and
    /// a brush [`Self::detach_brush`] has since reduced to that same state
    /// are both excluded: neither one is a brush a player can actually
    /// collide with, so counting either would overstate how much collision
    /// work an attached-brush count is meant to describe.
    #[must_use]
    pub fn brush_count(&self) -> usize {
        self.brushes.iter().filter(|part| !part.is_bare()).count()
    }

    fn nodes_of(&self, hull: Hull) -> &[HullNode] {
        if hull == Hull::Point {
            &self.point_nodes
        } else {
            &self.clip_nodes
        }
    }

    fn plane_distance(&self, node: &HullNode, point: Vec3) -> f32 {
        let plane = &self.planes[node.plane as usize];
        plane.normal.dot(point) - plane.dist
    }

    /// The contents value `point` falls in, as seen by `hull`.
    ///
    /// Precedence, nearest-wins-first within each tier: an attached solid
    /// brush entity wins over everything else (standing inside a closed
    /// `func_door` is solid even where the worldspawn tree says the space
    /// is empty); failing that, an attached contents volume
    /// (`func_ladder`/`func_water`; [`Self::attach_contents_brush`]) wins
    /// over the world (a `func_water` pool built over ordinary dry floor
    /// still swims); failing that, the world tree's own contents apply
    /// unchanged, so a world-compiled water/slime/lava/ladder volume works
    /// exactly as before this method knew about attached brushes at all. A
    /// brush that does not contain the point contributes nothing at any
    /// tier, so it can never turn water or a ladder volume back into plain
    /// empty space.
    #[must_use]
    pub fn contents_at(&self, hull: Hull, point: Vec3) -> i32 {
        for brush in &self.brushes {
            if brush.kind != BrushKind::Solid {
                continue;
            }
            if let Some(link) = self.brush_link(brush, hull, point)
                && contents::is_solid(link)
            {
                return link;
            }
        }
        for brush in &self.brushes {
            let BrushKind::Contents(kind) = brush.kind else {
                continue;
            };
            if let Some(link) = self.brush_link(brush, hull, point)
                && contents::is_present(link)
            {
                return kind.contents_value();
            }
        }
        self.walk(self.nodes_of(hull), self.heads[hull.index()], point)
    }

    /// The raw contents value `hull` sees inside `brush`'s own tree at
    /// `point`, or `None` when the brush cannot possibly contain it (a bare,
    /// boundary-less submodel, or a point outside its broad-phase bounds).
    fn brush_link(&self, brush: &BrushPart, hull: Hull, point: Vec3) -> Option<i32> {
        let head = brush.heads[hull.index()];
        if head < 0 {
            // A bare contents value, not a tree: see `trace`.
            return None;
        }
        // Broad phase: a point outside this brush's own (hull-expanded)
        // bounds cannot be inside its tree, so the walk below can only ever
        // answer "empty" for it — skip straight to that answer.
        let (mins, maxs) = brush.broad_bounds(hull);
        if !boxes_overlap(point, point, mins, maxs) {
            return None;
        }
        Some(self.walk(self.nodes_of(hull), head, brush.local_point(point)))
    }

    /// Walks `link`'s tree down to the contents value at `point`.
    fn walk(&self, nodes: &[HullNode], mut link: i32, point: Vec3) -> i32 {
        for _ in 0..MAX_TRACE_DEPTH {
            if link < 0 {
                return link;
            }
            // Every non-negative child link was bounds-checked at
            // construction, so this index is always in range.
            let node = &nodes[link.cast_unsigned() as usize];
            let side = usize::from(self.plane_distance(node, point) < 0.0);
            link = node.children[side];
        }
        // Only reachable through a cyclic tree, which construction cannot
        // detect without walking it; treat it as solid so nothing moves
        // through it.
        contents::SOLID
    }

    /// The contents value `point` falls in, as seen by the point hull.
    #[must_use]
    pub fn point_contents(&self, point: Vec3) -> i32 {
        self.contents_at(Hull::Point, point)
    }

    /// Traces the segment `start -> end` through `hull` and reports where it
    /// first entered solid.
    ///
    /// The world tree and every attached solid brush entity are traced, and
    /// the nearest of those hits is the answer: a `func_wall` floor slab
    /// stops a falling player exactly as a worldspawn floor does.
    #[must_use]
    pub fn trace(&self, hull: Hull, start: Vec3, end: Vec3) -> Trace {
        self.trace_ignoring(hull, start, end, None)
    }

    /// As [`Self::trace`], but skipping one attached brush entirely.
    ///
    /// This is what lets a brush entity be traced *as a mover*: a
    /// `func_pushable` being pushed has its own hull attached to this model
    /// like any other solid brush, so a trace of where it is about to go
    /// would otherwise start inside itself and report every move as blocked
    /// (see `ohl_engine::pushables`). `ignore` naming a detached or unknown
    /// id simply skips nothing, and the world tree is always traced.
    #[must_use]
    pub fn trace_ignoring(
        &self,
        hull: Hull,
        start: Vec3,
        end: Vec3,
        ignore: Option<BrushId>,
    ) -> Trace {
        if !start.is_finite() || !end.is_finite() {
            // Nothing sensible can be traced; report a fully blocked move so
            // callers keep the entity where it is.
            let mut trace = Trace::miss(end);
            trace.fraction = 0.0;
            trace.end_pos = start;
            trace.all_solid = true;
            trace.start_solid = true;
            trace.contents = contents::SOLID;
            return trace;
        }

        let mut trace = self.trace_tree(
            hull,
            self.heads[hull.index()],
            None,
            start,
            end,
            BrushKind::Solid,
        );
        for (index, brush) in self.brushes.iter().enumerate() {
            if ignore == Some(BrushId(index)) {
                continue;
            }
            let head = brush.heads[hull.index()];
            if head < 0 {
                // This submodel's tree for this hull is a bare contents
                // value with no plane in it, so it has no boundary and
                // bounds no volume: an "empty everywhere" brush is a no-op
                // and a "solid everywhere" one would fill the map with
                // solid, which no brush entity can be. Either way it
                // contributes nothing to the move.
                continue;
            }
            // Broad phase: skip the tree walk entirely when the segment's
            // own bounding box cannot reach this brush's (hull-expanded)
            // bounds. This can only ever rule out a miss, never a hit: a
            // segment whose box misses the brush's box cannot cross any
            // plane inside it.
            let (mins, maxs) = brush.broad_bounds(hull);
            let (seg_mins, seg_maxs) = (start.min(end), start.max(end));
            if !boxes_overlap(seg_mins, seg_maxs, mins, maxs) {
                continue;
            }
            // `brush.kind` decides whether this walk can ever report a
            // solid crossing at all: a `BrushKind::Contents` brush's raw
            // "inside the shape" leaf is remapped away from solid before
            // `recurse` ever tests it (see [`remap_leaf`]), so it can only
            // ever ride along in `combine`'s `in_water` union, never move
            // `trace.fraction` or set `start_solid`/`all_solid` — the
            // invariant a proptest checks directly.
            let hit = self.trace_tree(hull, head, Some(brush), start, end, brush.kind);
            combine(&mut trace, &hit, BrushId(index));
        }

        if trace.start_solid {
            // A move that begins inside solid goes nowhere, so the reported
            // position is where it started; this keeps
            // `end_pos == start + fraction * (end - start)` true for every
            // trace.
            trace.fraction = 0.0;
            trace.end_pos = start;
        }
        trace.contents = self.contents_at(hull, trace.end_pos);
        trace
    }

    /// Traces `start -> end` through the single tree rooted at `head`,
    /// which sits at `pose`'s current translation (and, for a rotating
    /// brush, rotation) away from where it was compiled. `pose` is `None`
    /// for the world tree, which never moves.
    ///
    /// The segment is moved into the tree's own frame, traced there, and the
    /// result moved back: a hull tree is a set of planes, so translating the
    /// query is the same as translating the tree and costs nothing per node.
    /// A rotating `pose` ([`BrushPart::has_rotation`]) instead inverse-
    /// rotates the query about the brush's pivot before the walk and
    /// rotates the hit position/normal back afterwards; every other case
    /// (`pose` absent, or present but not currently rotating) uses the
    /// original translation-only arithmetic unchanged, so no existing
    /// (non-rotating) trace's result changes by so much as a rounding bit.
    fn trace_tree(
        &self,
        hull: Hull,
        head: i32,
        pose: Option<&BrushPart>,
        start: Vec3,
        end: Vec3,
        kind: BrushKind,
    ) -> Trace {
        let rotating = pose.is_some_and(BrushPart::has_rotation);
        let offset = pose.map_or(Vec3::ZERO, |part| part.origin);
        let (local_start, local_end) = if rotating {
            let part = pose.expect("rotating implies pose is Some");
            (part.local_point(start), part.local_point(end))
        } else {
            (start - offset, end - offset)
        };
        let mut trace = Trace::miss(local_end);
        trace.all_solid = true;
        self.recurse(
            self.nodes_of(hull),
            head,
            0.0,
            1.0,
            local_start,
            local_end,
            MAX_TRACE_DEPTH,
            kind,
            &mut trace,
        );
        if trace.all_solid {
            trace.start_solid = true;
        }
        if trace.start_solid {
            trace.fraction = 0.0;
            trace.end_pos = local_start;
        }
        if rotating {
            let part = pose.expect("rotating implies pose is Some");
            let local_normal = trace.plane_normal;
            let local_dist = trace.plane_dist;
            trace.end_pos = part.world_point(trace.end_pos);
            trace.plane_normal = part.normal_to_world(local_normal);
            // Unlike a translation, a rotation does not move a plane's
            // distance from the origin by a value independent of where the
            // hit landed, so it cannot be adjusted incrementally the way
            // the translation-only branch below does. It also cannot be
            // read back off `trace.end_pos`: that point is deliberately
            // backed off the surface by `DIST_EPSILON` (and, on a
            // start-solid hit, is not on the hit plane at all), so
            // `plane_normal.dot(end_pos)` would be off by up to that
            // epsilon or arbitrarily wrong. Instead this is the exact
            // closed form for rotating a plane `n_local · x = local_dist`
            // (in the compiled frame) about `pivot` and then translating
            // by `origin`: for a world point `w`, `x = R⁻¹(w - pivot -
            // origin) + pivot`, so `n_local · x = local_dist` becomes,
            // using `n_world = R * n_local` (rotation is orthogonal, so
            // `n_local · R⁻¹ = n_world ·`), `n_world · w = local_dist +
            // n_world · (origin + pivot) - n_local · pivot`.
            trace.plane_dist = local_dist + trace.plane_normal.dot(part.origin + part.pivot)
                - local_normal.dot(part.pivot);
        } else {
            trace.end_pos += offset;
            // A plane's normal is unchanged by a translation; only its
            // distance from the origin moves with it.
            trace.plane_dist += trace.plane_normal.dot(offset);
        }
        trace
    }

    /// The recursive segment-versus-hull test.
    ///
    /// `p1`/`p2` are the endpoints of the sub-segment still being clipped and
    /// `p1f`/`p2f` their positions along the original segment. `kind`
    /// decides how a leaf's raw contents value is interpreted before it is
    /// tested for solidity (see [`remap_leaf`]): `BrushKind::Contents`
    /// makes every "inside the brush" leaf report its carried
    /// [`ContentsKind`] instead of the raw `SOLID` the submodel actually
    /// compiled to, which is never solid, so a contents-volume tree can
    /// never record a blocking crossing — only ride along in
    /// [`Trace::in_water`]. Returns `false` once the first solid crossing
    /// has been recorded, which unwinds the recursion without disturbing
    /// the result.
    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::too_many_lines,
        reason = "threading `kind` through every recursive call and remapping \
                  each leaf read is what makes a contents-volume brush never \
                  block; splitting it would only add indirection between \
                  call sites that must stay in lock-step"
    )]
    fn recurse(
        &self,
        nodes: &[HullNode],
        link: i32,
        p1f: f32,
        p2f: f32,
        p1: Vec3,
        p2: Vec3,
        depth: u32,
        kind: BrushKind,
        trace: &mut Trace,
    ) -> bool {
        if link < 0 {
            // A leaf: record what kind of space this stretch of the segment
            // passed through.
            let mapped = remap_leaf(link, kind);
            if contents::is_solid(mapped) {
                trace.start_solid = true;
            } else {
                trace.all_solid = false;
                if mapped == contents::EMPTY {
                    trace.in_open = true;
                } else {
                    trace.in_water = true;
                }
            }
            return true;
        }
        if depth == 0 {
            // Bounded traversal: treat an over-deep (cyclic) tree as solid.
            trace.start_solid = true;
            return false;
        }

        let node = &nodes[link.cast_unsigned() as usize];
        let d1 = self.plane_distance(node, p1);
        let d2 = self.plane_distance(node, p2);
        if (d1 >= 0.0 && d2 >= 0.0) || (d1 < 0.0 && d2 < 0.0) {
            // The segment stays on one side of this plane: no crossing to
            // split, just recurse into that side's own subtree.
            let side = usize::from(d1 < 0.0);
            return self.recurse(
                nodes,
                node.children[side],
                p1f,
                p2f,
                p1,
                p2,
                depth - 1,
                kind,
                trace,
            );
        }

        // The segment crosses this plane. Split it, keeping the crossing
        // point `DIST_EPSILON` short of the plane on the side it came from.
        let denominator = d1 - d2;
        let mid_fraction = if d1 < 0.0 {
            (d1 + DIST_EPSILON) / denominator
        } else {
            (d1 - DIST_EPSILON) / denominator
        };
        let mid_fraction = if mid_fraction.is_finite() {
            mid_fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mid = p1 + (p2 - p1) * mid_fraction;
        let mid_f = p1f + (p2f - p1f) * mid_fraction;

        let near = usize::from(d1 < 0.0);
        let far = 1 - near;

        // The near half first: if it is blocked, that hit is the answer.
        if !self.recurse(
            nodes,
            node.children[near],
            p1f,
            mid_f,
            p1,
            mid,
            depth - 1,
            kind,
            trace,
        ) {
            return false;
        }
        let far_contents = remap_leaf(
            self.contents_link(nodes, node.children[far], mid, depth - 1),
            kind,
        );
        if !contents::is_solid(far_contents) {
            return self.recurse(
                nodes,
                node.children[far],
                mid_f,
                p2f,
                mid,
                p2,
                depth - 1,
                kind,
                trace,
            );
        }
        if trace.all_solid {
            // The segment never left solid, so there is no surface to
            // report.
            return false;
        }

        // The crossing point is the first solid contact. Record the plane,
        // flipped so the normal points back out of the solid.
        let plane = &self.planes[node.plane as usize];
        if near == 0 {
            trace.plane_normal = plane.normal;
            trace.plane_dist = plane.dist;
        } else {
            trace.plane_normal = -plane.normal;
            trace.plane_dist = -plane.dist;
        }

        // Rounding can leave the midpoint just inside solid; back along the
        // segment until it is not.
        let mut end_fraction = mid_f;
        let mut end_point = mid;
        let mut backoff = mid_fraction;
        while contents::is_solid(remap_leaf(
            self.contents_link(nodes, link, end_point, depth),
            kind,
        )) {
            backoff -= 0.1;
            if backoff < 0.0 {
                trace.fraction = p1f;
                trace.end_pos = p1;
                return false;
            }
            end_fraction = p1f + (p2f - p1f) * backoff;
            end_point = p1 + (p2 - p1) * backoff;
        }

        trace.fraction = end_fraction.clamp(0.0, 1.0);
        trace.end_pos = end_point;
        false
    }

    /// The contents value at `point` starting from an arbitrary child link
    /// (which may already be a contents value).
    fn contents_link(&self, nodes: &[HullNode], mut link: i32, point: Vec3, depth: u32) -> i32 {
        for _ in 0..=depth {
            if link < 0 {
                return link;
            }
            // Every non-negative child link was bounds-checked at
            // construction, so this index is always in range.
            let node = &nodes[link.cast_unsigned() as usize];
            let side = usize::from(self.plane_distance(node, point) < 0.0);
            link = node.children[side];
        }
        contents::SOLID
    }
}

/// Wherever `kind` is [`BrushKind::Contents`], remaps a raw leaf contents
/// value `link` away from whatever the submodel actually compiled to
/// ("inside the shape" is ordinarily `SOLID`, "outside" `EMPTY`) to the
/// carried [`ContentsKind`]'s value — implementing the documented
/// `skin`-keyvalue override (see [`ContentsKind`]'s doc comment) at query
/// time. [`BrushKind::Solid`] and the world tree (which never has a `kind`
/// to remap) pass `link` through unchanged. `EMPTY` is never remapped
/// either way: a point strictly outside the brush's shape stays empty
/// regardless of what kind of brush it is.
fn remap_leaf(link: i32, kind: BrushKind) -> i32 {
    match kind {
        BrushKind::Solid => link,
        BrushKind::Contents(volume) => {
            if link == contents::EMPTY {
                link
            } else {
                volume.contents_value()
            }
        }
    }
}

/// Folds one attached brush's trace into the running best, which starts as
/// the world tree's own trace.
///
/// "Best" is the hit nearest the start: a move is stopped by whichever
/// solid it reaches first, so the smaller fraction (and its plane) wins.
/// `in_water` is a union — a segment that passed through open space in the
/// world and through a brush's liquid did both. A solid brush entity's hull
/// tree never reports a liquid contents itself (only the world's own
/// water/slime/lava volumes, and an attached *contents* volume, do); a
/// contents volume's own trace (see [`remap_leaf`]) can set `in_water` but,
/// by construction, never `start_solid`/`all_solid` and never lowers
/// `fraction` — [`CollisionModel::trace`]'s per-brush doc comment states
/// this as the invariant a proptest checks. `in_open` is deliberately *not*
/// unioned in: see its field doc on [`Trace`] for why that would be wrong,
/// and left to whatever the world tree's own trace already set. A start
/// inside any *solid* (world or brush) stops the move outright. `brush`
/// names which attached brush produced `hit`, recorded on
/// [`Trace::brush_index`] when `hit` wins the fraction comparison outright,
/// and also — even without winning that comparison — the first time a
/// brush is the one whose segment started inside solid: a `start_solid`
/// hit's own `fraction` is already forced to `0.0` (see
/// [`CollisionModel::trace_tree`]), so a brush that reports it can only
/// ever *tie* the fraction comparison against a world trace that is
/// start-solid too, never win it outright, and a caller that needs to know
/// "is the player embedded in an attached brush at all" (a mover push,
/// say) must not have that answer silently lost to a tie.
/// [`Option::get_or_insert`] means only the *first* such brush is recorded
/// when more than one embeds the segment, a deterministic but otherwise
/// arbitrary choice among ties.
fn combine(best: &mut Trace, hit: &Trace, brush: BrushId) {
    best.in_water |= hit.in_water;
    best.all_solid |= hit.all_solid;
    if hit.start_solid {
        best.start_solid = true;
        best.brush_index.get_or_insert(brush);
    }
    if hit.fraction < best.fraction {
        best.fraction = hit.fraction;
        best.end_pos = hit.end_pos;
        best.plane_normal = hit.plane_normal;
        best.plane_dist = hit.plane_dist;
        best.brush_index = Some(brush);
    }
}

/// Traces `start -> end` through `model`'s hull `hull_index`.
///
/// A `hull_index` above 3 has no hull to trace and reports a blocked move.
#[must_use]
pub fn trace_hull(model: &CollisionModel, hull_index: usize, start: Vec3, end: Vec3) -> Trace {
    if let Some(hull) = Hull::from_index(hull_index) {
        model.trace(hull, start, end)
    } else {
        let mut trace = Trace::miss(start);
        trace.fraction = 0.0;
        trace.start_solid = true;
        trace.all_solid = true;
        trace.contents = contents::SOLID;
        trace
    }
}

/// The contents value at `point`, as seen by the point hull.
#[must_use]
pub fn point_contents(model: &CollisionModel, point: Vec3) -> i32 {
    model.point_contents(point)
}
