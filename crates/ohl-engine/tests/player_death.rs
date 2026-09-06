// The position-freeze assertion below wants the exact same bits the
// engine already held (no recomputation happens once the player is
// gated out of movement), so an exact comparison is the right check,
// matching `tests/camera_sequences.rs`'s own precedent.
#![allow(clippy::float_cmp)]

//! Player-death handling: a lethal `trigger_hurt` kills the player exactly
//! once, and the engine stops simulating the player's own movement from
//! that point on rather than continuing to walk (or fall) a corpse around
//! the map.
//!
//! Reuses `ohl_engine::test_support::synthetic_map_bsp_with_extra_entity`,
//! the same fixture `tests/pickups.rs`'s own `trigger_hurt` coverage
//! builds its room from. No bytes here come from any game installation;
//! see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::synthetic_map_bsp_with_extra_entity;
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};

const NEXT_MAP: &str = "ohlsynth2";

/// A `trigger_hurt` at the same spot `tests/pickups.rs` uses (the
/// `info_player_start` origin), but with damage high enough to be lethal
/// in a single application rather than needing many hits.
fn lethal_hurt_assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    let extra = "{\n\"classname\" \"trigger_hurt\"\n\"origin\" \"0 0 32\"\n\"dmg\" \"1000\"\n}\n";
    let bytes = synthetic_map_bsp_with_extra_entity(NEXT_MAP, extra);
    assets.insert("maps/ohlsynth.bsp", bytes);
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, "ohlsynth").expect("the synthetic map loads")
}

fn forward_input() -> Input {
    Input {
        forward: 1,
        ..Input::default()
    }
}

/// Ticks `game`, collecting how many `GameEvent::PlayerDied` events fire.
fn tick_counting_deaths(game: &mut Game, ticks: usize, input: &Input) -> u32 {
    let mut deaths = 0;
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, input) {
            if matches!(event, GameEvent::PlayerDied) {
                deaths += 1;
            }
        }
    }
    deaths
}

/// A lethal `trigger_hurt` the player starts standing in brings health to
/// zero and fires `GameEvent::PlayerDied` exactly once, even across many
/// more ticks of the same volume still touching the player: the death
/// latch (`ohl_player::Player::check_health_warnings`) does not refire once
/// `state.dead` is already `true`, and `apply_damage` itself refuses
/// further damage to an already-dead player.
#[test]
fn a_lethal_hurt_volume_kills_the_player_exactly_once() {
    let assets = lethal_hurt_assets();
    let mut game = game(&assets);
    assert!(
        (game.player_health() - 100.0).abs() < f32::EPSILON,
        "the player starts at full health"
    );

    // One tick is enough for a 1000-damage hit; comfortably more than that
    // covers any documented `trigger_hurt` first-touch delay.
    let deaths = tick_counting_deaths(&mut game, 30, &forward_input());
    assert_eq!(deaths, 1, "the player must die exactly once");
    assert!(
        game.player_health() <= 0.0,
        "a dead player's health is clamped at or below zero"
    );

    // The same volume is still touching the player every one of these
    // further ticks; a second `PlayerDied` would mean the death latch
    // refired.
    let more_deaths = tick_counting_deaths(&mut game, 60, &forward_input());
    assert_eq!(
        more_deaths, 0,
        "an already-dead player must not die a second time"
    );
}

/// Once the player has died, holding `forward` no longer moves them: the
/// engine stops simulating the player's own movement (`ohl_engine::Systems
/// ::step`'s phase 2) rather than continuing to walk a corpse around the
/// map, which is what let PR #95's Xen report end with the player
/// embedded far from where the fall that killed them landed.
#[test]
fn a_dead_player_stops_moving() {
    let assets = lethal_hurt_assets();
    let mut game = game(&assets);

    let deaths = tick_counting_deaths(&mut game, 30, &forward_input());
    assert_eq!(deaths, 1, "the player must die within the first 30 ticks");
    let position_at_death = game.eye_position();

    // Plenty of ticks holding forward: if movement were still simulated,
    // the player would have travelled well outside the small synthetic
    // room by now.
    for _ in 0..300 {
        game.tick(TICK_SECONDS, &forward_input());
    }
    let position_after = game.eye_position();
    assert_eq!(
        position_after, position_at_death,
        "a dead player's position must not change while forward is held"
    );
}
