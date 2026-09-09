//! A player standing on a `func_platrot` must be carried by *both* halves
//! of its trip: the vertical travel and the rigid rotation about its own
//! axis, and must never be left inside its solid.
//!
//! `mover_riders.rs` covers a purely translating mover and
//! `rotating_riders.rs` a purely rotating one. A `func_platrot` is the one
//! brush entity in this engine that does both at once, over one trip, so
//! this file is the measurement that the two carries compose rather than
//! replace each other: the fixture's platform rises
//! `PLATROT_HEIGHT` units and turns `PLATROT_ROTATION` degrees about the
//! documented default `Z` axis in the same
//! `PLATROT_HEIGHT / PLATROT_SPEED` seconds, so a rider standing off the
//! axis must end up both higher *and* swung round a quarter circle.
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
    let start = game.player_origin();
    assert!(
        (start[0] - ROTATING_PLATFORM_SPAWN_RADIUS).abs() < 4.0 && start[1].abs() < 4.0,
        "the fixture spawns the player on the platform's +X radius: {start:?}"
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
    assert!(
        end[0].abs() < 16.0 && (end[1] - ROTATING_PLATFORM_SPAWN_RADIUS).abs() < 16.0,
        "the rider was not carried round the platform's axis: {end:?}"
    );
}
