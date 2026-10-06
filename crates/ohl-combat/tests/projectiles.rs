//! Projectile simulation against the project's synthetic collision room.
//!
//! No game data is loaded: the map is `ohl_formats::test_support`'s
//! collision-room fixture and the targets are hand-built hitboxes.

use ohl_combat::projectile::{
    HAND_GRENADE_FUSE_SECONDS, ProjectileEvent, ProjectileKind, ProjectileLimits, ProjectileSet,
    ProjectileTuning, ProjectileWorld, SNARK_LIFETIME_SECONDS,
};
use ohl_combat::{EntityHitboxes, EntityId, HitGroup, HitboxIndex, HitboxLimits, Vec3};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::{CollisionModel, Hull, MoveConfig, contents};

/// The fixed simulation tick the host runs at (100 Hz).
const TICK: f32 = 0.01;

#[test]
// Keep the bounce setup and all departure/rest assertions in one generated test.
#[allow(clippy::too_many_lines)]
fn boundary_contact_real_bounce_departs_after_first_impact() {
    let collision = room();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let movement = MoveConfig {
        gravity: 0.0,
        ..MoveConfig::default()
    };
    let tuning = ProjectileTuning {
        restitution: ohl_combat::BlackBox::new(0.45),
        bounce_friction: ohl_combat::BlackBox::new(0.8),
        rest_speed: ohl_combat::BlackBox::new(30.0),
        ..ProjectileTuning::default()
    };
    for (rotation, start, incoming, contact, normal, expected_velocity) in [
        (
            ohl_combat::Quat::IDENTITY,
            Vec3::new(-16.5, 0.0, 64.0),
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(-16.0, 0.0, 64.0),
            Vec3::NEG_X,
            Vec3::new(-45.0, 0.0, 0.0),
        ),
        (
            ohl_combat::Quat::IDENTITY,
            Vec3::new(0.0, -16.5, 64.0),
            Vec3::new(40.0, 100.0, 0.0),
            Vec3::new(0.2, -16.0, 64.0),
            Vec3::NEG_Y,
            Vec3::new(32.0, -45.0, 0.0),
        ),
        (
            ohl_combat::Quat::from_xyzw(0.0, 0.0, 1.0, 0.0),
            Vec3::new(0.0, -16.5, 64.0),
            Vec3::new(40.0, 100.0, 0.0),
            Vec3::new(0.2, -16.0, 64.0),
            Vec3::NEG_Y,
            Vec3::new(32.0, -45.0, 0.0),
        ),
    ] {
        let mut target = cube_entity(9, center);
        target.rotation = rotation;
        let entities = index_of(vec![target]);
        let world = ProjectileWorld {
            collision: &collision,
            entities: &entities,
            movement: &movement,
            tuning: &tuning,
        };
        let mut set = ProjectileSet::new(ProjectileLimits::default(), 7);
        let id = set
            .spawn(ProjectileKind::HandGrenade, None, start, incoming, &tuning)
            .expect("authored grenade fits");
        let mut events = Vec::new();
        set.tick(TICK, &world, &mut events);
        let first = events
            .iter()
            .find_map(|event| match event {
                ProjectileEvent::Impact {
                    entity,
                    position,
                    normal,
                    ..
                } => Some((*entity, *position, *normal)),
                _ => None,
            })
            .expect("outside approach reaches the authored entity face");
        assert_eq!(first.0, Some(EntityId(9)));
        assert!((first.1 - contact).length() < 0.0001);
        assert!((first.2 - normal).length() < 0.0001);
        let projectile = set.get(id).expect("fuse has not expired");
        assert!(
            (projectile.position - contact).dot(normal) > 0.0,
            "assertion failed: real entity bounce departs after first contact"
        );
        assert!(!projectile.resting);
        assert!((projectile.velocity - expected_velocity).length() < 0.0001);
        assert!((projectile.position - (contact + expected_velocity * 0.005)).length() < 0.0001);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ProjectileEvent::Impact { .. }))
                .count(),
            1
        );
        let after_first = projectile.position;
        events.clear();
        set.tick(TICK, &world, &mut events);
        let projectile = set.get(id).expect("departing grenade remains active");
        assert!((projectile.position - (after_first + expected_velocity * TICK)).length() < 0.0001);
        assert!(!projectile.resting && events.is_empty());
    }

    // A legitimately slow first bounce still rests without being nudged.
    let entities = index_of(vec![cube_entity(9, center)]);
    let world = ProjectileWorld {
        collision: &collision,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 8);
    let id = set
        .spawn(
            ProjectileKind::HandGrenade,
            None,
            Vec3::new(-16.25, 0.0, 64.0),
            Vec3::new(50.0, 0.0, 0.0),
            &tuning,
        )
        .expect("authored slow grenade fits");
    let mut events = Vec::new();
    set.tick(TICK, &world, &mut events);
    let projectile = set.get(id).expect("resting grenade retains its fuse");
    assert!(projectile.resting && projectile.velocity == Vec3::ZERO);
    assert_eq!(projectile.position, Vec3::new(-16.0, 0.0, 64.0));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProjectileEvent::Impact { .. }))
            .count(),
        1
    );
}

/// The synthetic room: interior `[-256, 256]` on X and Y, `[0, 256]` on Z.
fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// An entity at `origin` with one 32-unit cube hitbox.
fn cube_entity(id: u64, origin: Vec3) -> EntityHitboxes {
    let mut entity = EntityHitboxes::new(EntityId(id), origin);
    entity.push_box(0, Vec3::splat(-16.0), Vec3::splat(16.0), HitGroup::Generic);
    entity
}

fn index_of(entities: Vec<EntityHitboxes>) -> HitboxIndex {
    let mut index = HitboxIndex::new(HitboxLimits::default());
    for entity in entities {
        assert!(index.push(entity));
    }
    index
}

fn empty_index() -> HitboxIndex {
    HitboxIndex::new(HitboxLimits::default())
}

/// Runs `set` for `seconds` and returns every event it produced.
fn run(set: &mut ProjectileSet, world: &ProjectileWorld<'_>, seconds: f32) -> Vec<ProjectileEvent> {
    let mut events = Vec::new();
    let ticks = {
        let ticks = (seconds / TICK).round();
        assert!(ticks.is_finite() && ticks >= 0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ticks = ticks as u32;
        ticks
    };
    for _ in 0..ticks {
        set.tick(TICK, world, &mut events);
    }
    events
}

#[test]
fn a_hand_grenade_detonates_after_its_published_five_second_fuse() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 1);
    set.spawn(
        ProjectileKind::HandGrenade,
        Some(EntityId(1)),
        Vec3::new(0.0, 0.0, 64.0),
        Vec3::new(300.0, 0.0, 200.0),
        &tuning,
    )
    .expect("the set has room");

    // Nothing detonates before the fuse is up.
    let early = run(&mut set, &context, HAND_GRENADE_FUSE_SECONDS - 0.1);
    assert!(
        !early
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. })),
        "the grenade must survive its whole fuse: {early:?}"
    );
    assert_eq!(set.len(), 1);

    let late = run(&mut set, &context, 0.2);
    let detonations: Vec<_> = late
        .iter()
        .filter(|event| matches!(event, ProjectileEvent::Detonate { .. }))
        .collect();
    assert_eq!(detonations.len(), 1, "exactly one detonation: {late:?}");
    assert!(set.is_empty(), "the grenade is removed when it goes off");
}

#[test]
fn a_thrown_grenade_bounces_and_never_ends_inside_solid() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 7);
    set.spawn(
        ProjectileKind::HandGrenade,
        None,
        Vec3::new(-200.0, 0.0, 128.0),
        // Fast enough to cross the room in well under one tick.
        Vec3::new(4000.0, 1500.0, 0.0),
        &tuning,
    )
    .expect("the set has room");

    let mut events = Vec::new();
    for _ in 0..400 {
        set.tick(TICK, &context, &mut events);
        for projectile in set.projectiles() {
            assert!(
                !contents::is_solid(world.contents_at(Hull::Point, projectile.position)),
                "a swept projectile never ends inside solid: {:?}",
                projectile.position
            );
        }
    }
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, ProjectileEvent::Impact { .. }))
            .count()
            >= 2,
        "a grenade thrown across the room bounces more than once: {events:?}"
    );
}

#[test]
fn a_rocket_detonates_on_the_first_thing_it_hits() {
    let world = room();
    let entities = index_of(vec![cube_entity(9, Vec3::new(128.0, 0.0, 64.0))]);
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
    set.spawn(
        ProjectileKind::Rocket,
        None,
        Vec3::new(-200.0, 0.0, 64.0),
        Vec3::new(1000.0, 0.0, 0.0),
        &tuning,
    )
    .expect("the set has room");

    let events = run(&mut set, &context, 1.0);
    let hit = events
        .iter()
        .find_map(|event| match event {
            ProjectileEvent::Impact { entity, .. } => Some(*entity),
            _ => None,
        })
        .expect("the rocket hits the target");
    assert_eq!(hit, Some(EntityId(9)));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. })),
        "a rocket detonates on impact: {events:?}"
    );
    assert!(set.is_empty());
}

#[test]
fn a_guided_rocket_converges_on_its_laser_point() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    // A slower rocket, so the room is big enough to show a curve.
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let goal = Vec3::new(0.0, 100.0, 128.0);

    // The same rocket, once guided and once not, both aimed straight down +X.
    let mut miss_distance = [0.0f32; 2];
    for (slot, guided) in [false, true].into_iter().enumerate() {
        let mut set = ProjectileSet::new(ProjectileLimits::default(), 3);
        let id = set
            .spawn(
                ProjectileKind::Rocket,
                None,
                Vec3::new(-200.0, -100.0, 128.0),
                Vec3::new(150.0, 0.0, 0.0),
                &tuning,
            )
            .expect("the set has room");
        if guided {
            set.get_mut(id).expect("just spawned").guide_point = Some(goal);
        }

        let mut closest = f32::INFINITY;
        let mut alignment = f32::NEG_INFINITY;
        let mut events = Vec::new();
        for tick in 0..400 {
            set.tick(TICK, &context, &mut events);
            let Some(projectile) = set.get(id) else { break };
            closest = closest.min(projectile.position.distance(goal));
            // While the rocket is still approaching, guidance may only turn
            // it further toward the goal, never away.
            if guided && tick < 100 {
                let toward = (goal - projectile.position).normalize();
                let next = projectile.velocity.normalize().dot(toward);
                assert!(
                    next >= alignment - 1e-3,
                    "guidance never turns a rocket away from its laser point"
                );
                alignment = next;
            }
        }
        miss_distance[slot] = closest;
    }

    assert!(
        miss_distance[1] < miss_distance[0],
        "guidance must beat flying straight: guided {} vs unguided {}",
        miss_distance[1],
        miss_distance[0]
    );
    assert!(
        miss_distance[1] < 32.0,
        "a guided rocket converges on the point: {}",
        miss_distance[1]
    );
}

#[test]
fn a_homing_hornet_turns_toward_its_target_and_a_straight_one_does_not() {
    let world = room();
    let entities = index_of(vec![cube_entity(4, Vec3::new(0.0, 200.0, 64.0))]);
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 11);
    let homing = set
        .spawn(
            ProjectileKind::Hornet,
            None,
            Vec3::new(-200.0, 0.0, 64.0),
            Vec3::new(400.0, 0.0, 0.0),
            &tuning,
        )
        .expect("room for the homing hornet");
    let straight = set
        .spawn(
            ProjectileKind::Hornet,
            None,
            Vec3::new(-200.0, -100.0, 64.0),
            Vec3::new(400.0, 0.0, 0.0),
            &tuning,
        )
        .expect("room for the straight hornet");
    set.get_mut(homing).expect("just spawned").target = Some(EntityId(4));

    let mut previous = f32::NEG_INFINITY;
    let mut events = Vec::new();
    for _ in 0..30 {
        set.tick(TICK, &context, &mut events);
        let Some(projectile) = set.get(homing) else {
            break;
        };
        let toward = (Vec3::new(0.0, 200.0, 64.0) - projectile.position).normalize();
        let alignment = projectile.velocity.normalize().dot(toward);
        assert!(
            alignment >= previous - 1e-3,
            "a homing hornet's alignment with its target never decreases"
        );
        previous = alignment;
    }
    assert!(
        previous > 0.7,
        "the hornet turned substantially toward the target: {previous}"
    );
    let straight = set.get(straight).expect("the straight hornet still flies");
    assert!(
        (straight.velocity.y).abs() < 1e-3,
        "a hornet with no target flies straight: {:?}",
        straight.velocity
    );
}

#[test]
fn a_crossbow_bolt_ignores_gravity_and_stops_where_it_lands() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
    set.spawn(
        ProjectileKind::CrossbowBolt,
        None,
        Vec3::new(0.0, 0.0, 128.0),
        Vec3::new(2000.0, 0.0, 0.0),
        &tuning,
    )
    .expect("the set has room");

    let events = run(&mut set, &context, 1.0);
    let impact = events
        .iter()
        .find_map(|event| match event {
            ProjectileEvent::Impact { position, .. } => Some(*position),
            _ => None,
        })
        .expect("the bolt reaches the far wall");
    assert!(
        (impact.z - 128.0).abs() < 1e-2,
        "a bolt does not drop: {impact:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Expired { .. })),
        "the bolt is removed once it lands: {events:?}"
    );
    assert!(set.is_empty());
}

#[test]
fn a_snark_hops_toward_the_nearest_entity_and_bites_it() {
    let world = room();
    // Between the room's ledge (x -192..-64) and its step (x 64..192) the
    // floor is flat, so a hopping snark has a clear run at the target.
    let entities = index_of(vec![cube_entity(2, Vec3::new(40.0, 0.0, 16.0))]);
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 5);
    let id = set
        .spawn(
            ProjectileKind::Snark,
            None,
            Vec3::new(-40.0, 0.0, 32.0),
            Vec3::ZERO,
            &tuning,
        )
        .expect("the set has room");

    let mut events = Vec::new();
    let mut bit = false;
    for _ in 0..600 {
        set.tick(TICK, &context, &mut events);
        if events.iter().any(|event| {
            matches!(
                event,
                ProjectileEvent::Impact {
                    entity: Some(EntityId(2)),
                    ..
                }
            )
        }) {
            bit = true;
            break;
        }
        if let Some(projectile) = set.get(id) {
            assert!(projectile.position.x < 256.0, "the snark stays in the room");
        }
    }
    assert!(bit, "the snark closed on the target and bit it: {events:?}");
}

#[test]
fn a_snark_self_destructs_after_its_published_lifetime() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 5);
    set.spawn(
        ProjectileKind::Snark,
        None,
        Vec3::new(0.0, 0.0, 32.0),
        Vec3::ZERO,
        &tuning,
    )
    .expect("the set has room");

    let early = run(&mut set, &context, SNARK_LIFETIME_SECONDS - 0.5);
    assert!(
        !early
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. })),
        "a snark lives its full lifetime"
    );
    let late = run(&mut set, &context, 1.0);
    assert!(
        late.iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. })),
        "a snark self-destructs: {late:?}"
    );
    assert!(set.is_empty());
}

#[test]
fn the_set_is_bounded() {
    let tuning = ProjectileTuning::default();
    let mut set = ProjectileSet::new(ProjectileLimits { max_projectiles: 2 }, 0);
    for _ in 0..2 {
        assert!(
            set.spawn(ProjectileKind::Rocket, None, Vec3::ZERO, Vec3::X, &tuning)
                .is_some()
        );
    }
    assert!(
        set.spawn(ProjectileKind::Rocket, None, Vec3::ZERO, Vec3::X, &tuning)
            .is_none(),
        "spawning past the limit fails instead of growing the set"
    );
    assert!(
        set.spawn(
            ProjectileKind::Rocket,
            None,
            Vec3::new(f32::NAN, 0.0, 0.0),
            Vec3::X,
            &tuning
        )
        .is_none(),
        "a non-finite spawn is rejected"
    );
}

#[test]
fn the_same_seed_and_inputs_produce_the_same_events() {
    let world = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };

    let replay = |seed: u64| {
        let mut set = ProjectileSet::new(ProjectileLimits::default(), seed);
        set.spawn(
            ProjectileKind::HandGrenade,
            None,
            Vec3::new(-100.0, 0.0, 100.0),
            Vec3::new(900.0, 350.0, 120.0),
            &tuning,
        );
        // A snark with nothing to chase wanders from the seeded stream.
        set.spawn(
            ProjectileKind::Snark,
            None,
            Vec3::new(0.0, 0.0, 40.0),
            Vec3::ZERO,
            &tuning,
        );
        run(&mut set, &context, 6.0)
    };

    assert_eq!(replay(42), replay(42), "one seed, one event sequence");
    assert_ne!(
        replay(42),
        replay(43),
        "a different seed sends the wandering snark somewhere else"
    );
}

/// The per-trace exclusion this crate uses instead of asking a caller to
/// leave a projectile's own model and its owner out of the shared
/// `HitboxIndex` (see `Projectile::self_id`/`owner`'s doc and
/// `crate::trace::TraceFilter::ignoring`'s): a caller may put both in the
/// index — because something else's trace this same tick still needs to
/// hit them, e.g. a placed tripmine's model — and this projectile's own
/// sweep must still ignore exactly those two, and nothing else.
#[test]
fn a_rockets_own_model_and_owner_do_not_block_its_own_flight_trace() {
    let world = room();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();

    let self_id = EntityId(101);
    let owner_id = EntityId(202);
    // Both boxes sit squarely on the rocket's straight-line path, close
    // enough that an unfiltered trace would stop it on the very first
    // substep.
    let entities = index_of(vec![
        cube_entity(self_id.0, Vec3::new(10.0, 0.0, 40.0)),
        cube_entity(owner_id.0, Vec3::new(20.0, 0.0, 40.0)),
    ]);
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };

    let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
    let id = set
        .spawn(
            ProjectileKind::Rocket,
            Some(owner_id),
            Vec3::new(0.0, 0.0, 40.0),
            Vec3::new(500.0, 0.0, 0.0),
            &tuning,
        )
        .expect("the set has room for one rocket");
    set.get_mut(id).expect("just spawned").self_id = Some(self_id);

    let events = run(&mut set, &context, 0.2);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, ProjectileEvent::Detonate { .. })),
        "the rocket must not detonate on its own model or its owner: {events:?}"
    );
    let flown = set.get(id).map_or(0.0, |projectile| projectile.position.x);
    assert!(
        flown > 20.0,
        "the rocket must fly past both ignored boxes, got x={flown}"
    );
}

/// The same scene, minus the ignore list: an unrelated entity's box on the
/// same path still stops the rocket, so the fix above is a targeted
/// exclusion and not a blanket "projectiles hit nothing" regression.
#[test]
fn a_projectile_still_hits_an_unrelated_entity_in_its_path() {
    let world = room();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();

    let bystander_id = EntityId(303);
    let entities = index_of(vec![cube_entity(
        bystander_id.0,
        Vec3::new(10.0, 0.0, 40.0),
    )]);
    let context = ProjectileWorld {
        collision: &world,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };

    let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
    set.spawn(
        ProjectileKind::Rocket,
        None,
        Vec3::new(0.0, 0.0, 40.0),
        Vec3::new(500.0, 0.0, 0.0),
        &tuning,
    )
    .expect("the set has room for one rocket");

    let events = run(&mut set, &context, 0.2);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. })),
        "an unrelated entity in the flight path must still stop the rocket: {events:?}"
    );
}

#[test]
fn terminal_events_keep_the_owner_after_the_physical_projectile_is_removed() {
    for kind in [
        ProjectileKind::Rocket,
        ProjectileKind::BullsquidSpit,
        ProjectileKind::ControllerBall,
        ProjectileKind::ControllerHomingBall,
        ProjectileKind::GonarchMortar,
    ] {
        let collision = room();
        let entities = index_of(vec![cube_entity(2, Vec3::new(100.0, 0.0, 100.0))]);
        let movement = MoveConfig::default();
        let tuning = ProjectileTuning::default();
        let world = ProjectileWorld {
            collision: &collision,
            entities: &entities,
            movement: &movement,
            tuning: &tuning,
        };
        let mut set = ProjectileSet::default();
        let id = set
            .spawn(
                kind,
                Some(EntityId(1)),
                Vec3::new(0.0, 0.0, 100.0),
                Vec3::new(1000.0, 0.0, 0.0),
                &tuning,
            )
            .expect("spawn");
        let events = run(&mut set, &world, 0.2);
        assert!(set.get(id).is_none());
        let impacts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProjectileEvent::Impact { owner, entity, .. } => Some((*owner, *entity)),
                _ => None,
            })
            .collect();
        assert_eq!(impacts, [(Some(EntityId(1)), Some(EntityId(2)))]);
        for event in events {
            if let ProjectileEvent::Detonate { owner, .. } = event {
                assert_eq!(owner, Some(EntityId(1)));
            }
        }
    }
}

#[test]
fn mp5_grenade_falls_and_detonates_on_first_contact_without_a_fuse() {
    let collision = room();
    let entities = empty_index();
    let movement = MoveConfig::default();
    let tuning = ProjectileTuning::default();
    let world = ProjectileWorld {
        collision: &collision,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::default();
    let id = set
        .spawn(
            ProjectileKind::Mp5Grenade,
            None,
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::ZERO,
            &tuning,
        )
        .expect("spawn");
    assert_eq!(set.get(id).expect("grenade").fuse, None);
    let early = run(&mut set, &world, 0.1);
    assert!(early.is_empty());
    assert!(set.get(id).expect("falling").position.z < 100.0);
    let events = run(&mut set, &world, 0.7);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ProjectileEvent::Detonate { .. }))
            .count(),
        1
    );
    assert!(set.is_empty());
}
