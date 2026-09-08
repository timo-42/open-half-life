//! A corridor blocked by a `func_pushable` crate, shoved out of the way by
//! walking into it — end to end through the real `Game` loop (M9.10,
//! `docs/FORMAT_SOURCES.md` item 32).
//!
//! Nothing here forces component state: every test walks the player with an
//! ordinary `forward` `Input` and reads the crate's own
//! `ohl_game::registry::Pushable::offset` back out afterwards. The three
//! tests together pin the shape `ohl_engine::pushables` documents: a crate
//! the player walks into moves and lets them past, a crate with a wall
//! behind it stops against that wall (and does not slide through it), and a
//! crate the player never walks into never moves.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    OBSTACLE_NAME, OBSTACLE_NEAR_X, PUSHABLE_BACK_WALL_X, PUSHABLE_CRATE_MAXS, PUSHABLE_MAP,
    obstacle_corridor_bsp, pushable_corridor_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::Pushable;

const STEP: f32 = 1.0 / 60.0;

fn game_with(friction: f32, back_wall: bool) -> Game {
    let bytes = obstacle_corridor_bsp(
        &pushable_corridor_entities(friction, back_wall),
        PUSHABLE_CRATE_MAXS,
        true,
        back_wall,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{PUSHABLE_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, PUSHABLE_MAP).expect("the synthetic map loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn push_offset_x(game: &Game) -> f32 {
    let registry = game.registry();
    let entity = *registry
        .find(OBSTACLE_NAME)
        .first()
        .expect("the fixture declares one named pushable");
    registry
        .world
        .get::<&Pushable>(entity)
        .expect("the named entity is a pushable")
        .offset
        .x
}

fn player_x(game: &Game) -> f32 {
    game.eye_position()[0]
}

fn forward() -> Input {
    Input {
        forward: 1,
        ..Input::default()
    }
}

/// Walking into the crate pushes it down the corridor and lets the player
/// past where it originally stood.
#[test]
fn walking_into_a_pushable_moves_it_and_clears_the_corridor() {
    let mut game = game_with(0.0, false);
    assert!((push_offset_x(&game) - 0.0).abs() < f32::EPSILON);

    tick_n(&mut game, 180, &forward());

    let offset = push_offset_x(&game);
    assert!(
        offset > 16.0,
        "three seconds of walking into the crate only moved it {offset} units"
    );
    let x = player_x(&game);
    assert!(
        x > OBSTACLE_NEAR_X,
        "the player is still stuck at x = {x}, short of the crate's original near face"
    );
}

/// A crate with a static `func_wall` behind it stops against that wall: the
/// push is traced through the collision model, so the crate can never be
/// shoved through world geometry however long the player leans on it.
#[test]
fn a_pushable_stops_against_a_wall_behind_it() {
    let mut game = game_with(0.0, true);
    // Ten simulated seconds — far longer than the crate needs to cross the
    // whole gap to the wall at any plausible push speed.
    tick_n(&mut game, 600, &forward());

    let offset = push_offset_x(&game);
    assert!(offset > 16.0, "the crate never moved at all ({offset})");
    // The crate's far face may reach the wall's near face, and no further.
    let far_face = PUSHABLE_CRATE_MAXS[0] + offset;
    assert!(
        far_face <= PUSHABLE_BACK_WALL_X + 1.0,
        "the crate's far face reached x = {far_face}, past the wall at {PUSHABLE_BACK_WALL_X}"
    );
    // And the player never ends up inside the crate they are pushing.
    let x = player_x(&game);
    assert!(
        x < OBSTACLE_NEAR_X + offset,
        "the player at x = {x} is inside the crate whose near face is at {}",
        OBSTACLE_NEAR_X + offset
    );
}

/// Standing still (or walking away) never moves the crate: the push needs
/// the player to actually be moving into it.
#[test]
fn a_pushable_nobody_walks_into_never_moves() {
    let mut game = game_with(0.0, false);
    tick_n(&mut game, 180, &Input::default());
    assert!((push_offset_x(&game) - 0.0).abs() < f32::EPSILON);

    tick_n(
        &mut game,
        180,
        &Input {
            forward: -1,
            ..Input::default()
        },
    );
    assert!(
        (push_offset_x(&game) - 0.0).abs() < f32::EPSILON,
        "walking away from the crate must not push it"
    );
}

/// The documented `friction` resistance is honoured: a crate at the
/// documented `400` "most resistance" end does not budge, while the same
/// walk moves a `friction = 0` one.
#[test]
fn the_documented_friction_range_scales_the_push() {
    let mut free = game_with(0.0, false);
    tick_n(&mut free, 120, &forward());
    let free_offset = push_offset_x(&free);

    let mut stuck = game_with(400.0, false);
    tick_n(&mut stuck, 120, &forward());
    let stuck_offset = push_offset_x(&stuck);

    assert!(free_offset > 16.0, "the frictionless crate barely moved");
    assert!(
        (stuck_offset - 0.0).abs() < f32::EPSILON,
        "a friction = 400 crate must not move at all, moved {stuck_offset}"
    );
}

/// A pushed crate's position survives a save/load round trip (save tag 33).
#[test]
fn a_pushed_crate_keeps_its_position_across_a_save_and_load() {
    let mut game = game_with(0.0, false);
    tick_n(&mut game, 120, &forward());
    let pushed = push_offset_x(&game);
    assert!(pushed > 16.0);

    let bytes = game.save_bytes(1_700_000_000).expect("the save writes");
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PUSHABLE_MAP}.bsp"),
        obstacle_corridor_bsp(
            &pushable_corridor_entities(0.0, false),
            PUSHABLE_CRATE_MAXS,
            true,
            false,
        ),
    );
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let restored = push_offset_x(&reloaded);
    assert!(
        (restored - pushed).abs() < 0.001,
        "the crate was restored at {restored}, not where it was pushed to ({pushed})"
    );
}
