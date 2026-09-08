//! A ride whose track runs out is carried onto the next one.
//!
//! Two published entities meet here:
//!
//! - `path_track`'s `netname` ("fire on dead end"): the entity fired when
//!   a `func_tracktrain` reaches that node as the last node of its chain.
//!   `docs/FORMAT_SOURCES.md` already recorded this keyvalue (alongside
//!   `altpath`) as documented-but-unimplemented; it is implemented now.
//! - `func_trackautochange`: the moving piece of track that "takes the
//!   train from last path_track of the top path, rotating and descending,
//!   and then, after finishing, the train is assigned to path_track of the
//!   bottom path". This entity was not recorded anywhere in this
//!   repository before — no doc, no source, no test — so its public
//!   sources are newly written down here.
//!
//! Without them a ride that changes track simply stops at the dead end,
//! and every node beyond it — including the ones whose `message` opens the
//! doors ahead of the ride — is never passed. See `docs/FORMAT_SOURCES.md`
//! ("Track trains and paths") and `docs/MILESTONES.md` M9.19.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    TRACK_CHANGE_BOTTOM_START, TRACK_CHANGE_CAR_HALF_WIDTH, TRACK_CHANGE_HEIGHT, TRACK_CHANGE_MAP,
    TRACK_CHANGE_SPEED, TRACK_CHANGE_TOP_END, TRACK_CHANGE_TOP_START, track_change_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};

const STEP: f32 = 1.0 / 60.0;

fn loaded() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{TRACK_CHANGE_MAP}.bsp"), track_change_bsp());
    Game::load(&assets as &dyn AssetSource, TRACK_CHANGE_MAP).expect("the track-change map loads")
}

/// Steps for the whole documented trip: `height / speed` seconds.
fn lift_steps() -> u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = ((TRACK_CHANGE_HEIGHT / TRACK_CHANGE_SPEED) / STEP).round() as u32;
    steps
}

/// Steps for the run up to the dead end, plus a couple of settling steps.
fn approach_steps() -> u32 {
    let distance = TRACK_CHANGE_TOP_END[0] - TRACK_CHANGE_TOP_START[0];
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = ((distance / TRACK_CHANGE_TRAIN_SPEED) / STEP).ceil() as u32 + 8;
    steps
}

const TRACK_CHANGE_TRAIN_SPEED: f32 = ohl_engine::test_support::TRACK_CHANGE_TRAIN_SPEED;

/// The whole handover, end to end: the car rides its top chain into a dead
/// end, the dead end's `netname` starts the platform, the platform carries
/// the car — with its passenger still aboard — down and round, and the car
/// then rides the bottom chain away.
#[test]
fn a_train_that_dead_ends_is_carried_onto_the_bottom_track_with_its_passenger() {
    let mut game = loaded();
    for _ in 0..4 {
        game.tick(STEP, &Input::default());
    }
    let seat = game.player_origin();
    assert!(
        (seat[2] - TRACK_CHANGE_TOP_START[2]).abs() < 64.0,
        "the passenger should start seated on the car at the top: {seat:?}"
    );

    // Ride to the dead end.
    for _ in 0..approach_steps() {
        game.tick(STEP, &Input::default());
    }
    let at_dead_end = game.player_origin();
    assert!(
        (at_dead_end[0] - TRACK_CHANGE_TOP_END[0]).abs() < 64.0,
        "the car should have reached the top chain's dead end: {at_dead_end:?}"
    );
    assert!(
        (at_dead_end[2] - TRACK_CHANGE_TOP_START[2]).abs() < 64.0,
        "the car should still be at the top: {at_dead_end:?}"
    );

    // Half the documented trip: the descent is under way, not finished, so
    // the carry is a travel rather than a teleport at the far end.
    for _ in 0..lift_steps() / 2 {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "the passenger was pushed inside solid geometry during the descent"
        );
    }
    let midway = game.player_origin();
    assert!(
        midway[2] < at_dead_end[2] - TRACK_CHANGE_HEIGHT * 0.25
            && midway[2] > at_dead_end[2] - TRACK_CHANGE_HEIGHT * 0.75,
        "the passenger should be part-way down mid-trip, not at either end: {midway:?}"
    );

    // The rest of the trip, then a stretch of the bottom chain.
    for _ in 0..lift_steps() {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "the passenger was pushed inside solid geometry after the descent"
        );
    }
    let landed = game.player_origin();
    assert!(
        (landed[2] - TRACK_CHANGE_BOTTOM_START[2]).abs() < 64.0,
        "the passenger should have arrived on the bottom track: {landed:?}"
    );

    for _ in 0..120 {
        game.tick(STEP, &Input::default());
    }
    let away = game.player_origin();
    assert!(
        away[1] > landed[1] + 50.0,
        "the relinked car should have ridden off along the bottom chain, \
         carrying its passenger: {away:?} from {landed:?}"
    );
    assert!(
        (away[0] - TRACK_CHANGE_BOTTOM_START[0]).abs() < TRACK_CHANGE_CAR_HALF_WIDTH,
        "the passenger should be on the bottom chain's own line, not beside it: {away:?}"
    );
}

/// The passenger is carried continuously, never dropped: they are on a
/// mover every step of the trip, which is what distinguishes riding the
/// platform down from being left behind and falling the same distance.
#[test]
fn the_passenger_rides_the_platform_rather_than_falling_the_same_distance() {
    let mut game = loaded();
    for _ in 0..(4 + approach_steps()) {
        game.tick(STEP, &Input::default());
    }
    let mut descending_steps = 0_u32;
    for _ in 0..lift_steps() {
        game.tick(STEP, &Input::default());
        // A free fall accelerates well past the platform's own documented
        // speed within a fraction of a second; a carried passenger tracks
        // it exactly.
        let speed = game.ground_mover_speed();
        assert!(
            speed > 0.0,
            "the passenger left the mover during the trip (they were dropped)"
        );
        assert!(
            speed < TRACK_CHANGE_SPEED * 4.0,
            "the passenger is moving far faster than the platform's own speed: {speed}"
        );
        descending_steps += 1;
    }
    assert_eq!(descending_steps, lift_steps());
}
