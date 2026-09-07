//! A monster's `ohl_ai::Actor` (sensing, navigation, attacks) and its
//! `Transform` (rendering, and what the hitbox index `crate::combat::
//! rebuild_hitbox_index` rebuilds from) must never drift apart:
//!
//! - Every step, phase 8b (`crate::systems::Systems::sync_monster_transforms`)
//!   copies a walking monster's freshly-moved `Actor` onto its `Transform`,
//!   so the rendered model — and the hitbox index the next step's phase 5
//!   rebuilds — actually follows the route `AiWorld::tick` (phase 8) just
//!   advanced, instead of staying pinned at wherever the monster last stood
//!   (the model, and a shot's hit test, never used to move at all).
//! - Right after `Game::restore` loads a save, `Systems::
//!   sync_actor_from_transforms` copies the just-restored `Transform` back
//!   onto `Actor`, so a reloaded monster's next think step starts from the
//!   save's own position rather than from `ohl_ai::attach_monsters`'s
//!   spawn-time default.
//!
//! Scripted possession still winning the `Transform` write (`crate::ai`'s
//! `place`, guarded by `ohl_ai::ScriptHold`) is exercised by the existing
//! `tests/scripted_sequences.rs`, unmodified by this change.
//!
//! Reuses `ohl_engine::test_support::ai_room_bsp`, the same fixture
//! `tests/ai_wiring.rs` builds its rooms from. No bytes here come from any
//! game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{AI_MAP, actor_origin, ai_room_bsp, monster_entities};
use ohl_engine::{Game, Input, MemoryAssets};
use ohl_game::registry::Transform;

/// The room's entity block: a worldspawn, a player start facing `+X`, and
/// whatever the test adds.
fn entities(extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {extra}"
    )
}

/// A `monster_*` entity block at `origin`, facing `yaw`.
fn monster(classname: &str, origin: [f32; 3], yaw: f32) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"{yaw}\"\n}}\n",
        origin[0], origin[1], origin[2]
    )
}

fn game_from(block: &str) -> (MemoryAssets, Game) {
    let bytes = ai_room_bsp(block, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads");
    (assets, game)
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(ohl_engine::TICK_SECONDS, &input);
    }
}

fn transform_origin(game: &Game, entity: ohl_game::hecs::Entity) -> ohl_ai::Vec3 {
    game.registry()
        .world
        .get::<&Transform>(entity)
        .map(|transform| transform.origin)
        .unwrap_or_default()
}

/// A monster with nowhere it needs to see the player still wanders
/// (`ai_wiring.rs`'s `a_map_without_nodes_still_moves_monsters` already
/// covers that its `Actor` moves): this asserts its `Transform` — the
/// rendered model's own position, and what the hitbox index reads — tracks
/// that move step for step rather than staying pinned at spawn.
#[test]
fn a_walking_monsters_transform_tracks_its_actor() {
    let block = entities(&monster("monster_zombie", [200.0, 0.0, 36.0], 180.0));
    let (_assets, mut game) = game_from(&block);
    let entity = monster_entities(&game)[0];
    let start = transform_origin(&game, entity);

    tick(&mut game, 400);

    let actor = actor_origin(&game, entity);
    let transform = transform_origin(&game, entity);
    assert!(
        (actor - start).length() > 1.0,
        "the monster must actually have walked for this test to mean anything"
    );
    assert!(
        (transform - actor).length() < 1e-3,
        "the rendered Transform must track the AI's own Actor::origin \
         (Transform {transform:?}, Actor {actor:?}) — a route that moves \
         the monster must move the model and the hitbox index with it"
    );
}

/// A monster that has walked away from its map spawn point, saved and
/// reloaded, comes back with `Actor::origin`/`yaw` at exactly the save's
/// `Transform`, not at `ohl_ai::attach_monsters`'s spawn-time default —
/// otherwise a reloaded monster's very next think step senses, navigates
/// and attacks from the wrong place.
#[test]
fn actor_matches_transform_immediately_after_a_save_load_boundary() {
    let block = entities(&monster("monster_zombie", [200.0, 0.0, 36.0], 180.0));
    let (assets, mut game) = game_from(&block);
    let entity = monster_entities(&game)[0];
    let spawn = actor_origin(&game, entity);

    tick(&mut game, 400);
    let actor_before_save = actor_origin(&game, entity);
    assert!(
        (actor_before_save - spawn).length() > 1.0,
        "the monster must actually have walked away from its map spawn \
         for this test to catch attach_monsters's spawn default leaking \
         back in"
    );

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the mid-walk save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the mid-walk save loads");

    let reloaded_entity = monster_entities(&reloaded)[0];
    let actor = actor_origin(&reloaded, reloaded_entity);
    let transform = transform_origin(&reloaded, reloaded_entity);
    assert_eq!(
        actor, transform,
        "immediately after load, every monster's Actor::origin must equal \
         its just-restored Transform::origin, not the map's spawn point"
    );
    assert!(
        (actor - spawn).length() > 1.0,
        "the restored Actor::origin must be the save's walked-to position, \
         not silently equal to the map spawn point by coincidence"
    );
}
