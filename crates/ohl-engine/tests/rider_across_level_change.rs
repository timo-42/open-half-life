//! A passenger crosses a level boundary *aboard* the ride, and an arrival
//! the landmark offset puts inside solid is settled rather than frozen.
//!
//! The documented placement rule for a `trigger_changelevel` (see
//! `docs/FORMAT_SOURCES.md`, "Campaign flow") is the landmark offset: the
//! arriving player keeps the offset from the destination's `info_landmark`
//! they had from the source map's own. That rule silently assumes whatever
//! they were standing on is in the same place relative to the landmark in
//! both maps — true of world geometry, and false of a `func_tracktrain`,
//! whose destination copy is placed by the destination's *own* `path_track`
//! chain (`ohl_engine::transition`'s `restore_track_train`). Two maps that
//! share one ride routinely place that chain's head somewhere else and
//! point it somewhere else, and the offset then moves the passenger by
//! however far the two chains disagree — off the car, or through its
//! interior wall.
//!
//! So a rider's seat is carried relative to the mover they are riding
//! (`ohl_engine::transition::RiderSeat`), in that mover's own frame, and
//! takes precedence over the raw offset. A player standing on world
//! geometry still gets the documented offset, unchanged and exact.
//!
//! The last test here pins the other half: an arrival the offset lands
//! *inside* solid is not an offset the physics state can carry across at
//! all (no ground brush resolves while `start_solid` holds, and no traced
//! move out of solid succeeds), so `ohl_physics::settle_if_embedded` frees
//! it with the same bounded upward nudge a landing already uses.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use glam::Vec3;
use ohl_engine::test_support::{
    LANDMARK, RIDER_BLOCK_DISPLACEMENT_X, RIDER_BLOCK_MAP, RIDER_CAR_SPEED, RIDER_DESTINATION_MAP,
    RIDER_EMBEDDED_MAP, RIDER_FOOT_SPAWN, RIDER_SOURCE_MAP, RIDER_TERMINUS_MAP, RiderMap,
    rider_boundary_bsp,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};

/// How far the bounded nudge may raise an embedded arrival, mirrored from
/// `ohl_physics`' own `UNSTICK_MAX_NUDGE` (private there).
const MAX_NUDGE: f32 = 34.0;

fn assets(source: RiderMap, destination_map: &str, destination: RiderMap) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{RIDER_SOURCE_MAP}.bsp"),
        rider_boundary_bsp(source),
    );
    assets.insert(
        &format!("maps/{destination_map}.bsp"),
        rider_boundary_bsp(destination),
    );
    assets
}

fn tick_n(game: &mut Game, n: u32) {
    for _ in 0..n {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

/// The car's posed centre, straight out of the same pose helper the
/// renderer and the collision hull read.
fn car_center(game: &Game) -> Vec3 {
    let registry = game.registry();
    let entity = registry.find("ohl_rider_car")[0];
    ohl_game::pose::brush_center(registry, entity).expect("the fixture car is a brush entity")
}

/// The player's seat in the car's own frame: the offset from its centre,
/// turned back through the car's own heading.
fn seat(game: &Game) -> Vec3 {
    let registry = game.registry();
    let entity = registry.find("ohl_rider_car")[0];
    let yaw = ohl_game::pose::track_train_transform(registry, entity)
        .1
        .expect("the fixture car is on a horizontal segment");
    glam::Quat::from_rotation_z(-yaw.to_radians())
        * (Vec3::from_array(game.player_origin()) - car_center(game))
}

/// A passenger aboard the car when the change fires arrives aboard the
/// destination's copy of it, in the same seat, even though that copy sits
/// hundreds of units away from where the landmark offset would have put
/// them and points ninety degrees off.
#[test]
fn a_rider_arrives_in_the_same_seat_on_the_destination_map_s_car() {
    let assets = assets(
        RiderMap::Source,
        RIDER_DESTINATION_MAP,
        RiderMap::Destination,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 120);
    assert!(
        (game.ground_mover_speed() - RIDER_CAR_SPEED).abs() < 1.0,
        "the fixture must have the player riding the source car before the change"
    );
    let before_seat = seat(&game);
    let before_origin = Vec3::from_array(game.player_origin());

    game.change_level(&assets, RIDER_DESTINATION_MAP, LANDMARK)
        .expect("the destination map loads");

    let after_seat = seat(&game);
    assert!(
        (after_seat - before_seat).length() < 1.0,
        "the passenger must keep their seat in the car's own frame: \
         {before_seat:?} -> {after_seat:?}"
    );

    // Both maps put the landmark at the same coordinates, so the raw
    // offset is the identity placement — and it is nowhere near the
    // destination's own copy of the car.
    let raw_offset_arrival = before_origin;
    assert!(
        (Vec3::from_array(game.player_origin()) - raw_offset_arrival).length() > 100.0,
        "the landmark offset alone would have left the passenger behind at \
         {raw_offset_arrival:?}, which is the bug this test pins"
    );

    tick_n(&mut game, 2);
    assert!(
        (game.ground_mover_speed() - RIDER_CAR_SPEED).abs() < 1.0,
        "and they must be riding the destination's car within two steps"
    );
    tick_n(&mut game, 300);
    let later_seat = seat(&game);
    assert!(
        (later_seat - before_seat).length() < 4.0,
        "and keep that seat for the rest of the ride: {before_seat:?} -> {later_seat:?}"
    );
}

/// The other side of the precedence rule: a player standing on *world*
/// geometry is placed by the documented landmark offset, exactly, with no
/// seat and no nudge. Both maps put the landmark at the same coordinates,
/// so a correct arrival does not move the player at all.
#[test]
fn a_player_on_foot_still_arrives_at_the_plain_landmark_offset() {
    let assets = assets(
        RiderMap::SourceOnFoot,
        RIDER_DESTINATION_MAP,
        RiderMap::Destination,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    assert!(
        game.ground_mover_speed().abs() < f32::EPSILON,
        "the fixture must have this player standing on world geometry"
    );
    let before = Vec3::from_array(game.player_origin());

    game.change_level(&assets, RIDER_DESTINATION_MAP, LANDMARK)
        .expect("the destination map loads");

    let after = Vec3::from_array(game.player_origin());
    assert!(
        (after - before).length() < 0.001,
        "a landmark-relative arrival that is not embedded must stay a pure \
         offset: {before:?} -> {after:?}"
    );
}

/// An arrival the offset lands *inside* a brush entity's solid is freed by
/// the same bounded upward nudge a landing already uses, instead of being
/// left frozen there for the rest of the map.
#[test]
fn an_arrival_embedded_in_solid_is_settled_onto_what_it_was_stuck_in() {
    let assets = assets(
        RiderMap::SourceOnFoot,
        RIDER_EMBEDDED_MAP,
        RiderMap::Embedded,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    let before = Vec3::from_array(game.player_origin());
    assert!(
        (before - Vec3::from_array(RIDER_FOOT_SPAWN)).length() < 1.0,
        "the fixture must have this player standing where the destination \
         map's parked block is"
    );

    game.change_level(&assets, RIDER_EMBEDDED_MAP, LANDMARK)
        .expect("the destination map loads");

    let settled = Vec3::from_array(game.player_origin());
    assert!(
        settled.z > before.z + 1.0 && settled.z <= before.z + MAX_NUDGE,
        "the embedded arrival must be nudged clear, within the bound: \
         {before:?} -> {settled:?}"
    );
    assert!(
        (settled.x - before.x).abs() < 0.001 && (settled.y - before.y).abs() < 0.001,
        "and only straight up, never sideways: {before:?} -> {settled:?}"
    );

    tick_n(&mut game, 60);
    let later = Vec3::from_array(game.player_origin());
    assert!(
        (later - settled).length() < 0.001,
        "and they must then be resting on the block rather than falling \
         back into it: {settled:?} -> {later:?}"
    );
}

/// The car's heading, in degrees, as the pose helpers report it.
fn car_yaw(game: &Game) -> Option<f32> {
    let registry = game.registry();
    let entity = registry.find("ohl_rider_car")[0];
    ohl_game::pose::track_train_transform(registry, entity).1
}

/// A map that *ends* a shared ride parks its own copy of the car on a
/// chain of one `path_track`: no segment anywhere in that chain, so the
/// car has no heading of its own and would be posed unrotated — across
/// the track its geometry was authored along, with the arriving
/// passenger's seat nowhere near its floor. The heading the ride arrived
/// with is used as the last fallback, and only then.
#[test]
fn a_ride_that_ends_in_the_destination_keeps_the_heading_it_arrived_with() {
    let assets = assets(
        RiderMap::SourceTurned,
        RIDER_TERMINUS_MAP,
        RiderMap::Terminus,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    let before_yaw = car_yaw(&game).expect("the source car is on a horizontal segment");
    assert!(
        (before_yaw + 90.0).abs() < 0.01,
        "the fixture's source chain runs along +Y, so its car travels at 90 degrees and \
         is posed a `ohl_game::track_train::COMPILED_FACING_OFFSET_DEGREES` half turn \
         from that, at -90 — not {before_yaw}"
    );
    let before_seat = seat(&game);

    game.change_level(&assets, RIDER_TERMINUS_MAP, LANDMARK)
        .expect("the terminus map loads");

    let after_yaw = car_yaw(&game).expect("the arrived car must still report a heading");
    assert!(
        (after_yaw - before_yaw).abs() < 0.01,
        "a one-node chain defines no heading, so the car must keep the one \
         it arrived with: {before_yaw} -> {after_yaw}"
    );
    let after_seat = seat(&game);
    assert!(
        (after_seat - before_seat).length() < 1.0,
        "and the passenger's seat must land on its floor: {before_seat:?} -> {after_seat:?}"
    );

    tick_n(&mut game, 30);
    assert!(
        game.player_origin()[2] > f32::from(0i16),
        "the passenger must be standing on the parked car, not falling past it"
    );
    let later_seat = seat(&game);
    assert!(
        (later_seat - before_seat).length() < 1.0,
        "and stay there: {before_seat:?} -> {later_seat:?}"
    );
}

/// The rule is about a *ride*, not about any named brush entity: a train is
/// the one brush entity whose placement comes from a `path_track` chain
/// rather than from where its geometry was compiled, so it is the only one
/// the landmark offset can disagree with. A player standing on a named
/// `func_wall` keeps the documented offset even when the destination
/// declares that same name six hundred units away.
#[test]
fn standing_on_a_named_brush_that_is_not_a_ride_keeps_the_landmark_offset() {
    let assets = assets(
        RiderMap::SourceOnBlock,
        RIDER_BLOCK_MAP,
        RiderMap::BlockElsewhere,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    let before = Vec3::from_array(game.player_origin());
    assert!(
        (before.z - (24.0 + 36.0)).abs() < 1.0,
        "the fixture must have this player standing on the block's top, not {before:?}"
    );

    game.change_level(&assets, RIDER_BLOCK_MAP, LANDMARK)
        .expect("the destination map loads");

    let after = Vec3::from_array(game.player_origin());
    assert!(
        (after - before).length() < 0.001,
        "a non-ride brush must not take precedence over the landmark offset: \
         {before:?} -> {after:?}"
    );
    assert!(
        (after.x - (before.x + RIDER_BLOCK_DISPLACEMENT_X)).abs() > 100.0,
        "and certainly not place the player over the destination's own copy \
         of that brush: {after:?}"
    );
}

/// The heading a ride is handed over with is the only heading a car parked
/// on a single-node chain has, so it has to survive a save/load — otherwise
/// a quicksave taken on that arrival reloads the car unrotated and drops the
/// passenger through where its floor used to be. `SECTION_TRAIN_HANDOVER_YAW`
/// (tag 35) persists it.
#[test]
fn a_carried_heading_survives_a_save_and_load() {
    let assets = assets(
        RiderMap::SourceTurned,
        RIDER_TERMINUS_MAP,
        RiderMap::Terminus,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    game.change_level(&assets, RIDER_TERMINUS_MAP, LANDMARK)
        .expect("the terminus map loads");
    let arrived_yaw = car_yaw(&game).expect("the arrived car reports a heading");
    let arrived_seat = seat(&game);

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the arrival state encodes");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");

    let reloaded_yaw = car_yaw(&reloaded).expect("the reloaded car must still report a heading");
    assert!(
        (reloaded_yaw - arrived_yaw).abs() < 0.01,
        "the carried heading must survive the save: {arrived_yaw} -> {reloaded_yaw}"
    );
    assert!(
        (seat(&reloaded) - arrived_seat).length() < 1.0,
        "so the passenger reloads in the same seat on the car"
    );

    tick_n(&mut reloaded, 60);
    assert!(
        (seat(&reloaded) - arrived_seat).length() < 1.0,
        "and stays on it rather than falling through where its floor used to be"
    );
}

/// A save written before tag 35 existed still loads: the section is
/// optional, and a train whose own chain defines a heading never needed it.
/// The car that *did* need it simply reloads without one, which is the
/// behaviour every build before M9.25 had.
#[test]
fn a_save_without_the_carried_heading_section_still_loads() {
    let assets = assets(
        RiderMap::SourceTurned,
        RIDER_TERMINUS_MAP,
        RiderMap::Terminus,
    );
    let mut game = Game::load(&assets, RIDER_SOURCE_MAP).expect("the source map loads");
    tick_n(&mut game, 60);
    game.change_level(&assets, RIDER_TERMINUS_MAP, LANDMARK)
        .expect("the terminus map loads");

    let mut save = game.to_save(1_700_000_000);
    assert!(
        save.train_handover_yaw
            .as_ref()
            .is_some_and(|yaws| yaws.iter().any(Option::is_some)),
        "this build must be writing the section for this arrival"
    );
    save.train_handover_yaw = None;

    let bytes = save.to_bytes().expect("an older-shaped save still encodes");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("an older-shaped save still loads");
    assert!(
        car_yaw(&reloaded).is_none(),
        "and reloads exactly as a pre-M9.25 build did: the one-node chain \
         defines no heading and no section supplied one"
    );
}
