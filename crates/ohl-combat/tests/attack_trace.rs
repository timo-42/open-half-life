//! Attack traces against the project's synthetic collision room and a
//! synthetic two-bone hitbox skeleton.
//!
//! No game data is loaded: the map comes from `ohl_formats::test_support`'s
//! collision-room fixture and the "model" is a hand-built [`StudioPose`] with
//! two bone matrices, which is all `hitbox_bounds` needs.

use ohl_combat::{
    AttackTrace, EntityHitboxes, EntityId, HitGroup, HitboxIndex, HitboxLimits, Quat, TraceFilter,
    TraceMask, Vec3, trace_attack, trace_attack_filtered,
};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::CollisionModel;
use ohl_world::{BoneMatrix, StudioHitbox, StudioPose};

/// The synthetic room: interior `[-256, 256]` on X and Y, `[0, 256]` on Z,
/// with an 18-unit step at `x` 64..192 and a 19-unit ledge at `x` -192..-64.
fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// A column-major translation matrix, the layout [`BoneMatrix`] uses.
fn translation(x: f32, y: f32, z: f32) -> BoneMatrix {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        x, y, z, 1.0,
    ]
}

/// A two-bone skeleton: bone 0 sits 16 units above the entity origin (the
/// "head"), bone 1 at the origin (the "chest").
fn two_bone_pose() -> StudioPose {
    StudioPose {
        matrices: vec![translation(0.0, 0.0, 16.0), translation(0.0, 0.0, 0.0)],
    }
}

/// One 8-unit cube per bone, in the published head/chest hit groups.
fn two_bone_hitboxes() -> Vec<StudioHitbox> {
    vec![
        StudioHitbox {
            bone: 0,
            group: HitGroup::Head.index(),
            min: [-4.0, -4.0, -4.0],
            max: [4.0, 4.0, 4.0],
        },
        StudioHitbox {
            bone: 1,
            group: HitGroup::Chest.index(),
            min: [-8.0, -8.0, -8.0],
            max: [8.0, 8.0, 8.0],
        },
    ]
}

/// A posed entity standing at `origin`, unrotated.
fn posed_entity(id: u64, origin: Vec3) -> EntityHitboxes {
    let mut entity = EntityHitboxes::new(EntityId(id), origin);
    let added = entity.push_studio_hitboxes(&two_bone_pose(), &two_bone_hitboxes());
    assert_eq!(added, 2, "both hitboxes have a posed bone");
    entity
}

fn index_of(entities: Vec<EntityHitboxes>) -> HitboxIndex {
    let mut index = HitboxIndex::new(HitboxLimits::default());
    for entity in entities {
        assert!(index.push(entity));
    }
    index
}

/// Asserts the documented invariant that the impact point lies on the traced
/// segment at the reported fraction.
fn assert_on_segment(trace: &AttackTrace, start: Vec3, end: Vec3) {
    let expected = start + (end - start) * trace.fraction;
    assert!(
        (trace.end - expected).length() < 0.05,
        "end {:?} is not at fraction {} of {start:?} -> {end:?}",
        trace.end,
        trace.fraction
    );
}

#[test]
fn a_shot_at_head_height_reports_the_head_hitbox() {
    let world = room();
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 64.0))]);
    let start = Vec3::new(0.0, 0.0, 80.0);
    let end = Vec3::new(256.0, 0.0, 80.0);

    let trace = trace_attack(&world, &entities, start, end, TraceMask::SHOT);

    assert_eq!(trace.entity, Some(EntityId(7)));
    assert_eq!(trace.hitbox, Some(0));
    assert_eq!(trace.hitgroup, HitGroup::Head);
    // The head cube's near face is at x = 124.
    assert!((trace.end.x - 124.0).abs() < 0.01, "end {:?}", trace.end);
    assert!((trace.surface_normal - Vec3::NEG_X).length() < 1e-3);
    assert_on_segment(&trace, start, end);
}

#[test]
fn a_shot_at_chest_height_reports_the_chest_hitbox() {
    let world = room();
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 64.0))]);
    let trace = trace_attack(
        &world,
        &entities,
        Vec3::new(0.0, 0.0, 64.0),
        Vec3::new(256.0, 0.0, 64.0),
        TraceMask::SHOT,
    );

    assert_eq!(trace.hitbox, Some(1));
    assert_eq!(trace.hitgroup, HitGroup::Chest);
    assert!((trace.end.x - 120.0).abs() < 0.01, "end {:?}", trace.end);
}

#[test]
fn a_wall_in_front_of_the_entity_takes_the_shot_instead() {
    let world = room();
    // The 19-unit ledge spans x -192..-64; both the shooter and the target
    // are below its top, so its near face is the first thing in the way.
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 10.0))]);
    let start = Vec3::new(-230.0, 0.0, 10.0);
    let end = Vec3::new(240.0, 0.0, 10.0);

    let blocked = trace_attack(&world, &entities, start, end, TraceMask::SHOT);
    assert_eq!(blocked.entity, None, "the ledge is nearer than the entity");
    assert_eq!(blocked.hitgroup, HitGroup::Generic);
    assert!(
        (blocked.end.x - -192.0).abs() < 0.1,
        "end {:?}",
        blocked.end
    );
    assert!((blocked.surface_normal - Vec3::NEG_X).length() < 1e-3);
    assert_on_segment(&blocked, start, end);

    // Ignoring the world, the same shot reaches the entity, so the ordering
    // and not the geometry is what stopped it.
    let through = trace_attack(&world, &entities, start, end, TraceMask::ENTITIES_ONLY);
    assert_eq!(through.entity, Some(EntityId(7)));
    assert!(through.fraction > blocked.fraction);
}

#[test]
fn aiming_past_the_entity_misses_it() {
    let world = room();
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 64.0))]);
    let start = Vec3::new(0.0, 0.0, 64.0);
    // Same height, but the line passes 32 units to the side of a
    // 16-unit-wide chest box, and carries on into the far wall.
    let end = Vec3::new(512.0, 128.0, 64.0);

    let trace = trace_attack(&world, &entities, start, end, TraceMask::SHOT);
    assert_eq!(trace.entity, None);
    assert!(trace.hit(), "the far wall still stops the shot");

    let entities_only = trace_attack(&world, &entities, start, end, TraceMask::ENTITIES_ONLY);
    assert_eq!(entities_only, AttackTrace::miss(end));
    assert!((entities_only.fraction - 1.0).abs() < f32::EPSILON);
}

#[test]
fn the_nearest_of_two_entities_wins() {
    let world = room();
    let entities = index_of(vec![
        posed_entity(1, Vec3::new(192.0, 0.0, 64.0)),
        posed_entity(2, Vec3::new(96.0, 0.0, 64.0)),
    ]);
    let trace = trace_attack(
        &world,
        &entities,
        Vec3::new(0.0, 0.0, 64.0),
        Vec3::new(256.0, 0.0, 64.0),
        TraceMask::SHOT,
    );
    assert_eq!(trace.entity, Some(EntityId(2)));
}

#[test]
fn a_rotated_entity_is_hit_through_its_oriented_box() {
    let world = room();
    // A flat, long box: 64 units on X, 4 on Y, rotated a quarter turn so it
    // lies across the line of fire instead of along it.
    let mut entity = EntityHitboxes::new(EntityId(3), Vec3::new(128.0, 0.0, 64.0));
    entity.push_box(
        0,
        Vec3::new(-64.0, -4.0, -4.0),
        Vec3::new(64.0, 4.0, 4.0),
        HitGroup::Chest,
    );
    let across = entity
        .clone()
        .with_rotation(Quat::from_rotation_z(core::f32::consts::FRAC_PI_2));

    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(256.0, 0.0, 64.0);
    let along = trace_attack(&world, &index_of(vec![entity]), start, end, TraceMask::SHOT);
    let rotated = trace_attack(&world, &index_of(vec![across]), start, end, TraceMask::SHOT);

    // Unrotated, the box reaches back to x = 64; rotated, only to x = 124.
    assert!((along.end.x - 64.0).abs() < 0.01, "end {:?}", along.end);
    assert!(
        (rotated.end.x - 124.0).abs() < 0.01,
        "end {:?}",
        rotated.end
    );
}

#[test]
fn an_entity_with_no_usable_hitbox_is_rejected_by_the_index() {
    let mut index = HitboxIndex::new(HitboxLimits {
        max_entities: 1,
        max_boxes_per_entity: 1,
    });
    assert!(!index.push(EntityHitboxes::new(EntityId(1), Vec3::ZERO)));
    assert!(index.is_empty());
    assert_eq!(index.rejected(), 1);

    // The limits truncate rather than fail, and count the truncation.
    assert!(index.push(posed_entity(2, Vec3::ZERO)));
    assert_eq!(index.entries()[0].boxes.len(), 1);
    assert_eq!(index.rejected(), 2);

    // The index is now full.
    assert!(!index.push(posed_entity(3, Vec3::ZERO)));
    assert_eq!(index.len(), 1);

    index.clear();
    assert!(index.is_empty() && index.rejected() == 0);
}

#[test]
fn a_hitbox_whose_bone_is_missing_from_the_pose_is_skipped() {
    let pose = StudioPose {
        matrices: vec![translation(0.0, 0.0, 0.0)],
    };
    let mut entity = EntityHitboxes::new(EntityId(1), Vec3::ZERO);
    assert_eq!(entity.push_studio_hitboxes(&pose, &two_bone_hitboxes()), 1);
    assert_eq!(entity.boxes[0].group, HitGroup::Head);
}

#[test]
fn a_degenerate_or_non_finite_segment_is_a_miss() {
    let world = room();
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 64.0))]);
    let point = Vec3::new(0.0, 0.0, 64.0);
    let degenerate = trace_attack(&world, &entities, point, point, TraceMask::SHOT);
    assert_eq!(degenerate.entity, None);

    let nan = Vec3::new(f32::NAN, 0.0, 0.0);
    let trace = trace_attack(&world, &entities, nan, point, TraceMask::SHOT);
    assert_eq!(trace, AttackTrace::miss(point));
}

#[test]
fn the_hit_group_vocabulary_round_trips() {
    for group in [
        HitGroup::Generic,
        HitGroup::Head,
        HitGroup::Chest,
        HitGroup::Stomach,
        HitGroup::LeftArm,
        HitGroup::RightArm,
        HitGroup::LeftLeg,
        HitGroup::RightLeg,
    ] {
        assert_eq!(HitGroup::from_index(group.index()), group);
    }
    // Untrusted model data outside the published range is not rejected.
    assert_eq!(HitGroup::from_index(-3), HitGroup::Generic);
    assert_eq!(HitGroup::from_index(99), HitGroup::Generic);
}

#[test]
fn an_ignored_entity_in_front_of_another_is_passed_through() {
    let world = room();
    // Entity 1 (the owner) stands nearer the shooter than entity 2; without
    // a filter it would be the nearer hit.
    let entities = index_of(vec![
        posed_entity(1, Vec3::new(96.0, 0.0, 64.0)),
        posed_entity(2, Vec3::new(192.0, 0.0, 64.0)),
    ]);
    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(256.0, 0.0, 64.0);

    let unfiltered = trace_attack(&world, &entities, start, end, TraceMask::SHOT);
    assert_eq!(unfiltered.entity, Some(EntityId(1)));

    let filter = TraceFilter::ignoring(TraceMask::SHOT, EntityId(1));
    let filtered = trace_attack_filtered(&world, &entities, start, end, filter);
    assert_eq!(filtered.entity, Some(EntityId(2)));
    assert_on_segment(&filtered, start, end);
}

#[test]
fn ignoring_both_entities_returns_the_world_hit() {
    let world = room();
    let entities = index_of(vec![
        posed_entity(1, Vec3::new(96.0, 0.0, 64.0)),
        posed_entity(2, Vec3::new(192.0, 0.0, 64.0)),
    ]);
    // Both entities sit well inside the room, closer than the far wall; the
    // segment reaches past the wall at x = 256 so the world still stops it.
    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(512.0, 0.0, 64.0);

    let filter = TraceFilter {
        ignore: [Some(EntityId(1)), Some(EntityId(2))],
        mask: TraceMask::SHOT,
    };
    let trace = trace_attack_filtered(&world, &entities, start, end, filter);

    assert_eq!(trace.entity, None, "both candidates are ignored");
    assert!(trace.hit(), "the far wall still stops the shot");
    assert_on_segment(&trace, start, end);

    // With no entities ignored, the world result is the same trace ...
    let unfiltered = trace_attack(&world, &entities, start, end, TraceMask::WORLD_ONLY);
    assert!((unfiltered.fraction - trace.fraction).abs() < f32::EPSILON);
    assert_eq!(unfiltered.end, trace.end);
}

#[test]
fn trace_attack_is_equivalent_to_an_empty_filter() {
    let world = room();
    let entities = index_of(vec![posed_entity(7, Vec3::new(128.0, 0.0, 64.0))]);
    let start = Vec3::new(0.0, 0.0, 64.0);
    let end = Vec3::new(256.0, 0.0, 64.0);

    let plain = trace_attack(&world, &entities, start, end, TraceMask::SHOT);
    let via_filter = trace_attack_filtered(
        &world,
        &entities,
        start,
        end,
        TraceFilter::new(TraceMask::SHOT),
    );
    assert_eq!(plain, via_filter);
}
