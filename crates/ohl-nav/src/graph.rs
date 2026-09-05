//! The navigation node graph: ground snapping, per-hull link validation and
//! bounded, deterministic construction.
//!
//! A node is a hand-placed point in the map (`info_node` on the floor,
//! `info_node_air` in mid-air). The graph adds the edges: for every pair of
//! nodes closer than [`BuildLimits::link_radius`], the builder asks the
//! collision model, once per hull, whether a monster of that size can
//! actually travel between them, and stores the answer as a four-bit mask.
//! Path queries then only ever see the links their hull can use.
//!
//! Every numeric default in [`BuildLimits`] is a project choice, documented
//! on the field; none of them is a published Half-Life value.

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull, contents};

/// What a node is anchored to, which decides how links to it are validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NodeKind {
    /// A walking node. It is snapped down onto the floor at build time and
    /// its links are validated as walk moves (step up, walk across, drop).
    Ground,
    /// A flying node, used exactly where it was placed. Links are validated
    /// with a single hull trace through open space.
    Air,
    /// A swimming node, treated like [`NodeKind::Air`] for link validation
    /// (a swimmer needs no floor) but kept distinct so a host can restrict
    /// it to monsters that can enter liquids.
    Water,
}

impl NodeKind {
    /// Whether nodes of this kind are dropped onto the floor at build time.
    #[must_use]
    pub const fn is_grounded(self) -> bool {
        matches!(self, Self::Ground)
    }
}

/// One node as supplied by the caller, before ground snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeSeed {
    /// The entity origin the node was placed at.
    pub position: Vec3,
    /// Which kind of movement the node serves.
    pub kind: NodeKind,
}

impl NodeSeed {
    /// A seed at `position`.
    #[must_use]
    pub const fn new(position: Vec3, kind: NodeKind) -> Self {
        Self { position, kind }
    }
}

/// One node in a built graph.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Node {
    /// The node anchor. For [`NodeKind::Ground`] this is the point on the
    /// floor the node was snapped to (so it is hull-independent); for the
    /// other kinds it is the seed position unchanged. Use
    /// [`NodeGraph::waypoint`] to turn it into an entity origin for a hull.
    pub position: Vec3,
    /// The kind the seed asked for.
    pub kind: NodeKind,
    /// Whether ground snapping found a floor. A grounded node that found no
    /// floor keeps its seed position and will usually end up with no links.
    pub snapped: bool,
}

/// One directed link, stored in the source node's adjacency slice.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Link {
    /// The destination node index.
    pub to: u32,
    /// Travel cost, the Euclidean distance between the two node anchors.
    pub cost: f32,
    /// Bit `h` is set when [`Hull::from_index(h)`](Hull::from_index) can use
    /// this link. See [`Link::allows`].
    pub hull_mask: u8,
}

impl Link {
    /// Whether `hull` may traverse this link.
    #[must_use]
    pub const fn allows(&self, hull: Hull) -> bool {
        self.hull_mask & (1u8 << hull.index()) != 0
    }
}

/// The four hulls, in index order, so builders and queries agree.
pub const HULLS: [Hull; 4] = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];

/// How far above the floor a ground node's hull is placed while tracing, so
/// a trace that starts exactly on the floor plane is not reported as solid.
/// A project choice, comfortably above `ohl_physics::DIST_EPSILON`.
pub const GROUND_CLEARANCE: f32 = 1.0;

/// Bounds and tolerances for [`NodeGraph::build`].
///
/// Every default is a project choice made for this implementation. The two
/// distance tolerances are sized against the published GoldSrc hull table
/// (`docs/FORMAT_SOURCES.md`, "Collision hulls and player movement"): the
/// step height matches the movement code's step-up allowance, and the link
/// radius is a few humanoid hull widths.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BuildLimits {
    /// Only node pairs no further apart than this are considered for a link.
    /// Default 512 units.
    pub link_radius: f32,
    /// Seeds past this count are dropped. Default 4,096.
    pub max_nodes: usize,
    /// Each node keeps at most this many outgoing links, the cheapest first.
    /// Default 8.
    pub max_links_per_node: usize,
    /// Hard ceiling on the unordered node pairs the builder is willing to
    /// trace, so construction cost stays bounded even for a pathological
    /// seed cloud. Default 262,144.
    pub max_candidate_pairs: usize,
    /// How far a walking monster may step up while traversing a link, and
    /// the tolerance the far end of a ground link must land within. Default
    /// 18 units, the documented GoldSrc step height.
    pub step_height: f32,
    /// How far a walking monster may drop along a link before the link is
    /// rejected. Default 64 units.
    pub max_drop: f32,
    /// How far down a ground seed is searched for a floor. Default 256
    /// units.
    pub ground_snap_distance: f32,
    /// Spacing of the floor-support samples taken along a ground link.
    /// Default 32 units, one humanoid hull width.
    pub ground_sample_spacing: f32,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            link_radius: 512.0,
            max_nodes: 4096,
            max_links_per_node: 8,
            max_candidate_pairs: 262_144,
            step_height: 18.0,
            max_drop: 64.0,
            ground_snap_distance: 256.0,
            ground_sample_spacing: 32.0,
        }
    }
}

impl BuildLimits {
    fn sanitized(&self) -> Self {
        let positive = |value: f32, fallback: f32| {
            if value.is_finite() && value > 0.0 {
                value
            } else {
                fallback
            }
        };
        let default = Self::default();
        Self {
            link_radius: positive(self.link_radius, default.link_radius),
            max_nodes: self.max_nodes,
            max_links_per_node: self.max_links_per_node,
            max_candidate_pairs: self.max_candidate_pairs,
            step_height: positive(self.step_height, default.step_height),
            max_drop: positive(self.max_drop, default.max_drop),
            ground_snap_distance: positive(self.ground_snap_distance, default.ground_snap_distance),
            ground_sample_spacing: positive(
                self.ground_sample_spacing,
                default.ground_sample_spacing,
            )
            .max(1.0),
        }
    }
}

/// A built navigation graph: nodes plus per-hull-validated directed links.
///
/// Construction is deterministic — the node order is the seed order, and
/// each node's links are sorted by cost and then by destination index — so
/// the same seeds and collision model always produce the same graph, and a
/// serialised graph (feature `serde`) can be cached and compared.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeGraph {
    nodes: Vec<Node>,
    /// All adjacency slices, concatenated in node order.
    links: Vec<Link>,
    /// `offsets[i]..offsets[i + 1]` is node `i`'s slice of `links`; always
    /// `nodes.len() + 1` long.
    offsets: Vec<u32>,
}

impl NodeGraph {
    /// Builds the graph for `seeds` against `collision`.
    ///
    /// Ground and water seeds are dropped onto the floor (water seeds only
    /// so their anchor is stable; their links are still validated as swim
    /// moves), air seeds are left where they are. Then every unordered node
    /// pair within [`BuildLimits::link_radius`] is validated once per hull,
    /// in both directions, and each node keeps its cheapest
    /// [`BuildLimits::max_links_per_node`] links.
    #[must_use]
    pub fn build(seeds: &[NodeSeed], collision: &CollisionModel, limits: &BuildLimits) -> Self {
        let limits = limits.sanitized();
        let mut nodes: Vec<Node> = Vec::new();
        for seed in seeds.iter().take(limits.max_nodes) {
            if !seed.position.is_finite() {
                continue;
            }
            nodes.push(snap(*seed, collision, &limits));
        }

        let mut adjacency: Vec<Vec<Link>> = vec![Vec::new(); nodes.len()];
        let radius_squared = limits.link_radius * limits.link_radius;
        let mut budget = limits.max_candidate_pairs;
        'outer: for a in 0..nodes.len() {
            for b in (a + 1)..nodes.len() {
                let delta = nodes[b].position - nodes[a].position;
                let distance_squared = delta.length_squared();
                if distance_squared > radius_squared || distance_squared <= 0.0 {
                    continue;
                }
                if budget == 0 {
                    break 'outer;
                }
                budget -= 1;
                let cost = distance_squared.sqrt();
                let forward = hull_mask(&nodes[a], &nodes[b], collision, &limits);
                if forward != 0 {
                    adjacency[a].push(Link {
                        to: u32::try_from(b).unwrap_or(u32::MAX),
                        cost,
                        hull_mask: forward,
                    });
                }
                let backward = hull_mask(&nodes[b], &nodes[a], collision, &limits);
                if backward != 0 {
                    adjacency[b].push(Link {
                        to: u32::try_from(a).unwrap_or(u32::MAX),
                        cost,
                        hull_mask: backward,
                    });
                }
            }
        }

        let mut links = Vec::new();
        let mut offsets = Vec::with_capacity(nodes.len() + 1);
        offsets.push(0);
        for list in &mut adjacency {
            list.sort_by(|left, right| {
                left.cost
                    .total_cmp(&right.cost)
                    .then(left.to.cmp(&right.to))
            });
            list.truncate(limits.max_links_per_node);
            links.extend_from_slice(list);
            offsets.push(u32::try_from(links.len()).unwrap_or(u32::MAX));
        }

        Self {
            nodes,
            links,
            offsets,
        }
    }

    /// Every node, in seed order.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The number of nodes.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The total number of directed links.
    #[must_use]
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Node `index`, or `None` when it is out of range.
    #[must_use]
    pub fn node(&self, index: u32) -> Option<&Node> {
        self.nodes.get(index as usize)
    }

    /// Node `index`'s outgoing links, cheapest first. An out-of-range index
    /// yields an empty slice.
    #[must_use]
    pub fn links_from(&self, index: u32) -> &[Link] {
        let index = index as usize;
        let (Some(start), Some(end)) = (self.offsets.get(index), self.offsets.get(index + 1))
        else {
            return &[];
        };
        let (start, end) = (*start as usize, *end as usize);
        self.links.get(start..end).unwrap_or(&[])
    }

    /// Whether a link from `from` to `to` exists for `hull`.
    #[must_use]
    pub fn has_link(&self, from: u32, to: u32, hull: Hull) -> bool {
        self.links_from(from)
            .iter()
            .any(|link| link.to == to && link.allows(hull))
    }

    /// The entity origin a monster using `hull` should aim at to stand on
    /// node `index`: the anchor lifted by the hull's foot offset for a
    /// grounded node, and the anchor itself otherwise.
    #[must_use]
    pub fn waypoint(&self, index: u32, hull: Hull) -> Option<Vec3> {
        self.nodes
            .get(index as usize)
            .map(|node| waypoint_of(node, hull))
    }

    /// The node nearest to `position` that `hull` can reach in a straight
    /// hull trace, searching the `max_candidates` nearest nodes within
    /// `radius`.
    ///
    /// Returns `None` when no node is both in range and reachable.
    #[must_use]
    pub fn nearest_reachable(
        &self,
        position: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        radius: f32,
        max_candidates: usize,
    ) -> Option<u32> {
        if !position.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        let radius_squared = radius * radius;
        let mut candidates: Vec<(f32, u32)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                let index = u32::try_from(index).ok()?;
                let distance = (waypoint_of(node, hull) - position).length_squared();
                (distance <= radius_squared).then_some((distance, index))
            })
            .collect();
        candidates.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));

        candidates
            .into_iter()
            .take(max_candidates)
            .find(|(_, index)| {
                let Some(node) = self.nodes.get(*index as usize) else {
                    return false;
                };
                clear(collision, hull, position, waypoint_of(node, hull))
            })
            .map(|(_, index)| index)
    }
}

/// The entity origin for `node` under `hull`.
fn waypoint_of(node: &Node, hull: Hull) -> Vec3 {
    if node.kind.is_grounded() {
        node.position + Vec3::Z * hull.foot_offset()
    } else {
        node.position
    }
}

/// Whether a hull can move from `start` to `end` without hitting anything.
fn clear(collision: &CollisionModel, hull: Hull, start: Vec3, end: Vec3) -> bool {
    if !start.is_finite() || !end.is_finite() {
        return false;
    }
    let trace = collision.trace(hull, start, end);
    !trace.blocked()
}

/// Drops a grounded seed onto the floor beneath it.
fn snap(seed: NodeSeed, collision: &CollisionModel, limits: &BuildLimits) -> Node {
    if !seed.kind.is_grounded() {
        return Node {
            position: seed.position,
            kind: seed.kind,
            snapped: false,
        };
    }
    // Trace the point hull down from just above the seed. The anchor that
    // results is hull-independent, which is what lets one graph serve all
    // four hulls.
    let start = seed.position + Vec3::Z * GROUND_CLEARANCE;
    let end = start - Vec3::Z * (limits.ground_snap_distance + GROUND_CLEARANCE);
    let trace = collision.trace(Hull::Point, start, end);
    if trace.start_solid || trace.all_solid || trace.fraction >= 1.0 {
        return Node {
            position: seed.position,
            kind: seed.kind,
            snapped: false,
        };
    }
    Node {
        position: trace.end_pos,
        kind: seed.kind,
        snapped: true,
    }
}

/// Validates a directed link `from -> to` for every hull, returning the mask.
fn hull_mask(from: &Node, to: &Node, collision: &CollisionModel, limits: &BuildLimits) -> u8 {
    let mut mask = 0u8;
    for hull in HULLS {
        if link_valid(from, to, hull, collision, limits) {
            mask |= 1u8 << hull.index();
        }
    }
    mask
}

/// Whether `hull` can travel the link `from -> to`.
///
/// Ground nodes and air/water nodes never link to each other, matching the
/// published behaviour of `info_node` and `info_node_air` (they are dropped
/// to the floor and left in place respectively, and do not link across);
/// see `docs/FORMAT_SOURCES.md`, "Navigation".
///
/// A link between two non-grounded nodes is a swim or flight move: one hull
/// trace between the two waypoints has to be clear. A ground-to-ground link
/// is a walk move, validated the way the movement code walks: lift the hull
/// by the step height, sweep it horizontally to the far end, require floor
/// support under every sample along the way, and require the drop at the far
/// end to land within the step tolerance of the destination node's floor.
fn link_valid(
    from: &Node,
    to: &Node,
    hull: Hull,
    collision: &CollisionModel,
    limits: &BuildLimits,
) -> bool {
    let start = waypoint_of(from, hull);
    let end = waypoint_of(to, hull);
    if !start.is_finite() || !end.is_finite() {
        return false;
    }

    if from.kind.is_grounded() != to.kind.is_grounded() {
        return false;
    }
    if !from.kind.is_grounded() {
        return clear(collision, hull, start, end);
    }
    if !(from.snapped && to.snapped) {
        return false;
    }

    let lift = Vec3::Z * (GROUND_CLEARANCE + limits.step_height);
    let lifted_start = start + lift;
    let lifted_end = end + lift;
    if contents::is_solid(collision.contents_at(hull, lifted_start))
        || contents::is_solid(collision.contents_at(hull, lifted_end))
    {
        return false;
    }
    // Step up into the lifted lane, sweep across it, then drop.
    if !clear(
        collision,
        hull,
        start + Vec3::Z * GROUND_CLEARANCE,
        lifted_start,
    ) || !clear(collision, hull, lifted_start, lifted_end)
    {
        return false;
    }

    let drop = limits.step_height + limits.max_drop + GROUND_CLEARANCE;
    let horizontal = (lifted_end - lifted_start).length();
    let samples = sample_count(horizontal, limits.ground_sample_spacing);
    for sample in 1..=samples {
        let fraction = f64::from(sample) / f64::from(samples);
        let point = lifted_start + (lifted_end - lifted_start) * fraction_as_f32(fraction);
        let floor = collision.trace(hull, point, point - Vec3::Z * drop);
        if floor.start_solid || floor.all_solid || floor.fraction >= 1.0 {
            // No floor within the allowed drop: the link crosses a gap.
            return false;
        }
        // The final drop must arrive at the destination node's own floor,
        // not on a ledge above it or in a pit below it.
        if sample == samples
            && (floor.end_pos.z - end.z).abs() > limits.step_height + GROUND_CLEARANCE
        {
            return false;
        }
    }
    true
}

/// How many floor-support samples a ground link of `length` needs at
/// `spacing`, clamped so a degenerate spacing cannot explode the loop.
fn sample_count(length: f32, spacing: f32) -> u32 {
    let count = (length / spacing).ceil();
    if !count.is_finite() || count <= 1.0 {
        return 1;
    }
    if count >= 4096.0 {
        return 4096;
    }
    // In range `1.0..4096.0` and integral, so the conversion is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        count as u32
    }
}

/// Narrows an interpolation parameter in `0.0..=1.0`; the loss of precision
/// is the intended narrowing, not an accident.
fn fraction_as_f32(fraction: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        fraction as f32
    }
}

/// Collects navigation seeds from an already-parsed BSP entities lump.
///
/// Recognises the two published node entities, `info_node` (a ground node)
/// and `info_node_air` (a flying node); see `docs/FORMAT_SOURCES.md`,
/// "Navigation". Entities without a parseable `origin` are skipped, and at
/// most `max_nodes` seeds are returned. A host that already has typed
/// entities (`ohl-game`) should build [`NodeSeed`]s itself instead — this
/// crate deliberately does not depend on the entity layer.
#[must_use]
pub fn node_seeds_from_entities(
    entities: &[ohl_formats::bsp30::Entity],
    max_nodes: usize,
) -> Vec<NodeSeed> {
    let mut seeds = Vec::new();
    for entity in entities {
        if seeds.len() >= max_nodes {
            break;
        }
        let kind = match entity.get("classname").map(String::as_str) {
            Some("info_node") => NodeKind::Ground,
            Some("info_node_air") => NodeKind::Air,
            _ => continue,
        };
        let Some(origin) = entity.get("origin").and_then(|value| parse_origin(value)) else {
            continue;
        };
        seeds.push(NodeSeed::new(origin, kind));
    }
    seeds
}

/// Parses an `"x y z"` origin keyvalue.
fn parse_origin(value: &str) -> Option<Vec3> {
    let mut parts = value.split_ascii_whitespace();
    let mut coordinate = || -> Option<f32> {
        parts
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
    };
    let (x, y, z) = (coordinate()?, coordinate()?, coordinate()?);
    Some(Vec3::new(x, y, z))
}
