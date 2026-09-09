//! Two rules a scripted ride across a level change depends on, each pinned
//! against a project-authored synthetic fixture.
//!
//! 1. **`trigger_auto`'s documented `triggerstate`.** The published pages
//!    (see `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic")
//!    describe the key as choosing the use *type* a trigger sends — "On-
//!    turns entity on; Off- Turns entity off; Toggle- turns entity On when
//!    it's Off and vice versa" — and a map that starts its own ride at
//!    level load declares it as On. Read as a plain toggle instead, that
//!    start-of-map trigger *stops* a train that arrived still moving, and
//!    nothing else in such a map ever starts it again.
//!
//! 2. **A carried train's ride survives a chain it cannot be named onto.**
//!    A `func_tracktrain` correlated by `globalname` carries the *name* of
//!    the `path_track` it is at, because a node index belongs to the source
//!    map's chain. When the destination declares no node of that name there
//!    is nothing to correlate a position with — but the train's own motion
//!    is not a position, and discarding it left the destination's copy
//!    rolling on its own `startspeed` as if the ride that arrived had never
//!    happened.
//!
//! 3. **A carried mover keeps its own map's compiled keyvalues.** State
//!    travels across a level change; move direction, travel distance,
//!    `speed` and `wait` do not. Two maps routinely give unrelated doors
//!    the same `targetname`, and one map's leaf sliding one way is nonsense
//!    applied to another map's leaf that slides another — it parks a solid
//!    hull across the space its own map compiled it to clear.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, SYNTHETIC_MAP, synthetic_map_bsp_with_entities,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::registry::{Door, MoverState, TargetName};
use ohl_game::track_train::TrackTrainState;

/// Everything both maps of a two-map fixture need: a room, a player start,
/// a landmark and a boundary out.
fn common(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_exit\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n}}\n"
    )
}

fn tick_n(game: &mut Game, n: u32) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

// --- 1. `trigger_auto`'s `triggerstate` -----------------------------------

/// The train's cruise/start speed, units per second.
const TRAIN_SPEED: f32 = 100.0;

/// A one-map fixture: a `func_tracktrain` already rolling along a two-node
/// chain, plus a `trigger_auto` that fires it one second into the map with
/// the given `triggerstate` (omitted entirely when `None`).
fn train_map(trigger_state: Option<&str>) -> MemoryAssets {
    let state = trigger_state.map_or_else(String::new, |value| {
        format!("\"triggerstate\" \"{value}\"\n")
    });
    let text = common(NEXT_MAP)
        + &format!(
            "{{\n\"classname\" \"func_tracktrain\"\n\"targetname\" \"ohl_tram\"\n\
             \"model\" \"*1\"\n\"origin\" \"0 0 -64\"\n\"target\" \"ohl_node_a\"\n\
             \"speed\" \"{TRAIN_SPEED}\"\n\"startspeed\" \"{TRAIN_SPEED}\"\n\"height\" \"0\"\n}}\n\
             {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_node_a\"\n\
             \"origin\" \"0 0 0\"\n\"target\" \"ohl_node_b\"\n}}\n\
             {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_node_b\"\n\
             \"origin\" \"4000 0 0\"\n}}\n\
             {{\n\"classname\" \"trigger_auto\"\n\"origin\" \"0 0 32\"\n\
             \"target\" \"ohl_tram\"\n\"delay\" \"1\"\n\"spawnflags\" \"1\"\n{state}}}\n"
        );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&text),
    );
    assets
}

/// Whether the map's single train is still travelling.
fn train_is_moving(game: &Game) -> bool {
    let mut found = None;
    for state in &mut game.registry().world.query::<&TrackTrainState>() {
        found = Some(state.dynamic_state().4);
    }
    found.expect("the fixture declares exactly one train")
}

#[test]
fn a_trigger_auto_that_declares_on_does_not_stop_a_moving_train() {
    let assets = train_map(Some("1"));
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the fixture loads");
    assert!(train_is_moving(&game), "the train starts on its startspeed");
    // Comfortably past the auto trigger's one-second delay.
    tick_n(&mut game, 180);
    assert!(
        train_is_moving(&game),
        "a documented \"On\" use type must not stop a train that is already \
         going: that is the whole difference between it and a toggle"
    );
}

#[test]
fn a_trigger_auto_that_declares_off_stops_a_moving_train() {
    let assets = train_map(Some("0"));
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the fixture loads");
    tick_n(&mut game, 180);
    assert!(
        !train_is_moving(&game),
        "a documented \"Off\" use type stops the train"
    );
}

#[test]
fn a_trigger_auto_without_a_triggerstate_still_toggles() {
    for declared in [None, Some("2")] {
        let assets = train_map(declared);
        let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the fixture loads");
        tick_n(&mut game, 180);
        assert!(
            !train_is_moving(&game),
            "an absent or explicitly-Toggle triggerstate keeps this \
             project's original behaviour: {declared:?}"
        );
    }
}

// --- 2. A carried mover keeps the destination map's own keyvalues ---------

/// The `targetname` both maps' unrelated doors happen to share.
const SHARED_DOOR: &str = "ohl_shared_door";

/// A door on the fixture's own leaf submodel, opened at map load when
/// `auto_open` — the source map's copy — so that it is a *modified* mover
/// worth carrying across.
fn door_map(next_map: &str, angle: &str, speed: &str, auto_open: bool) -> Vec<u8> {
    let opener = if auto_open {
        format!(
            "{{\n\"classname\" \"trigger_auto\"\n\"origin\" \"0 0 32\"\n\
             \"target\" \"{SHARED_DOOR}\"\n\"delay\" \"0\"\n\"spawnflags\" \"1\"\n}}\n"
        )
    } else {
        String::new()
    };
    let text = common(next_map)
        + &format!(
            "{{\n\"classname\" \"func_door\"\n\"targetname\" \"{SHARED_DOOR}\"\n\
             \"model\" \"*1\"\n\"speed\" \"{speed}\"\n\"wait\" \"-1\"\n\
             \"angle\" \"{angle}\"\n\"lip\" \"0\"\n\"origin\" \"0 0 0\"\n}}\n"
        )
        + &opener;
    synthetic_map_bsp_with_entities(&text)
}

/// The one door named [`SHARED_DOOR`], as `(movedir, travel_distance,
/// speed, state)`.
fn shared_door(game: &Game) -> (glam::Vec3, f32, f32, MoverState) {
    let mut found = None;
    for (name, door) in &mut game.registry().world.query::<(&TargetName, &Door)>() {
        if name.0 == SHARED_DOOR {
            found = Some((door.movedir, door.travel_distance, door.speed, door.state));
        }
    }
    found.expect("both maps declare exactly one door under the shared name")
}

#[test]
fn a_carried_door_keeps_the_destination_maps_own_move_direction() {
    let mut assets = MemoryAssets::new();
    // The source map's leaf slides straight down at 100 units/second; the
    // destination's unrelated leaf of the same name slides along +X at 250.
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        door_map(NEXT_MAP, "-2", "100", true),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        door_map(SYNTHETIC_MAP, "0", "250", false),
    );

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    let (source_dir, source_travel, _, _) = shared_door(&game);
    assert_eq!(source_dir, -glam::Vec3::Z, "the source leaf slides down");
    // Long enough for the auto trigger to fire and the leaf to finish
    // opening, so a *modified* mover is what travels.
    tick_n(&mut game, 300);
    assert_eq!(
        shared_door(&game).3,
        MoverState::Open,
        "the source map's own door must be open before the change"
    );

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    let (dir, travel, speed, state) = shared_door(&game);
    assert_eq!(
        state,
        MoverState::Open,
        "the mover's *state* is what travels across a level change"
    );
    assert_eq!(
        dir,
        glam::Vec3::X,
        "the destination leaf must still slide the way its own map compiled \
         it, not the way the source map's same-named leaf did ({source_dir:?})"
    );
    assert!(
        (travel - source_travel).abs() > 1.0,
        "the destination leaf's travel distance is its own brush's, not the \
         source's ({travel} vs {source_travel})"
    );
    assert!(
        (speed - 250.0).abs() < f32::EPSILON,
        "and so are its own speed keyvalue: {speed}"
    );
}

/// A `func_door_rotating` on the fixture's leaf submodel, opened at map
/// load when `auto_open`.
fn rotating_door_map(next_map: &str, auto_open: bool) -> Vec<u8> {
    let opener = if auto_open {
        format!(
            "{{\n\"classname\" \"trigger_auto\"\n\"origin\" \"0 0 32\"\n\
             \"target\" \"{SHARED_DOOR}\"\n\"delay\" \"0\"\n\"spawnflags\" \"1\"\n}}\n"
        )
    } else {
        String::new()
    };
    let text = common(next_map)
        + &format!(
            "{{\n\"classname\" \"func_door_rotating\"\n\"targetname\" \"{SHARED_DOOR}\"\n\
             \"model\" \"*1\"\n\"speed\" \"90\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
             \"origin\" \"0 0 0\"\n}}\n"
        )
        + &opener;
    synthetic_map_bsp_with_entities(&text)
}

/// The one rotating door named [`SHARED_DOOR`], as `(rotation_axis,
/// travel_distance, state)`.
fn shared_rotating_door(game: &Game) -> (Option<glam::Vec3>, f32, MoverState) {
    let mut found = None;
    for (name, door) in &mut game.registry().world.query::<(&TargetName, &Door)>() {
        if name.0 == SHARED_DOOR {
            found = Some((door.rotation_axis, door.travel_distance, door.state));
        }
    }
    found.expect("both maps declare exactly one door under the shared name")
}

/// `Door::rotation_axis` is the one field of that component which is not
/// purely compiled: its axis comes from the spawnflags/`distance` pair, but
/// its *sign* is rewritten at every open so the leaf swings away from
/// whoever opened it (`ohl_game::logic`'s door arm, PR #110). Carrying only
/// `state`/`timer` would mirror a door the player left open onto the wrong
/// side of the destination's frame — the same "a mover's hull is parked
/// where its own map never put it" fault the rest of
/// `apply_onto_existing` exists to stop, reflected instead of translated.
#[test]
fn a_carried_rotating_door_keeps_the_swing_side_its_activator_chose() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        rotating_door_map(NEXT_MAP, true),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        rotating_door_map(SYNTHETIC_MAP, false),
    );

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    let entity = {
        let mut found = None;
        for (entity, name) in &mut game
            .registry()
            .world
            .query::<(ohl_game::hecs::Entity, &TargetName)>()
        {
            if name.0 == SHARED_DOOR {
                found = Some(entity);
            }
        }
        found.expect("the source map declares the door")
    };
    {
        let mut door = game
            .registry()
            .world
            .get::<&mut Door>(entity)
            .expect("the door carries a Door");
        // The spawnflags alone choose `+Z`; an activator on that side is
        // what flips it, and that runtime choice is what has to travel.
        assert_eq!(door.rotation_axis, Some(glam::Vec3::Z));
        door.rotation_axis = Some(-glam::Vec3::Z);
    }
    tick_n(&mut game, 300);
    let (source_axis, _, source_state) = shared_rotating_door(&game);
    assert_eq!(source_state, MoverState::Open);
    assert_eq!(source_axis, Some(-glam::Vec3::Z));

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    let (axis, _, state) = shared_rotating_door(&game);
    assert_eq!(state, MoverState::Open, "the open state still travels");
    assert_eq!(
        axis,
        Some(-glam::Vec3::Z),
        "the destination leaf must swing the side the activator chose, not \
         revert to its own spawnflag default"
    );
}

// --- 3. A carried train's ride outlives a chain it cannot be named onto --

/// Two maps whose trains share a `globalname` but whose `path_track`s share
/// no name at all: the destination's chain is named differently end to end,
/// so the carried node cannot be found there under either correlation.
const RIDE_GLOBAL: &str = "ohl_global_ride";

fn ride_map(next_map: &str, prefix: &str, start_speed: f32) -> Vec<u8> {
    let text = common(next_map)
        + &format!(
            "{{\n\"classname\" \"func_tracktrain\"\n\"targetname\" \"ohl_ride\"\n\
             \"globalname\" \"{RIDE_GLOBAL}\"\n\"model\" \"*1\"\n\"origin\" \"0 0 -64\"\n\
             \"target\" \"{prefix}_a\"\n\"speed\" \"{TRAIN_SPEED}\"\n\
             \"startspeed\" \"{start_speed}\"\n\"height\" \"0\"\n}}\n\
             {{\n\"classname\" \"path_track\"\n\"targetname\" \"{prefix}_a\"\n\
             \"origin\" \"0 0 0\"\n\"target\" \"{prefix}_b\"\n}}\n\
             {{\n\"classname\" \"path_track\"\n\"targetname\" \"{prefix}_b\"\n\
             \"origin\" \"4000 0 0\"\n}}\n"
        );
    synthetic_map_bsp_with_entities(&text)
}

/// The one train's `(moving, speed)`.
fn ride_state(game: &Game) -> (bool, f32) {
    let mut found = None;
    for state in &mut game.registry().world.query::<&TrackTrainState>() {
        let (_, _, _, speed, moving, _) = state.dynamic_state();
        found = Some((moving, speed));
    }
    found.expect("each map declares exactly one train")
}

#[test]
fn a_carried_rides_motion_survives_a_destination_chain_it_cannot_be_named_onto() {
    let mut assets = MemoryAssets::new();
    // The source train rolls; the destination's own copy would spawn
    // rolling too, on a chain whose node names the source never uses.
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        ride_map(NEXT_MAP, "ohl_src", TRAIN_SPEED),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        ride_map(SYNTHETIC_MAP, "ohl_dst", TRAIN_SPEED),
    );

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    assert_eq!(ride_state(&game), (true, TRAIN_SPEED));
    // Park the source train, the way a map that hands its ride over at a
    // node and lets the next map start it again would leave it.
    for state in &mut game.registry().world.query::<&mut TrackTrainState>() {
        state.turn_off();
    }
    tick_n(&mut game, 30);
    assert!(!ride_state(&game).0, "the source train is parked");

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    assert!(
        !ride_state(&game).0,
        "a parked ride must arrive parked even when the destination map \
         declares no node of the carried name — otherwise the destination's \
         own startspeed silently restarts a ride the player left standing"
    );
}
