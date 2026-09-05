//! A* queries: per-hull routing, optimality, endpoint attachment and bounds.

mod common;

use common::{doorway_lattice, doorway_room, ground, open_room};
use ohl_nav::{BuildLimits, NodeGraph, Path, PathLimits, Vec3, find_path, straight_path_if_clear};
use ohl_physics::{CollisionModel, Hull};

/// Brute-force shortest node distance, for comparing A* against.
fn dijkstra(graph: &NodeGraph, start: u32, goal: u32, hull: Hull) -> Option<f32> {
    let count = graph.node_count();
    let mut best = vec![f32::INFINITY; count];
    let mut done = vec![false; count];
    best[start as usize] = 0.0;
    loop {
        let mut current = None;
        for index in 0..count {
            if !done[index]
                && best[index].is_finite()
                && current.is_none_or(|chosen: usize| best[index] < best[chosen])
            {
                current = Some(index);
            }
        }
        let Some(current) = current else { break };
        done[current] = true;
        for link in graph.links_from(u32::try_from(current).expect("index fits")) {
            if !link.allows(hull) {
                continue;
            }
            let candidate = best[current] + link.cost;
            if candidate < best[link.to as usize] {
                best[link.to as usize] = candidate;
            }
        }
    }
    best.get(goal as usize)
        .copied()
        .filter(|cost| cost.is_finite())
}

/// The cost of a route's links, i.e. the path cost without the endpoint
/// attachments.
fn route_cost(graph: &NodeGraph, path: &Path, hull: Hull) -> f32 {
    path.nodes
        .windows(2)
        .map(|pair| {
            graph
                .links_from(pair[0])
                .iter()
                .find(|link| link.to == pair[1] && link.allows(hull))
                .expect("consecutive route nodes are linked")
                .cost
        })
        .sum()
}

fn lattice_graph(collision: &CollisionModel) -> NodeGraph {
    // A 5x5 lattice with a 96-unit pitch: the link radius of 160 keeps
    // orthogonal (96) and diagonal (136) neighbours only, so the optimal
    // costs are easy to reason about.
    let mut seeds = Vec::new();
    for x in 0..5u8 {
        for y in 0..5u8 {
            seeds.push(ground(
                f32::from(x) * 96.0 - 192.0,
                f32::from(y) * 96.0 - 192.0,
            ));
        }
    }
    NodeGraph::build(
        &seeds,
        collision,
        &BuildLimits {
            link_radius: 160.0,
            max_links_per_node: 8,
            ..BuildLimits::default()
        },
    )
}

#[test]
fn a_star_returns_the_optimal_cost() {
    let collision = open_room();
    let graph = lattice_graph(&collision);
    let hull = Hull::Standing;

    // Corner to corner across the lattice: four diagonal steps.
    let from = graph.waypoint(0, hull).expect("node 0 exists");
    let to = graph.waypoint(24, hull).expect("node 24 exists");
    let path = find_path(&graph, &collision, from, to, hull, &PathLimits::default())
        .expect("the lattice is fully connected");

    assert_eq!(path.nodes.first(), Some(&0));
    assert_eq!(path.nodes.last(), Some(&24));
    let optimal = dijkstra(&graph, 0, 24, hull).expect("brute force agrees a route exists");
    assert!(
        (route_cost(&graph, &path, hull) - optimal).abs() < 0.01,
        "A* cost {} is not the optimal {optimal}",
        route_cost(&graph, &path, hull)
    );
    // Four 96x96 diagonal steps.
    let expected = 4.0 * (96.0f32 * 96.0 * 2.0).sqrt();
    assert!((optimal - expected).abs() < 0.01, "optimal was {optimal}");
    assert!(path.explored <= PathLimits::default().max_explored);
}

#[test]
fn every_reachable_pair_matches_brute_force() {
    let collision = open_room();
    let graph = lattice_graph(&collision);
    let hull = Hull::Standing;
    let limits = PathLimits::default();

    for start in 0..u32::try_from(graph.node_count()).expect("small graph") {
        for goal in 0..u32::try_from(graph.node_count()).expect("small graph") {
            let from = graph.waypoint(start, hull).expect("node exists");
            let to = graph.waypoint(goal, hull).expect("node exists");
            let path = find_path(&graph, &collision, from, to, hull, &limits);
            match (path, dijkstra(&graph, start, goal, hull)) {
                (Some(path), Some(optimal)) => assert!(
                    (route_cost(&graph, &path, hull) - optimal).abs() < 0.01,
                    "{start} -> {goal} was not optimal"
                ),
                (None, None) => {}
                (found, expected) => {
                    panic!(
                        "{start} -> {goal}: A* {found:?} disagrees with brute force {expected:?}"
                    )
                }
            }
        }
    }
}

#[test]
fn the_humanoid_hull_routes_through_a_doorway_the_large_hull_cannot_use() {
    let collision = doorway_room(40.0);
    let graph = NodeGraph::build(&doorway_lattice(), &collision, &BuildLimits::default());
    let limits = PathLimits::default();

    let west = Vec3::new(-192.0, 128.0, Hull::Standing.foot_offset());
    let east = Vec3::new(192.0, 128.0, Hull::Standing.foot_offset());
    let path = find_path(&graph, &collision, west, east, Hull::Standing, &limits)
        .expect("a humanoid fits through the doorway");
    // The wall may only be crossed between the two approach nodes lined up
    // with the doorway, so both of them are on the route.
    assert!(
        path.nodes.contains(&1) && path.nodes.contains(&3),
        "the humanoid route must cross at the doorway, got {:?}",
        path.nodes
    );
    assert_eq!(path.waypoints.len(), path.nodes.len() + 1);
    assert_eq!(path.goal(), Some(east));

    let west = Vec3::new(-192.0, 128.0, Hull::Large.foot_offset());
    let east = Vec3::new(192.0, 128.0, Hull::Large.foot_offset());
    assert!(
        find_path(&graph, &collision, west, east, Hull::Large, &limits).is_none(),
        "the large hull has no route through a 40-unit doorway"
    );
}

#[test]
fn endpoints_attach_to_the_nearest_reachable_node() {
    let collision = open_room();
    let graph = lattice_graph(&collision);
    let hull = Hull::Standing;

    // 20 units from node 12 (the lattice centre, at the world origin).
    let from = Vec3::new(20.0, 0.0, hull.foot_offset());
    let to = graph.waypoint(0, hull).expect("node 0 exists");
    let path = find_path(&graph, &collision, from, to, hull, &PathLimits::default())
        .expect("route exists");

    assert_eq!(path.nodes.first(), Some(&12), "attached to the wrong node");
    assert!(
        path.cost > route_cost(&graph, &path, hull),
        "attachment costs"
    );

    // Out of attachment range: no route.
    let far = Vec3::new(-2048.0, -2048.0, hull.foot_offset());
    assert!(
        graph
            .nearest_reachable(far, hull, &collision, 64.0, 16)
            .is_none()
    );
    assert!(find_path(&graph, &collision, far, to, hull, &PathLimits::default()).is_none());
}

#[test]
fn the_exploration_bound_is_honoured() {
    let collision = open_room();
    let graph = lattice_graph(&collision);
    let hull = Hull::Standing;
    let from = graph.waypoint(0, hull).expect("node 0 exists");
    let to = graph.waypoint(24, hull).expect("node 24 exists");

    let limits = PathLimits {
        max_explored: 1,
        ..PathLimits::default()
    };
    assert!(find_path(&graph, &collision, from, to, hull, &limits).is_none());
}

#[test]
fn the_straight_shortcut_only_fires_when_the_way_is_clear() {
    let collision = doorway_room(40.0);
    let hull = Hull::Standing;
    let foot = hull.foot_offset();

    // Straight along the doorway: clear.
    let clear = straight_path_if_clear(
        &collision,
        Vec3::new(-192.0, 0.0, foot),
        Vec3::new(192.0, 0.0, foot),
        hull,
    )
    .expect("the doorway is lined up");
    assert_eq!(clear.waypoints.len(), 1);
    assert_eq!(clear.nodes.len(), 0);
    assert!((clear.cost - 384.0).abs() < 0.01);

    // Straight through the wall: blocked.
    assert!(
        straight_path_if_clear(
            &collision,
            Vec3::new(-192.0, 128.0, foot),
            Vec3::new(192.0, 128.0, foot),
            hull,
        )
        .is_none()
    );
    // ...and so is the same move for a hull too big for the doorway.
    assert!(
        straight_path_if_clear(
            &collision,
            Vec3::new(-192.0, 0.0, Hull::Large.foot_offset()),
            Vec3::new(192.0, 0.0, Hull::Large.foot_offset()),
            Hull::Large,
        )
        .is_none()
    );
}

#[test]
fn a_query_on_an_empty_graph_returns_nothing() {
    let collision = open_room();
    let graph = NodeGraph::default();
    assert!(
        find_path(
            &graph,
            &collision,
            Vec3::ZERO,
            Vec3::new(64.0, 0.0, 0.0),
            Hull::Standing,
            &PathLimits::default(),
        )
        .is_none()
    );
    assert!(
        find_path(
            &graph,
            &collision,
            Vec3::new(f32::NAN, 0.0, 0.0),
            Vec3::ZERO,
            Hull::Standing,
            &PathLimits::default(),
        )
        .is_none()
    );
}
