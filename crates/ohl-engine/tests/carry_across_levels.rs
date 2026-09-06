//! The player's health, armor, weapons and ammo survive a `Game::change_level`
//! call: `Systems::{capture_carry, restore_carry}` bind `#62`'s `PlayerCarry`
//! seam (`transition.rs`) to the real `ohl_player`/`ohl_combat` state this
//! package owns, in memory, across the transition.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_combat::{AmmoType, WeaponId};
use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, SYNTHETIC_MAP, synthetic_map_bsp_with_extra_entity,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, TICK_SECONDS};

fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    // The source map: the usual fixture plus a `.357` (with its bundled
    // ammo) and a `trigger_hurt` volume near the spawn point.
    let source = synthetic_map_bsp_with_extra_entity(
        NEXT_MAP,
        "{\n\"classname\" \"weapon_357\"\n\"origin\" \"10 0 32\"\n}\n\
         {\n\"classname\" \"trigger_hurt\"\n\"origin\" \"0 0 32\"\n\"dmg\" \"5\"\n}\n",
    );
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), source);
    // The destination is the plain fixture: it declares the same landmark,
    // but nothing this test needs to pick up.
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_extra_entity(SYNTHETIC_MAP, ""),
    );
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, SYNTHETIC_MAP).expect("the synthetic map loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, input);
    }
}

#[test]
fn ammo_and_health_persist_across_a_level_change() {
    let assets = assets();
    let mut game = game(&assets);

    // Pick up the .357 and its bundled ammo, and take some hurt-volume
    // damage, before the weapon is ever fired.
    tick_n(&mut game, 60, &Input::default());
    assert!(game.inventory().has_weapon(WeaponId::Python));
    let ammo_before_firing = game.inventory().ammo(AmmoType::ThreeFiveSeven).current();
    assert!(ammo_before_firing > 0, "the pickup bundles ammo");
    assert!(
        game.player_health() < 100.0,
        "the hurt volume must have applied before the reload wait"
    );

    // Select the .357 (HUD slot 2) and reload it: the first tick only
    // draws the weapon, the second starts the reload, and a clip-based
    // weapon's reload is what actually moves ammo from reserve into the
    // clip (firing alone only spends the clip, per `ohl_combat::firing`).
    tick_n(
        &mut game,
        1,
        &Input {
            select_slot: Some(2),
            ..Input::default()
        },
    );
    tick_n(
        &mut game,
        1,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    // Comfortably longer than any plausible reload time.
    tick_n(&mut game, 300, &Input::default());

    // Captured only now (not before the reload wait): the `trigger_hurt`
    // volume keeps applying while the reload timer runs, so health right
    // before the transition is lower than it was right after the pickup.
    let ammo_before_transition = game.inventory().ammo(AmmoType::ThreeFiveSeven).current();
    let clip_before_transition = game.inventory().clip(WeaponId::Python);
    let health_before_transition = game.player_health();
    assert!(
        ammo_before_transition < ammo_before_firing,
        "reloading must have spent some reserve ammo into the clip before \
         the transition: {ammo_before_firing} -> {ammo_before_transition}"
    );
    assert!(
        clip_before_transition > 0,
        "the reload must have loaded the clip"
    );

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    assert_eq!(game.map(), NEXT_MAP);

    assert!(
        game.inventory().has_weapon(WeaponId::Python),
        "the weapon must still be owned after the transition"
    );
    assert_eq!(
        game.inventory().ammo(AmmoType::ThreeFiveSeven).current(),
        ammo_before_transition,
        "reserve ammo must carry across the transition unchanged"
    );
    assert_eq!(
        game.inventory().clip(WeaponId::Python),
        clip_before_transition,
        "the loaded clip must carry across the transition unchanged"
    );
    assert!(
        (game.player_health() - health_before_transition).abs() < f32::EPSILON,
        "health must carry across the transition unchanged: {} -> {}",
        health_before_transition,
        game.player_health()
    );
}
