//! Synthetic source-brush exclusion; all other world/actor geometry stays live.

use ohl_combat::{
    EntityHitboxes, EntityId, HitGroup, HitboxIndex, HitboxLimits, ProjectileEvent, ProjectileKind,
    ProjectileLimits, ProjectileSet, ProjectileTuning, ProjectileWorld, TraceFilter, TraceMask,
    Vec3, trace_attack_filtered, trace_attack_filtered_ignoring_brush,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_physics::{BrushId, CollisionModel, MoveConfig};

fn fixture() -> (CollisionModel, BrushId, BrushId, HitboxIndex) {
    let mut builder = Bsp30Builder::new();
    let wall = builder.push_collision_hulls(&[CollisionBrush::box_brush(
        [192.0, -32.0, -32.0],
        [200.0, 32.0, 32.0],
    )]);
    let cube = builder.push_collision_hulls(&[CollisionBrush::box_brush([-8.0; 3], [8.0; 3])]);
    builder.push_model([-256.0; 3], [256.0; 3], [0.0; 3], wall, 2, 0, 0);
    builder.push_model([-8.0; 3], [8.0; 3], [0.0; 3], cube, 2, 0, 0);
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    let mut collision = CollisionModel::from_bsp(&bsp, &limits).unwrap();
    let source = collision
        .attach_brush(&bsp, &limits, 1, Vec3::ZERO)
        .unwrap();
    let blocker = collision
        .attach_brush(&bsp, &limits, 1, Vec3::X * 64.0)
        .unwrap();
    let mut entities = HitboxIndex::new(HitboxLimits::default());
    for (id, point) in [(101, Vec3::ZERO), (202, Vec3::X * 128.0)] {
        let mut hitboxes = EntityHitboxes::new(EntityId(id), point);
        hitboxes.push_box(0, Vec3::splat(-4.0), Vec3::splat(4.0), HitGroup::Generic);
        assert!(entities.push(hitboxes));
    }
    (collision, source, blocker, entities)
}

#[test]
fn ignoring_only_source_keeps_moving_brush_actor_and_world_occlusion() {
    let (mut collision, source, blocker, entities) = fixture();
    let filter = TraceFilter::ignoring(TraceMask::SHOT, EntityId(101));
    let end = Vec3::X * 256.0;
    let legacy = trace_attack_filtered(&collision, &entities, Vec3::ZERO, end, filter);
    assert!(legacy.end.x.abs() < 0.1);
    let blocked = trace_attack_filtered_ignoring_brush(
        &collision,
        &entities,
        Vec3::ZERO,
        end,
        filter,
        Some(source),
    );
    assert!(blocked.entity.is_none());
    assert!((blocked.end.x - 56.0).abs() < 0.1);
    collision.set_brush_origin(blocker, Vec3::new(64.0, 80.0, 0.0));
    let actor = trace_attack_filtered_ignoring_brush(
        &collision,
        &entities,
        Vec3::ZERO,
        end,
        filter,
        Some(source),
    );
    assert_eq!(actor.entity, Some(EntityId(202)));
    assert!((actor.end.x - 124.0).abs() < 0.1);
    let empty = HitboxIndex::new(HitboxLimits::default());
    let world = trace_attack_filtered_ignoring_brush(
        &collision,
        &empty,
        Vec3::ZERO,
        end,
        filter,
        Some(source),
    );
    assert!(world.entity.is_none());
    assert!((world.end.x - 192.0).abs() < 0.1);
}

#[test]
fn physical_rocket_exits_source_brush_and_detonates_on_separate_blocker() {
    let (collision, source, _, entities) = fixture();
    let tuning = ProjectileTuning::default();
    let movement = MoveConfig::default();
    let world = ProjectileWorld {
        collision: &collision,
        entities: &entities,
        movement: &movement,
        tuning: &tuning,
    };
    let mut set = ProjectileSet::new(ProjectileLimits::default(), 1);
    let id = set
        .spawn(
            ProjectileKind::Rocket,
            Some(EntityId(101)),
            Vec3::ZERO,
            Vec3::X * 1000.0,
            &tuning,
        )
        .unwrap();
    let mut events = Vec::new();
    set.tick_with_brush_filter(0.01, &world, &mut events, |query| {
        (query == id).then_some(source)
    });
    assert!(events.is_empty());
    assert!((set.get(id).unwrap().position.x - 10.0).abs() < 0.01);
    set.tick_with_brush_filter(0.1, &world, &mut events, |query| {
        (query == id).then_some(source)
    });
    assert!(set.get(id).is_none());
    assert!(events.iter().any(|event| matches!(event,
        ProjectileEvent::Detonate { id: found, owner: Some(EntityId(101)), position, .. }
            if *found == id && (position.x - 56.0).abs() < 0.1
    )));
}
