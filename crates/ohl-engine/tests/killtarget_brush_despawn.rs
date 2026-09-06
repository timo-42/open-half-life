//! A solid brush entity a scripted `killtarget` despawns must stop blocking
//! the player, not go on blocking it forever.
//!
//! `Level::sync_brush_collision` moves every attached brush hull to where
//! its entity currently is, once per simulation step; before this package,
//! it had no way to notice an attached brush's entity had been despawned
//! entirely (as `ai.rs`'s `finish_script_step` does for a scripted
//! sequence's `killtarget`), so the brush's collision stayed attached at
//! its last position and kept blocking the player as if nothing had
//! happened. The fixture below reuses `brush_entity_collision.rs`'s
//! void-world-plus-slab shape (a `func_wall` floor that only exists as a
//! brush entity), with a scripted sequence wired to `killtarget` the floor
//! as soon as the map loads.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{entity_block, killable_brush_floor_bsp};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_formats::test_support::BRUSH_FLOOR_TOP_Z;

const MAP: &str = "ohlkilltargetfloor";
const STEP: f32 = 1.0 / 60.0;

/// The floor's own `targetname`, named by the script's `killtarget`.
const FLOOR_NAME: &str = "ohl_floor";

/// Builds the entity block: a player start over the slab, the slab itself
/// (`func_wall`, `targetname` [`FLOOR_NAME`]), a monster to possess, and a
/// `scripted_sequence` — triggered the instant the map loads — whose only
/// job is to `killtarget` the floor. `m_fMoveTo "0"` and no `m_iszPlay`
/// mean the script neither moves the monster nor waits on an action
/// animation (see `ai.rs`'s `action_seconds`: an unset action animation
/// fires immediately), so the floor is gone within the very first tick.
fn entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_wall\"\n\"targetname\" \"{FLOOR_NAME}\"\n\
         \"model\" \"*1\"\n}}\n\
         {}{}{}",
        entity_block(
            "monster_barney",
            [64.0, 64.0, 40.0],
            0.0,
            &[("targetname", "ohl_guard")],
        ),
        entity_block(
            "scripted_sequence",
            [64.0, 64.0, 40.0],
            0.0,
            &[
                ("targetname", "ohl_script"),
                ("m_iszEntity", "ohl_guard"),
                ("m_fMoveTo", "0"),
                ("killtarget", FLOOR_NAME),
            ],
        ),
        entity_block(
            "trigger_auto",
            [0.0, 0.0, 0.0],
            0.0,
            &[("target", "ohl_script")],
        ),
    )
}

fn game_with(entities: &str) -> Game {
    let bytes = killable_brush_floor_bsp(entities);
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
fn a_killtargeted_floor_lets_the_player_fall_through() {
    let mut game = game_with(&entities());

    // The script fires, completes and kills the floor within the very
    // first tick; give it a couple more for good measure before checking
    // anything, so this is not a race against exactly which tick fires
    // `trigger_auto`.
    settle(&mut game, 0.5);
    assert_eq!(
        game.script_completion_count(),
        1,
        "the script never completed, so the killtarget never fired"
    );

    // With no floor left at all, a further couple of seconds of gravity
    // must take the player well below where the (now-gone) slab's top
    // used to be — the same floor that, left attached, held the player up
    // for two full seconds in `brush_entity_collision.rs`'s equivalent
    // test.
    settle(&mut game, 2.0);
    let z = game.eye_position()[2];
    assert!(
        z < BRUSH_FLOOR_TOP_Z - 32.0,
        "the killtargeted floor is still holding the player up at z = {z}"
    );
}

/// The control: with no `scripted_sequence` at all (so nothing ever fires
/// the `killtarget`), the same floor holds the player up exactly like
/// `brush_entity_collision.rs`'s `a_func_wall_floor_holds_the_player_up`.
/// This is what proves the fall above is the killtarget's doing and not
/// some other change to the fixture's geometry.
#[test]
fn the_same_floor_holds_the_player_up_without_a_killtarget() {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_wall\"\n\"targetname\" \"{FLOOR_NAME}\"\n\
         \"model\" \"*1\"\n}}\n"
    );
    let mut game = game_with(&entities);
    settle(&mut game, 2.0);
    let z = game.eye_position()[2];
    assert!(
        z > BRUSH_FLOOR_TOP_Z,
        "the player sank to {z} even though nothing killtargeted the floor"
    );
}
