//! Clip-hull tracing against the project's synthetic collision fixtures.

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{
    Bsp30Builder, build_brush_entity_floor_bsp, build_collision_room_bsp, build_collision_slope_bsp,
};
use ohl_physics::{CollisionModel, Hull, Trace, Vec3, contents, point_contents, trace_hull};

fn model_from(bytes: &[u8]) -> CollisionModel {
    let limits = Limits::default();
    let bsp = Bsp::parse(bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

fn room() -> CollisionModel {
    model_from(&build_collision_room_bsp())
}

/// Asserts the documented invariant that a trace's end position lies on the
/// traced segment at its reported fraction.
fn assert_on_segment(trace: &Trace, start: Vec3, end: Vec3) {
    let expected = start + (end - start) * trace.fraction;
    assert!(
        (trace.end_pos - expected).length() < 0.05,
        "end_pos {:?} is not at fraction {} of {start:?} -> {end:?}",
        trace.end_pos,
        trace.fraction
    );
}

#[test]
fn standing_hull_lands_on_the_floor_at_the_analytic_fraction() {
    let model = room();
    let start = Vec3::new(0.0, 0.0, 100.0);
    let end = Vec3::new(0.0, 0.0, -100.0);
    let trace = model.trace(Hull::Standing, start, end);

    // The standing hull's bottom is 36 units below the origin, so the origin
    // stops 36 units above the floor (plus the trace epsilon).
    let expected_z = 36.0;
    let expected_fraction = (100.0 - expected_z) / 200.0;
    assert!(
        (trace.fraction - expected_fraction).abs() < 0.001,
        "fraction {} is not the analytic {expected_fraction}",
        trace.fraction
    );
    assert!((trace.end_pos.z - expected_z).abs() < 0.1);
    assert!((trace.plane_normal - Vec3::Z).length() < 1e-3);
    assert!(!trace.start_solid && !trace.all_solid);
    assert!(trace.in_open);
    assert_on_segment(&trace, start, end);
}

#[test]
fn point_hull_reaches_the_floor_surface_itself() {
    let model = room();
    let trace = model.trace(
        Hull::Point,
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
    );
    assert!(trace.end_pos.z > 0.0 && trace.end_pos.z < 0.1);
}

#[test]
fn crouched_hull_stops_lower_than_the_standing_hull() {
    let model = room();
    let start = Vec3::new(0.0, 0.0, 100.0);
    let end = Vec3::new(0.0, 0.0, -100.0);
    let standing = model.trace(Hull::Standing, start, end);
    let crouched = model.trace(Hull::Crouched, start, end);
    assert!((crouched.end_pos.z - 18.0).abs() < 0.1);
    assert!(crouched.end_pos.z < standing.end_pos.z);
}

#[test]
fn trace_into_a_wall_reports_the_wall_normal() {
    let model = room();
    // Well above the 18-unit step, so the wall is the first thing in the way.
    let start = Vec3::new(0.0, 0.0, 150.0);
    let end = Vec3::new(400.0, 0.0, 150.0);
    let trace = model.trace(Hull::Standing, start, end);

    // The wall's inner face is at x = 256 and the hull is 16 units wide from
    // its origin, so the origin stops at x = 240.
    assert!((trace.end_pos.x - 240.0).abs() < 0.1, "{:?}", trace.end_pos);
    assert!((trace.plane_normal - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-3);
    assert_on_segment(&trace, start, end);
}

#[test]
fn trace_through_open_space_hits_nothing() {
    let model = room();
    let start = Vec3::new(0.0, 0.0, 100.0);
    let end = Vec3::new(0.0, 0.0, 150.0);
    let trace = model.trace(Hull::Standing, start, end);
    assert!((trace.fraction - 1.0).abs() < f32::EPSILON);
    assert_eq!(trace.end_pos, end);
    assert_eq!(trace.plane_normal, Vec3::ZERO);
    assert!(!trace.blocked());
    assert_eq!(trace.contents, contents::EMPTY);
}

#[test]
fn a_trace_that_starts_inside_solid_is_start_solid() {
    let model = room();
    let start = Vec3::new(0.0, 0.0, -40.0);
    let end = Vec3::new(0.0, 0.0, 100.0);
    let trace = model.trace(Hull::Standing, start, end);
    assert!(trace.start_solid);
    assert!((trace.fraction - 0.0).abs() < f32::EPSILON);
    assert_eq!(trace.end_pos, start);
}

#[test]
fn a_trace_entirely_inside_solid_is_all_solid() {
    let model = room();
    let trace = model.trace(
        Hull::Standing,
        Vec3::new(0.0, 0.0, -200.0),
        Vec3::new(0.0, 0.0, -180.0),
    );
    assert!(trace.all_solid && trace.start_solid);
    assert!(!trace.in_open);
}

#[test]
fn point_contents_distinguishes_solid_from_empty() {
    let model = room();
    assert_eq!(
        point_contents(&model, Vec3::new(0.0, 0.0, 100.0)),
        contents::EMPTY
    );
    assert_eq!(
        point_contents(&model, Vec3::new(0.0, 0.0, -10.0)),
        contents::SOLID
    );
    // Inside the 18-unit step.
    assert_eq!(
        point_contents(&model, Vec3::new(128.0, 0.0, 9.0)),
        contents::SOLID
    );
    // Just above it.
    assert_eq!(
        point_contents(&model, Vec3::new(128.0, 0.0, 24.0)),
        contents::EMPTY
    );
}

#[test]
fn the_free_function_rejects_hull_indices_above_three() {
    let model = room();
    let start = Vec3::new(0.0, 0.0, 100.0);
    let end = Vec3::new(0.0, 0.0, 50.0);
    for index in 0..4 {
        assert!(!trace_hull(&model, index, start, end).start_solid);
    }
    let bad = trace_hull(&model, 4, start, end);
    assert!(bad.start_solid && bad.all_solid);
    assert_eq!(bad.end_pos, start);
}

#[test]
fn hull_selection_follows_the_documented_size_table() {
    assert_eq!(
        Hull::for_size(Vec3::ZERO, Vec3::ZERO),
        Hull::Point,
        "a zero-sized box uses the point hull"
    );
    assert_eq!(
        Hull::for_size(Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0)),
        Hull::Standing
    );
    assert_eq!(
        Hull::for_size(Vec3::new(-16.0, -16.0, -18.0), Vec3::new(16.0, 16.0, 18.0)),
        Hull::Crouched
    );
    assert_eq!(
        Hull::for_size(Vec3::splat(-32.0), Vec3::splat(32.0)),
        Hull::Large
    );
    assert!((Hull::Standing.foot_offset() - 36.0).abs() < f32::EPSILON);
    assert!((Hull::Crouched.foot_offset() - 18.0).abs() < f32::EPSILON);
}

#[test]
fn a_slope_reports_its_own_normal() {
    let model = model_from(&build_collision_slope_bsp(-0.6, 0.8));
    let trace = model.trace(
        Hull::Standing,
        Vec3::new(0.0, 0.0, 200.0),
        Vec3::new(0.0, 0.0, -200.0),
    );
    assert!(trace.fraction < 1.0);
    assert!((trace.plane_normal - Vec3::new(0.0, -0.6, 0.8)).length() < 1e-3);
}

#[test]
fn a_cyclic_hull_tree_is_reported_as_blocked_instead_of_recursing() {
    // A clipnode whose children both point back at itself would loop
    // forever without the traversal depth limit.
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    builder.push_plane([0.0, 0.0, 1.0], 0.0, 3);
    builder.push_leaf(-2, -1, [0; 3], [0; 3], 0, 0, [0; 4]);
    builder.push_node(0, -1, -1, [0; 3], [0; 3], 0, 0);
    builder.push_clipnode(0, 0, 0);
    builder.push_model([-64.0; 3], [64.0; 3], [0.0; 3], [0, 0, 0, 0], 1, 0, 0);
    let bytes = builder.build();
    let model = model_from(&bytes);
    let trace = model.trace(
        Hull::Standing,
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
    );
    assert!(trace.start_solid);
    assert_eq!(
        model.contents_at(Hull::Standing, Vec3::ZERO),
        contents::SOLID
    );
}

#[test]
fn a_map_with_an_out_of_range_child_is_rejected() {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    builder.push_plane([0.0, 0.0, 1.0], 0.0, 3);
    builder.push_leaf(-2, -1, [0; 3], [0; 3], 0, 0, [0; 4]);
    builder.push_node(0, -1, -1, [0; 3], [0; 3], 0, 0);
    // Child 9 does not exist.
    builder.push_clipnode(0, 9, -2);
    builder.push_model([-64.0; 3], [64.0; 3], [0.0; 3], [0, 0, 0, 0], 1, 0, 0);
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("parses");
    assert!(CollisionModel::from_bsp(&bsp, &limits).is_err());
}

// ---------------------------------------------------------------------
// Brush-entity hulls
//
// A map's worldspawn model is not the whole collision world: the compiler
// moves every brush entity into its own `BSPMODEL`, so a floor built as a
// `func_wall` is absent from submodel 0 entirely. These tests use the
// `build_brush_entity_floor_bsp` fixture, whose worldspawn model is an
// empty void and whose submodel 1 is a floor slab.
// ---------------------------------------------------------------------

/// The fixture's void world plus its slab attached as a solid brush entity.
fn brush_floor() -> CollisionModel {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model =
        CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls");
    model
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .expect("the fixture declares submodel 1");
    model
}

#[test]
fn the_worldspawn_model_alone_has_no_floor_to_stand_on() {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");

    assert_eq!(model.brush_count(), 0);
    let trace = model.trace(
        Hull::Standing,
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
    );
    assert!(
        (trace.fraction - 1.0).abs() < f32::EPSILON,
        "the void world stopped a fall it has nothing to stop with"
    );
}

#[test]
fn an_attached_brush_entity_stops_the_same_fall() {
    let model = brush_floor();
    assert_eq!(model.brush_count(), 1);

    let start = Vec3::new(0.0, 0.0, 100.0);
    let end = Vec3::new(0.0, 0.0, -100.0);
    let trace = model.trace(Hull::Standing, start, end);

    assert!(trace.fraction < 1.0, "the slab did not stop the fall");
    // The standing hull's bottom is 36 units below its origin, so the origin
    // comes to rest 36 units above the slab's top surface.
    assert!(
        (trace.end_pos.z - 36.0).abs() < 0.2,
        "stopped at {} rather than on the slab",
        trace.end_pos.z
    );
    assert!(trace.plane_normal.z > 0.9, "the hit plane is not a floor");
    assert_on_segment(&trace, start, end);
}

#[test]
fn a_fall_beside_the_slab_still_passes_through() {
    let model = brush_floor();
    let trace = model.trace(
        Hull::Standing,
        Vec3::new(512.0, 0.0, 100.0),
        Vec3::new(512.0, 0.0, -100.0),
    );
    assert!(
        (trace.fraction - 1.0).abs() < f32::EPSILON,
        "a brush 512 units away blocked the fall"
    );
}

#[test]
fn moving_a_brush_moves_what_it_blocks() {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    let brush = model
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .expect("the fixture declares submodel 1");

    // Slide the slab 512 units along +X, the way a door's map logic would.
    model.set_brush_origin(brush, Vec3::new(512.0, 0.0, 0.0));

    let over_the_old_place = model.trace(
        Hull::Standing,
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, -100.0),
    );
    assert!(
        (over_the_old_place.fraction - 1.0).abs() < f32::EPSILON,
        "the slab still blocks where it no longer is"
    );

    let over_the_new_place = model.trace(
        Hull::Standing,
        Vec3::new(512.0, 0.0, 100.0),
        Vec3::new(512.0, 0.0, -100.0),
    );
    assert!(
        over_the_new_place.fraction < 1.0,
        "the slab does not block where it now is"
    );
    assert!((over_the_new_place.end_pos.z - 36.0).abs() < 0.2);
}

#[test]
fn a_point_inside_an_attached_brush_reads_as_solid() {
    let model = brush_floor();
    assert!(contents::is_solid(point_contents(
        &model,
        Vec3::new(0.0, 0.0, -8.0)
    )));
    assert!(!contents::is_solid(point_contents(
        &model,
        Vec3::new(0.0, 0.0, 64.0)
    )));
}

#[test]
fn attaching_the_worldspawn_model_or_a_missing_one_is_rejected() {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");

    assert!(model.attach_brush(&bsp, &limits, 0, Vec3::ZERO).is_err());
    assert!(model.attach_brush(&bsp, &limits, 99, Vec3::ZERO).is_err());
    assert!(
        model
            .attach_brush(&bsp, &limits, 1, Vec3::new(f32::NAN, 0.0, 0.0))
            .is_err()
    );
    assert_eq!(model.brush_count(), 0);
}
