//! A `func_train`/`func_tracktrain` is placed on the first node of its
//! path at spawn, not left wherever its brushes were compiled.
//!
//! The public documentation describes a train as riding its path with its
//! *origin brush* on it — `height` is documented as the offset "above the
//! path_track that the train will ride, based on the location of the
//! train's origin brush" (`docs/FORMAT_SOURCES.md`, "Track trains and
//! paths"). The compiler writes that origin brush's position into the
//! entity's `origin` keyvalue and stores the submodel's geometry relative
//! to it, so a map is free to build the train's brushes anywhere and let
//! the first `path_track` place them at spawn — and a map that authors the
//! player's own start inside the train relies on exactly that having
//! happened before the very first tick.
//!
//! Before this, `ohl-engine`'s `track_train_transform` measured the train's
//! travel from its first path node rather than from its `origin` keyvalue,
//! which made the spawn offset identically zero and froze every train at
//! its compiled position: a player the map put inside one had nothing
//! under them and fell.
//!
//! The companion case is a train authored *without* an origin brush: a
//! `0 0 0` `origin` keyvalue and world-baked vertices. Nothing cancels for
//! one of those, so measuring its travel from `origin` displaces the whole
//! car by the full magnitude of its own path coordinates the instant the
//! level loads — thousands of units on a real map — and drops any player
//! the map spawned inside it. Such a map places its first path node inside
//! the car it drew, which is the only reference point the geometry gives,
//! so the offset is measured from there instead and the car stays put at
//! spawn. See `ohl_game::pose::track_train_transform`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};

const MAP: &str = "ohltrainplacement";
const STEP: f32 = 1.0 / 100.0;

/// Half-extent of the train car's floor slab on X and Y, and the slab's
/// top/bottom relative to the train's own origin brush. Project-authored.
const CAR_HALF: f32 = 64.0;
const CAR_TOP_Z: f32 = 4.0;
const CAR_BOTTOM_Z: f32 = -12.0;

/// Where the map *compiled* the train's brushes: far from its own track,
/// on every axis. Only the `origin` keyvalue ties the geometry to the
/// world, exactly as a compiler with an origin brush leaves it.
const AUTHORED: [f32; 3] = [-3000.0, 2500.0, -1800.0];

/// The train's first `path_track`, and the second one it rides to.
const NODE_ONE: [f32; 3] = [0.0, 0.0, 0.0];
const SEGMENT_LENGTH: f32 = 400.0;

/// The train's `speed`/`startspeed`, units/second.
const TRAIN_SPEED: f32 = 100.0;

/// How far above the car's floor the map authors the player's start: a
/// short drop, the way a real map stands a player inside a vehicle.
const PLAYER_DROP: f32 = 20.0;

/// A void world (submodel `*0`, no collision anywhere) plus one solid box
/// (submodel `*1`) whose geometry is stored relative to a
/// `func_tracktrain`'s origin brush, with that origin brush authored at
/// [`AUTHORED`] and the train's own two-node `path_track` chain running
/// along `+X` from the world origin. The `info_player_start` stands just
/// above the car's floor at the *first node*, which is where the train
/// belongs at spawn — and nowhere near where its brushes were compiled.
fn train_placement_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{px} {py} {pz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"target\" \"ohl_place1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"{ax} {ay} {az}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_place1\"\n\
         \"target\" \"ohl_place2\"\n\"origin\" \"{n1x} {n1y} {n1z}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_place2\"\n\
         \"origin\" \"{n2x} {n1y} {n1z}\"\n}}\n",
        px = NODE_ONE[0],
        py = NODE_ONE[1],
        pz = NODE_ONE[2] + CAR_TOP_Z + 36.0 + PLAYER_DROP,
        speed = TRAIN_SPEED,
        ax = AUTHORED[0],
        ay = AUTHORED[1],
        az = AUTHORED[2],
        n1x = NODE_ONE[0],
        n1y = NODE_ONE[1],
        n1z = NODE_ONE[2],
        n2x = NODE_ONE[0] + SEGMENT_LENGTH,
    ));

    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-8192.0; 3],
        [8192.0; 3],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the car's floor slab, in the train's own origin-brush
    // frame — the frame a compiler leaves a brush entity's geometry in.
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [-CAR_HALF, -CAR_HALF, CAR_BOTTOM_Z],
        [CAR_HALF, CAR_HALF, CAR_TOP_Z],
    )]);
    b.push_model(
        [-CAR_HALF, -CAR_HALF, CAR_BOTTOM_Z],
        [CAR_HALF, CAR_HALF, CAR_TOP_Z],
        [0.0, 0.0, 0.0],
        car_heads,
        2,
        0,
        0,
    );
    b.build()
}

fn settle(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for _ in 0..steps {
        game.tick(STEP, &Input::default());
    }
}

fn loaded_game(assets: &MemoryAssets) -> Game {
    Game::load(assets as &dyn AssetSource, MAP).expect("the synthetic map loads")
}

#[test]
fn a_player_spawned_inside_a_train_lands_on_its_floor_instead_of_falling() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), train_placement_bsp());
    let mut game = loaded_game(&assets);

    // The world model is a void: the only thing in this map that can stop
    // the fall is the train's own brush, and only if it was placed on its
    // first node rather than left at `AUTHORED`.
    settle(&mut game, 0.5);
    let eye = game.eye_position();
    let expected = NODE_ONE[2] + CAR_TOP_Z + 36.0 + 28.0;
    assert!(
        (eye[2] - expected).abs() < 4.0,
        "the player did not come to rest on the train's floor: {eye:?}, expected an eye height near {expected}"
    );
    assert!(
        !game.eye_is_in_solid(),
        "the player came to rest embedded in the train"
    );
}

#[test]
fn the_train_carries_the_player_it_was_spawned_inside() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), train_placement_bsp());
    let mut game = loaded_game(&assets);

    settle(&mut game, 0.5);
    let start = game.eye_position();
    assert!(
        game.ground_mover_speed() > 0.0,
        "the player is not standing on the moving train"
    );

    settle(&mut game, SEGMENT_LENGTH / TRAIN_SPEED * 0.5);
    let end = game.eye_position();
    let travelled = end[0] - start[0];
    assert!(
        travelled > SEGMENT_LENGTH * 0.25,
        "the player was not carried along with the train: moved {travelled} from {start:?} to {end:?}"
    );
    assert!(
        (end[2] - start[2]).abs() < 8.0,
        "the player's height above the train drifted: {start:?} -> {end:?}"
    );
}

/// Where a map with no origin brush *baked* the car's floor slab: absolute
/// world coordinates, far from the world origin. Project-authored.
const BAKED_CAR_CENTRE: [f32; 3] = [1200.0, -900.0, 300.0];

/// The same void world and the same car floor slab as
/// [`train_placement_bsp`], but with the car's geometry baked into
/// absolute world space and the entity's `origin` left at `0 0 0` — the
/// shape a compiler leaves a brush entity in when the mapper gave it no
/// origin brush. The train's first `path_track` sits at the car's own
/// centre, which is where such a map has to put it for the car to be on
/// its own track at all, and the second node is [`SEGMENT_LENGTH`] along
/// `+X`. The `info_player_start` stands just above the car's floor.
fn world_baked_train_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let mins = [
        BAKED_CAR_CENTRE[0] - CAR_HALF,
        BAKED_CAR_CENTRE[1] - CAR_HALF,
        BAKED_CAR_CENTRE[2] + CAR_BOTTOM_Z,
    ];
    let maxs = [
        BAKED_CAR_CENTRE[0] + CAR_HALF,
        BAKED_CAR_CENTRE[1] + CAR_HALF,
        BAKED_CAR_CENTRE[2] + CAR_TOP_Z,
    ];
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{px} {py} {pz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"target\" \"ohl_baked1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_baked1\"\n\
         \"target\" \"ohl_baked2\"\n\"origin\" \"{cx} {cy} {cz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_baked2\"\n\
         \"origin\" \"{nx} {cy} {cz}\"\n}}\n",
        px = BAKED_CAR_CENTRE[0],
        py = BAKED_CAR_CENTRE[1],
        pz = BAKED_CAR_CENTRE[2] + CAR_TOP_Z + 36.0 + PLAYER_DROP,
        speed = TRAIN_SPEED,
        cx = BAKED_CAR_CENTRE[0],
        cy = BAKED_CAR_CENTRE[1],
        cz = BAKED_CAR_CENTRE[2],
        nx = BAKED_CAR_CENTRE[0] + SEGMENT_LENGTH,
    ));

    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-8192.0; 3],
        [8192.0; 3],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(mins, maxs)]);
    b.push_model(mins, maxs, [0.0, 0.0, 0.0], car_heads, 2, 0, 0);
    b.build()
}

#[test]
fn a_world_baked_train_stays_where_it_was_compiled_at_spawn() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), world_baked_train_bsp());
    let mut game = loaded_game(&assets);

    // Again a void world: nothing but the car itself can stop the fall,
    // and only if it was left where its vertices were baked rather than
    // displaced by the full magnitude of its first node's coordinates.
    settle(&mut game, 0.5);
    let eye = game.eye_position();
    let expected = BAKED_CAR_CENTRE[2] + CAR_TOP_Z + 36.0 + 28.0;
    assert!(
        (eye[2] - expected).abs() < 4.0,
        "the player did not come to rest on the world-baked train's floor: {eye:?}, expected an eye height near {expected}"
    );
    assert!(
        !game.eye_is_in_solid(),
        "the player came to rest embedded in the train"
    );
}

#[test]
fn a_world_baked_train_still_carries_its_passenger_along_the_path() {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MAP}.bsp"), world_baked_train_bsp());
    let mut game = loaded_game(&assets);

    settle(&mut game, 0.5);
    let start = game.eye_position();
    assert!(
        game.ground_mover_speed() > 0.0,
        "the player is not standing on the moving world-baked train"
    );

    settle(&mut game, SEGMENT_LENGTH / TRAIN_SPEED * 0.5);
    let end = game.eye_position();
    let travelled = end[0] - start[0];
    assert!(
        travelled > SEGMENT_LENGTH * 0.25,
        "the player was not carried along with the world-baked train: moved {travelled} from {start:?} to {end:?}"
    );
    assert!(
        (end[2] - start[2]).abs() < 8.0,
        "the player's height above the world-baked train drifted: {start:?} -> {end:?}"
    );
}
