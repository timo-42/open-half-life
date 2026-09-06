//! Pickup touch tests and use-and-hold chargers, exercised through the full
//! `Game` loop rather than `ohl-engine`'s own crate-internal state.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use std::fmt::Write as _;

use ohl_combat::{AmmoType, WeaponId};
use ohl_engine::test_support::synthetic_map_bsp_with_extra_entity;
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, TICK_SECONDS};

const NEXT_MAP: &str = "ohlsynth2";

fn assets_with_extra(extra: &str) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    let bytes = synthetic_map_bsp_with_extra_entity(NEXT_MAP, extra);
    assets.insert("maps/ohlsynth.bsp", bytes);
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, "ohlsynth").expect("the synthetic map loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, input);
    }
}

/// A `weapon_357` sitting within reach of the spawn point is picked up
/// exactly once: the weapon is unlocked, its bundled ammo is added, and
/// ticking further does not add the weapon (or its ammo) again.
#[test]
fn walking_over_a_weapon_adds_it_once() {
    let assets = assets_with_extra("{\n\"classname\" \"weapon_357\"\n\"origin\" \"10 0 32\"\n}\n");
    let mut game = game(&assets);

    tick_n(&mut game, 1, &Input::default());
    let inventory = game.inventory();
    assert!(inventory.has_weapon(WeaponId::Python));
    let after_first = inventory.ammo(AmmoType::ThreeFiveSeven).current();
    assert!(
        after_first > 0,
        "a weapon pickup bundles its published ammo"
    );

    tick_n(&mut game, 20, &Input::default());
    let inventory = game.inventory();
    assert!(inventory.has_weapon(WeaponId::Python));
    assert_eq!(
        inventory.ammo(AmmoType::ThreeFiveSeven).current(),
        after_first,
        "a taken pickup does not grant its effect again"
    );
}

/// Several `ammo_357` boxes within reach of the spawn point, well beyond the
/// published carry cap, never push the pool past that cap.
#[test]
fn ammo_pickups_respect_the_published_carry_cap() {
    let mut extra = String::new();
    for index in 0..20 {
        let _ = write!(
            extra,
            "{{\n\"classname\" \"ammo_357\"\n\"origin\" \"{} 0 32\"\n}}\n",
            f64::from(index) * 0.1
        );
    }
    let assets = assets_with_extra(&extra);
    let mut game = game(&assets);

    tick_n(&mut game, 5, &Input::default());

    let cap = AmmoType::ThreeFiveSeven
        .published_max_carry()
        .expect("the .357 carry cap is published");
    assert!(game.inventory().ammo(AmmoType::ThreeFiveSeven).current() <= cap);
}

/// A hurt-and-charger fixture: a `trigger_hurt` volume near the spawn point
/// brings health below the maximum so there is something for the charger to
/// restore, and a `func_healthcharger` sits at the same spot.
fn hurt_and_charger_assets() -> MemoryAssets {
    assets_with_extra(
        "{\n\"classname\" \"trigger_hurt\"\n\"origin\" \"0 0 32\"\n\"dmg\" \"5\"\n}\n\
         {\n\"classname\" \"func_healthcharger\"\n\"origin\" \"5 0 32\"\n}\n",
    )
}

/// `Input::use_held` (not `Input::use_pressed`, which is an edge a real
/// host clears every frame regardless of how long the key stays down) is
/// what gates the charger drain.
fn use_held_input() -> Input {
    Input {
        use_held: true,
        ..Input::default()
    }
}

/// A `func_healthcharger` restores health while `use` is held, and stops
/// once the player is topped up.
#[test]
fn a_health_charger_drains_while_use_is_held() {
    let assets = hurt_and_charger_assets();
    let mut game = game(&assets);

    // Take damage from the hurt volume for a little over half a second (the
    // documented `trigger_hurt` cadence), with `use` released.
    tick_n(&mut game, 60, &Input::default());
    let hurt_health = game.player_health();
    assert!(hurt_health < 100.0, "the hurt volume must have applied");

    // Now hold `use` near the charger (not a fresh press each tick: the
    // same `Input` snapshot fed to every `Game::tick` call, exactly as a
    // real host holding the key down would deliver it); health must climb
    // back up.
    tick_n(&mut game, 200, &use_held_input());
    let charged_health = game.player_health();
    assert!(
        charged_health > hurt_health,
        "the charger must have restored some health: {hurt_health} -> {charged_health}"
    );
}

/// A single held frame (`use_held` true for exactly one `Game::tick` call,
/// mirroring a real quick tap the host reports as one frame of the key
/// being down) restores only that one fixed step's worth of drain, not the
/// whole reservoir — and releasing `use` immediately afterward stops the
/// drain rather than continuing it.
#[test]
fn a_single_held_tick_restores_only_one_ticks_worth() {
    let assets = hurt_and_charger_assets();
    let mut game = game(&assets);

    tick_n(&mut game, 60, &Input::default());
    let hurt_health = game.player_health();
    assert!(hurt_health < 100.0, "the hurt volume must have applied");

    // Exactly one held frame.
    game.tick(TICK_SECONDS, &use_held_input());
    let after_one_tick = game.player_health();
    let healed_in_one_tick = after_one_tick - hurt_health;
    assert!(
        healed_in_one_tick > 0.0,
        "one held tick must restore something: {hurt_health} -> {after_one_tick}"
    );
    let expected = ohl_combat::CHARGER_DRAIN_RATE.value * TICK_SECONDS;
    assert!(
        (healed_in_one_tick - expected).abs() < 1e-3,
        "one held tick must restore exactly the published drain rate's worth \
         of one step ({expected}), not more: got {healed_in_one_tick}"
    );

    // `use` released for every following tick: health must not keep
    // climbing as if the key were still held. Kept well under the
    // documented `trigger_hurt` half-second cadence's remaining time so
    // this assertion is not racing the next hurt hit.
    tick_n(&mut game, 20, &Input::default());
    let after_release = game.player_health();
    assert!(
        (after_release - after_one_tick).abs() < f32::EPSILON,
        "the charger must not drain once `use_held` goes false: \
         {after_one_tick} -> {after_release}"
    );
}
