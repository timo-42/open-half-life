//! Project-authored authority/presentation fixtures only; no captured packets,
//! original assets, first-person animation or interoperability assertions.
#![allow(clippy::float_cmp)]

use glam::{Mat4, Quat, Vec3};
use ohl_combat::EntityId;
use ohl_engine::deathmatch::{
    Deathmatch, DeathmatchConfig, PlayerButtons, PlayerCommand, PlayerId, SessionId,
};
use ohl_engine::recipient::{self, RecipientError, RecipientFrame, RecipientReplica};
use ohl_engine::scene::{
    HeldModelRecord, SceneEpoch, SceneError, SceneFrame, SceneIdentity, SceneLimits,
};
use ohl_engine::test_support::{AI_MAP, deathmatch_room_bsp};
use ohl_engine::{MemoryAssets, RenderTarget, SkirmishConfig, TICK_SECONDS};
use ohl_formats::test_support::build_minimal_mdl10;
use ohl_game::hecs::World;
use ohl_physics::PlayerController;
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};

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
    let mut assets = MemoryAssets::new();
    let (mut model, layout) = build_minimal_mdl10();
    for vertex in 0..4 {
        for axis in 0..2 {
            let offset = layout.verts_offset + vertex * 12 + axis * 4;
            let value = f32::from_le_bytes(model[offset..offset + 4].try_into().unwrap());
            model[offset..offset + 4].copy_from_slice(&(value * 24.0).to_le_bytes());
        }
    }
    assets.insert(ohl_engine::skirmish::PLAYER_MODEL_PATH, model);
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
