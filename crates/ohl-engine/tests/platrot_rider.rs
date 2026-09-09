//! A player standing on a `func_platrot` must be carried by *both* halves
//! of its trip — the vertical travel and the rotation about its own axis —
//! and must never be left inside its solid.
//!
//! `mover_riders.rs` covers a purely translating mover and
//! `rotating_riders.rs` a purely rotating one. A `func_platrot` is the one
//! brush entity in this engine that does both at once, over one trip, so
//! this file measures that the two *compose*: the fixture's platform rises
//! `PLATROT_HEIGHT` units and turns `PLATROT_ROTATION` degrees about the
//! documented default `Z` axis in the same
//! `PLATROT_HEIGHT / PLATROT_SPEED` seconds, and a rider standing off the
//! axis must end at the point their seat is carried to — their boarding
//! point *rotated about the axis* and then raised — not at the point
//! directly above where they boarded, and not spiralled off the platform
//! either.
//!
//! **Which code path delivers the rotation is deliberately not asserted,
//! because for this entity the two agree.** `ohl_engine` has two: the
//! tangential `omega x r` ride velocity blended into the move
//! (`Level::brush_ride_velocity`), and the rigid whole-angle step applied
//! before it (`Level::rotational_carry`). The rigid step exists for a mover
//! that turns through a *large* angle in a single tick — a
//! `func_tracktrain` taking a corner — where integrating a velocity walks
//! the rider off the arc. A `func_platrot` at any speed a map declares
//! turns a fraction of a degree per tick, so the two paths agree to well
//! under a unit over a whole quarter turn, and asserting which one ran
//! would be asserting an implementation detail rather than a behaviour.
//! What is asserted is the *result*: the seat is preserved, to a tolerance
//! far tighter than the difference between being carried round and not
//! being carried round at all.
//!
//! The fixture is a square slab in an otherwise empty void, centred on its
//! own pivot: a quarter turn maps its footprint onto itself, so a rider who
//! is not carried round has somewhere to stand and is not rescued by the
//! geometry, and a rider who is not carried up simply falls.
//!
//! No bytes here come from any game installation; see
//! `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    PLATROT_HEIGHT, PLATROT_MAP, PLATROT_NAME, PLATROT_ROTATION, PLATROT_SPEED,
    ROTATING_PLATFORM_SPAWN_RADIUS, platrot_entities, rotating_platform_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::{MoverState, PlatRot};

const STEP: f32 = 1.0 / 100.0;

/// The physics origin of a standing player resting on the slab's top
/// surface at `z = 0`: `Hull::Standing`'s own foot offset.
const RIDER_REST_Z: f32 = 36.0;

/// How far, in world units, the rider's finishing seat may sit from the
/// point the platform's own trip carries it to.
///
/// Measured slack at the fixture's own speed is under 2.5 units over a
/// whole quarter turn and a 128-unit lift, most of it the player's spawn
/// drop onto the slab before the trip begins. This bound is comfortably
/// above that and far, far below the ~135 units that separate "carried
/// round the axis" from "merely raised".
const COMPOSED_CARRY_TOLERANCE: f32 = 6.0;

fn game() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLATROT_MAP}.bsp"),
        rotating_platform_bsp(&platrot_entities()),
    );
    Game::load(&assets as &dyn AssetSource, PLATROT_MAP).expect("the fixture loads")
}

/// Ticks `seconds` of idle input, asserting on every step that the player
/// is never inside solid geometry.
fn ride(game: &mut Game, seconds: f32) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let steps = (seconds / STEP).round() as u32;
    for step in 0..steps {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "step {step}: the rider ended up inside the platform at {:?}",
            game.eye_position()
        );
    }
}

fn platrot(game: &Game) -> PlatRot {
    let entity = *game
        .registry()
        .find(PLATROT_NAME)
        .first()
        .expect("the fixture declares one named func_platrot");
    *game
        .registry()
        .world
        .get::<&PlatRot>(entity)
        .expect("the named entity is a func_platrot")
}

#[test]
fn the_keyvalues_and_default_axis_are_read_from_the_map() {
    let platrot = platrot(&game());
    assert!((platrot.travel_distance - PLATROT_HEIGHT).abs() < f32::EPSILON);
    assert!((platrot.rotation_degrees - PLATROT_ROTATION).abs() < f32::EPSILON);
    assert!((platrot.speed - PLATROT_SPEED).abs() < f32::EPSILON);
    assert_eq!(platrot.movedir, glam::Vec3::Z, "a positive height goes up");
    assert_eq!(platrot.axis, glam::Vec3::Z, "no axis spawnflag means Z");
    assert!(!platrot.toggle, "the fixture sets no spawnflags");
}

#[test]
fn stepping_onto_a_func_platrot_starts_it() {
    let mut game = game();
    assert_eq!(
        platrot(&game).state,
        MoverState::Closed,
        "nothing has touched it yet"
    );
    // Long enough to settle the spawn drop and register one touch.
    ride(&mut game, 0.2);
    assert_ne!(
        platrot(&game).state,
        MoverState::Closed,
        "standing on a func_platrot without the Toggle spawnflag starts it"
    );
}

#[test]
fn a_func_platrot_carries_a_rider_up_and_round_together() {
    let mut game = game();
    let start = [ROTATING_PLATFORM_SPAWN_RADIUS, 0.0, RIDER_REST_Z];
    let spawned = game.player_origin();
    assert!(
        (spawned[0] - start[0]).abs() < 4.0 && spawned[1].abs() < 4.0,
        "the fixture spawns the player on the platform's +X radius: {spawned:?}"
    );

    // The whole trip, plus a margin: the player's own spawn drop onto the
    // slab is what starts it, and the first tick or two go on that.
    ride(&mut game, PLATROT_HEIGHT / PLATROT_SPEED + 0.6);
    assert_eq!(
        platrot(&game).state,
        MoverState::Open,
        "the trip should be over"
    );

    let end = game.player_origin();
    // Carried *up*: the platform's whole travel, on top of where the rider
    // was resting.
    assert!(
        (end[2] - (RIDER_REST_Z + PLATROT_HEIGHT)).abs() < 8.0,
        "the rider was not carried the platform's full travel: {end:?}"
    );
    // Carried *round*: a positive `rotation` about `+Z` takes a rider on
    // the `+X` radius to the `+Y` one (this project's own sign convention;
    // see `ohl_game::registry::PlatRot::rotation_degrees`).
    //
    // The whole composed carry in one assertion: where the seat ends up is
    // the boarding point rotated about the platform's own axis and then
    // raised by its travel. A rider carried only by the translation would
    // be a full `2 * ROTATING_PLATFORM_SPAWN_RADIUS / sqrt(2)` — about 135
    // units — from here, so this tolerance is not close to being satisfied
    // by accident.
    let expected = glam::Vec3::new(
        0.0,
        ROTATING_PLATFORM_SPAWN_RADIUS,
        start[2] + PLATROT_HEIGHT,
    );
    let end = glam::Vec3::from_array(end);
    assert!(
        end.distance(expected) < COMPOSED_CARRY_TOLERANCE,
        "the rider is not where the platform's own trip carries their seat: \
         {end:?}, expected about {expected:?}"
    );
    // And the seat is *on the arc*, not spiralled off it: a rigid turn
    // preserves a rider's distance from the axis exactly, and the velocity
    // blend that delivers the same turn tick by tick preserves it to well
    // inside this margin.
    let radius = end.truncate().length();
    assert!(
        (radius - ROTATING_PLATFORM_SPAWN_RADIUS).abs() < COMPOSED_CARRY_TOLERANCE,
        "the rider drifted off the radius they boarded at: {radius}"
    );
}
