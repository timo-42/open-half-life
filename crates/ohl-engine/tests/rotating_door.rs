//! A `func_door_rotating` blocks a corridor while closed and, once opened
//! through a `use` press, swings clear of it — proving the rotation this
//! package adds to `ohl_physics::CollisionModel::set_brush_pose` actually
//! blocks and unblocks the player, not just draws differently.
//!
//! `ohl_engine::test_support::rotating_door_bsp`/`rotating_door_entities`
//! build a 192-unit-wide corridor (`y` in `-96..96`) with a real solid door
//! leaf (176 units wide, `y` in `-88..88`) pivoting about its own `y = -88`
//! edge; TWHL wiki `func_door_rotating` (`docs/FORMAT_SOURCES.md`, "Entity
//! keyvalues and map logic") documents the entity's `origin` keyvalue —
//! placed there by a required "origin brush" — as the axis to rotate on.
//! See [`ohl_engine::test_support::ROTATING_DOOR_MINS`]'s own doc comment
//! for why the fixture is sized this generously relative to the standing
//! hull.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_MAXS, ROTATING_DOOR_MINS, ROTATING_DOOR_NAME,
    rotating_door_bsp, rotating_door_entities, use_input,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::{Door, MoverState};

const STEP: f32 = 1.0 / 60.0;

fn game() -> Game {
    let bytes = rotating_door_bsp(&rotating_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROTATING_DOOR_NAME)
        .first()
        .expect("the fixture declares one named rotating door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// A door with no rotation at all (never `use`d) fully blocks the corridor:
/// walking forward for far longer than it would take to reach it leaves the
/// player short of its closed leaf, not through it.
#[test]
fn a_closed_rotating_door_blocks_the_corridor() {
    let mut game = game();
    assert_eq!(door_state(&game), MoverState::Closed);

    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    tick_n(&mut game, 600, &forward);

    let x = game.eye_position()[0];
    assert!(
        x < ROTATING_DOOR_MINS[0],
        "the closed door did not stop the player: eye x = {x}, door starts at x = {}",
        ROTATING_DOOR_MINS[0]
    );
}

/// `use` opens the door 90 degrees (TWHL wiki `func_door_rotating`'s
/// `distance`/`speed`, `docs/FORMAT_SOURCES.md`), and once it has finished
/// swinging clear of the corridor the same forward walk that was blocked
/// above now carries the player through the doorway and past it.
#[test]
fn using_the_door_opens_it_and_the_player_walks_through() {
    let mut game = game();

    tick_n(&mut game, 1, &use_input());
    assert_ne!(
        door_state(&game),
        MoverState::Closed,
        "the nearest usable entity did not start moving"
    );

    // 90 degrees at 360 degrees/second finishes in a quarter second (15
    // ticks); give it a healthy margin before checking the state, then
    // confirm it actually reached Open (not just Opening) before walking.
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(
        door_state(&game),
        MoverState::Open,
        "the door did not finish opening in time"
    );

    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    tick_n(&mut game, 600, &forward);

    let x = game.eye_position()[0];
    assert!(
        x > ROTATING_DOOR_MAXS[0],
        "the open door still blocked the player: eye x = {x}, door ends at x = {}",
        ROTATING_DOOR_MAXS[0]
    );
}
