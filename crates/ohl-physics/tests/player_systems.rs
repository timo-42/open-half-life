//! Ladders, liquid categorisation, riding a mover, the long jump and the
//! landing report, all against this project's own synthetic fixtures.

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_physics::movement::{in_ladder_volume, ladder_normal, player_move_events};
use ohl_physics::test_support::{
    LIQUID_SURFACE_Z, build_flat_floor_bsp, build_ladder_entity_room_bsp, build_ladder_room_bsp,
    build_liquid_room_bsp, build_thin_ladder_room_bsp, build_water_entity_room_bsp,
    collision_model_from,
};
use ohl_physics::{
    CollisionModel, ContentsKind, LiquidKind, MoveConfig, MoveInput, PlayerState, Vec3, WaterLevel,
    contents,
};

const TICK: f32 = 1.0 / 100.0;

fn ladder_room() -> CollisionModel {
    collision_model_from(&build_ladder_room_bsp())
}

fn thin_ladder_room() -> CollisionModel {
    collision_model_from(&build_thin_ladder_room_bsp())
}

/// The origin of a player standing beside [`build_thin_ladder_room_bsp`]'s
/// ladder with their *origin* outside the volume (`x = 26 < 40`) but their
/// standing hull (`x` ±16) overlapping it (up to `x = 42`, inside the
/// volume's `x` 40..48).
const BESIDE_THIN_LADDER: Vec3 = Vec3::new(26.0, 0.0, 36.0);

/// The origin of a player standing inside the ladder fixture's volume.
const IN_LADDER: Vec3 = Vec3::new(72.0, 0.0, 36.0);

fn walk(direction: Vec3) -> MoveInput {
    MoveInput {
        wish_move: direction.normalize_or_zero(),
        ..MoveInput::default()
    }
}

#[test]
fn the_ladder_fixture_has_ladder_contents_and_an_outward_normal() {
    let model = ladder_room();
    assert_eq!(model.point_contents(IN_LADDER), contents::LADDER);
    assert_eq!(
        model.point_contents(Vec3::new(0.0, 0.0, 36.0)),
        contents::EMPTY
    );
    assert_eq!(
        model.point_contents(Vec3::new(200.0, 0.0, 36.0)),
        contents::SOLID
    );

    let state = PlayerState::at(IN_LADDER);
    // The only open horizontal face of the volume is toward -X (the wall is
    // at x = 96), so that is the ladder's outward normal.
    assert_eq!(ladder_normal(&model, &state), Vec3::new(-1.0, 0.0, 0.0));
}

#[test]
fn pressing_into_a_ladder_climbs_it_and_releasing_holds_position() {
    let model = ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(IN_LADDER);
    // Wishing toward +X pushes into the ladder face, which climbs.
    let input = walk(Vec3::X);

    let events = player_move_events(&model, &mut state, &input, &config, TICK);
    assert!(events.ladder_attached);
    assert!(state.on_ladder);

    let start_z = state.origin.z;
    for _ in 0..100 {
        player_move_events(&model, &mut state, &input, &config, TICK);
    }
    let climbed = state.origin.z - start_z;
    assert!(
        climbed > config.ladder_speed * 0.9,
        "one second of climbing rose only {climbed} units"
    );
    assert!(state.on_ladder);

    // Letting go of every key holds the player where they are: no gravity
    // applies on a ladder.
    let held_z = state.origin.z;
    for _ in 0..100 {
        player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
    }
    assert!((state.origin.z - held_z).abs() < 0.5, "{}", state.origin.z);
}

#[test]
fn pulling_away_from_a_ladder_climbs_down() {
    let model = ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(72.0, 0.0, 150.0));
    let input = walk(-Vec3::X);
    for _ in 0..50 {
        player_move_events(&model, &mut state, &input, &config, TICK);
    }
    assert!(state.origin.z < 150.0 - config.ladder_speed * 0.4);
}

#[test]
fn jumping_detaches_from_a_ladder_and_pushes_away_from_it() {
    let model = ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(IN_LADDER);
    player_move_events(&model, &mut state, &walk(Vec3::X), &config, TICK);
    assert!(state.on_ladder);

    let jump = MoveInput {
        jump: true,
        ..MoveInput::default()
    };
    let events = player_move_events(&model, &mut state, &jump, &config, TICK);
    assert!(events.ladder_detached);
    assert!(!state.on_ladder);
    assert!(state.velocity.x < 0.0, "{:?}", state.velocity);
    assert!(state.ladder_lockout > 0.0);
}

#[test]
fn leaving_the_ladder_volume_detaches() {
    let model = ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(IN_LADDER);
    player_move_events(&model, &mut state, &walk(Vec3::X), &config, TICK);
    assert!(state.on_ladder);

    // Away from the volume (the fixture's ladder spans x 56..96) the very
    // next step releases the player.
    state.origin.x = 0.0;
    let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
    assert!(events.ladder_detached);
    assert!(!state.on_ladder);
}

#[test]
fn catching_a_ladder_cancels_a_fall_and_reports_no_landing() {
    let model = ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(72.0, 0.0, 240.0));
    state.velocity = Vec3::new(0.0, 0.0, -600.0);
    let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
    assert!(events.ladder_attached);
    assert_eq!(state.velocity, Vec3::ZERO);
    assert_eq!(events.landed_speed, None);
}

#[test]
fn water_levels_step_from_dry_to_eyes_as_the_player_descends() {
    let model = collision_model_from(&build_liquid_room_bsp(contents::WATER));
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 400.0));

    // Above the pool: dry.
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Dry);
    assert_eq!(state.liquid, LiquidKind::None);
    assert_eq!(state.water_level.as_index(), 0);

    // Feet (origin - 35) under the surface but the origin above it.
    state.origin.z = LIQUID_SURFACE_Z + 28.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Feet);
    assert_eq!(state.liquid, LiquidKind::Water);
    assert_eq!(state.water_level.as_index(), 1);

    // Origin under the surface, eye (origin + 28) above it.
    state.origin.z = LIQUID_SURFACE_Z - 10.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Waist);
    assert_eq!(state.water_level.as_index(), 2);
    assert!(state.is_swimming());

    // Fully under.
    state.origin.z = 100.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Eyes);
    assert_eq!(state.water_level.as_index(), 3);
}

#[test]
fn slime_and_lava_are_categorised_by_their_own_contents() {
    for (value, expected) in [
        (contents::SLIME, LiquidKind::Slime),
        (contents::LAVA, LiquidKind::Lava),
    ] {
        let model = collision_model_from(&build_liquid_room_bsp(value));
        let config = MoveConfig::default();
        let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 100.0));
        ohl_physics::movement::categorize_position(&model, &mut state, &config);
        assert_eq!(state.water_level, WaterLevel::Eyes);
        assert_eq!(state.liquid, expected);
    }
}

#[test]
fn a_water_level_change_is_reported_once() {
    let model = collision_model_from(&build_liquid_room_bsp(contents::WATER));
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 400.0));

    let mut changes = 0;
    let mut levels: Vec<WaterLevel> = Vec::new();
    for _ in 0..400 {
        let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
        if events.water_level_changed {
            changes += 1;
            levels.push(state.water_level);
        }
    }
    // Falling into a deep pool passes through every level.
    assert!(
        changes >= 3,
        "only {changes} water level changes: {levels:?}"
    );
}

#[test]
fn standing_on_a_moving_platform_carries_the_player_along() {
    let model = collision_model_from(&build_flat_floor_bsp());
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 36.0));
    let ride = MoveInput {
        base_velocity: Vec3::new(100.0, 0.0, 0.0),
        ..MoveInput::default()
    };
    for _ in 0..100 {
        player_move_events(&model, &mut state, &ride, &config, TICK);
    }
    assert!(state.on_ground);
    // One second of a 100 units/s platform.
    assert!(
        (state.origin.x - 100.0).abs() < 2.0,
        "rode to {:?}",
        state.origin
    );
    // The ride is not stored as the player's own velocity: stepping off
    // stops them.
    assert!(state.velocity.length() < 1.0, "{:?}", state.velocity);
}

#[test]
fn a_long_jump_needs_the_module_and_fires_only_inside_the_duck_window() {
    let model = collision_model_from(&build_flat_floor_bsp());
    let config = MoveConfig::default();

    // Duck and jump on the same tick, with the module: a long jump.
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 36.0));
    player_move_events(&model, &mut state, &walk(Vec3::X), &config, TICK);
    let combo = MoveInput {
        wish_move: Vec3::X,
        jump: true,
        duck: true,
        long_jump: true,
        ..MoveInput::default()
    };
    let events = player_move_events(&model, &mut state, &combo, &config, TICK);
    assert!(events.long_jumped);
    assert!(state.velocity.x >= config.long_jump_forward_speed - 1.0);
    assert!((state.velocity.z - config.long_jump_up_speed).abs() < config.gravity * TICK + 1.0);

    // Same combo without the module: an ordinary crouch jump.
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 36.0));
    player_move_events(&model, &mut state, &walk(Vec3::X), &config, TICK);
    let no_module = MoveInput {
        long_jump: false,
        ..combo
    };
    let events = player_move_events(&model, &mut state, &no_module, &config, TICK);
    assert!(!events.long_jumped);
    assert!(state.velocity.x < config.max_speed);

    // Ducked for longer than the window: also an ordinary crouch jump.
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 36.0));
    let ducking = MoveInput {
        wish_move: Vec3::X,
        duck: true,
        long_jump: true,
        ..MoveInput::default()
    };
    // Comfortably past the duck window at 100 ticks per second.
    assert!(config.long_jump_duck_window < 0.5);
    for _ in 0..60 {
        player_move_events(&model, &mut state, &ducking, &config, TICK);
    }
    let events = player_move_events(&model, &mut state, &combo, &config, TICK);
    assert!(!events.long_jumped, "the duck window should have expired");
}

#[test]
fn landing_reports_the_impact_speed_for_a_fall_but_not_for_a_step() {
    let model = collision_model_from(&build_flat_floor_bsp());
    let config = MoveConfig::default();

    // A 400-unit fall.
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 436.0));
    let mut landing = None;
    for _ in 0..400 {
        let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
        if let Some(speed) = events.landed_speed {
            landing = Some(speed);
            break;
        }
    }
    let speed = landing.expect("the player lands");
    // v = sqrt(2 * g * h) for a 400-unit drop under 800 units/s^2.
    let expected = (2.0 * config.gravity * 400.0).sqrt();
    assert!(
        (speed - expected).abs() < 40.0,
        "impact speed {speed} is not near {expected}"
    );

    // Walking off an 18-unit step never leaves the ground long enough to
    // build up a fall: no landing event, or one far below the fall speed.
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 38.0));
    let mut fastest: f32 = 0.0;
    for _ in 0..100 {
        let events = player_move_events(&model, &mut state, &walk(Vec3::X), &config, TICK);
        if let Some(speed) = events.landed_speed {
            fastest = fastest.max(speed);
        }
    }
    assert!(fastest < 100.0, "a step reported an impact of {fastest}");
}

#[test]
fn landing_in_a_liquid_reports_no_impact() {
    let model = collision_model_from(&build_liquid_room_bsp(contents::WATER));
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 600.0));
    for _ in 0..800 {
        let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
        assert_eq!(events.landed_speed, None, "at {:?}", state.origin);
        if state.on_ground {
            break;
        }
    }
}

// ---------------------------------------------------------------------
// `func_ladder`/`func_water` attached as entity contents volumes.
//
// Every test above this point uses a fixture with `CONTENTS_LADDER`/
// `CONTENTS_WATER` baked directly into the *world* tree — the case that
// worked before this milestone. These fixtures instead attach the exact
// same-shaped volume as submodel 1 with
// `CollisionModel::attach_contents_brush`, the way a real map's
// `func_ladder`/`func_water` brush entity actually arrives (see
// `docs/FORMAT_SOURCES.md`, "Player systems"), to prove the entity path
// itself makes climbing/swimming work — with no change to this crate's own
// movement code, since it only ever reads probed contents.
// ---------------------------------------------------------------------

fn ladder_entity_room() -> CollisionModel {
    let bytes = build_ladder_entity_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    model
        .attach_contents_brush(&bsp, &limits, 1, Vec3::ZERO, ContentsKind::Ladder)
        .expect("the fixture declares submodel 1");
    model
}

fn water_entity_room() -> CollisionModel {
    let bytes = build_water_entity_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    model
        .attach_contents_brush(&bsp, &limits, 1, Vec3::ZERO, ContentsKind::Water)
        .expect("the fixture declares submodel 1");
    model
}

#[test]
fn a_func_ladder_entity_volume_attaches_and_climbs_with_forward_input() {
    // Same volume, same origin (`IN_LADDER`) and the same assertions as
    // `pressing_into_a_ladder_climbs_it_and_releasing_holds_position`
    // above, but against the brush-entity-attached model: proves a real
    // `func_ladder`'s own submodel, not just a world-baked
    // `CONTENTS_LADDER` leaf, makes the player attach and climb.
    let model = ladder_entity_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(IN_LADDER);
    let input = walk(Vec3::X);

    let events = player_move_events(&model, &mut state, &input, &config, TICK);
    assert!(
        events.ladder_attached,
        "the entity-attached volume did not attach the player"
    );
    assert!(state.on_ladder);

    let start_z = state.origin.z;
    for _ in 0..100 {
        player_move_events(&model, &mut state, &input, &config, TICK);
    }
    let climbed = state.origin.z - start_z;
    assert!(
        climbed > config.ladder_speed * 0.9,
        "one second of climbing rose only {climbed} units"
    );
    assert!(state.on_ladder);
}

#[test]
fn a_func_water_entity_pool_steps_through_every_water_level_and_swims() {
    // Same pool geometry and surface height as `build_liquid_room_bsp`
    // (`LIQUID_SURFACE_Z`), attached as submodel 1 instead of baked into
    // the world tree: proves a real `func_water`'s own submodel makes the
    // pool swimmable.
    let model = water_entity_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 400.0));

    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Dry);
    assert_eq!(state.water_level.as_index(), 0);

    state.origin.z = LIQUID_SURFACE_Z + 28.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Feet);
    assert_eq!(state.liquid, LiquidKind::Water);
    assert_eq!(state.water_level.as_index(), 1);

    state.origin.z = LIQUID_SURFACE_Z - 10.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Waist);
    assert_eq!(state.water_level.as_index(), 2);
    assert!(state.is_swimming());

    state.origin.z = 100.0;
    ohl_physics::movement::categorize_position(&model, &mut state, &config);
    assert_eq!(state.water_level, WaterLevel::Eyes);
    assert_eq!(state.water_level.as_index(), 3);

    // Fully submerged and swimming: holding jump (the documented "swim up"
    // input) rises.
    let swim_up = MoveInput {
        jump: true,
        ..MoveInput::default()
    };
    let start_z = state.origin.z;
    for _ in 0..100 {
        player_move_events(&model, &mut state, &swim_up, &config, TICK);
    }
    assert!(
        state.origin.z > start_z,
        "swimming up did not rise: {} -> {}",
        start_z,
        state.origin.z
    );
}

#[test]
fn landing_in_a_func_water_entity_pool_reports_no_fall_damage_impact() {
    // The same rule `landing_in_a_liquid_reports_no_impact` checks above
    // (SourceRuns "Landing methods", already cited in
    // `docs/FORMAT_SOURCES.md`: "landing in puddles of water, no matter how
    // shallow, will always break your fall"), against the entity-attached
    // pool.
    let model = water_entity_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 600.0));
    for _ in 0..800 {
        let events = player_move_events(&model, &mut state, &MoveInput::default(), &config, TICK);
        assert_eq!(events.landed_speed, None, "at {:?}", state.origin);
        if state.on_ground {
            break;
        }
    }
}

#[test]
fn a_hull_overlapping_a_ladder_attaches_even_though_the_origin_is_outside_it() {
    let model = thin_ladder_room();
    // The origin alone reads as ordinary open space, not `CONTENTS_LADDER`:
    // proves this test's premise before checking the hull-aware result.
    assert_eq!(model.point_contents(BESIDE_THIN_LADDER), contents::EMPTY);

    let state = PlayerState::at(BESIDE_THIN_LADDER);
    assert!(
        in_ladder_volume(&model, &state),
        "a hull overlapping the ladder volume must attach even with the \
         origin outside it"
    );
    assert_ne!(ladder_normal(&model, &state), Vec3::ZERO);
}

#[test]
fn a_hull_beside_a_thin_ladder_climbs_it_with_forward_input() {
    let model = thin_ladder_room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(BESIDE_THIN_LADDER);
    // Wishing toward +X presses into the ladder (the open room is toward
    // -X, where the player's origin already is), which climbs.
    let input = walk(Vec3::X);

    let events = player_move_events(&model, &mut state, &input, &config, TICK);
    assert!(events.ladder_attached);
    assert!(state.on_ladder);

    let start_z = state.origin.z;
    for _ in 0..100 {
        player_move_events(&model, &mut state, &input, &config, TICK);
    }
    let climbed = state.origin.z - start_z;
    assert!(
        climbed > config.ladder_speed * 0.9,
        "one second of climbing rose only {climbed} units"
    );
}

#[test]
fn climbing_past_the_top_of_the_ladder_volume_detaches() {
    let model = ladder_room();
    let config = MoveConfig::default();
    // The fixture's ladder spans z 0..256; starting near the top and
    // climbing up must eventually take the whole hull clear of it.
    let mut state = PlayerState::at(Vec3::new(72.0, 0.0, 240.0));
    let input = walk(Vec3::X);

    let mut detached = false;
    for _ in 0..600 {
        let events = player_move_events(&model, &mut state, &input, &config, TICK);
        if events.ladder_detached {
            detached = true;
            break;
        }
    }
    assert!(
        detached,
        "the player never left the ladder volume climbing up; ended at \
         {:?}",
        state.origin
    );
    assert!(!state.on_ladder);
    // Detaching at the top must not leave the player wedged in solid.
    let trace = model.trace(state.hull(), state.origin, state.origin);
    assert!(!trace.start_solid, "stuck in solid at {:?}", state.origin);
}
