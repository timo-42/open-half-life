//! A player the map spawns *inside* a mover rides it from the mover's
//! first tick of motion.
//!
//! A campaign opening that starts the player aboard a `func_tracktrain`
//! authors its `info_player_start` against the original engine's own
//! compiled clip tree. This project's clip tree is not bit-identical to
//! it, so such a spawn point can land a few units inside the car's solid
//! here. That is not a cosmetic difference: an embedded hull has no
//! ground brush at all (`ohl_physics::categorize_position` refuses the
//! probe while `start_solid` holds), so the host's `base_velocity` lookup
//! finds nothing and the ride never starts — and no trace out of solid
//! succeeds either, so the passenger cannot even fall free. The car pulls
//! out from under someone who never moves at all.
//!
//! `ohl_physics::settle_at_spawn`, run once by `Game::from_level` against
//! the *posed* mover hulls, is what closes that: the same bounded upward
//! nudge a landing already uses, followed by an immediate
//! `categorize_position`, so the mover under a freshly spawned rider is
//! their ground brush on the very first step. See
//! `docs/FORMAT_SOURCES.md`, "Riding movers".
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    EMBEDDED_SPAWN_CAR_HALF_LENGTH, EMBEDDED_SPAWN_CAR_HALF_WIDTH, EMBEDDED_SPAWN_CAR_TOP_Z,
    EMBEDDED_SPAWN_MAP, EMBEDDED_SPAWN_SEAT_OFFSET_X, EMBEDDED_SPAWN_TRAIN_END,
    EMBEDDED_SPAWN_TRAIN_ORIGIN, EMBEDDED_SPAWN_TRAIN_SPEED, embedded_spawn_track_train_bsp,
    embedded_spawn_train_origin,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_physics::{Hull, Vec3};

const STEP: f32 = 1.0 / 60.0;

/// How far the seat may drift, in units, over the whole ride. A couple of
/// units of slack for the per-step order of "sync the hulls, then move the
/// rider by the velocity that sync measured", which always leaves the
/// passenger a fraction of one step's travel behind the car.
const SEAT_TOLERANCE: f32 = 4.0;

fn loaded() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{EMBEDDED_SPAWN_MAP}.bsp"),
        embedded_spawn_track_train_bsp(),
    );
    Game::load(&assets as &dyn AssetSource, EMBEDDED_SPAWN_MAP)
        .expect("the embedded-spawn train map loads")
}

/// How many steps the ride is sampled for: most of the single segment, at
/// the train's own constant speed.
fn ride_steps() -> u32 {
    let length = EMBEDDED_SPAWN_TRAIN_END[0] - EMBEDDED_SPAWN_TRAIN_ORIGIN[0];
    let seconds = 0.8 * length / EMBEDDED_SPAWN_TRAIN_SPEED;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    steps
}

/// The seat, in the car's own frame, on the very first step: the passenger
/// is standing on the car, not embedded in it and not in the void.
#[test]
fn a_player_spawned_inside_a_departing_car_is_standing_on_it_from_the_first_step() {
    let mut game = loaded();

    // Before any tick at all: the spawn settle has already run, so the
    // car is under the passenger's feet rather than around their waist.
    assert!(
        !game.eye_is_in_solid(),
        "the passenger was left embedded in the car at spawn: {:?}",
        game.player_origin()
    );
    let feet_above_floor = game.player_origin()[2]
        - (EMBEDDED_SPAWN_TRAIN_ORIGIN[2] + EMBEDDED_SPAWN_CAR_TOP_Z + 36.0);
    assert!(
        feet_above_floor.abs() < SEAT_TOLERANCE,
        "the passenger did not settle onto the car's floor: {feet_above_floor} units off"
    );

    // Two steps: the car's own logic phase runs *after* the move, so the
    // first step is the one that gives the car a measured velocity and the
    // second is the first that can spend it on a rider. The passenger must
    // be riding by then, which can only happen if the car was already
    // their ground brush before either move ran.
    game.tick(STEP, &Input::default());
    game.tick(STEP, &Input::default());
    assert!(
        game.ground_mover_speed() > EMBEDDED_SPAWN_TRAIN_SPEED * 0.5,
        "the passenger was not riding the car on the first step: ride speed {}",
        game.ground_mover_speed()
    );
    let seat = game.player_origin()[0] - embedded_spawn_train_origin(&game)[0];
    assert!(
        (seat - EMBEDDED_SPAWN_SEAT_OFFSET_X).abs() < SEAT_TOLERANCE,
        "the passenger's seat slid on the first steps: {seat} (spawned at {EMBEDDED_SPAWN_SEAT_OFFSET_X})"
    );
}

/// The seat stays put for the whole departure, rather than sliding the
/// length of the car while the passenger stands still in world space.
#[test]
fn a_spawned_in_passengers_seat_stays_put_for_the_whole_departure() {
    let mut game = loaded();

    for step in 0..ride_steps() {
        game.tick(STEP, &Input::default());
        let car = embedded_spawn_train_origin(&game);
        let here = game.player_origin();
        let seat_x = here[0] - car[0];
        assert!(
            (seat_x - EMBEDDED_SPAWN_SEAT_OFFSET_X).abs() < SEAT_TOLERANCE,
            "the passenger's seat slid to {seat_x} on step {step} (spawned at {EMBEDDED_SPAWN_SEAT_OFFSET_X})"
        );
        assert!(
            seat_x.abs() < EMBEDDED_SPAWN_CAR_HALF_LENGTH
                && (here[1] - car[1]).abs() < EMBEDDED_SPAWN_CAR_HALF_WIDTH,
            "the passenger was left off the car's own footprint on step {step}: {here:?}"
        );
        assert!(
            step < 2 || game.ground_mover_speed() > EMBEDDED_SPAWN_TRAIN_SPEED * 0.5,
            "the passenger stopped riding the car on step {step}"
        );
    }
}

/// The map name the no-spawn fixture is published under.
const NO_SPAWN_MAP: &str = "ohlnospawnsynth";

/// A map with real collision but **no** `info_player_start`, whose one
/// solid brush entity swallows the world origin — the exact place
/// `PlayerController::default` leaves a player when a map names no spawn
/// point.
///
/// Nothing here comes from any game installation; every keyvalue and
/// coordinate is authored for this project (`docs/CLEAN_ROOM.md`).
fn no_player_start_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n\
         {\n\"classname\" \"func_wall\"\n\"model\" \"*1\"\n\
         \"origin\" \"0 0 0\"\n}\n",
    );
    // Submodel 0: a void world, so the block below is the only solid.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: a block whose top is 8 units *below* the world origin.
    // The default controller's origin therefore starts inside the block's
    // hull-expanded solid, and a clear standing height is 28 units up —
    // comfortably inside the nudge's own bound, so a settle that ran here
    // would visibly lift the player rather than give up.
    let block_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [-64.0, -64.0, -64.0],
        [64.0, 64.0, -8.0],
    )]);
    b.push_model(
        [-64.0, -64.0, -64.0],
        [64.0, 64.0, -8.0],
        [0.0, 0.0, 0.0],
        block_heads,
        2,
        0,
        0,
    );
    b.build()
}

/// The settle is an `info_player_start` placement's own step, and a map
/// that names no spawn point has no such placement: the player is left at
/// `PlayerController::default`'s world origin, which no map authored, so
/// nothing may nudge it — not even out of the solid it happens to sit in.
///
/// Gated on `Level::spawn` rather than on the map merely having collision,
/// which is what makes this test discriminate: the fixture's own brush
/// swallows the world origin, so a settle that ran here would move the
/// player tens of units up and out of it.
#[test]
fn a_map_with_no_player_start_is_left_exactly_where_the_default_controller_puts_it() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{NO_SPAWN_MAP}.bsp"), no_player_start_bsp());
    let game =
        Game::load(&assets as &dyn AssetSource, NO_SPAWN_MAP).expect("the no-spawn map loads");

    assert!(!game.has_player_start(), "the fixture declares no spawn");
    assert!(game.has_collision(), "the fixture does have collision");
    // The discriminating half: this placement is one a settle *would*
    // have moved — the standing hull starts inside the block's own
    // hull-expanded solid, with a clear standing height well inside the
    // nudge's bound. (A *point* at the world origin is above the block;
    // it is the expanded standing hull that overlaps it, which is exactly
    // the test `settle_at_spawn` itself makes.)
    let collision = game.collision().expect("the fixture has collision");
    assert!(
        collision
            .trace(Hull::Standing, Vec3::ZERO, Vec3::ZERO)
            .start_solid,
        "the fixture must put the default placement somewhere a settle would move it"
    );
    let origin = game.player_origin();
    assert!(
        origin.iter().all(|component| *component == 0.0),
        "a map with no spawn point had its default placement settled: {origin:?}"
    );
}
