//! A `func_rot_button` pressed through the real `use_pressed` input path —
//! `ohl_game::logic::find_usable_within` locates it by proximity, exactly
//! the same [`ohl_engine::Game::tick`] path a player's own "use" key drives
//! — rotates and, once it finishes swinging, fires its `target`, opening a
//! plain `func_door`.
//!
//! `crates/ohl-game/src/logic.rs`'s own
//! `func_rot_button_presses_fires_target_and_auto_returns` unit test already
//! proves the underlying `RotButton` state machine and `target` firing in
//! isolation (`Simulation::use_entity`, called directly by entity, not by
//! proximity). This integration test additionally drives the button
//! through the real `use_pressed` proximity path
//! (`ohl_game::logic::find_usable_within` -> `ohl_game::pose::
//! brush_center`), the same real path `crates/ohl-engine/tests/
//! rotating_door.rs` now also drives its own rotating door through (`docs/
//! FORMAT_SOURCES.md`'s `TODO(black-box)` item 25, the `BrushCenter` gap
//! that once kept a proximity press from finding an origin-brush entity at
//! all, is resolved). This fixture's `func_rot_button` carries an `origin`
//! keyvalue of `0 0 0` purely as a simplification — `BrushCenter` already
//! adds `origin` unconditionally, so a zero one just means the button's
//! compiled box can be placed directly at its true world position without
//! also working out a compile-relative-to-pivot offset — not a workaround
//! this test still needs.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_combat::WeaponId;
use ohl_engine::test_support::{
    ROT_BUTTON_DOOR_NAME, ROT_BUTTON_HEALTH_DOOR_NAME, ROT_BUTTON_HEALTH_MAP,
    ROT_BUTTON_HEALTH_NAME, ROT_BUTTON_MAP, ROT_BUTTON_NAME, rot_button_bsp, rot_button_entities,
    rot_button_health_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::{Door, MoverState, RotButton};

const STEP: f32 = 1.0 / 60.0;

fn game() -> Game {
    let bytes = rot_button_bsp(&rot_button_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROT_BUTTON_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROT_BUTTON_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn button_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROT_BUTTON_NAME)
        .first()
        .expect("the fixture declares one named rot button");
    registry
        .world
        .get::<&RotButton>(entity)
        .expect("the named entity is a rot button")
        .state
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROT_BUTTON_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// Idling near the button without pressing "use" leaves it, and the door it
/// targets, untouched.
#[test]
fn idling_near_the_button_does_nothing() {
    let mut game = game();
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(button_state(&game), MoverState::Closed);
    assert_eq!(door_state(&game), MoverState::Closed);
}

/// A single `use_pressed` frame while standing in reach — the same
/// `ohl_engine::USE_RADIUS` proximity search a player's own "use" key drives
/// through `ohl_engine::Systems::triggers_and_movers` — presses the button
/// (found via `ohl_game::logic::find_usable_within`, not forced state),
/// swings it through its full `distance`, and fires its `target`, opening
/// the door.
#[test]
fn pressing_use_in_reach_swings_the_button_and_opens_its_target_door() {
    let mut game = game();
    assert_eq!(button_state(&game), MoverState::Closed);

    let press = Input {
        use_pressed: true,
        ..Input::default()
    };
    game.tick(STEP, &press);
    assert_eq!(
        button_state(&game),
        MoverState::Opening,
        "the press must be found by proximity and start the button swinging"
    );

    // 90 degrees at 360 degrees/second finishes in a quarter second.
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(button_state(&game), MoverState::Open);
    assert_eq!(
        door_state(&game),
        MoverState::Open,
        "the button's target door must have opened once it finished pressing"
    );
}

// ---------------------------------------------------------------------
// A health-gated `func_rot_button`, pressed by a real weapon shot rather
// than a `use` press — `ohl_game::logic::Simulation::damage_button`,
// closing the `TODO(black-box)` `docs/FORMAT_SOURCES.md` item 27 recorded.
// ---------------------------------------------------------------------

fn health_game() -> Game {
    let bytes = rot_button_bsp(&rot_button_health_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROT_BUTTON_HEALTH_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROT_BUTTON_HEALTH_MAP).expect("the fixture loads")
}

fn health_button_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROT_BUTTON_HEALTH_NAME)
        .first()
        .expect("the fixture declares one named rot button");
    registry
        .world
        .get::<&RotButton>(entity)
        .expect("the named entity is a rot button")
        .state
}

fn health_door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROT_BUTTON_HEALTH_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// A real weapon shot from the player's own spawn point presses a
/// `health`-gated `func_rot_button` and opens its target door — the
/// documented "(or by being shot, if Health is > 0)" `func_rot_button`
/// activation path (`docs/FORMAT_SOURCES.md` item 27), driven end to end
/// through the real `Game` loop: pick up a `weapon_357` sitting at the
/// spawn point, select it, reload it, then fire a single shot along the
/// spawn's own facing straight at the button. No forced component state
/// anywhere in this test — every step is an `Input` a real host could send.
#[test]
fn a_weapon_shot_from_spawn_presses_a_health_gated_button_and_opens_its_target_door() {
    let mut game = health_game();
    assert_eq!(health_button_state(&game), MoverState::Closed);
    assert_eq!(health_door_state(&game), MoverState::Closed);

    // Step 1: the weapon sits exactly at the spawn point, so the very
    // first pickup-touch step already grants it.
    game.tick(1.0 / 60.0, &Input::default());
    assert!(
        game.inventory().has_weapon(WeaponId::Python),
        "the weapon_357 at the spawn point must be picked up on the first step"
    );

    // Step 2: select the newly owned .357 (HUD slot 2).
    let select = Input {
        select_slot: Some(2),
        ..Input::default()
    };
    game.tick(1.0 / 60.0, &select);
    assert_eq!(game.inventory().selected(), Some(WeaponId::Python));

    // Step 3: a freshly given weapon's clip starts empty; reload it, then
    // idle past the published reload time so the clip is actually loaded
    // before the shot.
    let reload = Input {
        reload: true,
        ..Input::default()
    };
    game.tick(1.0 / 60.0, &reload);
    tick_n(&mut game, 180, &Input::default());
    assert!(
        game.inventory().clip(WeaponId::Python) > 0,
        "reload must have loaded the .357's clip before the shot"
    );

    // Step 4: fire once, straight down the spawn's own facing (`angle 90`,
    // aimed directly at the button — see `rot_button_health_entities`'s own
    // doc comment for why no pitch adjustment is needed).
    let shoot = Input {
        attack: true,
        ..Input::default()
    };
    game.tick(1.0 / 60.0, &shoot);

    assert_eq!(
        health_button_state(&game),
        MoverState::Opening,
        "a single .357 shot (40 damage) must exhaust the button's 30 health and press it"
    );
    // 90 degrees at 360 degrees/second finishes in a quarter second.
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(health_button_state(&game), MoverState::Open);
    assert_eq!(
        health_door_state(&game),
        MoverState::Open,
        "the damage-triggered press must fire target and open the door, same as a use press"
    );
}

/// The same shot fixture, but the button is never damaged: idling (no
/// attack) never presses it, so a `health`-gated button is not somehow
/// pressed by mere proximity or the hitbox this package added for it to be
/// shootable.
#[test]
fn idling_near_a_health_gated_button_does_not_press_it() {
    let mut game = health_game();
    tick_n(&mut game, 60, &Input::default());
    assert_eq!(health_button_state(&game), MoverState::Closed);
    assert_eq!(health_door_state(&game), MoverState::Closed);
}
