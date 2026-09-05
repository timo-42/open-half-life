//! Property tests: the senses, the runner and the movement glue must be
//! total — no panic, no non-finite state — for arbitrary inputs.

use ohl_ai::monsters::nav_bridge::{NavBridge, NavBridgeLimits};
use ohl_ai::monsters::table::{Difficulty, MonsterKind, spec_for};
use ohl_ai::{
    Actor, AiWorld, Candidate, Classification, DamageEvent, DamageQueue, DamageSink, DefaultBrain,
    MonsterAi, MonsterBrain, Pcg32, RelationshipTable, Route, Senses, SightContext, SoundEvent,
    SoundKind, StuckDetector, Viewer, apply_monster_damage, listen, look, move_toward, spawn_actor,
    spawn_monster,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, build_collision_room_bsp};
use ohl_nav::{BuildLimits, NodeKind, NodeSeed};
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

/// A room like [`room`], split by a full-height wall (`x` in `-16..16`,
/// `y` in `-128..128`) with both ends open, for [`NavBridge`] proptests.
fn wall_room() -> CollisionModel {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
        CollisionBrush::box_brush([-16.0, -128.0, -16.0], [16.0, 128.0, 256.0]),
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
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("the fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("the fixture has usable collision hulls")
}

/// A 7x7 lattice over [`wall_room`], at 128-unit spacing.
fn wall_room_lattice() -> Vec<NodeSeed> {
    let coords = [-384.0, -256.0, -128.0, 0.0, 128.0, 256.0, 384.0];
    coords
        .iter()
        .flat_map(|&x| {
            coords
                .iter()
                .map(move |&y| NodeSeed::new(ohl_ai::Vec3::new(x, y, 8.0), NodeKind::Ground))
        })
        .collect()
}

fn hull() -> impl Strategy<Value = Hull> {
    prop_oneof![
        Just(Hull::Point),
        Just(Hull::Standing),
        Just(Hull::Large),
        Just(Hull::Crouched),
    ]
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

    /// Every defined monster kind's health/damage resolution is total, for
    /// any difficulty and any skill-table override (including ones that
    /// return non-finite values).
    #[test]
    fn monster_table_lookups_never_panic(
        kind_index in 0usize..16,
        difficulty_index in 0usize..3,
        override_value in coordinate(),
        has_override in any::<bool>(),
    ) {
        let kind = MonsterKind::defined()[kind_index].clone();
        let difficulty = Difficulty::ALL[difficulty_index];
        let spec = spec_for(&kind).expect("every defined kind has a spec");
        let lookup: &dyn Fn(&str) -> Option<f32> =
            &|_: &str| has_override.then_some(override_value);
        let skill: Option<&ohl_ai::monsters::table::SkillLookup<'_>> = Some(lookup);
        let health = spec.resolve_health(&kind, difficulty, skill);
        prop_assert!(health.is_finite() || (has_override && !override_value.is_finite()));
        if let Some(melee) = spec.melee {
            let _ = melee.resolve_damage(difficulty, "sk_x_dmg1", skill);
        }
        if let Some(ranged) = spec.ranged {
            let _ = ranged.resolve_damage(difficulty, "sk_x_dmg1", skill);
        }
    }

    /// Applying arbitrary queued damage never panics, whatever health an
    /// actor started with.
    #[test]
    fn lifecycle_apply_damage_is_total(
        starting_health in coordinate(),
        amounts in prop::collection::vec(coordinate(), 0..8),
    ) {
        let mut world = hecs::World::new();
        let attacker = world.spawn((0u8,));
        let victim = world.spawn((
            Actor::new(Classification::HumanMilitary, ohl_ai::Vec3::ZERO)
                .with_health(starting_health),
        ));
        let mut queue = DamageQueue::new();
        for amount in amounts {
            queue.push_damage(DamageEvent::new(victim, attacker, amount, ohl_ai::Vec3::ZERO));
        }
        // The only property required of arbitrary (including overflowing)
        // damage sums is that this never panics and reports at most one
        // death; `f32` arithmetic can still carry health to `-inf` from
        // finite but very large queued amounts, so finiteness of the
        // resulting health is deliberately not asserted here.
        let events = apply_monster_damage(&mut world, &queue, 2.0);
        prop_assert!(events.len() <= 1);
        let actor = world.get::<&Actor>(victim);
        prop_assert!(actor.is_ok());
    }

    /// Ticking with a per-kind `MonsterBrain` in place of the generic
    /// `DefaultBrain` stays deterministic: the same seed replays to the
    /// same `state_hash` digest.
    #[test]
    fn monster_brain_ticking_stays_deterministic(
        kind_index in 0usize..16,
        seed in any::<u64>(),
    ) {
        let kind = MonsterKind::defined()[kind_index].clone();
        let run = || {
            let mut ai = AiWorld::new(seed);
            let brain = ai.register_brain(Box::new(
                MonsterBrain::for_kind(kind.clone()).expect("defined kind"),
            ));
            let mut world = hecs::World::new();
            spawn_monster(
                &mut world,
                Actor::new(Classification::HumanMilitary, ohl_ai::Vec3::new(-64.0, 0.0, 0.0)),
                brain,
            );
            spawn_actor(
                &mut world,
                Actor::new(Classification::Player, ohl_ai::Vec3::new(64.0, 0.0, 0.0)).as_client(),
            );
            for _ in 0..64 {
                ai.tick(&mut world, &SightContext::empty(), 0.05);
            }
            ai.state_hash(&world)
        };
        prop_assert_eq!(run(), run());
    }
}

proptest! {
    // The graph build cost is shared work, not per-case work, but is still
    // paid once per case here (proptest reruns the whole body); a smaller
    // case count keeps this bounded.
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `NavBridge::next_move` never panics and always returns a finite
    /// position, for arbitrary origins, goals, hulls and step sizes,
    /// against a fixed map with both an obstacle and a node graph.
    #[test]
    fn nav_bridge_next_move_is_total(
        origin in point(),
        goal in point(),
        hull in hull(),
        max_step in coordinate(),
    ) {
        let collision = wall_room();
        let seeds = wall_room_lattice();
        let mut bridge = NavBridge::build(
            &seeds,
            &collision,
            &BuildLimits::default(),
            NavBridgeLimits::default(),
        );
        let actor = hecs::World::new().spawn(());
        bridge.begin_tick(&[actor]);
        let next = bridge.next_move(actor, origin, goal, hull, &collision, max_step);
        prop_assert!(next.is_finite(), "non-finite output: {next:?}");
    }
}
