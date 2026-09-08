//! `Level::monster_collision` must actually track the same map-logic
//! state `Level::collision` (the player's) does, not just exist.
//!
//! M9.11 (`docs/FORMAT_SOURCES.md` item 33) added a second collision
//! model, built once at load and kept in step every tick by
//! `Level::sync_monster_brush_collision`, so AI-side tracing sees the same
//! opened doors, moved movers and despawned brushes the player does. A PR
//! #129 review round found this "kept in sync" claim itself had no test:
//! making `sync_monster_brush_collision` an unconditional no-op left
//! `cargo test --workspace` green and a real scenario's log byte-identical
//! — nothing in the repo would have noticed `monster_collision` freezing
//! every door and despawned brush at its compiled position forever. These
//! two tests are that regression guard.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_NAME, entity_block, killable_brush_floor_bsp,
    rotating_door_bsp, rotating_door_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

/// The rotating door's own `BrushId` in each model, found by matching
/// which attached entity carries [`ROTATING_DOOR_NAME`] — the same
/// targetname lookup `rotating_door.rs`'s own tests use, just resolved to
/// a `BrushId` in both `brush_collision`/`monster_brush_collision` lists
/// instead of a `Door` component.
fn door_brush_ids(game: &Game) -> (ohl_physics::BrushId, ohl_physics::BrushId) {
    let entity = *game
        .registry()
        .find(ROTATING_DOOR_NAME)
        .first()
        .expect("the fixture declares one named rotating door");
    let player_id = game
        .brush_collision()
        .iter()
        .find(|(e, _)| *e == entity)
        .map(|(_, id)| *id)
        .expect("the door is attached to the player's own collision model");
    let monster_id = game
        .monster_brush_collision()
        .iter()
        .find(|(e, _)| *e == entity)
        .map(|(_, id)| *id)
        .expect("the door is attached to the monster collision model too");
    (player_id, monster_id)
}

/// A door opened for the player (through the real `use` proximity path,
/// not forced state) must also be open — same brush pose — in the
/// monster model, not still sitting closed there.
#[test]
fn an_opened_door_moves_in_the_monster_model_too() {
    let bytes = rotating_door_bsp(&rotating_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");

    let (player_id, monster_id) = door_brush_ids(&game);

    // Before opening: both models agree the door sits at its closed pose
    // (the whole point of a *second* collision model is that it starts
    // out identical to the first for everything but `func_monsterclip`).
    let player_origin_closed = game
        .collision()
        .expect("collision hulls")
        .brush_origin(player_id);
    let monster_origin_closed = game
        .monster_collision()
        .expect("monster collision hulls")
        .brush_origin(monster_id);
    assert_eq!(
        player_origin_closed, monster_origin_closed,
        "both models must agree on the door's closed pose"
    );

    // Open it through the real `use` proximity path, then let the quarter
    // turn finish (`rotating_door.rs`'s own technique).
    let use_press = Input {
        use_pressed: true,
        ..Input::default()
    };
    game.tick(STEP, &use_press);
    tick_n(&mut game, 60, &Input::default());

    let player_origin_open = game
        .collision()
        .expect("collision hulls")
        .brush_origin(player_id);
    let monster_origin_open = game
        .monster_collision()
        .expect("monster collision hulls")
        .brush_origin(monster_id);

    // A rotating door's own *translation* is always zero (`render.rs`'s
    // own doc comment: only its rotation carries the swing), so the
    // meaningful "did it actually move" check is `brush_is_rotating`, not
    // `brush_origin` — but the origin comparison between the two models
    // is still exactly what would catch a frozen `monster_collision`, so
    // both are asserted.
    assert!(
        game.collision().unwrap().brush_is_rotating(player_id),
        "sanity check: the player's own model must show the door mid-rotation \
         after a quarter turn at full speed"
    );
    assert_eq!(
        player_origin_open, monster_origin_open,
        "the two models disagree on the open door's brush origin: \
         Level::sync_monster_brush_collision is not tracking it \
         (this fails outright if that sync is ever made a no-op)"
    );
    assert_eq!(
        game.collision().unwrap().brush_is_rotating(player_id),
        game.monster_collision()
            .unwrap()
            .brush_is_rotating(monster_id),
        "the monster model's door brush is not rotating: \
         Level::sync_monster_brush_collision is not tracking the open door's \
         rotation (this fails outright if that sync is ever made a no-op)"
    );
}

/// The `killtarget_brush_despawn.rs` fixture, reused here: a solid brush
/// entity a scripted `killtarget` despawns must be detached from *both*
/// collision models, not left blocking the monster model forever the way
/// it would have blocked the player before that package's own fix.
#[test]
fn a_killtargeted_brush_is_detached_from_the_monster_model_too() {
    const MAP: &str = "ohlmonsterclipsyncfloor";
    const FLOOR_NAME: &str = "ohl_floor";

    let entities = format!(
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
    );

    let bytes = killable_brush_floor_bsp(&entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), bytes);
    let mut game = Game::load(&assets as &dyn AssetSource, MAP).expect("the fixture loads");

    let floor_entity = *game
        .registry()
        .find(FLOOR_NAME)
        .first()
        .expect("the fixture declares one named floor");

    assert!(
        game.monster_brush_collision()
            .iter()
            .any(|(e, _)| *e == floor_entity),
        "the floor must start attached to the monster model"
    );

    // The script fires and kills the floor within the first tick
    // (`killtarget_brush_despawn.rs`'s own timing note); settle a little
    // longer for good measure.
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(
        game.script_completion_count(),
        1,
        "the script never completed, so the killtarget never fired"
    );

    assert!(
        !game
            .brush_collision()
            .iter()
            .any(|(e, _)| *e == floor_entity),
        "sanity check: the floor must be gone from the player's own model too"
    );
    assert!(
        !game
            .monster_brush_collision()
            .iter()
            .any(|(e, _)| *e == floor_entity),
        "the killtargeted floor is still attached to the monster model: \
         Level::sync_monster_brush_collision is not detaching a despawned \
         entity's brush (this fails outright if that sync is ever made a \
         no-op)"
    );
}
