//! The bots' walkable-floor graph: a coarse grid of standing positions
//! flood-filled over the live collision model, with the edges a walking
//! player can take between them, and an A* search over it.
//!
//! Deathmatch maps rarely place `info_node` entities (the node graph
//! `crate::nav` builds for monsters is usually absent there), so bots do
//! not use it. Instead this module reuses [`crate::reachability`]'s own
//! ascend/move/drop edge test ([`try_edge`]) — the same step-up, jump and
//! fall shapes the route-triage walk already measures against the walking
//! player's own [`Hull::Standing`] — and keeps the edges it finds rather
//! than only the cells, so a bot can ask for a path instead of only
//! whether a place is reachable.
//!
//! Every bound here is project-authored: the grid pitch, the node cap, the
//! jump distances tried and the edge costs are tuning, not engine facts.
//! The fall bound is not: it is [`crate::route_plan::safe_drop_height`],
//! the tallest fall the player's own published fall-damage curve lets them
//! take for free, so a bot never plans a route that hurts it.
//!
//! Nothing here logs; positions are map-derived and stay data.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull, MoveConfig};

use crate::reachability::{EdgeOutcome, JumpBounds, STEP_UP, settle_start, try_edge};

/// The grid pitch the flood fill steps in, in world units. Project
/// tuning: the standing hull is 32 units wide, so a 32-unit pitch keeps
/// one node per hull width without the cost of
/// [`crate::reachability::CELL_SIZE`]'s finer triage grid.
pub(crate) const NAV_CELL: f32 = 32.0;

/// The most nodes one map's graph may hold. Project-authored bound so a
/// pathological map cannot make the flood fill run without limit; far
/// above what a deathmatch arena needs at [`NAV_CELL`] spacing.
pub(crate) const MAX_NAV_NODES: usize = 60_000;

/// Two landings in the same grid column are one node when their heights
/// differ by less than this. Larger than a stair step, smaller than a
/// storey, so a staircase stays one walkable column per cell and a
/// balcony above a floor stays a separate node.
const SAME_FLOOR_TOLERANCE: f32 = 40.0;

/// Extra cost a jump edge carries on top of its length, so a path only
/// jumps when walking around costs more than this. Project tuning.
const JUMP_PENALTY: f32 = 64.0;

/// Extra cost per unit of height a drop edge falls, so a path prefers the
/// stairs to an equally long ledge drop. Project tuning.
const DROP_PENALTY_PER_UNIT: f32 = 0.5;

/// How a bot crosses one edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// Walk across; a step up or down no taller than a stair.
    Walk,
    /// Jump: the edge climbs onto something taller than a stair, or
    /// clears a gap with no floor in between.
    Jump,
    /// Walk off a ledge and fall, at most a safe height.
    Drop,
    /// A fall further than a safe height. Never part of a planned path;
    /// kept so the flood fill still discovers the floor below (a bot that
    /// slips off a ledge lands somewhere the graph knows, and can plan its
    /// way back out from there).
    Fall,
}

/// One standing position on the graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NavNode {
    /// The standing hull's origin (its centre), resting on the floor.
    pub(crate) position: Vec3,
    first_edge: u32,
    edge_count: u16,
}

/// One directed edge between two nodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NavEdge {
    pub(crate) to: u32,
    pub(crate) kind: EdgeKind,
    cost: f32,
}

/// The flood-filled walkable graph for one map.
#[derive(Debug, Clone, Default)]
pub(crate) struct NavGraph {
    nodes: Vec<NavNode>,
    edges: Vec<NavEdge>,
    /// Node indices by `(x, y)` grid column, for nearest-node queries and
    /// for merging landings onto existing nodes.
    columns: BTreeMap<(i32, i32), Vec<u32>>,
    /// Which connected piece of the graph each node belongs to (edges taken
    /// as two-way, `Fall` edges left out), so a search between two pieces
    /// is refused without expanding one node.
    components: Vec<u32>,
}

/// One edge attempt the flood fill accepted, before node ids exist.
struct Landing {
    position: Vec3,
    kind: EdgeKind,
}

/// The eight compass directions the flood fill steps in, as whole grid
/// offsets so a diagonal step lands on the diagonal neighbour's cell.
const DIRECTIONS: [(i8, i8); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

// A published GoldSrc map's coordinates fit comfortably within `i16`, so
// dividing by `NAV_CELL` and rounding never approaches `i32`'s range.
#[allow(clippy::cast_possible_truncation)]
fn column_of(position: Vec3) -> (i32, i32) {
    (
        (position.x / NAV_CELL).round() as i32,
        (position.y / NAV_CELL).round() as i32,
    )
}

impl NavGraph {
    /// Flood-fills the graph from every `seeds` position (each first
    /// dropped onto the floor beneath it), over `collision`, with the jump
    /// reach and fall bound of the walking player's own `config`.
    pub(crate) fn build(collision: &CollisionModel, config: &MoveConfig, seeds: &[Vec3]) -> Self {
        let jump = JumpBounds::from_move_config(config);
        let safe_drop = crate::route_plan::safe_drop_height(config);
        let mut graph = Self::default();
        let mut pending: Vec<Vec<Landing>> = Vec::new();
        let mut queue = VecDeque::new();

        for seed in seeds {
            if !seed.is_finite() {
                continue;
            }
            // On the grid when there is room there, so every seed's flood
            // fill lands on the same lattice and a merged landing is the
            // very node it was tested against.
            let snapped = Vec3::new(
                (seed.x / NAV_CELL).round() * NAV_CELL,
                (seed.y / NAV_CELL).round() * NAV_CELL,
                seed.z,
            );
            let snapped = settle_start(collision, snapped);
            let start = if collision
                .trace(Hull::Standing, snapped, snapped)
                .start_solid
            {
                settle_start(collision, *seed)
            } else {
                snapped
            };
            if collision.trace(Hull::Standing, start, start).start_solid {
                continue;
            }
            if graph.find_or_insert(start, &mut pending).1 {
                queue.push_back(graph.nodes.len() - 1);
            }
        }

        while let Some(index) = queue.pop_front() {
            let from = graph.nodes[index].position;
            let mut landings = Vec::new();
            for (dx, dy) in DIRECTIONS {
                let step = Vec3::new(f32::from(dx), f32::from(dy), 0.0) * NAV_CELL;
                if let Some(landing) = Self::try_direction(collision, from, step, jump, safe_drop) {
                    landings.push(landing);
                }
            }
            pending[index] = landings;
            // Materialise every landing as a node now, so the queue keeps
            // exploring breadth-first; edges are resolved once at the end.
            let count = pending[index].len();
            for slot in 0..count {
                let position = pending[index][slot].position;
                if graph.nodes.len() >= MAX_NAV_NODES {
                    break;
                }
                let (_, inserted) = graph.find_or_insert(position, &mut pending);
                if inserted {
                    queue.push_back(graph.nodes.len() - 1);
                }
            }
            if graph.nodes.len() >= MAX_NAV_NODES && queue.is_empty() {
                break;
            }
        }

        graph.resolve_edges(&pending);
        graph.label_components();
        graph
    }

    /// Labels [`Self::components`] by union-find over every non-`Fall`
    /// edge, taken as two-way.
    fn label_components(&mut self) {
        fn root(parent: &mut [u32], mut node: u32) -> u32 {
            while parent[node as usize] != node {
                let grand = parent[parent[node as usize] as usize];
                parent[node as usize] = grand;
                node = grand;
            }
            node
        }
        let count = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        let mut parent: Vec<u32> = (0..count).collect();
        for from in 0..count {
            for edge in self.edges_from(from) {
                if edge.kind == EdgeKind::Fall {
                    continue;
                }
                let (a, b) = (root(&mut parent, from), root(&mut parent, edge.to));
                if a != b {
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        }
        self.components = (0..count).map(|node| root(&mut parent, node)).collect();
    }

    /// Whether `a` and `b` lie in the same connected piece of the graph (a
    /// path between them is at least possible).
    pub(crate) fn connected(&self, a: u32, b: u32) -> bool {
        matches!(
            (self.components.get(a as usize), self.components.get(b as usize)),
            (Some(x), Some(y)) if x == y
        )
    }

    /// Tries one compass direction from `from`: a plain step first, then a
    /// short hop onto something taller than a stair, then a running jump
    /// across a gap. Returns the first landing a bot can take without
    /// falling further than `safe_drop`.
    fn try_direction(
        collision: &CollisionModel,
        from: Vec3,
        step: Vec3,
        jump: JumpBounds,
        safe_drop: f32,
    ) -> Option<Landing> {
        let length = step.length();
        let direction = step / length;
        let plain = try_edge(collision, Hull::Standing, from, direction, STEP_UP, length);
        let (gap, fall) = match plain {
            EdgeOutcome::Landed { position, drop } if drop <= safe_drop => {
                let kind = if drop > STEP_UP * 2.0 {
                    EdgeKind::Drop
                } else {
                    EdgeKind::Walk
                };
                return Some(Landing { position, kind });
            }
            // Too deep: a gap a running jump may clear, and otherwise a
            // fall to remember.
            EdgeOutcome::Landed { position, .. } => (true, Some(position)),
            // No floor at all: a gap a running jump may clear.
            EdgeOutcome::NoFloor => (true, None),
            // A wall or a ledge: a short hop may climb onto it.
            EdgeOutcome::BlockedAcross(_) | EdgeOutcome::BlockedUp => (false, None),
        };
        let mut distance = length;
        while distance <= jump.horizontal {
            if let EdgeOutcome::Landed { position, drop } = try_edge(
                collision,
                Hull::Standing,
                from,
                direction,
                jump.ascend,
                distance,
            ) && drop <= safe_drop
            {
                // A "jump" that ends no higher than a stair above the start
                // and crossed no gap is just a walk the plain step could not
                // see past an overhang; keep it a walk.
                let climbed = position.z - from.z > STEP_UP;
                let kind = if climbed || gap {
                    EdgeKind::Jump
                } else {
                    EdgeKind::Walk
                };
                return Some(Landing { position, kind });
            }
            if !gap {
                // A ledge is only worth a hop onto its own edge, not a
                // flying leap past it.
                if distance >= length * 2.0 {
                    break;
                }
            }
            distance += length;
        }
        fall.map(|position| Landing {
            position,
            kind: EdgeKind::Fall,
        })
    }

    /// The node standing at `position`, inserting a new one (with an empty
    /// pending edge list) when none of its column's nodes is on the same
    /// floor. Returns the index and whether it was inserted.
    fn find_or_insert(&mut self, position: Vec3, pending: &mut Vec<Vec<Landing>>) -> (u32, bool) {
        let column = column_of(position);
        if let Some(existing) = self.columns.get(&column).and_then(|indices| {
            indices.iter().copied().find(|index| {
                (self.nodes[*index as usize].position.z - position.z).abs() < SAME_FLOOR_TOLERANCE
            })
        }) {
            return (existing, false);
        }
        let index = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.nodes.push(NavNode {
            position,
            first_edge: 0,
            edge_count: 0,
        });
        pending.push(Vec::new());
        self.columns.entry(column).or_default().push(index);
        (index, true)
    }

    /// Turns every node's pending landings into edges to the node each
    /// landing merged onto.
    fn resolve_edges(&mut self, pending: &[Vec<Landing>]) {
        for (index, landings) in pending.iter().enumerate() {
            let from = self.nodes[index].position;
            let first = u32::try_from(self.edges.len()).unwrap_or(u32::MAX);
            let mut count = 0u16;
            for landing in landings {
                let Some(to) = self.node_at(landing.position) else {
                    continue;
                };
                if to as usize == index {
                    continue;
                }
                let target = self.nodes[to as usize].position;
                let horizontal = (target - from).truncate().length();
                let drop = (from.z - target.z).max(0.0);
                // A landing merged onto a node a little higher than itself
                // is a climb by that node's own height, not the landing's.
                let kind = if landing.kind == EdgeKind::Walk && target.z - from.z > STEP_UP + 2.0 {
                    EdgeKind::Jump
                } else {
                    landing.kind
                };
                let cost = horizontal
                    + match kind {
                        EdgeKind::Walk => 0.0,
                        EdgeKind::Jump => JUMP_PENALTY,
                        EdgeKind::Drop | EdgeKind::Fall => drop * DROP_PENALTY_PER_UNIT,
                    };
                self.edges.push(NavEdge { to, kind, cost });
                count = count.saturating_add(1);
            }
            self.nodes[index].first_edge = first;
            self.nodes[index].edge_count = count;
        }
    }

    /// The node on `position`'s own floor in its grid column, if any.
    fn node_at(&self, position: Vec3) -> Option<u32> {
        self.columns.get(&column_of(position)).and_then(|indices| {
            indices.iter().copied().find(|index| {
                (self.nodes[*index as usize].position.z - position.z).abs() < SAME_FLOOR_TOLERANCE
            })
        })
    }

    /// How many nodes the graph holds.
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// One node, by index.
    pub(crate) fn node(&self, index: u32) -> Option<&NavNode> {
        self.nodes.get(index as usize)
    }

    /// Every edge leaving `index`.
    pub(crate) fn edges_from(&self, index: u32) -> &[NavEdge] {
        self.nodes.get(index as usize).map_or(&[], |node| {
            let first = node.first_edge as usize;
            &self.edges[first..first + usize::from(node.edge_count)]
        })
    }

    /// The edge from `from` to `to`, when the graph has one.
    pub(crate) fn edge_between(&self, from: u32, to: u32) -> Option<&NavEdge> {
        self.edges_from(from).iter().find(|edge| edge.to == to)
    }

    /// The node nearest `position` within two grid columns of it, weighing
    /// height differences double so a node on the floor above or below
    /// loses to one on the same floor.
    pub(crate) fn nearest(&self, position: Vec3) -> Option<u32> {
        let (cx, cy) = column_of(position);
        let mut best: Option<(f32, u32)> = None;
        for dx in -2..=2 {
            for dy in -2..=2 {
                let Some(indices) = self.columns.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for index in indices {
                    let node = self.nodes[*index as usize].position;
                    let offset = node - position;
                    let score = offset.truncate().length_squared() + 4.0 * offset.z * offset.z;
                    if best.is_none_or(|(current, _)| score < current) {
                        best = Some((score, *index));
                    }
                }
            }
        }
        best.map(|(_, index)| index)
    }

    /// The cheapest path from `start` to `goal` as node indices (both
    /// ends included), or `None` when the goal cannot be reached. Bounded
    /// by `max_expanded` node expansions so one query cannot stall a step.
    /// Every `(from, to)` edge in `avoid` (sorted) is left out — one a bot
    /// already failed to cross. Two nodes in different connected pieces
    /// are refused without a search.
    pub(crate) fn find_path(
        &self,
        start: u32,
        goal: u32,
        max_expanded: usize,
        avoid: &[(u32, u32)],
    ) -> Option<Vec<u32>> {
        let goal_position = self.node(goal)?.position;
        self.node(start)?;
        if !self.connected(start, goal) {
            return None;
        }
        let mut came_from: Vec<u32> = vec![u32::MAX; self.nodes.len()];
        let mut best_cost: Vec<f32> = vec![f32::INFINITY; self.nodes.len()];
        let mut open = BinaryHeap::new();
        best_cost[start as usize] = 0.0;
        open.push(Frontier {
            estimate: self.nodes[start as usize].position.distance(goal_position),
            cost: 0.0,
            node: start,
        });
        let mut expanded = 0usize;
        while let Some(Frontier { cost, node, .. }) = open.pop() {
            if node == goal {
                let mut path = vec![goal];
                let mut cursor = goal;
                while cursor != start {
                    cursor = came_from[cursor as usize];
                    path.push(cursor);
                }
                path.reverse();
                return Some(path);
            }
            if cost > best_cost[node as usize] {
                continue;
            }
            expanded += 1;
            if expanded > max_expanded {
                return None;
            }
            for edge in self.edges_from(node) {
                if edge.kind == EdgeKind::Fall || avoid.binary_search(&(node, edge.to)).is_ok() {
                    continue;
                }
                let next_cost = cost + edge.cost;
                if next_cost < best_cost[edge.to as usize] {
                    best_cost[edge.to as usize] = next_cost;
                    came_from[edge.to as usize] = node;
                    let estimate = next_cost
                        + self.nodes[edge.to as usize]
                            .position
                            .distance(goal_position);
                    open.push(Frontier {
                        estimate,
                        cost: next_cost,
                        node: edge.to,
                    });
                }
            }
        }
        None
    }
}

/// One open-list entry, ordered so the [`BinaryHeap`] pops the lowest
/// estimate first (ties broken by node index, for a reproducible order).
#[derive(Debug, Clone, Copy)]
struct Frontier {
    estimate: f32,
    cost: f32,
    node: u32,
}

impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Frontier {}

impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then_with(|| other.node.cmp(&self.node))
    }
}

#[cfg(test)]
mod tests {
    use super::{EdgeKind, NavGraph};
    use crate::Game;
    use crate::assets::MemoryAssets;
    use crate::test_support::{AI_MAP, ai_room_bsp, synthetic_map_bsp};
    use glam::Vec3;

    fn ai_room(interior_wall: bool) -> Game {
        let entities = "{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"info_player_start\"\n\"origin\" \"-128 0 36\"\n}\n";
        let bytes = ai_room_bsp(entities, interior_wall);
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
        Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
    }

    fn build(game: &Game, seeds: &[Vec3]) -> NavGraph {
        NavGraph::build(
            game.collision().expect("collision"),
            game.move_config(),
            seeds,
        )
    }

    #[test]
    fn an_open_room_floods_into_one_connected_graph() {
        let game = ai_room(false);
        let graph = build(&game, &[Vec3::new(-128.0, 0.0, 64.0)]);
        assert!(graph.len() > 50, "a 512-unit room holds many 32-unit cells");
        let start = graph.nearest(Vec3::new(-200.0, -200.0, 36.0)).unwrap();
        let goal = graph.nearest(Vec3::new(200.0, 200.0, 36.0)).unwrap();
        let path = graph.find_path(start, goal, 100_000, &[]).expect("a path");
        assert_eq!(path.first(), Some(&start));
        assert_eq!(path.last(), Some(&goal));
        for pair in path.windows(2) {
            assert!(graph.edge_between(pair[0], pair[1]).is_some());
        }
    }

    #[test]
    fn a_full_height_wall_splits_the_room_in_two() {
        let game = ai_room(true);
        let graph = build(&game, &[Vec3::new(-128.0, 0.0, 64.0)]);
        assert!(
            graph.nodes.iter().all(|node| node.position.x < 0.0),
            "nothing past the wall is reachable from the west half"
        );
    }

    #[test]
    fn the_synthetic_ledge_is_climbed_by_a_jump_and_left_by_a_drop_or_walk() {
        let bytes = synthetic_map_bsp();
        let mut assets = MemoryAssets::new();
        assets.insert("maps/ohlsynth.bsp", bytes.clone());
        let game = Game::from_map_bytes(&assets, "ohlsynth", &bytes).expect("loads");
        let graph = build(&game, &[Vec3::new(0.0, 0.0, 40.0)]);
        assert!(graph.len() > 20);
        let edges = || {
            (0..u32::try_from(graph.len()).unwrap())
                .flat_map(|index| graph.edges_from(index).iter().map(|edge| edge.kind))
        };
        assert!(
            edges().any(|kind| kind == EdgeKind::Jump),
            "the ledge needs a jump"
        );
        assert!(
            edges().any(|kind| matches!(kind, EdgeKind::Drop | EdgeKind::Walk)),
            "and is left on foot"
        );
        // Every edge stays within the safe fall bound and is well formed.
        for index in 0..u32::try_from(graph.len()).unwrap() {
            for edge in graph.edges_from(index) {
                let from = graph.node(index).unwrap().position;
                let to = graph.node(edge.to).unwrap().position;
                if edge.kind != EdgeKind::Fall {
                    assert!(
                        from.z - to.z
                            <= crate::route_plan::safe_drop_height(game.move_config()) + 1.0
                    );
                }
                if edge.kind == EdgeKind::Walk {
                    assert!(to.z - from.z <= crate::reachability::STEP_UP + 1.0);
                }
            }
        }
    }

    #[test]
    fn an_unreachable_goal_has_no_path_and_the_search_is_bounded() {
        let game = ai_room(true);
        let west = build(&game, &[Vec3::new(-128.0, 0.0, 64.0)]);
        let start = west.nearest(Vec3::new(-200.0, 0.0, 36.0)).unwrap();
        let far = west.nearest(Vec3::new(-40.0, 200.0, 36.0)).unwrap();
        assert!(
            west.find_path(start, far, 1, &[]).is_none(),
            "one expansion is not enough"
        );
        assert!(west.find_path(start, far, 100_000, &[]).is_some());
    }
}
