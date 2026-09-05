//! Integration tests for `NavBridge`, the real `ohl-nav`-backed navigator.
//!
//! Every fixture here is project-authored, from `ohl_formats::test_support`'s
//! synthetic BSP builder; no game data is loaded.

use hecs::Entity;
use ohl_ai::monsters::nav_bridge::{NavBridge, NavBridgeLimits};
use ohl_ai::{Navigator, StraightLineNavigator, Vec3};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_nav::{BuildLimits, NodeKind, NodeSeed};
use ohl_physics::{CollisionModel, Hull};

const DT: f32 = 0.02;

/// A room spanning `[-512, 512]` on X and Y and `[0, 256]` on Z.
fn room_shell() -> Vec<CollisionBrush> {
    vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
    ]
}

fn model_from_brushes(brushes: &[CollisionBrush]) -> CollisionModel {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads = builder.push_collision_hulls(brushes);
    builder.push_model(
        [-512.0, -512.0, 0.0],
        [512.0, 512.0, 256.0],
        [0.0, 0.0, 0.0],
        heads,
        2,
        0,
        0,
    );
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// The room, split by a 32-unit-thick, 256-unit-long, full-height wall
/// centred on the origin: `x` in `-16..16`, `y` in `-128..128`. Both ends
/// (`y` beyond `+-128`) are open, so a monster on one side can only reach
/// the other by going around one end.
fn wall_room() -> CollisionModel {
    let mut brushes = room_shell();
    brushes.push(CollisionBrush::box_brush(
        [-16.0, -128.0, -16.0],
        [16.0, 128.0, 256.0],
    ));
    model_from_brushes(&brushes)
}

fn open_room() -> CollisionModel {
    model_from_brushes(&room_shell())
}

/// A ground seed on the floor at `x, y`.
fn ground(x: f32, y: f32) -> NodeSeed {
    NodeSeed::new(Vec3::new(x, y, 8.0), NodeKind::Ground)
}

/// A 7x7 lattice covering the room at 128-unit spacing. Nodes that land
/// inside the wall simply fail to snap (no floor directly beneath them) and
/// end up with no links, which is harmless: the rest of the lattice still
/// connects around both open ends.
fn wall_room_lattice() -> Vec<NodeSeed> {
    let coords = [-384.0, -256.0, -128.0, 0.0, 128.0, 256.0, 384.0];
    coords
        .iter()
        .flat_map(|&x| coords.iter().map(move |&y| ground(x, y)))
        .collect()
}

fn dummy_actor() -> Entity {
    hecs::World::new().spawn(())
}

#[test]
fn a_monster_routes_around_a_wall_via_the_node_graph() {
    let collision = wall_room();
    let seeds = wall_room_lattice();
    let mut bridge = NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    assert!(
        bridge.node_count() > 0,
        "the lattice should build some nodes"
    );

    let actor = dummy_actor();
    let hull = Hull::Standing;
    let goal = Vec3::new(300.0, 0.0, 40.0);
    let step = 200.0 * DT;

    let mut pos = Vec3::new(-300.0, 0.0, 40.0);
    let mut max_abs_y = 0.0f32;
    let mut reached = false;
    for _ in 0..4_000 {
        bridge.begin_tick(&[actor]);
        pos = bridge.next_move(actor, pos, goal, hull, &collision, step);
        assert!(pos.is_finite(), "position must stay finite: {pos:?}");
        max_abs_y = max_abs_y.max(pos.y.abs());
        if (pos - goal).length() < 24.0 {
            reached = true;
            break;
        }
    }

    assert!(
        reached,
        "the monster never reached the goal: ended at {pos:?}"
    );
    assert!(
        max_abs_y > 140.0,
        "the monster never routed around the wall's open end: max |y| = {max_abs_y}"
    );
}

#[test]
fn with_no_nodes_and_a_blocked_line_the_bridge_falls_back_to_the_straight_line_mover() {
    let collision = wall_room();
    let seeds: Vec<NodeSeed> = Vec::new();
    let mut bridge = NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    assert_eq!(bridge.node_count(), 0);

    let actor = dummy_actor();
    let hull = Hull::Standing;
    let origin = Vec3::new(-300.0, 0.0, 40.0);
    let goal = Vec3::new(300.0, 0.0, 40.0);
    let step = 10.0;

    bridge.begin_tick(&[actor]);
    let bridged = bridge.next_move(actor, origin, goal, hull, &collision, step);
    let straight = StraightLineNavigator.next_move(origin, goal, step);
    assert_eq!(
        bridged, straight,
        "with no graph, the bridge is the straight-line mover"
    );
}

#[test]
fn the_per_tick_search_budget_is_respected() {
    let collision = wall_room();
    let seeds = wall_room_lattice();
    let limits = NavBridgeLimits {
        max_searches_per_tick: 1,
        ..NavBridgeLimits::default()
    };
    let mut bridge = NavBridge::build(&seeds, &collision, &BuildLimits::default(), limits);

    let first = dummy_actor();
    let second = dummy_actor();
    let hull = Hull::Standing;
    let goal = Vec3::new(300.0, 0.0, 40.0);
    let step = 4.0;

    bridge.begin_tick(&[first, second]);
    // Spends the one search this tick's budget allows.
    let _ = bridge.next_move(
        first,
        Vec3::new(-300.0, 0.0, 40.0),
        goal,
        hull,
        &collision,
        step,
    );
    // The budget is spent: the second actor gets the exact straight-line
    // fallback this tick, even though it needs a route just as much.
    let origin = Vec3::new(-300.0, 0.0, 40.0);
    let second_move = bridge.next_move(second, origin, goal, hull, &collision, step);
    let straight = StraightLineNavigator.next_move(origin, goal, step);
    assert_eq!(
        second_move, straight,
        "budget exhausted: straight-line fallback expected"
    );

    // A fresh tick resets the budget. Run the second actor from scratch for
    // long enough to reach the goal, which it can only do by routing
    // around the wall, proving the earlier fallback was a one-tick budget
    // effect and not a permanent inability to path.
    let mut pos = origin;
    let mut max_abs_y = 0.0f32;
    let mut reached = false;
    for _ in 0..4_000 {
        bridge.begin_tick(&[second]);
        pos = bridge.next_move(second, pos, goal, hull, &collision, step);
        max_abs_y = max_abs_y.max(pos.y.abs());
        if (pos - goal).length() < 24.0 {
            reached = true;
            break;
        }
    }
    assert!(
        reached,
        "the second actor never reached the goal once budget was available"
    );
    assert!(
        max_abs_y > 140.0,
        "the second actor never routed around the wall once budget was available"
    );
}

#[test]
fn identical_inputs_produce_identical_trajectories() {
    fn run() -> Vec<Vec3> {
        let collision = wall_room();
        let seeds = wall_room_lattice();
        let mut bridge = NavBridge::build(
            &seeds,
            &collision,
            &BuildLimits::default(),
            NavBridgeLimits::default(),
        );
        let actor = hecs::World::new().spawn(());
        let hull = Hull::Standing;
        let goal = Vec3::new(300.0, 0.0, 40.0);
        let mut pos = Vec3::new(-300.0, 0.0, 40.0);
        let mut trace = Vec::new();
        for _ in 0..500 {
            bridge.begin_tick(&[actor]);
            pos = bridge.next_move(actor, pos, goal, hull, &collision, 200.0 * DT);
            trace.push(pos);
        }
        trace
    }

    assert_eq!(
        run(),
        run(),
        "identical inputs must produce identical trajectories"
    );
}

#[test]
fn an_unobstructed_goal_still_moves_with_no_nodes_at_all() {
    let collision = open_room();
    let seeds: Vec<NodeSeed> = Vec::new();
    let mut bridge = NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let hull = Hull::Standing;
    let origin = Vec3::new(-100.0, 0.0, 40.0);
    let goal = Vec3::new(100.0, 0.0, 40.0);

    bridge.begin_tick(&[actor]);
    let next = bridge.next_move(actor, origin, goal, hull, &collision, 10.0);
    assert!(
        (next.x - (origin.x + 10.0)).abs() < 1e-3,
        "unexpected step: {next:?}"
    );
    assert!(next.y.abs() < 1e-3);
}
