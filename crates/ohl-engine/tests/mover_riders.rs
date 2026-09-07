//! A player standing on a moving brush entity must be carried with it.
//!
//! `Level::sync_brush_collision` already moved every attached brush's
//! collision hull to where the map logic put it, once per step; this
//! package makes that move actually carry a rider: the ground trace now
//! reports which attached brush (if any) it hit
//! (`ohl_physics::hull::Trace::brush_index`), `Level::brush_velocity`
//! records that brush's per-step displacement, and the player-move phase
//! feeds it back in as `PlayerController::base_velocity` — the mechanism
//! `docs/FORMAT_SOURCES.md` already documents under "Riding movers" —
//! before the next `advance` call. See `crates/ohl-physics/tests/
//! mover_riders.rs` for the same behaviour tested directly against
//! `player_move_events`, without the engine or a real `func_train`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};

const MAP: &str = "ohlmovertrain";
const STEP: f32 = 1.0 / 100.0;

/// Half-extent of the train's brush on X and Y, and its height above/below
/// its own origin — an arbitrary, project-authored box big enough for a
/// standing player to rest on.
const TRAIN_HALF: f32 = 32.0;
const TRAIN_TOP_Z: f32 = 8.0;
const TRAIN_BOTTOM_Z: f32 = -8.0;

/// How far apart the two `path_corner` nodes are, along `+X`.
const SEGMENT_LENGTH: f32 = 150.0;
/// The `func_train`'s `speed`/`startspeed`, units/second — non-zero
/// `startspeed` starts it moving immediately at map load, without needing
/// a trigger (`ohl_game::track_train::TrackTrainState::spawn`).
const TRAIN_SPEED: f32 = 50.0;

/// A void world (submodel `*0`, no collision at all) with a single solid
/// box (submodel `*1`) resting at the world origin, referenced by a
/// `func_train` riding a two-node `path_corner` chain along `+X`. An
/// `info_player_start` sits on top of the train's brush at its resting
/// position. Every keyvalue and coordinate here is authored for this
/// project; nothing is derived from any payload.
fn train_room_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"0 0 {player_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_train\"\n\"model\" \"*1\"\n\
         \"target\" \"ohl_node1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_corner\"\n\"targetname\" \"ohl_node1\"\n\
         \"target\" \"ohl_node2\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_corner\"\n\"targetname\" \"ohl_node2\"\n\
         \"origin\" \"{segment} 0 0\"\n}}\n",
        player_z = TRAIN_TOP_Z + 36.0,
        speed = TRAIN_SPEED,
        segment = SEGMENT_LENGTH,
    ));

    // Submodel 0: the worldspawn model, with no solids of its own — the
    // train's own brush is the only thing the player can stand on.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the train's own brush, resting at its first node.
    let train_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [-TRAIN_HALF, -TRAIN_HALF, TRAIN_BOTTOM_Z],
        [TRAIN_HALF, TRAIN_HALF, TRAIN_TOP_Z],
    )]);
    b.push_model(
        [-TRAIN_HALF, -TRAIN_HALF, TRAIN_BOTTOM_Z],
        [TRAIN_HALF, TRAIN_HALF, TRAIN_TOP_Z],
        [0.0, 0.0, 0.0],
        train_heads,
        2,
        0,
        0,
    );
    b.build()
}

fn settle(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for _ in 0..steps {
        game.tick(STEP, &Input::default());
    }
}

#[test]
fn a_func_train_carries_a_standing_player_along_one_segment() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), train_room_bsp());
    let mut game = Game::load(&assets as &dyn AssetSource, MAP).expect("the synthetic map loads");

    // Let the player settle onto the train before it starts covering
    // ground, so the very first tick of travel is already a ride rather
    // than a fall onto a train that has already left.
    settle(&mut game, 0.05);
    let start = game.eye_position();
    assert!(
        (start[2] - (TRAIN_TOP_Z + 36.0 + 28.0)).abs() < 4.0,
        "the player did not settle on the train: {start:?}"
    );

    // The segment is `SEGMENT_LENGTH` units at `TRAIN_SPEED` units/second,
    // plus a little slack for the settling time already spent.
    let ride_seconds = SEGMENT_LENGTH / TRAIN_SPEED;
    settle(&mut game, ride_seconds);

    let end = game.eye_position();
    let travelled = end[0] - start[0];
    assert!(
        travelled > SEGMENT_LENGTH * 0.5,
        "the player was not carried along the train's segment: moved {travelled} of {SEGMENT_LENGTH}, from {start:?} to {end:?}"
    );
    // Riding the train must not drop the player off it or lift them clear;
    // the eye height above the (unmoving, in Z) train top should still be
    // close to the settled value.
    assert!(
        (end[2] - start[2]).abs() < 8.0,
        "the player's height above the train drifted: {start:?} -> {end:?}"
    );
}
