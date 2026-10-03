//! A `path_track` with a "Branch Path" is a railway switch: triggered, it
//! sends the next train that reaches it down its `altpath` instead of its
//! `target` (Sven Co-op Manor's and the Sven Co-op wiki's `path_track`
//! pages, fetched directly; `docs/FORMAT_SOURCES.md`, "Mover blocking,
//! branching paths and monster-opened doors").
//!
//! The fixture is `ohl_engine::test_support::rotating_door_bsp`'s corridor
//! with its solid submodel `*1` declared as a `func_tracktrain` rather
//! than a door, riding a three-node `path_track` fork: `start` at the
//! origin, `fork` 100 units along `+x`, and the fork's two ends 300 units
//! along `+y` (its `target`) and `-y` (its `altpath`). The switch is
//! thrown, when it is, by a `trigger_relay` a `trigger_auto` fires on the
//! first tick — a `triggerstate` of "on", the same use type the
//! `TrackTrainState` arm already honours — long before the train's own
//! `startspeed` carries it to the fork. Every keyvalue is a published one.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{ROTATING_DOOR_MAP, ROTATING_DOOR_PIVOT, rotating_door_bsp};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::TrackTrainState;
use ohl_game::registry::Path;

fn entities(throw_the_switch: bool) -> String {
    let switch = if throw_the_switch {
        "{\n\"classname\" \"trigger_auto\"\n\"target\" \"ohl_switch\"\n}\n\
         {\n\"classname\" \"trigger_relay\"\n\"targetname\" \"ohl_switch\"\n\
         \"target\" \"ohl_fork\"\n\"triggerstate\" \"1\"\n}\n"
    } else {
        ""
    };
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-600 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"targetname\" \"ohl_tram\"\n\
         \"model\" \"*1\"\n\"target\" \"ohl_start\"\n\"speed\" \"100\"\n\
         \"startspeed\" \"100\"\n\"height\" \"0\"\n\
         \"origin\" \"{} {} {}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_start\"\n\
         \"target\" \"ohl_fork\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_fork\"\n\
         \"target\" \"ohl_main_end\"\n\"altpath\" \"ohl_branch_end\"\n\
         \"origin\" \"100 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_main_end\"\n\
         \"origin\" \"100 300 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_branch_end\"\n\
         \"origin\" \"100 -300 0\"\n}}\n{switch}",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

fn assets(throw_the_switch: bool) -> MemoryAssets {
    let bytes = rotating_door_bsp(&entities(throw_the_switch));
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    assets
}

fn game(throw_the_switch: bool) -> Game {
    Game::load(
        &assets(throw_the_switch) as &dyn AssetSource,
        ROTATING_DOOR_MAP,
    )
    .expect("the fixture loads")
}

fn train_position(game: &Game) -> glam::Vec3 {
    let registry = game.registry();
    let tram = registry.find("ohl_tram")[0];
    registry
        .world
        .get::<&TrackTrainState>(tram)
        .expect("the train has a chain")
        .position()
}

fn fork_is_thrown(game: &Game) -> bool {
    let registry = game.registry();
    let fork = registry.find("ohl_fork")[0];
    registry
        .world
        .get::<&Path>(fork)
        .expect("a path node")
        .branch_active
}

/// 100 units to the fork and 150 more past it, at 100 units/second: well
/// clear of the fork on whichever side it sent the train.
fn ride(game: &mut Game) {
    ride_for(game, 250);
}

fn ride_for(game: &mut Game, ticks: u32) {
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

/// The control: nothing throws the switch, and the train follows the
/// fork's `target`.
#[test]
fn an_untriggered_fork_sends_the_train_down_its_target() {
    let mut game = game(false);
    assert!(!fork_is_thrown(&game));
    ride(&mut game);
    assert!(!fork_is_thrown(&game));
    let position = train_position(&game);
    assert!(
        position.y > 100.0,
        "the train should be on the main line (+y): {position:?}"
    );
}

#[test]
fn a_fork_triggered_on_sends_the_next_train_down_its_branch() {
    let mut game = game(true);
    ride(&mut game);
    assert!(fork_is_thrown(&game), "the relay's \"on\" threw the switch");
    let position = train_position(&game);
    assert!(
        position.y < -100.0,
        "the train should be on the branch (-y): {position:?}"
    );
}

// --- Saving a thrown switch and the chain a train holds (tag 40) ----------

/// Saves `game` and loads the save back over the same map.
fn save_and_load(game: &Game, assets: &MemoryAssets) -> Game {
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    Game::load_bytes(assets, &bytes).expect("the save loads")
}

/// A train saved out on the branch comes back out on the branch, at the
/// same place, and carries on along it — not at the same node index and
/// progress on the main line, which is where a chain rebuilt from its own
/// `target` against an unthrown switch would put it.
#[test]
fn a_train_saved_on_the_branch_loads_on_the_branch() {
    let assets = assets(true);
    let mut game = Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("loads");
    ride_for(&mut game, 200);
    let before = train_position(&game);
    assert!(before.y < -50.0, "out on the branch: {before:?}");

    let mut reloaded = save_and_load(&game, &assets);
    assert!(fork_is_thrown(&reloaded), "the switch is still thrown");
    let after = train_position(&reloaded);
    assert!(
        after.abs_diff_eq(before, 1e-3),
        "the train is where it was saved: {before:?} -> {after:?}"
    );
    ride_for(&mut game, 50);
    ride_for(&mut reloaded, 50);
    assert!(
        train_position(&reloaded).abs_diff_eq(train_position(&game), 1e-3),
        "and carries on exactly as the unsaved game does"
    );
}

/// A train saved *approaching* a thrown switch takes the branch after the
/// load, as it would have without one.
#[test]
fn a_train_saved_approaching_a_thrown_switch_takes_the_branch_after_load() {
    let assets = assets(true);
    let mut game = Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("loads");
    ride_for(&mut game, 30);
    assert!(fork_is_thrown(&game));
    assert!(train_position(&game).x < 90.0, "still short of the fork");

    let mut reloaded = save_and_load(&game, &assets);
    ride(&mut reloaded);
    let position = train_position(&reloaded);
    assert!(
        position.y < -100.0,
        "the reloaded train takes the branch: {position:?}"
    );
}

/// A save written before tag 40 existed — the section simply absent —
/// still loads, with every switch back at its spawn position and the train
/// on the chain its `target` resolves to.
#[test]
fn a_save_from_before_section_40_existed_still_loads() {
    let assets = assets(true);
    let mut game = Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("loads");
    ride_for(&mut game, 30);
    let mut save = game.to_save(1_700_000_000);
    assert!(save.path_states.is_some(), "this build writes tag 40");
    save.path_states = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing tag 40 still encodes");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-40 save still loads");
    assert!(
        !fork_is_thrown(&reloaded),
        "without tag 40 the switch is back at its spawn position"
    );
    let registry = reloaded.registry();
    let tram = registry.find("ohl_tram")[0];
    let main_end = registry.find("ohl_main_end")[0];
    let state = registry
        .world
        .get::<&TrackTrainState>(tram)
        .expect("the train has a chain");
    assert_eq!(
        state.chain().nodes.last().map(|node| node.entity),
        Some(main_end)
    );
}
