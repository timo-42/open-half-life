//! A `func_train`/`func_tracktrain` that is moving when the player leaves a
//! map keeps moving, from the same place, in the next one.
//!
//! The public documentation this project works from (see
//! `docs/FORMAT_SOURCES.md`, "Campaign flow") states that entities persist
//! across a transition when correlated by a shared `globalname`, and (see
//! "Track trains and paths") that a train's route is a chain of
//! `path_corner`/`path_track` nodes addressed by `targetname`. Put together:
//! the destination's copy of the node the train is *at* is the only thing
//! about a chain position that means anything on the other side of a level
//! change, so that is what travels — never a node index (which belongs to
//! the source map's chain) and never a world position.
//!
//! Without this, a ride that spans a level change stopped being a ride: the
//! destination map spawned its own copy of the train at its own `target`
//! node, thousands of units away, and drove off empty while the passenger
//! stood where the transition had put them down.
//!
//! The same fixture pins two neighbouring rules the ride depends on:
//! a `path_track`'s documented fire-on-pass `message`, and that the
//! player's arrival is measured from their own origin rather than from
//! their eye.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, SYNTHETIC_MAP, synthetic_map_bsp_with_entities,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::registry::{Door, MoverState};
use ohl_game::track_train::TrackTrainState;

/// The `globalname` both maps' trains share — the documented cross-level
/// correlation key. Project-authored, like every other name here.
const TRAIN_GLOBAL: &str = "ohl_global_tram";

/// The train's cruise/start speed, units per second.
const SPEED: f32 = 100.0;

/// The nodes both maps place at the same coordinates. The source map's
/// chain runs `a -> b -> c`; the destination map declares `b -> c -> d` and
/// its own train targets `c`, so a train handed over at `b` can only end up
/// in the right place by being re-seated on the destination's own `b`.
const NODE_A: [f32; 3] = [-400.0, 0.0, 0.0];
const NODE_B: [f32; 3] = [0.0, 0.0, 0.0];
const NODE_C: [f32; 3] = [400.0, 0.0, 0.0];
const NODE_D: [f32; 3] = [800.0, 0.0, 0.0];

fn node(name: &str, origin: [f32; 3], target: Option<&str>, message: Option<&str>) -> String {
    let target = target.map_or_else(String::new, |target| format!("\"target\" \"{target}\"\n"));
    let message = message.map_or_else(String::new, |message| {
        format!("\"message\" \"{message}\"\n")
    });
    format!(
        "{{\n\"classname\" \"path_track\"\n\"targetname\" \"{name}\"\n\
         \"origin\" \"{} {} {}\"\n{target}{message}}}\n",
        origin[0], origin[1], origin[2]
    )
}

fn train(first_node: &str, start_speed: f32) -> String {
    format!(
        "{{\n\"classname\" \"func_tracktrain\"\n\"targetname\" \"ohl_tram\"\n\
         \"globalname\" \"{TRAIN_GLOBAL}\"\n\"model\" \"*1\"\n\"origin\" \"0 0 -64\"\n\
         \"target\" \"{first_node}\"\n\"speed\" \"{SPEED}\"\n\
         \"startspeed\" \"{start_speed}\"\n\"height\" \"0\"\n}}\n"
    )
}

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

/// `destination_first_node` is the destination train's own `target`, which
/// is what decides whether the handover re-seats the train on the chain it
/// already built or has to rebuild one from the carried node's name.
fn assets(destination_first_node: &str) -> MemoryAssets {
    let source = common(NEXT_MAP)
        + &train("ohl_node_a", SPEED)
        + &node("ohl_node_a", NODE_A, Some("ohl_node_b"), None)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), None)
        + &node("ohl_node_c", NODE_C, None, None);
    let destination = common(SYNTHETIC_MAP)
        + &train(destination_first_node, SPEED)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), None)
        + &node("ohl_node_c", NODE_C, Some("ohl_node_d"), None)
        + &node("ohl_node_d", NODE_D, None, None);

    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&source),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&destination),
    );
    assets
}

/// The one train's path position, straight off its own runtime state.
fn train_position(game: &Game) -> glam::Vec3 {
    let mut found = None;
    for state in &mut game.registry().world.query::<&TrackTrainState>() {
        found = Some(state.position());
    }
    found.expect("the map declares exactly one train, and it resolved a chain")
}

fn tick_n(game: &mut Game, n: u32) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

/// Drives the source map until its train has left the first node and is
/// somewhere along the `a -> b` segment, then hands over.
fn ride_then_change(assets: &MemoryAssets) -> (Game, glam::Vec3) {
    let mut game = Game::load(assets, SYNTHETIC_MAP).expect("the source map loads");
    // Long enough to pass `b` and be partway along `b -> c`: the carried
    // node is then `b`, which the destination declares, and the carried
    // progress is a real fraction rather than zero.
    tick_n(&mut game, 600);
    let before = train_position(&game);
    assert!(
        before.x > NODE_B[0] && before.x < NODE_C[0],
        "the source train must be partway along its second segment, not {before:?}"
    );
    game.change_level(assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    (game, before)
}

/// The destination map's own train targets a node the carried train is not
/// on, so the handover has to rebuild the chain from the carried node's
/// name. Without the carry it would sit at its own first node instead.
#[test]
fn a_moving_train_arrives_where_it_left_and_keeps_going() {
    let assets = assets("ohl_node_c");
    let (mut game, before) = ride_then_change(&assets);

    let arrived = train_position(&game);
    assert!(
        (arrived - before).length() < 1.0,
        "the carried train must arrive where the source map's copy was \
         ({before:?}), not at {arrived:?}"
    );
    assert!(
        (arrived.x - NODE_C[0]).abs() > 1.0,
        "a train left at the destination's own first node would sit exactly \
         there, which is the bug this test pins"
    );

    tick_n(&mut game, 60);
    let later = train_position(&game);
    assert!(
        later.x > arrived.x + 1.0,
        "the carried train must still be moving forward along the chain: \
         {arrived:?} -> {later:?}"
    );
}

/// The other branch: the destination map's own train already builds a chain
/// containing the carried node, so it is re-seated on that chain rather
/// than given a rebuilt one. The observable outcome is the same.
#[test]
fn a_moving_train_is_reseated_on_a_chain_that_already_holds_its_node() {
    let assets = assets("ohl_node_b");
    let (mut game, before) = ride_then_change(&assets);

    let arrived = train_position(&game);
    assert!(
        (arrived - before).length() < 1.0,
        "the carried train must arrive where the source map's copy was \
         ({before:?}), not at {arrived:?}"
    );
    tick_n(&mut game, 60);
    assert!(
        train_position(&game).x > arrived.x + 1.0,
        "the re-seated train must still be moving"
    );
}

/// A train that is *stopped* when the player leaves stays stopped on the
/// other side: the carry moves the train's real state, not just "it moves".
#[test]
fn a_stopped_train_stays_stopped_across_the_change() {
    let mut assets = MemoryAssets::new();
    let source = common(NEXT_MAP)
        + &train("ohl_node_b", 0.0)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), None)
        + &node("ohl_node_c", NODE_C, None, None);
    let destination = common(SYNTHETIC_MAP)
        + &train("ohl_node_c", SPEED)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), None)
        + &node("ohl_node_c", NODE_C, Some("ohl_node_d"), None)
        + &node("ohl_node_d", NODE_D, None, None);
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&source),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&destination),
    );

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    tick_n(&mut game, 120);
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    let arrived = train_position(&game);
    tick_n(&mut game, 120);
    let later = train_position(&game);
    assert!(
        (later - arrived).length() < 0.01,
        "a train that was not moving must not be started by the handover: \
         {arrived:?} -> {later:?}"
    );
    assert!(
        (arrived.x - NODE_B[0]).abs() < 1.0,
        "and it must still be parked on the node it was parked on, not \
         {arrived:?}"
    );
}

/// The player's own arrival offset is measured from their origin, not their
/// eye. Both maps here put the landmark at the same coordinates, so a
/// correct transition lands the player exactly where they stood; measuring
/// the offset from the eye instead lifts them by the standing view offset
/// on every single level change, which is enough to drop a passenger out of
/// a moving car before they land.
#[test]
fn the_player_arrives_at_their_own_origin_offset_not_their_eye() {
    let assets = assets("ohl_node_c");
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    let before = game.player_origin();
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    let after = game.player_origin();

    assert!(
        (after[2] - before[2]).abs() < 0.01,
        "the two maps place the landmark identically, so the player's own \
         origin must not move at all: {before:?} -> {after:?}"
    );
}

/// A `path_track`'s documented fire-on-pass `message`: the entity it names
/// is fired as the train passes the node.
#[test]
fn a_path_node_fires_its_message_as_the_train_passes() {
    const DOOR: &str = "ohl_fired_door";
    let map = common(NEXT_MAP)
        + &train("ohl_node_a", SPEED)
        + &node("ohl_node_a", NODE_A, Some("ohl_node_b"), None)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), Some(DOOR))
        + &node("ohl_node_c", NODE_C, None, None)
        + &format!(
            "{{\n\"classname\" \"func_door\"\n\"targetname\" \"{DOOR}\"\n\
             \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"-1\"\n\"angle\" \"90\"\n\
             \"origin\" \"0 0 0\"\n}}\n"
        );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&map),
    );
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the map loads");

    let closed = |game: &Game| {
        game.registry()
            .world
            .query::<&Door>()
            .iter()
            .all(|door| door.state == MoverState::Closed)
    };

    // One second in, the train is still short of the node carrying the
    // `message`, so nothing has fired yet.
    tick_n(&mut game, 60);
    assert!(
        closed(&game),
        "the door must not open before the train reaches the node"
    );

    // Long enough to carry it well past that node.
    tick_n(&mut game, 540);
    assert!(
        !closed(&game),
        "passing a node with a fire-on-pass message must fire it"
    );
}

/// The handover leaves the collision model already re-baselined against
/// wherever it just put the carried train, so the first step after a level
/// change reads one step's worth of motion rather than the whole handover
/// displacement.
///
/// `Game::apply_transition` calls `Level::sync_brush_collision(0.0)` for
/// exactly this: a destination map attaches its brushes at their
/// *spawn-time* placement (its own train sits on its own first node),
/// which for a carried train is thousands of units from where the carry
/// just put it. Without the re-baseline the next step's own sync divides
/// that whole gap by one tick and synthesises a brush velocity in the tens
/// of thousands of units per second — enough to fling any rider standing
/// on it, or to shove aside anyone standing in the destination train's
/// hull. Nothing else in the suite noticed the line going missing, which
/// is what this pins.
#[test]
fn the_handover_re_baselines_the_collision_model_before_the_first_step() {
    let assets = assets("ohl_node_d");
    let (mut game, _) = ride_then_change(&assets);

    let (entity, brush) = train_brush(&game);
    let placed = train_placed_origin(&game, entity);
    let hull = game
        .collision()
        .expect("the destination map has collision")
        .brush_origin(brush);
    assert!(
        (hull - placed).length() < 1.0,
        "the carried train's hull was left at {hull:?} while the train itself is at {placed:?}"
    );

    // And the consequence the re-baseline exists for: one step of ride,
    // not one step containing the whole handover.
    let before = game
        .collision()
        .expect("the destination map has collision")
        .brush_origin(brush);
    tick_n(&mut game, 1);
    let after = game
        .collision()
        .expect("the destination map has collision")
        .brush_origin(brush);
    let speed = (after - before).length() / TICK_SECONDS;
    assert!(
        speed <= SPEED * 2.0,
        "the first step after the handover moved the train's hull at {speed} units/second, \
         far past its own {SPEED}-unit/second ride"
    );
}

/// A carried node name the destination map does not declare *at all*
/// leaves its own train exactly where that map spawned it, rather than
/// guessing a placement.
///
/// `transition::restore_track_train` has two ways to seat a carried train:
/// find the node in the chain the destination's train already built, or
/// rebuild a chain from the carried node's name. This pins the third case
/// — neither works — which is the one that must not move anything.
#[test]
fn a_carried_node_the_destination_does_not_declare_leaves_its_train_alone() {
    let source = common(NEXT_MAP)
        + &train("ohl_node_a", SPEED)
        + &node("ohl_node_a", NODE_A, Some("ohl_node_b"), None)
        + &node("ohl_node_b", NODE_B, Some("ohl_node_c"), None)
        + &node("ohl_node_c", NODE_C, None, None);
    // The destination shares the train's `globalname` (so the carry is
    // applied to it) but declares an entirely separate track whose nodes
    // are named nothing the source map ever rode.
    let destination = common(SYNTHETIC_MAP)
        + &train("ohl_far_node_1", SPEED)
        + &node("ohl_far_node_1", NODE_D, Some("ohl_far_node_2"), None)
        + &node("ohl_far_node_2", [NODE_D[0] + 400.0, 0.0, 0.0], None, None);

    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&source),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_entities(&destination),
    );

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    tick_n(&mut game, 600);
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");

    let arrived = train_position(&game);
    assert!(
        (arrived.x - NODE_D[0]).abs() < 1.0,
        "the destination train should have stayed on its own first node at x = {}, not {arrived:?}",
        NODE_D[0]
    );
}

/// The one attached track-train brush of the level the game is on.
fn train_brush(game: &Game) -> (ohl_game::hecs::Entity, ohl_physics::BrushId) {
    game.brush_collision()
        .iter()
        .copied()
        .find(|(entity, _)| {
            game.registry()
                .world
                .get::<&TrackTrainState>(*entity)
                .is_ok()
        })
        .expect("the map declares exactly one attached track train")
}

/// Where that train's compiled geometry currently sits: the same `origin`
/// keyvalue plus mover offset `Level::sync_brush_collision` hands
/// `ohl_physics::CollisionModel::set_brush_pose`.
fn train_placed_origin(game: &Game, entity: ohl_game::hecs::Entity) -> glam::Vec3 {
    let registry = game.registry();
    let authored = registry
        .world
        .get::<&ohl_game::registry::Transform>(entity)
        .expect("the train has a transform")
        .origin;
    authored + ohl_game::pose::brush_offset(registry, entity)
}
