//! Real actor attachment and movement over project-generated BSP/MDL inputs.

use std::collections::BTreeMap;

use ohl_ai::{
    Actor, AiWorld, BodyFrame, Classification, DefaultBrain, MonsterAi, MonsterKind, MonsterSpawn,
    NavBridge, NavBridgeLimits, Route, ScriptHold, SightContext, Vec3, attach_monsters,
};
use ohl_formats::bsp30::{Bsp, Entity as RawEntity, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, build_minimal_mdl10};
use ohl_game::{EntityDef, Registry};
use ohl_nav::{BuildLimits, NodeKind, NodeSeed};
use ohl_physics::{CollisionModel, Hull};

fn collision(brushes: &[CollisionBrush]) -> CollisionModel {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\"classname\" \"worldspawn\"}");
    let heads = builder.push_collision_hulls(brushes);
    builder.push_model(
        [-512.0, -512.0, -256.0],
        [512.0, 512.0, 512.0],
        [0.0; 3],
        heads,
        2,
        0,
        0,
    );
    let bytes = builder.build();
    let bsp = Bsp::parse(&bytes, &Limits::default()).expect("synthetic BSP");
    CollisionModel::from_bsp(&bsp, &Limits::default()).expect("synthetic hulls")
}

fn floor() -> CollisionBrush {
    CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)
}

fn map_actor(hull: Hull, anchor: Vec3, goal: Vec3) -> (AiWorld, Registry, hecs::Entity) {
    let raw = RawEntity::from([
        ("classname".to_string(), "monster_barney".to_string()),
        (
            "origin".to_string(),
            format!("{} {} {}", anchor.x, anchor.y, anchor.z),
        ),
    ]);
    let defs = ohl_game::keyvalues::parse_entities(&[raw], &ohl_game::keyvalues::Limits::default());
    let mut registry = Registry::build(
        &defs,
        &BTreeMap::new(),
        &ohl_game::keyvalues::Limits::default(),
    );
    let mut ai = AiWorld::new(19);
    let brain = ai.register_brain(Box::new(DefaultBrain::default()));
    let actors = attach_monsters(&mut registry, &defs, &|_: &EntityDef| {
        Some(MonsterSpawn::new(Classification::PlayerAlly, brain).with_hull(hull))
    });
    let entity = actors[0];
    registry
        .world
        .insert_one(entity, ScriptHold)
        .expect("script owns route");
    {
        let mut monster = registry.world.get::<&mut MonsterAi>(entity).expect("AI");
        let actor = registry.world.get::<&Actor>(entity).expect("actor");
        monster.route = Route::straight_line(actor.body_frame.anchor_to_query(hull, goal));
        monster.move_speed = 100.0;
    }
    (ai, registry, entity)
}

#[test]
fn map_feet_walkers_move_without_lifting_the_authored_anchor() {
    let collision = collision(&[floor()]);
    for hull in [Hull::Standing, Hull::Large, Hull::Crouched] {
        let (mut ai, mut registry, entity) = map_actor(hull, Vec3::ZERO, Vec3::X * 100.0);
        let actor = *registry.world.get::<&Actor>(entity).expect("actor");
        assert_eq!(actor.origin, Vec3::ZERO);
        assert!(
            !collision
                .trace(hull, actor.query_origin(), actor.query_origin())
                .start_solid
        );
        assert_eq!(
            actor
                .body_frame
                .world_bounds(hull, actor.origin)
                .0
                .z
                .to_bits(),
            0.0_f32.to_bits()
        );
        for _ in 0..50 {
            ai.tick(
                &mut registry.world,
                &SightContext::tracing(&collision),
                0.01,
            );
        }
        let actor = registry.world.get::<&Actor>(entity).expect("actor");
        assert!(actor.origin.x > 40.0, "{hull:?} moves from its feet spawn");
        assert!(
            actor.origin.z.abs() < 0.1,
            "{hull:?} keeps feet on the floor"
        );
    }
}

#[test]
fn actor_step_returns_feet_and_respects_clearance_for_each_box_hull() {
    for hull in [Hull::Standing, Hull::Large, Hull::Crouched] {
        let collision = collision(&[
            floor(),
            CollisionBrush::box_brush([0.0, -256.0, 0.0], [100.0, 256.0, 12.0]),
        ]);
        let (mut ai, mut registry, entity) = map_actor(
            hull,
            Vec3::new(-100.0, 0.0, 0.0),
            Vec3::new(80.0, 0.0, 12.0),
        );
        for _ in 0..140 {
            ai.tick(
                &mut registry.world,
                &SightContext::tracing(&collision),
                0.01,
            );
        }
        let actor = registry.world.get::<&Actor>(entity).expect("actor");
        assert!(actor.origin.x > 20.0, "{hull:?} crosses low step");
        assert!(
            (actor.origin.z - 12.0).abs() < 0.1,
            "{hull:?} result is feet-relative"
        );
    }
}

#[test]
fn a_low_ceiling_blocks_tall_proxies_but_admits_the_crouched_body() {
    let collision = collision(&[
        floor(),
        CollisionBrush::box_brush([0.0, -256.0, 48.0], [200.0, 256.0, 256.0]),
    ]);
    for hull in [Hull::Standing, Hull::Large, Hull::Crouched] {
        let (mut ai, mut registry, entity) = map_actor(
            hull,
            Vec3::new(-100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
        );
        for _ in 0..200 {
            ai.tick(
                &mut registry.world,
                &SightContext::tracing(&collision),
                0.01,
            );
        }
        let actor = registry.world.get::<&Actor>(entity).expect("actor");
        if hull == Hull::Crouched {
            assert!(actor.origin.x > 80.0, "short proxy fits below the ceiling");
        } else {
            assert!(actor.origin.x < 0.0, "tall proxy stops before the ceiling");
        }
        assert!(actor.origin.z.abs() < 0.1);
    }
}

#[test]
fn cover_routes_keep_feet_on_the_floor_and_stop_before_the_wall() {
    use ohl_ai::{Brain, Conditions, MonsterState, Schedule, Task};
    static COVER: Schedule = Schedule::new(
        "test/feet_cover",
        &[
            Task::FindCover,
            Task::TakeCover,
            Task::RunPath,
            Task::WaitForMovement,
            Task::Wait(100.0),
        ],
        Conditions::EMPTY,
    );
    struct CoverBrain;
    impl Brain for CoverBrain {
        fn classification(&self) -> Classification {
            Classification::None
        }
        fn select_schedule(&self, _: MonsterState, _: Conditions) -> &'static Schedule {
            &COVER
        }
    }
    let collision = collision(&[
        floor(),
        CollisionBrush::box_brush([-120.0, -256.0, 0.0], [-100.0, 256.0, 200.0]),
    ]);
    let mut ai = AiWorld::new(8);
    let brain = ai.register_brain(Box::new(CoverBrain));
    let mut world = hecs::World::new();
    let entity = ohl_ai::spawn_monster(
        &mut world,
        Actor::new(Classification::None, Vec3::ZERO),
        brain,
    );
    world.get::<&mut MonsterAi>(entity).expect("AI").move_target = Some(Vec3::X * 100.0);
    for _ in 0..200 {
        ai.tick(&mut world, &SightContext::tracing(&collision), 0.01);
    }
    let actor = world.get::<&Actor>(entity).expect("actor");
    let state = world.get::<&MonsterAi>(entity).expect("AI");
    let cover = state.cover.expect("cover goal");
    assert!(
        cover.x < -50.0 && cover.x > -85.0,
        "cover reaches the near wall face"
    );
    assert!(
        (cover.z - actor.query_origin().z).abs() < 0.1,
        "saved goal is query-relative"
    );
    assert!(actor.origin.x < -50.0 && actor.origin.x > -85.0);
    assert!(actor.origin.z.abs() < 0.1);
    assert!(state.route.is_finished());
}

#[test]
fn feet_endpoint_attachment_uses_ground_nodes_exactly_once() {
    let collision = collision(&[
        floor(),
        CollisionBrush::box_brush([-16.0, -100.0, 0.0], [16.0, 100.0, 256.0]),
    ]);
    let coords = [-256.0, -128.0, 0.0, 128.0, 256.0];
    let seeds: Vec<_> = coords
        .iter()
        .flat_map(|&x| {
            coords
                .iter()
                .map(move |&y| NodeSeed::new(Vec3::new(x, y, 8.0), NodeKind::Ground))
        })
        .collect();
    let (mut ai, mut registry, entity) = map_actor(
        Hull::Standing,
        Vec3::new(-250.0, 0.0, 0.0),
        Vec3::new(250.0, 0.0, 0.0),
    );
    ai.attach_navigator(NavBridge::build(
        &seeds,
        &collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    ));
    let mut around = false;
    for _ in 0..1_500 {
        ai.tick(
            &mut registry.world,
            &SightContext::tracing(&collision),
            0.01,
        );
        let actor = registry.world.get::<&Actor>(entity).expect("actor");
        around |= actor.origin.y.abs() > 110.0;
        assert!(actor.origin.z.abs() < 0.2);
    }
    assert!(around, "route used the open wall end");
    assert!(
        registry
            .world
            .get::<&Actor>(entity)
            .expect("actor")
            .origin
            .x
            > 230.0
    );
    assert_eq!(ai.navigator().expect("bridge").stats().untraced_steps, 0);
}

fn studio(
    eye: [f32; 3],
    hull_min: [f32; 3],
    hull_max: [f32; 3],
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
) -> ohl_world::StudioModel {
    let (mut bytes, _) = build_minimal_mdl10();
    for (offset, value) in [eye, hull_min, hull_max, bounds_min, bounds_max]
        .into_iter()
        .flatten()
        .enumerate()
    {
        let start = 76 + offset * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_le_bytes());
    }
    ohl_world::StudioModel::parse(&bytes, &ohl_formats::mdl10::Limits::default())
        .expect("synthetic MDL")
}

#[test]
fn point_flight_keeps_its_anchor_and_hits_the_ceiling_without_floor_snapping() {
    let collision = collision(&[
        floor(),
        CollisionBrush::box_brush([-256.0, -256.0, 160.0], [256.0, 256.0, 200.0]),
    ]);
    // A diagonal approach reaches the ceiling before horizontal arrival.
    // Pure vertical route completion uses the existing XY-only tolerance,
    // a separate P5 arrival limitation, independent of this identity frame.
    let (mut ai, mut registry, entity) =
        map_actor(Hull::Point, Vec3::Z * 100.0, Vec3::new(120.0, 0.0, 220.0));
    registry
        .world
        .get::<&mut Actor>(entity)
        .expect("actor")
        .configure_model(&MonsterKind::Apache, None);
    for _ in 0..200 {
        ai.tick(
            &mut registry.world,
            &SightContext::tracing(&collision),
            0.01,
        );
    }
    let actor = registry.world.get::<&Actor>(entity).expect("actor");
    assert!(actor.origin.z > 150.0 && actor.origin.z < 160.0);
    assert!(actor.origin.x > 50.0 && actor.origin.x < 70.0);
    assert_eq!(actor.query_origin(), actor.origin);
    let (min, max) = actor.fallback_damage_bounds();
    assert!(
        (max - min).min_element() > 0.0,
        "point movement still has damage volume"
    );
}

#[test]
fn model_hull_eye_and_clipping_metadata_remain_distinct() {
    let model = studio(
        [10.0, 2.0, 20.0],
        [-16.0, -16.0, -36.0],
        [16.0, 16.0, 36.0],
        [-6.0, -10.0, 0.0],
        [8.0, 12.0, 144.0],
    );
    let mut actor = Actor::new(Classification::None, Vec3::new(20.0, 30.0, 40.0)).facing(90.0);
    actor.configure_model(&MonsterKind::Generic, Some(&model));
    assert_eq!(
        actor.query_origin(),
        actor.origin,
        "centered custom pivot uses header hull, not clipping bounds"
    );
    assert!(actor.eye().abs_diff_eq(Vec3::new(18.0, 40.0, 60.0), 0.001));
    let player = actor.as_client().facing(90.0);
    assert_eq!(player.eye(), player.origin + Vec3::Z * 28.0);
    let mut ceiling = Actor::new(Classification::Barnacle, Vec3::Z * 200.0);
    ceiling.configure_model(&MonsterKind::Barnacle, Some(&model));
    assert_eq!(ceiling.body_frame, BodyFrame::Ceiling);
    assert!(
        ceiling.eye().z < ceiling.origin.z,
        "upward metadata does not move the mouth above its ceiling"
    );
}

#[test]
fn centered_custom_model_moves_at_its_pivot_without_using_clipping_height() {
    let model = studio(
        [0.0; 3],
        [-16.0, -16.0, -36.0],
        [16.0, 16.0, 36.0],
        [-10.0, -10.0, 0.0],
        [10.0, 10.0, 144.0],
    );
    let collision = collision(&[floor()]);
    let (mut ai, mut registry, entity) =
        map_actor(Hull::Standing, Vec3::Z * 36.0, Vec3::new(100.0, 0.0, 36.0));
    registry
        .world
        .get::<&mut Actor>(entity)
        .expect("actor")
        .configure_model(&MonsterKind::Generic, Some(&model));
    for _ in 0..100 {
        ai.tick(
            &mut registry.world,
            &SightContext::tracing(&collision),
            0.01,
        );
    }
    let actor = registry.world.get::<&Actor>(entity).expect("actor");
    assert!(actor.origin.x > 80.0);
    assert!(
        (actor.origin.z - 36.0).abs() < 0.1,
        "centered model pivot stays above its feet"
    );
    assert!(
        actor
            .body_frame
            .world_bounds(actor.hull, actor.origin)
            .0
            .z
            .abs()
            < 0.1
    );
}

#[test]
fn metadata_fallback_tracks_short_and_tall_model_bounds_and_rejects_extremes() {
    for height in [18.0, 144.0] {
        let model = studio(
            [0.0; 3],
            [0.0; 3],
            [0.0; 3],
            [-4.0, -8.0, 0.0],
            [8.0, 4.0, height],
        );
        let mut actor = Actor::new(Classification::None, Vec3::ZERO);
        actor.configure_model(&MonsterKind::Zombie, Some(&model));
        assert!(
            actor
                .eye()
                .abs_diff_eq(Vec3::new(2.0, -2.0, height * (8.0 / 9.0)), 0.001)
        );
        assert_eq!(
            actor.query_origin().z.to_bits(),
            36.0_f32.to_bits(),
            "clipping height never selects a new movement hull"
        );
    }
    let invalid = studio(
        [f32::MAX; 3],
        [f32::NEG_INFINITY; 3],
        [f32::INFINITY; 3],
        [-f32::MAX; 3],
        [f32::MAX; 3],
    );
    let mut actor = Actor::new(Classification::None, Vec3::ZERO);
    actor.configure_model(&MonsterKind::Generic, Some(&invalid));
    assert!(actor.eye().is_finite());
    assert_eq!(actor.query_origin().z.to_bits(), 36.0_f32.to_bits());
}

#[test]
fn player_stances_publish_the_same_floor_goal() {
    for hull in [Hull::Standing, Hull::Crouched] {
        let mut actor =
            Actor::new(Classification::Player, Vec3::Z * hull.foot_offset()).as_client();
        actor.hull = hull;
        assert_eq!(actor.navigation_anchor(), Vec3::ZERO);
        assert_eq!(actor.query_origin(), actor.origin);
    }
}

#[test]
fn perception_rotates_model_local_eyes_before_tracing() {
    let collision = collision(&[
        floor(),
        CollisionBrush::box_brush([10.0, 30.0, 0.0], [40.0, 80.0, 100.0]),
    ]);
    let mut ai = AiWorld::new(3);
    let brain = ai.register_brain(Box::new(DefaultBrain::ranged(
        Classification::HumanMilitary,
    )));
    let mut world = hecs::World::new();
    let mut viewer = Actor::new(Classification::HumanMilitary, Vec3::ZERO).facing(90.0);
    viewer.view_ofs = Vec3::new(50.0, 0.0, 40.0);
    let viewer = ohl_ai::spawn_monster(&mut world, viewer, brain);
    let mut target = Actor::new(Classification::Player, Vec3::new(0.0, 150.0, 40.0)).as_client();
    target.view_ofs = Vec3::ZERO;
    let target = ohl_ai::spawn_actor(&mut world, target);
    let events = ai.tick(&mut world, &SightContext::tracing(&collision), 0.01);
    assert!(
        events.iter().any(|event| event.entity == viewer
            && event.kind == ohl_ai::AiEventKind::EnemyAcquired(target)),
        "the rotated eye sees around the synthetic obstacle"
    );
    // Reverse the experiment: a candidate's model-local eye must also be
    // converted before it enters the sensory snapshot.
    let mut target_actor = world.get::<&mut Actor>(target).expect("target");
    target_actor.is_client = false;
    target_actor.yaw = 90.0;
    target_actor.origin = Vec3::new(0.0, 100.0, 0.0);
    target_actor.view_ofs = Vec3::new(50.0, 0.0, 40.0);
    drop(target_actor);
    let mut viewer_actor = world.get::<&mut Actor>(viewer).expect("viewer");
    viewer_actor.view_ofs = Vec3::Z * 40.0;
    drop(viewer_actor);
    world.get::<&mut MonsterAi>(viewer).expect("AI").memory = None;
    let events = ai.tick(&mut world, &SightContext::tracing(&collision), 0.01);
    assert!(
        events.iter().any(|event| event.entity == viewer
            && event.kind == ohl_ai::AiEventKind::EnemyAcquired(target))
    );
}
