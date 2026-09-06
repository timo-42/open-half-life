//! The fixed timestep: what one [`Game::tick`] call does depends on how much
//! time it is given, never on how that time was cut into frames.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

// Exact float comparison is the point of most of these assertions: a fixed
// timestep is only worth having if two runs that deliver the same simulated
// time produce bit-identical results.
#![allow(clippy::float_cmp)]

use ohl_engine::test_support::{SYNTHETIC_MAP, synthetic_map_bsp};
use ohl_engine::{
    AssetSource, Game, Input, MAX_TICKS_PER_FRAME, MemoryAssets, PlayerTag, StudioAnim,
    TICK_SECONDS,
};
use ohl_game::registry::Transform;
use proptest::prelude::*;

/// One second of simulated time, as whole steps.
const ONE_SECOND_STEPS: u32 = 100;

fn assets() -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), synthetic_map_bsp());
    assets
}

fn game(assets: &dyn AssetSource) -> Game {
    Game::load(assets, SYNTHETIC_MAP).expect("the synthetic map loads")
}

/// The most simulated time one call can release, i.e. what the frame clamp
/// and the step clamp agree on.
fn max_advance_per_call() -> f32 {
    TICK_SECONDS * f32::from(u16::try_from(MAX_TICKS_PER_FRAME).expect("a small step count"))
}

#[test]
fn the_same_simulated_second_lands_in_the_same_place_however_it_is_cut() {
    let assets = assets();

    // A frame that releases the whole clamped burst, ten times over...
    let mut coarse = game(&assets);
    for _ in 0..(ONE_SECOND_STEPS / MAX_TICKS_PER_FRAME) {
        coarse.tick(max_advance_per_call(), &Input::default());
    }

    // ...and a hundred frames that release exactly one step each.
    let mut fine = game(&assets);
    for _ in 0..ONE_SECOND_STEPS {
        fine.tick(TICK_SECONDS, &Input::default());
    }

    assert_eq!(
        coarse.eye_position(),
        fine.eye_position(),
        "the player ends the second at the same place either way"
    );
    assert_eq!(
        coarse.elapsed(),
        fine.elapsed(),
        "and with the same simulated clock"
    );
    assert!(
        (coarse.elapsed() - 1.0).abs() <= TICK_SECONDS,
        "a hundred steps is a second of simulated time"
    );
}

#[test]
fn one_overlong_call_is_clamped_rather_than_replaying_the_whole_second() {
    // Handing a whole second to a single call does *not* simulate a second:
    // the frame clamp bounds the work one call may do, and the backlog is
    // dropped rather than banked, so a stall costs one clamped frame instead
    // of an unbounded catch-up.
    let assets = assets();
    let mut game = game(&assets);
    game.tick(1.0, &Input::default());
    assert!(game.elapsed() <= max_advance_per_call() + f32::EPSILON);
    assert!(
        (game.elapsed() - max_advance_per_call()).abs() < 1e-6,
        "exactly the clamped burst ran"
    );
}

#[test]
fn a_frame_shorter_than_a_step_banks_its_time_rather_than_dropping_it() {
    let assets = assets();
    let mut game = game(&assets);
    let half = TICK_SECONDS * 0.5;
    game.tick(half, &Input::default());
    assert_eq!(game.elapsed(), 0.0, "half a step releases no step");
    game.tick(half, &Input::default());
    assert_eq!(
        game.elapsed(),
        TICK_SECONDS,
        "the banked half completes the step"
    );
}

#[test]
fn the_level_spawns_exactly_one_player_entity_carrying_the_published_maxima() {
    let assets = assets();
    let game = game(&assets);
    let player = game.player_entity();
    let world = &game.registry().world;

    assert!(world.contains(player));
    assert!(world.get::<&PlayerTag>(player).is_ok());
    let health = *world.get::<&ohl_combat::Health>(player).expect("health");
    assert_eq!(health.current, ohl_engine::PLAYER_MAX_HEALTH);
    let armor = *world.get::<&ohl_combat::Armor>(player).expect("armor");
    assert_eq!(armor.max, ohl_engine::PLAYER_MAX_ARMOR);
    assert_eq!(
        armor.current, 0.0,
        "the suit is picked up, not spawned with"
    );

    let mut tagged = world.query::<&PlayerTag>();
    assert_eq!((&mut tagged).into_iter().count(), 1);
}

#[test]
fn the_player_entity_is_not_in_the_definition_aligned_entity_list() {
    // `Registry::entities` is index-aligned with the parsed entity lump, and
    // a save references entities by their index in it, so the engine's own
    // player entity must stay out of it.
    let assets = assets();
    let game = game(&assets);
    assert_eq!(game.registry().entities.len(), game.entity_defs().len());
    assert!(!game.registry().entities.contains(&game.player_entity()));
}

#[test]
fn the_player_entity_follows_the_player() {
    let assets = assets();
    let mut game = game(&assets);
    for _ in 0..20 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    let transform = *game
        .registry()
        .world
        .get::<&Transform>(game.player_entity())
        .expect("the player entity carries a transform");
    let eye = game.eye_position();
    assert!(
        (transform.origin.x - eye[0]).abs() < 1e-3 && (transform.origin.y - eye[1]).abs() < 1e-3,
        "the entity stands where the player stands"
    );
}

#[test]
fn every_prop_placement_becomes_a_drawable_entity_whose_cursor_advances() {
    let map = ohl_engine::test_support::synthetic_map_bsp_with_extra_entity(
        "ohl_next",
        "{\n\"classname\" \"monster_generic\"\n\
         \"model\" \"models/ohl_prop.mdl\"\n\
         \"origin\" \"10 20 30\"\n\"sequence\" \"0\"\n}\n",
    );
    let (mdl_bytes, _layout) = ohl_formats::test_support::build_minimal_mdl10();
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), map);
    assets.insert("models/ohl_prop.mdl", mdl_bytes);

    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the map loads");
    assert_eq!(game.prop_count(), 1);

    let cursor = |game: &Game| {
        let mut query = game.registry().world.query::<&StudioAnim>();
        let cycles: Vec<f32> = (&mut query).into_iter().map(|anim| anim.cycle).collect();
        assert_eq!(cycles.len(), 1, "one placement, one drawable entity");
        cycles[0]
    };

    assert_eq!(cursor(&game), 0.0);
    for _ in 0..5 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert!(
        (cursor(&game) - 5.0 * TICK_SECONDS).abs() < 1e-4,
        "the animation cursor advances one step per step"
    );
}

/// An arbitrary input snapshot, including axis values outside the tri-state
/// the controller documents.
fn arbitrary_input() -> impl Strategy<Value = Input> {
    (
        (any::<i8>(), any::<i8>(), any::<i8>()),
        (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()),
        (any::<bool>(), any::<bool>(), any::<bool>()),
        proptest::option::of(any::<u8>()),
        (any::<f32>(), any::<f32>()),
    )
        .prop_map(
            |(
                (forward, right, up),
                (jump, duck, use_pressed, use_held),
                (attack, attack2, reload),
                select_slot,
                mouse_delta,
            )| Input {
                forward,
                right,
                up,
                jump,
                duck,
                use_pressed,
                use_held,
                attack,
                attack2,
                reload,
                select_slot,
                flashlight_pressed: reload,
                mouse_delta,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any frame time and any input snapshot: the frame loop never panics,
    /// and one call never advances the simulated clock by more than the
    /// clamped burst.
    #[test]
    fn an_arbitrary_frame_never_panics_and_never_outruns_the_clamp(
        dts in proptest::collection::vec(any::<f32>(), 1..24),
        input in arbitrary_input(),
    ) {
        let assets = assets();
        let mut game = game(&assets);
        for dt in dts {
            let before = game.elapsed();
            game.tick(dt, &input);
            let advanced = game.elapsed() - before;
            prop_assert!(advanced >= 0.0, "the clock never runs backwards");
            prop_assert!(
                advanced <= max_advance_per_call() + f32::EPSILON,
                "one call runs at most MAX_TICKS_PER_FRAME steps"
            );
        }
    }
}
