//! A corridor blocked by a `func_breakable`, walked, shot, and walked
//! through — end to end through the real `Game` loop (M9.10,
//! `docs/FORMAT_SOURCES.md` item 32).
//!
//! Nothing here forces component state: the player picks up the weapon by
//! standing on it, selects it, reloads it and fires it through ordinary
//! `Input`s, and the shot lands on the whole-brush hitbox
//! `ohl_engine::combat::push_damageable_brush_hitboxes` publishes for a
//! breakable with hit points left. The control test walks the same corridor
//! without ever firing, so "the wall stopped blocking" is the shot's doing
//! and not the walk's.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_combat::WeaponId;
use ohl_engine::test_support::{
    BREAKABLE_DOOR_NAME, BREAKABLE_HEALTH, BREAKABLE_MAP, BREAKABLE_OBSTACLE_MAXS, OBSTACLE_NAME,
    OBSTACLE_NEAR_X, breakable_corridor_entities, obstacle_corridor_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::{Breakable, Door, MoverState};

const STEP: f32 = 1.0 / 60.0;

/// The asset source both the fresh loads and the save/load round trip read
/// the fixture map out of.
fn assets() -> MemoryAssets {
    let bytes = obstacle_corridor_bsp(
        &breakable_corridor_entities(BREAKABLE_HEALTH, 0),
        BREAKABLE_OBSTACLE_MAXS,
        false,
        false,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{BREAKABLE_MAP}.bsp"), bytes);
    assets
}

fn game_with(entities: &str) -> Game {
    let bytes = obstacle_corridor_bsp(entities, BREAKABLE_OBSTACLE_MAXS, false, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{BREAKABLE_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, BREAKABLE_MAP).expect("the synthetic map loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn player_x(game: &Game) -> f32 {
    game.eye_position()[0]
}

fn broken(game: &Game) -> bool {
    let registry = game.registry();
    let entity = *registry
        .find(OBSTACLE_NAME)
        .first()
        .expect("the fixture declares one named obstacle");
    registry
        .world
        .get::<&Breakable>(entity)
        .expect("the named entity is a breakable")
        .broken
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(BREAKABLE_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

fn forward() -> Input {
    Input {
        forward: 1,
        ..Input::default()
    }
}

/// Arms the player with the `weapon_357` sitting on the spawn point and
/// loads its clip, the same real-input recipe `rot_button.rs` uses.
fn arm(game: &mut Game) {
    game.tick(STEP, &Input::default());
    assert!(
        game.inventory().has_weapon(WeaponId::Python),
        "the weapon_357 at the spawn point must be picked up on the first step"
    );
    game.tick(
        STEP,
        &Input {
            select_slot: Some(2),
            ..Input::default()
        },
    );
    game.tick(
        STEP,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    tick_n(game, 180, &Input::default());
    assert!(
        game.inventory().clip(WeaponId::Python) > 0,
        "reload must have loaded the clip before the shot"
    );
}

/// The corridor is genuinely blocked while the breakable is intact: walking
/// forward for three simulated seconds never gets past its near face.
#[test]
fn an_intact_breakable_blocks_the_corridor() {
    let mut game = game_with(&breakable_corridor_entities(BREAKABLE_HEALTH, 0));
    tick_n(&mut game, 180, &forward());
    assert!(!broken(&game));
    let x = player_x(&game);
    assert!(
        x < OBSTACLE_NEAR_X,
        "the player walked to x = {x}, past the intact breakable's near face at {OBSTACLE_NEAR_X}"
    );
}

/// Shooting it once (40 published damage against 30 hit points) breaks it,
/// fires its `target` door, and opens the corridor: the same walk that got
/// nowhere above now walks straight through where the wall stood.
#[test]
fn shooting_a_breakable_opens_the_corridor_and_fires_its_target() {
    let mut game = game_with(&breakable_corridor_entities(BREAKABLE_HEALTH, 0));
    arm(&mut game);
    assert!(!broken(&game));
    assert_eq!(door_state(&game), MoverState::Closed);

    game.tick(
        STEP,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    assert!(
        broken(&game),
        "a single .357 shot (40 damage) must exhaust the breakable's 30 hit points"
    );

    tick_n(&mut game, 30, &Input::default());
    assert_eq!(
        door_state(&game),
        MoverState::Open,
        "breaking must fire the documented Target on Break"
    );

    tick_n(&mut game, 180, &forward());
    let x = player_x(&game);
    assert!(
        x > OBSTACLE_NEAR_X,
        "the player is still stuck at x = {x} even though the wall broke"
    );
}

/// A `func_breakable` with the documented "Only Trigger (1)" flag ignores
/// damage entirely, and breaks when its `targetname` is fired instead.
#[test]
fn an_only_trigger_breakable_ignores_damage_but_breaks_on_a_trigger() {
    let entities = format!(
        "{}{{\n\"classname\" \"trigger_auto\"\n\"target\" \"{OBSTACLE_NAME}\"\n\"delay\" \"10\"\n}}\n",
        breakable_corridor_entities(BREAKABLE_HEALTH, 1)
    );
    let mut game = game_with(&entities);
    arm(&mut game);
    game.tick(
        STEP,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    assert!(
        !broken(&game),
        "an Only Trigger breakable must not break from being shot"
    );
    // The fixture's own `trigger_auto` fires the breakable ten seconds in —
    // comfortably after `arm`'s own reload wait, so the shot above is
    // unambiguously not what breaks it.
    tick_n(&mut game, 600, &Input::default());
    assert!(
        broken(&game),
        "an Only Trigger breakable must break once something triggers it"
    );
}

/// A broken breakable is still broken after a save/load round trip (save tag
/// 33), and a save taken before it broke restores it intact.
#[test]
fn a_broken_breakable_stays_broken_across_a_save_and_load() {
    let mut game = game_with(&breakable_corridor_entities(BREAKABLE_HEALTH, 0));
    arm(&mut game);
    let before = game
        .save_bytes(1_700_000_000)
        .expect("the pre-break save writes");
    game.tick(
        STEP,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    assert!(broken(&game));
    let after = game
        .save_bytes(1_700_000_000)
        .expect("the post-break save writes");

    let mut reloaded = Game::load_bytes(&assets(), &after).expect("the post-break save loads");
    assert!(
        broken(&reloaded),
        "a breakable broken before the save must still be broken after the load"
    );
    tick_n(&mut reloaded, 180, &forward());
    assert!(
        player_x(&reloaded) > OBSTACLE_NEAR_X,
        "a restored broken breakable must not block the corridor again"
    );

    let mut rolled_back = Game::load_bytes(&assets(), &before).expect("the pre-break save loads");
    assert!(
        !broken(&rolled_back),
        "a save taken before the break must restore an intact breakable"
    );
    tick_n(&mut rolled_back, 180, &forward());
    assert!(
        player_x(&rolled_back) < OBSTACLE_NEAR_X,
        "a restored intact breakable must block the corridor again"
    );
}
