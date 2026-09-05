//! The playable loop over the project's own synthetic map: ticking advances
//! the simulation, "use" opens the door in front of the player, and a
//! `trigger_changelevel` reached through a button surfaces a
//! [`GameEvent::LevelChange`] the host acts on.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{DOOR_NAME, LANDMARK, NEXT_MAP, SYNTHETIC_MAP, synthetic_map_bsp};
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets};
use ohl_game::registry::{Door, MoverState};

const STEP: f32 = 1.0 / 60.0;

fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), synthetic_map_bsp());
    assets.insert(&format!("maps/{NEXT_MAP}.bsp"), synthetic_map_bsp());
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, SYNTHETIC_MAP).expect("the synthetic map loads")
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    let door = registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door");
    door.state
}

#[test]
fn a_tick_advances_the_simulation_clock() {
    let assets = assets();
    let mut game = game(&assets);
    assert!(
        game.elapsed() < f32::EPSILON,
        "a fresh level starts at zero"
    );
    for _ in 0..10 {
        assert!(game.tick(STEP, &Input::default()).is_empty());
    }
    assert!(
        (game.elapsed() - 10.0 * STEP).abs() < 1e-3,
        "ten steps advance ten steps of simulated time"
    );
}

#[test]
fn an_overlong_frame_is_clamped_rather_than_tunnelling() {
    let assets = assets();
    let mut game = game(&assets);
    game.tick(60.0, &Input::default());
    assert!(game.elapsed() <= ohl_engine::MAX_TICK_SECONDS);
}

#[test]
fn the_map_loads_with_collision_and_a_brush_submodel() {
    let assets = assets();
    let game = game(&assets);
    assert_eq!(game.map(), SYNTHETIC_MAP);
    assert!(game.has_collision(), "the fixture publishes clip hulls");
    assert_eq!(game.submodel_count(), 1, "one brush entity, one submodel");
    assert_eq!(game.missing_model_count(), 0, "no studio models referenced");
    assert!(!game.has_skybox(), "the fixture publishes no sky faces");
}

#[test]
fn pressing_use_opens_the_door_in_front_of_the_player() {
    let assets = assets();
    let mut game = game(&assets);
    assert_eq!(door_state(&game), MoverState::Closed);

    game.tick(
        STEP,
        &Input {
            use_pressed: true,
            ..Input::default()
        },
    );
    assert_eq!(
        door_state(&game),
        MoverState::Opening,
        "the nearest usable entity starts moving"
    );

    for _ in 0..300 {
        game.tick(STEP, &Input::default());
    }
    assert!(
        matches!(door_state(&game), MoverState::Open | MoverState::Closing),
        "the door finishes its travel without further input"
    );
}

#[test]
fn a_change_level_event_reaches_the_host() {
    let assets = assets();
    let mut game = game(&assets);
    // Stand at the button rather than the door, so "use" reaches the
    // entity whose target is the level change.
    game.set_viewpoint([0.0, 100.0, 32.0], 0.0, 90.0);

    let mut change = None;
    for step in 0..600 {
        let events = game.tick(
            STEP,
            &Input {
                use_pressed: step == 0,
                ..Input::default()
            },
        );
        if let Some(event) = events
            .into_iter()
            .find(|event| matches!(event, GameEvent::LevelChange { .. }))
        {
            change = Some(event);
            break;
        }
    }

    let GameEvent::LevelChange { map, landmark } =
        change.expect("the button's target is a trigger_changelevel")
    else {
        panic!("the level-change event is the one that was searched for");
    };
    assert_eq!(map, NEXT_MAP);
    assert_eq!(landmark, LANDMARK);
}

#[test]
fn a_level_change_reloads_the_next_map_relative_to_the_landmark() {
    let assets = assets();
    let mut game = game(&assets);
    game.set_viewpoint([48.0, 0.0, 40.0], 0.0, 0.0);
    let before = game.eye_position();

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    assert_eq!(game.map(), NEXT_MAP);
    // Both maps place the landmark at the same origin, so the player's
    // offset from it is preserved exactly.
    let after = game.eye_position();
    for axis in 0..3 {
        assert!(
            (after[axis] - before[axis]).abs() < 1e-3,
            "the player keeps its offset from the landmark"
        );
    }
}

#[test]
fn a_missing_destination_leaves_the_current_level_running() {
    let assets = assets();
    let mut game = game(&assets);
    let error = game
        .change_level(&assets, "ohl_absent", LANDMARK)
        .expect_err("no such map is published");
    assert_eq!(error, ohl_engine::EngineError::MapNotFound);
    assert_eq!(game.map(), SYNTHETIC_MAP);
}
