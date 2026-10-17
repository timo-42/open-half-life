//! Project-authored authority/presentation fixtures only; no captured packets,
//! original assets or interoperability assertions. First-person animation
//! fixtures and action labels are authored for this project.
#![allow(clippy::float_cmp)]

use glam::{Mat4, Quat, Vec3};
use ohl_combat::EntityId;
use ohl_engine::deathmatch::{
    Deathmatch, DeathmatchConfig, PlayerButtons, PlayerCommand, PlayerId, SessionId,
};
use ohl_engine::recipient::{
    self, RecipientError, RecipientFrame, RecipientReplica, RecipientViewModel,
};
use ohl_engine::scene::{
    HeldModelRecord, SceneEpoch, SceneError, SceneFrame, SceneIdentity, SceneLimits, SceneReplica,
    StudioRecord,
};
use ohl_engine::test_support::{AI_MAP, deathmatch_room_bsp};
use ohl_engine::{MemoryAssets, RenderTarget, SkirmishConfig, TICK_SECONDS};
use ohl_formats::test_support::{
    MinimalMdl10Layout, build_minimal_mdl10, build_minimal_mdl10_with_sequences,
};
use ohl_game::hecs::World;
use ohl_physics::PlayerController;
use ohl_render::{FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};
use ohl_world::{StudioLimits, StudioModel, StudioPose};

const EPOCH: SceneEpoch = SceneEpoch::new(91);
const SPAWNS: [(f32, f32); 4] = [
    (-192.0, -192.0),
    (192.0, -192.0),
    (-192.0, 192.0),
    (192.0, 192.0),
];

fn authority() -> Deathmatch {
    authority_with_bots(0)
}

fn authority_with_bots(bots: u8) -> Deathmatch {
    authority_with_models(bots, false)
}

fn authority_with_models(bots: u8, first_person: bool) -> Deathmatch {
    let mut assets = MemoryAssets::new();
    let (mut model, layout) = build_minimal_mdl10();
    for vertex in 0..4 {
        for axis in 0..2 {
            let offset = layout.verts_offset + vertex * 12 + axis * 4;
            let value = f32::from_le_bytes(model[offset..offset + 4].try_into().unwrap());
            model[offset..offset + 4].copy_from_slice(&(value * 24.0).to_le_bytes());
        }
    }
    if first_person {
        opaque_quad_model(&mut model, &layout, [128, 128, 128]);
    }
    assets.insert(ohl_engine::skirmish::PLAYER_MODEL_PATH, model);
    if first_person {
        // Existing view_model_path literals, with public per-literal provenance
        // in FORMAT_SOURCES.md, "First-person view models". Bytes are synthetic.
        for path in ["models/v_9mmhandgun.mdl", "models/v_crowbar.mdl"] {
            assets.insert(path, authored_first_person_model());
        }
    }
    let bytes = deathmatch_room_bsp(&SPAWNS, false, "");
    Deathmatch::from_map_bytes(
        &assets,
        AI_MAP,
        &bytes,
        &DeathmatchConfig {
            session_id: SessionId(37),
            max_players: 4,
            rules: SkirmishConfig {
                bots,
                frag_limit: 0,
                time_limit_seconds: 120.0,
                ..SkirmishConfig::default()
            },
        },
    )
    .unwrap()
}

/// A full rectangle strip and solid fullbright palette, all authored here.
fn opaque_quad_model(bytes: &mut [u8], layout: &MinimalMdl10Layout, colour: [u8; 3]) {
    bytes[layout.textures_offset + 64..layout.textures_offset + 68]
        .copy_from_slice(&4i32.to_le_bytes());
    let palette = layout.texture_data_offset + 16 * 16;
    for entry in bytes[palette..palette + 256 * 3].as_chunks_mut::<3>().0 {
        entry.copy_from_slice(&colour);
    }
    // The generic fixture's four vertices are in cyclic order. A complete
    // rectangle triangle strip visits the opposing corners in this order.
    for (command, vertex) in [0i16, 1, 3, 2].into_iter().enumerate() {
        let offset = layout.tricommands_offset + 2 + command * 8;
        bytes[offset..offset + 2].copy_from_slice(&vertex.to_le_bytes());
    }
}

/// Five project-authored action labels and distinguishable animated poses.
fn authored_first_person_model() -> Vec<u8> {
    let (mut bytes, layout) = build_minimal_mdl10_with_sequences(&[
        "ohl_idle",
        "ohl_deploy",
        "ohl_shoot",
        "ohl_reload",
        "ohl_holster",
    ]);
    for vertex in 0..4 {
        let offset = layout.verts_offset + vertex * 12;
        let x = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let y = f32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        bytes[offset..offset + 4].copy_from_slice(&(40.0 + x * 8.0).to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&(y * 6.0).to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&(-4.0_f32).to_le_bytes());
    }
    // An opaque red fullbright model gives the depth test a distinct oracle.
    opaque_quad_model(&mut bytes, &layout, [255, 0, 0]);
    let animation = bytes[layout.anim_data_offset..layout.anim_data_offset + 30].to_vec();
    for index in 0..5 {
        let start = bytes.len();
        bytes.extend_from_slice(&animation);
        let base = i16::try_from(index * 2).unwrap();
        bytes[start + 26..start + 28].copy_from_slice(&base.to_le_bytes());
        bytes[start + 28..start + 30].copy_from_slice(&(base + 1).to_le_bytes());
        let sequence = layout.sequences_offset + index * 176;
        bytes[sequence + 124..sequence + 128]
            .copy_from_slice(&u32::try_from(start).unwrap().to_le_bytes());
    }
    let length = u32::try_from(bytes.len()).unwrap();
    bytes[72..76].copy_from_slice(&length.to_le_bytes());
    bytes
}

fn expected_first_person_pose(sequence: usize, cycle: f32) -> StudioPose {
    let model =
        StudioModel::parse(&authored_first_person_model(), &StudioLimits::default()).unwrap();
    StudioPose::sample(&model, sequence, cycle).unwrap()
}

fn first_person_humans() -> (Deathmatch, PlayerId, PlayerId) {
    let mut host = authority_with_models(0, true);
    let a = host.join("First human").unwrap();
    let b = host.join("Second human").unwrap();
    (host, a, b)
}

fn first_person(host: &Deathmatch, id: PlayerId) -> RecipientViewModel {
    host.capture_recipient(EPOCH, id)
        .unwrap()
        .state()
        .view_model
        .clone()
        .expect("this living human's optional synthetic model is loaded")
}

fn humans() -> (Deathmatch, PlayerId, PlayerId) {
    let mut host = authority();
    let a = host.join("Same label").unwrap();
    let b = host.join("Same label").unwrap();
    (host, a, b)
}

fn replica(frame: &RecipientFrame) -> RecipientReplica {
    let mut view = RecipientReplica::new(
        frame.scene().catalog().clone(),
        EPOCH,
        frame.state().recipient,
        SceneLimits::default(),
    )
    .unwrap();
    view.apply(frame.clone()).unwrap();
    view
}

fn restamp(frame: &RecipientFrame, tick: u64, time: f32) -> RecipientFrame {
    RecipientFrame::new(
        SceneFrame::new(
            frame.scene().catalog().clone(),
            frame.scene().epoch(),
            tick,
            time,
            frame.scene().brushes().to_vec(),
            frame.scene().studios().to_vec(),
        ),
        frame.state().clone(),
    )
}

fn reject(view: &mut RecipientReplica, candidate: RecipientFrame, error: RecipientError) {
    let previous = view.frame().map(|f| (f.scene().clone(), f.state().clone()));
    assert_eq!(view.apply(candidate), Err(error));
    assert_eq!(
        view.frame().map(|f| (f.scene().clone(), f.state().clone())),
        previous
    );
}

#[test]
fn selected_humans_use_actual_eyes_views_and_private_numeric_hud() {
    let (mut host, a, b) = humans();
    host.submit(
        a,
        PlayerCommand {
            view_angles: [25.0, 135.0, 0.0],
            buttons: PlayerButtons {
                duck: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    host.submit(
        b,
        PlayerCommand {
            view_angles: [-12.0, 310.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    host.tick(TICK_SECONDS);
    let snapshot = host.snapshot();
    for id in [a, b] {
        let actor = snapshot.players.iter().find(|p| p.id == id).unwrap();
        let frame = host.capture_recipient(EPOCH, id).unwrap();
        let mut controller = PlayerController::spawn_at(
            Vec3::from_array(actor.origin),
            actor.view_angles[1],
            actor.view_angles[0],
        );
        controller.state.ducked = actor.ducked;
        assert_eq!(
            frame.state().camera.eye,
            controller.eye_position().to_array()
        );
        assert_eq!(frame.state().camera.pitch, actor.view_angles[0]);
        assert_eq!(frame.state().camera.yaw, actor.view_angles[1]);
        assert_eq!(frame.state().hud.health, 100);
        assert_eq!(frame.state().hud.armor, 0);
        assert_eq!(frame.state().hud.weapon, actor.weapon);
        assert_eq!(
            frame.state().hud.clip_ammo,
            Some(i32::try_from(actor.clip).unwrap())
        );
        assert_eq!(frame.scene().tick(), snapshot.tick);
        assert_eq!(frame.scene().time(), snapshot.elapsed);
        let view = replica(&frame);
        let board = recipient::scoreboard(view.frame().unwrap());
        assert_eq!(board.rows.iter().filter(|r| r.is_local).count(), 1);
        assert_eq!(
            board.rows.iter().position(|r| r.is_local),
            Some(usize::from(id.slot))
        );
        let hud = recipient::hud(view.frame().unwrap(), true);
        assert_eq!(hud.health, 100);
        assert!(hud.message.is_none() && hud.center_notice.is_none() && hud.kill_feed.is_empty());
        assert_eq!(hud.damage_flash, 0.0);
    }
    assert!(snapshot.players[0].ducked);
    assert_eq!(host.game().player_health(), 0.0);
    assert_ne!(
        host.capture_recipient(EPOCH, a).unwrap().state().camera.eye,
        host.game().eye_position()
    );
}

#[test]
fn opponent_ammo_changes_are_not_in_selected_hud_or_public_roster() {
    let (mut host, a, b) = humans();
    for _ in 0..100 {
        host.tick(TICK_SECONDS);
    }
    let before = host.capture_recipient(EPOCH, a).unwrap();
    let prior = host.snapshot().players[1].clip;
    host.submit(
        b,
        PlayerCommand {
            sequence: 1,
            view_angles: [-70.0, 0.0, 0.0],
            buttons: PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    host.tick(TICK_SECONDS);
    let after = host.capture_recipient(EPOCH, a).unwrap();
    assert_eq!(host.snapshot().players[1].clip, prior - 1);
    assert_eq!(after.state().hud, before.state().hud);
    assert_eq!(
        after.state().scoreboard.rows,
        before.state().scoreboard.rows
    );
    assert_eq!(before.state().hud.clip_ammo, after.state().hud.clip_ammo);
}

#[test]
fn public_bot_identity_is_not_a_recipient_and_clipless_selection_hides_ammo() {
    let mut host = authority_with_bots(1);
    let bot = host.snapshot().players[0].id;
    let human = host.join("Recipient").unwrap();
    assert_eq!(
        host.capture_recipient(EPOCH, bot),
        Err(RecipientError::WrongRecipient)
    );
    host.submit(
        human,
        PlayerCommand {
            weapon: Some(ohl_combat::WeaponId::Crowbar),
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    host.tick(TICK_SECONDS);
    let frame = host.capture_recipient(EPOCH, human).unwrap();
    assert_eq!(
        frame.state().hud.weapon,
        Some(ohl_combat::WeaponId::Crowbar)
    );
    assert!(frame.state().hud.clip_ammo.is_none() && frame.state().hud.reserve_ammo.is_none());
    let view = replica(&frame);
    let rows = recipient::scoreboard(view.frame().unwrap()).rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.iter().position(|row| row.is_local), Some(1));
}

#[test]
fn utf8_label_boundary_and_roster_count_are_bounded_before_replacement() {
    let (host, a, _) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut state = first.state().clone();
    state.scoreboard.rows[0].name = "é".repeat(16);
    state.scoreboard.winner_label = Some("w".repeat(32));
    let boundary = RecipientFrame::new(first.scene().clone(), state.clone());
    let mut view = replica(&boundary);
    assert_eq!(
        recipient::scoreboard(view.frame().unwrap()).rows[0]
            .name
            .len(),
        32
    );
    state.scoreboard.rows = vec![state.scoreboard.rows[0].clone(); 33];
    reject(
        &mut view,
        RecipientFrame::new(restamp(&first, 1, 0.0).scene().clone(), state),
        RecipientError::InvalidRoster,
    );
    let mut state = first.state().clone();
    state.scoreboard.rows.swap(0, 1);
    reject(
        &mut view,
        RecipientFrame::new(restamp(&first, 1, 0.0).scene().clone(), state),
        RecipientError::InvalidRoster,
    );
}

#[test]
fn capture_apply_and_ui_do_not_advance_or_consume_authority_commands() {
    let (mut host, a, b) = humans();
    let (mut control, ca, cb) = humans();
    for (authority, first, second) in [(&mut host, a, b), (&mut control, ca, cb)] {
        authority
            .submit(
                first,
                PlayerCommand {
                    sequence: 1,
                    duration_millis: 5,
                    buttons: PlayerButtons {
                        flashlight: true,
                        ..PlayerButtons::default()
                    },
                    ..PlayerCommand::default()
                },
            )
            .unwrap();
        authority
            .submit(
                second,
                PlayerCommand {
                    sequence: 1,
                    movement: [0.5, 0.0, 0.0],
                    ..PlayerCommand::default()
                },
            )
            .unwrap();
    }
    let before = host.snapshot();
    // Persistent skirmish saves are deliberately unsupported. This public
    // read-only state container is used only as an in-memory fingerprint.
    let save = host.game().to_save(31).to_bytes().unwrap();
    let ai = host.game().ai_state_hash();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let owned = first.clone();
    let mut view = replica(&first);
    for tick in 1..5 {
        view.apply(restamp(&first, tick, 0.0)).unwrap();
        let _ = recipient::hud(view.frame().unwrap(), true);
        let _ = recipient::scoreboard(view.frame().unwrap());
        reject(
            &mut view,
            restamp(&first, tick, 0.0),
            SceneError::StaleTick.into(),
        );
    }
    assert_eq!(host.snapshot(), before);
    assert_eq!(host.game().to_save(31).to_bytes().unwrap(), save);
    assert_eq!(host.game().ai_state_hash(), ai);
    host.tick(TICK_SECONDS);
    control.tick(TICK_SECONDS);
    assert_eq!(host.snapshot(), control.snapshot());
    assert_eq!(
        host.game().to_save(31).to_bytes().unwrap(),
        control.game().to_save(31).to_bytes().unwrap()
    );
    assert_eq!(first, owned);
}

#[test]
fn bindings_reject_unadmitted_sessions_and_expired_generations() {
    let (mut host, a, _) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    for bad in [
        PlayerId {
            session: SessionId(0),
            ..a
        },
        PlayerId { slot: 32, ..a },
        PlayerId { generation: 0, ..a },
    ] {
        assert_eq!(
            RecipientReplica::new(
                first.scene().catalog().clone(),
                EPOCH,
                bad,
                SceneLimits::default()
            )
            .unwrap_err(),
            RecipientError::InvalidIdentity
        );
    }
    let mut view = replica(&first);
    for bad in [
        PlayerId {
            session: SessionId(9),
            ..a
        },
        PlayerId {
            generation: a.generation + 1,
            ..a
        },
    ] {
        let mut state = first.state().clone();
        state.recipient = bad;
        reject(
            &mut view,
            RecipientFrame::new(restamp(&first, 1, 0.0).scene().clone(), state),
            RecipientError::WrongRecipient,
        );
        assert_eq!(
            host.capture_recipient(EPOCH, bad).unwrap_err(),
            RecipientError::WrongRecipient
        );
    }
    host.leave(a).unwrap();
    let replacement = host.join("Replacement").unwrap();
    assert_eq!(a.slot, replacement.slot);
    assert_ne!(a.generation, replacement.generation);
    assert_eq!(
        host.capture_recipient(EPOCH, a).unwrap_err(),
        RecipientError::WrongRecipient
    );
    let fresh = host.capture_recipient(EPOCH, replacement).unwrap();
    reject(&mut view, fresh.clone(), RecipientError::WrongRecipient);
    let mut reconnected = RecipientReplica::new(
        fresh.scene().catalog().clone(),
        EPOCH,
        replacement,
        SceneLimits::default(),
    )
    .unwrap();
    assert!(reconnected.frame().is_none());
    reconnected.apply(fresh).unwrap();
    let board = recipient::scoreboard(reconnected.frame().unwrap());
    assert!(board.rows[0].is_local);
    assert_eq!(board.rows[0].name, "Replacement");
}

#[test]
fn mixed_roster_ids_labels_clocks_and_hud_fail_as_whole_frames() {
    let (host, a, _) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut view = replica(&first);
    let mut cases = Vec::new();
    let mut state = first.state().clone();
    state.scoreboard.rows[1].id.session = SessionId(99);
    cases.push((state, RecipientError::InvalidIdentity));
    let mut state = first.state().clone();
    state.scoreboard.rows[1].id.generation = 0;
    cases.push((state, RecipientError::InvalidIdentity));
    let mut state = first.state().clone();
    state.scoreboard.rows[1].id = PlayerId { generation: 2, ..a };
    cases.push((state, RecipientError::InvalidRoster));
    let mut state = first.state().clone();
    state.scoreboard.rows.remove(0);
    cases.push((state, RecipientError::InvalidRoster));
    let mut state = first.state().clone();
    state.scoreboard.rows[0].name = "é".repeat(17);
    cases.push((state, RecipientError::InvalidLabel));
    let mut state = first.state().clone();
    state.scoreboard.winner_label = Some("x\n".into());
    cases.push((state, RecipientError::InvalidLabel));
    let mut state = first.state().clone();
    state.scoreboard.seconds_left = Some(f32::NAN);
    cases.push((state, RecipientError::InvalidRoster));
    let mut state = first.state().clone();
    state.hud.clip_ammo = Some(-1);
    cases.push((state, RecipientError::InvalidHud));
    let mut state = first.state().clone();
    state.hud.weapon = None;
    cases.push((state, RecipientError::InvalidHud));
    let mut state = first.state().clone();
    state.camera.eye[0] = f32::INFINITY;
    cases.push((state, RecipientError::InvalidCamera));
    let mut state = first.state().clone();
    state.camera.pitch = 90.0;
    cases.push((state, RecipientError::InvalidCamera));
    let mut state = first.state().clone();
    state.camera.eye = [f32::MAX; 3];
    state.camera.yaw = 45.0;
    cases.push((state, RecipientError::InvalidCamera));
    let mut state = first.state().clone();
    state.body = SceneIdentity::from_local_id(EPOCH, EntityId(0));
    cases.push((state, RecipientError::InvalidIdentity));
    let mut state = first.state().clone();
    state.body = SceneIdentity::from_local_id(SceneEpoch::new(3), state.body.local_id());
    cases.push((state, RecipientError::InvalidIdentity));
    for (mut state, error) in cases {
        state.hud.health = 7;
        reject(
            &mut view,
            RecipientFrame::new(restamp(&first, 1, 0.0).scene().clone(), state),
            error,
        );
    }
}

#[test]
fn scene_binding_stamp_pose_and_budget_failures_preserve_private_state() {
    let (host, a, _) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut view = replica(&first);
    let (other, oa, _) = humans();
    let foreign = other.capture_recipient(EPOCH, oa).unwrap();
    reject(&mut view, foreign, SceneError::WrongCatalog.into());
    let wrong_epoch = SceneFrame::new(
        first.scene().catalog().clone(),
        SceneEpoch::new(8),
        1,
        0.0,
        Vec::new(),
        Vec::new(),
    );
    let mut state = first.state().clone();
    state.hud.health = 7;
    reject(
        &mut view,
        RecipientFrame::new(wrong_epoch, state.clone()),
        SceneError::WrongEpoch.into(),
    );
    reject(
        &mut view,
        RecipientFrame::new(first.scene().clone(), state.clone()),
        SceneError::StaleTick.into(),
    );
    view.apply(restamp(&first, 1, 1.0)).unwrap();
    reject(
        &mut view,
        restamp(&first, 2, 0.5),
        SceneError::StaleTime.into(),
    );
    let mut studios = first.scene().studios().to_vec();
    assert!(!studios.is_empty());
    studios[0].pose.matrices[0][0] = f32::NAN;
    let invalid = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        2,
        1.0,
        Vec::new(),
        studios,
    );
    reject(
        &mut view,
        RecipientFrame::new(invalid, state),
        SceneError::NonFiniteState.into(),
    );
    let mut bounded = RecipientReplica::new(
        first.scene().catalog().clone(),
        EPOCH,
        a,
        SceneLimits {
            max_instances: 0,
            max_bones: 0,
        },
    )
    .unwrap();
    assert_eq!(bounded.apply(first), Err(SceneError::InstanceLimit.into()));
    assert!(bounded.frame().is_none());
}

#[test]
fn full_replacement_removes_absent_rows_geometry_and_numeric_ammo() {
    let (mut host, a, b) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut view = replica(&first);
    host.leave(b).unwrap();
    host.tick(TICK_SECONDS);
    let next = host.capture_recipient(EPOCH, a).unwrap();
    assert_eq!(next.state().scoreboard.rows.len(), 1);
    view.apply(next).unwrap();
    assert_eq!(view.frame().unwrap().state().scoreboard.rows.len(), 1);
    let mut state = first.state().clone();
    state.hud.weapon = None;
    state.hud.clip_ammo = None;
    state.hud.reserve_ammo = None;
    state.scoreboard.rows[0].alive = false;
    let empty = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        100,
        1.0,
        Vec::new(),
        Vec::new(),
    );
    view.apply(RecipientFrame::new(empty, state)).unwrap();
    let accepted = view.frame().unwrap();
    assert!(accepted.scene().studios().is_empty());
    let hud = recipient::hud(accepted, true);
    assert_eq!(hud.clip_ammo, None);
    assert_eq!(hud.reserve_ammo, None);
    assert!(!hud.show_crosshair);
    assert_eq!(first.state().scoreboard.rows.len(), 2);
}

#[test]
fn automatic_debug_and_errors_do_not_format_selected_private_data() {
    let (host, a, _) = humans();
    let frame = host.capture_recipient(EPOCH, a).unwrap();
    let view = replica(&frame);
    let text = format!(
        "{frame:?} {:?} {:?} {:?} {:?}",
        frame.state(),
        frame.state().hud,
        view,
        view.frame().unwrap()
    );
    for forbidden in ["Same label", "clip_ammo", "health", "camera", "generation"] {
        assert!(!text.contains(forbidden));
    }
    assert_eq!(
        format!("{}", RecipientError::InvalidRoster),
        "invalid recipient roster"
    );
}

fn selected_command(
    host: &mut Deathmatch,
    id: PlayerId,
    sequence: u32,
    buttons: PlayerButtons,
    weapon: Option<ohl_combat::WeaponId>,
) {
    host.submit(
        id,
        PlayerCommand {
            sequence,
            view_angles: [-70.0, 0.0, 0.0],
            buttons,
            weapon,
            ..PlayerCommand::default()
        },
    )
    .unwrap();
}

fn sound_count(events: &[ohl_engine::GameEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ohl_engine::GameEvent::Sound(_)))
        .count()
}

#[test]
fn two_humans_play_actual_fire_and_reload_independently() {
    let (mut host, a, b) = first_person_humans();
    for id in [a, b] {
        assert_eq!(
            first_person(&host, id).pose,
            expected_first_person_pose(1, 0.0)
        );
    }
    for _ in 0..100 {
        host.tick(TICK_SECONDS);
    }
    let before = host.snapshot();
    for id in [a, b] {
        selected_command(
            &mut host,
            id,
            1,
            PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            None,
        );
    }
    let events = host.tick(TICK_SECONDS);
    assert_eq!(sound_count(&events), 2);
    for (index, id) in [a, b].into_iter().enumerate() {
        assert_eq!(
            host.snapshot().players[index].clip,
            before.players[index].clip - 1
        );
        assert_eq!(
            first_person(&host, id).pose,
            expected_first_person_pose(2, TICK_SECONDS)
        );
    }
    let after_shots = host.snapshot();
    host.tick(TICK_SECONDS);
    for (index, id) in [a, b].into_iter().enumerate() {
        assert_eq!(
            host.snapshot().players[index].clip,
            after_shots.players[index].clip
        );
        assert_eq!(
            first_person(&host, id).pose,
            expected_first_person_pose(2, 2.0 * TICK_SECONDS),
            "held attack during cooldown must not restart a shot"
        );
        selected_command(&mut host, id, 2, PlayerButtons::default(), None);
    }
    for _ in 0..100 {
        host.tick(TICK_SECONDS);
    }
    selected_command(
        &mut host,
        a,
        3,
        PlayerButtons {
            reload: true,
            ..PlayerButtons::default()
        },
        None,
    );
    selected_command(
        &mut host,
        b,
        3,
        PlayerButtons {
            attack: true,
            ..PlayerButtons::default()
        },
        None,
    );
    let before = host.snapshot();
    let events = host.tick(TICK_SECONDS);
    assert_eq!(sound_count(&events), 2);
    assert_eq!(host.snapshot().players[0].clip, before.players[0].clip);
    assert_eq!(host.snapshot().players[1].clip, before.players[1].clip - 1);
    assert_eq!(
        first_person(&host, a).pose,
        expected_first_person_pose(3, TICK_SECONDS)
    );
    assert_eq!(
        first_person(&host, b).pose,
        expected_first_person_pose(2, TICK_SECONDS)
    );

    host.tick(TICK_SECONDS);
    assert_eq!(
        first_person(&host, a).pose,
        expected_first_person_pose(3, 2.0 * TICK_SECONDS),
        "a held reload must not restart its action"
    );
}

#[test]
fn only_an_owned_weapon_selection_starts_a_new_draw() {
    let (mut host, _, b) = first_person_humans();
    for _ in 0..100 {
        host.tick(TICK_SECONDS);
    }
    selected_command(
        &mut host,
        b,
        1,
        PlayerButtons {
            attack: true,
            ..PlayerButtons::default()
        },
        None,
    );
    host.tick(TICK_SECONDS);
    let pistol = first_person(&host, b).model;
    selected_command(
        &mut host,
        b,
        2,
        PlayerButtons::default(),
        Some(ohl_combat::WeaponId::Python),
    );
    host.tick(TICK_SECONDS);
    assert_eq!(
        host.snapshot().players[1].weapon,
        Some(ohl_combat::WeaponId::Glock)
    );
    assert_eq!(first_person(&host, b).model, pistol);
    assert_eq!(
        first_person(&host, b).pose,
        expected_first_person_pose(2, 2.0 * TICK_SECONDS),
        "an unowned selection must not create a draw action"
    );
    selected_command(
        &mut host,
        b,
        3,
        PlayerButtons::default(),
        Some(ohl_combat::WeaponId::Crowbar),
    );
    host.tick(TICK_SECONDS);
    assert_eq!(
        host.snapshot().players[1].weapon,
        Some(ohl_combat::WeaponId::Crowbar)
    );
    assert_ne!(first_person(&host, b).model, pistol);
    assert_eq!(
        first_person(&host, b).pose,
        expected_first_person_pose(1, TICK_SECONDS)
    );
    assert_eq!(
        host.snapshot().players[0].weapon,
        Some(ohl_combat::WeaponId::Glock)
    );
}

#[test]
fn denied_full_clip_reload_does_not_create_or_restart_an_action() {
    let (mut host, a, b) = first_person_humans();
    for _ in 0..100 {
        host.tick(TICK_SECONDS);
    }
    let before = host.snapshot();
    selected_command(
        &mut host,
        a,
        1,
        PlayerButtons {
            reload: true,
            ..PlayerButtons::default()
        },
        None,
    );
    selected_command(&mut host, b, 1, PlayerButtons::default(), None);
    for _ in 0..4 {
        let events = host.tick(TICK_SECONDS);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ohl_engine::GameEvent::Sound(_)))
        );
        assert_eq!(first_person(&host, a).pose, first_person(&host, b).pose);
        for index in 0..2 {
            assert_eq!(
                host.snapshot().players[index].clip,
                before.players[index].clip
            );
            assert_eq!(
                host.snapshot().players[index].reserve,
                before.players[index].reserve
            );
        }
    }
}

#[test]
fn viewmodel_capture_and_ui_never_advance_playback_or_consume_commands() {
    let (mut host, a, b) = first_person_humans();
    let (mut control, ca, cb) = first_person_humans();
    for (authority, first, second) in [(&mut host, a, b), (&mut control, ca, cb)] {
        selected_command(
            authority,
            first,
            1,
            PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            None,
        );
        selected_command(
            authority,
            second,
            1,
            PlayerButtons::default(),
            Some(ohl_combat::WeaponId::Crowbar),
        );
    }
    let before = host.snapshot();
    let save = host.game().to_save(31).to_bytes().unwrap();
    let ai = host.game().ai_state_hash();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut view = replica(&first);
    for tick in 1..5 {
        assert_eq!(host.capture_recipient(EPOCH, a).unwrap(), first);
        view.apply(restamp(&first, tick, 0.0)).unwrap();
        let _ = recipient::hud(view.frame().unwrap(), true);
        let _ = recipient::scoreboard(view.frame().unwrap());
        reject(
            &mut view,
            restamp(&first, tick, 0.0),
            SceneError::StaleTick.into(),
        );
    }
    assert_eq!(host.snapshot(), before);
    assert_eq!(host.game().to_save(31).to_bytes().unwrap(), save);
    assert_eq!(host.game().ai_state_hash(), ai);
    for dt in [0.0, TICK_SECONDS / 2.0] {
        host.tick(dt);
        control.tick(dt);
        assert_eq!(host.capture_recipient(EPOCH, a).unwrap(), first);
    }
    host.tick(TICK_SECONDS / 2.0);
    control.tick(TICK_SECONDS / 2.0);
    assert_eq!(host.snapshot(), control.snapshot());
    assert_eq!(
        first_person(&host, a).pose,
        expected_first_person_pose(1, TICK_SECONDS)
    );
    for (id, cid) in [(a, ca), (b, cb)] {
        let actual = first_person(&host, id);
        let expected = first_person(&control, cid);
        assert_eq!(actual.pose, expected.pose);
        assert_eq!(actual.transform, expected.transform);
    }
    let selected = host.capture_recipient(EPOCH, a).unwrap();
    let vm = selected.state().view_model.as_ref().unwrap();
    let camera = selected.state().camera;
    assert_eq!(vm.transform[12..15], camera.eye);
    assert_ne!(camera.eye, host.game().eye_position());
}

#[test]
fn real_death_respawn_and_slot_reuse_reset_first_person_playback() {
    let (mut host, shooter, victim) = first_person_humans();
    let original = first_person(&host, victim);
    for sequence in 0..1000 {
        let snapshot = host.snapshot();
        if !snapshot.players[1].alive {
            break;
        }
        let delta = Vec3::from_array(snapshot.players[1].origin)
            - (Vec3::from_array(snapshot.players[0].origin) + Vec3::Z * 28.0);
        host.submit(
            shooter,
            PlayerCommand {
                sequence,
                view_angles: [
                    (-delta.z).atan2(delta.truncate().length()).to_degrees(),
                    delta.y.atan2(delta.x).to_degrees(),
                    0.0,
                ],
                buttons: PlayerButtons {
                    attack: sequence % 2 == 0,
                    ..PlayerButtons::default()
                },
                ..PlayerCommand::default()
            },
        )
        .unwrap();
        host.tick(TICK_SECONDS);
    }
    assert!(
        !host.snapshot().players[1].alive,
        "real combat must reach the death lifecycle"
    );
    assert!(
        host.capture_recipient(EPOCH, victim)
            .unwrap()
            .state()
            .view_model
            .is_none()
    );
    for _ in 0..250 {
        host.tick(TICK_SECONDS);
    }
    assert!(!host.snapshot().players[1].alive);
    selected_command(
        &mut host,
        victim,
        1,
        PlayerButtons {
            attack: true,
            ..PlayerButtons::default()
        },
        None,
    );
    host.tick(TICK_SECONDS);
    assert!(host.snapshot().players[1].alive);
    let respawned = first_person(&host, victim);
    assert_eq!(respawned.model, original.model);
    assert_eq!(
        respawned.pose,
        expected_first_person_pose(1, 0.0),
        "late rules respawn must not advance playback a second time"
    );
    host.tick(TICK_SECONDS);
    assert_eq!(
        first_person(&host, victim).pose,
        expected_first_person_pose(1, TICK_SECONDS)
    );
    let before_leave = host.capture_recipient(EPOCH, victim).unwrap();
    let mut old_view = replica(&before_leave);
    host.leave(victim).unwrap();
    assert_eq!(
        host.capture_recipient(EPOCH, victim),
        Err(RecipientError::WrongRecipient)
    );
    let replacement = host.join("Fresh life").unwrap();
    assert_eq!(replacement.slot, victim.slot);
    assert_ne!(replacement.generation, victim.generation);
    let fresh = host.capture_recipient(EPOCH, replacement).unwrap();
    assert_eq!(
        fresh.state().view_model.as_ref().unwrap().pose,
        expected_first_person_pose(1, 0.0)
    );
    reject(&mut old_view, fresh.clone(), RecipientError::WrongRecipient);
    assert!(
        replica(&fresh)
            .frame()
            .unwrap()
            .state()
            .view_model
            .is_some()
    );
}

#[test]
fn invalid_viewmodels_and_invalid_worlds_preserve_the_whole_recipient() {
    let (host, a, _) = first_person_humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut view = replica(&first);
    let (other, oa, _) = first_person_humans();
    let mut cases = Vec::new();
    for id in [
        PlayerId {
            generation: a.generation + 1,
            ..a
        },
        PlayerId {
            session: SessionId(99),
            ..a
        },
    ] {
        let mut state = first.state().clone();
        state.recipient = id;
        cases.push((state, RecipientError::WrongRecipient));
    }
    let mut state = first.state().clone();
    state.view_model = Some(first_person(&other, oa));
    cases.push((state, SceneError::UnknownResource.into()));
    let mut state = first.state().clone();
    state.view_model.as_mut().unwrap().pose.matrices.pop();
    cases.push((state, SceneError::PoseLayout.into()));
    let mut state = first.state().clone();
    state.view_model.as_mut().unwrap().pose.matrices[0][0] = f32::NAN;
    cases.push((state, SceneError::NonFiniteState.into()));
    let mut state = first.state().clone();
    state.view_model.as_mut().unwrap().transform[0] = f32::INFINITY;
    cases.push((state, RecipientError::InvalidViewModel));
    let mut state = first.state().clone();
    state.hud.weapon = None;
    state.hud.clip_ammo = None;
    state.hud.reserve_ammo = None;
    cases.push((state, RecipientError::InvalidViewModel));
    let mut state = first.state().clone();
    state
        .scoreboard
        .rows
        .iter_mut()
        .find(|row| row.id == a)
        .unwrap()
        .alive = false;
    cases.push((state, RecipientError::InvalidViewModel));
    for (mut state, error) in cases {
        state.hud.health = 13;
        reject(
            &mut view,
            RecipientFrame::new(restamp(&first, 1, 0.0).scene().clone(), state),
            error,
        );
    }
    let mut studios = first.scene().studios().to_vec();
    studios[0].transform[0] = f32::NAN;
    let invalid = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        1,
        0.0,
        Vec::new(),
        studios,
    );
    let mut state = first.state().clone();
    state.hud.health = 13;
    reject(
        &mut view,
        RecipientFrame::new(invalid, state),
        SceneError::NonFiniteState.into(),
    );
    reject(&mut view, first.clone(), SceneError::StaleTick.into());
    let mut none = first.state().clone();
    none.view_model = None;
    none.hud.health = 13;
    view.apply(RecipientFrame::new(
        restamp(&first, 1, 0.0).scene().clone(),
        none,
    ))
    .unwrap();
    assert!(view.frame().unwrap().state().view_model.is_none());
    assert_eq!(recipient::hud(view.frame().unwrap(), true).health, 13);
    let text = format!(
        "{:?} {:?}",
        first.state().view_model,
        RecipientError::InvalidViewModel
    );
    for forbidden in ["matrices", "pose:", "transform:", "model:", "catalog:"] {
        assert!(!text.contains(forbidden));
    }
}

#[test]
fn viewmodel_world_and_held_geometry_share_one_aggregate_budget() {
    let (host, a, _) = first_person_humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut studios = first.scene().studios().to_vec();
    studios[0].held = Some(HeldModelRecord {
        model: studios[0].model.clone(),
        pose: studios[0].pose.clone(),
        body: Vec::new(),
        skin: 0,
    });
    let scene = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        0,
        0.0,
        first.scene().brushes().to_vec(),
        studios,
    );
    let instances = scene.brushes().len() + scene.studios().len() + 1;
    let bones = scene
        .studios()
        .iter()
        .map(|studio| {
            studio.pose.matrices.len()
                + studio
                    .held
                    .as_ref()
                    .map_or(0, |held| held.pose.matrices.len())
        })
        .sum::<usize>();
    let vm_bones = first
        .state()
        .view_model
        .as_ref()
        .unwrap()
        .pose
        .matrices
        .len();
    for (limits, error) in [
        (
            SceneLimits {
                max_instances: instances,
                max_bones: bones + vm_bones,
            },
            SceneError::InstanceLimit,
        ),
        (
            SceneLimits {
                max_instances: instances + 1,
                max_bones: bones,
            },
            SceneError::BoneLimit,
        ),
    ] {
        let mut ordinary =
            SceneReplica::with_limits(scene.catalog().clone(), EPOCH, limits).unwrap();
        ordinary.apply(scene.clone()).unwrap();
        let mut recipient =
            RecipientReplica::new(scene.catalog().clone(), EPOCH, a, limits).unwrap();
        assert_eq!(
            recipient.apply(RecipientFrame::new(scene.clone(), first.state().clone())),
            Err(error.into())
        );
        assert!(recipient.frame().is_none());
    }
    let mut boundary = RecipientReplica::new(
        scene.catalog().clone(),
        EPOCH,
        a,
        SceneLimits {
            max_instances: instances + 1,
            max_bones: bones + vm_bones,
        },
    )
    .unwrap();
    boundary
        .apply(RecipientFrame::new(scene, first.state().clone()))
        .unwrap();
    assert!(boundary.frame().unwrap().state().view_model.is_some());
}

/// The near wall fills the complete viewport; the exact produced VM geometry
/// becomes a separately identified world draw behind it for the depth control.
fn authored_depth_control(first: &RecipientFrame) -> (StudioRecord, StudioRecord) {
    let vm = first.state().view_model.as_ref().unwrap();
    let eye = first.state().camera;
    let camera = FreeFlyCamera {
        position: eye.eye,
        pitch: eye.pitch,
        yaw: eye.yaw,
        ..FreeFlyCamera::default()
    };
    let forward = Vec3::from_array(camera.direction());
    let left = Vec3::Z.cross(forward).normalize();
    let up = forward.cross(left);
    let mut entities = World::new();
    let mut next_identity = || {
        for _ in 0..=ohl_engine::scene::MAX_SCENE_INSTANCES {
            let id = SceneIdentity::from_entity(EPOCH, entities.spawn(()));
            if id != first.state().body
                && !first
                    .scene()
                    .studios()
                    .iter()
                    .any(|record| record.identity == id)
                && !first
                    .scene()
                    .brushes()
                    .iter()
                    .any(|record| record.identity == id)
            {
                return id;
            }
        }
        panic!("bounded fixture identities must leave an unused slot");
    };
    let mut wall = first.scene().studios()[0].clone();
    wall.identity = next_identity();
    wall.held = None;
    wall.pose.matrices.fill(Mat4::IDENTITY.to_cols_array());
    wall.transform = Mat4::from_cols(
        left.extend(0.0),
        up.extend(0.0),
        forward.extend(0.0),
        // The original authored quad is [0,24] on both axes. Centre it
        // around the eye ray: at depth8 this +/-12 wall covers the entire
        // viewport and the produced weapon at depth42..50, below the eye.
        (Vec3::from_array(eye.eye) + forward * 8.0 - left * 12.0 - up * 12.0).extend(1.0),
    )
    .to_cols_array();
    let mut depth_control = wall.clone();
    depth_control.identity = next_identity();
    depth_control.model = vm.model.clone();
    depth_control.pose = vm.pose.clone();
    depth_control.transform = vm.transform;
    (wall, depth_control)
}

fn pixels(
    host: &mut Deathmatch,
    context: &GpuContext,
    target: &OffscreenTarget,
    view: &RecipientReplica,
    fov: f32,
) -> Result<Vec<u8>, RecipientError> {
    host.render_recipient(
        context,
        RenderTarget {
            view: target.view(),
            width: 192,
            height: 144,
            format: OFFSCREEN_FORMAT,
        },
        view,
        fov,
    )?;
    Ok(target.read_rgba(context).unwrap())
}

fn red_pixel(pixel: [u8; 4]) -> bool {
    pixel[0] > 64
        && u16::from(pixel[0]) > u16::from(pixel[1]) * 2
        && u16::from(pixel[0]) > u16::from(pixel[2]) * 2
}

fn red_pixels(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| red_pixel(**p))
        .count()
}

#[test]
fn produced_viewmodel_reaches_pixels_behind_world_depth_without_advancing_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("opt-in requires a graphics adapter");
    let (mut host, a, _) = first_person_humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let (wall, depth_control) = authored_depth_control(&first);
    let scene = |tick, studios| {
        SceneFrame::new(
            first.scene().catalog().clone(),
            EPOCH,
            tick,
            0.0,
            Vec::new(),
            studios,
        )
    };
    let mut without = first.state().clone();
    without.view_model = None;
    let mut view = replica(&RecipientFrame::new(
        scene(1, vec![wall.clone()]),
        without.clone(),
    ));
    let target = OffscreenTarget::new(&context, 192, 144).unwrap();
    let before = host.snapshot();
    let save = host.game().to_save(31).to_bytes().unwrap();
    let ai = host.game().ai_state_hash();
    let baseline = pixels(&mut host, &context, &target, &view, 75.0).unwrap();
    let grey_pixels = baseline
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 32 && p[0] == p[1] && p[1] == p[2])
        .count();
    assert_eq!(
        grey_pixels,
        192 * 144,
        "the authored grey wall must cover the entire viewport"
    );
    let resources = host.game().render_resource_stats();
    view.apply(RecipientFrame::new(
        scene(2, vec![wall.clone()]),
        first.state().clone(),
    ))
    .unwrap();
    let displayed = pixels(&mut host, &context, &target, &view, 75.0).unwrap();
    let mut changed = 0;
    for (base, actual) in baseline
        .as_chunks::<4>()
        .0
        .iter()
        .zip(displayed.as_chunks::<4>().0.iter())
    {
        if base != actual {
            assert!(
                red_pixel(*actual),
                "only opaque authored weapon pixels may change world colour"
            );
            changed += 1;
        }
    }
    assert!(changed > 0 && changed < 192 * 144 / 4);

    // The exact produced weapon geometry is behind the wall: as ordinary
    // world geometry it must be fully depth-occluded. This authored control
    // has a real synthetic identity, unrelated to the VM's validation path.
    view.apply(RecipientFrame::new(
        scene(3, vec![wall.clone(), depth_control]),
        without,
    ))
    .unwrap();
    let ordinary_depth = pixels(&mut host, &context, &target, &view, 75.0).unwrap();
    assert!(
        ordinary_depth == baseline,
        "the authored near wall must fully occlude the produced weapon in the ordinary depth pass"
    );
    view.apply(RecipientFrame::new(
        scene(4, vec![wall]),
        first.state().clone(),
    ))
    .unwrap();
    for _ in 0..3 {
        assert_eq!(
            pixels(&mut host, &context, &target, &view, 75.0).unwrap(),
            displayed
        );
        assert_eq!(host.game().render_resource_stats(), resources);
    }
    let narrow = pixels(&mut host, &context, &target, &view, 40.0).unwrap();
    assert!(red_pixels(&narrow) > red_pixels(&displayed));
    assert_eq!(host.snapshot(), before);
    assert_eq!(host.game().to_save(31).to_bytes().unwrap(), save);
    assert_eq!(host.game().ai_state_hash(), ai);
    assert_eq!(host.capture_recipient(EPOCH, a).unwrap(), first);
}

#[test]
fn recipient_pixels_use_full_body_identity_camera_and_reuse_resources_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("opt-in requires a graphics adapter");
    let (mut host, a, _) = humans();
    let first = host.capture_recipient(EPOCH, a).unwrap();
    let mut entities = World::new();
    let old = entities.spawn(());
    entities.despawn(old).unwrap();
    let new = entities.spawn(());
    assert_eq!(old.id(), new.id());
    assert_ne!(old, new);
    let mut body = first.scene().studios()[0].clone();
    body.identity = SceneIdentity::from_entity(EPOCH, old);
    body.transform = Mat4::from_scale_rotation_translation(
        Vec3::ONE,
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        Vec3::new(0.0, 0.0, 50.0),
    )
    .to_cols_array();
    body.held = Some(HeldModelRecord {
        model: body.model.clone(),
        pose: body.pose.clone(),
        body: Vec::new(),
        skin: 0,
    });
    let mut state = first.state().clone();
    state.body = body.identity;
    state.camera.eye = [0.0, -150.0, 50.0];
    state.camera.pitch = 0.0;
    state.camera.yaw = 90.0;
    let scene = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        1,
        0.0,
        Vec::new(),
        vec![body.clone()],
    );
    let hidden = RecipientFrame::new(scene, state.clone());
    let mut view = replica(&hidden);
    let target = OffscreenTarget::new(&context, 192, 144).unwrap();
    assert_eq!(
        pixels(&mut host, &context, &target, &view, f32::NAN),
        Err(RecipientError::InvalidCamera)
    );
    assert_eq!(
        host.game().render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
    let before = host.snapshot();
    let save = host.game().to_save(31).to_bytes().unwrap();
    let hidden_pixels = pixels(&mut host, &context, &target, &view, 75.0).unwrap();
    let prepared = host.game().render_resource_stats();
    for _ in 0..3 {
        assert_eq!(
            pixels(&mut host, &context, &target, &view, 75.0).unwrap(),
            hidden_pixels
        );
        assert_eq!(host.game().render_resource_stats(), prepared);
    }
    // Same slot, different entity generation must not suppress the old record
    // or its held geometry. No guessed ID bit layout is used.
    state.body = SceneIdentity::from_entity(EPOCH, new);
    let scene = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        2,
        0.0,
        Vec::new(),
        vec![body],
    );
    view.apply(RecipientFrame::new(scene, state.clone()))
        .unwrap();
    let visible = pixels(&mut host, &context, &target, &view, 75.0).unwrap();
    assert_ne!(visible, hidden_pixels);
    state.camera.yaw = 270.0;
    let scene = SceneFrame::new(
        first.scene().catalog().clone(),
        EPOCH,
        3,
        0.0,
        Vec::new(),
        view.frame().unwrap().scene().studios().to_vec(),
    );
    view.apply(RecipientFrame::new(scene, state)).unwrap();
    assert_ne!(
        pixels(&mut host, &context, &target, &view, 75.0).unwrap(),
        visible
    );
    assert_eq!(host.snapshot(), before);
    assert_eq!(host.game().to_save(31).to_bytes().unwrap(), save);
    let (mut foreign, _, _) = humans();
    assert_eq!(
        pixels(&mut foreign, &context, &target, &view, 75.0),
        Err(SceneError::WrongCatalog.into())
    );
    assert_eq!(
        foreign.game().render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
}
