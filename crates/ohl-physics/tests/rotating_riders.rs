//! Riding a *rotating* attached brush entity.
//!
//! `mover_riders.rs` next door covers a brush that translates: every point
//! of a `func_train`/`func_plat` moves alike, so one velocity describes the
//! whole ride. A rotating brush (`func_rotating`, a swinging
//! `func_door_rotating`) does not — the point under the player's feet moves
//! at `omega x r`, growing with their distance from the axis and vanishing
//! on it — which is what [`ohl_physics::rotational_ride_velocity`] computes
//! and what these tests pin down: the formula's own algebraic properties,
//! and the behaviour that matters in a map, that a player standing on a
//! slowly turning disc is carried by it, stays on it, and is never left
//! inside its solid.
//!
//! The disc here is `build_platform_room`'s slab (256 x 256 units, top
//! surface at `z = 0`) posed with `CollisionModel::set_brush_pose` about
//! its own centre each tick — the same call `ohl-engine`'s
//! `Level::sync_brush_collision` makes for a real rotating mover, driven
//! here directly so no engine is needed. A box rotated about `Z` keeps its
//! top face in the plane `z = 0`, so this isolates the *carry* from any
//! vertical sweeping.
//!
//! No bytes from any game installation are read or embedded; see
//! `docs/CLEAN_ROOM.md`.

use ohl_physics::controller::TICK_SECONDS;
use ohl_physics::test_support::build_platform_room;
use ohl_physics::{
    MoveConfig, MoveInput, PlayerState, Vec3, player_move_events, rotational_ride_velocity,
};
use proptest::prelude::*;

const TICK: f32 = TICK_SECONDS;

/// The origin height of a standing player resting on the disc's top
/// surface at `z = 0` (`ohl_formats::test_support::BRUSH_FLOOR_TOP_Z`).
const RIDER_ORIGIN_Z: f32 = 36.0;

/// How many ticks the ride tests run for ([`RIDE_SECONDS`] at this crate's
/// own 100 Hz movement tick): long enough for a slow disc to carry a rider
/// a visible distance and for any per-tick sinking, drift into solid, or
/// lost ground contact to accumulate into a failure rather than hide
/// inside one step's epsilon.
const RIDE_TICKS: u32 = 120;

/// [`RIDE_TICKS`] expressed in seconds (`120 * 0.01`), for the arc the
/// ride is expected to cover.
const RIDE_SECONDS: f32 = 1.2;

fn any_finite() -> impl Strategy<Value = f32> {
    -4096.0f32..4096.0
}

fn any_vec3() -> impl Strategy<Value = Vec3> {
    (any_finite(), any_finite(), any_finite()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

/// An angular velocity in radians per second, bounded well above anything a
/// map's own `speed` keyvalue produces (32 rad/s is over 1,800 degrees per
/// second) while staying in a range where the float bounds below stay
/// meaningful.
fn any_angular_velocity() -> impl Strategy<Value = Vec3> {
    let rate = -32.0f32..32.0;
    (rate.clone(), rate.clone(), rate).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

/// The absolute error `omega x (point - pivot)` can carry purely from
/// evaluating `point - pivot` in `f32` at world coordinates: the
/// subtraction's own rounding is proportional to the magnitudes involved,
/// and the cross product then scales it by the rate. Used where the exact
/// answer is zero, so no relative bound is available.
fn cancellation_tolerance(pivot: Vec3, omega: Vec3, along: f32) -> f32 {
    32.0 * f32::EPSILON * omega.length() * (pivot.length() + along.abs()) + 1e-3
}

/// `v = omega x r` is what a point on a rotating rigid body moves at, so the
/// reported velocity must be perpendicular to both the axis and the radius,
/// must have magnitude `|omega|` times the distance from the axis, and must
/// vanish exactly on the axis.
///
/// The proptest body itself, extracted so
/// [`the_tangential_velocity_is_the_rigid_body_relation`] can run it over
/// generated cases and a fixed regression test (below) can replay a single
/// one directly — see that module's doc comment for why fixed cases exist
/// at all instead of a `.proptest-regressions` file.
fn tangential_velocity_is_the_rigid_body_relation(
    pivot: Vec3,
    omega: Vec3,
    point: Vec3,
) -> Result<(), TestCaseError> {
    let velocity = rotational_ride_velocity(pivot, omega, point);
    prop_assert!(velocity.is_finite());

    let radius = point - pivot;
    let scale = (omega.length() * radius.length()).max(1.0);
    prop_assert!(
        velocity.dot(omega).abs() <= 1e-3 * scale * omega.length().max(1.0),
        "velocity {velocity:?} is not perpendicular to omega {omega:?}"
    );
    prop_assert!(
        velocity.dot(radius).abs() <= 1e-3 * scale * radius.length().max(1.0),
        "velocity {velocity:?} is not perpendicular to the radius {radius:?}"
    );

    // The distance from the axis is the radius' component perpendicular to
    // omega; the speed is that times the rate.
    let axis = omega.normalize_or_zero();
    let perpendicular = radius - axis * radius.dot(axis);
    let expected = omega.length() * perpendicular.length();
    prop_assert!(
        (velocity.length() - expected).abs() <= 1e-2 * scale,
        "speed {} is not |omega| * perpendicular radius {expected}",
        velocity.length()
    );
    Ok(())
}

/// A point *on* the axis of rotation does not move, however fast the body
/// spins, and reversing the spin reverses every point's velocity.
///
/// Extracted for the same reason as
/// [`tangential_velocity_is_the_rigid_body_relation`]: reused by
/// [`a_point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride`]
/// and by the fixed regression tests below.
fn point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride(
    pivot: Vec3,
    omega: Vec3,
    along: f32,
    point: Vec3,
) -> Result<(), TestCaseError> {
    let on_axis = pivot + omega.normalize_or_zero() * along;
    let velocity = rotational_ride_velocity(pivot, omega, on_axis);
    let tolerance = cancellation_tolerance(pivot, omega, along);
    prop_assert!(
        velocity.length() <= tolerance,
        "a point on the axis moved at {velocity:?}"
    );

    let forward = rotational_ride_velocity(pivot, omega, point);
    let backward = rotational_ride_velocity(pivot, -omega, point);
    prop_assert!((forward + backward).length() <= 1e-3 * forward.length().max(1.0));
    Ok(())
}

/// A player standing on a slowly rotating disc stays on it: for every tick
/// of [`RIDE_TICKS`] they keep reporting that brush as their ground, are
/// never embedded in its solid, never sink below its surface, and are
/// carried around rather than left behind on the spot.
///
/// Extracted for the same reason as
/// [`tangential_velocity_is_the_rigid_body_relation`]: reused by
/// [`a_hull_resting_on_a_slowly_rotating_disc_stays_on_it`] and by the fixed
/// regression tests below.
fn hull_resting_on_a_slowly_rotating_disc_stays_on_it(
    degrees_per_second: f32,
    clockwise: bool,
    radius: f32,
) -> Result<(), TestCaseError> {
    let (mut model, brush) = build_platform_room();
    let axis = if clockwise { -Vec3::Z } else { Vec3::Z };
    let angular_velocity = axis * degrees_per_second.to_radians();
    let config = MoveConfig::default();

    let start = Vec3::new(radius, 0.0, RIDER_ORIGIN_Z);
    let mut state = PlayerState::at(start);
    let mut angle = 0.0f32;

    for tick in 0..RIDE_TICKS {
        // The engine's own order: the brush is posed first, then the
        // player moves with that brush's ride fed in as base velocity.
        angle += degrees_per_second * TICK;
        model.set_brush_pose(brush, Vec3::ZERO, Vec3::ZERO, axis, angle);

        let base_velocity = if state.ground_brush == Some(brush) {
            rotational_ride_velocity(Vec3::ZERO, angular_velocity, state.origin)
        } else {
            Vec3::ZERO
        };
        let input = MoveInput {
            base_velocity,
            ..MoveInput::default()
        };
        player_move_events(&model, &mut state, &input, &config, TICK);

        prop_assert!(
            state.origin.is_finite(),
            "tick {tick}: origin {:?}",
            state.origin
        );
        // "Inside solid geometry" in the sense the engine's own guard means
        // it (`ohl_engine::Game::eye_is_in_solid`, the line every smoke
        // scenario asserts absent): a hull-0 point query at the player's
        // eye. A *zero-length hull* trace is deliberately not used here — a
        // player resting exactly on a surface sits exactly on that hull's
        // expanded plane, where "just inside" and "just outside" differ by
        // a rounding step, which is the ambiguity `movement.rs`'s own
        // ground probe already handles.
        let eye = state.origin + Vec3::Z * config.view_height_standing;
        prop_assert!(
            !ohl_physics::contents::is_solid(model.point_contents(eye)),
            "tick {tick}: the rider's eye ended up inside the disc at {:?}",
            state.origin
        );
        prop_assert!(
            state.on_ground && state.ground_brush == Some(brush),
            "tick {tick}: the rider left the disc (on_ground {}, ground {:?})",
            state.on_ground,
            state.ground_brush
        );
        prop_assert!(
            (state.origin.z - RIDER_ORIGIN_Z).abs() <= 1.0,
            "tick {tick}: the rider sank or rose to z = {}",
            state.origin.z
        );
    }

    // Carried, not left standing on the spot: the ride's own seconds sweeps
    // the rider at least the arc its own angular rate implies, less a
    // generous allowance for the chord-versus-arc difference and for
    // ground friction acting on the blended velocity.
    let travelled = (state.origin - start).truncate().length();
    let arc = (degrees_per_second * RIDE_SECONDS).to_radians() * radius;
    prop_assert!(
        travelled >= arc * 0.5,
        "the rider travelled {travelled} units, far less than the {arc}-unit arc of the disc"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_tangential_velocity_is_the_rigid_body_relation(
        pivot in any_vec3(),
        omega in any_angular_velocity(),
        point in any_vec3(),
    ) {
        tangential_velocity_is_the_rigid_body_relation(pivot, omega, point)?;
    }

    #[test]
    fn a_point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride(
        pivot in any_vec3(),
        omega in any_angular_velocity(),
        along in any_finite(),
        point in any_vec3(),
    ) {
        point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride(
            pivot, omega, along, point,
        )?;
    }

    #[test]
    fn a_hull_resting_on_a_slowly_rotating_disc_stays_on_it(
        degrees_per_second in 5.0f32..45.0,
        clockwise in any::<bool>(),
        radius in 24.0f32..96.0,
    ) {
        hull_resting_on_a_slowly_rotating_disc_stays_on_it(degrees_per_second, clockwise, radius)?;
    }
}

/// Fixed regression cases for four failures `proptest` found and shrunk
/// during PR #110's development (see that PR's now-deleted
/// `rotating_riders.proptest-regressions`).
///
/// That file's persistence mechanism itself was not broken: `proptest`'s
/// default `FileFailurePersistence::SourceParallel` does print "failed to
/// find lib.rs or main.rs" to stderr for an integration test under
/// `tests/` (there is no `lib.rs`/`main.rs` for it to locate), but it then
/// falls back to resolving the file sitting next to the test's own source
/// — exactly this file's `.proptest-regressions` sibling — and both writes
/// and reads it there. The stderr line is harmless noise, not evidence the
/// seeds went unread.
///
/// The real problem is staleness. A persisted seed replays the *RNG seed*
/// that produced a shrunk failure, not the shrunk values themselves: three
/// of these four seeds shrank under this file's old `any_angular_velocity`
/// strategy, which allowed `|omega|` up to roughly 1600-2700 (for example
/// `Vec3(-1810.9149, -2027.4081, 0.0)`), and `135a9e7` later narrowed that
/// strategy to +/-32 rad/s per axis. Replaying the old seeds under the
/// narrower strategy no longer regenerates anything near the inputs that
/// originally failed, so the checked-in file silently stopped covering the
/// cases it was added for. Pinning the four shrunk cases down as ordinary
/// tests that always run, calling the same property bodies directly with
/// their literal values, keeps covering them across any future strategy
/// change rather than depending on a seed to still reproduce them.
#[test]
fn regression_axis_case_pivot_y_and_along_positive() {
    // shrinks to pivot = Vec3(0.0, -2270.4446, 0.0),
    // omega = Vec3(0.0, -633.6775, -1512.0397), along = 7.8597956,
    // point = Vec3(0.0, 0.0, 0.0)
    point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride(
        Vec3::new(0.0, -2270.4446, 0.0),
        Vec3::new(0.0, -633.6775, -1512.0397),
        7.859_795_6,
        Vec3::new(0.0, 0.0, 0.0),
    )
    .unwrap();
}

#[test]
fn regression_axis_case_pivot_x_and_z() {
    // shrinks to pivot = Vec3(2807.683, 0.0, 2975.182),
    // omega = Vec3(-1810.9149, -2027.4081, 0.0), along = 0.7551345,
    // point = Vec3(0.0, 0.0, 0.0)
    point_on_the_axis_never_moves_and_reversing_the_spin_reverses_the_ride(
        Vec3::new(2807.683, 0.0, 2975.182),
        Vec3::new(-1810.9149, -2027.4081, 0.0),
        0.755_134_5,
        Vec3::new(0.0, 0.0, 0.0),
    )
    .unwrap();
}

#[test]
fn regression_disc_ride_case_slow_rate() {
    // shrinks to degrees_per_second = 5.0, clockwise = false, radius = 24.0
    hull_resting_on_a_slowly_rotating_disc_stays_on_it(5.0, false, 24.0).unwrap();
}

#[test]
fn regression_disc_ride_case_faster_rate() {
    // shrinks to degrees_per_second = 17.39962, clockwise = false, radius = 24.0
    hull_resting_on_a_slowly_rotating_disc_stays_on_it(17.39962, false, 24.0).unwrap();
}
