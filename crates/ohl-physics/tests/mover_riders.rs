//! Riding an attached brush entity that moves up, down, or sideways.
//!
//! Unlike `player_systems.rs`'s existing
//! `standing_on_a_moving_platform_carries_the_player_along` (which hands
//! `player_move_events` a hand-picked `base_velocity` directly), these
//! tests drive a *real* attached brush with
//! `CollisionModel::set_brush_origin` each tick and check
//! `PlayerState::ground_brush` — the piece `ohl-engine`'s `Level`
//! (`brush_velocity`, `sync_brush_collision`) and `Systems::player_move`
//! use to look a mover's velocity up and feed it back in as
//! `base_velocity`, without needing the whole engine here. No wish input is
//! given in any of these; the player only rides.
//!
//! See `docs/FORMAT_SOURCES.md`, "Riding movers", and
//! `crates/ohl-engine/tests/mover_riders.rs` for the same behaviour through
//! a real `func_train` and `Simulation`.

use ohl_physics::controller::TICK_SECONDS;
use ohl_physics::movement::categorize_position;
use ohl_physics::test_support::build_platform_room;
use ohl_physics::{CollisionModel, MoveConfig, MoveInput, PlayerState, Vec3};

const TICK: f32 = TICK_SECONDS;

/// The origin height of a standing player resting on the platform fixture's
/// slab, whose top surface is at `z = 0` (`ohl_formats::test_support::
/// BRUSH_FLOOR_TOP_Z`, which `build_platform_room` builds from).
const RIDER_ORIGIN_Z: f32 = 36.0;

/// Runs `ticks` steps of zero-input movement, moving `brush` by
/// `velocity_per_tick` each step first (mirroring `ohl-engine`'s own order:
/// `sync_brush_collision` moves the brush before the player-move phase
/// runs), and feeding `velocity` back in as `base_velocity` whenever the
/// player is standing on `brush` at the start of the tick — exactly what
/// `Systems::player_move` computes from `PlayerState::ground_brush` and
/// `Level::brush_velocity`.
fn ride(
    model: &mut CollisionModel,
    brush: ohl_physics::BrushId,
    state: &mut PlayerState,
    brush_origin: &mut Vec3,
    velocity: Vec3,
    ticks: u32,
) {
    let config = MoveConfig::default();
    for _ in 0..ticks {
        *brush_origin += velocity * TICK;
        model.set_brush_origin(brush, *brush_origin);
        let base_velocity = if state.ground_brush == Some(brush) {
            velocity
        } else {
            Vec3::ZERO
        };
        let input = MoveInput {
            base_velocity,
            ..MoveInput::default()
        };
        ohl_physics::player_move_events(model, state, &input, &config, TICK);
    }
}

/// Primes `state.ground_brush` by running one zero-input, zero-velocity
/// tick before the platform starts moving, the same as a player who was
/// already standing still on it.
fn settle_onto(model: &CollisionModel, state: &mut PlayerState) {
    let config = MoveConfig::default();
    ohl_physics::player_move_events(model, state, &MoveInput::default(), &config, TICK);
    assert!(state.on_ground, "the player did not settle onto the slab");
}

#[test]
fn a_rising_platform_carries_the_player_along_with_zero_input() {
    let (mut model, brush) = build_platform_room();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z));
    settle_onto(&model, &mut state);
    assert_eq!(state.ground_brush, Some(brush));

    let start = state.origin;
    let mut brush_origin = Vec3::ZERO;
    let velocity = Vec3::new(0.0, 0.0, 50.0);
    ride(
        &mut model,
        brush,
        &mut state,
        &mut brush_origin,
        velocity,
        200,
    );

    // 200 ticks at `TICK_SECONDS` each, at 50 units/s: the platform (and
    // the player riding it) should have risen by very close to that.
    let expected_delta = brush_origin; // brush started at Vec3::ZERO
    let actual_delta = state.origin - start;
    assert!(
        (actual_delta - expected_delta).length() < 2.0,
        "expected to rise by {expected_delta:?}, actually moved {actual_delta:?}"
    );
    assert!(state.on_ground, "the player was left airborne by the ride");
    assert_eq!(state.ground_brush, Some(brush));
}

#[test]
fn a_descending_platform_keeps_ground_contact() {
    let (mut model, brush) = build_platform_room();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z));
    settle_onto(&model, &mut state);

    let start = state.origin;
    let mut brush_origin = Vec3::ZERO;
    let velocity = Vec3::new(0.0, 0.0, -50.0);
    let config = MoveConfig::default();
    for _ in 0..200 {
        brush_origin += velocity * TICK;
        model.set_brush_origin(brush, brush_origin);
        let base_velocity = if state.ground_brush == Some(brush) {
            velocity
        } else {
            Vec3::ZERO
        };
        let input = MoveInput {
            base_velocity,
            ..MoveInput::default()
        };
        ohl_physics::player_move_events(&model, &mut state, &input, &config, TICK);
        // A vertical mover is tracked directly (see
        // `ohl_physics::movement::ride_vertical_mover`'s module-internal
        // doc, exercised here through `player_move_events`), so ground
        // contact must never be lost mid-ride, not just at the end.
        assert!(
            state.on_ground,
            "lost ground contact while descending at brush z = {}",
            brush_origin.z
        );
    }

    let expected_delta = brush_origin;
    let actual_delta = state.origin - start;
    assert!(
        (actual_delta - expected_delta).length() < 2.0,
        "expected to descend by {expected_delta:?}, actually moved {actual_delta:?}"
    );
}

#[test]
fn a_horizontal_train_carries_the_player_along_with_zero_input() {
    let (mut model, brush) = build_platform_room();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z));
    settle_onto(&model, &mut state);

    let start = state.origin;
    let mut brush_origin = Vec3::ZERO;
    let velocity = Vec3::new(80.0, 0.0, 0.0);
    ride(
        &mut model,
        brush,
        &mut state,
        &mut brush_origin,
        velocity,
        200,
    );

    let expected_delta = brush_origin;
    let actual_delta = state.origin - start;
    assert!(
        (actual_delta - expected_delta).length() < 2.0,
        "expected to travel {expected_delta:?}, actually moved {actual_delta:?}"
    );
    assert!(state.on_ground, "the player fell off the moving train");
    // The ride is not stored as the player's own velocity: it does not
    // persist once the platform stops.
    let held = state.origin;
    for _ in 0..20 {
        ohl_physics::player_move_events(
            &model,
            &mut state,
            &MoveInput::default(),
            &MoveConfig::default(),
            TICK,
        );
    }
    assert!(
        (state.origin - held).length() < 1.0,
        "the ride left residual velocity: moved to {:?} after stopping",
        state.origin
    );
}

/// `ground_probe`'s one-unit retry (`crate::movement`) is restricted to a
/// ground brush that is currently *rotating* — the review of PR #110 (which
/// added the retry) found the shipped version fired for any attached brush,
/// including a `func_train`/`func_plat`-style translating one that never
/// moved this tick. This pins down that an ordinary, motionless translating
/// mover's shallow (up to one unit) embed still reports `on_ground = false`
/// with the origin untouched, exactly as it did before PR #110 — matching
/// the review's own measurement of `z_in` 39.00-39.75 on a brush resting at
/// 39 (`RIDER_ORIGIN_Z` here is 36, so the same one-unit window is
/// `35.00..=36.00`).
#[test]
fn a_shallow_embed_on_a_stationary_translating_brush_is_not_snapped_up() {
    let (model, _brush) = build_platform_room();
    let config = MoveConfig::default();

    for embed in [0.05_f32, 0.25, 0.5, 0.75, 1.0] {
        let origin = Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z - embed);
        let mut state = PlayerState::at(origin);
        categorize_position(&model, &mut state, &config);

        assert!(
            !state.on_ground,
            "a {embed}-unit embed on a stationary translating brush must not \
             report on_ground (got origin {:?})",
            state.origin
        );
        assert_eq!(
            state.origin, origin,
            "a stationary translating brush's embed must leave the origin untouched"
        );
    }
}
