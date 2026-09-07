//! Property tests: whatever it is asked to trace or simulate, this crate
//! must terminate, stay finite, and honour its documented invariants.

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{build_brush_entity_floor_bsp, build_collision_room_bsp};
use ohl_physics::controller::TICK_SECONDS;
use ohl_physics::movement::{in_ladder_volume, ladder_normal};
use ohl_physics::test_support::{build_ladder_entity_room_bsp, build_thin_ladder_room_bsp};
use ohl_physics::{
    CollisionModel, ContentsKind, Hull, MoveConfig, MoveInput, PlayerState, Vec3, player_move,
    trace_hull,
};
use proptest::prelude::*;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// [`build_brush_entity_floor_bsp`]'s slab, attached as a solid brush
/// entity, alongside the same model with that brush's broad-phase bounds
/// widened to cover the whole coordinate space (see
/// [`CollisionModel::widen_brush_bounds_for_test`]). The two differ only in
/// whether `trace`/`contents_at`'s broad phase ever gets a chance to skip
/// this brush's tree walk; comparing them is exactly how the broad phase's
/// central claim — "skipping never changes the answer" — gets checked.
fn brush_with_and_without_broad_phase() -> (CollisionModel, CollisionModel) {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut narrow = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    let brush = narrow
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .expect("the fixture declares submodel 1");
    let mut wide = narrow.clone();
    wide.widen_brush_bounds_for_test(brush);
    (narrow, wide)
}

/// [`build_ladder_entity_room_bsp`]'s submodel 1, attached as a
/// `ContentsKind::Ladder` contents volume rather than solid — the fixture
/// this crate's own doc comments point to as "the submodel any brush
/// entity actually compiles to" (an ordinary solid-shaped brush); this
/// model additionally still has the world's own floor and backing wall
/// attached, so a proptest against arbitrary segments still has real solid
/// geometry to hit *outside* the volume.
fn ladder_contents_room() -> CollisionModel {
    let bytes = build_ladder_entity_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    model
        .attach_contents_brush(&bsp, &limits, 1, Vec3::ZERO, ContentsKind::Ladder)
        .expect("the fixture declares submodel 1");
    model
}

/// The same fixture's world geometry alone, with submodel 1 never
/// attached at all — the baseline [`ladder_contents_room`] is compared
/// against to isolate exactly what attaching the ladder as a contents
/// volume changes about a trace.
fn ladder_room_world_only() -> CollisionModel {
    let bytes = build_ladder_entity_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls")
}

/// [`build_thin_ladder_room_bsp`]'s free-standing, 8-unit-thick ladder
/// slab: thin enough that a hull placed almost anywhere near it either
/// misses it entirely or straddles both of its faces at once, which is
/// exactly the geometry that should stress
/// [`ohl_physics::movement::ladder_normal`]'s hull-sample probe.
fn thin_ladder_room() -> CollisionModel {
    let bytes = build_thin_ladder_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls")
}

prop_compose! {
    fn any_point()(
        x in -1000.0f32..1000.0,
        y in -1000.0f32..1000.0,
        z in -1000.0f32..1000.0,
    ) -> Vec3 {
        Vec3::new(x, y, z)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn a_trace_always_reports_a_fraction_on_the_segment(
        start in any_point(),
        end in any_point(),
        hull_index in 0usize..4,
    ) {
        let model = room();
        let trace = trace_hull(&model, hull_index, start, end);

        prop_assert!((0.0..=1.0).contains(&trace.fraction), "fraction {}", trace.fraction);
        prop_assert!(trace.end_pos.is_finite());
        prop_assert!(trace.plane_normal.is_finite());

        let expected = start + (end - start) * trace.fraction;
        let tolerance = 0.05 + (end - start).length() * 1e-4;
        prop_assert!(
            (trace.end_pos - expected).length() <= tolerance,
            "end_pos {:?} is not at fraction {} of {start:?} -> {end:?}",
            trace.end_pos,
            trace.fraction
        );

        // A move that was stopped must report the surface that stopped it.
        if trace.fraction < 1.0 && !trace.start_solid {
            prop_assert!(trace.plane_normal.length() > 0.9);
        }
    }

    #[test]
    fn a_trace_to_the_same_point_never_moves(point in any_point(), hull_index in 0usize..4) {
        let model = room();
        let trace = trace_hull(&model, hull_index, point, point);
        prop_assert!((trace.end_pos - point).length() < 1e-3);
    }

    #[test]
    fn moving_never_produces_a_non_finite_state(
        start in any_point(),
        wish in any_point(),
        jump in any::<bool>(),
        duck in any::<bool>(),
        ticks in 1u32..40,
    ) {
        let model = room();
        let config = MoveConfig::default();
        let mut state = PlayerState::at(start);
        let input = MoveInput {
            wish_move: wish.normalize_or_zero(),
            jump,
            duck,
            ..MoveInput::default()
        };
        for _ in 0..ticks {
            player_move(&model, &mut state, &input, &config, TICK_SECONDS);
            prop_assert!(state.origin.is_finite());
            prop_assert!(state.velocity.is_finite());
            prop_assert!(state.velocity.abs().max_element() <= config.max_velocity + 1.0);
        }
    }

    #[test]
    fn a_player_that_starts_in_open_space_stays_out_of_solid(
        x in -200.0f32..200.0,
        y in -100.0f32..100.0,
        wish_x in -1.0f32..1.0,
        wish_y in -1.0f32..1.0,
    ) {
        // Start well inside the room and above the tallest obstruction.
        let model = room();
        let config = MoveConfig::default();
        let mut state = PlayerState::at(Vec3::new(x, y, 120.0));
        let input = MoveInput {
            wish_move: Vec3::new(wish_x, wish_y, 0.0).normalize_or_zero(),
            jump: false,
            duck: false,
            ..MoveInput::default()
        };
        for _ in 0..200 {
            player_move(&model, &mut state, &input, &config, TICK_SECONDS);
        }
        // The hull trace from the final position to itself must not be
        // inside solid: movement never pushes the player into geometry.
        let trace = model.trace(state.hull(), state.origin, state.origin);
        prop_assert!(!trace.start_solid, "ended inside solid at {:?}", state.origin);
    }

    #[test]
    fn the_broad_phase_never_changes_a_trace(
        start in any_point(),
        end in any_point(),
        hull_index in 0usize..4,
    ) {
        let (narrow, wide) = brush_with_and_without_broad_phase();
        let with_broad_phase = trace_hull(&narrow, hull_index, start, end);
        let without_broad_phase = trace_hull(&wide, hull_index, start, end);
        prop_assert_eq!(with_broad_phase, without_broad_phase);
    }

    #[test]
    fn the_broad_phase_never_changes_a_contents_query(
        point in any_point(),
        hull_index in 0usize..4,
    ) {
        let (narrow, wide) = brush_with_and_without_broad_phase();
        let hull = Hull::from_index(hull_index).expect("0..4 is always a valid hull index");
        prop_assert_eq!(narrow.contents_at(hull, point), wide.contents_at(hull, point));
    }

    /// The invariant `CollisionModel::attach_contents_brush`'s doc comment
    /// states directly: attaching a `func_ladder`/`func_water` submodel as
    /// a contents volume can never make a trace more blocked than the same
    /// world geometry without it. Comparing against the model with
    /// submodel 1 never attached at all isolates exactly that: every field
    /// that decides whether/where a move was stopped
    /// (`fraction`/`end_pos`/`start_solid`/`all_solid`/`plane_normal`/
    /// `plane_dist`) must come out identical, whatever the world's own
    /// solid geometry (the fixture's floor and backing wall) independently
    /// does. `in_water`/`in_open`/`contents` are deliberately excluded:
    /// those are exactly what the contents volume is supposed to change.
    /// The comparisons are deliberately *exact*: both traces run the same
    /// deterministic floating-point arithmetic over the same world
    /// geometry, differing only by whether a brush loop iteration that can
    /// never change the outcome ran at all, so bit-for-bit equality (not an
    /// epsilon compare) is the correct check.
    #[test]
    #[allow(clippy::float_cmp)]
    fn a_contents_ladder_volume_never_blocks_more_than_the_bare_world(
        start in any_point(),
        end in any_point(),
        hull_index in 0usize..4,
    ) {
        let with_ladder = ladder_contents_room();
        let world_only = ladder_room_world_only();
        let hull = Hull::from_index(hull_index).expect("0..4 is always a valid hull index");

        let attached = with_ladder.trace(hull, start, end);
        let bare = world_only.trace(hull, start, end);

        prop_assert_eq!(attached.fraction, bare.fraction);
        prop_assert_eq!(attached.end_pos, bare.end_pos);
        prop_assert_eq!(attached.start_solid, bare.start_solid);
        prop_assert_eq!(attached.all_solid, bare.all_solid);
        prop_assert_eq!(attached.plane_normal, bare.plane_normal);
        prop_assert_eq!(attached.plane_dist, bare.plane_dist);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The hull-aware ladder probe ([`ohl_physics::movement::in_ladder_volume`]
    /// / [`ohl_physics::movement::ladder_normal`], PR #100 review follow-up)
    /// must never panic for an arbitrary hull placement — including origins
    /// far outside the map's own coordinate bounds, exactly on a brush
    /// face, or straddling the ladder volume's thin axis — and must always
    /// report a normal that is finite, at most unit length, and zero
    /// exactly when the hull was not found to be in a ladder volume at all.
    #[test]
    fn the_ladder_probe_never_panics_for_an_arbitrary_hull_placement(
        point in any_point(),
        ducked in any::<bool>(),
        thin in any::<bool>(),
    ) {
        let model = if thin { thin_ladder_room() } else { ladder_contents_room() };
        let mut state = PlayerState::at(point);
        state.ducked = ducked;

        let inside = in_ladder_volume(&model, &state);
        let normal = ladder_normal(&model, &state);

        prop_assert!(normal.is_finite());
        prop_assert!(normal.length() <= 1.0 + 1e-4, "normal {normal:?}");
        if !inside {
            prop_assert_eq!(normal, Vec3::ZERO);
        }
    }
}
