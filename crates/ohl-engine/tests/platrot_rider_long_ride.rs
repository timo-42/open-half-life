//! A rider on a `func_platrot` must stay seated at the *same point of the
//! platform* for the platform's whole trip, however long that trip is or
//! however many turns it makes — not just for the single quarter turn
//! `platrot_rider.rs` measures.
//!
//! This file is a targeted regression test for a scenario a visual
//! walkthrough of a real map's `func_platrot` once raised: a scripted rider
//! on a two-full-turn, ~976-unit, ~19.5-second lift stayed carried for most
//! of the ride and then died partway through, plausibly because that
//! walkthrough's own fixed lateral spawn offset drifted off the platform's
//! rotating footprint late in the trip rather than because of a defect in
//! the carry code. This test checks that attribution the only way that is
//! falsifiable in a synthetic fixture: reproduce the same shape of ride (a
//! multi-turn `func_platrot`, a rider seated off-axis) on a **cross**-shaped
//! synthetic platform — one with open notches between its arms at every
//! angle that is not a multiple of 90 degrees, unlike the full square slab
//! `platrot_rider.rs` uses — and measure, every tick, how far the rider's
//! own seat has drifted from the point [`ohl_game::pose::platrot_degrees`]
//! and [`ohl_game::pose::platrot_offset`] (the same functions the renderer,
//! collision and rider-carry code already read) say the platform has
//! carried it to.
//!
//! No bytes here come from any game installation; see
//! `docs/CLEAN_ROOM.md`.

use glam::{Quat, Vec3};
use ohl_engine::test_support::{
    PLATROT_LONG_HEIGHT, PLATROT_LONG_LATERAL_MARGIN, PLATROT_LONG_MAP, PLATROT_LONG_NAME,
    PLATROT_LONG_SPEED, platrot_long_bsp, platrot_long_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::pose::{platrot_degrees, platrot_offset};
use ohl_game::registry::PlatRot;

const STEP: f32 = 1.0 / 100.0;

/// How far, in the platform's own rotating frame, the rider's seat may
/// drift from the point [`platrot_degrees`]/[`platrot_offset`] predict it
/// is at, at any single tick of the whole trip.
///
/// [`PLATROT_LONG_LATERAL_MARGIN`] is the most this fixture's own geometry
/// can absorb before the rider is over the open notch between two arms
/// (about 12 units); this bound is comfortably tighter than that; a real
/// defect (a refused carry silently falling back to a tangential-velocity
/// approximation, a pivot that has already moved by the time the carry
/// reads it, or similar) would show up growing over the trip rather than
/// staying pinned near zero.
const SEAT_DRIFT_TOLERANCE: f32 = 6.0;

fn game() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLATROT_LONG_MAP}.bsp"),
        platrot_long_bsp(&platrot_long_entities()),
    );
    Game::load(&assets as &dyn AssetSource, PLATROT_LONG_MAP).expect("the fixture loads")
}

fn platrot_entity(game: &Game) -> ohl_game::hecs::Entity {
    *game
        .registry()
        .find(PLATROT_LONG_NAME)
        .first()
        .expect("the fixture declares one named func_platrot")
}

/// Where the rider's own seat — fixed at `local_xy` in the platform's own
/// rotating frame, on top of a surface that lifts by
/// [`platrot_offset`]'s `Z` — currently is, per the same pose functions the
/// renderer, collision and rider-carry code already read.
fn expected_seat(game: &Game, entity: ohl_game::hecs::Entity, local_xy: Vec3, rest_z: f32) -> Vec3 {
    let registry = game.registry();
    let (axis, degrees) = platrot_degrees(registry, entity);
    let translation = platrot_offset(registry, entity);
    let rotated = if axis == Vec3::ZERO {
        local_xy
    } else {
        Quat::from_axis_angle(axis.normalize(), degrees.to_radians()) * local_xy
    };
    Vec3::new(rotated.x, rotated.y, rest_z) + translation
}

#[test]
fn a_rider_stays_seated_on_a_two_turn_func_platrot_for_the_whole_trip() {
    let mut game = game();
    let entity = platrot_entity(&game);

    // Long enough to settle the spawn drop and register one touch, exactly
    // like `platrot_rider.rs`'s own `stepping_onto_a_func_platrot_starts_it`
    // — the platform may already be a little way into its trip by the time
    // this returns, which is fine: `local_xy`/`rest_z` below are derived
    // from wherever it now is, not assumed to be time zero.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let settle_steps = (0.2 / STEP).round() as u32;
    for _ in 0..settle_steps {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "settling: the rider ended up inside the platform at {:?}",
            game.eye_position()
        );
    }

    // The rider's own seat, in the platform's rotating frame: the settled
    // world position rotated *back* by however far the platform has already
    // turned, and lowered by however far it has already lifted.
    let settled = Vec3::from_array(game.player_origin());
    let (axis, degrees) = platrot_degrees(game.registry(), entity);
    let translation = platrot_offset(game.registry(), entity);
    let local_xy = if axis == Vec3::ZERO {
        Vec3::new(settled.x, settled.y, 0.0)
    } else {
        Quat::from_axis_angle(axis.normalize(), -degrees.to_radians())
            * Vec3::new(settled.x, settled.y, 0.0)
    };
    let rest_z = settled.z - translation.z;

    let total_seconds = PLATROT_LONG_HEIGHT / PLATROT_LONG_SPEED + 0.6;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (total_seconds / STEP).round() as u32;

    let mut max_drift = 0.0f32;
    for step in 0..steps {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "step {step}: the rider ended up inside the platform at {:?}",
            game.eye_position()
        );

        let expected = expected_seat(&game, entity, local_xy, rest_z);
        let actual = Vec3::from_array(game.player_origin());
        let drift = actual.truncate().distance(expected.truncate());
        max_drift = max_drift.max(drift);
        assert!(
            drift < SEAT_DRIFT_TOLERANCE,
            "step {step}: the rider's seat drifted {drift} units from the \
             platform's own rotated seat (actual {actual:?}, expected \
             {expected:?}); the fixture's own geometry only absorbs about \
             {PLATROT_LONG_LATERAL_MARGIN} units before this is a fall \
             rather than a measurement",
        );
        // A rider dropped off the ride would first show up as `on_ground`
        // going false while the platform is still mid-trip (nothing else
        // in this void world to stand on), well before the drift bound
        // above would itself catch a fall straight down through a notch.
        let platrot = game
            .registry()
            .world
            .get::<&PlatRot>(entity)
            .expect("the fixture's own entity is a func_platrot");
        // Only while the platform is actively travelling: before the rider
        // has touched it and started it (`Closed`) the ordinary spawn drop
        // is still settling, and once it has finished (`Open`) the platform
        // is no longer moving, so neither end is informative about whether
        // the *ride* kept the rider seated.
        let mid_trip = platrot.state == ohl_game::registry::MoverState::Opening;
        if mid_trip {
            assert!(
                game.player_on_ground(),
                "step {step}: the rider left the ground mid-trip, with \
                 nothing else in this void world to stand on",
            );
        }
    }

    // The trip should have completed; a rider still being carried this long
    // after `total_seconds` would mean the platform itself never finished,
    // which is a fixture bug rather than the thing under test.
    let platrot = game
        .registry()
        .world
        .get::<&PlatRot>(entity)
        .expect("the fixture's own entity is a func_platrot");
    assert_eq!(
        platrot.state,
        ohl_game::registry::MoverState::Open,
        "the two-turn trip should be over by now"
    );

    // Redundant with the per-tick assertion above, but keeps the trip's own
    // worst-case drift figure available if this ever needs debugging.
    assert!(
        max_drift < SEAT_DRIFT_TOLERANCE,
        "max drift over the whole trip: {max_drift}"
    );
}
