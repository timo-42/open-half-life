//! Integration tests for `NavBridge`, the real `ohl-nav`-backed navigator.
//!
//! Every fixture here is project-authored, from `ohl_formats::test_support`'s
//! synthetic BSP builder; no game data is loaded.

use hecs::Entity;
use ohl_ai::monsters::nav_bridge::{NavBridge, NavBridgeLimits};
use ohl_ai::{Navigator, StraightLineNavigator, Vec3, move_toward};
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
#[ignore = "P5: graph steering stalls before a low step with centered endpoints; reproduced with predecessor cache/arrival semantics"]
fn a_centered_graph_route_walks_up_a_low_step() {
    let collision = model_from_brushes(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::box_brush([0.0, -256.0, 0.0], [200.0, 256.0, 12.0]),
    ]);
    let seeds = [
        ground(-100.0, 0.0),
        NodeSeed::new(Vec3::new(100.0, 0.0, 20.0), NodeKind::Ground),
    ];
    let mut bridge = NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let mut center = Vec3::new(-100.0, 0.0, 36.0);
    let goal = Vec3::new(100.0, 0.0, 48.0);
    for _ in 0..1_200 {
        bridge.begin_tick(&[actor]);
        center = bridge.next_move(actor, center, goal, Hull::Standing, &collision, 0.4);
    }
    assert!(
        center.x > 65.0,
        "centered graph step endpoint={center:?}, stats={:?}",
        bridge.stats()
    );
}

#[test]
fn an_elevated_walker_descends_to_a_ground_graph_before_following_its_detour() {
    // Project-authored coordinates: the same flat-floor wall/lattice as the
    // ordinary graph test, with the centered hull initially 48 units above
    // its standing floor position. The direct segment still meets the wall.
    // This isolates downward graph attachment from climbing an upward step.
    let collision = wall_room();
    let hull = Hull::Standing;
    let floor_center = hull.foot_offset();
    let start = Vec3::new(-250.0, 0.0, floor_center + 48.0);
    let goal = Vec3::new(250.0, 0.0, floor_center);
    assert!(!collision.trace(hull, start, start).start_solid);
    assert!(collision.trace(hull, start, goal).blocked());
    let mut bridge = NavBridge::build(
        &wall_room_lattice(),
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let max_step = 2.0;
    let mut center = start;
    let mut settled = false;
    let mut detoured = false;
    let mut reached = false;
    for _ in 0..4_000 {
        let before = center;
        bridge.begin_tick(&[actor]);
        center = bridge.next_move_with_walking_attachment(
            actor,
            center,
            goal,
            hull,
            &collision,
            max_step,
            ohl_ai::monsters::Fallback::Traced,
        );
        assert!(center.is_finite());
        assert!((center - before).length() <= max_step + 0.001);
        assert!(
            center.z <= before.z + 0.001,
            "the flat-floor route needs no upward snap"
        );
        assert!(
            center.z >= floor_center - 0.001,
            "descent stays above the floor"
        );
        assert!(
            !collision.trace(hull, before, center).blocked(),
            "each committed segment is clear"
        );
        assert!(!collision.trace(hull, center, center).start_solid);
        if !settled {
            assert!(
                center.truncate().abs_diff_eq(start.truncate(), 0.001),
                "initial attachment holds XY until supported landing"
            );
        }
        let near_floor = center.z <= floor_center + 1.0;
        if settled {
            assert!(
                near_floor,
                "a grounded graph walk stays at its supported height"
            );
        }
        settled |= near_floor;
        detoured |= center.y.abs() > 144.0;
        if (center - goal).length() < 24.0 {
            reached = true;
            break;
        }
    }
    assert!(bridge.stats().graph_steps > 0, "the fixture uses the graph");
    assert!(
        settled,
        "a below-waypoint mismatch must resolve by actual bounded descent"
    );
    assert!(detoured, "the route goes around the wall's open end");
    assert!(reached, "the elevated walker completes its grounded detour");
}

fn attachment_bridge(collision: &CollisionModel, max_drop: f32, searches: usize) -> NavBridge {
    NavBridge::build(
        &wall_room_lattice(),
        collision,
        &BuildLimits {
            max_drop,
            ..BuildLimits::default()
        },
        NavBridgeLimits {
            max_searches_per_tick: searches,
            ..NavBridgeLimits::default()
        },
    )
}

fn walking_query(
    bridge: &mut NavBridge,
    actor: Entity,
    origin: Vec3,
    hull: Hull,
    collision: &CollisionModel,
    step: f32,
) -> Vec3 {
    bridge.next_move_with_walking_attachment(
        actor,
        origin,
        Vec3::new(250.0, 0.0, hull.foot_offset()),
        hull,
        collision,
        step,
        ohl_ai::monsters::Fallback::Traced,
    )
}

fn wall_room_with_attached_floor() -> (CollisionModel, ohl_physics::BrushId) {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\"classname\" \"worldspawn\"}");
    let mut walls = room_shell()[1..].to_vec();
    walls.push(CollisionBrush::box_brush(
        [-16.0, -128.0, -16.0],
        [16.0, 128.0, 256.0],
    ));
    let heads = builder.push_collision_hulls(&walls);
    builder.push_model(
        [-512.0, -512.0, -256.0],
        [512.0; 3],
        [0.0; 3],
        heads,
        2,
        0,
        0,
    );
    let floor = builder.push_collision_hulls(&[CollisionBrush::box_brush(
        [-512.0, -512.0, -32.0],
        [512.0, 512.0, 0.0],
    )]);
    builder.push_model(
        [-512.0, -512.0, -32.0],
        [512.0, 512.0, 0.0],
        [0.0; 3],
        floor,
        2,
        0,
        0,
    );
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("generated floor");
    let mut collision = CollisionModel::from_bsp(&bsp, &limits).expect("generated world");
    let floor = collision
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .expect("generated brush");
    (collision, floor)
}

#[test]
fn initial_descent_uses_each_selected_box_hull_and_requires_opt_in() {
    let collision = wall_room();
    for hull in [Hull::Standing, Hull::Large, Hull::Crouched] {
        let mut bridge = attachment_bridge(&collision, 64.0, 8);
        let actor = dummy_actor();
        let start = Vec3::new(-250.0, 0.0, hull.foot_offset() + 48.0);
        let generic = bridge.next_move(
            actor,
            start,
            Vec3::new(250.0, 0.0, hull.foot_offset()),
            hull,
            &collision,
            2.0,
        );
        assert_eq!(
            generic.z.to_bits(),
            start.z.to_bits(),
            "a box alone grants no permission"
        );
        let next = walking_query(&mut bridge, actor, start, hull, &collision, 2.0);
        assert!(next.z < start.z && (next - start).length() <= 2.001);
        assert_eq!(next.truncate(), start.truncate());
        let mut center = next;
        for _ in 0..24 {
            center = walking_query(&mut bridge, actor, center, hull, &collision, 2.0);
        }
        assert!(center.z >= hull.foot_offset() && center.z <= hull.foot_offset() + 1.0);
    }
    let mut bridge = attachment_bridge(&collision, 64.0, 8);
    let actor = dummy_actor();
    let start = Vec3::new(-250.0, 0.0, 96.0);
    let next = bridge.next_move_with_walking_attachment(
        actor,
        start,
        Vec3::new(250.0, 0.0, 96.0),
        Hull::Point,
        &collision,
        2.0,
        ohl_ai::monsters::Fallback::Traced,
    );
    assert_eq!(
        next.z.to_bits(),
        start.z.to_bits(),
        "flight never attaches to a ground floor"
    );
}

#[test]
fn initial_descent_does_not_change_direct_fallback_or_already_arrivable_queries() {
    let open = open_room();
    let mut bridge = attachment_bridge(&open, 64.0, 8);
    let actor = dummy_actor();
    let start = Vec3::new(-250.0, 0.0, 84.0);
    let next = bridge.next_move_with_walking_attachment(
        actor,
        start,
        Vec3::new(-150.0, 0.0, 84.0),
        Hull::Standing,
        &open,
        2.0,
        ohl_ai::monsters::Fallback::Traced,
    );
    assert_eq!(next.z.to_bits(), start.z.to_bits());
    assert_eq!(bridge.stats().direct_steps, 1);
    let collision = wall_room();
    let mut bridge = attachment_bridge(&collision, 64.0, 0);
    let next = walking_query(&mut bridge, actor, start, Hull::Standing, &collision, 2.0);
    assert_eq!(next.z.to_bits(), start.z.to_bits());
    assert_eq!(bridge.stats().traced_steps, 1);
    let mut bridge = attachment_bridge(&collision, 64.0, 8);
    let close = Vec3::new(-250.0, 0.0, 48.0);
    let next = walking_query(&mut bridge, actor, close, Hull::Standing, &collision, 2.0);
    assert_eq!(
        next.z.to_bits(),
        close.z.to_bits(),
        "existing 3D arrival remains unchanged"
    );
}

#[test]
fn initial_descent_holds_for_missing_deep_steep_or_intervening_support() {
    let built = wall_room();
    let actor = dummy_actor();
    let hull = Hull::Standing;
    let start = Vec3::new(-250.0, 0.0, 84.0);
    let mut no_floor = room_shell()[1..].to_vec();
    no_floor.push(CollisionBrush::box_brush(
        [-16.0, -128.0, -16.0],
        [16.0, 128.0, 256.0],
    ));
    let missing = model_from_brushes(&no_floor);
    let mut steep = no_floor;
    // This independent sloped plane gives the standing hull the same contact
    // height as the flat graph floor; only its unwalkable normal rejects it.
    steep.push(CollisionBrush::half_space([0.8, 0.0, 0.6], -212.8));
    let steep = model_from_brushes(&steep);
    let mut obstructed = room_shell();
    obstructed.push(CollisionBrush::box_brush(
        [-16.0, -128.0, -16.0],
        [16.0, 128.0, 256.0],
    ));
    // Off-center obstacle: the standing hull's descent lane hits it, while a
    // point cast beneath the actor would miss it.
    obstructed.push(CollisionBrush::box_brush(
        [-242.0, -2.0, 0.0],
        [-240.0, 2.0, 40.0],
    ));
    let obstructed = model_from_brushes(&obstructed);
    let mut embedded = room_shell();
    embedded.push(CollisionBrush::box_brush(
        [-260.0, -8.0, 70.0],
        [-240.0, 8.0, 90.0],
    ));
    let embedded = model_from_brushes(&embedded);
    assert!(
        embedded
            .trace(hull, start - Vec3::Z * 2.0, start - Vec3::Z * 2.0)
            .start_solid
    );
    for runtime in [&missing, &steep, &obstructed, &embedded] {
        let mut bridge = attachment_bridge(&built, 64.0, 8);
        let admitted = walking_query(&mut bridge, actor, start, hull, &built, 2.0);
        assert!(admitted.z < start.z);
        let stopped = walking_query(&mut bridge, actor, admitted, hull, runtime, 2.0);
        assert_eq!(
            stopped, admitted,
            "live invalid support cannot lower a pending actor"
        );
    }
    let mut bridge = attachment_bridge(&built, 16.0, 8);
    assert_eq!(
        walking_query(&mut bridge, actor, start, hull, &built, 2.0),
        start,
        "support beyond max_drop is rejected"
    );
}

#[test]
fn an_upward_initial_waypoint_preserves_existing_steering() {
    let mut brushes = room_shell();
    brushes.push(CollisionBrush::half_space([0.0, 0.0, 1.0], 48.0));
    brushes.push(CollisionBrush::box_brush(
        [-16.0, -128.0, -16.0],
        [16.0, 128.0, 256.0],
    ));
    let elevated = model_from_brushes(&brushes);
    let seeds: Vec<_> = wall_room_lattice()
        .into_iter()
        .map(|mut seed| {
            seed.position.z += 48.0;
            seed
        })
        .collect();
    let build = || {
        NavBridge::build(
            &seeds,
            &elevated,
            &BuildLimits::default(),
            NavBridgeLimits::default(),
        )
    };
    let mut generic = build();
    let mut walking = build();
    let runtime = wall_room();
    let actor = dummy_actor();
    let start = Vec3::new(-250.0, 0.0, 60.0);
    let goal = Vec3::new(250.0, 0.0, 84.0);
    let ordinary = generic.next_move(actor, start, goal, Hull::Standing, &runtime, 2.0);
    let opted = walking.next_move_with_walking_attachment(
        actor,
        start,
        goal,
        Hull::Standing,
        &runtime,
        2.0,
        ohl_ai::monsters::Fallback::Traced,
    );
    assert_eq!(walking.stats().graph_steps, 1);
    assert_eq!(opted, ordinary);
    assert_eq!(
        opted.z, start.z,
        "upward attachment is outside this descent slice"
    );
}

#[test]
fn initial_descent_rechecks_attached_support_and_keeps_its_original_drop_bound() {
    let (mut collision, floor) = wall_room_with_attached_floor();
    let actor = dummy_actor();
    let hull = Hull::Standing;
    let mut bridge = attachment_bridge(&collision, 64.0, 8);
    // Leave a quarter unit inside the drop bound: an endpoint exactly on
    // the clip plane is a clear trace, not an observed support hit. Moving
    // the floor down half a unit then takes it beyond the original bound.
    let start = Vec3::new(-250.0, 0.0, 99.75);
    let first = walking_query(&mut bridge, actor, start, hull, &collision, 2.0);
    assert!(first.z < start.z);
    collision.set_brush_solid(floor, false);
    assert_eq!(
        walking_query(&mut bridge, actor, first, hull, &collision, 2.0),
        first,
        "removed live floor stops descent"
    );
    collision.set_brush_solid(floor, true);
    collision.set_brush_origin(floor, Vec3::new(0.0, 0.0, 16.0));
    assert_eq!(
        walking_query(&mut bridge, actor, first, hull, &collision, 2.0),
        first,
        "a higher floor is not the selected node floor"
    );
    collision.set_brush_origin(floor, Vec3::new(0.0, 0.0, -0.5));
    assert_eq!(
        walking_query(&mut bridge, actor, first, hull, &collision, 2.0),
        first,
        "small slices cannot renew the original total drop allowance"
    );
    collision.set_brush_origin(floor, Vec3::ZERO);
    let resumed = walking_query(&mut bridge, actor, first, hull, &collision, 2.0);
    assert!(
        resumed.z < first.z,
        "restored live support permits the cached descent"
    );
}

#[test]
fn a_cached_pending_descent_survives_another_actors_search_budget_use() {
    let collision = wall_room();
    let mut bridge = attachment_bridge(&collision, 64.0, 1);
    let mut world = hecs::World::new();
    let actor = world.spawn(());
    let other = world.spawn(());
    let start = Vec3::new(-250.0, 0.0, 84.0);
    bridge.begin_tick(&[actor, other]);
    let first = walking_query(&mut bridge, actor, start, Hull::Standing, &collision, 2.0);
    assert!(first.z < start.z);
    bridge.begin_tick(&[actor, other]);
    let _ = bridge.next_move(
        other,
        Vec3::new(-300.0, 0.0, 40.0),
        Vec3::new(300.0, 0.0, 40.0),
        Hull::Standing,
        &collision,
        2.0,
    );
    let next = walking_query(&mut bridge, actor, first, Hull::Standing, &collision, 2.0);
    assert!(
        next.z < first.z,
        "actual cached center avoids another path search"
    );
    assert_eq!(bridge.stats().traced_steps, 0);
    assert_eq!(bridge.stats().untraced_steps, 0);
}

#[test]
fn an_external_anchor_change_rebuilds_the_old_graph_route() {
    let collision = wall_room();
    let mut bridge = NavBridge::build(
        &wall_room_lattice(),
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let goal = Vec3::new(250.0, 0.0, 40.0);
    let _ = bridge.next_move(
        actor,
        Vec3::new(-250.0, 0.0, 40.0),
        goal,
        Hull::Standing,
        &collision,
        1.0,
    );
    assert_eq!(bridge.stats().graph_steps, 1);
    // A carry/teleport changes the caller's anchor without changing its
    // destination. The new side of the wall has a clear direct segment.
    let origin = Vec3::new(200.0, 0.0, 40.0);
    let next = bridge.next_move(actor, origin, goal, Hull::Standing, &collision, 1.0);
    assert!(next.x > origin.x);
    assert_eq!(
        bridge.stats().direct_steps,
        1,
        "stale graph path was discarded"
    );
}

#[test]
fn reached_goal_is_bounded_and_traced_against_a_newly_closed_wall() {
    let open = open_room();
    let mut limits = NavBridgeLimits::default();
    limits.steer.arrive_radius = 100.0;
    let mut bridge = NavBridge::build(&[], &open, &BuildLimits::default(), limits);
    let actor = dummy_actor();
    let start = Vec3::new(0.0, 0.0, 40.0);
    let goal = Vec3::new(64.0, 0.0, 40.0);
    let next = bridge.next_move(actor, start, goal, Hull::Standing, &open, 1.0);
    assert!(
        (next - start).length() <= 1.001,
        "arrival does not snap beyond this step"
    );
    let mut brushes = room_shell();
    brushes.push(CollisionBrush::box_brush(
        [24.0, -512.0, 0.0],
        [26.0, 512.0, 256.0],
    ));
    let closed = model_from_brushes(&brushes);
    let blocked = bridge.next_move(actor, next, goal, Hull::Standing, &closed, 64.0);
    assert!(
        blocked.x < 8.01,
        "cached arrival cannot cross a live obstruction"
    );
    assert!(!closed.trace(Hull::Standing, blocked, blocked).start_solid);
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
fn with_no_nodes_and_a_blocked_line_the_bridge_falls_back_to_a_traced_step() {
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
    let traced = move_toward(&collision, hull, origin, goal, step, 1.0).position;
    assert_eq!(
        bridged, traced,
        "with no graph, the bridge is one traced step"
    );
    // In the open, that step is the straight line.
    assert_eq!(bridged, StraightLineNavigator.next_move(origin, goal, step));

    // Driven on into the wall, it stops at it: the fallback never carries a
    // monster through what its own hull trace says is solid.
    let mut pos = origin;
    for _ in 0..200 {
        bridge.begin_tick(&[actor]);
        pos = bridge.next_move(actor, pos, goal, hull, &collision, step);
    }
    assert!(
        pos.x < -16.0 - 15.0,
        "the fallback walked a monster through the wall: {pos:?}"
    );
}

/// A flier is not routed through ground nodes. With only a ground lattice
/// and the wall between it and its goal, a point-hull mover high in the
/// room gets no graph route (every one would run through floor-level
/// waypoints) and flies its traced fallback instead, at its own height,
/// rather than diving to the floor to follow the walkers' graph.
#[test]
fn a_flier_is_not_routed_through_ground_nodes() {
    let collision = wall_room();
    let seeds = wall_room_lattice();
    let mut bridge = NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let goal = Vec3::new(300.0, 0.0, 200.0);
    let mut pos = Vec3::new(-300.0, 0.0, 200.0);
    let mut lowest = pos.z;
    for _ in 0..600 {
        bridge.begin_tick(&[actor]);
        pos = bridge.next_move(actor, pos, goal, Hull::Point, &collision, 4.0);
        lowest = lowest.min(pos.z);
    }
    assert!(lowest > 150.0, "the flier dived toward the floor: {lowest}");
    assert!(pos.x <= -16.0, "and did not pass through the wall: {pos:?}");
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
    let traced = move_toward(&collision, hull, origin, goal, step, 1.0).position;
    assert_eq!(
        second_move, traced,
        "budget exhausted: the traced fallback step expected"
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

/// The steering's stuck window is measured against the mover's own pace:
/// a walker at 32 units per second, at the project's 100 Hz tick, walks
/// straight to a goal in an open room instead of being told it is stuck —
/// its 20-tick window covers 6.4 units, short of the default 8 — and
/// side-stepping off its line. (The default walk of 40 sits exactly on
/// that line, where a rounding error decides it.)
#[test]
fn a_slow_walker_is_not_taken_for_stuck() {
    let collision = open_room();
    let mut bridge = NavBridge::build(
        &[],
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    );
    let actor = dummy_actor();
    let goal = Vec3::new(100.0, 0.0, 40.0);
    let step = 32.0 * 0.01;
    let mut pos = Vec3::new(-100.0, 0.0, 40.0);
    let mut max_abs_y = 0.0f32;
    let mut ticks = 0;
    while (pos - goal).length() > 1.0 && ticks < 2_000 {
        bridge.begin_tick(&[actor]);
        pos = bridge.next_move(actor, pos, goal, Hull::Standing, &collision, step);
        max_abs_y = max_abs_y.max(pos.y.abs());
        ticks += 1;
    }
    assert!(
        ticks <= 640,
        "200 units at 0.32 a tick took {ticks} ticks, ending at {pos:?}"
    );
    assert!(max_abs_y < 1.0, "it side-stepped off its line: {max_abs_y}");
}
