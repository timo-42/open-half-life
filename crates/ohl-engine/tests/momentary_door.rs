//! A `momentary_rot_button` held through the real `use_held` input path —
//! `ohl_game::logic::find_momentary_rot_button_within` locates it by
//! proximity every tick `use` is held, exactly the same
//! [`ohl_engine::Game::tick`] path a player's own held "use" key drives —
//! pushes its `0.0..=1.0` fraction onto a `momentary_door` sharing its
//! `target` keyvalue, opening it; releasing `use` (with the fixture's
//! "Auto return" spawnflag set) drives the button, and so the door, back
//! toward `fraction = 0.0`.
//!
//! `crates/ohl-game/src/logic.rs`'s own `drive_momentary_rot_button` unit
//! tests already prove the underlying `MomentaryRotButton` state machine in
//! isolation (`Simulation::drive_momentary_rot_button`, called directly
//! with a forced `held_entity`, not by proximity). This integration test
//! additionally drives the button through the real `use_held` proximity
//! path (`ohl_game::logic::find_momentary_rot_button_within` ->
//! `ohl_game::pose::brush_center`), the same real path
//! `crates/ohl-engine/tests/rot_button.rs` already drives `func_rot_button`
//! through, and proves the new `momentary_door` link end to end: render/
//! collision/use-proximity pose (`ohl_game::pose::momentary_door_offset`,
//! chained into `ohl_game::pose::brush_offset`) all read the same
//! `MomentaryDoor::fraction` the button pushes.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

// The idle-fraction assertions below compare against an exact `0.0`: a
// freshly spawned `momentary_door` either starts at exactly that resting
// value or it does not, matching the same precedent
// `crates/ohl-engine/tests/save_format_frozen.rs` and `camera_sequences.rs`
// already set for this crate's own mover-position assertions.
#![allow(clippy::float_cmp)]

use ohl_engine::test_support::{
    MOMENTARY_DOOR_MAP, MOMENTARY_DOOR_NAME, momentary_door_bsp, momentary_door_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::MomentaryDoor;

const STEP: f32 = 1.0 / 60.0;

fn game() -> Game {
    let bytes = momentary_door_bsp(&momentary_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MOMENTARY_DOOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, MOMENTARY_DOOR_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn door_fraction(game: &Game) -> f32 {
    let registry = game.registry();
    let entity = *registry
        .find(MOMENTARY_DOOR_NAME)
        .first()
        .expect("the fixture declares one named momentary_door");
    registry
        .world
        .get::<&MomentaryDoor>(entity)
        .expect("the named entity is a momentary_door")
        .fraction
}

/// Idling near the button without holding "use" leaves the door untouched.
#[test]
fn idling_near_the_button_does_nothing() {
    let mut game = game();
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(door_fraction(&game), 0.0);
}

/// Holding `use` in reach — the same `ohl_engine::USE_RADIUS` proximity
/// search a player's own held "use" key drives through
/// `ohl_engine::Systems::triggers_and_movers` — turns the button (found via
/// `ohl_game::logic::find_momentary_rot_button_within`, not forced state)
/// and drives its target `momentary_door` open in step; releasing `use`
/// then drives both back toward `fraction = 0.0` (the fixture's "Auto
/// return" spawnflag).
#[test]
fn holding_use_opens_the_door_and_releasing_closes_it() {
    let mut game = game();
    assert_eq!(door_fraction(&game), 0.0);

    let held = Input {
        use_held: true,
        ..Input::default()
    };
    // The button's own 90-degree sweep at 180 degrees/second, matched by
    // the door's own `speed = 128` / `travel_distance = 64` ratio, finishes
    // in half a second — 30 ticks at 60 Hz.
    tick_n(&mut game, 30, &held);
    let opened = door_fraction(&game);
    assert!(
        (opened - 1.0).abs() < 1e-3,
        "holding use for the button's full sweep must fully open its target door, got {opened}"
    );

    // Releasing `use` lets the "Auto return" button (and so the door
    // tracking it) animate back toward `fraction = 0.0`.
    tick_n(&mut game, 30, &Input::default());
    let closed = door_fraction(&game);
    assert!(
        closed < 1e-3,
        "releasing use must let the auto-returning button close its target door again, got {closed}"
    );
}

/// A partial hold pushes the door partway open, proving the door tracks the
/// button's own commanded fraction continuously rather than only snapping
/// to `0.0`/`1.0`.
#[test]
fn a_partial_hold_opens_the_door_partway() {
    let mut game = game();
    let held = Input {
        use_held: true,
        ..Input::default()
    };
    // A quarter of the button's full 30-tick sweep.
    tick_n(&mut game, 7, &held);
    let fraction = door_fraction(&game);
    assert!(
        fraction > 0.0 && fraction < 0.5,
        "a short hold must leave the door partway open, got {fraction}"
    );
}
