//! Property test: a [`TransitionState`] survives the save container's
//! serialization for arbitrary component values.
//!
//! The values are generated, not sampled from any game data.

use ohl_engine::transition::{
    CarriedEntity, EntitySnapshot, GlobalStateTable, MoverSnapshot, PlayerCarryState, RiderSeat,
    TrackTrainCarry, TransitionState,
};
use ohl_game::registry::{Door, GlobalStateValue, MoverState, Rotator, Transform};
use proptest::prelude::*;

/// The application section tag the round trip stores the state under; any
/// tag at or above `ohl_save::MIN_APPLICATION_TAG` would do.
const TAG: u32 = ohl_save::MIN_APPLICATION_TAG;

fn finite() -> impl Strategy<Value = f32> {
    (-100_000.0f32..100_000.0).prop_filter("finite", |value| value.is_finite())
}

fn mover_state() -> impl Strategy<Value = MoverState> {
    prop_oneof![
        Just(MoverState::Closed),
        Just(MoverState::Opening),
        Just(MoverState::Open),
        Just(MoverState::Closing),
    ]
}

fn global_value() -> impl Strategy<Value = GlobalStateValue> {
    prop_oneof![
        Just(GlobalStateValue::Off),
        Just(GlobalStateValue::On),
        Just(GlobalStateValue::Dead),
    ]
}

prop_compose! {
    fn door()(
        speed in finite(),
        wait in finite(),
        lip in finite(),
        dmg in finite(),
        health in finite(),
        delay in finite(),
        travel_distance in finite(),
        timer in finite(),
        state in mover_state(),
        movesnd in any::<u8>(),
        stopsnd in any::<u8>(),
        movedir in (finite(), finite(), finite()),
        rotation_axis in proptest::option::of((finite(), finite(), finite())),
    ) -> Door {
        Door {
            speed,
            wait,
            lip,
            movedir: glam::Vec3::new(movedir.0, movedir.1, movedir.2),
            dmg,
            health,
            delay,
            sounds: (movesnd, stopsnd),
            travel_distance,
            rotation_axis: rotation_axis.map(|(x, y, z)| glam::Vec3::new(x, y, z)),
            state,
            timer,
        }
    }
}

prop_compose! {
    fn rotator()(
        axis in (finite(), finite(), finite()),
        speed in finite(),
        spinning in any::<bool>(),
        angle_deg in finite(),
    ) -> Rotator {
        Rotator {
            axis: glam::Vec3::new(axis.0, axis.1, axis.2),
            speed,
            spinning,
            angle_deg,
        }
    }
}

prop_compose! {
    fn snapshot()(
        spawnflags in proptest::option::of(any::<u32>()),
        door in proptest::option::of(door()),
        rotator in proptest::option::of(rotator()),
        origin in (finite(), finite(), finite()),
        angles in (finite(), finite(), finite()),
    ) -> EntitySnapshot {
        EntitySnapshot {
            spawnflags,
            door,
            rotator,
            transform: Some(Transform {
                origin: glam::Vec3::new(origin.0, origin.1, origin.2),
                angles: glam::Vec3::new(angles.0, angles.1, angles.2),
            }),
            ..EntitySnapshot::default()
        }
    }
}

prop_compose! {
    /// A `func_train`/`func_tracktrain`'s carried ride state: the node it
    /// is at, by name, plus its own motion.
    fn track_train_carry()(
        node in "[a-z_]{1,16}",
        t in finite(),
        direction in finite(),
        speed in finite(),
        moving in proptest::bool::ANY,
        wait_timer in finite(),
        yaw in proptest::option::of(finite()),
    ) -> TrackTrainCarry {
        TrackTrainCarry { node, t, direction, speed, moving, wait_timer, yaw }
    }
}

prop_compose! {
    fn carried()(
        classname in "[a-z_]{1,16}",
        targetname in proptest::option::of("[a-z_]{1,16}"),
        globalname in proptest::option::of("[a-z_]{1,16}"),
        target in proptest::option::of("[a-z_]{1,16}"),
        offset in proptest::option::of((finite(), finite(), finite())),
        snapshot in snapshot(),
        track_train in proptest::option::of(track_train_carry()),
        keyvalues in proptest::collection::vec(("[a-z_]{1,8}", "[a-z_0-9 -]{0,12}"), 0..4),
    ) -> CarriedEntity {
        CarriedEntity {
            classname,
            targetname,
            globalname,
            target,
            offset: offset.map(|(x, y, z)| [x, y, z]),
            snapshot,
            track_train,
            keyvalues,
        }
    }
}

prop_compose! {
    fn rider_seat()(
        globalname in proptest::option::of("[a-z_]{1,16}"),
        targetname in proptest::option::of("[a-z_]{1,16}"),
        seat in (finite(), finite(), finite()),
        yaw in proptest::option::of(finite()),
    ) -> RiderSeat {
        RiderSeat {
            globalname,
            targetname,
            seat: [seat.0, seat.1, seat.2],
            yaw,
        }
    }
}

prop_compose! {
    fn transition()(
        landmark in "[a-z_]{1,16}",
        offset in proptest::option::of((finite(), finite(), finite())),
        yaw in finite(),
        pitch in finite(),
        health in finite(),
        armor in finite(),
        extra in proptest::collection::vec(any::<u8>(), 0..32),
        entities in proptest::collection::vec(carried(), 0..8),
        movers in proptest::collection::vec(
            ("[a-z_]{1,16}", snapshot()),
            0..4,
        ),
        globals in proptest::collection::vec(("[a-z_]{1,16}", global_value()), 0..8),
        rider in proptest::option::of(rider_seat()),
    ) -> TransitionState {
        let mut table = GlobalStateTable::new();
        for (name, value) in globals {
            table.set(name, value);
        }
        TransitionState {
            landmark,
            player_offset: offset.map(|(x, y, z)| [x, y, z]),
            yaw,
            pitch,
            player: PlayerCarryState { health, armor, extra },
            entities,
            globals: table,
            movers: movers
                .into_iter()
                .map(|(targetname, snapshot)| MoverSnapshot { targetname, snapshot })
                .collect(),
            rider,
        }
    }
}

proptest! {
    #[test]
    fn a_transition_state_round_trips_through_the_save_container(state in transition()) {
        let header = ohl_save::Header {
            game_version: "0.1.0".to_string(),
            created_at_unix_secs: 0,
            map_identity: "ohl-synthetic".to_string(),
            title: "Transition".to_string(),
            thumbnail: Vec::new(),
        };
        let mut writer = ohl_save::SaveWriter::begin(header);
        writer
            .add_section_serde(TAG, &state)
            .expect("the state serializes");
        let bytes = writer
            .finish(&ohl_save::Limits::default())
            .expect("the container is written");

        let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default())
            .expect("the container opens");
        let decoded: TransitionState = reader.deserialize(TAG).expect("the state deserializes");
        prop_assert_eq!(decoded, state);
    }
}

/// A `func_rotating` the player switched on travels across a level
/// transition: `EntitySnapshot` carries its `spinning`/`angle_deg` (the
/// gap `docs/FORMAT_SOURCES.md`'s item 24 recorded, where only the save
/// container's own mover-state section carried it), and
/// `is_modified_mover` recognises a spinning rotator as worth carrying at
/// all.
#[test]
fn a_spinning_rotator_travels_through_an_entity_snapshot() {
    use std::collections::BTreeMap;

    use ohl_game::keyvalues::{Limits, parse_entities};
    use ohl_game::registry::Registry;

    let entity_text: ohl_formats::bsp30::Entity = [
        ("classname".to_string(), "func_rotating".to_string()),
        ("targetname".to_string(), "spinner".to_string()),
        ("speed".to_string(), "90".to_string()),
    ]
    .into_iter()
    .collect();
    let defs = parse_entities(&[entity_text], &Limits::default());
    let source = Registry::build(&defs, &BTreeMap::default(), &Limits::default());
    let entity = source.find("spinner")[0];
    {
        let mut rotator = source
            .world
            .get::<&mut Rotator>(entity)
            .expect("a func_rotating carries a Rotator");
        rotator.spinning = true;
        rotator.angle_deg = 123.5;
    }

    let snapshot = EntitySnapshot::capture(&source, entity);
    let carried = snapshot.rotator.expect("the rotator is captured");
    assert!(carried.spinning);
    assert!((carried.angle_deg - 123.5).abs() < 1e-4);
    assert!(
        snapshot.is_modified_mover(),
        "a spinning rotator is a modified mover"
    );

    // Apply it onto a fresh copy of the same map, whose rotator is idle.
    let mut destination = Registry::build(&defs, &BTreeMap::default(), &Limits::default());
    let destination_entity = destination.find("spinner")[0];
    assert!(
        !destination
            .world
            .get::<&Rotator>(destination_entity)
            .expect("a func_rotating carries a Rotator")
            .spinning
    );
    snapshot.apply(&mut destination, destination_entity);
    let restored = destination
        .world
        .get::<&Rotator>(destination_entity)
        .expect("a func_rotating carries a Rotator");
    assert!(restored.spinning);
    assert!((restored.angle_deg - 123.5).abs() < 1e-4);
}
