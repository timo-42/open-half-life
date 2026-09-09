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
    BEND_CAR_HALF_LENGTH, BEND_CAR_HALF_WIDTH, BEND_CAR_TOP_Z, BEND_PILLAR_CENTER,
    BEND_SEAT_OFFSET_X, BEND_TRAIN_CORNER, BEND_TRAIN_END, BEND_TRAIN_MAP, BEND_TRAIN_ORIGIN,
    BEND_TRAIN_SPEED, bend_train_pose, bending_track_train_bsp,
    bending_track_train_bsp_with_pillar,
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

/// [`loaded`], but with a static pillar standing across the seat a rider
/// would land in if the corner's quarter-turn carry were applied without
/// its refusal guard (see [`BEND_PILLAR_CENTER`]).
fn loaded_with_pillar() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{BEND_TRAIN_MAP}.bsp"),
        bending_track_train_bsp_with_pillar(),
    );
    Game::load(&assets as &dyn AssetSource, BEND_TRAIN_MAP).expect("the bending-train map loads")
}

/// How long the ride is sampled for: long enough to cross the corner
/// (the first segment is `corner - origin` units at [`BEND_TRAIN_SPEED`]
/// units per second) and travel well up the second segment, but stopping
/// short of the chain's last node — a parked train keeps the heading of
/// the segment it arrived on (see `a_passenger_is_not_snapped_over_when_
/// the_car_parks_at_the_end_of_the_bend` below), but that is still a
/// separate case from the turning one under test here.
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

/// A train that runs off the end of its chain and parks keeps facing the
/// way its last segment pointed, rather than snapping its collision hull
/// back to an unrotated pose the instant it has nowhere left to go.
///
/// [`TrackTrainState::yaw_degrees`] used to return `None` once
/// [`ohl_game::track_train::PathChain::next_index`] ran out of nodes, and
/// every consumer of that yaw — the renderer, `Level::sync_brush_collision`
/// (through `ohl_game::pose::brush_pose_rotation`) and `brush_center` —
/// treated a `None` exactly like an un-turned `func_train`, i.e. axis
/// `Vec3::ZERO`. So the frame this fixture's car finished its 90-degree
/// turn and parked at [`BEND_TRAIN_END`], its hull un-rotated back to 0
/// degrees in a single step while the passenger, still seated
/// [`BEND_SEAT_OFFSET_X`] units along the car's *turned* `+X` (now world
/// `+Y`), was left standing over open air the moment the floor under them
/// rotated away — exactly the drop this package's carry mechanism exists
/// to prevent for a *moving* turn, just triggered by parking instead.
///
/// This checks the whole ride through to well after the train parks: the
/// passenger is never in solid, never sinks through the floor, and is
/// always within the car's own footprint, both while it is still turning
/// and once it has stopped — and that the reported pose itself is the
/// second segment's heading (90 degrees, at [`BEND_TRAIN_END`]), not the
/// zeroed one a stale `None` would still produce.
#[test]
fn a_passenger_is_not_snapped_over_when_the_car_parks_at_the_end_of_the_bend() {
    let mut game = loaded();

    for _ in 0..4 {
        game.tick(STEP, &Input::default());
    }

    // The full chain (origin -> corner -> end) is 600 units at
    // `BEND_TRAIN_SPEED` = 100/sec, so 6 seconds covers it; ride well past
    // that so the train has certainly parked and settled before the final
    // assertions run.
    let total =
        BEND_TRAIN_CORNER[0] - BEND_TRAIN_ORIGIN[0] + BEND_TRAIN_END[1] - BEND_TRAIN_CORNER[1];
    let seconds = total / BEND_TRAIN_SPEED + 2.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;

    for step in 0..steps {
        let train = bend_train_pose(&game);
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "the passenger was pushed inside solid geometry on step {step}: {:?}",
            game.player_origin()
        );
        let here = game.player_origin();
        let dx = here[0] - train.origin[0];
        let dy = here[1] - train.origin[1];
        let (sin, cos) = (-train.yaw_degrees.to_radians()).sin_cos();
        let local_x = dx * cos - dy * sin;
        let local_y = dx * sin + dy * cos;
        assert!(
            local_x.abs() <= BEND_CAR_HALF_LENGTH && local_y.abs() <= BEND_CAR_HALF_WIDTH,
            "on step {step} the passenger stood at car-local ({local_x}, {local_y}), \
             outside the car's own {BEND_CAR_HALF_LENGTH} x {BEND_CAR_HALF_WIDTH} floor \
             (car pose {train:?})"
        );
        assert!(
            here[2] > train.origin[2] + BEND_CAR_TOP_Z - 8.0,
            "on step {step} the passenger sank through the car's floor: {here:?}"
        );
    }

    let final_pose = bend_train_pose(&game);
    assert!(
        (final_pose.origin[0] - BEND_TRAIN_END[0]).abs() < 1.0
            && (final_pose.origin[1] - BEND_TRAIN_END[1]).abs() < 1.0,
        "the parked car should sit at the chain's last node: {final_pose:?}"
    );
    assert!(
        (final_pose.yaw_degrees - 90.0).abs() < 1.0,
        "a car parked at the end of a bend must keep the last segment's \
         heading (90 degrees here), not snap back to unrotated: {final_pose:?}"
    );
}

/// `Systems::player_move`'s rigid-turn carry is refused outright when the
/// destination seat is not free (`.filter(|carried| !start_solid)`, right
/// after `Level::rotational_carry`): a rider seated off the pivot who
/// would otherwise be swung straight into solid geometry is instead left
/// exactly where they were standing, and the ordinary ride blend (the
/// car's translation plus its tangential `omega x r` term for the rider's
/// own position) takes over from there through the usual traced move.
///
/// [`bending_track_train_bsp_with_pillar`] plants a static pillar centred
/// on [`BEND_PILLAR_CENTER`] — precisely the world point the corner's
/// quarter turn would carry this fixture's seated rider into — so a
/// carry that is *not* refused embeds the rider in solid the instant it
/// runs, and one that *is* refused never does. This is the same fixture
/// [`a_passenger_seated_off_the_pivot_keeps_their_seat_through_a_corner`]
/// rides, with one brush added; every other tick of the corner is
/// identical, and this test rides well past it.
#[test]
fn a_refused_carry_leaves_the_rider_behind_instead_of_embedding_them() {
    let mut game = loaded_with_pillar();

    for _ in 0..4 {
        game.tick(STEP, &Input::default());
    }

    let mut in_solid_ticks = 0u32;
    // Past the corner (around tick 181 for this fixture's speed/geometry)
    // and well into the second segment, so the refused carry's aftermath
    // — the rider falling through this fixture's deliberately floorless
    // world once they are no longer on the car — has time to play out.
    for _step in 0..220 {
        if game.eye_is_in_solid() {
            in_solid_ticks += 1;
        }
        game.tick(STEP, &Input::default());
    }
    assert_eq!(
        in_solid_ticks, 0,
        "the refused carry must never leave the rider inside the pillar's solid"
    );

    let end = game.player_origin();
    let horizontal = ((end[0] - BEND_PILLAR_CENTER[0]).powi(2)
        + (end[1] - BEND_PILLAR_CENTER[1]).powi(2))
    .sqrt();
    assert!(
        horizontal > BEND_CAR_HALF_WIDTH,
        "the rider ended up at the pillar's seat destination {end:?} rather than \
         being left behind by the refused carry"
    );
}

/// `Game::ground_mover_speed`'s reading of a rider going round this
/// fixture's corner never balloons past a small multiple of the car's own
/// travel speed.
///
/// Before `TrackTrainState::yaw_degrees` blended a corner's heading change
/// over `ohl_game::track_train::DEFAULT_YAW_BLEND_DISTANCE` (see that
/// constant's doc comment), a `func_tracktrain` turned through the whole
/// angle between two path segments in the single tick it changed segment
/// on, and a rider seated `BEND_SEAT_OFFSET_X` units off the car's pivot
/// was carried through that turn's whole chord in one
/// `ohl_physics::controller::TICK_SECONDS` step — a real, if brief, spike
/// in how fast their seat actually moved, on the order of the chord
/// (`2 * BEND_SEAT_OFFSET_X * sin(45 degrees)`) divided by that one tick's
/// duration, tens of times the car's own 100 units/second. Blending the
/// heading change over a short distance spreads that same total turn
/// across many ticks instead, so no single tick's chord is large. This
/// steps at the physics engine's own fixed tick (rather than this file's
/// other tests' `1.0 / 60.0`, which does not divide evenly into
/// `ohl_physics::controller::TICK_SECONDS` and so can coalesce more than
/// one physics step into a single `Game::tick` call, hiding exactly the
/// single-tick spike this test exists to catch) so every simulation step
/// through the corner is actually observed.
#[test]
fn a_riders_reported_speed_never_exceeds_the_cars_own_by_more_than_a_small_bound() {
    let tick_seconds = ohl_physics::controller::TICK_SECONDS;
    let mut game = loaded();

    for _ in 0..40 {
        game.tick(tick_seconds, &Input::default());
    }

    let total =
        BEND_TRAIN_CORNER[0] - BEND_TRAIN_ORIGIN[0] + BEND_TRAIN_END[1] - BEND_TRAIN_CORNER[1];
    let seconds = total / BEND_TRAIN_SPEED + 2.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / tick_seconds).round() as u32;

    let bound = BEND_TRAIN_SPEED * 2.0;
    for step in 0..steps {
        game.tick(tick_seconds, &Input::default());
        let reported = game.ground_mover_speed();
        assert!(
            reported <= bound,
            "step {step}: ground_mover_speed reported {reported}, more than {bound} \
             (twice the car's own {BEND_TRAIN_SPEED} units/second travel speed)"
        );
    }
}
