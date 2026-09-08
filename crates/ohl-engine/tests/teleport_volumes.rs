//! `trigger_teleport` moves the player to the entity its `target` names.
//!
//! Published behaviour (`docs/FORMAT_SOURCES.md`, "Teleport volumes and
//! destinations"): the volume "will teleport the player to the origin of
//! the target entity that was provided to it in its list of properties,
//! when the player touches it"; its `target` is "the name of the
//! `info_teleport_destination` or any other entity to use as destination";
//! a destination's `angles` are "the angles at which the entity will be
//! facing upon teleportation"; and the "No Clients (2)" spawnflag means
//! "players cannot activate this entity".
//!
//! Before this existed, a `trigger_teleport` was spawned as an ordinary
//! `trigger_*` volume that fired its `target` on touch and nothing on the
//! far end of that fire did anything at all — so a map whose opening
//! sequence teleports the player out of a sealed start volume simply never
//! started, however long it was left running.
//!
//! Two further rules this file pins, both from the same section: a volume
//! fires on the *rising edge* of a touch, so a destination placed inside
//! the next volume of a scripted chain does not cascade the whole chain in
//! a handful of fixed steps; and a volume whose `master` names a
//! `multisource` does not work until that master is active.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::door_behind_touch_trigger_bsp;
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};

const MAP: &str = "ohlteleportsynth";
const STEP: f32 = 1.0 / 100.0;

/// The `targetname` this fixture's destinations use. Project-authored.
const DESTINATION: &str = "ohl_tp_dest";

/// Where the player starts: inside the fixture's submodel `*1` box, which
/// these entity sets claim as the teleport volume — the shape the map this
/// test was written for uses, where the player is already standing in the
/// volume at spawn and no input is needed to fire it.
const START: [f32; 3] = [200.0, 0.0, 36.0];

/// Where the destination stands, well away from [`START`] and still on the
/// fixture's floor.
const DESTINATION_ORIGIN: [f32; 3] = [-200.0, 128.0, 36.0];

/// The yaw the destination's `angles` name, in degrees.
const DESTINATION_YAW: f32 = 90.0;

/// A `worldspawn`, an `info_player_start` inside the teleport volume, a
/// `trigger_teleport` (submodel `*1`) carrying `spawnflags` and `delay`,
/// and one destination entity of classname `destination_class`.
fn entities(destination_class: &str, spawnflags: &str, delay: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{sx} {sy} {sz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*1\"\n\
         \"target\" \"{DESTINATION}\"\n\"spawnflags\" \"{spawnflags}\"\n\
         \"delay\" \"{delay}\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"{destination_class}\"\n\
         \"targetname\" \"{DESTINATION}\"\n\
         \"origin\" \"{dx} {dy} {dz}\"\n\"angles\" \"0 {DESTINATION_YAW} 0\"\n}}\n",
        sx = START[0],
        sy = START[1],
        sz = START[2],
        dx = DESTINATION_ORIGIN[0],
        dy = DESTINATION_ORIGIN[1],
        dz = DESTINATION_ORIGIN[2],
    )
}

fn game(entities: &str) -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{MAP}.bsp"),
        door_behind_touch_trigger_bsp(entities),
    );
    Game::load(&assets as &dyn AssetSource, MAP).expect("the synthetic map loads")
}

fn run(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for _ in 0..steps {
        game.tick(STEP, &Input::default());
    }
}

fn horizontal_distance_to_destination(game: &Game) -> f32 {
    let origin = game.player_origin();
    let dx = origin[0] - DESTINATION_ORIGIN[0];
    let dy = origin[1] - DESTINATION_ORIGIN[1];
    dx.hypot(dy)
}

#[test]
fn a_player_standing_in_a_teleport_volume_is_moved_to_its_destination() {
    let mut game = game(&entities("info_teleport_destination", "0", "0"));
    assert_eq!(game.teleport_count(), 0, "nothing has fired before a tick");

    run(&mut game, 0.1);

    assert_eq!(
        game.teleport_count(),
        1,
        "the volume the player spawned inside should have moved them exactly once"
    );
    assert!(
        horizontal_distance_to_destination(&game) < 1.0,
        "the player was not placed at the destination: {:?}",
        game.player_origin()
    );
    assert!(
        (game.camera().yaw - DESTINATION_YAW).abs() < 0.01,
        "the player did not adopt the destination's facing: {}",
        game.camera().yaw
    );
    assert!(
        !game.eye_is_in_solid(),
        "the teleported player came to rest inside solid geometry"
    );
}

#[test]
fn the_no_clients_spawnflag_keeps_the_volume_from_moving_the_player() {
    // Spawnflag 2, per `ohl_game::registry::SPAWNFLAG_TELEPORT_NO_CLIENTS`.
    let mut game = game(&entities("info_teleport_destination", "2", "0"));
    run(&mut game, 1.0);

    assert_eq!(
        game.teleport_count(),
        0,
        "a \"No Clients\" volume must never move the player"
    );
    assert!(
        horizontal_distance_to_destination(&game) > 100.0,
        "the player left the volume they were told not to be moved by: {:?}",
        game.player_origin()
    );
}

#[test]
fn any_named_entity_can_be_the_destination() {
    // "the name of the info_teleport_destination or any other entity to
    // use as destination".
    let mut game = game(&entities("info_target", "0", "0"));
    run(&mut game, 0.1);

    assert_eq!(
        game.teleport_count(),
        1,
        "a destination of another classname must still move the player"
    );
    assert!(
        horizontal_distance_to_destination(&game) < 1.0,
        "the player was not placed at the destination: {:?}",
        game.player_origin()
    );
}

#[test]
fn the_volumes_own_delay_is_honoured() {
    let mut game = game(&entities("info_teleport_destination", "0", "0.5"));
    run(&mut game, 0.2);
    assert_eq!(
        game.teleport_count(),
        0,
        "the volume's `delay` had not elapsed yet"
    );

    run(&mut game, 0.5);
    assert_eq!(
        game.teleport_count(),
        1,
        "the volume's `delay` elapsed and the player should have been moved"
    );
}

#[test]
fn a_destination_fired_by_something_other_than_a_teleport_volume_moves_nobody() {
    // The same destination, activated by an ordinary `trigger_auto` chain
    // instead of by a teleport volume: a destination entity is only a
    // destination *for the volume that names it*, so nothing here should
    // move the player. (A `trigger_teleport` with "No Clients" set keeps
    // the fixture's own volume out of the picture.)
    let mut game = game(&format!(
        "{}{{\n\"classname\" \"trigger_auto\"\n\
         \"target\" \"{DESTINATION}\"\n\"origin\" \"0 0 0\"\n}}\n",
        entities("info_teleport_destination", "2", "0"),
    ));
    run(&mut game, 1.0);

    assert_eq!(
        game.teleport_count(),
        0,
        "an ordinary fire chain reaching a destination entity must not move the player"
    );
}

/// A second teleport volume, submodel `*2` of the shared fixture (the box
/// at `x` 64..128, `y` -48..48), whose own destination is somewhere else
/// again. The first volume's destination is placed *inside* it, which is
/// how a scripted chain of scenes is authored.
const CHAINED_DESTINATION: [f32; 3] = [96.0, 0.0, 36.0];

/// Where the second volume would send the player, if it fired.
const FAR_DESTINATION: [f32; 3] = [-200.0, -200.0, 36.0];

fn chained_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{sx} {sy} {sz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*1\"\n\
         \"target\" \"{DESTINATION}\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"info_teleport_destination\"\n\
         \"targetname\" \"{DESTINATION}\"\n\
         \"origin\" \"{cx} {cy} {cz}\"\n\"angles\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*2\"\n\
         \"target\" \"ohl_tp_far\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"info_teleport_destination\"\n\
         \"targetname\" \"ohl_tp_far\"\n\
         \"origin\" \"{fx} {fy} {fz}\"\n\"angles\" \"0 0 0\"\n}}\n",
        sx = START[0],
        sy = START[1],
        sz = START[2],
        cx = CHAINED_DESTINATION[0],
        cy = CHAINED_DESTINATION[1],
        cz = CHAINED_DESTINATION[2],
        fx = FAR_DESTINATION[0],
        fy = FAR_DESTINATION[1],
        fz = FAR_DESTINATION[2],
    )
}

#[test]
fn arriving_inside_the_next_volume_does_not_cascade_the_chain() {
    let mut game = game(&chained_entities());
    run(&mut game, 1.0);

    assert_eq!(
        game.teleport_count(),
        1,
        "arriving inside the next volume must not itself be a rising edge"
    );
    let origin = game.player_origin();
    let dx = origin[0] - CHAINED_DESTINATION[0];
    let dy = origin[1] - CHAINED_DESTINATION[1];
    assert!(
        dx.hypot(dy) < 1.0,
        "the player should have stopped at the first destination: {origin:?}"
    );
}

/// A `trigger_teleport` gated by a `master`, plus a `multisource` of that
/// name. `firing_class` is the classname of the one entity that targets
/// the master; passing `"info_target"` (which nothing ever activates)
/// leaves the master shut, while `"trigger_auto"` fires it on load.
fn mastered_entities(firing_class: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{sx} {sy} {sz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*1\"\n\
         \"target\" \"{DESTINATION}\"\n\"master\" \"ohl_tp_master\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"multisource\"\n\
         \"targetname\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"{firing_class}\"\n\
         \"target\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"info_teleport_destination\"\n\
         \"targetname\" \"{DESTINATION}\"\n\
         \"origin\" \"{dx} {dy} {dz}\"\n\"angles\" \"0 0 0\"\n}}\n",
        sx = START[0],
        sy = START[1],
        sz = START[2],
        dx = DESTINATION_ORIGIN[0],
        dy = DESTINATION_ORIGIN[1],
        dz = DESTINATION_ORIGIN[2],
    )
}

#[test]
fn a_shut_master_keeps_its_teleport_volume_from_working() {
    // The only entity targeting the master is one nothing ever activates,
    // so the master is never fired and never becomes active.
    let mut game = game(&mastered_entities("info_target"));
    run(&mut game, 1.0);

    assert_eq!(
        game.teleport_count(),
        0,
        "a volume whose master is shut must not move the player"
    );
}

#[test]
fn an_active_master_lets_its_teleport_volume_work() {
    // A `trigger_auto` fires the master on load, satisfying the only
    // entity that targets it.
    let mut game = game(&mastered_entities("trigger_auto"));
    run(&mut game, 1.0);

    assert_eq!(
        game.teleport_count(),
        1,
        "a volume whose master went active must move the player"
    );
    assert!(
        horizontal_distance_to_destination(&game) < 1.0,
        "the player was not placed at the destination: {:?}",
        game.player_origin()
    );
}

/// The regression `SECTION_TELEPORT_STATE` (34) exists to rule out: the
/// arrival seed must survive a save/load, or the first step after a load is
/// a rising edge on the volume the player is standing in and the chain
/// advances a scene the player never walked into.
///
/// Uses [`chained_entities`] exactly as
/// [`arriving_inside_the_next_volume_does_not_cascade_the_chain`] does — the
/// player is teleported once, and comes to rest *inside* the second volume
/// — with a `save_bytes`/`load_bytes` round trip inserted before the extra
/// half second. No-oping `Game::restore`'s `restore_teleport_state` call
/// fails this test rather than leaving the suite green.
#[test]
fn a_teleport_arrival_does_not_re_fire_after_a_save_load() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{MAP}.bsp"),
        door_behind_touch_trigger_bsp(&chained_entities()),
    );
    let mut game = Game::load(&assets as &dyn AssetSource, MAP).expect("the fixture loads");
    run(&mut game, 0.5);
    assert_eq!(
        game.teleport_count(),
        1,
        "the fixture must have teleported exactly once before the save"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    run(&mut reloaded, 0.5);

    // The counter itself starts fresh on a load, like every other
    // per-level counter, so what this asserts is that the reloaded game
    // teleports *nobody*: any count above zero is the spurious edge.
    assert_eq!(
        reloaded.teleport_count(),
        0,
        "the reloaded map saw a spurious rising edge on the volume the player \
         was standing in: SECTION_TELEPORT_STATE's teleport_touch half did not restore"
    );
    let origin = reloaded.player_origin();
    let dx = origin[0] - CHAINED_DESTINATION[0];
    let dy = origin[1] - CHAINED_DESTINATION[1];
    assert!(
        dx.hypot(dy) < 1.0,
        "the reloaded player was moved on to the next scene: {origin:?}"
    );
}

/// The other half of tag 34: a `multisource` that had gone active before the
/// save must not re-lock on load, or a map whose sequence is gated on one
/// stalls where it had been progressing.
///
/// The fixture's master is satisfied by a `trigger_auto`, which fires only
/// on the map's first tick and so cannot satisfy it a second time after a
/// load. The volume is left un-fired before the save (the player starts
/// outside it here), so the *reloaded* game is what has to teleport them.
#[test]
fn an_active_master_stays_active_across_a_save_load() {
    // Start outside the volume, so the master is satisfied on tick 0 but
    // nothing has been teleported yet when the save is taken.
    let outside = [-200.0, 200.0, 36.0];
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{ox} {oy} {oz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*1\"\n\
         \"target\" \"{DESTINATION}\"\n\"master\" \"ohl_tp_master\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"multisource\"\n\
         \"targetname\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"trigger_auto\"\n\
         \"target\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"info_teleport_destination\"\n\
         \"targetname\" \"{DESTINATION}\"\n\
         \"origin\" \"{dx} {dy} {dz}\"\n\"angles\" \"0 0 0\"\n}}\n",
        ox = outside[0],
        oy = outside[1],
        oz = outside[2],
        dx = DESTINATION_ORIGIN[0],
        dy = DESTINATION_ORIGIN[1],
        dz = DESTINATION_ORIGIN[2],
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{MAP}.bsp"),
        door_behind_touch_trigger_bsp(&entities),
    );
    let mut game = Game::load(&assets as &dyn AssetSource, MAP).expect("the fixture loads");
    // Long enough for the `trigger_auto` to fire the master, short enough
    // that the player (who starts outside the volume) has not moved into it.
    run(&mut game, 0.1);
    assert_eq!(
        game.teleport_count(),
        0,
        "the player starts outside the volume: nothing should have teleported yet"
    );

    let save = game.to_save(1_700_000_000);
    // The master is recorded as already fired; nothing has teleported.
    let teleport_state = save
        .teleport_state
        .clone()
        .expect("this build writes SECTION_TELEPORT_STATE");
    assert!(
        !teleport_state.master_fires.is_empty(),
        "the fixture's trigger_auto must have fired the master before the save"
    );

    let bytes = save.to_bytes().expect("the save encodes");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    // Walk into the volume: the reloaded master must still be active.
    reloaded.set_viewpoint(START, 0.0, 0.0);
    run(&mut reloaded, 0.5);
    assert_eq!(
        reloaded.teleport_count(),
        1,
        "the reloaded master re-locked: SECTION_TELEPORT_STATE's master_fires half did not restore"
    );
}

/// A save written by a build before `SECTION_TELEPORT_STATE` (34) existed
/// (the tag simply absent, reproduced by clearing `GameSave::teleport_state`
/// before encoding — the same technique `save_sections.rs`'s own
/// `a_save_from_before_section_30_existed_still_loads` and its tag-31
/// sibling already use) must still load.
#[test]
fn a_save_from_before_section_34_existed_still_loads() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{MAP}.bsp"),
        door_behind_touch_trigger_bsp(&chained_entities()),
    );
    let mut game = Game::load(&assets as &dyn AssetSource, MAP).expect("the fixture loads");
    run(&mut game, 0.5);

    let mut save = game.to_save(1_700_000_000);
    save.teleport_state = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_TELEPORT_STATE still encodes");

    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-34 save still loads");
    assert_eq!(
        reloaded.teleport_count(),
        0,
        "a pre-tag-34 save starts its own teleport counter fresh, like every other level load"
    );
}

/// The core of the master rule with more than one targeter: a `multisource`
/// two entities target is not active until *both* have fired it. Both
/// masters in the tests above have a single targeter, which cannot tell an
/// AND gate from a "any one fire opens it" rule.
#[test]
fn a_master_two_entities_target_needs_both_of_them() {
    // Two `trigger_auto`s target the master; the second carries a `delay`,
    // so there is a window in which exactly one of the two has fired it.
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{sx} {sy} {sz}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_teleport\"\n\"model\" \"*1\"\n\
         \"target\" \"{DESTINATION}\"\n\"master\" \"ohl_tp_master\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"multisource\"\n\
         \"targetname\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"trigger_auto\"\n\
         \"target\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"trigger_auto\"\n\"delay\" \"1\"\n\
         \"target\" \"ohl_tp_master\"\n\"origin\" \"0 0 64\"\n}}\n\
         {{\n\"classname\" \"info_teleport_destination\"\n\
         \"targetname\" \"{DESTINATION}\"\n\
         \"origin\" \"{dx} {dy} {dz}\"\n\"angles\" \"0 0 0\"\n}}\n",
        sx = START[0],
        sy = START[1],
        sz = START[2],
        dx = DESTINATION_ORIGIN[0],
        dy = DESTINATION_ORIGIN[1],
        dz = DESTINATION_ORIGIN[2],
    );
    let mut game = game(&entities);

    // Half a second in, only the undelayed `trigger_auto` has fired the
    // master: one of two, so the gate is still shut.
    run(&mut game, 0.5);
    assert_eq!(
        game.teleport_count(),
        0,
        "one of two targeters is not all of them: the master must still be shut"
    );

    // Past the second targeter's own delay, both have fired it.
    run(&mut game, 1.0);
    assert_eq!(
        game.teleport_count(),
        1,
        "with both targeters fired the master must be active"
    );
}
