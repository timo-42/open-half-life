//! Project-authored follow recovery control; no installed-game data is used.
//! Player viewpoint changes are explicit fixture placement, not traversal.

use ohl_ai::monsters::brains::FOLLOW_PLAYER;
use ohl_ai::movement::{ROUTE_REFRESH_DISTANCE, STUCK_TICKS, WAYPOINT_TOLERANCE};
use ohl_ai::{Actor, Conditions, MonsterAi, Vec3};
use ohl_engine::test_support::{
    SCRIPT_MAP, ai_room_bsp, entity_block, entity_of_classname, script_room_entities, use_input,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::hecs::Entity;
use ohl_physics::{DIST_EPSILON, Hull};

fn actor(game: &Game, entity: Entity) -> Actor {
    *game.registry().world.get::<&Actor>(entity).expect("actor")
}

fn follower_state(game: &Game, entity: Entity) -> MonsterAi {
    (*game.registry().world.get::<&MonsterAi>(entity).expect("AI")).clone()
}

fn place_player_eye(game: &mut Game, center: Vec3) {
    let offset = Vec3::from_array(game.eye_position()) - Vec3::from_array(game.player_origin());
    game.set_viewpoint((center + offset).to_array(), 0.0, 0.0);
    assert!(Vec3::from_array(game.player_origin()).abs_diff_eq(center, DIST_EPSILON));
    let trace = game
        .collision()
        .expect("collision")
        .trace(Hull::Standing, center, center);
    assert!(!trace.start_solid && !trace.all_solid);
}

fn assert_supported_clear_leg(game: &Game, body: Actor, goal: Vec3) {
    let from = body.query_origin();
    assert!(from.is_finite() && goal.is_finite());
    assert!((from.z - goal.z).abs() <= DIST_EPSILON);
    let collision = game.collision().expect("collision");
    let chord = collision.trace(body.hull, from, goal);
    assert!(!chord.start_solid && !chord.all_solid);
    assert!(chord.fraction >= 1.0 && chord.end_pos.abs_diff_eq(goal, DIST_EPSILON));
    // The whole authored room is flat. Sample the full leg, including endpoints.
    for sample in 0..=32_u16 {
        let mut point = from.lerp(goal, f32::from(sample) / 32.0);
        point.z = from.z;
        let floor = collision.trace(body.hull, point, point - Vec3::Z);
        assert!(!floor.start_solid && !floor.all_solid);
        assert!(floor.fraction < 1.0 && floor.plane_normal.z > 0.9);
    }
}

#[test]
fn an_entered_blocked_follower_moves_toward_a_new_supported_player_goal() {
    // Existing generated room: floor, tall full-width x=0 wall, no node entities.
    let standing_z = Hull::Standing.foot_offset() + DIST_EPSILON;
    let entities = script_room_entities(
        [-128.0, 0.0, standing_z],
        &entity_block("monster_scientist", [-96.0, 0.0, 0.0], 180.0, &[]),
    );
    let bytes = ai_room_bsp(&entities, true);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());
    let mut game = Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("authored room");
    let follower = entity_of_classname(&game, "monster_scientist").expect("scientist");
    assert!(actor(&game, follower).alive && game.player_health() > 0.0);

    game.tick(TICK_SECONDS, &use_input());
    for _ in 0..4 {
        if game.followers() == [follower] {
            break;
        }
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert_eq!(
        game.followers(),
        &[follower],
        "ordinary Use recruits the follower"
    );

    let old_center = Vec3::new(160.0, 0.0, standing_z);
    place_player_eye(&mut game, old_center);
    game.tick(TICK_SECONDS, &Input::default());
    let old_raw_goal = actor(&game, game.player_entity()).navigation_anchor();
    assert!(old_raw_goal.abs_diff_eq(
        old_center - Vec3::Z * Hull::Standing.foot_offset(),
        DIST_EPSILON
    ));
    let body = actor(&game, follower);
    let old_query_goal = body.body_frame.anchor_to_query(body.hull, old_raw_goal);
    let old_chord =
        game.collision()
            .expect("collision")
            .trace(body.hull, body.query_origin(), old_query_goal);
    assert!(!old_chord.start_solid && old_chord.fraction < 1.0);

    let mut consecutive_task_zero = 0;
    let mut retained_route = None;
    let mut walked_old_leg = false;
    for _ in 0..600 {
        game.tick(TICK_SECONDS, &Input::default());
        let ai = follower_state(&game, follower);
        assert!(actor(&game, follower).alive && ai.memory.is_none());
        assert_eq!(game.followers(), &[follower]);
        if ai
            .runner
            .schedule()
            .is_some_and(|schedule| std::ptr::eq(schedule, &raw const FOLLOW_PLAYER))
            && ai.runner.task_index() == 3
            && ai.move_speed > 0.0
        {
            walked_old_leg = true;
        }
        let entered = ai
            .runner
            .schedule()
            .is_some_and(|schedule| std::ptr::eq(schedule, &raw const FOLLOW_PLAYER))
            && ai.runner.task_index() == 0
            && !ai.runner.started()
            && ai.stuck.ticks() >= STUCK_TICKS
            && ai.conditions.contains(Conditions::BLOCKED)
            && !ai.route.is_finished()
            && ai.move_speed > 0.0;
        if entered {
            if let Some(route) = &retained_route {
                assert_eq!(
                    &ai.route, route,
                    "same-goal reselection retains the old route"
                );
            } else {
                retained_route = Some(ai.route.clone());
            }
            consecutive_task_zero += 1;
            if consecutive_task_zero > STUCK_TICKS {
                break;
            }
        } else {
            consecutive_task_zero = 0;
        }
    }
    assert!(
        walked_old_leg,
        "ordinary tasks must have started the old walking leg"
    );
    assert!(
        consecutive_task_zero > STUCK_TICKS,
        "enter and retain the task-0 BLOCKED selection loop before retargeting"
    );
    let blocked = follower_state(&game, follower);
    let start = actor(&game, follower);
    assert!(
        blocked
            .route
            .goal
            .abs_diff_eq(old_query_goal - Vec3::X * 96.0, DIST_EPSILON)
    );
    let old_leg = game.collision().expect("collision").trace(
        start.hull,
        start.query_origin(),
        blocked.route.goal,
    );
    assert!(!old_leg.start_solid && old_leg.fraction < 1.0);

    let new_center = Vec3::new(-160.0, 160.0, standing_z);
    let new_raw_goal = new_center - Vec3::Z * Hull::Standing.foot_offset();
    assert!((new_raw_goal - old_raw_goal).length() > ROUTE_REFRESH_DISTANCE + WAYPOINT_TOLERANCE);
    let new_query_goal = start.body_frame.anchor_to_query(start.hull, new_raw_goal);
    let delta = new_query_goal - start.query_origin();
    assert!(delta.length() > 96.0 + WAYPOINT_TOLERANCE + 32.0);
    assert_supported_clear_leg(&game, start, new_query_goal);
    let toward = Vec3::new(delta.x, delta.y, 0.0).normalize();
    let walk_step = blocked.move_speed * TICK_SECONDS;
    assert!(walk_step.is_finite() && walk_step > 0.0);
    // Six stuck windows leave ample ordinary task setup and >32 units of walking.
    let recovery_ticks = 6 * STUCK_TICKS + 8;
    assert!(f32::from(u16::try_from(6 * STUCK_TICKS).expect("bounded")) * walk_step > 32.0);
    place_player_eye(&mut game, new_center);
    let mut previous = start.query_origin();
    for _ in 0..recovery_ticks {
        game.tick(TICK_SECONDS, &Input::default());
        assert_eq!(game.followers(), &[follower]);
        assert!(
            actor(&game, game.player_entity())
                .navigation_anchor()
                .abs_diff_eq(new_raw_goal, DIST_EPSILON)
        );
        let current = actor(&game, follower);
        let position = current.query_origin();
        assert!(current.alive && game.player_health() > 0.0);
        assert!(
            position.is_finite() && (position.z - start.query_origin().z).abs() <= DIST_EPSILON
        );
        assert!(
            (position - previous).length() <= walk_step + DIST_EPSILON,
            "ordinary movement cannot teleport"
        );
        let trace = game
            .collision()
            .expect("collision")
            .trace(current.hull, previous, position);
        assert!(!trace.start_solid && !trace.all_solid && trace.fraction >= 1.0);
        previous = position;
    }
    assert!(
        (previous - start.query_origin()).dot(toward) > 32.0,
        "an entered blocked follower must make bounded real progress toward the new supported player goal"
    );
}
