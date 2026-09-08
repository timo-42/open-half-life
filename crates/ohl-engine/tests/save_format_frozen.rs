//! Byte-level guards for the save sections whose `postcard` wire shape is
//! frozen. Tags 16, 17, 18, 19, 20, 21, 22 and 28 are pinned here at the
//! shapes `f64ccfc` writes — the shapes every save file any build of this
//! project has produced *and can still open* uses.
//!
//! `postcard` is not self-describing, so a section is decoded as one fixed
//! shape, field for field. Adding (or removing) a field on a type any of
//! these tags serializes therefore does not "extend" the format — it
//! invalidates every save file already written whose section for that tag is
//! non-empty. See `ohl_engine::save`'s "Frozen section shapes" module doc,
//! which also records the compatibility floor these tags actually set, and
//! `docs/FORMAT_SOURCES.md` `TODO(black-box)` items 27 and 28.
//!
//! Each test below owns a committed golden byte array: the exact bytes the
//! current format writes for a known value. The tests
//!
//! 1. re-encode that same known value with today's types and assert the
//!    bytes are unchanged (so a shape change anywhere in the section's
//!    transitive type graph — a new `Door` field, a widened enum — fails
//!    here), and
//! 2. decode the committed bytes with today's reader and assert every
//!    field lands where it belongs.
//!
//! Two further tests cover the *other* half of the guarantee, which a review
//! found missing: that a section whose bytes do not match the reader's shape
//! fails **closed**. Neither `postcard::from_bytes` nor a hand-driven
//! `postcard::Deserializer` rejects unconsumed input on its own, so a reader
//! one field shorter than the writer used to decode such a section happily,
//! misassigning every later field. Every section-decode path in this project
//! now requires its bytes to be consumed exactly.
//!
//! Regenerating a golden is not a routine action. If one of these fails, the
//! section's wire shape changed, and every save file already written is now
//! unreadable by this build — put the new state in a new optional tag
//! instead (`ohl_engine::save`'s module doc states the rule).
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

// Exact float comparison is the point: a decoded golden either reproduces
// the written value bit for bit or the shape has drifted.
#![allow(clippy::float_cmp)]
// `..Default::default()` on values that already name every field is
// deliberate here, and the lint is the opposite of what this file wants: it
// is what makes a *newly added* field compile in these fixtures, so the
// failure a maintainer sees is the golden-byte mismatch that explains the
// save-compatibility break, not a build error in the test's own scaffolding.
#![allow(clippy::needless_update)]

use glam::Vec3;
use ohl_engine::save::{EngineHeader, TeleportStateSnapshot, ViewState};
use ohl_engine::save_state::{
    BreakableSnapshot, MomentaryDoorSnapshot, MonsterMakerSnapshot, MoverSnapshot, RotatorSnapshot,
    ScriptRunnerSnapshot, TrackTrainSnapshot, TriggerCameraSnapshot,
};
use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_NAME, rotating_door_bsp, rotating_door_entities,
};
use ohl_engine::transition::{EntitySnapshot, GlobalStateTable, PlayerCarryState};
use ohl_engine::{AssetSource, EngineError, Game, Input, MemoryAssets};
use ohl_game::registry::{
    Button, Door, GlobalStateValue, Light, Message, MoverState, Platform, RenderPropsComponent,
    Rotator, Transform, Trigger,
};
use ohl_game::{PendingFire, SimulationState, TriggerSnapshot};
use serde::Serialize;

/// `SECTION_ENGINE_HEADER` (16) at its frozen shape: the exact bytes
/// [`frozen_engine_header`]'s value encodes to.
const GOLDEN_TAG_16: &[u8] = &[
    0x0e, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x6d, 0x61, 0x70, 0x01,
    0x12, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x63, 0x68, 0x61, 0x70,
    0x74, 0x65, 0x72, 0x02, 0x00, 0x00, 0x2a, 0x42,
];

/// `SECTION_PLAYER_CARRY` (17) at its frozen shape: the exact bytes
/// [`frozen_player_carry`]'s value encodes to.
const GOLDEN_TAG_17: &[u8] = &[
    0x00, 0x00, 0xaf, 0x42, 0x00, 0x00, 0xc8, 0x41, 0x04, 0x01, 0x02, 0x03, 0xfa,
];

/// `SECTION_GLOBAL_STATE` (20) at its frozen shape: the exact bytes
/// [`frozen_globals`]'s value encodes to.
const GOLDEN_TAG_20: &[u8] = &[
    0x03, 0x0f, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x64, 0x65, 0x61,
    0x64, 0x02, 0x0e, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x6f, 0x66,
    0x66, 0x00, 0x0d, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x6f, 0x6e,
    0x01,
];

/// `SECTION_LIGHT_STYLE_TIME` (21) at its frozen shape: a bare `f32`.
const GOLDEN_TAG_21: &[u8] = &[0x00, 0x00, 0x48, 0x41];

/// `SECTION_VIEW` (22) at its frozen shape: the exact bytes
/// [`frozen_view`]'s value encodes to.
const GOLDEN_TAG_22: &[u8] = &[
    0x00, 0x00, 0x20, 0x41, 0x00, 0x00, 0x60, 0xc1, 0x00, 0x00, 0x88, 0x42, 0x00, 0x00, 0x34, 0x42,
    0x00, 0x00, 0x48, 0xc1,
];

/// The value [`GOLDEN_TAG_16`] holds.
fn frozen_engine_header() -> EngineHeader {
    EngineHeader {
        map: "ohl_frozen_map".to_string(),
        chapter_title: Some("ohl_frozen_chapter".to_string()),
        difficulty: 2,
        elapsed: 42.5,
    }
}

/// The value [`GOLDEN_TAG_17`] holds.
fn frozen_player_carry() -> PlayerCarryState {
    PlayerCarryState {
        health: 87.5,
        armor: 25.0,
        extra: vec![1, 2, 3, 250],
    }
}

/// The value [`GOLDEN_TAG_20`] holds: one variable per
/// `GlobalStateValue` arm, so every discriminant is on the wire.
fn frozen_globals() -> GlobalStateTable {
    let mut globals = GlobalStateTable::new();
    globals.set("ohl_frozen_off", GlobalStateValue::Off);
    globals.set("ohl_frozen_on", GlobalStateValue::On);
    globals.set("ohl_frozen_dead", GlobalStateValue::Dead);
    globals
}

/// The value [`GOLDEN_TAG_22`] holds.
fn frozen_view() -> ViewState {
    ViewState {
        position: [10.0, -14.0, 68.0],
        yaw: 45.0,
        pitch: -12.5,
    }
}

/// `SECTION_ENTITY_REGISTRY` (18) at its frozen shape: the exact bytes
/// [`frozen_entity_registry`]'s value encodes to.
const GOLDEN_TAG_18: &[u8] = &[
    0x02, 0x01, 0xd2, 0x09, 0x01, 0x0a, 0x16, 0x16, 0x21, 0xff, 0x01, 0x00, 0x00, 0x20, 0x41, 0x00,
    0x00, 0x60, 0xc1, 0x00, 0x00, 0x88, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x42, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x48, 0x43, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x80, 0x3f,
    0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xa0, 0x41, 0x00, 0x00, 0xc8, 0x42, 0x03, 0x04, 0x00, 0x00, 0x16, 0x43, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xbf, 0x02, 0x00, 0x00, 0x20, 0x41,
    0x01, 0x00, 0x00, 0x20, 0x42, 0x00, 0x00, 0x30, 0x42, 0x00, 0x00, 0x48, 0x42, 0x00, 0x00, 0x60,
    0x42, 0x05, 0x01, 0x00, 0x00, 0x80, 0x3f, 0x01, 0x00, 0x00, 0x70, 0x42, 0x00, 0x00, 0x80, 0x42,
    0x00, 0x00, 0x88, 0x42, 0x00, 0x00, 0x90, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
    0x00, 0x0b, 0x03, 0x00, 0x00, 0x98, 0x42, 0x01, 0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa0, 0x42, 0x01, 0x00, 0x00, 0xa8, 0x42, 0x01, 0x00, 0x00,
    0x28, 0x42, 0x2b, 0x2c, 0x07, 0x01, 0x01, 0x00, 0x00, 0xb0, 0x42, 0x01, 0x01, 0x00, 0x00, 0xb8,
    0x42, 0x00, 0x00, 0xc0, 0x42, 0x01, 0x12, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65,
    0x6e, 0x5f, 0x6d, 0x65, 0x73, 0x73, 0x61, 0x67, 0x65, 0x01, 0x01, 0x00, 0x00, 0xc8, 0x42, 0x01,
    0x00, 0x00, 0xd0, 0x42, 0x01, 0x00, 0x00, 0xd8, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00,
];

/// `SECTION_SIMULATION` (19) at its frozen shape: the exact bytes
/// [`frozen_simulation`]'s value encodes to.
const GOLDEN_TAG_19: &[u8] = &[
    0x02, 0x0f, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f, 0x7a, 0x65, 0x6e, 0x5f, 0x66, 0x69, 0x72,
    0x65, 0x01, 0xb9, 0x0a, 0x00, 0x00, 0xc0, 0x3f, 0x0e, 0x6f, 0x68, 0x6c, 0x5f, 0x66, 0x72, 0x6f,
    0x7a, 0x65, 0x6e, 0x5f, 0x6e, 0x6f, 0x77, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xa1, 0x1f, 0x01,
    0x00, 0x00, 0x20, 0x41, 0x01, 0x01, 0xd3, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// `SECTION_MOVER_STATE` (28) at its frozen shape: the exact bytes
/// [`frozen_mover_state`]'s value encodes to.
const GOLDEN_TAG_28: &[u8] = &[
    0x03, 0x00, 0x01, 0x01, 0x07, 0x00, 0x00, 0x00, 0x3f, 0x00, 0x00, 0x80, 0xbf, 0x00, 0x00, 0x48,
    0x42, 0x01, 0x00, 0x00, 0x40, 0x40, 0x01, 0x02, 0x00, 0x00, 0x00, 0x3e, 0x00, 0x00, 0x20, 0x42,
    0x00, 0x00, 0x80, 0x3f, 0x01, 0x00, 0x00, 0x60, 0x41, 0x01, 0x05, 0x00, 0x00, 0x00, 0x40, 0x0b,
    0x01, 0x00, 0x00, 0xa0, 0x40, 0x01, 0x11, 0x01, 0x00, 0x00, 0x00, 0xc0, 0x40, 0x00, 0x00, 0x80,
    0x3f, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x40, 0x40, 0x01, 0x1f, 0x03, 0x01, 0x00, 0x00, 0xe0,
    0x40, 0x02, 0x01, 0x01, 0x00, 0x00, 0xf7, 0x42, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00,
];

/// `SECTION_MOMENTARY_DOOR_STATE` (31, M9.8) at the shape this build writes:
/// the exact bytes [`frozen_momentary_door_state`]'s value encodes to.
/// **New golden, not a revision of any of the tags above**: tag 31 did not
/// exist before this package, so there is no earlier shape to protect —
/// this pins the shape this build introduces, the same way a future package
/// adding tag 33 would pin its own golden from scratch (`docs/
/// FORMAT_SOURCES.md` item 29).
const GOLDEN_TAG_31: &[u8] = &[
    0x03, 0x00, 0x01, 0x00, 0x00, 0x40, 0x3f, 0x01, 0x00, 0x00, 0x00, 0x00,
];

/// `SECTION_TELEPORT_STATE` (34) at the shape this build writes: the exact
/// bytes [`frozen_teleport_state`]'s value encodes to. **New golden, not a
/// revision of any tag above**: tag 34 did not exist before this package,
/// exactly the "a future package adding a tag would pin its own golden
/// from scratch" case [`GOLDEN_TAG_31`]'s own comment anticipated.
const GOLDEN_TAG_34: &[u8] = &[
    0x02, 0xd3, 0x2c, 0x01, 0xa1, 0x1f, 0x00, 0x02, 0xd3, 0x2c, 0x03, 0xa1, 0x1f, 0x81, 0x02,
];

/// The value [`GOLDEN_TAG_34`] holds: both halves non-empty, and both
/// arms of the touch-edge `bool` on the wire, plus a fire count past the
/// single-byte varint boundary.
fn frozen_teleport_state() -> TeleportStateSnapshot {
    TeleportStateSnapshot {
        teleport_touch: vec![(5715, true), (4001, false)],
        master_fires: vec![(5715, 3), (4001, 257)],
    }
}

/// The value [`GOLDEN_TAG_18`] holds: one entity carrying every component
/// this section persists, and one carrying none, so both arms of every
/// `Option` are on the wire.
fn frozen_entity_registry() -> Vec<EntitySnapshot> {
    vec![
        EntitySnapshot {
            spawnflags: Some(1234),
            render: Some(RenderPropsComponent {
                mode: 5,
                amt: 11,
                color: [22, 33, 255],
            }),
            transform: Some(Transform {
                origin: Vec3::new(10.0, -14.0, 68.0),
                angles: Vec3::new(0.0, 45.0, 0.0),
            }),
            door: Some(Door {
                speed: 200.0,
                wait: 2.0,
                lip: 1.0,
                movedir: Vec3::new(-1.0, 0.0, 0.0),
                dmg: 0.0,
                health: 20.0,
                delay: 100.0,
                sounds: (3, 4),
                travel_distance: 150.0,
                rotation_axis: Some(Vec3::new(0.0, 0.0, -1.0)),
                state: MoverState::Open,
                timer: 10.0,
            }),
            button: Some(Button {
                speed: 40.0,
                wait: 44.0,
                health: 50.0,
                delay: 56.0,
                sound: 5,
                state: MoverState::Opening,
                timer: 1.0,
            }),
            platform: Some(Platform {
                speed: 60.0,
                wait: 64.0,
                movedir: Vec3::new(68.0, 72.0, 0.0),
                travel_distance: 2.0,
                sounds: (0, 11),
                state: MoverState::Closing,
                timer: 76.0,
            }),
            rotator: Some(Rotator {
                axis: Vec3::new(-1.0, 0.0, 0.0),
                speed: 80.0,
                spinning: true,
                angle_deg: 84.0,
            }),
            light: Some(Light {
                brightness: 42.0,
                color: [43, 44, 7],
                style: 1,
                cone: Some(88.0),
            }),
            trigger: Some(Trigger {
                once: true,
                wait: 92.0,
                delay: 96.0,
            }),
            message: Some(Message {
                message: "ohl_frozen_message".to_string(),
                literal: true,
                fadein: Some(100.0),
                fadeout: Some(104.0),
                holdtime: Some(108.0),
            }),
            // See `frozen_mover_state`'s own note on `..Default::default()`.
            ..EntitySnapshot::default()
        },
        EntitySnapshot::default(),
    ]
}

/// The value [`GOLDEN_TAG_19`] holds: one delayed event, one immediate one,
/// and two trigger cooldowns covering both arms of
/// `TriggerSnapshot::changelevel_touching`.
fn frozen_simulation() -> SimulationState {
    SimulationState {
        pending: vec![
            PendingFire {
                target: "ohl_frozen_fire".to_string(),
                activator: Some(1337),
                delay: 1.5,
            },
            PendingFire {
                target: "ohl_frozen_now".to_string(),
                activator: None,
                delay: 0.0,
            },
        ],
        triggers: vec![
            TriggerSnapshot {
                entity: 4001,
                used: true,
                cooldown: 10.0,
                changelevel_touching: Some(true),
            },
            TriggerSnapshot {
                entity: 5715,
                used: false,
                cooldown: 0.0,
                changelevel_touching: None,
            },
        ],
        // See `frozen_mover_state`'s own note on `..Default::default()`.
        ..SimulationState::default()
    }
}

/// The value [`GOLDEN_TAG_28`] holds: an empty slot, a slot with every one
/// of the section's five members populated, and an all-`None` slot.
fn frozen_mover_state() -> Vec<Option<MoverSnapshot>> {
    vec![
        None,
        Some(MoverSnapshot {
            track_train: Some(TrackTrainSnapshot {
                node_index: 7,
                t: 0.5,
                direction: -1.0,
                speed: 50.0,
                moving: true,
                wait_timer: 3.0,
            }),
            camera: Some(TriggerCameraSnapshot {
                node_index: 2,
                t: 0.125,
                speed: 40.0,
                wait_timer: 1.0,
                active: true,
                hold_remaining: 14.0,
            }),
            script: Some(ScriptRunnerSnapshot {
                phase_tag: 5,
                timer: 2.0,
                completions: 11,
                warped: true,
                moving_elapsed: 5.0,
                actor: Some(17),
                pending_trigger: true,
                was_active: false,
                played: 6.0,
                play_origin: [1.0, -2.0, 3.0],
            }),
            maker: Some(MonsterMakerSnapshot {
                spawned_total: 31,
                live_children: 3,
                active: true,
                timer: 7.0,
                pending_activation: 2,
            }),
            rotator: Some(RotatorSnapshot {
                spinning: true,
                angle_deg: 123.5,
            }),
            auto_trigger_fired: Some(true),
            // `..Default::default()` on purpose, not laziness: a field added
            // to `MoverSnapshot` then still compiles here and fails at the
            // golden-byte comparison — a wire-shape report, not a build
            // break in this test's own scaffolding.
            ..MoverSnapshot::default()
        }),
        Some(MoverSnapshot::default()),
    ]
}

/// `SECTION_BREAKABLE_STATE` (33, M9.10) at the shape this build writes: the
/// exact bytes [`frozen_breakable_state`]'s value encodes to. **New golden,
/// not a revision of any tag above**: tag 33 did not exist before this
/// package, so there is no earlier shape to protect — it is pinned from the
/// moment it ships, exactly as item 29 said a future tag should be
/// (`docs/FORMAT_SOURCES.md` item 32).
const GOLDEN_TAG_33: &[u8] = &[
    0x04, 0x00, 0x01, 0x00, 0x00, 0x48, 0x41, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x20, 0x42, 0x00, 0x00, 0x00, 0x80, 0x42,
    0x00, 0x00, 0x00, 0xc2, 0x00, 0x00, 0x00, 0x00,
];

/// The value [`GOLDEN_TAG_31`] holds: an empty slot, a partway-open door,
/// and a fully-closed one — both arms of the `Option` and both ends of the
/// `0.0..=1.0` fraction range on the wire.
fn frozen_momentary_door_state() -> Vec<Option<MomentaryDoorSnapshot>> {
    vec![
        None,
        Some(MomentaryDoorSnapshot { fraction: 0.75 }),
        Some(MomentaryDoorSnapshot { fraction: 0.0 }),
    ]
}

/// `MoverSnapshot`'s field list as it stood *before* the `rotator` field was
/// added to it (`9ea7029`), transcribed from that revision. Serialize-only,
/// and deliberately **not** the shape this build reads: it stands in for an
/// older writer, to prove that bytes one field short of today's shape are
/// *rejected* rather than silently misassigned. Section 28's own history is
/// why that matters — `auto_trigger_fired` sits immediately after `rotator`,
/// so a one-field slip reads a fired `trigger_auto` back as unfired, which
/// is exactly the replay bug that field exists to prevent.
///
/// The four member types are named through the crate's own public
/// definitions rather than re-declared, on purpose: they are frozen too, so
/// if a field is ever added to one of *them*, this mirror follows it.
#[derive(Serialize)]
struct FrozenMoverSnapshot28 {
    track_train: Option<TrackTrainSnapshot>,
    camera: Option<TriggerCameraSnapshot>,
    script: Option<ScriptRunnerSnapshot>,
    maker: Option<MonsterMakerSnapshot>,
    auto_trigger_fired: Option<bool>,
}

impl From<&MoverSnapshot> for FrozenMoverSnapshot28 {
    fn from(snapshot: &MoverSnapshot) -> Self {
        Self {
            track_train: snapshot.track_train,
            camera: snapshot.camera,
            script: snapshot.script,
            maker: snapshot.maker,
            auto_trigger_fired: snapshot.auto_trigger_fired,
        }
    }
}

/// Renders a mismatch as something a maintainer can act on: the whole point
/// of a golden is that it is not silently regenerated.
fn assert_golden(actual: &[u8], golden: &[u8], tag: u32) {
    assert_eq!(
        actual,
        golden,
        "the wire shape of save section {tag} changed: {} bytes now, {} in the committed golden. \
         Every save file already written is unreadable by this build. Put the new state in a new \
         optional section instead (see ohl_engine::save's module doc); do not regenerate this \
         array to make the test pass.",
        actual.len(),
        golden.len(),
    );
}

#[test]
fn tag_18_entity_registry_keeps_its_frozen_wire_shape() {
    let value = frozen_entity_registry();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_18, 18);

    let decoded: Vec<EntitySnapshot> =
        postcard::from_bytes(GOLDEN_TAG_18).expect("an older save's section 18 still decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_19_simulation_keeps_its_frozen_wire_shape() {
    let value = frozen_simulation();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_19, 19);

    let decoded: SimulationState =
        postcard::from_bytes(GOLDEN_TAG_19).expect("an older save's section 19 still decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_28_mover_state_keeps_its_frozen_wire_shape() {
    let value = frozen_mover_state();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_28, 28);

    let decoded: Vec<Option<MoverSnapshot>> =
        postcard::from_bytes(GOLDEN_TAG_28).expect("an older save's section 28 still decodes");
    assert_eq!(decoded, value);
}

/// The value [`GOLDEN_TAG_33`] holds: an empty slot, an intact
/// `func_breakable` at partial health with no push offset, a broken one, and
/// a pushed `func_pushable` — both arms of the `Option`, both values of
/// `broken`, and a non-zero push offset, all on the wire.
fn frozen_breakable_state() -> Vec<Option<BreakableSnapshot>> {
    vec![
        None,
        Some(BreakableSnapshot {
            health: 12.5,
            broken: false,
            push_offset: [0.0; 3],
        }),
        Some(BreakableSnapshot {
            health: 0.0,
            broken: true,
            push_offset: [0.0; 3],
        }),
        Some(BreakableSnapshot {
            health: 40.0,
            broken: false,
            push_offset: [64.0, -32.0, 0.0],
        }),
    ]
}

#[test]
fn tag_33_breakable_state_keeps_its_frozen_wire_shape() {
    let value = frozen_breakable_state();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_33, 33);

    let decoded: Vec<Option<BreakableSnapshot>> =
        postcard::from_bytes(GOLDEN_TAG_33).expect("section 33 decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_31_momentary_door_state_keeps_its_frozen_wire_shape() {
    let value = frozen_momentary_door_state();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_31, 31);

    let decoded: Vec<Option<MomentaryDoorSnapshot>> =
        postcard::from_bytes(GOLDEN_TAG_31).expect("section 31 decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_34_teleport_state_keeps_its_frozen_wire_shape() {
    let value = frozen_teleport_state();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_34, 34);

    let decoded: TeleportStateSnapshot =
        postcard::from_bytes(GOLDEN_TAG_34).expect("section 34 decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_16_engine_header_keeps_its_frozen_wire_shape() {
    let value = frozen_engine_header();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_16, 16);

    let decoded: EngineHeader =
        postcard::from_bytes(GOLDEN_TAG_16).expect("an older save's section 16 still decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_17_player_carry_keeps_its_frozen_wire_shape() {
    let value = frozen_player_carry();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_17, 17);

    let decoded: PlayerCarryState =
        postcard::from_bytes(GOLDEN_TAG_17).expect("an older save's section 17 still decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_20_global_state_keeps_its_frozen_wire_shape() {
    let value = frozen_globals();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_20, 20);

    let decoded: GlobalStateTable =
        postcard::from_bytes(GOLDEN_TAG_20).expect("an older save's section 20 still decodes");
    assert_eq!(decoded, value);
}

/// Tag 21 is a bare `f32`, so its "shape" is four bytes and cannot drift
/// without the type itself changing. Pinned anyway, for the same reason the
/// others are: the section list is the unit of compatibility, not the
/// interesting-looking subset of it.
#[test]
fn tag_21_light_style_time_keeps_its_frozen_wire_shape() {
    let value: f32 = 12.5;
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_21, 21);

    let decoded: f32 =
        postcard::from_bytes(GOLDEN_TAG_21).expect("an older save's section 21 still decodes");
    assert_eq!(decoded, value);
}

#[test]
fn tag_22_view_keeps_its_frozen_wire_shape() {
    let value = frozen_view();
    let encoded = postcard::to_allocvec(&value).expect("the section encodes");
    assert_golden(&encoded, GOLDEN_TAG_22, 22);

    let decoded: ViewState =
        postcard::from_bytes(GOLDEN_TAG_22).expect("an older save's section 22 still decodes");
    assert_eq!(decoded, value);
}

/// A section written by a *newer* writer — one field longer than this build
/// reads — must be rejected, not decoded with the surplus bytes ignored.
///
/// This is the hole a review of this module found. `postcard::from_bytes`
/// discards an unused tail, and a hand-driven `postcard::Deserializer` (the
/// bounded-vec path tags 28 and 29 use) simply stops once its sequence is
/// complete, so before the fix every section-decode path in this project
/// returned `Ok` for over-long input. For tag 28 specifically that is not an
/// abstract risk: the field *after* `rotator` is `auto_trigger_fired`, so a
/// reader one field short reads a fired `trigger_auto` back as unfired and
/// replays it on load — silently re-toggling, and so stopping, whatever
/// train or camera the same section had just restored.
#[test]
fn a_section_written_by_a_longer_writer_is_rejected_not_misread() {
    /// [`MoverSnapshot`] plus one field, standing in for a future writer.
    #[derive(Serialize)]
    struct LongerMoverSnapshot {
        track_train: Option<TrackTrainSnapshot>,
        camera: Option<TriggerCameraSnapshot>,
        script: Option<ScriptRunnerSnapshot>,
        maker: Option<MonsterMakerSnapshot>,
        rotator: Option<RotatorSnapshot>,
        auto_trigger_fired: Option<bool>,
        added_by_a_later_build: Option<u32>,
    }

    // One slot, so the surplus field lands at the very end of the section
    // and the bytes are otherwise *exactly* what this build's own reader
    // expects: every field decodes and aligns, and the only evidence of the
    // mismatch is the unconsumed tail. That is the case that used to pass
    // silently; a multi-slot section would also trip over the second
    // element's misaligned start, which proves less.
    let snapshot = frozen_mover_state()
        .into_iter()
        .flatten()
        .next()
        .expect("the fixture has a populated slot");
    let longer = vec![Some(LongerMoverSnapshot {
        track_train: snapshot.track_train,
        camera: snapshot.camera,
        script: snapshot.script,
        maker: snapshot.maker,
        rotator: snapshot.rotator,
        auto_trigger_fired: snapshot.auto_trigger_fired,
        added_by_a_later_build: Some(7),
    })];
    let bytes = postcard::to_allocvec(&longer).expect("the longer shape encodes");
    let ours = postcard::to_allocvec(&vec![Some(snapshot)]).expect("today's shape encodes");
    assert!(
        bytes.len() > ours.len() && bytes.starts_with(&ours),
        "the stand-in writer must produce today's bytes plus a surplus tail"
    );

    let save = save_with_section(ohl_engine::save::SECTION_MOVER_STATE, &bytes);
    assert!(
        matches!(
            ohl_engine::GameSave::from_bytes(&save),
            Err(EngineError::SaveUnreadable)
        ),
        "a section with unconsumed trailing bytes must fail closed"
    );
}

/// The same guarantee for the other two decode paths, so all three agree:
/// a required section ([`ohl_save::SaveReader::deserialize`], tag 16), an
/// optional one (`optional_section`, tag 30) and a bounded-vector one
/// (`optional_bounded_vec_section`, tag 28) each reject a section with even
/// one surplus byte.
#[test]
fn a_bounded_vec_section_with_trailing_bytes_fails_closed() {
    for tag in [
        ohl_engine::save::SECTION_ENGINE_HEADER,
        ohl_engine::save::SECTION_ROTATING_MOVER_STATE,
        ohl_engine::save::SECTION_MOVER_STATE,
    ] {
        let original = base_save();
        let reader = ohl_save::SaveReader::open(&original, &ohl_save::Limits::default())
            .expect("the base save opens");
        let mut payload = reader
            .section(tag)
            .expect("the base save carries this section")
            .to_vec();
        payload.push(0);
        drop(reader);

        let save = save_with_section(tag, &payload);
        assert!(
            matches!(
                ohl_engine::GameSave::from_bytes(&save),
                Err(EngineError::SaveUnreadable)
            ),
            "section {tag} decoded despite a surplus byte"
        );
    }
}

/// A save file written at tag 28's *pre-`rotator`* shape (`9ea7029`) is
/// rejected rather than misread. It is unreadable at `main` anyway — tag 18
/// moved twice after that revision, which is what actually sets this
/// project's compatibility floor — but a reader must never be the thing that
/// silently paves over the difference.
#[test]
fn a_save_whose_tag_28_predates_the_rotator_field_fails_closed() {
    let entities = format!(
        "{}{{\n\"classname\" \"trigger_auto\"\n\"target\" \"{}\"\n\"delay\" \"0\"\n}}\n",
        rotating_door_entities(),
        ROTATING_DOOR_NAME,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ROTATING_DOOR_MAP}.bsp"),
        rotating_door_bsp(&entities),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");
    // Long enough for the `trigger_auto` to fire, so its `fired` flag makes
    // the section non-empty rather than an all-`None` vector.
    for _ in 0..10 {
        game.tick(ohl_engine::TICK_SECONDS, &Input::default());
    }

    let save = game.to_save(1_700_000_000);
    let mover_state = save
        .mover_state
        .clone()
        .expect("this build always writes tag 28");
    assert!(
        mover_state
            .iter()
            .flatten()
            .any(|snapshot| snapshot.auto_trigger_fired == Some(true)),
        "the fixture must record a fired trigger_auto, or this test proves nothing"
    );
    let historic: Vec<Option<FrozenMoverSnapshot28>> = mover_state
        .iter()
        .map(|slot| slot.as_ref().map(FrozenMoverSnapshot28::from))
        .collect();
    let historic_bytes = postcard::to_allocvec(&historic).expect("the historic shape encodes");

    let bytes = save.to_bytes().expect("the save encodes");
    let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default())
        .expect("the save just written opens");
    let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
    for entry in reader.sections() {
        let payload = if entry.tag == ohl_engine::save::SECTION_MOVER_STATE {
            historic_bytes.as_slice()
        } else {
            reader.section(entry.tag).expect("a listed section reads")
        };
        writer
            .add_section(entry.tag, payload)
            .expect("the section is re-added");
    }
    let rewritten = writer
        .finish(&ohl_save::Limits::default())
        .expect("the rewritten container closes");

    assert!(
        matches!(
            Game::load_bytes(&assets, &rewritten),
            Err(EngineError::SaveUnreadable)
        ),
        "a save at tag 28's pre-rotator shape must fail closed, never load \
         with its fields shifted by one"
    );
}

/// A minimal, deterministic save file this build writes, for the
/// fail-closed tests to mutate one section of.
fn base_save() -> Vec<u8> {
    let entities = format!(
        "{}{{\n\"classname\" \"trigger_auto\"\n\"target\" \"{}\"\n\"delay\" \"0\"\n}}\n",
        rotating_door_entities(),
        ROTATING_DOOR_NAME,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ROTATING_DOOR_MAP}.bsp"),
        rotating_door_bsp(&entities),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");
    for _ in 0..10 {
        game.tick(ohl_engine::TICK_SECONDS, &Input::default());
    }
    game.save_bytes(1_700_000_000).expect("the save is written")
}

/// [`base_save`] with `tag`'s payload replaced by `payload`, every other
/// section left exactly as written.
fn save_with_section(tag: u32, payload: &[u8]) -> Vec<u8> {
    let bytes = base_save();
    let reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default())
        .expect("the base save opens");
    let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
    for entry in reader.sections() {
        let section = if entry.tag == tag {
            payload
        } else {
            reader.section(entry.tag).expect("a listed section reads")
        };
        writer
            .add_section(entry.tag, section)
            .expect("the section is re-added");
    }
    writer
        .finish(&ohl_save::Limits::default())
        .expect("the rewritten container closes")
}
