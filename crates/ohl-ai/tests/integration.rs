//! Integration tests for the monster AI core.
//!
//! Everything runs against project-authored synthetic fixtures — a BSP room
//! built by `ohl_formats::test_support` and, for the visibility pre-filter,
//! `ohl_world::test_support`'s synthetic room. No game data is loaded.

use ohl_ai::brain::{ALERT_STAND, IDLE_STAND};
use ohl_ai::{
    Actor, AiEventKind, AiWorld, Brain, BrainId, Classification, Conditions, DamageEvent,
    MonsterAi, MonsterState, Schedule, Senses, SightContext, SoundEvent, SoundKind, SquadTag, Task,
    Vec3, spawn_actor, spawn_monster, spawn_squad_monster,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_physics::CollisionModel;
use ohl_world::{WorldBuildOptions, WorldModel};

const DT: f32 = 0.01;

/// A room spanning `[-512, 512]` on X and Y and `[0, 256]` on Z, with a
/// solid wall filling `x` in `-16..16` for `y` in `-512..512`, so the two
/// halves of the room have no line of sight to each other.
fn divided_room_bsp() -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
        CollisionBrush::box_brush([-16.0, -512.0, -16.0], [16.0, 512.0, 256.0]),
    ]);
    builder.push_model(
        [-512.0, -512.0, 0.0],
        [512.0, 512.0, 256.0],
        [0.0, 0.0, 0.0],
        heads,
        2,
        0,
        0,
    );
    builder.build()
}

fn divided_room() -> CollisionModel {
    let bytes = divided_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("the fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("the fixture has usable collision hulls")
}

/// A scripted brain: it names the schedule it wants per state and records
/// nothing else, so a test can assert about selection without depending on
/// the crate's default schedule set.
struct TestBrain {
    classification: Classification,
    senses: Senses,
}

impl TestBrain {
    fn new(classification: Classification) -> Self {
        Self {
            classification,
            senses: Senses::default(),
        }
    }
}

static TEST_ENGAGE: Schedule = Schedule::new(
    "test/engage",
    &[Task::FaceEnemy, Task::MeleeAttack1, Task::Wait(0.1)],
    Conditions::ENEMY_OCCLUDED
        .union(Conditions::HEAR_DANGER)
        .union(Conditions::GENERAL_INTERRUPTS),
);

static TEST_COWER: Schedule = Schedule::new(
    "test/cower",
    &[Task::StopMoving, Task::Wait(0.25)],
    Conditions::EMPTY,
);

impl Brain for TestBrain {
    fn classification(&self) -> Classification {
        self.classification
    }

    fn senses(&self) -> Senses {
        self.senses
    }

    fn select_schedule(&self, state: MonsterState, conditions: Conditions) -> &'static Schedule {
        if conditions.contains(Conditions::HEAR_DANGER) {
            return &TEST_COWER;
        }
        match state {
            MonsterState::Combat => &TEST_ENGAGE,
            MonsterState::Alert | MonsterState::Hunt => &ALERT_STAND,
            _ => &IDLE_STAND,
        }
    }
}

fn scripted_world() -> (AiWorld, hecs::World, BrainId) {
    let mut ai = AiWorld::new(0x00A1_5EED_0000_0001);
    let brain = ai.register_brain(Box::new(TestBrain::new(Classification::HumanMilitary)));
    (ai, hecs::World::new(), brain)
}

#[test]
fn line_of_sight_flips_idle_to_combat_within_one_tick() {
    let collision = divided_room();
    let (mut ai, mut world, brain) = scripted_world();
    // Both on the same side of the wall, facing each other.
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-200.0, 0.0, 36.0)),
        brain,
    );
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-80.0, 0.0, 36.0))
            .as_client()
            .facing(180.0),
    );

    let events = ai.tick(&mut world, &SightContext::tracing(&collision), DT);
    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert_eq!(state.state, MonsterState::Combat);
    assert!(state.conditions.contains(Conditions::SEE_ENEMY));
    assert_eq!(state.schedule_name(), "test/engage");
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, AiEventKind::EnemyAcquired(_)))
    );
}

#[test]
fn the_wall_blocks_sight_that_the_open_room_allows() {
    let collision = divided_room();
    let (mut ai, mut world, brain) = scripted_world();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-100.0, 0.0, 36.0)),
        brain,
    );
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(100.0, 0.0, 36.0)).as_client(),
    );

    ai.tick(&mut world, &SightContext::tracing(&collision), DT);
    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert!(!state.conditions.contains(Conditions::SEE_ENEMY));
    assert_eq!(state.state, MonsterState::Idle);
    assert!(state.enemy().is_none());
}

#[test]
fn occlusion_drops_to_alert_and_keeps_the_last_known_position() {
    let collision = divided_room();
    let (mut ai, mut world, brain) = scripted_world();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-100.0, 0.0, 36.0)),
        brain,
    );
    let player = spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-40.0, 60.0, 36.0)).as_client(),
    );

    // Seen first, in the open on the monster's own side of the wall.
    ai.tick(&mut world, &SightContext::tracing(&collision), DT);
    {
        let state = world.get::<&MonsterAi>(monster).expect("component");
        assert_eq!(state.state, MonsterState::Combat);
        assert_eq!(state.enemy(), Some(player));
    }

    // Now step the player behind the wall, still within 256 units.
    world.get::<&mut Actor>(player).expect("component").origin = Vec3::new(60.0, 60.0, 36.0);
    ai.tick(&mut world, &SightContext::tracing(&collision), DT);

    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert!(state.conditions.contains(Conditions::ENEMY_OCCLUDED));
    assert!(!state.conditions.contains(Conditions::SEE_ENEMY));
    assert_eq!(state.state, MonsterState::Alert);
    assert_eq!(state.enemy(), Some(player), "the enemy is still tracked");
    assert_eq!(
        state.last_known_position(),
        Some(Vec3::new(-40.0, 60.0, 36.0)),
        "the last position it was actually seen at is retained"
    );
}

#[test]
fn an_enemy_occluded_beyond_the_memory_distance_is_forgotten() {
    let collision = divided_room();
    let (mut ai, mut world, brain) = scripted_world();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-100.0, 0.0, 36.0)),
        brain,
    );
    let player = spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-40.0, 60.0, 36.0)).as_client(),
    );
    ai.tick(&mut world, &SightContext::tracing(&collision), DT);
    // Far behind the wall: more than 256 units away and out of sight.
    world.get::<&mut Actor>(player).expect("component").origin = Vec3::new(400.0, 60.0, 36.0);
    ai.tick(&mut world, &SightContext::tracing(&collision), DT);

    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert!(state.conditions.contains(Conditions::ENEMY_OCCLUDED));
    assert!(state.enemy().is_none());
    assert_eq!(state.state, MonsterState::Alert);
}

#[test]
fn a_danger_sound_interrupts_the_running_schedule() {
    let collision = divided_room();
    let (mut ai, mut world, brain) = scripted_world();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-200.0, 0.0, 36.0)),
        brain,
    );
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-80.0, 0.0, 36.0)).as_client(),
    );

    let context = SightContext::tracing(&collision);
    ai.tick(&mut world, &context, DT);
    assert_eq!(
        world
            .get::<&MonsterAi>(monster)
            .expect("component")
            .schedule_name(),
        "test/engage"
    );

    ai.emit_sound(
        SoundEvent::new(SoundKind::Danger, Vec3::new(-190.0, 0.0, 36.0), 300.0).lasting(1.0),
    );
    let events = ai.tick(&mut world, &context, DT);

    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert!(state.conditions.contains(Conditions::HEAR_DANGER));
    assert_eq!(state.schedule_name(), "test/cower");
    assert!(events.iter().any(|event| matches!(
        event.kind,
        AiEventKind::ScheduleEnded {
            name: "test/engage",
            outcome: ohl_ai::RunOutcome::Interrupted,
        }
    )));
}

#[test]
fn a_squad_leader_recruits_three_members_and_no_more() {
    let (mut ai, mut world, brain) = scripted_world();
    let leader = spawn_squad_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::ZERO),
        brain,
        SquadTag::leader("hgrunt_squad"),
    );
    let followers: Vec<_> = (1u8..=6)
        .map(|index| {
            spawn_squad_monster(
                &mut world,
                Actor::new(
                    Classification::HumanMilitary,
                    Vec3::new(0.0, 32.0 * f32::from(index), 0.0),
                ),
                brain,
                SquadTag::member("hgrunt_squad"),
            )
        })
        .collect();

    ai.tick(&mut world, &SightContext::empty(), DT);

    let squad = ai.squads().get("hgrunt_squad").expect("the squad formed");
    assert_eq!(squad.leader, leader);
    assert_eq!(squad.recruit_count(), ohl_ai::MAX_RECRUITS);
    assert_eq!(squad.members.len(), ohl_ai::MAX_SQUAD_SIZE);
    assert_eq!(
        ai.squads().rejected().len(),
        followers.len() - ohl_ai::MAX_RECRUITS
    );
    for member in &followers[..ohl_ai::MAX_RECRUITS] {
        assert!(squad.contains(*member));
    }
    for member in &followers[ohl_ai::MAX_RECRUITS..] {
        assert!(!squad.contains(*member));
        assert!(ai.squads().squad_of(*member).is_none());
    }
}

#[test]
fn a_fixed_seed_replays_one_thousand_ticks_bit_identically() {
    let collision = divided_room();
    let run = || {
        let (mut ai, mut world, brain) = scripted_world();
        let hunter = spawn_monster(
            &mut world,
            Actor::new(
                Classification::HumanMilitary,
                Vec3::new(-300.0, -200.0, 36.0),
            ),
            brain,
        );
        spawn_squad_monster(
            &mut world,
            Actor::new(
                Classification::HumanMilitary,
                Vec3::new(-300.0, 200.0, 36.0),
            ),
            brain,
            SquadTag::leader("bravo"),
        );
        let player = spawn_actor(
            &mut world,
            Actor::new(Classification::Player, Vec3::new(-120.0, 0.0, 36.0)).as_client(),
        );

        let context = SightContext::tracing(&collision);
        let mut digests = Vec::new();
        for tick in 0..1_000 {
            if tick % 211 == 0 {
                ai.emit_sound(SoundEvent::new(
                    SoundKind::Combat,
                    Vec3::new(-200.0, 0.0, 36.0),
                    600.0,
                ));
            }
            if tick % 337 == 0 {
                ai.apply_damage(DamageEvent::new(
                    hunter,
                    player,
                    7.0,
                    Vec3::new(-120.0, 0.0, 36.0),
                ));
            }
            if tick == 500 {
                world.get::<&mut Actor>(player).expect("component").origin =
                    Vec3::new(-400.0, 300.0, 36.0);
            }
            ai.tick(&mut world, &context, DT);
            if tick % 100 == 0 {
                digests.push(ai.state_hash(&world));
            }
        }
        digests.push(ai.state_hash(&world));
        digests
    };

    let first = run();
    let second = run();
    assert_eq!(first, second);
    // The run must actually do something, or the assertion above is vacuous.
    assert!(first.windows(2).any(|pair| pair[0] != pair[1]));
}

#[test]
fn the_visibility_pre_filter_never_hides_what_the_trace_would_see() {
    // `ohl-world`'s synthetic room is one convex leaf, so its PVS admits
    // everything; the point of this test is that wiring a `WorldModel` in
    // does not change an answer the trace already gave.
    let bytes = ohl_world::test_support::synthetic_room_bsp();
    let wad = ohl_world::test_support::synthetic_room_wad();
    let wads = [wad.as_slice()];
    let model = WorldModel::build(
        &Bsp::parse(&bytes, &Limits::default()).expect("the fixture parses"),
        &WorldBuildOptions {
            wads: &wads,
            limits: Limits::default(),
            ..Default::default()
        },
    )
    .expect("the fixture builds a world model");

    let (mut ai, mut world, brain) = scripted_world();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-64.0, 0.0, 8.0)),
        brain,
    );
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(64.0, 0.0, 8.0)).as_client(),
    );

    let context = SightContext {
        collision: None,
        world: Some(&model),
    };
    ai.tick(&mut world, &context, DT);
    let state = world.get::<&MonsterAi>(monster).expect("component");
    assert!(state.conditions.contains(Conditions::SEE_ENEMY));
}

#[test]
fn a_monster_walks_toward_its_enemy_without_leaving_the_room() {
    let collision = divided_room();
    let mut ai = AiWorld::new(7);
    let brain = ai.register_brain(Box::new(ohl_ai::DefaultBrain::melee(
        Classification::HumanMilitary,
    )));
    let mut world = hecs::World::new();
    let monster = spawn_monster(
        &mut world,
        Actor::new(Classification::HumanMilitary, Vec3::new(-400.0, 0.0, 36.0)),
        brain,
    );
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-100.0, 0.0, 36.0)).as_client(),
    );

    let context = SightContext::tracing(&collision);
    let start = world.get::<&Actor>(monster).expect("component").origin;
    for _ in 0..600 {
        ai.tick(&mut world, &context, DT);
    }
    let actor = *world.get::<&Actor>(monster).expect("component");
    assert!(actor.origin.x > start.x, "the monster closed the distance");
    assert!(actor.origin.x < 512.0 && actor.origin.x > -512.0);
    assert!(actor.origin.is_finite());
}

/// Wave 1 batch B: a Gonarch with a trail walks its legs through the
/// ordinary tick, ignoring the player it can see on the way, arrives at a
/// node with health and only then turns to fight.
#[test]
fn a_gonarch_walks_its_trail_through_the_tick_and_fights_only_on_arrival() {
    use ohl_ai::MonsterKind;
    use ohl_ai::monsters::bigmomma::{TrailNode, TrailPhase};
    use ohl_ai::{GonarchTrail, MonsterBrain};

    let collision = divided_room();
    let mut ai = AiWorld::new(11);
    let gonarch_brain = ai.register_brain(Box::new(
        MonsterBrain::for_kind(MonsterKind::BigMomma).expect("defined"),
    ));
    let mut world = hecs::World::new();

    // The trail runs along the left half of the room, away from the wall.
    let node = |name: &str, x: f32, next: Option<&str>, health: Option<f32>| TrailNode {
        name: name.to_string(),
        position: Vec3::new(x, 0.0, 36.0),
        next: next.map(str::to_string),
        health,
        wait: 0.0,
        fire_on_reach: None,
        kill_on_reach: None,
        sequence_on_reach: None,
        run: true,
        wait_indefinitely: false,
    };
    let trail = ohl_ai::monsters::Trail::new(vec![
        node("a", -300.0, Some("b"), None),
        node("b", -100.0, None, Some(100.0)),
    ]);
    let mut actor =
        Actor::new(Classification::AlienMonster, Vec3::new(-450.0, 0.0, 36.0)).with_health(225.0);
    actor.hull = ohl_physics::Hull::Large;
    let gonarch = world.spawn((
        actor,
        MonsterAi::new(gonarch_brain),
        GonarchTrail::new(trail, 1.5, 225.0),
    ));
    // The player stands right beside the trail's start, in plain sight.
    spawn_actor(
        &mut world,
        Actor::new(Classification::Player, Vec3::new(-450.0, 200.0, 36.0)).as_client(),
    );

    let context = SightContext::tracing(&collision);
    let mut saw_travel = false;
    let mut arrived_tick = None;
    for tick in 0..800 {
        ai.tick(&mut world, &context, DT);
        let state = world.get::<&MonsterAi>(gonarch).expect("component");
        if state.schedule_name() == "ohl/monsters/gonarch_travel" {
            saw_travel = true;
        }
        let phase = world.get::<&GonarchTrail>(gonarch).expect("trail").phase();
        if arrived_tick.is_none() && phase == (TrailPhase::Holding { at: 1 }) {
            arrived_tick = Some(tick);
        }
    }
    assert!(
        saw_travel,
        "the trail leg ran through the brain's own schedule"
    );
    assert!(arrived_tick.is_some(), "the Gonarch reached the last node");
    let actor = *world.get::<&Actor>(gonarch).expect("component");
    assert!(
        (actor.origin.x - (-100.0)).abs() < 64.0,
        "{:?}",
        actor.origin
    );
    assert!(actor.origin.is_finite());
    // The last node's health, scaled by the medium factor, was set on
    // arrival; and once there the brain is free to fight.
    assert!((actor.health - 150.0).abs() < 1e-3);
    let state = world.get::<&MonsterAi>(gonarch).expect("component");
    assert_ne!(state.schedule_name(), "ohl/monsters/gonarch_travel");
    assert!(!state.conditions.contains(Conditions::SPECIAL1));
}

/// A Gonarch whose next node is on the far side of the room's dividing
/// wall cannot reach it: it walks into the wall and stays there. The
/// project-authored stall fallback (`GonarchTrail::note_progress`) counts
/// that leg as arriving after `TRAIL_STALL_SECONDS`, so the node's health
/// is set and the Gonarch, at the last node, is killable rather than
/// shielded for good.
#[test]
fn a_gonarch_that_cannot_reach_its_node_arrives_once_its_leg_stalls() {
    use ohl_ai::MonsterKind;
    use ohl_ai::monsters::bigmomma::{TRAIL_STALL_SECONDS, TrailNode, TrailPhase};
    use ohl_ai::{GonarchTrail, MonsterBrain};

    let collision = divided_room();
    let mut ai = AiWorld::new(13);
    let brain = ai.register_brain(Box::new(
        MonsterBrain::for_kind(MonsterKind::BigMomma).expect("defined"),
    ));
    let mut world = hecs::World::new();
    let trail = ohl_ai::monsters::Trail::new(vec![TrailNode {
        name: "beyond".to_string(),
        position: Vec3::new(300.0, 0.0, 36.0),
        next: None,
        health: Some(100.0),
        wait: 0.0,
        fire_on_reach: None,
        kill_on_reach: None,
        sequence_on_reach: None,
        run: true,
        wait_indefinitely: false,
    }]);
    let mut actor =
        Actor::new(Classification::AlienMonster, Vec3::new(-300.0, 0.0, 36.0)).with_health(150.0);
    actor.hull = ohl_physics::Hull::Large;
    let gonarch = world.spawn((
        actor,
        MonsterAi::new(brain),
        GonarchTrail::new(trail, 1.0, 150.0),
    ));

    let context = SightContext::tracing(&collision);
    let phase = |world: &hecs::World| world.get::<&GonarchTrail>(gonarch).expect("trail").phase();
    // Well short of the stall delay: still travelling, still shielded,
    // and stopped at the wall rather than through it.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let short = (TRAIL_STALL_SECONDS * 0.5 / DT) as usize;
    for _ in 0..short {
        ai.tick(&mut world, &context, DT);
    }
    assert_eq!(phase(&world), TrailPhase::Traveling { to: 0 });
    let origin = world.get::<&Actor>(gonarch).expect("actor").origin;
    assert!(origin.x < -16.0, "{origin:?}");

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rest = (TRAIL_STALL_SECONDS * 1.5 / DT) as usize;
    for _ in 0..rest {
        ai.tick(&mut world, &context, DT);
    }
    assert_eq!(phase(&world), TrailPhase::Holding { at: 0 });
    let actor = *world.get::<&Actor>(gonarch).expect("actor");
    assert!(
        (actor.health - 100.0).abs() < 1e-3,
        "the node's health was set"
    );
    assert!(actor.origin.x < -16.0, "it never crossed the wall");
}

/// An aircraft, through the whole tick against a real collision model: on
/// its point hull the ordinary movement step flies it along its plan in
/// three dimensions (climbing to a higher node), and stops it at a wall
/// rather than carrying it through.
fn fly_an_apache(nodes: Vec<Vec3>, from: Vec3, ticks: usize) -> (Vec3, f32) {
    use ohl_ai::MonsterKind;
    use ohl_ai::monsters::aircraft::FLIGHT_SPEED;
    use ohl_ai::{FlightPlan, MonsterBrain};

    let collision = divided_room();
    let mut ai = AiWorld::new(12);
    let brain = ai.register_brain(Box::new(
        MonsterBrain::for_kind(MonsterKind::Apache).expect("defined"),
    ));
    let mut world = hecs::World::new();
    let spec = ohl_ai::monsters::spec_for(&MonsterKind::Apache).expect("defined");
    let mut actor = Actor::new(Classification::HumanMilitary, from).with_health(250.0);
    actor.hull = spec.hull;
    let apache = world.spawn((
        actor,
        MonsterAi::new(brain),
        FlightPlan::new(nodes, true, FLIGHT_SPEED),
    ));
    let context = SightContext::tracing(&collision);
    let mut highest = f32::NEG_INFINITY;
    for _ in 0..ticks {
        ai.tick(&mut world, &context, DT);
        let origin = world.get::<&Actor>(apache).expect("component").origin;
        assert!(origin.is_finite());
        highest = highest.max(origin.z);
    }
    (
        world.get::<&Actor>(apache).expect("component").origin,
        highest,
    )
}

#[test]
fn an_apache_flies_its_route_in_three_dimensions_through_the_tick() {
    let from = Vec3::new(-400.0, -400.0, 160.0);
    let (origin, highest) = fly_an_apache(vec![Vec3::new(-400.0, 400.0, 240.0), from], from, 300);
    assert!(
        (origin - from).length() > 100.0,
        "it left its spawn point: {origin:?}"
    );
    assert!(
        highest > 230.0,
        "it climbed toward the higher node: {highest}"
    );
    assert!(origin.x < -16.0 && origin.y.abs() <= 512.0);
}

#[test]
fn an_apache_is_stopped_by_a_wall_its_route_runs_through() {
    // The node is on the far side of the room's dividing wall (x in
    // -16..16): the traced flight stops short of it.
    let from = Vec3::new(-400.0, 0.0, 128.0);
    let (origin, _) = fly_an_apache(vec![Vec3::new(400.0, 0.0, 128.0)], from, 400);
    assert!(origin.x < -15.0, "carried through the wall: {origin:?}");
    assert!(origin.x > -100.0, "it did fly to the wall: {origin:?}");
}
