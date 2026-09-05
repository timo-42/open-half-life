//! A* over a [`NodeGraph`], plus the straight-line shortcut.
//!
//! The search runs on the subgraph a single hull may use: a link is only
//! relaxed when its [`Link::allows`] bit for that hull is set, so a route
//! valid for a 32x32x72 humanoid and invalid for a 64x64x64 monster comes
//! out of the same graph as two different answers.
//!
//! The heuristic is the Euclidean distance between node anchors. Link costs
//! are the Euclidean distances between the same anchors, so the heuristic is
//! admissible and consistent and A* returns an optimal route.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull};

use crate::graph::{Link, NodeGraph};

/// A route: the node sequence and the positions to steer through.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Positions to move through, in order, ending at the requested goal
    /// position. Always non-empty for a returned path.
    pub waypoints: Vec<Vec3>,
    /// The graph nodes the route uses, in order. Empty for a straight-line
    /// path produced by [`straight_path_if_clear`].
    pub nodes: Vec<u32>,
    /// Total route length: the endpoint attachments plus the link costs.
    pub cost: f32,
    /// How many nodes the search expanded, always `<=`
    /// [`PathLimits::max_explored`].
    pub explored: usize,
}

impl Path {
    /// The final waypoint, i.e. the goal.
    #[must_use]
    pub fn goal(&self) -> Option<Vec3> {
        self.waypoints.last().copied()
    }

    /// Whether the route has any waypoints at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }

    /// The number of waypoints.
    #[must_use]
    pub fn len(&self) -> usize {
        self.waypoints.len()
    }
}

/// Bounds for one query. All defaults are project choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathLimits {
    /// Ceiling on expanded nodes; the search gives up (returns `None`) once
    /// it is exceeded, so a query on a huge graph costs a bounded amount.
    /// Default 4,096.
    pub max_explored: usize,
    /// How far from the start and goal positions a node may be and still be
    /// used as the attachment point. Default 512 units.
    pub attach_radius: f32,
    /// How many of the nearest in-range nodes are trace-tested per endpoint
    /// before the attachment is given up on. Default 16.
    pub max_attach_candidates: usize,
}

impl Default for PathLimits {
    fn default() -> Self {
        Self {
            max_explored: 4096,
            attach_radius: 512.0,
            max_attach_candidates: 16,
        }
    }
}

/// The A* open-set entry. `Ord` is reversed on the f-score so the standard
/// max-heap pops the cheapest node; ties break on the node index, which
/// keeps the search deterministic.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Open {
    estimate: f32,
    node: u32,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A straight move from `from` to `to` if `hull` can make it in one sweep.
///
/// This is the cheap common case (an enemy in the same room) and the
/// intended first call before [`find_path`]; it is deliberately *not* run
/// inside `find_path`, so a caller that wants the graph route always gets
/// the graph route.
#[must_use]
pub fn straight_path_if_clear(
    collision: &CollisionModel,
    from: Vec3,
    to: Vec3,
    hull: Hull,
) -> Option<Path> {
    if !from.is_finite() || !to.is_finite() {
        return None;
    }
    let trace = collision.trace(hull, from, to);
    if trace.blocked() {
        return None;
    }
    Some(Path {
        waypoints: vec![to],
        nodes: Vec::new(),
        cost: (to - from).length(),
        explored: 0,
    })
}

/// Finds a route from `from` to `to` for `hull`.
///
/// Both endpoints are first attached to the nearest graph node the hull can
/// actually reach in a straight trace ([`NodeGraph::nearest_reachable`]).
/// A* then runs over the hull's link subgraph. The returned waypoints are
/// the attached start node, every node on the route, and finally `to`.
///
/// Returns `None` when either endpoint cannot be attached, when the goal is
/// unreachable for this hull, or when the search would expand more than
/// [`PathLimits::max_explored`] nodes.
#[must_use]
pub fn find_path(
    graph: &NodeGraph,
    collision: &CollisionModel,
    from: Vec3,
    to: Vec3,
    hull: Hull,
    limits: &PathLimits,
) -> Option<Path> {
    if !from.is_finite() || !to.is_finite() {
        return None;
    }
    let start = graph.nearest_reachable(
        from,
        hull,
        collision,
        limits.attach_radius,
        limits.max_attach_candidates,
    )?;
    let goal = graph.nearest_reachable(
        to,
        hull,
        collision,
        limits.attach_radius,
        limits.max_attach_candidates,
    )?;

    let (route, route_cost, explored) = search(graph, start, goal, hull, limits)?;

    let mut waypoints = Vec::with_capacity(route.len() + 1);
    for node in &route {
        waypoints.push(graph.waypoint(*node, hull)?);
    }
    let attach_start = (waypoints.first().copied()? - from).length();
    let attach_goal = (to - waypoints.last().copied()?).length();
    waypoints.push(to);

    Some(Path {
        waypoints,
        nodes: route,
        cost: attach_start + route_cost + attach_goal,
        explored,
    })
}

/// The A* core: returns the node route, its cost and the expansion count.
fn search(
    graph: &NodeGraph,
    start: u32,
    goal: u32,
    hull: Hull,
    limits: &PathLimits,
) -> Option<(Vec<u32>, f32, usize)> {
    let count = graph.node_count();
    let start_index = start as usize;
    let goal_index = goal as usize;
    if start_index >= count || goal_index >= count {
        return None;
    }
    if start == goal {
        return Some((vec![start], 0.0, 0));
    }

    let heuristic = |node: u32| -> f32 {
        match (graph.node(node), graph.node(goal)) {
            (Some(node), Some(goal)) => (goal.position - node.position).length(),
            _ => 0.0,
        }
    };

    let mut best = vec![f32::INFINITY; count];
    let mut came_from = vec![u32::MAX; count];
    let mut closed = vec![false; count];
    let mut open = BinaryHeap::new();
    best[start_index] = 0.0;
    open.push(Open {
        estimate: heuristic(start),
        node: start,
    });

    let mut explored = 0usize;
    while let Some(Open { node, .. }) = open.pop() {
        let index = node as usize;
        if closed[index] {
            continue;
        }
        closed[index] = true;
        explored += 1;
        if explored > limits.max_explored {
            return None;
        }
        if node == goal {
            return Some((reconstruct(&came_from, start, goal), best[index], explored));
        }

        let node_cost = best[index];
        for link in graph.links_from(node) {
            if !usable(link, hull, count) {
                continue;
            }
            let next = link.to as usize;
            let candidate = node_cost + link.cost;
            if candidate < best[next] {
                best[next] = candidate;
                came_from[next] = node;
                open.push(Open {
                    estimate: candidate + heuristic(link.to),
                    node: link.to,
                });
            }
        }
    }
    None
}

/// Whether a link may be relaxed: usable by `hull`, in range, and finite.
fn usable(link: &Link, hull: Hull, count: usize) -> bool {
    link.allows(hull) && (link.to as usize) < count && link.cost.is_finite() && link.cost >= 0.0
}

/// Walks the predecessor table back from `goal`, returning `start..=goal`.
fn reconstruct(came_from: &[u32], start: u32, goal: u32) -> Vec<u32> {
    let mut route = vec![goal];
    let mut current = goal;
    // Each step moves to a strictly earlier node in the search tree, so the
    // node count bounds the walk; the guard makes that explicit anyway.
    for _ in 0..came_from.len() {
        if current == start {
            break;
        }
        let Some(previous) = came_from.get(current as usize).copied() else {
            break;
        };
        if previous == u32::MAX {
            break;
        }
        route.push(previous);
        current = previous;
    }
    route.reverse();
    route
}
