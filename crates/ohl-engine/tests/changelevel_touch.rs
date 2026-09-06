//! `trigger_changelevel` fires from the player's own bounding box touching
//! its volume, not only from `use` — the same touch-trigger overlap test
//! `Simulation::touch_triggers` already applies to `trigger_once`/
//! `trigger_multiple` (see `crates/ohl-engine/tests/touch_trigger_door.rs`)
//! — unless the entity's "USE Only" spawnflag is set (TWHL wiki:
//! `trigger_changelevel`; see `docs/FORMAT_SOURCES.md`, "Entity keyvalues
//! and map logic").
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, TOUCH_CHANGELEVEL_MAP, door_behind_touch_trigger_bsp, synthetic_map_bsp,
    touch_changelevel_entities, touch_changelevel_use_only_entities,
};
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

fn assets(entities: &str) -> MemoryAssets {
    let bytes = door_behind_touch_trigger_bsp(entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{TOUCH_CHANGELEVEL_MAP}.bsp"), bytes);
    assets.insert(&format!("maps/{NEXT_MAP}.bsp"), synthetic_map_bsp());
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, TOUCH_CHANGELEVEL_MAP).expect("the synthetic map loads")
}

fn forward_walk(steps: usize) -> Vec<Input> {
    (0..steps)
        .map(|_| Input {
            forward: 1,
            ..Input::default()
        })
        .collect()
}

/// Walking into a plain (non-"USE Only") `trigger_changelevel` volume fires
/// exactly one `GameEvent::LevelChange`, even though the player's bounding
/// box keeps overlapping the volume for many more steps after that: this
/// reproduces the bug where the dedicated `trigger_changelevel` registry
/// arm never attached a touch-capable component at all, so only `use`
/// could ever reach it, and guards against the opposite failure mode (a
/// naive touch implementation re-firing every overlapping frame).
#[test]
fn walking_into_the_volume_fires_exactly_one_level_change() {
    let entities = touch_changelevel_entities(NEXT_MAP);
    let assets = assets(&entities);
    let mut game = game(&assets);

    let mut level_changes: Vec<GameEvent> = Vec::new();
    for input in forward_walk(300) {
        for event in game.tick(STEP, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                level_changes.push(event);
            }
        }
    }

    assert_eq!(
        level_changes.len(),
        1,
        "the volume must fire exactly once for one continuous crossing, \
         not once per overlapping frame"
    );
    let GameEvent::LevelChange { map, landmark } = &level_changes[0] else {
        unreachable!("filtered above");
    };
    assert_eq!(map, NEXT_MAP);
    assert_eq!(landmark, LANDMARK);
}

/// The event a touch fires drives an actual `Game::change_level` the same
/// way the host loop (`ohl-app`) would, landing on the destination map with
/// its landmark-relative position applied — the same transition mechanics
/// `crates/ohl-engine/tests/game_loop.rs` already verifies for a
/// `use`-fired `trigger_changelevel`.
#[test]
fn the_fired_event_can_drive_a_real_transition() {
    let entities = touch_changelevel_entities(NEXT_MAP);
    let assets = assets(&entities);
    let mut game = game(&assets);

    let mut change = None;
    for input in forward_walk(300) {
        for event in game.tick(STEP, &input) {
            if let GameEvent::LevelChange { map, landmark } = event {
                change = Some((map, landmark));
            }
        }
        if change.is_some() {
            break;
        }
    }
    let (map, landmark) = change.expect("walking into the volume must fire the level change");

    game.change_level(&assets, &map, &landmark)
        .expect("the destination map loads");
    assert_eq!(game.map(), NEXT_MAP);
}

/// A `trigger_changelevel` with the published "USE Only" spawnflag (`2`)
/// set never fires from touch, only from `use`/being targeted.
#[test]
fn use_only_ignores_touch() {
    let entities = touch_changelevel_use_only_entities(NEXT_MAP);
    let assets = assets(&entities);
    let mut game = game(&assets);

    let mut saw_change = false;
    for input in forward_walk(300) {
        for event in game.tick(STEP, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                saw_change = true;
            }
        }
    }
    assert!(!saw_change, "\"USE Only\" must never fire from touch");
}
