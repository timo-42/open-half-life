//! Property: an arbitrary sequence of `Input`s never drives the player's
//! health, armor, clip or reserve ammo out of range, and never panics.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_combat::AmmoType;
use ohl_engine::test_support::synthetic_map_bsp_with_extra_entity;
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, TICK_SECONDS};
use proptest::prelude::*;

const MAP: &str = "ohlsynth";

/// The fixture grants a `.357` within reach of the spawn point and an
/// `item_battery` beside it, so a run that happens to walk forward and
/// select slot 2 actually exercises the clip/armor bounds this property
/// claims to check, rather than never reaching `CombatState::weapons` at
/// all (an empty inventory selects nothing).
fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    let bytes = synthetic_map_bsp_with_extra_entity(
        "ohlsynth2",
        "{\n\"classname\" \"weapon_357\"\n\"origin\" \"10 0 32\"\n}\n\
         {\n\"classname\" \"item_battery\"\n\"origin\" \"-10 0 32\"\n}\n\
         {\n\"classname\" \"item_suit\"\n\"origin\" \"0 10 32\"\n}\n",
    );
    assets.insert(&format!("maps/{MAP}.bsp"), bytes);
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, MAP).expect("the synthetic map loads")
}

/// An arbitrary input snapshot, including axis values and mouse deltas
/// outside anything a real host would ever deliver.
fn arbitrary_input() -> impl Strategy<Value = Input> {
    (
        (any::<i8>(), any::<i8>(), any::<i8>()),
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        (any::<bool>(), any::<bool>(), any::<bool>()),
        proptest::option::of(any::<u8>()),
        (any::<f32>(), any::<f32>()),
    )
        .prop_map(
            |(
                (forward, right, up),
                (jump, duck, use_pressed, use_held),
                (attack, attack2, reload),
                select_slot,
                mouse_delta,
            )| Input {
                forward,
                right,
                up,
                jump,
                duck,
                use_pressed,
                use_held,
                attack,
                attack2,
                reload,
                select_slot,
                flashlight_pressed: reload,
                mouse_delta,
            },
        )
}

fn assert_in_range(game: &Game) {
    let health = game.player_health();
    assert!(health.is_finite(), "health must stay finite");
    assert!(
        (0.0..=100.0 + f32::EPSILON).contains(&health),
        "health {health} out of the published 0..=100 range"
    );

    let armor = game.player_armor();
    assert!(armor.is_finite(), "armor must stay finite");
    assert!(
        (0.0..=100.0 + f32::EPSILON).contains(&armor),
        "armor {armor} out of the published 0..=100 range"
    );

    let inventory = game.inventory();
    if let Some(selected) = inventory.selected() {
        let clip = inventory.clip(selected);
        let cap = ohl_combat::spec(selected).clip_size.unwrap_or(0);
        assert!(clip <= cap, "clip {clip} exceeds the weapon's own {cap}");
    }
    for kind in AmmoType::ALL {
        let current = inventory.ammo(kind).current();
        let cap = kind.default_capacity();
        assert!(
            current <= cap,
            "{kind:?} reserve {current} exceeds its published (or black-box) cap {cap}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn arbitrary_input_sequences_never_leave_range_and_never_panic(
        inputs in proptest::collection::vec(arbitrary_input(), 1..40),
    ) {
        let assets = assets();
        let mut game = game(&assets);
        for input in inputs {
            game.tick(TICK_SECONDS, &input);
            assert_in_range(&game);
        }
    }
}
