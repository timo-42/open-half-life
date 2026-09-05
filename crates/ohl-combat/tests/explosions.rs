//! Radius damage against the project's synthetic collision room.

use ohl_combat::explosion::{BlastTarget, ExplosionRule, falloff, radius_damage};
use ohl_combat::weapons::BlackBox;
use ohl_combat::{DamageType, EntityId, Vec3};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::CollisionModel;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

#[test]
fn falloff_is_linear_and_monotonic() {
    assert!((falloff(0.0, 100.0) - 1.0).abs() < 1e-6);
    assert!((falloff(50.0, 100.0) - 0.5).abs() < 1e-6);
    assert!(falloff(100.0, 100.0) < 1e-6);
    assert!(falloff(1000.0, 100.0) < 1e-6);
    assert!(falloff(10.0, 0.0) < 1e-6, "a zero radius damages nothing");
    assert!(falloff(f32::NAN, 100.0) < 1e-6);

    let mut previous = f32::INFINITY;
    for step in 0..=200u16 {
        let value = falloff(f32::from(step), 100.0);
        assert!(value <= previous + 1e-7, "falloff never rises");
        assert!((0.0..=1.0).contains(&value));
        previous = value;
    }
}

#[test]
fn damage_falls_off_with_distance_and_stops_at_the_radius() {
    let world = room();
    let rule = ExplosionRule::default();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let targets = [
        BlastTarget::new(EntityId(1), center),
        BlastTarget::new(EntityId(2), center + Vec3::X * 50.0),
        BlastTarget::new(EntityId(3), center + Vec3::X * 150.0),
        BlastTarget::new(EntityId(4), center + Vec3::X * 200.0),
    ];
    let hits = radius_damage(
        center,
        200.0,
        100.0,
        DamageType::BLAST,
        Some(EntityId(9)),
        targets.into_iter(),
        &world,
        &rule,
    );

    assert_eq!(hits.len(), 3, "the target at exactly the radius is spared");
    assert_eq!(hits[0].target, EntityId(1));
    assert!((hits[0].damage.amount - 100.0).abs() < 1e-3);
    assert!((hits[1].damage.amount - 75.0).abs() < 1e-3);
    assert!((hits[2].damage.amount - 25.0).abs() < 1e-3);
    for hit in &hits {
        assert_eq!(hit.damage.attacker, Some(EntityId(9)));
        assert_eq!(hit.damage.kind, DamageType::BLAST);
        assert_eq!(hit.damage.origin, center);
    }
    // Monotonic in distance.
    assert!(hits[0].damage.amount > hits[1].damage.amount);
    assert!(hits[1].damage.amount > hits[2].damage.amount);
}

#[test]
fn a_hitbox_is_measured_from_its_nearest_face() {
    let world = room();
    let rule = ExplosionRule::default();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let origin_only = BlastTarget::new(EntityId(1), center + Vec3::X * 150.0);
    let with_box = BlastTarget::new(EntityId(2), center + Vec3::X * 150.0)
        .with_hitbox(center + Vec3::X * 100.0, center + Vec3::X * 200.0);
    let hits = radius_damage(
        center,
        200.0,
        100.0,
        DamageType::BLAST,
        None,
        [origin_only, with_box].into_iter(),
        &world,
        &rule,
    );
    assert_eq!(hits.len(), 2);
    assert!(
        hits[1].damage.amount > hits[0].damage.amount,
        "the boxed target is hurt from its near face: {hits:?}"
    );
}

#[test]
fn a_target_behind_world_geometry_is_untouched() {
    let world = room();
    let center = Vec3::new(0.0, 0.0, 64.0);
    // Outside the room's +X wall, well within the blast radius.
    let hidden = BlastTarget::new(EntityId(1), Vec3::new(300.0, 0.0, 64.0));
    let exposed = BlastTarget::new(EntityId(2), Vec3::new(200.0, 0.0, 64.0));

    let occluding = ExplosionRule::default();
    assert!(occluding.occlusion);
    let hits = radius_damage(
        center,
        400.0,
        100.0,
        DamageType::BLAST,
        None,
        [hidden, exposed].into_iter(),
        &world,
        &occluding,
    );
    assert_eq!(hits.len(), 1, "only the exposed target is hurt: {hits:?}");
    assert_eq!(hits[0].target, EntityId(2));

    let transparent = ExplosionRule {
        occlusion: false,
        ..ExplosionRule::default()
    };
    let hits = radius_damage(
        center,
        400.0,
        100.0,
        DamageType::BLAST,
        None,
        [hidden, exposed].into_iter(),
        &world,
        &transparent,
    );
    assert_eq!(
        hits.len(),
        2,
        "with the line-of-sight rule off, cover stops protecting"
    );
}

#[test]
fn the_self_damage_hook_scales_only_the_attacker() {
    let world = room();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let rule = ExplosionRule {
        self_damage_scale: BlackBox::new(0.0),
        ..ExplosionRule::default()
    };
    let targets = [
        BlastTarget::new(EntityId(7), center),
        BlastTarget::new(EntityId(8), center),
    ];
    let hits = radius_damage(
        center,
        200.0,
        100.0,
        DamageType::BLAST,
        Some(EntityId(7)),
        targets.into_iter(),
        &world,
        &rule,
    );
    assert_eq!(
        hits.len(),
        1,
        "a zero self-damage scale spares the attacker"
    );
    assert_eq!(hits[0].target, EntityId(8));

    let halved = ExplosionRule {
        self_damage_scale: BlackBox::new(0.5),
        ..ExplosionRule::default()
    };
    let hits = radius_damage(
        center,
        200.0,
        100.0,
        DamageType::BLAST,
        Some(EntityId(7)),
        targets.into_iter(),
        &world,
        &halved,
    );
    assert_eq!(hits.len(), 2);
    assert!((hits[0].damage.amount - 50.0).abs() < 1e-3);
    assert!((hits[1].damage.amount - 100.0).abs() < 1e-3);
}

#[test]
fn pushback_points_away_from_the_blast() {
    let world = room();
    let rule = ExplosionRule::default();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let hits = radius_damage(
        center,
        200.0,
        100.0,
        DamageType::BLAST,
        None,
        [
            BlastTarget::new(EntityId(1), center + Vec3::X * 50.0),
            BlastTarget::new(EntityId(2), center),
        ]
        .into_iter(),
        &world,
        &rule,
    );
    assert_eq!(hits.len(), 2);
    assert!(hits[0].pushback.x > 0.0, "pushed away from the centre");
    assert!(hits[0].pushback.y.abs() < 1e-6);
    assert_eq!(
        hits[1].pushback,
        Vec3::ZERO,
        "a blast has no direction for something standing in it"
    );
    assert_eq!(hits[0].damage.direction, Vec3::X);
}

#[test]
fn degenerate_blasts_hurt_nobody() {
    let world = room();
    let rule = ExplosionRule::default();
    let center = Vec3::new(0.0, 0.0, 64.0);
    let target = || [BlastTarget::new(EntityId(1), center)].into_iter();
    for (radius, damage) in [
        (0.0, 100.0),
        (-10.0, 100.0),
        (f32::NAN, 100.0),
        (100.0, 0.0),
        (100.0, -5.0),
        (100.0, f32::INFINITY),
    ] {
        let hits = radius_damage(
            center,
            radius,
            damage,
            DamageType::BLAST,
            None,
            target(),
            &world,
            &rule,
        );
        assert!(
            hits.is_empty(),
            "radius {radius} damage {damage} must produce nothing"
        );
    }
    assert!(
        radius_damage(
            Vec3::new(f32::NAN, 0.0, 0.0),
            100.0,
            100.0,
            DamageType::BLAST,
            None,
            target(),
            &world,
            &rule,
        )
        .is_empty()
    );
}
