//! `func_monsterclip` must not block the player.
//!
//! TWHL wiki `func_monsterclip` (search-engine result summary; the page
//! itself returns HTTP 403 to automated fetches from this environment, the
//! same caveat `docs/FORMAT_SOURCES.md` already records for other TWHL
//! citations): "an invisible brush entity" that is "solid to monsters" but
//! "not solid to players" — used by mappers to shape monster paths without
//! affecting where the player can walk. See `docs/FORMAT_SOURCES.md`, item
//! 30.
//!
//! This fixture reuses `ohl_engine::test_support::rotating_door_bsp`'s own
//! corridor geometry (a 192-unit-wide corridor with a real solid submodel
//! `*1` sitting across it) through the new
//! `ohl_engine::test_support::corridor_brush_entities` helper, which swaps
//! in a caller-chosen classname for that submodel instead of a
//! `func_door_rotating`. The `func_wall` case is the control: it proves the
//! fixture's geometry really does block an ordinary solid brush entity, so
//! the `func_monsterclip` case walking through it is a real regression
//! guard for the engine gap this milestone fixes, not an artefact of a
//! corridor that was never solid to begin with.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    CLIP_CORRIDOR_MAP, ROTATING_DOOR_MAXS, ROTATING_DOOR_MINS, corridor_brush_entities,
    rotating_door_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

fn game_with(classname: &str) -> Game {
    let bytes = rotating_door_bsp(&corridor_brush_entities(classname));
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{CLIP_CORRIDOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, CLIP_CORRIDOR_MAP).expect("the fixture loads")
}

fn walk_forward(game: &mut Game, ticks: u32) {
    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    for _ in 0..ticks {
        game.tick(STEP, &forward);
    }
}

/// Control: an ordinary `func_wall` across the corridor really does stop
/// the player, proving the fixture's geometry is solid when the engine
/// says it should be.
#[test]
fn a_func_wall_blocks_the_corridor() {
    let mut game = game_with("func_wall");
    walk_forward(&mut game, 600);

    let x = game.eye_position()[0];
    assert!(
        x < ROTATING_DOOR_MINS[0],
        "a func_wall did not stop the player: eye x = {x}, brush starts at x = {}",
        ROTATING_DOOR_MINS[0]
    );
}

/// The regression guard: the same corridor, blocked only by a
/// `func_monsterclip`, is walkable — the brush must not be attached to the
/// player's collision model at all.
#[test]
fn a_func_monsterclip_does_not_block_the_player() {
    let mut game = game_with("func_monsterclip");
    walk_forward(&mut game, 600);

    let x = game.eye_position()[0];
    assert!(
        x > ROTATING_DOOR_MAXS[0],
        "a func_monsterclip blocked the player: eye x = {x}, brush ends at x = {}",
        ROTATING_DOOR_MAXS[0]
    );
}
