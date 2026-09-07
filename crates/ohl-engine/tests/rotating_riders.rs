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
