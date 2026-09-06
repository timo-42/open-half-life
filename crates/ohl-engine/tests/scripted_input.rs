//! M7.9 P4a: the headless scripted-input determinism guarantee that
//! `xtask combat-smoke` and `ohl-app --script` both rely on.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. See `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{AI_MAP, ai_room_bsp, run_script};
use ohl_engine::{Game, Input, MemoryAssets};

/// A monster room, a player start, and a fixed scripted input sequence: a
/// deterministic mix of movement, turning and idle ticks that never
/// depends on wall-clock time or map content, only on the fixed seed
/// (`ohl-engine`'s `systems::DEFAULT_RNG_SEED`).
fn scripted_inputs() -> Vec<Input> {
    let mut inputs = Vec::new();
    for _ in 0..200 {
        inputs.push(Input {
            forward: 1,
            mouse_delta: (2.0, -1.0),
            ..Input::default()
        });
    }
    for _ in 0..100 {
        inputs.push(Input::default());
    }
    for _ in 0..100 {
        inputs.push(Input {
            right: 1,
            jump: true,
            ..Input::default()
        });
    }
    inputs
}

fn game_from(entities: &str) -> Game {
    let bytes = ai_room_bsp(entities, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
}

fn entities(extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {extra}"
    )
}

fn monster(classname: &str, origin: [f32; 3], yaw: f32) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"{yaw}\"\n}}\n",
        origin[0], origin[1], origin[2]
    )
}

/// Two games built from the same map bytes and the same default seed,
/// ticked with the same scripted input sequence, produce identical
/// `ai_state_hash` after every one of that sequence's ticks — not just at
/// the end, so a divergence partway through is caught rather than masked
/// by a later tick coincidentally re-converging.
#[test]
fn a_scripted_run_is_deterministic_across_two_fresh_games() {
    let block = entities(&monster("monster_headcrab", [128.0, 0.0, 36.0], 180.0));
    let inputs = scripted_inputs();

    let mut first = game_from(&block);
    let mut second = game_from(&block);

    for input in &inputs {
        first.tick(ohl_engine::TICK_SECONDS, input);
        second.tick(ohl_engine::TICK_SECONDS, input);
        assert_eq!(
            first.ai_state_hash(),
            second.ai_state_hash(),
            "two fresh games diverged mid-script"
        );
    }

    assert_eq!(first.ai_state_hash(), second.ai_state_hash());
}

/// `test_support::run_script` reproduces the same tick-by-tick result as
/// calling `Game::tick` directly in a loop: it is a thin convenience, not
/// a different code path.
#[test]
fn run_script_matches_ticking_the_same_inputs_by_hand() {
    let block = entities(&monster("monster_headcrab", [128.0, 0.0, 36.0], 180.0));
    let inputs = scripted_inputs();

    let mut via_helper = game_from(&block);
    run_script(&mut via_helper, &inputs);

    let mut via_hand = game_from(&block);
    for input in &inputs {
        via_hand.tick(ohl_engine::TICK_SECONDS, input);
    }

    assert_eq!(via_helper.ai_state_hash(), via_hand.ai_state_hash());
}
