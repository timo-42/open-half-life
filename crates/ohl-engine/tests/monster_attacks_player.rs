//! A hostile monster's attack reduces the player's health: the AI reports
//! it (phase 8), the shared damage queue carries it, and phase 9 resolves
//! it through `ohl_player::Player::apply_damage` rather than dropping it
//! (a `QueuedDamage` targeting the player used to be silently discarded by
//! phase 10's monster-only drain when phase 9 was still an empty hook).
//!
//! Reuses `ohl_engine::test_support::ai_room_bsp`, the same fixture M7.9 P2's
//! own `tests/ai_wiring.rs` builds its rooms from. No bytes here come from
//! any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{AI_MAP, ai_room_bsp};
use ohl_engine::{Game, Input, MemoryAssets};

/// A room with a player start facing `+X` and a single hostile monster
/// close enough to acquire and attack the player quickly.
fn entities() -> String {
    "{\n\"classname\" \"worldspawn\"\n}\n\
     {\n\"classname\" \"info_player_start\"\n\"origin\" \"-64 0 36\"\n\"angle\" \"0\"\n}\n\
     {\n\"classname\" \"monster_human_grunt\"\n\"origin\" \"64 0 36\"\n\"angle\" \"180\"\n}\n"
        .to_string()
}

fn game() -> Game {
    let bytes = ai_room_bsp(&entities(), false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(ohl_engine::TICK_SECONDS, &input);
    }
}

/// A hostile monster within range eventually attacks the player, and the
/// attack actually reaches `ohl_player::Player`'s health — not just the
/// AI's own internal bookkeeping.
#[test]
fn a_monster_attack_reduces_player_health() {
    let mut game = game();
    assert_eq!(game.monster_count(), 1);
    assert!(
        (game.player_health() - 100.0).abs() < f32::EPSILON,
        "the player starts at full health"
    );

    // Comfortably long enough for the monster to acquire the player, close
    // the distance and land at least one attack.
    tick(&mut game, 3_000);

    assert!(
        game.player_health() < 100.0,
        "a hostile monster's attack must reduce the player's health, not \
         be silently dropped by phase 10's monster-only drain"
    );
}
