//! World collision keeps following map logic when player movement stops.
//!
//! Project-authored regression using synthetic corridor geometry only.
//! The delayed door starts moving after lethal damage has already latched
//! player death, so syncing collision only inside the alive guard fails.

use glam::Vec3;
use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_PIVOT, entity_block, rotating_door_bsp,
};
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::registry::{Door, MoverState};
use ohl_physics::Hull;

fn game() -> Game {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n{}{}{}{}",
        entity_block("info_player_start", [0.0, 0.0, 40.0], 0.0, &[]),
        entity_block("trigger_hurt", [0.0, 0.0, 40.0], 0.0, &[("dmg", "1000")],),
        entity_block(
            "func_door",
            ROTATING_DOOR_PIVOT,
            90.0,
            &[
                ("model", "*1"),
                ("targetname", "ohl_delayed_door"),
                ("speed", "100"),
                ("lip", "0"),
                ("wait", "-1"),
            ],
        ),
        entity_block(
            "trigger_auto",
            [0.0, 0.0, 0.0],
            0.0,
            &[("target", "ohl_delayed_door"), ("delay", "0.2")],
        ),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ROTATING_DOOR_MAP}.bsp"),
        rotating_door_bsp(&entities),
    );
    Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("synthetic corridor loads")
}

fn door_state(game: &Game) -> MoverState {
    game.registry()
        .world
        .query::<&Door>()
        .iter()
        .next()
        .expect("one synthetic door")
        .state
}

/// Both collision models must clear the old trailing edge and acquire the
/// new leading edge of a door that moves while the player remains dead.
#[test]
fn a_door_moving_after_player_death_updates_monster_traces() {
    let mut game = game();
    let across_door = |collision: &ohl_physics::CollisionModel, y| {
        collision.trace(
            Hull::Point,
            Vec3::new(160.0, y, 48.0),
            Vec3::new(224.0, y, 48.0),
        )
    };
    for collision in [game.collision().unwrap(), game.monster_collision().unwrap()] {
        assert!(across_door(collision, -80.0).fraction < 1.0);
        assert!(across_door(collision, 94.0).fraction >= 1.0);
    }

    let mut deaths = 0;
    for _ in 0..5 {
        deaths += game
            .tick(TICK_SECONDS, &Input::default())
            .iter()
            .filter(|event| matches!(event, GameEvent::PlayerDied))
            .count();
    }
    assert_eq!(deaths, 1, "death must latch before the door starts moving");
    assert!(game.player_health() <= 0.0);
    assert_eq!(door_state(&game), MoverState::Closed);
    let dead_origin = game.player_origin().map(f32::to_bits);

    for _ in 0..45 {
        deaths += game
            .tick(
                TICK_SECONDS,
                &Input {
                    forward: 1,
                    ..Input::default()
                },
            )
            .iter()
            .filter(|event| matches!(event, GameEvent::PlayerDied))
            .count();
    }
    assert_eq!(deaths, 1);
    assert_eq!(game.player_origin().map(f32::to_bits), dead_origin);
    assert_eq!(door_state(&game), MoverState::Opening);
    for collision in [game.collision().unwrap(), game.monster_collision().unwrap()] {
        assert!(
            across_door(collision, -80.0).fraction >= 1.0,
            "the door's old trailing edge must be clear after death"
        );
        assert!(
            across_door(collision, 94.0).fraction < 1.0,
            "the door's new leading edge must block after death"
        );
    }
}
