//! A `func_door_rotating` blocks a corridor while closed and, once opened,
//! swings clear of it — proving the rotation this package adds to
//! `ohl_physics::CollisionModel::set_brush_pose` actually blocks and
//! unblocks the player, not just draws differently.
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
//! This does not drive the open through a `use` press and
//! `ohl_engine::USE_RADIUS` proximity: `ohl_game::registry::BrushCenter`
//! (what `find_usable_within` targets) is computed from a submodel's raw
//! compiled bounding box with no origin-keyvalue offset added, which is
//! only correct for a brush entity compiled without an origin brush.
//! `rotating_door_bsp`'s door — like a real map's own `func_door_rotating`
//! — compiles relative to its origin brush (see its own doc comment), so
//! its `BrushCenter` lands nowhere near its true world position; that gap
//! is `docs/FORMAT_SOURCES.md`'s `TODO(black-box)` item 25, discovered
//! while writing this test, and is out of this package's scope to fix.
//! `logic.rs`'s `func_door_rotating_opens_and_closes_through_the_shared_door_timer`
//! already proves `Simulation::use_entity` opens a rotating door when
//! targeted directly (by entity, not by proximity); this test instead
//! forces the door open the same way `level.rs`'s own
//! `render_and_collision_agree_on_a_rotated_door_pose` unit test does, so
//! it stays focused on rendering/colliding a rotating brush through a real
//! `Game`/`Input` tick loop.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_MAXS, ROTATING_DOOR_MINS, ROTATING_DOOR_NAME,
    rotating_door_bsp, rotating_door_entities,
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

/// A door opened 90 degrees (TWHL wiki `func_door_rotating`'s
/// `distance`/`speed`, `docs/FORMAT_SOURCES.md`) swings clear of the
/// corridor, and the same forward walk that was blocked above now carries
/// the player through the doorway and past it.
#[test]
fn an_open_door_lets_the_player_walk_through() {
    let mut game = game();

    {
        let registry = game.registry();
        let entity = *registry
            .find(ROTATING_DOOR_NAME)
            .first()
            .expect("the fixture declares one named rotating door");
        // Force it straight to `Open` (see this module's doc comment for
        // why a `use` press is not driven through proximity here); `Open`
        // is the same terminal pose `MoverState::Open` always reports
        // (`ohl_engine::render::mover_fraction`) regardless of `timer`.
        let mut door = registry
            .world
            .get::<&mut Door>(entity)
            .expect("the named entity is a door");
        door.state = MoverState::Open;
        door.timer = door.wait;
    }
    assert_eq!(door_state(&game), MoverState::Open);

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
