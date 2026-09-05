//! A weapon fired at the project's synthetic collision room deposits the
//! documented damage on a target standing against a wall, through the same
//! `trace_attack` / `resolve_hitscan` / `apply_damage` pipeline the
//! composition root will drive.
//!
//! No game data is loaded: the map is `ohl_formats::test_support`'s
//! collision-room fixture, already used by `ohl-physics` and
//! `crates/ohl-combat/tests/attack_trace.rs`.

use ohl_combat::{
    AmmoPool, AmmoType, EntityHitboxes, EntityId, FiringState, Health, HitGroup, HitboxIndex,
    HitboxLimits, TraceMask, Vec3, WeaponAction, WeaponId, WeaponInput, apply_damage,
    resolve_hitscan, spec, trace_attack,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::CollisionModel;
use ohl_world::{BoneMatrix, StudioHitbox, StudioPose};

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

fn translation(x: f32, y: f32, z: f32) -> BoneMatrix {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0,
    ]
}

/// A single-bone target standing at `origin`, one 16-unit generic hitbox.
fn target_at(id: u64, origin: Vec3) -> HitboxIndex {
    let pose = StudioPose {
        matrices: vec![translation(0.0, 0.0, 0.0)],
    };
    let hitboxes = vec![StudioHitbox {
        bone: 0,
        group: HitGroup::Generic.index(),
        min: [-8.0, -8.0, -8.0],
        max: [8.0, 8.0, 8.0],
    }];
    let mut entity = EntityHitboxes::new(EntityId(id), origin);
    let added = entity.push_studio_hitboxes(&pose, &hitboxes);
    assert_eq!(added, 1);
    let mut index = HitboxIndex::new(HitboxLimits::default());
    assert!(index.push(entity));
    index
}

#[test]
fn a_python_shot_deposits_its_published_damage_on_a_target() {
    let world = room();
    let entry = spec(WeaponId::Python);
    let mut pool = AmmoPool::new(AmmoType::ThreeFiveSeven);
    pool.add(36);
    let mut firing = FiringState::new(entry);
    firing.tick(
        1.0,
        WeaponInput {
            select: true,
            ..Default::default()
        },
        &mut pool,
    );
    firing.set_clip(6);

    let action = firing.tick(
        0.1,
        WeaponInput {
            primary: true,
            ..Default::default()
        },
        &mut pool,
    );
    assert_eq!(
        action,
        WeaponAction::Hitscan {
            count: 1,
            spread: 0.0
        }
    );

    let target = target_at(1, Vec3::new(128.0, 0.0, 64.0));
    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(256.0, 0.0, 64.0);
    let trace = trace_attack(&world, &target, start, end, TraceMask::SHOT);
    assert_eq!(
        trace.entity,
        Some(EntityId(1)),
        "the shot reaches the target"
    );

    let hits = resolve_hitscan(action, &entry, &[trace], None, None);
    assert_eq!(hits.len(), 1);
    assert!(
        (hits[0].amount - 40.0).abs() < f32::EPSILON,
        "the .357's published damage"
    );

    let mut health = Health::new(100.0);
    let outcome = apply_damage(
        &mut health,
        None,
        &hits[0],
        ohl_combat::ArmorRule::default(),
    )
    .expect("a positive amount is accepted");
    assert!((outcome.health_lost - 40.0).abs() < f32::EPSILON);
    assert!((health.current - 60.0).abs() < f32::EPSILON);
}

#[test]
fn a_shotgun_blast_deposits_damage_per_pellet_that_reaches_the_target() {
    let world = room();
    let entry = spec(WeaponId::Shotgun);
    let mut pool = AmmoPool::new(AmmoType::Buckshot);
    pool.add(125);
    let mut firing = FiringState::new(entry);
    firing.tick(
        1.0,
        WeaponInput {
            select: true,
            ..Default::default()
        },
        &mut pool,
    );
    firing.set_clip(8);

    let action = firing.tick(
        0.1,
        WeaponInput {
            primary: true,
            ..Default::default()
        },
        &mut pool,
    );
    let WeaponAction::Hitscan { count, .. } = action else {
        panic!("expected a hitscan action, got {action:?}");
    };
    assert_eq!(count, 6, "the shotgun's published pellet count");

    let target = target_at(1, Vec3::new(128.0, 0.0, 64.0));
    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(256.0, 0.0, 64.0);
    // Every pellet is resolved as the same trace here: a real caller would
    // sample `count` directions inside the (BBO) spread cone and trace each.
    let trace = trace_attack(&world, &target, start, end, TraceMask::SHOT);
    let traces = vec![trace; count as usize];

    let hits = resolve_hitscan(action, &entry, &traces, None, None);
    assert_eq!(hits.len(), count as usize);
    for hit in &hits {
        assert!(
            (hit.amount - 5.0).abs() < f32::EPSILON,
            "the shotgun's published per-pellet damage"
        );
    }

    let mut health = Health::new(100.0);
    let mut total_lost = 0.0;
    for hit in &hits {
        let outcome = apply_damage(&mut health, None, hit, ohl_combat::ArmorRule::default())
            .expect("a positive amount is accepted");
        total_lost += outcome.health_lost;
    }
    assert!(
        (total_lost - 30.0).abs() < f32::EPSILON,
        "6 pellets at 5 damage each"
    );
    assert!((health.current - 70.0).abs() < f32::EPSILON);
}
