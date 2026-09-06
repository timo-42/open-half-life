//! Player movement against the project's synthetic collision fixtures.

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{
    build_brush_entity_floor_bsp, build_collision_pool_bsp, build_collision_room_bsp,
    build_collision_slope_bsp,
};
use ohl_physics::controller::TICK_SECONDS;
use ohl_physics::{
    CollisionModel, ControllerInput, MoveConfig, MoveInput, PlayerController, PlayerState, Vec3,
    WaterLevel, player_move,
};

fn model_from(bytes: &[u8]) -> CollisionModel {
    let limits = Limits::default();
    let bsp = Bsp::parse(bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

fn room() -> CollisionModel {
    model_from(&build_collision_room_bsp())
}

/// The origin height of a standing player resting on a floor at `z = 0`.
const FLOOR_ORIGIN_Z: f32 = 36.0;

fn simulate(
    model: &CollisionModel,
    state: &mut PlayerState,
    input: &MoveInput,
    config: &MoveConfig,
    seconds: f32,
) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ticks = (seconds / TICK_SECONDS).round() as u32;
    for _ in 0..ticks {
        player_move(model, state, input, config, TICK_SECONDS);
    }
}

fn walking(direction: Vec3) -> MoveInput {
    MoveInput {
        wish_move: direction.normalize_or_zero(),
        jump: false,
        duck: false,
        ..MoveInput::default()
    }
}

#[test]
fn gravity_settles_the_player_onto_the_floor() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);

    assert!(
        (state.origin.z - FLOOR_ORIGIN_Z).abs() < 0.2,
        "settled at {}",
        state.origin.z
    );
    assert!(state.on_ground);
    assert!(state.velocity.length() < 1.0);
    assert_eq!(state.water_level, WaterLevel::Dry);
}

#[test]
fn a_jump_reaches_the_documented_forty_five_unit_apex() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);
    let ground_z = state.origin.z;

    let jump = MoveInput {
        jump: true,
        ..MoveInput::default()
    };
    let mut apex: f32 = ground_z;
    for tick in 0..200 {
        // Release the key after the jump so it cannot be re-triggered.
        let input = if tick == 0 {
            jump
        } else {
            MoveInput::default()
        };
        player_move(&model, &mut state, &input, &config, TICK_SECONDS);
        apex = apex.max(state.origin.z);
    }

    let height = apex - ground_z;
    assert!(
        (height - 45.0).abs() < 1.0,
        "jump reached {height} units, expected about 45"
    );
    assert!(state.on_ground, "the player lands again");
}

#[test]
fn holding_jump_does_not_double_jump_within_one_tick() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);
    let held = MoveInput {
        jump: true,
        ..MoveInput::default()
    };
    player_move(&model, &mut state, &held, &config, TICK_SECONDS);
    let first = state.velocity.z;
    player_move(&model, &mut state, &held, &config, TICK_SECONDS);
    assert!(state.velocity.z < first, "velocity keeps falling off");
}

#[test]
fn walking_climbs_an_eighteen_unit_step_but_not_a_nineteen_unit_ledge() {
    let model = room();
    let config = MoveConfig::default();

    let mut on_step = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut on_step, &MoveInput::default(), &config, 2.0);
    // Half a second of walking is enough to reach and climb the step, and
    // not enough to walk off its far side.
    simulate(&model, &mut on_step, &walking(Vec3::X), &config, 0.5);
    assert!(
        (on_step.origin.z - (18.0 + FLOOR_ORIGIN_Z)).abs() < 0.3,
        "expected to stand on the 18-unit step, ended at z {}",
        on_step.origin.z
    );
    assert!(on_step.origin.x > 64.0, "walked onto the step");

    let mut at_ledge = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut at_ledge, &MoveInput::default(), &config, 2.0);
    simulate(&model, &mut at_ledge, &walking(-Vec3::X), &config, 1.0);
    assert!(
        (at_ledge.origin.z - FLOOR_ORIGIN_Z).abs() < 0.3,
        "the 19-unit ledge is too tall to step onto, ended at z {}",
        at_ledge.origin.z
    );
    assert!(
        at_ledge.origin.x > -49.0,
        "blocked in front of the ledge, ended at x {}",
        at_ledge.origin.x
    );
}

#[test]
fn a_walkable_slope_supports_the_player_and_a_steep_one_does_not() {
    let config = MoveConfig::default();

    // Surface normal z = 0.8, above the 0.7 slope limit.
    let walkable = model_from(&build_collision_slope_bsp(-0.6, 0.8));
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&walkable, &mut state, &MoveInput::default(), &config, 2.0);
    assert!(state.on_ground, "0.8 is a walkable slope");
    assert!(state.velocity.length() < 1.0, "the player rests on it");

    // Surface normal z = 0.5, below the slope limit.
    let steep = model_from(&build_collision_slope_bsp(-0.866_025_4, 0.5));
    let mut sliding = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&steep, &mut sliding, &MoveInput::default(), &config, 2.0);
    assert!(!sliding.on_ground, "0.5 is too steep to stand on");
    assert!(
        sliding.velocity.length() > 10.0,
        "the player keeps sliding down it"
    );
}

#[test]
fn ground_friction_decays_speed_to_a_stop() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);

    state.velocity = Vec3::new(config.max_speed, 0.0, 0.0);
    let mut previous = state.velocity.length();
    for _ in 0..20 {
        player_move(
            &model,
            &mut state,
            &MoveInput::default(),
            &config,
            TICK_SECONDS,
        );
        let speed = state.velocity.length();
        assert!(speed <= previous, "friction never adds speed");
        previous = speed;
    }
    assert!(previous < config.max_speed * 0.9, "speed fell noticeably");

    simulate(&model, &mut state, &MoveInput::default(), &config, 3.0);
    assert!(state.velocity.length() < 1.0, "the player comes to a stop");
}

#[test]
fn air_acceleration_stops_at_the_air_speed_cap() {
    // A flat floor far below, so the player stays airborne throughout.
    let model = model_from(&build_collision_slope_bsp(0.0, 1.0));
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 2000.0));

    simulate(&model, &mut state, &walking(Vec3::X), &config, 1.0);
    assert!(!state.on_ground);
    assert!(
        (state.velocity.x - config.air_speed_cap).abs() < 0.5,
        "air speed along the wish direction settled at {}, expected the {} cap",
        state.velocity.x,
        config.air_speed_cap
    );
}

#[test]
fn ground_acceleration_reaches_max_speed() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);
    // A fifth of a second is enough to reach full speed, and not enough to
    // reach the step.
    simulate(&model, &mut state, &walking(Vec3::X), &config, 0.2);
    let speed = state.velocity.length();
    assert!(
        speed > config.max_speed * 0.9 && speed <= config.max_speed + 0.1,
        "reached {speed} units/s"
    );
}

#[test]
fn walking_into_a_wall_stops_the_player_without_leaving_the_room() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);
    // Walking east crosses the 18-unit step, drops off its far side and
    // ends against the far wall, whose face is 16 units from the origin.
    simulate(&model, &mut state, &walking(Vec3::X), &config, 5.0);
    assert!(
        state.origin.x > 239.0 && state.origin.x < 241.0,
        "stopped at the wall, ended at x {}",
        state.origin.x
    );
    assert!((state.origin.z - FLOOR_ORIGIN_Z).abs() < 0.3);
}

#[test]
fn the_player_walks_up_a_shallow_ramp_but_is_stopped_by_a_steep_one() {
    let model = room();
    let config = MoveConfig::default();

    // The fixture's +Y ramp has a surface normal of z = 0.8.
    let mut climbing = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut climbing, &MoveInput::default(), &config, 2.0);
    simulate(&model, &mut climbing, &walking(Vec3::Y), &config, 1.0);
    assert!(
        climbing.origin.z > FLOOR_ORIGIN_Z + 10.0,
        "walked up the ramp to z {}",
        climbing.origin.z
    );
    assert!(climbing.on_ground, "still standing on the ramp");

    // The -Y ramp has a surface normal of z = 0.5, below the slope limit, so
    // the player is stopped by its face instead of walking up it.
    let mut blocked = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut blocked, &MoveInput::default(), &config, 2.0);
    simulate(&model, &mut blocked, &walking(-Vec3::Y), &config, 1.0);
    assert!(
        (blocked.origin.z - FLOOR_ORIGIN_Z).abs() < 1.0,
        "did not climb the steep ramp, z {}",
        blocked.origin.z
    );
    assert!(blocked.origin.y > -130.0, "y {}", blocked.origin.y);
}

#[test]
fn noclip_passes_through_walls_and_collision_resumes_afterwards() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 100.0));
    state.noclip = true;
    simulate(&model, &mut state, &walking(Vec3::X), &config, 3.0);
    assert!(
        state.origin.x > 300.0,
        "noclip left the room, ended at x {}",
        state.origin.x
    );

    // Back inside, with collision on again: the wall stops the player.
    state.noclip = false;
    state.origin = Vec3::new(0.0, 0.0, 100.0);
    state.velocity = Vec3::ZERO;
    simulate(&model, &mut state, &walking(Vec3::X), &config, 3.0);
    assert!(state.origin.x < 241.0, "ended at x {}", state.origin.x);
}

#[test]
fn ducking_lowers_the_hull_and_the_eye_height_and_standing_up_restores_them() {
    let model = room();
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);
    let standing_eye = state.eye_position(&config).z;

    let duck = MoveInput {
        duck: true,
        ..MoveInput::default()
    };
    simulate(&model, &mut state, &duck, &config, 0.5);
    assert!(state.ducked);
    assert!(
        (state.origin.z - 18.0).abs() < 0.2,
        "ducked origin at {}",
        state.origin.z
    );
    let ducked_eye = state.eye_position(&config).z;
    assert!(
        (ducked_eye - 30.0).abs() < 0.3,
        "ducked eye at {ducked_eye}"
    );
    assert!(ducked_eye < standing_eye);

    simulate(&model, &mut state, &MoveInput::default(), &config, 0.5);
    assert!(!state.ducked);
    assert!((state.origin.z - FLOOR_ORIGIN_Z).abs() < 0.2);
    assert!((state.eye_position(&config).z - standing_eye).abs() < 0.3);
}

#[test]
fn entering_water_switches_to_swimming_and_holding_jump_swims_up() {
    let model = model_from(&build_collision_pool_bsp());
    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 200.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 3.0);

    assert_ne!(state.water_level, WaterLevel::Dry, "the player fell in");
    assert!(state.is_swimming(), "waist-deep or more counts as swimming");

    let start_z = state.origin.z;
    let swim_up = MoveInput {
        jump: true,
        ..MoveInput::default()
    };
    simulate(&model, &mut state, &swim_up, &config, 1.0);
    assert!(
        state.origin.z > start_z,
        "swimming up rose from {start_z} to {}",
        state.origin.z
    );
}

#[test]
fn the_controller_runs_whole_ticks_and_carries_the_remainder() {
    let model = room();
    let mut controller = PlayerController::spawn_at(Vec3::new(0.0, 0.0, 40.0), 0.0, 0.0);
    let input = ControllerInput::default();

    assert_eq!(controller.advance(&model, &input, TICK_SECONDS / 2.0), 0);
    assert_eq!(controller.advance(&model, &input, TICK_SECONDS / 2.0), 1);
    // A long stall is clamped instead of simulating the whole gap.
    assert_eq!(controller.advance(&model, &input, 10.0), 10);
    assert_eq!(controller.advance(&model, &input, f32::NAN), 0);
}

#[test]
fn the_controller_walks_forward_along_its_yaw_and_toggles_noclip() {
    let model = room();
    // Yaw 180 faces -X, toward the 19-unit ledge on a flat floor.
    let mut controller = PlayerController::spawn_at(Vec3::new(0.0, 0.0, 40.0), 180.0, 0.0);
    let forward = ControllerInput {
        forward: 1,
        ..ControllerInput::default()
    };
    for _ in 0..100 {
        controller.advance(&model, &forward, TICK_SECONDS);
    }
    assert!(
        controller.state.origin.x < -40.0,
        "walked west and stopped at the ledge"
    );
    assert!(controller.state.origin.y.abs() < 1.0);
    assert!(
        (controller.eye_position().z - (FLOOR_ORIGIN_Z + 28.0)).abs() < 0.3,
        "eye at {}",
        controller.eye_position().z
    );

    assert!(controller.toggle_noclip());
    assert_eq!(controller.state.velocity, Vec3::ZERO);
    assert!(!controller.toggle_noclip());
}

/// A map whose floor is a brush entity (a `func_wall` slab over a void) is
/// the ordinary case, not an exotic one: without the entity's own hulls in
/// the collision model there is nothing under the player at all and they
/// fall for as long as the simulation runs. With it attached, gravity must
/// settle them on the slab exactly as on a worldspawn floor.
#[test]
fn the_player_stands_on_a_brush_entity_floor() {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let mut model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");
    model
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .expect("the fixture declares submodel 1");

    let config = MoveConfig::default();
    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 64.0));
    simulate(&model, &mut state, &MoveInput::default(), &config, 2.0);

    assert!(state.on_ground, "the player never found the slab");
    assert!(
        (state.origin.z - FLOOR_ORIGIN_Z).abs() < 0.2,
        "settled at {} rather than on the slab",
        state.origin.z
    );

    // And walking off its edge is still a fall: the slab is 128 units wide,
    // so a player driven far enough in +X leaves it.
    simulate(&model, &mut state, &walking(Vec3::X), &config, 3.0);
    assert!(
        state.origin.z < FLOOR_ORIGIN_Z - 64.0,
        "the player did not fall off the edge of a finite slab"
    );
}

/// Without the brush entity attached the same fixture has no floor at all,
/// which is exactly the failure a worldspawn-only collision model produced
/// on a real map.
#[test]
fn a_worldspawn_only_model_lets_the_player_fall_through_a_brush_entity_floor() {
    let bytes = build_brush_entity_floor_bsp("func_wall");
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    let model = CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable hulls");

    let mut state = PlayerState::at(Vec3::new(0.0, 0.0, 64.0));
    simulate(
        &model,
        &mut state,
        &MoveInput::default(),
        &MoveConfig::default(),
        2.0,
    );
    assert!(!state.on_ground);
    assert!(state.origin.z < 0.0);
}
