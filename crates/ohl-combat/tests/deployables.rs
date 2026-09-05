//! Satchel charges and tripmines in the project's synthetic collision room.

use ohl_combat::deployables::{
    DeployableEvent, DeployableKind, DeployableSet, DeployableTuning, MAX_SATCHELS, MAX_TRIPMINES,
    TRIPMINE_ARM_SECONDS,
};
use ohl_combat::{EntityHitboxes, EntityId, HitGroup, HitboxIndex, HitboxLimits, Vec3};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::CollisionModel;

const TICK: f32 = 0.01;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// How many fixed ticks `seconds` is, rounded to the nearest whole tick.
fn ticks_for(seconds: f32) -> u32 {
    let ticks = (seconds / TICK).round();
    assert!(ticks.is_finite() && ticks >= 0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ticks = ticks as u32;
    ticks
}

fn empty_index() -> HitboxIndex {
    HitboxIndex::new(HitboxLimits::default())
}

fn cube_index(id: u64, origin: Vec3) -> HitboxIndex {
    let mut entity = EntityHitboxes::new(EntityId(id), origin);
    entity.push_box(0, Vec3::splat(-16.0), Vec3::splat(16.0), HitGroup::Generic);
    let mut index = HitboxIndex::new(HitboxLimits::default());
    assert!(index.push(entity));
    index
}

#[test]
fn satchels_respect_the_published_carry_maximum() {
    let tuning = DeployableTuning::default();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    for step in 0..MAX_SATCHELS {
        let offset = f32::from(u8::try_from(step).expect("five fits in a byte"));
        assert!(
            set.place_satchel(
                Some(EntityId(1)),
                Vec3::new(offset * 32.0, 0.0, 0.0),
                &mut events
            )
            .is_some()
        );
    }
    assert_eq!(set.satchels().len(), MAX_SATCHELS);
    assert!(
        set.place_satchel(Some(EntityId(1)), Vec3::ZERO, &mut events)
            .is_none(),
        "a sixth satchel is refused"
    );
    assert_eq!(
        events.len(),
        MAX_SATCHELS,
        "one Placed event per accepted charge"
    );
    assert!(
        events
            .iter()
            .all(|event| matches!(event, DeployableEvent::Placed { kind, .. } if *kind == DeployableKind::Satchel))
    );

    events.clear();
    assert_eq!(
        set.detonate_all_satchels(&tuning, &mut events),
        MAX_SATCHELS
    );
    assert_eq!(events.len(), MAX_SATCHELS, "every charge goes off at once");
    assert!(set.satchels().is_empty());
    for event in &events {
        let DeployableEvent::Detonated {
            kind,
            owner,
            radius,
            ..
        } = event
        else {
            panic!("expected a detonation, got {event:?}");
        };
        assert_eq!(*kind, DeployableKind::Satchel);
        assert_eq!(*owner, Some(EntityId(1)));
        assert!((*radius - tuning.satchel_radius.value).abs() < 1e-6);
    }

    events.clear();
    assert_eq!(
        set.detonate_all_satchels(&tuning, &mut events),
        0,
        "detonating twice does nothing the second time"
    );
    assert!(events.is_empty());
}

#[test]
fn a_tripmine_sticks_to_the_wall_a_trace_found() {
    let world = room();
    let tuning = DeployableTuning::default();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();

    let id = set
        .place_tripmine(
            Some(EntityId(1)),
            Vec3::new(200.0, 0.0, 64.0),
            Vec3::X,
            &world,
            &tuning,
            &mut events,
        )
        .expect("the +X wall is within reach");
    let mine = set.tripmines()[0];
    assert_eq!(mine.id, id);
    assert!(
        (mine.position.x - 256.0).abs() < 1.0,
        "the mine sits on the wall: {:?}",
        mine.position
    );
    assert_eq!(mine.normal, -Vec3::X, "its beam points back into the room");
    assert!(!mine.armed);

    // Nothing within reach: no mine.
    let mut aimless = DeployableSet::new();
    assert!(
        aimless
            .place_tripmine(
                None,
                Vec3::ZERO + Vec3::Z * 64.0,
                Vec3::X,
                &world,
                &tuning,
                &mut events,
            )
            .is_none(),
        "a mine needs a surface within the placement range"
    );
    assert!(
        aimless
            .place_tripmine(
                None,
                Vec3::new(200.0, 0.0, 64.0),
                Vec3::ZERO,
                &world,
                &tuning,
                &mut events,
            )
            .is_none(),
        "a mine needs a direction"
    );
}

#[test]
fn tripmines_respect_the_published_carry_maximum() {
    let world = room();
    let tuning = DeployableTuning::default();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    for step in 0..MAX_TRIPMINES {
        let offset = f32::from(u8::try_from(step).expect("five fits in a byte"));
        assert!(
            set.place_tripmine(
                None,
                Vec3::new(200.0, offset * 16.0, 64.0),
                Vec3::X,
                &world,
                &tuning,
                &mut events,
            )
            .is_some()
        );
    }
    assert!(
        set.place_tripmine(
            None,
            Vec3::new(200.0, 0.0, 96.0),
            Vec3::X,
            &world,
            &tuning,
            &mut events,
        )
        .is_none(),
        "a sixth tripmine is refused"
    );
}

#[test]
fn a_tripmine_arms_after_the_published_three_seconds_and_only_then_trips() {
    let world = room();
    let tuning = DeployableTuning::default();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    set.place_tripmine(
        Some(EntityId(1)),
        Vec3::new(200.0, 0.0, 64.0),
        Vec3::X,
        &world,
        &tuning,
        &mut events,
    )
    .expect("the wall is within reach");
    events.clear();

    // Something is already standing in the beam, but the mine is not armed.
    let victim = cube_index(5, Vec3::new(100.0, 0.0, 64.0));
    let ticks = ticks_for(TRIPMINE_ARM_SECONDS - 0.1);
    for _ in 0..ticks {
        set.tick(TICK, &world, &victim, &tuning, &mut events);
    }
    assert!(
        events.is_empty(),
        "an unarmed mine neither arms nor trips: {events:?}"
    );
    assert!(!set.tripmines()[0].armed);

    // One more tick past the arming delay arms it, and the very next beam
    // trace trips it.
    for _ in 0..20 {
        set.tick(TICK, &world, &victim, &tuning, &mut events);
    }
    assert!(
        matches!(events.first(), Some(DeployableEvent::Armed { .. })),
        "the mine arms first: {events:?}"
    );
    let detonation = events
        .iter()
        .find(|event| matches!(event, DeployableEvent::Detonated { .. }))
        .expect("an armed mine trips on the target in its beam");
    let DeployableEvent::Detonated {
        kind,
        owner,
        radius,
        ..
    } = detonation
    else {
        unreachable!()
    };
    assert_eq!(*kind, DeployableKind::Tripmine);
    assert_eq!(*owner, Some(EntityId(1)));
    assert!((*radius - tuning.tripmine_radius.value).abs() < 1e-6);
    assert!(set.tripmines().is_empty(), "a tripped mine is removed");
}

#[test]
fn an_armed_tripmine_with_a_clear_beam_stays_put() {
    let world = room();
    let tuning = DeployableTuning::default();
    let empty = empty_index();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    let id = set
        .place_tripmine(
            None,
            Vec3::new(200.0, 0.0, 64.0),
            Vec3::X,
            &world,
            &tuning,
            &mut events,
        )
        .expect("the wall is within reach");

    for _ in 0..1000 {
        set.tick(TICK, &world, &empty, &tuning, &mut events);
    }
    assert_eq!(set.tripmines().len(), 1, "nothing crossed the beam");
    assert!(set.tripmines()[0].armed);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, DeployableEvent::Armed { .. }))
            .count(),
        1,
        "the arming cue fires exactly once"
    );

    // The beam ends on the far wall.
    let end = set
        .beam_end(id, &world, &empty, &tuning)
        .expect("an armed mine has a beam");
    assert!(
        end.x < set.tripmines()[0].position.x,
        "the beam runs along the mine's normal: {end:?}"
    );
}

#[test]
fn a_charge_can_be_detonated_by_handle() {
    let world = room();
    let tuning = DeployableTuning::default();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    let satchel = set
        .place_satchel(None, Vec3::new(0.0, 0.0, 16.0), &mut events)
        .expect("room for a satchel");
    let mine = set
        .place_tripmine(
            None,
            Vec3::new(200.0, 0.0, 64.0),
            Vec3::X,
            &world,
            &tuning,
            &mut events,
        )
        .expect("room for a mine");
    events.clear();

    assert!(set.detonate(satchel, &tuning, &mut events));
    assert!(set.detonate(mine, &tuning, &mut events));
    assert!(!set.detonate(satchel, &tuning, &mut events), "only once");
    assert_eq!(events.len(), 2);
    assert!(set.satchels().is_empty() && set.tripmines().is_empty());
}

#[test]
fn a_non_positive_tick_changes_nothing() {
    let world = room();
    let tuning = DeployableTuning::default();
    let empty = empty_index();
    let mut set = DeployableSet::new();
    let mut events = Vec::new();
    set.place_satchel(None, Vec3::ZERO, &mut events);
    events.clear();
    for dt in [0.0, -1.0, f32::NAN] {
        set.tick(dt, &world, &empty, &tuning, &mut events);
    }
    assert!(events.is_empty());
    assert!(set.satchels()[0].age < 1e-6, "a rejected tick does not age");
}
