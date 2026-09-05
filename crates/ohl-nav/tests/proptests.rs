//! Property tests: building a graph and querying it never panics for
//! arbitrary node positions and bounded limits, and every path it returns is
//! contiguous.

mod common;

use common::{corner_corridor, doorway_room, open_room};
use ohl_nav::{
    BuildLimits, NodeGraph, NodeKind, NodeSeed, Path, PathLimits, Steer, SteerLimits, Vec3,
    find_path, straight_path_if_clear,
};
use ohl_physics::{CollisionModel, Hull};
use proptest::prelude::*;

const HULLS: [Hull; 4] = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];

fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        9 => -600.0f32..600.0f32,
        1 => prop::sample::select(vec![
            0.0f32,
            -4096.0,
            4096.0,
            f32::MAX,
            f32::MIN_POSITIVE,
        ]),
    ]
}

fn seed() -> impl Strategy<Value = NodeSeed> {
    (
        coordinate(),
        coordinate(),
        coordinate(),
        prop::sample::select(vec![NodeKind::Ground, NodeKind::Air, NodeKind::Water]),
    )
        .prop_map(|(x, y, z, kind)| NodeSeed::new(Vec3::new(x, y, z), kind))
}

fn build_limits() -> impl Strategy<Value = BuildLimits> {
    (0usize..24, 0usize..6, 0usize..64, 1.0f32..768.0).prop_map(
        |(max_nodes, max_links, pairs, radius)| BuildLimits {
            max_nodes,
            max_links_per_node: max_links,
            max_candidate_pairs: pairs,
            link_radius: radius,
            ..BuildLimits::default()
        },
    )
}

fn collision_for(index: usize) -> CollisionModel {
    match index % 3 {
        0 => open_room(),
        1 => doorway_room(40.0),
        _ => corner_corridor(),
    }
}

/// Every consecutive node pair on a route must be joined by a link the hull
/// may use, and the waypoints must line up with the nodes plus the goal.
fn assert_contiguous(graph: &NodeGraph, path: &Path, hull: Hull, goal: Vec3) {
    assert_eq!(path.waypoints.len(), path.nodes.len() + 1);
    assert_eq!(path.waypoints.last().copied(), Some(goal));
    for (index, node) in path.nodes.iter().enumerate() {
        assert_eq!(
            graph.waypoint(*node, hull),
            path.waypoints.get(index).copied()
        );
    }
    for pair in path.nodes.windows(2) {
        assert!(
            graph.has_link(pair[0], pair[1], hull),
            "route step {} -> {} is not a link for {hull:?}",
            pair[0],
            pair[1]
        );
    }
    assert!(path.cost.is_finite() && path.cost >= 0.0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn building_and_querying_never_panics(
        seeds in prop::collection::vec(seed(), 0..24),
        limits in build_limits(),
        map in 0usize..3,
        hull_index in 0usize..4,
        from in (coordinate(), coordinate(), coordinate()),
        to in (coordinate(), coordinate(), coordinate()),
        max_explored in 0usize..512,
    ) {
        let collision = collision_for(map);
        let hull = HULLS[hull_index];
        let graph = NodeGraph::build(&seeds, &collision, &limits);

        prop_assert!(graph.node_count() <= limits.max_nodes.min(seeds.len()));
        for index in 0..u32::try_from(graph.node_count()).expect("small graph") {
            prop_assert!(graph.links_from(index).len() <= limits.max_links_per_node);
            for link in graph.links_from(index) {
                prop_assert!((link.to as usize) < graph.node_count());
                prop_assert!(link.to != index);
                prop_assert!(link.cost.is_finite() && link.cost >= 0.0);
            }
        }

        let from = Vec3::new(from.0, from.1, from.2);
        let to = Vec3::new(to.0, to.1, to.2);
        let path_limits = PathLimits {
            max_explored,
            ..PathLimits::default()
        };
        if let Some(path) = find_path(&graph, &collision, from, to, hull, &path_limits) {
            prop_assert!(path.explored <= max_explored);
            assert_contiguous(&graph, &path, hull, to);
        }
        // The shortcut and the steering layer are total over the same input.
        let _ = straight_path_if_clear(&collision, from, to, hull);
        let mut steer = Steer::new();
        let path = Path {
            waypoints: vec![from, to],
            nodes: Vec::new(),
            cost: 0.0,
            explored: 0,
        };
        for _ in 0..4 {
            let intent = steer.next_move(from, &path, hull, &collision, &SteerLimits::default());
            prop_assert!(intent.speed_scale >= 0.0 && intent.speed_scale <= 1.0);
            prop_assert!(intent.dir.is_finite());
        }
    }

    /// The same contiguity property, but on a lattice where routes really
    /// are found, so the assertions above are exercised rather than skipped.
    #[test]
    fn lattice_routes_are_contiguous(
        from in (-200.0f32..200.0, -200.0f32..200.0),
        to in (-200.0f32..200.0, -200.0f32..200.0),
        hull_index in 0usize..4,
    ) {
        let collision = open_room();
        let hull = HULLS[hull_index];
        let mut seeds = Vec::new();
        for x in 0..5u8 {
            for y in 0..5u8 {
                seeds.push(NodeSeed::new(
                    Vec3::new(f32::from(x) * 96.0 - 192.0, f32::from(y) * 96.0 - 192.0, 8.0),
                    NodeKind::Ground,
                ));
            }
        }
        let graph = NodeGraph::build(&seeds, &collision, &BuildLimits {
            link_radius: 160.0,
            ..BuildLimits::default()
        });
        let foot = hull.foot_offset();
        let from = Vec3::new(from.0, from.1, foot);
        let to = Vec3::new(to.0, to.1, foot);
        let path = find_path(&graph, &collision, from, to, hull, &PathLimits::default())
            .expect("an open lattice is fully connected for every hull");
        assert_contiguous(&graph, &path, hull, to);
    }

    #[test]
    fn a_rebuild_is_deterministic(
        seeds in prop::collection::vec(seed(), 0..16),
        limits in build_limits(),
        map in 0usize..3,
    ) {
        let collision = collision_for(map);
        let first = NodeGraph::build(&seeds, &collision, &limits);
        let second = NodeGraph::build(&seeds, &collision, &limits);
        prop_assert_eq!(first, second);
    }
}
