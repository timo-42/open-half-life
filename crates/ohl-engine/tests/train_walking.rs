//! Walking into a moving car's walls must not leave its passenger behind.
//! All geometry and entity names here are project-authored.

use glam::{Quat, Vec3};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_game::track_train::TrackTrainState;
use ohl_physics::Hull;

const MAP: &str = "ohlwalkingtrain";
const SPEED: f32 = 100.0;
const FLOOR_Z: f32 = 128.0;

fn train_map(enclosed: bool, bends: bool) -> Game {
    train_map_with_obstacle(enclosed, bends, SPEED, &[])
}

fn train_map_with_obstacle(
    enclosed: bool,
    bends: bool,
    speed: f32,
    obstacles: &[CollisionBrush],
) -> Game {
    let mut bsp = Bsp30Builder::new();
    let corner = if bends { 400.0 } else { 4000.0 };
    bsp.set_entities_text(&format!(
        "{{\"classname\" \"worldspawn\"}}\n\
         {{\"classname\" \"info_player_start\" \"origin\" \"0 0 164\" \"angle\" \"0\"}}\n\
         {{\"classname\" \"func_tracktrain\" \"model\" \"*1\" \
         \"origin\" \"0 0 128\" \"target\" \"ohl_ride_start\" \
         \"speed\" \"{speed}\" \"startspeed\" \"{speed}\" \"height\" \"0\"}}\n\
         {{\"classname\" \"path_track\" \"targetname\" \"ohl_ride_start\" \
         \"origin\" \"0 0 128\" \"target\" \"ohl_ride_end\"}}\n\
         {{\"classname\" \"path_track\" \"targetname\" \"ohl_ride_end\" \
         \"origin\" \"{corner} 0 128\" \"target\" \"ohl_ride_final\"}}\n\
         {{\"classname\" \"path_track\" \"targetname\" \"ohl_ride_final\" \
         \"origin\" \"{corner} 2000 128\"}}\n"
    ));
    let world_heads = bsp.push_collision_hulls(obstacles);
    bsp.push_model([-8192.0; 3], [8192.0; 3], [0.0; 3], world_heads, 2, 0, 0);
    let mut brushes = vec![CollisionBrush::box_brush(
        [-128.0, -48.0, -12.0],
        [128.0, 48.0, 0.0],
    )];
    if enclosed {
        brushes.extend([
            CollisionBrush::box_brush([-128.0, -48.0, 0.0], [-120.0, 48.0, 96.0]),
            CollisionBrush::box_brush([120.0, -48.0, 0.0], [128.0, 48.0, 96.0]),
            CollisionBrush::box_brush([-128.0, -48.0, 0.0], [128.0, -40.0, 96.0]),
            CollisionBrush::box_brush([-128.0, 40.0, 0.0], [128.0, 48.0, 96.0]),
        ]);
    }
    let car_heads = bsp.push_collision_hulls(&brushes);
    bsp.push_model(
        [-128.0, -48.0, -12.0],
        [128.0, 48.0, 96.0],
        [0.0; 3],
        car_heads,
        2,
        0,
        0,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), bsp.build());
    Game::load(&assets, MAP).expect("the synthetic train map loads")
}

fn train_position(game: &Game) -> Vec3 {
    game.registry()
        .world
        .query::<&TrackTrainState>()
        .iter()
        .next()
        .expect("the fixture has one train")
        .position()
}

fn car_rotation(game: &Game) -> Quat {
    let (entity, _) = game.brush_collision()[0];
    let (axis, angle, pivot) = ohl_game::pose::brush_pose_rotation(game.registry(), entity);
    assert_eq!(
        pivot,
        Vec3::ZERO,
        "the car is compiled around its origin brush"
    );
    Quat::from_axis_angle(axis.normalize(), angle.to_radians())
}

fn assert_aboard(game: &Game, rotation: Quat, step: u32) {
    let (_, brush) = game.brush_collision()[0];
    // Map logic advances the train at the end of the tick. Use the heading
    // captured before that advance and the live collision translation.
    let origin = game.collision().unwrap().brush_origin(brush);
    let relative = rotation.inverse() * (Vec3::from_array(game.player_origin()) - origin);
    assert!(
        relative.x.abs() < 106.0 && relative.y.abs() < 26.0,
        "the passenger left the car on step {step}: {relative:?}"
    );
    assert!(
        (game.player_origin()[2] - (FLOOR_Z + 36.0)).abs() < 1.0,
        "the passenger fell through the floor on step {step}: {relative:?}"
    );
    assert!(
        game.player_on_ground(),
        "lost the train floor on step {step}: {relative:?}"
    );
    // Allow the trace's own clearance epsilon for roundoff at the floor;
    // this does not clear an overlap with any of the car's tall walls.
    let origin = Vec3::from_array(game.player_origin()) + Vec3::Z * ohl_physics::DIST_EPSILON;
    assert!(
        !game
            .collision()
            .unwrap()
            .trace(Hull::Standing, origin, origin)
            .start_solid,
        "the passenger was embedded in the car on step {step}: {relative:?}"
    );
}

#[test]
fn walking_against_the_rear_wall_keeps_the_passenger_aboard() {
    let mut game = train_map(true, false);
    for step in 0..600 {
        let rotation = car_rotation(&game);
        game.tick(
            TICK_SECONDS,
            &Input {
                forward: -1,
                ..Input::default()
            },
        );
        assert_aboard(&game, rotation, step);
    }
}

#[test]
fn walking_into_walls_and_corners_keeps_the_passenger_aboard_through_a_bend() {
    for (forward, right) in [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ] {
        let mut game = train_map(true, true);
        for step in 0..800 {
            let rotation = car_rotation(&game);
            game.tick(
                TICK_SECONDS,
                &Input {
                    forward,
                    right,
                    ..Input::default()
                },
            );
            assert_aboard(&game, rotation, step);
        }
    }
}

#[test]
fn the_passenger_can_walk_away_after_pressing_against_a_wall() {
    let mut game = train_map(true, false);
    for (forward, ticks) in [(-1, 200), (0, 100), (1, 200)] {
        for step in 0..ticks {
            let rotation = car_rotation(&game);
            game.tick(
                TICK_SECONDS,
                &Input {
                    forward,
                    ..Input::default()
                },
            );
            assert_aboard(&game, rotation, step);
        }
    }
    let relative = Vec3::from_array(game.player_origin()) - train_position(&game);
    assert!(
        relative.x > 90.0,
        "the passenger could not cross the car: {relative:?}"
    );
}

#[test]
fn the_passenger_can_still_walk_off_an_open_platform() {
    let mut game = train_map(false, false);
    for _ in 0..200 {
        game.tick(
            TICK_SECONDS,
            &Input {
                right: 1,
                ..Input::default()
            },
        );
    }
    assert!(!game.player_on_ground());
    assert!(game.player_origin()[2] < FLOOR_Z - 36.0);
}

#[test]
fn a_fast_carry_cannot_pass_through_a_thin_world_wall() {
    let wall = CollisionBrush::box_brush([44.0, -256.0, 0.0], [45.0, 256.0, 256.0]);
    let mut game = train_map_with_obstacle(false, false, 12000.0, &[wall]);
    // One mover step spans 120 units, enough to put the complete player
    // hull beyond the wall. A destination-only carry would miss it.
    for _ in 0..2 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert!(
        train_position(&game).x > 200.0,
        "the train must have crossed the wall"
    );
    assert!(
        game.player_origin()[0] < 28.0,
        "the carry crossed the world wall"
    );
    let origin = Vec3::from_array(game.player_origin());
    assert!(
        !game
            .collision()
            .unwrap()
            .trace(Hull::Standing, origin, origin)
            .start_solid
    );
}
