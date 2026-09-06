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

use glam::Vec3;
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

/// Identifies one solid brush entity attached to a [`CollisionModel`] with
/// [`CollisionModel::attach_brush`], so its origin can be updated as the
/// map logic moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BrushId(usize);

/// One solid brush entity's hulls: the head links of its `BSPMODEL` plus
/// the world-space offset the map logic has moved it by since the map was
/// compiled.
#[derive(Debug, Clone, Copy)]
struct BrushPart {
    heads: [i32; 4],
    origin: Vec3,
    /// The submodel's own compiled bounding box (`BSPMODEL::mins/maxs`),
    /// relative to the same frame [`Self::heads`]'s trees are in (i.e.
    /// before [`Self::origin`] is added). Used only for the broad-phase
    /// check in [`CollisionModel::trace`] and
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

    /// This brush's world-space bounding box, expanded by `hull`'s box, so
    /// a segment or point that falls entirely outside it cannot reach the
    /// brush's hull tree at all (the Minkowski sum of the hull box with the
    /// brush's own bounds).
    fn broad_bounds(&self, hull: Hull) -> (Vec3, Vec3) {
        let (hull_mins, hull_maxs) = hull.bounds();
        (
            self.origin + self.mins + hull_mins,
            self.origin + self.maxs + hull_maxs,
        )
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
    /// The caller decides *which* entities are solid: `trigger_*` volumes,
    /// `func_illusionary` and `func_ladder` are documented as non-solid and
    /// must not be attached (see `ohl_game::brush::is_solid_brush`).
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
            mins,
            maxs,
        });
        Ok(BrushId(self.brushes.len() - 1))
    }

    /// Moves an attached brush entity to `origin` (its offset from where it
    /// was compiled), so a door or platform collides where it currently is.
    /// A non-finite `origin` is ignored rather than poisoning every later
    /// trace.
    pub fn set_brush_origin(&mut self, brush: BrushId, origin: Vec3) {
        if !origin.is_finite() {
            return;
        }
        if let Some(part) = self.brushes.get_mut(brush.0) {
            part.origin = origin;
        }
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
    /// An attached solid brush entity wins over the world: standing inside
    /// a closed `func_door` is solid even where the worldspawn tree says
    /// the space is empty. A brush that does *not* contain the point
    /// contributes nothing, so it can never turn world water or a ladder
    /// volume back into plain empty space.
    #[must_use]
    pub fn contents_at(&self, hull: Hull, point: Vec3) -> i32 {
        for brush in &self.brushes {
            let head = brush.heads[hull.index()];
            if head < 0 {
                // A bare contents value, not a tree: see `trace`.
                continue;
            }
            // Broad phase: a point outside this brush's own (hull-expanded)
            // bounds cannot be inside its tree, so the walk below can only
            // ever answer "empty" for it — skip straight to that answer.
            let (mins, maxs) = brush.broad_bounds(hull);
            if !boxes_overlap(point, point, mins, maxs) {
                continue;
            }
            let link = self.walk(self.nodes_of(hull), head, point - brush.origin);
            if contents::is_solid(link) {
                return link;
            }
        }
        self.walk(self.nodes_of(hull), self.heads[hull.index()], point)
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

        let mut trace = self.trace_tree(hull, self.heads[hull.index()], Vec3::ZERO, start, end);
        for brush in &self.brushes {
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
            let hit = self.trace_tree(hull, head, brush.origin, start, end);
            combine(&mut trace, &hit);
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
    /// which sits `offset` away from where it was compiled.
    ///
    /// The segment is moved into the tree's own frame, traced there, and the
    /// result moved back: a hull tree is a set of planes, so translating the
    /// query is the same as translating the tree and costs nothing per node.
    fn trace_tree(&self, hull: Hull, head: i32, offset: Vec3, start: Vec3, end: Vec3) -> Trace {
        let (local_start, local_end) = (start - offset, end - offset);
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
            &mut trace,
        );
        if trace.all_solid {
            trace.start_solid = true;
        }
        if trace.start_solid {
            trace.fraction = 0.0;
            trace.end_pos = local_start;
        }
        trace.end_pos += offset;
        // A plane's normal is unchanged by a translation; only its distance
        // from the origin moves with it.
        trace.plane_dist += trace.plane_normal.dot(offset);
        trace
    }

    /// The recursive segment-versus-hull test.
    ///
    /// `p1`/`p2` are the endpoints of the sub-segment still being clipped and
    /// `p1f`/`p2f` their positions along the original segment. Returns
    /// `false` once the first solid crossing has been recorded, which
    /// unwinds the recursion without disturbing the result.
    #[allow(clippy::too_many_arguments)]
    fn recurse(
        &self,
        nodes: &[HullNode],
        link: i32,
        p1f: f32,
        p2f: f32,
        p1: Vec3,
        p2: Vec3,
        depth: u32,
        trace: &mut Trace,
    ) -> bool {
        if link < 0 {
            // A leaf: record what kind of space this stretch of the segment
            // passed through.
            if contents::is_solid(link) {
                trace.start_solid = true;
            } else {
                trace.all_solid = false;
                if link == contents::EMPTY {
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
        if d1 >= 0.0 && d2 >= 0.0 {
            return self.recurse(nodes, node.children[0], p1f, p2f, p1, p2, depth - 1, trace);
        }
        if d1 < 0.0 && d2 < 0.0 {
            return self.recurse(nodes, node.children[1], p1f, p2f, p1, p2, depth - 1, trace);
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
            trace,
        ) {
            return false;
        }
        if !contents::is_solid(self.contents_link(nodes, node.children[far], mid, depth - 1)) {
            return self.recurse(
                nodes,
                node.children[far],
                mid_f,
                p2f,
                mid,
                p2,
                depth - 1,
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
        while contents::is_solid(self.contents_link(nodes, link, end_point, depth)) {
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

/// Folds one attached brush's trace into the running best, which starts as
/// the world tree's own trace.
///
/// "Best" is the hit nearest the start: a move is stopped by whichever
/// solid it reaches first, so the smaller fraction (and its plane) wins.
/// `in_water` is a union — a segment that passed through open space in the
/// world and through a brush's liquid did both, and a solid brush entity's
/// hull tree never reports a liquid contents anyway (only the world's
/// water/slime/lava volumes do), so this never fires from an attached
/// brush in practice. `in_open` is deliberately *not* unioned in: see its
/// field doc on [`Trace`] for why that would be wrong, and left to whatever
/// the world tree's own trace already set. A start inside *any* solid
/// (world or brush) stops the move outright.
fn combine(best: &mut Trace, hit: &Trace) {
    best.in_water |= hit.in_water;
    best.all_solid |= hit.all_solid;
    if hit.start_solid {
        best.start_solid = true;
    }
    if hit.fraction < best.fraction {
        best.fraction = hit.fraction;
        best.end_pos = hit.end_pos;
        best.plane_normal = hit.plane_normal;
        best.plane_dist = hit.plane_dist;
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
