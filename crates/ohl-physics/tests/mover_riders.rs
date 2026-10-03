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
use ohl_physics::test_support::{build_platform_room, build_two_platform_room};
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

/// The slab's own half-extent in `x`/`y` (`ohl_formats::test_support::
/// BRUSH_FLOOR_HALF_EXTENT`), which `build_platform_room` builds from.
const SLAB_HALF_EXTENT: f32 = 128.0;

/// A mover that slides its face into a player standing beside it pushes
/// them along by its own move — `push_from_mover`'s destination test finds
/// the pushed position clear and moves them there — and reports the push
/// as not blocked.
#[test]
fn a_mover_sliding_into_the_player_pushes_them_its_own_distance() {
    let (mut model, brush) = build_platform_room();
    // Standing beside the slab's `+x` face: the 32-wide standing hull's
    // `-x` side sits one unit off the face, the hull's `z` span (`-36..36`
    // about the origin) straddling the slab's own (`-16..0`).
    let origin = Vec3::new(SLAB_HALF_EXTENT + 16.0 + 1.0, 0.0, 0.0);
    let mut state = PlayerState::at(origin);
    assert!(
        !model.trace(state.hull(), origin, origin).start_solid,
        "the player starts clear of the slab"
    );

    // The slab moves 4 units into them this step.
    let displacement = Vec3::new(4.0, 0.0, 0.0);
    model.set_brush_origin(brush, displacement);
    assert!(
        model.trace(state.hull(), origin, origin).start_solid,
        "the slab's move embeds the player"
    );

    assert!(
        ohl_physics::push_from_mover(&model, &mut state, displacement),
        "a push with a clear destination is not a block"
    );
    assert_eq!(state.origin, origin + displacement);
    assert!(
        !model
            .trace(state.hull(), state.origin, state.origin)
            .start_solid,
        "the pushed player is clear of the slab again"
    );
}

/// A push whose destination is still inside solid — here, a player
/// embedded far deeper than one step's move can clear — moves nobody and
/// reports the mover blocked, which is what a door reversal and its `dmg`
/// hang off.
#[test]
fn a_push_that_cannot_clear_the_player_is_reported_blocked() {
    let (mut model, brush) = build_platform_room();
    let origin = Vec3::new(SLAB_HALF_EXTENT + 16.0 + 1.0, 0.0, 0.0);
    let mut state = PlayerState::at(origin);
    // The slab lands 40 units into them; the step's own move is only 4.
    model.set_brush_origin(brush, Vec3::new(40.0, 0.0, 0.0));
    assert!(model.trace(state.hull(), origin, origin).start_solid);

    assert!(
        !ohl_physics::push_from_mover(&model, &mut state, Vec3::new(4.0, 0.0, 0.0)),
        "a push that leaves the player embedded is a block"
    );
    assert_eq!(state.origin, origin, "a blocked push moves nobody");
}

/// A rider a hair inside the slab under them (a rising lift's top face, a
/// step before the ride blend catches them up), pushed sideways by a
/// second slab sliding across at chest height. Judged against the whole
/// model, the push's destination is still inside the lift and reads as a
/// block; with the lift left out, it is clear, and the rider is pushed.
#[test]
fn a_push_ignoring_the_lift_underfoot_is_not_a_block() {
    // The pusher: the same 256-wide slab, raised to `z` 4..20 and with its
    // `-x` face starting just clear of the rider at `x = 20`.
    let (mut model, lift, pusher) =
        build_two_platform_room(Vec3::new(SLAB_HALF_EXTENT + 20.0, 0.0, 20.0));
    // Half a unit inside the lift's top face.
    let origin = Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z - 0.5);
    let state = PlayerState::at(origin);
    assert!(model.trace(state.hull(), origin, origin).start_solid);
    assert!(
        !model
            .trace_ignoring(state.hull(), origin, origin, Some(lift))
            .start_solid
    );
    // The pusher slides 8 units toward `-x`, its face now 4 units into
    // the rider.
    let displacement = Vec3::new(-8.0, 0.0, 0.0);
    model.set_brush_origin(pusher, Vec3::new(SLAB_HALF_EXTENT + 12.0, 0.0, 20.0));
    assert!(
        model
            .trace_brush(state.hull(), origin, origin, pusher)
            .start_solid
    );

    let mut judged_whole = state;
    assert!(
        !ohl_physics::push_from_mover(&model, &mut judged_whole, displacement),
        "with the lift in the test, the push reads as a block"
    );
    let mut judged_without_lift = state;
    assert!(
        ohl_physics::push_from_mover_ignoring(
            &model,
            &mut judged_without_lift,
            displacement,
            Some(lift)
        ),
        "with the lift left out, the destination is clear"
    );
    assert_eq!(judged_without_lift.origin, origin + displacement);
}

/// `trace_brush` answers for the one brush it is asked about, whatever
/// else the hull is also inside: a rider half a unit into the lift and
/// nowhere near the second slab is inside the one and not the other.
#[test]
fn trace_brush_answers_for_one_brush_alone() {
    let (model, lift, far) = build_two_platform_room(Vec3::new(1000.0, 0.0, 0.0));
    let origin = Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z - 0.5);
    let hull = PlayerState::at(origin).hull();
    assert!(model.trace_brush(hull, origin, origin, lift).start_solid);
    assert!(!model.trace_brush(hull, origin, origin, far).start_solid);
    let clear = Vec3::new(0.0, 0.0, RIDER_ORIGIN_Z + 1.0);
    assert!(!model.trace_brush(hull, clear, clear, lift).start_solid);
}
