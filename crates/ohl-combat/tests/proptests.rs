//! Property tests: attack traces and damage application stay total.

use ohl_combat::{
    Armor, ArmorRule, DamageInfo, DamageType, EntityHitboxes, EntityId, HitGroup, HitboxIndex,
    HitboxLimits, Quat, TraceFilter, TraceMask, Vec3, apply_damage, trace_attack,
    trace_attack_filtered,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::CollisionModel;
use proptest::prelude::*;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

fn coordinate() -> impl Strategy<Value = f32> {
    -512.0f32..512.0f32
}

fn point() -> impl Strategy<Value = Vec3> {
    (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

prop_compose! {
    fn entity()(
        id in 0u64..8,
        origin in point(),
        yaw in -360.0f32..360.0f32,
        half in (1.0f32..48.0, 1.0f32..48.0, 1.0f32..48.0),
        group in -1i32..9,
    ) -> EntityHitboxes {
        let mut entity = EntityHitboxes::new(EntityId(id), origin)
            .with_rotation(Quat::from_rotation_z(yaw.to_radians()));
        let extents = Vec3::new(half.0, half.1, half.2);
        entity.push_box(0, -extents, extents, HitGroup::from_index(group));
        entity
    }
}

proptest! {
    /// Whatever the geometry, a trace terminates, reports a fraction in
    /// `0..=1`, and puts its impact point on the traced segment.
    #[test]
    fn a_trace_never_panics_and_stays_on_its_segment(
        start in point(),
        end in point(),
        entities in prop::collection::vec(entity(), 0..6),
        world_blocks in any::<bool>(),
        entities_block in any::<bool>(),
    ) {
        let world = room();
        let mut index = HitboxIndex::new(HitboxLimits::default());
        for entity in entities {
            index.push(entity);
        }
        let mask = TraceMask { world: world_blocks, entities: entities_block };
        let trace = trace_attack(&world, &index, start, end, mask);

        prop_assert!((0.0..=1.0).contains(&trace.fraction), "fraction {}", trace.fraction);
        let expected = start + (end - start) * trace.fraction;
        prop_assert!(
            (trace.end - expected).length() <= 0.05 + expected.length() * 1e-4,
            "end {:?} is not at fraction {} of {start:?} -> {end:?}",
            trace.end,
            trace.fraction
        );
        prop_assert!(trace.end.is_finite());
        prop_assert!(trace.surface_normal.is_finite());
        // A hitbox index entry is only ever reported together with its entity.
        prop_assert_eq!(trace.entity.is_some(), trace.hitbox.is_some());
        if trace.entity.is_none() {
            prop_assert_eq!(trace.hitgroup, HitGroup::Generic);
        }
        // Masking entities out can only push the impact further away.
        let world_only = trace_attack(&world, &index, start, end, TraceMask::WORLD_ONLY);
        if mask.world {
            prop_assert!(world_only.fraction >= trace.fraction - 1e-4);
            prop_assert!(world_only.entity.is_none());
        }
    }

    /// A filtered trace stays total too: a fraction in `0..=1`, an impact
    /// point on the segment, and never a hit against an ignored entity.
    #[test]
    fn a_filtered_trace_never_panics_and_never_hits_an_ignored_entity(
        start in point(),
        end in point(),
        entities in prop::collection::vec(entity(), 0..6),
        world_blocks in any::<bool>(),
        entities_block in any::<bool>(),
        ignore_a in prop::option::of(0u64..8),
        ignore_b in prop::option::of(0u64..8),
    ) {
        let world = room();
        let mut index = HitboxIndex::new(HitboxLimits::default());
        for entity in entities {
            index.push(entity);
        }
        let filter = TraceFilter {
            ignore: [ignore_a.map(EntityId), ignore_b.map(EntityId)],
            mask: TraceMask { world: world_blocks, entities: entities_block },
        };
        let trace = trace_attack_filtered(&world, &index, start, end, filter);

        prop_assert!((0.0..=1.0).contains(&trace.fraction), "fraction {}", trace.fraction);
        let expected = start + (end - start) * trace.fraction;
        prop_assert!(
            (trace.end - expected).length() <= 0.05 + expected.length() * 1e-4,
            "end {:?} is not at fraction {} of {start:?} -> {end:?}",
            trace.end,
            trace.fraction
        );
        prop_assert!(trace.end.is_finite());
        prop_assert!(trace.surface_normal.is_finite());
        if let Some(id) = trace.entity {
            prop_assert_ne!(Some(id.0), ignore_a);
            prop_assert_ne!(Some(id.0), ignore_b);
        }
    }

    /// Damage never restores health or armour, never removes more than the
    /// target had, and reports exactly what it removed.
    #[test]
    fn damage_conserves_health_and_armor(
        amount in 0.001f32..10_000.0,
        ratio in 0.0f32..1.0,
        bonus in 0.01f32..8.0,
        health_start in 0.0f32..200.0,
        armor_start in 0.0f32..200.0,
    ) {
        let mut health = ohl_combat::Health { current: health_start, max: 200.0 };
        let mut armor = Armor { current: armor_start, max: 200.0 };
        let info = DamageInfo::new(amount, DamageType::GENERIC);
        let was_dead = health.is_dead();

        let outcome = apply_damage(
            &mut health,
            Some(&mut armor),
            &info,
            ArmorRule { ratio, bonus },
        ).expect("a positive finite amount is accepted");

        prop_assert!(outcome.health_lost >= 0.0 && outcome.health_lost <= amount + 1e-3);
        prop_assert!(outcome.armor_lost >= 0.0 && outcome.armor_lost <= armor_start + 1e-3);
        prop_assert!((health_start - health.current - outcome.health_lost).abs() < 1e-2);
        prop_assert!((armor_start - armor.current - outcome.armor_lost).abs() < 1e-2);
        prop_assert!(armor.current >= 0.0);
        prop_assert_eq!(outcome.killed, !was_dead && health.is_dead());
        prop_assert!(health.current.is_finite() && armor.current.is_finite());
    }
}
