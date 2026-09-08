//! A `path_track` carrying the documented default "New Train Speed" of `0`
//! must not park the train that passes it.
//!
//! `0` is that keyvalue's own default and is documented to mean "no speed
//! change" (see `docs/FORMAT_SOURCES.md`, "Track trains and paths"). Read
//! literally instead, it assigned a speed of zero, and since nothing in the
//! published behaviour ever restores a speed a train no longer has, the
//! train sat on that node forever — still "moving", covering no distance.
//! A ride whose whole job is to carry its passenger somewhere then stopped
//! short, with no input the player could give to recover.
//!
//! The fixture is the smallest shape that reproduces that: a rider standing
//! on a track train, a zero-`speed` node partway along its path, and a
//! `trigger_changelevel` volume beyond that node. No bytes here come from
//! any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, ZERO_SPEED_END_X, ZERO_SPEED_NODE_MAP, ZERO_SPEED_NODE_X,
    ZERO_SPEED_TRIGGER_MIN_X, synthetic_map_bsp, zero_speed_node_train_bsp,
};
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

/// Long enough for the fixture's train to cover the whole path at its own
/// speed several times over, so a failure means "never arrives", not
/// "needed longer".
const STEPS: usize = 60 * 60;

fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ZERO_SPEED_NODE_MAP}.bsp"),
        zero_speed_node_train_bsp(),
    );
    assets.insert(&format!("maps/{NEXT_MAP}.bsp"), synthetic_map_bsp());
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, ZERO_SPEED_NODE_MAP).expect("the synthetic map loads")
}

/// The regression itself: standing still on the train, the ride carries the
/// player past the zero-`speed` node and into the level-exit volume.
#[test]
fn a_zero_speed_node_does_not_strand_the_rider_before_the_level_exit() {
    let assets = assets();
    let mut game = game(&assets);

    let mut change = None;
    for _ in 0..STEPS {
        for event in game.tick(STEP, &Input::default()) {
            if let GameEvent::LevelChange { map, landmark } = event {
                change = Some((map, landmark));
            }
        }
        if change.is_some() {
            break;
        }
    }

    let (map, landmark) = change
        .expect("the ride must carry the player past the zero-speed node and into the exit volume");
    assert_eq!(map, NEXT_MAP);
    assert_eq!(landmark, LANDMARK);
}

/// The mechanism behind that outcome, asserted directly on the rider's own
/// position: the player travels well past the zero-`speed` node's own
/// distance along the path, rather than stopping on it.
#[test]
fn the_rider_travels_past_the_zero_speed_node() {
    let assets = assets();
    let mut game = game(&assets);

    let start = game.eye_position()[0];
    let mut furthest = start;
    for _ in 0..STEPS {
        game.tick(STEP, &Input::default());
        furthest = furthest.max(game.eye_position()[0]);
    }

    assert!(
        furthest > ZERO_SPEED_TRIGGER_MIN_X,
        "the rider stopped at {furthest}, short of the exit volume at {ZERO_SPEED_TRIGGER_MIN_X}; \
         the zero-speed node at {ZERO_SPEED_NODE_X} must not have parked the train"
    );
    assert!(
        furthest <= ZERO_SPEED_END_X + 1.0,
        "the rider ran past the path's own last node at {ZERO_SPEED_END_X}"
    );
}
