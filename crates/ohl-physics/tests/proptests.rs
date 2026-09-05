//! Property tests: whatever it is asked to trace or simulate, this crate
//! must terminate, stay finite, and honour its documented invariants.

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::controller::TICK_SECONDS;
use ohl_physics::{
    CollisionModel, MoveConfig, MoveInput, PlayerState, Vec3, player_move, trace_hull,
};
use proptest::prelude::*;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
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
}
