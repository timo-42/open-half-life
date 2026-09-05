//! Damage arithmetic, armour absorption boundaries and combat events.
//!
//! Every rule parameter is passed explicitly: the defaults are deliberately
//! neutral placeholders for values still to be black-box observed, so no test
//! asserts an unpublished number.

use ohl_combat::{
    Armor, ArmorRule, CombatEvent, CombatEventQueue, DamageInfo, DamageType, Difficulty,
    DifficultyScale, EntityId, Health, HitGroup, HitGroupScale, SurfaceKind, Vec3, apply_damage,
};
use ohl_core::SanitizedError;

/// A rule that sends half the damage straight to health and charges the rest
/// to armour one for one. The numbers are the test's own, not Half-Life's.
const HALF_AND_HALF: ArmorRule = ArmorRule {
    ratio: 0.5,
    bonus: 1.0,
};

fn bullet(amount: f32) -> DamageInfo {
    DamageInfo::new(amount, DamageType::BULLET)
}

#[test]
fn without_armor_every_hit_point_reaches_health() {
    let mut health = Health::new(100.0);
    let outcome = apply_damage(&mut health, None, &bullet(30.0), HALF_AND_HALF)
        .expect("a positive amount is valid");
    assert!((outcome.health_lost - 30.0).abs() < 1e-4);
    assert!((outcome.armor_lost).abs() < 1e-6);
    assert!(!outcome.killed);
    assert!((health.current - 70.0).abs() < 1e-4);
}

#[test]
fn armor_absorbs_its_share_while_it_lasts() {
    let mut health = Health::new(100.0);
    let mut armor = Armor::full(100.0);
    let outcome = apply_damage(&mut health, Some(&mut armor), &bullet(40.0), HALF_AND_HALF)
        .expect("a positive amount is valid");

    assert!((outcome.health_lost - 20.0).abs() < 1e-4);
    assert!((outcome.armor_lost - 20.0).abs() < 1e-4);
    assert!((health.current - 80.0).abs() < 1e-4);
    assert!((armor.current - 80.0).abs() < 1e-4);
}

#[test]
fn armor_that_runs_out_part_way_passes_the_remainder_to_health() {
    let mut health = Health::new(100.0);
    // Only 5 armour points against a hit whose armour share is 25.
    let mut armor = Armor {
        current: 5.0,
        max: 100.0,
    };
    let outcome = apply_damage(&mut health, Some(&mut armor), &bullet(50.0), HALF_AND_HALF)
        .expect("a positive amount is valid");

    // 25 bypasses armour, armour pays for 5, the other 20 reaches health.
    assert!((outcome.health_lost - 45.0).abs() < 1e-4);
    assert!((outcome.armor_lost - 5.0).abs() < 1e-4);
    assert!((armor.current).abs() < 1e-6);
}

#[test]
fn the_boundary_where_armor_is_exactly_spent_loses_no_extra_health() {
    let mut health = Health::new(100.0);
    let mut armor = Armor {
        current: 25.0,
        max: 100.0,
    };
    let outcome = apply_damage(&mut health, Some(&mut armor), &bullet(50.0), HALF_AND_HALF)
        .expect("a positive amount is valid");

    assert!((outcome.health_lost - 25.0).abs() < 1e-4);
    assert!((outcome.armor_lost - 25.0).abs() < 1e-4);
    assert!(armor.current.abs() < 1e-6);
}

#[test]
fn a_bonus_above_one_spends_armor_faster_than_it_stops_damage() {
    let mut health = Health::new(100.0);
    let mut armor = Armor::full(100.0);
    let rule = ArmorRule {
        ratio: 0.0,
        bonus: 2.0,
    };
    let outcome =
        apply_damage(&mut health, Some(&mut armor), &bullet(20.0), rule).expect("valid amount");

    assert!(outcome.health_lost.abs() < 1e-6, "armour stopped all of it");
    assert!((outcome.armor_lost - 40.0).abs() < 1e-4);
}

#[test]
fn the_neutral_default_rule_makes_armor_irrelevant() {
    let mut health = Health::new(100.0);
    let mut armor = Armor::full(100.0);
    let outcome = apply_damage(
        &mut health,
        Some(&mut armor),
        &bullet(25.0),
        ArmorRule::default(),
    )
    .expect("valid amount");

    assert!((outcome.health_lost - 25.0).abs() < 1e-4);
    assert!(outcome.armor_lost.abs() < 1e-6);
    assert!((armor.current - 100.0).abs() < 1e-6);
}

#[test]
fn out_of_range_rule_parameters_are_clamped() {
    let rule = ArmorRule::new(4.0, -1.0);
    assert!((rule.ratio - 1.0).abs() < f32::EPSILON);
    assert!(rule.bonus > 0.0);
    let nan = ArmorRule::new(f32::NAN, f32::INFINITY);
    assert!((nan.ratio - 1.0).abs() < f32::EPSILON);
    assert!((nan.bonus - 1.0).abs() < f32::EPSILON);
}

#[test]
fn the_killing_blow_is_flagged_exactly_once() {
    let mut health = Health::new(20.0);
    let first = apply_damage(&mut health, None, &bullet(20.0), HALF_AND_HALF).expect("valid");
    assert!(first.killed);
    assert!(health.is_dead());

    let second = apply_damage(&mut health, None, &bullet(5.0), HALF_AND_HALF).expect("valid");
    assert!(
        !second.killed,
        "an already dead target is not killed a second time"
    );
}

#[test]
fn zero_negative_and_non_finite_amounts_are_rejected() {
    let mut health = Health::new(100.0);
    for amount in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            apply_damage(&mut health, None, &bullet(amount), HALF_AND_HALF),
            Err(SanitizedError::InvalidInput),
            "amount {amount} must be rejected"
        );
    }
    assert!((health.current - 100.0).abs() < f32::EPSILON, "unchanged");
}

#[test]
fn healing_and_recharging_clamp_to_their_maxima() {
    let mut health = Health::new(100.0);
    health.current = 40.0;
    assert!((health.heal(1000.0) - 60.0).abs() < 1e-4);
    assert!((health.current - 100.0).abs() < 1e-4);
    assert!(health.heal(-5.0).abs() < f32::EPSILON);

    let mut armor = Armor::empty(100.0);
    assert!((armor.recharge(75.0) - 75.0).abs() < 1e-4);
    assert!((armor.recharge(75.0) - 25.0).abs() < 1e-4);
    assert!((armor.current - 100.0).abs() < 1e-4);
}

#[test]
fn damage_types_combine_and_print_by_their_published_names() {
    let kind = DamageType::BULLET | DamageType::SHOCK;
    assert!(kind.contains(DamageType::BULLET));
    assert!(kind.intersects(DamageType::SHOCK | DamageType::BURN));
    assert!(!kind.contains(DamageType::BURN));
    assert_eq!(kind.to_string(), "bullet+shock");
    assert_eq!(DamageType::NONE.to_string(), "none");
    assert_eq!(DamageType::NAMED.len(), 20);
    assert_eq!(
        DamageType::from_bits_truncate(u32::MAX),
        DamageType::ALL,
        "unnamed bits are dropped"
    );
    assert!((!DamageType::ALL).is_empty());
}

#[test]
fn difficulty_and_hit_group_scales_default_to_no_scaling() {
    let info = bullet(10.0);
    for difficulty in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
        let scaled = DifficultyScale::default().scaled(&info, difficulty);
        assert!((scaled.amount - 10.0).abs() < f32::EPSILON);
    }
    assert_eq!(Difficulty::default(), Difficulty::Medium);
    assert_eq!(Difficulty::Hard.pick([1, 2, 3]), 3);

    let scale = HitGroupScale {
        head: 3.0,
        ..HitGroupScale::default()
    };
    assert!((scale.factor(HitGroup::Head) - 3.0).abs() < f32::EPSILON);
    assert!((scale.factor(HitGroup::LeftLeg) - 1.0).abs() < f32::EPSILON);

    // Nonsense entries fall back to "no scaling" rather than poisoning the
    // damage arithmetic with a NaN.
    let broken = DifficultyScale {
        easy: f32::NAN,
        medium: -2.0,
        hard: 2.0,
    };
    assert!((broken.factor(Difficulty::Easy) - 1.0).abs() < f32::EPSILON);
    assert!((broken.factor(Difficulty::Medium) - 1.0).abs() < f32::EPSILON);
    assert!((broken.factor(Difficulty::Hard) - 2.0).abs() < f32::EPSILON);
}

#[test]
fn damage_info_carries_its_attacker_and_geometry() {
    let info = bullet(8.0)
        .from_entities(EntityId(1), EntityId(2))
        .from_point(Vec3::new(1.0, 2.0, 3.0), Vec3::X);
    assert_eq!(info.attacker, Some(EntityId(1)));
    assert_eq!(info.inflictor, Some(EntityId(2)));
    assert!((info.origin.y - 2.0).abs() < f32::EPSILON);
    assert!((info.with_amount(4.0).amount - 4.0).abs() < f32::EPSILON);
}

#[test]
fn the_event_queue_is_bounded_and_counts_what_it_drops() {
    let mut queue = CombatEventQueue::with_capacity(2);
    assert!(queue.is_empty());
    assert!(queue.push(CombatEvent::DamageDealt {
        target: EntityId(1),
        attacker: Some(EntityId(2)),
        health_lost: 10.0,
        armor_lost: 0.0,
        kind: DamageType::BULLET,
    }));
    assert!(queue.push(CombatEvent::Killed {
        target: EntityId(1),
        attacker: Some(EntityId(2)),
    }));
    assert!(!queue.push(CombatEvent::Impact {
        surface: SurfaceKind::World,
        position: Vec3::ZERO,
        normal: Vec3::Z,
        hitgroup: HitGroup::Generic,
    }));

    assert_eq!(queue.len(), 2);
    assert_eq!(queue.capacity(), 2);
    assert_eq!(queue.dropped(), 1);
    assert!(matches!(
        queue.events()[1],
        CombatEvent::Killed { target, .. } if target == EntityId(1)
    ));

    let drained: Vec<_> = queue.drain().collect();
    assert_eq!(drained.len(), 2);
    assert!(queue.is_empty() && queue.dropped() == 0);

    let mut default = CombatEventQueue::default();
    assert_eq!(default.capacity(), CombatEventQueue::DEFAULT_CAPACITY);
    default.clear();
    assert!(default.is_empty());
}
