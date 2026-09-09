//! A player standing on a *rotating* brush entity must be carried around
//! with it, must not sink into or fall through it, and must never be left
//! inside its solid.
//!
//! `mover_riders.rs` next door covers the translating case (a `func_train`
//! sliding along a path): one velocity describes that whole brush's motion,
//! and `Level::brush_velocity` alone carries the rider. A rotating brush
//! (`func_rotating`, a swinging `func_door_rotating`) moves the point under
//! the player's feet at `omega x r` instead — nothing at all on its axis,
//! fastest at its rim — which `Level::brush_rotation` records and
//! `Level::brush_ride_velocity` turns into the per-point ride the
//! player-move phase feeds in as `PlayerController::base_velocity`.
//!
//! The fixture (`ohl_engine::test_support::rotating_platform_bsp`) is a
//! turntable in an otherwise empty void, so a rider who is not carried, or
//! who falls through the disc, has nothing else to land on.
//!
//! See `crates/ohl-physics/tests/rotating_riders.rs` for the same mechanism
//! tested directly against `player_move_events` and for the tangential
//! velocity formula's own properties. No bytes here come from any game
//! installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_PLATFORM_MAP, ROTATING_PLATFORM_NAME, ROTATING_PLATFORM_SPAWN_RADIUS,
    ROTATING_PLATFORM_SPEED, rotating_platform_bsp, rotating_platform_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::Rotator;

const STEP: f32 = 1.0 / 100.0;

/// The eye height of a standing player resting on the turntable's top
/// surface at `z = 0`: the physics origin (36) plus the standing view
/// height (28).
const RIDER_EYE_Z: f32 = 64.0;

fn game() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ROTATING_PLATFORM_MAP}.bsp"),
        rotating_platform_bsp(&rotating_platform_entities()),
    );
    Game::load(&assets as &dyn AssetSource, ROTATING_PLATFORM_MAP).expect("the fixture loads")
}

/// Ticks `seconds` of idle input, asserting on every step that the player
/// never ends up inside solid geometry — the same guard every combat-smoke
/// scenario asserts absent through `Game::eye_is_in_solid`.
fn ride(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for step in 0..steps {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "step {step}: the rider ended up inside the turntable at {:?}",
            game.eye_position()
        );
    }
}

#[test]
fn a_func_rotating_carries_a_standing_player_around_its_axis() {
    let mut game = game();
    let entity = *game
        .registry()
        .find(ROTATING_PLATFORM_NAME)
        .first()
        .expect("the fixture declares one named turntable");
    assert!(
        game.registry()
            .world
            .get::<&Rotator>(entity)
            .expect("the named entity is a func_rotating")
            .spinning,
        "the fixture's Start On spawnflag should leave it already spinning"
    );

    // Settle onto the disc first, so the first ride tick is a ride and not
    // a fall.
    ride(&mut game, 0.2);
    let start = game.eye_position();
    assert!(
        (start[2] - RIDER_EYE_Z).abs() < 4.0,
        "the player did not settle on the turntable: {start:?}"
    );

    // A quarter turn: at the fixture's own speed that takes two seconds,
    // and sweeps a rider at the spawn radius through a quarter circle.
    let seconds = 90.0 / ROTATING_PLATFORM_SPEED;
    ride(&mut game, seconds);
    let end = game.eye_position();

    // Carried: the rider has moved a good part of the arc their radius
    // implies, and specifically has been swept in `+y` (the disc turns the
    // documented default `+Z` way with no "Reverse Direction" spawnflag).
    let arc = std::f32::consts::FRAC_PI_2 * ROTATING_PLATFORM_SPAWN_RADIUS;
    let travelled = ((end[0] - start[0]).powi(2) + (end[1] - start[1]).powi(2)).sqrt();
    assert!(
        travelled > arc * 0.5,
        "the player was not carried around the turntable: moved {travelled} of about {arc}, \
         from {start:?} to {end:?}"
    );
    assert!(
        end[1] > start[1] + 16.0,
        "the player was not carried the way the disc turns: {start:?} -> {end:?}"
    );

    // Not sunk into it, not lifted off it, and still standing on the disc
    // rather than falling through the void the fixture leaves everywhere
    // else.
    assert!(
        (end[2] - RIDER_EYE_Z).abs() < 4.0,
        "the player sank into or fell off the turntable: {end:?}"
    );
    assert!(
        game.ground_mover_speed() > 1.0,
        "a player standing on a spinning turntable should read as riding a mover"
    );
}

#[test]
fn stopping_the_turntable_stops_the_ride() {
    let mut game = game();
    ride(&mut game, 0.2);
    {
        let registry = game.registry();
        let entity = *registry
            .find(ROTATING_PLATFORM_NAME)
            .first()
            .expect("the fixture declares one named turntable");
        registry
            .world
            .get::<&mut Rotator>(entity)
            .expect("the named entity is a func_rotating")
            .spinning = false;
    }
    // One tick for the stopped pose to be synced, then measure.
    ride(&mut game, 0.05);
    let start = game.eye_position();
    ride(&mut game, 1.0);
    let end = game.eye_position();

    let travelled = ((end[0] - start[0]).powi(2) + (end[1] - start[1]).powi(2)).sqrt();
    assert!(
        travelled < 1.0,
        "a stopped turntable still carried the player: {start:?} -> {end:?}"
    );
    assert!(
        game.ground_mover_speed() <= 1.0,
        "a stopped turntable should not read as a mover being ridden"
    );
}

/// `Game::ground_mover_speed`'s reading of a rider riding a rotating brush
/// matches what the physics step actually carried them by: the rider's
/// own raw per-tick displacement (their eye position's, since they stand
/// fixed on the disc with no input of their own) divided by `dt`.
///
/// This pins the specific formula `ground_mover_speed` uses — the chord
/// `Level::rotational_carry` moves the rider's own point through this
/// tick, not the instantaneous tangential rate `omega x r` a spin's
/// angle-per-tick would suggest — against the ride's own observed effect,
/// rather than against another re-derivation of the same rotation state
/// that would agree with a wrong formula just as readily as a right one.
/// Restoring the old `brush_ride_velocity`-only body (translation plus the
/// instantaneous tangential rate) still passes every other test in this
/// file, since a `func_rotating`'s own per-tick angle is small enough that
/// the two formulas agree to a fraction of a unit/second here; this test
/// exists specifically to keep that agreement pinned rather than assumed.
#[test]
fn ground_mover_speed_matches_the_riders_own_observed_per_tick_displacement() {
    let mut game = game();
    ride(&mut game, 0.2);

    let mut previous = game.eye_position();
    let mut worst_diff: f32 = 0.0;
    for _ in 0..100 {
        game.tick(STEP, &Input::default());
        let here = game.eye_position();
        let dx = here[0] - previous[0];
        let dy = here[1] - previous[1];
        let dz = here[2] - previous[2];
        let observed = (dx * dx + dy * dy + dz * dz).sqrt() / STEP;
        let reported = game.ground_mover_speed();
        worst_diff = worst_diff.max((observed - reported).abs());
        previous = here;
    }
    assert!(
        worst_diff < 1.0,
        "ground_mover_speed should track the rider's own observed per-tick \
         displacement over dt to within a rounding error, not diverge from it; \
         worst difference seen was {worst_diff} units/second"
    );
}

/// The same reading, but with the turntable's spin cranked up so a single
/// tick turns it through a large angle — the same size of single-tick
/// heading change a `func_tracktrain` corner used to produce before yaw
/// blending (`crates/ohl-game/src/track_train.rs`'s `DEFAULT_YAW_BLEND_DISTANCE`),
/// which is exactly the case that makes the old, naive `omega x r`
/// tangential-rate formula overstate how far a rider was actually carried:
/// the chord of a 90-degree arc is only about 90% of the arc length, and
/// the gap widens as the angle grows. `ground_mover_speed_matches_the_riders_own_observed_per_tick_displacement`
/// above cannot tell the two formulas apart, since an ordinary turntable's
/// per-tick angle is small enough that they agree; this test forces a
/// large one so a regression back to the arc-rate formula is caught.
#[test]
fn ground_mover_speed_matches_the_chord_even_for_a_large_single_tick_turn() {
    let mut game = game();
    ride(&mut game, 0.2);

    // 9000 degrees/second at this file's 0.01-second tick is 90 degrees
    // per tick — comfortably under the 180-degree ambiguity limit of the
    // shortest-arc delta `Level`'s own angular-velocity bookkeeping takes,
    // but a large enough single-tick turn that the arc and the chord
    // clearly disagree.
    {
        let registry = game.registry();
        let entity = *registry
            .find(ROTATING_PLATFORM_NAME)
            .first()
            .expect("the fixture declares one named turntable");
        registry
            .world
            .get::<&mut Rotator>(entity)
            .expect("the named entity is a func_rotating")
            .speed = 9000.0;
    }

    // `Systems::player_move`'s own `Level::sync_brush_collision` call
    // always poses a mover where *last* step's map logic left it (see
    // that call's doc comment in `crates/ohl-engine/src/systems.rs`), so
    // the speed change above only reaches the pose this test measures
    // against one tick from now: this tick's map logic (phase 12) is what
    // actually turns the disc through the new, larger angle, which the
    // *next* tick's `sync_brush_collision` then sees against the smaller
    // angle recorded before the change. One settling tick lets that catch
    // up before the measured tick below.
    game.tick(STEP, &Input::default());

    let before = game.eye_position();
    game.tick(STEP, &Input::default());
    let after = game.eye_position();
    let dx = after[0] - before[0];
    let dy = after[1] - before[1];
    let dz = after[2] - before[2];
    let observed = (dx * dx + dy * dy + dz * dz).sqrt() / STEP;
    let reported = game.ground_mover_speed();

    // Sanity on the test itself: the naive arc-rate estimate for a rider
    // at this fixture's spawn radius really is meaningfully larger than
    // what the rider was actually carried by this tick, so this test can
    // actually tell the two formulas apart.
    let arc_rate = ROTATING_PLATFORM_SPAWN_RADIUS * 90f32.to_radians() / STEP;
    assert!(
        arc_rate > observed * 1.05,
        "this fixture's turn is not large enough to distinguish the chord from \
         the arc-rate estimate: arc_rate {arc_rate}, observed {observed}"
    );

    assert!(
        (reported - observed).abs() < observed * 0.05 + 1.0,
        "ground_mover_speed should match the rider's own observed chord for a \
         large single-tick turn, not the larger arc-rate estimate; \
         reported {reported}, observed {observed}, naive arc-rate would be {arc_rate}"
    );
}
