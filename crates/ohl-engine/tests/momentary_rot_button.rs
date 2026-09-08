//! A `momentary_rot_button` walked up to and driven by held `use`, through
//! the real `Game` loop — the synthetic-fixture counterpart of the M9
//! chapter-walk `combat-smoke` scenarios (`xtask/src/combat_smoke.rs`), not
//! a real-payload scenario there.
//!
//! `docs/FORMAT_SOURCES.md` item 27's own `TODO(black-box)` records a
//! bounded, aggregate-only probe of the real payload (parsing each cited-
//! table map's entities lump directly; booleans/counts only, nothing
//! media-derived committed, the probe script itself not committed, matching
//! that item's own already-established precedent) that found no
//! `momentary_rot_button` within a fixed radius of an `info_player_start`
//! on any of the 93 maps `ohl_campaign::CHAPTERS`/`HAZARD_COURSE_MAPS`
//! cite. With no reachable real-map instance to script a `combat-smoke`
//! scenario against, this synthetic fixture stands in for one instead: a
//! `forward`+`use_held` walk from a spawn point well outside
//! `ohl_engine::USE_RADIUS` of the valve, driven entirely through
//! `ohl_engine::Game::tick`'s real input path
//! (`ohl_game::logic::find_momentary_rot_button_within`/
//! `Simulation::drive_momentary_rot_button`, wired into
//! `ohl_engine::Systems::triggers_and_movers`, phase 12), proving the same
//! "walk to it, then hold use" technique a real-map scenario would exercise
//! actually turns the valve — not the crate-internal
//! `Simulation::use_entity`/forced-state unit tests
//! `crates/ohl-game/src/logic.rs` already has
//! (`momentary_rot_button_turns_while_held_and_flips_at_endpoints`/
//! `momentary_rot_button_auto_return_animates_back_to_zero_once_released`).
//!
//! No new fixed milestone line or `combat-smoke` scenario was added: that
//! part of the task only applies once a real map has one within reach; see
//! this file's own header above for why none does today.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    MOMENTARY_ROT_BUTTON_MAP, MOMENTARY_ROT_BUTTON_WALK_NAME, momentary_rot_button_entities,
    rot_button_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::MomentaryRotButton;

const STEP: f32 = 1.0 / 60.0;

fn game() -> Game {
    let bytes = rot_button_bsp(&momentary_rot_button_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MOMENTARY_ROT_BUTTON_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, MOMENTARY_ROT_BUTTON_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn valve_fraction(game: &Game) -> f32 {
    let registry = game.registry();
    let entity = *registry
        .find(MOMENTARY_ROT_BUTTON_WALK_NAME)
        .first()
        .expect("the fixture declares one named momentary_rot_button");
    registry
        .world
        .get::<&MomentaryRotButton>(entity)
        .expect("the named entity is a momentary_rot_button")
        .fraction
}

/// Idling at the spawn point (never walking into reach, never holding
/// `use`) leaves the valve at rest.
#[test]
fn idling_at_spawn_never_turns_the_valve() {
    let mut game = game();
    tick_n(&mut game, 60, &Input::default());
    assert!((valve_fraction(&game) - 0.0).abs() < f32::EPSILON);
}

/// Walking forward (with `use` held the whole time, exactly as a player
/// who wants to start turning a valve the moment they are close enough
/// would) closes the ~200-unit gap to the valve and then turns it: the
/// fraction moves off `0.0` only once the walk has actually brought the
/// player within `ohl_engine::USE_RADIUS`, and the whole thing runs
/// through the real `Game` loop with no forced component state.
#[test]
fn walking_up_and_holding_use_turns_the_momentary_rot_button() {
    let mut game = game();
    assert!((valve_fraction(&game) - 0.0).abs() < f32::EPSILON);

    let walk_and_hold = Input {
        forward: 1,
        use_held: true,
        ..Input::default()
    };
    // Three simulated seconds is comfortably enough to close a ~200-unit
    // gap at any plausible walking speed and then turn the valve for a
    // while once in reach.
    tick_n(&mut game, 180, &walk_and_hold);

    let fraction = valve_fraction(&game);
    assert!(
        fraction > 0.0,
        "walking up and holding use must have turned the valve off its 0.0 rest position, \
         got {fraction}"
    );
}
