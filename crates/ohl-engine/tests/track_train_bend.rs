//! A passenger keeps their seat when the car they are riding turns.
//!
//! `Level::sync_brush_collision` now poses a `func_tracktrain`'s collision
//! hull at the same yaw the renderer draws it at (`ohl_game::pose::
//! brush_pose_rotation`), so at a corner the car's whole body — and the
//! patch of floor a passenger is standing on — swings about its origin
//! brush. This package's other half is what carries the passenger with it:
//! the turn is applied to a rider as one rigid step about the same pivot
//! (`Level::rotational_carry`, `ohl_physics::rotational_ride_step`) rather
//! than as a tangential velocity integrated through the ordinary move,
//! because a train changes heading by the whole angle between two path
//! segments in the single step it changes segment on.
//!
//! See `docs/FORMAT_SOURCES.md`, "Riding movers", and
//! `crates/ohl-physics/tests/rotating_riders.rs` for the same behaviour
//! tested directly against the physics crate with no engine involved.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    BEND_CAR_HALF_LENGTH, BEND_CAR_HALF_WIDTH, BEND_CAR_TOP_Z, BEND_SEAT_OFFSET_X,
    BEND_TRAIN_CORNER, BEND_TRAIN_MAP, BEND_TRAIN_ORIGIN, BEND_TRAIN_SPEED,
    bending_track_train_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

fn loaded() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{BEND_TRAIN_MAP}.bsp"),
        bending_track_train_bsp(),
    );
    Game::load(&assets as &dyn AssetSource, BEND_TRAIN_MAP).expect("the bending-train map loads")
}

/// How long the ride is sampled for: long enough to cross the corner
/// (the first segment is `corner - origin` units at [`BEND_TRAIN_SPEED`]
/// units per second) and travel well up the second segment, but stopping
/// short of the chain's last node — a parked train has no segment left to
/// face and so no defined heading, which is a separate case from the
/// turning one under test here.
fn ride_steps() -> u32 {
    let first = BEND_TRAIN_CORNER[0] - BEND_TRAIN_ORIGIN[0];
    let seconds = 2.0 * first / BEND_TRAIN_SPEED - 0.5;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    steps
}

/// The passenger rides the corner: they are never left inside solid, never
/// dropped into the void the fixture leaves under the car, and end up
/// standing where the *turned* car's seat is — swung a quarter turn about
/// the car's origin brush — rather than where their old world offset
/// would have left them.
#[test]
fn a_passenger_seated_off_the_pivot_keeps_their_seat_through_a_corner() {
    let mut game = loaded();

    // Settle onto the car before it has covered any ground.
    for _ in 0..4 {
        game.tick(STEP, &Input::default());
    }
    let start = game.player_origin();
    let seat_z = start[2];

    for step in 0..ride_steps() {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "the passenger was pushed inside solid geometry on step {step}: {:?}",
            game.player_origin()
        );
        let here = game.player_origin();
        assert!(
            (here[2] - seat_z).abs() < 24.0,
            "the passenger left the car's floor on step {step}: {here:?} (seated at {seat_z})"
        );
    }

    // After the corner the car faces `+Y`, so the seat that was
    // `BEND_SEAT_OFFSET_X` units along `+X` from the origin brush is now
    // that far along `+Y` from it. The train has also travelled up the
    // second segment by then, so only the *across-track* coordinate is
    // pinned here: the passenger must be within the car's own narrow
    // width of the track's own `x`, which the un-turned world offset
    // (`corner_x + BEND_SEAT_OFFSET_X`) is far outside.
    let end = game.player_origin();
    assert!(
        (end[0] - BEND_TRAIN_CORNER[0]).abs() < BEND_CAR_HALF_WIDTH,
        "the passenger was left beside the turned car rather than on it: {end:?}"
    );
    assert!(
        end[1] > BEND_TRAIN_ORIGIN[1] + BEND_SEAT_OFFSET_X,
        "the passenger was not carried up the second segment: {end:?}"
    );
}

/// The same ride, checked against the car's own live footprint every step
/// rather than only at the end: at no point is the passenger standing
/// somewhere the car is not.
///
/// The pose is read *before* each tick, because a tick's player-move phase
/// syncs the collision hulls to what the previous tick's map logic left
/// behind and only then advances the simulation again
/// (`Systems::player_move`'s own "moved by last step's map logic"
/// comment). So the pose the passenger was actually standing on during a
/// tick is the one the registry held going into it, not the one it holds
/// coming out.
#[test]
fn the_passenger_is_inside_the_cars_footprint_at_every_step_of_the_bend() {
    let mut game = loaded();
    for _ in 0..4 {
        game.tick(STEP, &Input::default());
    }

    for step in 0..ride_steps() {
        let train = ohl_engine::test_support::bend_train_pose(&game);
        game.tick(STEP, &Input::default());
        let here = game.player_origin();
        // Into the car's own frame: undo the translation, then the yaw.
        let dx = here[0] - train.origin[0];
        let dy = here[1] - train.origin[1];
        let (sin, cos) = (-train.yaw_degrees.to_radians()).sin_cos();
        let local_x = dx * cos - dy * sin;
        let local_y = dx * sin + dy * cos;
        assert!(
            local_x.abs() <= BEND_CAR_HALF_LENGTH && local_y.abs() <= BEND_CAR_HALF_WIDTH,
            "on step {step} the passenger stood at car-local ({local_x}, {local_y}), \
             outside the car's own {BEND_CAR_HALF_LENGTH} x {BEND_CAR_HALF_WIDTH} floor"
        );
        assert!(
            here[2] > train.origin[2] + BEND_CAR_TOP_Z - 8.0,
            "on step {step} the passenger sank through the car's floor: {here:?}"
        );
    }
}
