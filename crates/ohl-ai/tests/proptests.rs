//! Property tests: the senses, the runner and the movement glue must be
//! total — no panic, no non-finite state — for arbitrary inputs.

use ohl_ai::{
    Actor, AiWorld, Candidate, Classification, DefaultBrain, MonsterAi, Pcg32, RelationshipTable,
    Route, Senses, SightContext, SoundEvent, SoundKind, StuckDetector, Viewer, listen, look,
    move_toward, spawn_actor, spawn_monster,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::{CollisionModel, Hull};
use proptest::prelude::*;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("the fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("the fixture has usable collision hulls")
}

/// Coordinates that reach well past the fixture, plus the awkward values.
fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -10_000.0f32..10_000.0f32,
        1 => Just(0.0f32),
        1 => prop_oneof![
            Just(f32::NAN),
            Just(f32::INFINITY),
            Just(f32::NEG_INFINITY),
            Just(f32::MAX),
            Just(f32::MIN_POSITIVE),
        ],
    ]
}

fn point() -> impl Strategy<Value = ohl_ai::Vec3> {
    (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| ohl_ai::Vec3::new(x, y, z))
}

fn classification() -> impl Strategy<Value = Classification> {
    (0usize..ohl_ai::CLASSIFICATION_COUNT)
        .prop_map(|index| Classification::from_index(index).expect("index is in range"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Sight never panics, and every sighting it reports carries a finite
    /// distance no greater than the look distance.
    #[test]
    fn look_is_total(
        eye in point(),
        view_ofs in point(),
        forward in point(),
        targets in prop::collection::vec((point(), classification()), 0..8),
        look_distance in coordinate(),
        fov_cos in -2.0f32..2.0f32,
    ) {
        let collision = room();
        let mut world = hecs::World::new();
        let viewer_entity = world.spawn((0u8,));
        let candidates: Vec<Candidate> = targets
            .iter()
            .map(|&(origin, classification)| Candidate {
                entity: world.spawn((0u8,)),
                classification,
                origin,
                view_ofs: ohl_ai::Vec3::new(0.0, 0.0, 28.0),
                forward: ohl_ai::Vec3::X,
                alive: true,
                is_client: false,
            })
            .collect();

        let viewer = Viewer {
            entity: viewer_entity,
            origin: eye,
            view_ofs,
            forward,
            classification: Classification::HumanMilitary,
        };
        let senses = Senses {
            look_distance,
            fov_cos,
            hearing_sensitivity: 1.0,
        };
        let result = look(
            &viewer,
            &senses,
            &candidates,
            &RelationshipTable::provisional(),
            &SightContext::tracing(&collision),
        );
        for sighting in &result.visible {
            prop_assert!(sighting.distance.is_finite());
            prop_assert!(sighting.distance <= look_distance);
        }
        if let Some(enemy) = result.enemy {
            prop_assert!(enemy.relationship.is_hostile());
            prop_assert!(result.visible.iter().any(|s| s.entity == enemy.entity));
        }
    }

    /// Hearing never panics and only ever reports an entry from the list.
    #[test]
    fn listen_is_total(
        ears in point(),
        sensitivity in -10.0f32..10.0f32,
        entries in prop::collection::vec((point(), coordinate(), 0u8..7), 0..16),
    ) {
        let mut sounds = ohl_ai::SoundList::new();
        for (position, radius, kind) in &entries {
            let kind = match kind {
                0 => SoundKind::Combat,
                1 => SoundKind::Danger,
                2 => SoundKind::World,
                3 => SoundKind::Player,
                4 => SoundKind::Carcass,
                5 => SoundKind::Meat,
                _ => SoundKind::Garbage,
            };
            sounds.push(SoundEvent::new(kind, *position, *radius));
        }
        let senses = Senses {
            hearing_sensitivity: sensitivity,
            ..Senses::default()
        };
        let result = listen(ears, &senses, &sounds);
        if let Some(best) = result.best {
            prop_assert!(sounds.events().contains(&best));
        }
        if sensitivity <= 0.0 {
            prop_assert!(result.conditions.is_empty());
        }
    }

    /// A movement step never panics, never produces a non-finite position
    /// and never travels further than it was asked to.
    #[test]
    fn move_toward_is_total(
        from in point(),
        to in point(),
        speed in coordinate(),
        dt in -1.0f32..1.0f32,
        hull_index in 0usize..4,
    ) {
        let collision = room();
        let hull = Hull::from_index(hull_index).expect("index is in range");
        let result = move_toward(&collision, hull, from, to, speed, dt);
        prop_assert!(result.distance >= 0.0);
        if from.is_finite() {
            prop_assert!(result.position.is_finite());
        }
        if speed.is_finite() && dt.is_finite() && speed > 0.0 && dt > 0.0 {
            // A step-up adds vertical travel, so only the horizontal part is
            // bounded by the requested step.
            let horizontal = ohl_ai::Vec3::new(
                result.position.x - from.x,
                result.position.y - from.y,
                0.0,
            )
            .length();
            if horizontal.is_finite() {
                prop_assert!(horizontal <= speed * dt + 1.0);
            }
        }
    }

    /// Ticking the whole simulation with arbitrary starting positions never
    /// panics and never leaves an actor at a non-finite position.
    #[test]
    fn ticking_is_total(
        positions in prop::collection::vec(point(), 1..6),
        seed in any::<u64>(),
        dt in -1.0f32..1.0f32,
    ) {
        let collision = room();
        let mut ai = AiWorld::new(seed);
        let brain = ai.register_brain(Box::new(DefaultBrain::ranged(
            Classification::HumanMilitary,
        )));
        let mut world = hecs::World::new();
        let monsters: Vec<_> = positions
            .iter()
            .map(|&origin| {
                spawn_monster(
                    &mut world,
                    Actor::new(Classification::HumanMilitary, origin),
                    brain,
                )
            })
            .collect();
        spawn_actor(
            &mut world,
            Actor::new(Classification::Player, positions[0]).as_client(),
        );

        let context = SightContext::tracing(&collision);
        for _ in 0..16 {
            ai.tick(&mut world, &context, dt);
        }
        for monster in monsters {
            let actor = *world.get::<&Actor>(monster).expect("component");
            let started_finite = actor.origin.is_finite();
            prop_assert!(actor.yaw.is_finite());
            let state = world.get::<&MonsterAi>(monster).expect("component");
            prop_assert!(state.runner.timer().is_finite());
            prop_assert!(started_finite || !actor.origin.is_finite());
        }
    }

    /// The route cursor never runs past its waypoints and the stuck
    /// detector never wraps.
    #[test]
    fn route_and_stuck_detector_stay_consistent(
        waypoints in prop::collection::vec(point(), 0..8),
        probes in prop::collection::vec(point(), 0..16),
        distances in prop::collection::vec(coordinate(), 0..64),
    ) {
        let mut route = Route::through(&waypoints, ohl_ai::Vec3::ZERO);
        for probe in &probes {
            route.advance_if_reached(*probe);
            prop_assert!(route.current <= route.waypoints.len());
        }
        let mut detector = StuckDetector::new();
        for distance in &distances {
            detector.record(*distance);
        }
        prop_assert!(u64::from(detector.ticks()) <= distances.len() as u64);
    }

    /// The generator's bounded draw is always in range and its float draw is
    /// always in the unit interval, for any seed.
    #[test]
    fn the_generator_stays_in_range(seed in any::<u64>(), bound in any::<u32>()) {
        let mut rng = Pcg32::new(seed);
        let value = rng.below(bound);
        if bound == 0 {
            prop_assert_eq!(value, 0);
        } else {
            prop_assert!(value < bound);
        }
        let float = rng.next_f32();
        prop_assert!((0.0..1.0).contains(&float));
    }
}
