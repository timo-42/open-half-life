//! Node-graph construction: ground snapping, per-hull link validation and
//! the documented bounds.

mod common;

use common::{doorway_lattice, doorway_room, ground, open_room};
use ohl_nav::{BuildLimits, NodeGraph, NodeKind, NodeSeed, Vec3, node_seeds_from_entities};
use ohl_physics::Hull;

#[test]
fn ground_nodes_are_dropped_onto_the_floor() {
    let collision = open_room();
    let seeds = vec![
        NodeSeed::new(Vec3::new(0.0, 0.0, 96.0), NodeKind::Ground),
        NodeSeed::new(Vec3::new(64.0, 0.0, 96.0), NodeKind::Air),
    ];
    let graph = NodeGraph::build(&seeds, &collision, &BuildLimits::default());

    let nodes = graph.nodes();
    assert!(nodes[0].snapped, "the ground node should find the floor");
    assert!(
        nodes[0].position.z.abs() < 0.5,
        "ground node landed at {}",
        nodes[0].position.z
    );
    assert!(!nodes[1].snapped, "air nodes keep their placed position");
    assert_eq!(nodes[1].position, Vec3::new(64.0, 0.0, 96.0));

    // The waypoint for a grounded node is the entity origin for that hull.
    let waypoint = graph.waypoint(0, Hull::Standing).expect("node 0 exists");
    assert!((waypoint.z - Hull::Standing.foot_offset()).abs() < 0.5);
}

#[test]
fn a_narrow_doorway_links_only_for_the_smaller_hull() {
    let collision = doorway_room(40.0);
    let graph = NodeGraph::build(&doorway_lattice(), &collision, &BuildLimits::default());

    // The doorway node (2) links to both sides for the humanoid hull...
    assert!(graph.has_link(1, 2, Hull::Standing));
    assert!(graph.has_link(2, 3, Hull::Standing));
    // ...and to neither for the large hull, which does not fit.
    assert!(!graph.has_link(1, 2, Hull::Large));
    assert!(!graph.has_link(2, 3, Hull::Large));
    // A link whose segment crosses solid wall is rejected for every hull,
    // while the pair lined up with the doorway is accepted for exactly the
    // hulls that fit through it.
    for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
        assert!(
            !graph.has_link(0, 4, hull),
            "{hull:?} should not link straight through the wall"
        );
    }
    assert!(graph.has_link(1, 3, Hull::Standing));
    assert!(!graph.has_link(1, 3, Hull::Large));
}

#[test]
fn a_wide_doorway_links_for_every_hull() {
    let collision = doorway_room(160.0);
    let graph = NodeGraph::build(&doorway_lattice(), &collision, &BuildLimits::default());

    for hull in [Hull::Standing, Hull::Large, Hull::Crouched] {
        assert!(
            graph.has_link(1, 2, hull) && graph.has_link(2, 3, hull),
            "{hull:?} fits through a 160-unit doorway"
        );
    }
}

#[test]
fn a_link_across_a_gap_in_the_floor_is_rejected() {
    // Two floor slabs with a 128-unit hole between them: the hull trace
    // across the hole is clear, but there is no floor support.
    let collision = common::model_from_brushes(&[
        ohl_formats::test_support::CollisionBrush::box_brush(
            [-512.0, -512.0, -64.0],
            [-64.0, 512.0, 0.0],
        ),
        ohl_formats::test_support::CollisionBrush::box_brush(
            [64.0, -512.0, -64.0],
            [512.0, 512.0, 0.0],
        ),
    ]);
    let seeds = vec![ground(-128.0, 0.0), ground(128.0, 0.0)];
    let graph = NodeGraph::build(&seeds, &collision, &BuildLimits::default());

    assert_eq!(
        graph.link_count(),
        0,
        "a ground link may not cross an unsupported gap"
    );
}

#[test]
fn air_nodes_link_through_open_space_without_floor_support() {
    let collision = open_room();
    let seeds = vec![
        NodeSeed::new(Vec3::new(-128.0, 0.0, 192.0), NodeKind::Air),
        NodeSeed::new(Vec3::new(128.0, 0.0, 192.0), NodeKind::Air),
    ];
    let graph = NodeGraph::build(&seeds, &collision, &BuildLimits::default());

    assert!(graph.has_link(0, 1, Hull::Point));
    assert!(graph.has_link(1, 0, Hull::Point));
}

#[test]
fn ground_and_air_nodes_never_link_to_each_other() {
    // The published rule: `info_node` and `info_node_air` do not link to
    // one another (docs/FORMAT_SOURCES.md, "Navigation").
    let collision = open_room();
    let seeds = vec![
        ground(0.0, 0.0),
        NodeSeed::new(Vec3::new(0.0, 0.0, 64.0), NodeKind::Air),
    ];
    let graph = NodeGraph::build(&seeds, &collision, &BuildLimits::default());

    assert_eq!(graph.link_count(), 0);
}

#[test]
fn construction_respects_its_bounds() {
    let collision = open_room();
    let mut seeds = Vec::new();
    for x in 0..8u8 {
        for y in 0..8u8 {
            seeds.push(ground(
                f32::from(x) * 48.0 - 192.0,
                f32::from(y) * 48.0 - 192.0,
            ));
        }
    }

    let limits = BuildLimits {
        max_nodes: 24,
        max_links_per_node: 3,
        ..BuildLimits::default()
    };
    let graph = NodeGraph::build(&seeds, &collision, &limits);

    assert_eq!(graph.node_count(), 24);
    for index in 0..24u32 {
        assert!(graph.links_from(index).len() <= 3);
        // Links are kept cheapest first, so the slice is sorted by cost.
        let costs: Vec<f32> = graph.links_from(index).iter().map(|l| l.cost).collect();
        assert!(costs.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    // A pair budget of zero yields no links at all, and building is
    // deterministic.
    let starved = NodeGraph::build(
        &seeds,
        &collision,
        &BuildLimits {
            max_candidate_pairs: 0,
            ..limits
        },
    );
    assert_eq!(starved.link_count(), 0);
    assert_eq!(graph, NodeGraph::build(&seeds, &collision, &limits));
}

#[test]
fn out_of_range_and_degenerate_input_is_handled() {
    let collision = open_room();
    let seeds = vec![
        ground(0.0, 0.0),
        NodeSeed::new(Vec3::new(f32::NAN, 0.0, 0.0), NodeKind::Ground),
        NodeSeed::new(Vec3::new(0.0, f32::INFINITY, 0.0), NodeKind::Air),
    ];
    let graph = NodeGraph::build(&seeds, &collision, &BuildLimits::default());

    assert_eq!(graph.node_count(), 1, "non-finite seeds are dropped");
    assert!(graph.node(7).is_none());
    assert!(graph.links_from(7).is_empty());
    assert!(graph.waypoint(7, Hull::Standing).is_none());
}

#[test]
fn seeds_are_collected_from_the_published_node_entities() {
    let text = "{\n\"classname\" \"worldspawn\"\n}\n\
        {\n\"classname\" \"info_node\"\n\"origin\" \"16 -32 8\"\n}\n\
        {\n\"classname\" \"info_node_air\"\n\"origin\" \"0 0 192\"\n}\n\
        {\n\"classname\" \"info_node\"\n\"origin\" \"broken\"\n}\n\
        {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 36\"\n}\n\0";
    let entities =
        ohl_formats::bsp30::parse_entities(text.as_bytes(), &ohl_formats::bsp30::Limits::default())
            .expect("fixture entity text parses");

    let seeds = node_seeds_from_entities(&entities, 16);
    assert_eq!(
        seeds,
        vec![
            NodeSeed::new(Vec3::new(16.0, -32.0, 8.0), NodeKind::Ground),
            NodeSeed::new(Vec3::new(0.0, 0.0, 192.0), NodeKind::Air),
        ]
    );
    assert_eq!(node_seeds_from_entities(&entities, 1).len(), 1);
}

/// The graph is serialisable so a host can cache it beside a map instead of
/// re-tracing every link on load.
#[cfg(feature = "serde")]
#[test]
fn a_built_graph_round_trips_through_serde() {
    let collision = doorway_room(40.0);
    let graph = NodeGraph::build(&doorway_lattice(), &collision, &BuildLimits::default());

    let bytes = postcard::to_allocvec(&graph).expect("the graph serialises");
    let restored: NodeGraph = postcard::from_bytes(&bytes).expect("the graph deserialises");
    assert_eq!(graph, restored);
}
