//! A map whose floor is a brush entity must still hold the player up.
//!
//! The map compiler moves every brush entity out of the worldspawn model
//! and into its own submodel, so a floor a mapper built as a `func_wall` is
//! not in the worldspawn hulls at all. The fixture here is exactly that
//! case — an empty void for submodel 0 and a slab for submodel 1 — so a
//! level that only traced the worldspawn hulls would drop the player
//! through it forever.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_formats::test_support::{BRUSH_FLOOR_TOP_Z, build_brush_entity_floor_bsp};

const MAP: &str = "ohlbrushfloor";
const STEP: f32 = 1.0 / 60.0;

fn game_with(classname: &str) -> Game {
    let bytes = build_brush_entity_floor_bsp(classname);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, MAP).expect("the synthetic map loads")
}

fn settle(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for _ in 0..steps {
        game.tick(STEP, &Input::default());
    }
}

#[test]
fn a_func_wall_floor_holds_the_player_up() {
    let mut game = game_with("func_wall");
    settle(&mut game, 2.0);

    let z = game.eye_position()[2];
    assert!(
        z > BRUSH_FLOOR_TOP_Z,
        "the player sank to {z}, below a floor at {BRUSH_FLOOR_TOP_Z}"
    );
    // Standing on the slab puts the origin 36 units up and the eye 28 above
    // that; the exact eye height is `MoveConfig`'s business, so this only
    // asserts the player is resting somewhere sensible above the slab.
    assert!(z < BRUSH_FLOOR_TOP_Z + 128.0, "the player floated to {z}");
}

#[test]
fn a_non_solid_brush_entity_is_not_attached_and_does_not_hold_the_player_up() {
    // `func_illusionary` is documented as a drawn-but-non-solid brush, so
    // the same slab must not become a floor.
    let mut game = game_with("func_illusionary");
    settle(&mut game, 2.0);
    assert!(
        game.eye_position()[2] < BRUSH_FLOOR_TOP_Z,
        "a func_illusionary was treated as solid"
    );
}

#[test]
fn a_trigger_volume_is_not_attached_either() {
    let mut game = game_with("trigger_multiple");
    settle(&mut game, 2.0);
    assert!(
        game.eye_position()[2] < BRUSH_FLOOR_TOP_Z,
        "a trigger volume was treated as solid"
    );
}
