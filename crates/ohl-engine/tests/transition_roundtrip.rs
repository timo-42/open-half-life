//! Property test: a [`TransitionState`] survives the save container's
//! serialization for arbitrary component values.
//!
//! The values are generated, not sampled from any game data.

use ohl_engine::transition::{
    CarriedEntity, EntitySnapshot, GlobalStateTable, MoverSnapshot, PlayerCarryState,
    TransitionState,
};
use ohl_game::registry::{Door, GlobalStateValue, MoverState, Transform};
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
            state,
            timer,
        }
    }
}

prop_compose! {
    fn snapshot()(
        spawnflags in proptest::option::of(any::<u32>()),
        door in proptest::option::of(door()),
        origin in (finite(), finite(), finite()),
        angles in (finite(), finite(), finite()),
    ) -> EntitySnapshot {
        EntitySnapshot {
            spawnflags,
            door,
            transform: Some(Transform {
                origin: glam::Vec3::new(origin.0, origin.1, origin.2),
                angles: glam::Vec3::new(angles.0, angles.1, angles.2),
            }),
            ..EntitySnapshot::default()
        }
    }
}

prop_compose! {
    fn carried()(
        classname in "[a-z_]{1,16}",
        targetname in proptest::option::of("[a-z_]{1,16}"),
        globalname in proptest::option::of("[a-z_]{1,16}"),
        target in proptest::option::of("[a-z_]{1,16}"),
        offset in (finite(), finite(), finite()),
        snapshot in snapshot(),
    ) -> CarriedEntity {
        CarriedEntity {
            classname,
            targetname,
            globalname,
            target,
            offset: [offset.0, offset.1, offset.2],
            snapshot,
        }
    }
}

prop_compose! {
    fn transition()(
        landmark in "[a-z_]{1,16}",
        offset in (finite(), finite(), finite()),
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
    ) -> TransitionState {
        let mut table = GlobalStateTable::new();
        for (name, value) in globals {
            table.set(name, value);
        }
        TransitionState {
            landmark,
            player_offset: [offset.0, offset.1, offset.2],
            yaw,
            pitch,
            player: PlayerCarryState { health, armor, extra },
            entities,
            globals: table,
            movers: movers
                .into_iter()
                .map(|(targetname, snapshot)| MoverSnapshot { targetname, snapshot })
                .collect(),
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
