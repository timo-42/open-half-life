//! A scripted walk into a `trigger_multiple` volume opens the `func_door`
//! it gates, reproducing (as a synthetic fixture) the training-map bug
//! where a door behind a touch trigger never opened for a walking player:
//! nothing tested the player's movement against a touch trigger's volume
//! at all, so its `target` never fired outside a direct `use`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    TOUCH_DOOR_MAP, TOUCH_DOOR_NAME, door_behind_touch_trigger_bsp,
    door_behind_touch_trigger_entities, run_script,
};
use ohl_engine::{Game, Input, MemoryAssets};
use ohl_game::registry::{Door, MoverState};

fn game() -> Game {
    let entities = door_behind_touch_trigger_entities();
    let bytes = door_behind_touch_trigger_bsp(&entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{TOUCH_DOOR_MAP}.bsp"), bytes.clone());
    Game::from_map_bytes(&assets, TOUCH_DOOR_MAP, &bytes).expect("the fixture loads")
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(TOUCH_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// The door starts closed: nothing has walked into the trigger volume yet.
#[test]
fn the_door_starts_closed() {
    let game = game();
    assert_eq!(door_state(&game), MoverState::Closed);
}

/// A scripted forward walk — the same shape of input the reported bug's
/// probes used — crosses the trigger volume standing between the player
/// start and the door, and the door opens without the player ever
/// pressing `use` on anything.
#[test]
fn walking_into_the_trigger_volume_opens_the_gated_door() {
    let mut game = game();
    let forward_walk: Vec<Input> = (0..300)
        .map(|_| Input {
            forward: 1,
            ..Input::default()
        })
        .collect();
    run_script(&mut game, &forward_walk);
    assert_eq!(
        door_state(&game),
        MoverState::Open,
        "walking through the touch trigger's volume must fire its target"
    );
}

/// Standing still at the player start, well short of the trigger volume,
/// never opens the door: the touch test must not fire from distance or on
/// a timer, only from the player's own bounding box actually overlapping
/// the volume.
#[test]
fn standing_at_the_start_leaves_the_door_closed() {
    let mut game = game();
    let idle: Vec<Input> = (0..300).map(|_| Input::default()).collect();
    run_script(&mut game, &idle);
    assert_eq!(door_state(&game), MoverState::Closed);
}
