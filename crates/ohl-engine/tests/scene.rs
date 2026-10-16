//! Presentation tests use only project-authored BSP/MDL generators and synthetic
//! names. The player/held selectors reuse the skirmish selector provenance in
//! docs/FORMAT_SOURCES.md; their fixture contents are entirely synthetic.
//! Exact float comparisons assert copied immutable state or analytic defaults;
//! geometric rotation comparisons use a tolerance.
#![allow(clippy::float_cmp)]

use glam::{Mat4, Vec3};
use ohl_combat::EntityId;
use ohl_engine::components::{HeldWeapon, StudioGait};
use ohl_engine::scene::{
    HeldModelRecord, MAX_SCENE_BODY_SELECTIONS, MAX_SCENE_BONES, MAX_SCENE_INSTANCES, SceneEpoch,
    SceneError, SceneFrame, SceneGait, SceneIdentity, SceneLimits, SceneReplica, StudioRecord,
};
use ohl_engine::test_support::{
    DOOR_NAME, LANDMARK, NEXT_MAP, ROTATING_DOOR_MAP, ROTATING_DOOR_NAME, ROTATING_DOOR_PIVOT,
    SYNTHETIC_MAP, entity_block, rotating_door_bsp, rotating_door_entities,
    synthetic_map_bsp_with_extra_entity,
};
use ohl_engine::{
    Game, Input, MemoryAssets, Pickup, RenderTarget, SkirmishConfig, StudioAnim, TICK_SECONDS,
    entity_id,
};
use ohl_formats::test_support::{build_biped_mdl10_with_sequences, build_minimal_mdl10};
use ohl_game::hecs::{Entity, World};
use ohl_game::registry::{Door, MoverState, Transform};
use ohl_render::{FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, RenderMode};

const EPOCH: SceneEpoch = SceneEpoch::new(73);
const ACTOR: &str = "ohl_scene_actor";
const PICKUP: &str = "ohl_scene_pickup";
const MODEL_A: &str = "models/ohl_scene_a.mdl";
const MODEL_B: &str = "models/ohl_scene_b.mdl";

fn model() -> Vec<u8> {
    let mut bytes = build_biped_mdl10_with_sequences(&[
        "idle",
        "run",
        "ohl_aim_onehanded",
        "ohl_shoot_onehanded",
    ]);
    let (_, layout) = build_minimal_mdl10();
    for vertex in 0..4 {
        for axis in 0..2 {
            let offset = layout.verts_offset + vertex * 12 + axis * 4;
            let value = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            bytes[offset..offset + 4].copy_from_slice(&(value * 24.0).to_le_bytes());
        }
    }
    bytes
}

fn game() -> (Game, MemoryAssets) {
    let extra = format!(
        "{}{}",
        entity_block(
            "monster_generic",
            [-64.0, 0.0, 40.0],
            0.0,
            &[("targetname", ACTOR), ("model", MODEL_A)]
        ),
        entity_block(
            "item_healthkit",
            [64.0, 0.0, 40.0],
            0.0,
            &[("targetname", PICKUP), ("model", MODEL_B)]
        ),
    );
    let map = synthetic_map_bsp_with_extra_entity(NEXT_MAP, &extra);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), map.clone());
    assets.insert(&format!("maps/{NEXT_MAP}.bsp"), map);
    for path in [
        MODEL_A,
        MODEL_B,
        ohl_engine::skirmish::PLAYER_MODEL_PATH,
        "models/p_9mmhandgun.mdl",
    ] {
        assets.insert(path, model());
    }
    (Game::load(&assets, SYNTHETIC_MAP).unwrap(), assets)
}

fn named(game: &Game, name: &str) -> Entity {
    game.registry().find(name)[0]
}

fn studio(frame: &SceneFrame, entity: Entity) -> &StudioRecord {
    frame
        .studios()
        .iter()
        .find(|record| record.identity.local_id() == entity_id(entity))
        .unwrap()
}

fn restamp(frame: &SceneFrame, tick: u64, time: f32) -> SceneFrame {
    SceneFrame::new(
        frame.catalog().clone(),
        frame.epoch(),
        tick,
        time,
        frame.brushes().to_vec(),
        frame.studios().to_vec(),
    )
}

fn records(
    frame: &SceneFrame,
    brushes: Vec<ohl_engine::scene::BrushRecord>,
    studios: Vec<StudioRecord>,
) -> SceneFrame {
    SceneFrame::new(
        frame.catalog().clone(),
        frame.epoch(),
        frame.tick() + 1,
        frame.time(),
        brushes,
        studios,
    )
}

fn assert_rejected(replica: &mut SceneReplica, candidate: SceneFrame, expected: SceneError) {
    let previous = replica.frame().cloned();
    assert_eq!(replica.apply(candidate), Err(expected));
    assert_eq!(replica.frame(), previous.as_ref());
}

fn accepted(frame: &SceneFrame) -> SceneReplica {
    let mut replica = SceneReplica::new(frame.catalog().clone(), frame.epoch());
    replica.apply(frame.clone()).unwrap();
    replica
}

#[test]
fn headless_capture_owns_complete_records_without_advancing_authority() {
    let (game, _) = game();
    let before = game.save_bytes(42).unwrap();
    let ai_before = game.ai_state_hash();
    let frame = game.capture_scene(EPOCH).unwrap();
    assert_eq!(frame.tick(), 0);
    assert_eq!(frame.time(), 0.0);
    assert_eq!(frame.epoch(), EPOCH);
    assert_eq!(frame.brushes().len(), 1);
    assert_eq!(frame.studios().len(), 2);
    assert!(
        frame
            .studios()
            .iter()
            .all(|record| record.pose.matrices.len() == 7)
    );
    assert_eq!(
        game.render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
    let mut replica = accepted(&frame);
    replica.apply(restamp(&frame, 1, 1.0)).unwrap();
    assert_eq!(game.save_bytes(42).unwrap(), before);
    assert_eq!(game.ai_state_hash(), ai_before);
    assert_eq!(game.capture_scene(EPOCH).unwrap(), frame);
}

#[test]
fn capture_reports_the_existing_fixed_step_stamp_without_advancing_it() {
    let (mut game, _) = game();
    game.tick(TICK_SECONDS, &Input::default());
    let frame = game.capture_scene(EPOCH).unwrap();
    assert_eq!(frame.tick(), 1);
    assert_eq!(frame.time(), TICK_SECONDS);
    assert_eq!(game.capture_scene(EPOCH).unwrap(), frame);
    assert_eq!(game.elapsed(), TICK_SECONDS);
}

#[test]
fn capture_uses_live_translation_and_quarter_turn_brush_poses() {
    let (game, _) = game();
    let entity = named(&game, DOOR_NAME);
    let closed = game.capture_scene(EPOCH).unwrap();
    assert_eq!(
        closed.brushes()[0].transform,
        Mat4::IDENTITY.to_cols_array()
    );
    let offset = {
        let mut door = game.registry().world.get::<&mut Door>(entity).unwrap();
        door.state = MoverState::Open;
        door.movedir * door.travel_distance
    };
    assert!(offset.length() > 0.0);
    let open = game.capture_scene(EPOCH).unwrap();
    assert_eq!(
        open.brushes()[0].transform,
        Mat4::from_translation(offset).to_cols_array()
    );

    let bytes = rotating_door_bsp(&rotating_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let rotating = Game::load(&assets, ROTATING_DOOR_MAP).unwrap();
    let entity = named(&rotating, ROTATING_DOOR_NAME);
    rotating
        .registry()
        .world
        .get::<&mut Door>(entity)
        .unwrap()
        .state = MoverState::Open;
    let frame = rotating.capture_scene(EPOCH).unwrap();
    let placed =
        Mat4::from_cols_array(&frame.brushes()[0].transform).transform_point3(Vec3::X * 8.0);
    let expected = Vec3::from_array(ROTATING_DOOR_PIVOT) + Vec3::Y * 8.0;
    assert!((placed - expected).length() < 1e-4);
}

#[test]
fn capture_samples_gait_animation_and_held_model_changes_without_ticking() {
    let (mut game, assets) = game();
    game.start_skirmish(
        &assets,
        &SkirmishConfig {
            bots: 1,
            ..SkirmishConfig::default()
        },
    )
    .unwrap();
    let bot = game.skirmish_bots()[0];
    {
        let mut anim = game.registry().world.get::<&mut StudioAnim>(bot).unwrap();
        anim.sequence = 3;
        anim.cycle = 0.1;
        anim.frame_rate = 0.5;
        anim.body = 19;
        anim.skin = 23;
    }
    {
        let mut gait = game.registry().world.get::<&mut StudioGait>(bot).unwrap();
        gait.sequence = Some(1);
        gait.cycle = 0.05;
        gait.frame_rate = 0.75;
    }
    let before = game.capture_scene(EPOCH).unwrap();
    let body = studio(&before, bot);
    assert!((body.pose.matrices[1][14] - 0.5).abs() < 1e-5);
    assert_eq!(body.animation.sequence, 3);
    assert_eq!(body.animation.cycle, 0.1);
    assert_eq!(body.animation.frame_rate, 0.5);
    assert_eq!(body.animation.gait.unwrap().cycle, 0.05);
    assert_eq!(body.body, [19]);
    assert_eq!(body.skin, 23);
    assert_eq!(body.held.as_ref().unwrap().pose, body.pose);
    let old_slot = body.held.as_ref().unwrap().model.local_index();
    let replacement = game
        .scene_catalog()
        .studio_handles()
        .find(|handle| handle.local_index() != old_slot)
        .unwrap();
    game.registry()
        .world
        .get::<&mut HeldWeapon>(bot)
        .unwrap()
        .model = Some(replacement.local_index());
    let after = game.capture_scene(EPOCH).unwrap();
    assert_eq!(
        studio(&after, bot).held.as_ref().unwrap().model,
        replacement
    );
    assert_eq!(
        studio(&before, bot)
            .held
            .as_ref()
            .unwrap()
            .model
            .local_index(),
        old_slot
    );
    game.registry()
        .world
        .get::<&mut HeldWeapon>(bot)
        .unwrap()
        .model = None;
    assert!(
        studio(&game.capture_scene(EPOCH).unwrap(), bot)
            .held
            .is_none()
    );
    assert_eq!(game.elapsed(), 0.0);
}

#[test]
fn source_mutation_cannot_change_an_earlier_owned_frame() {
    let (game, _) = game();
    let entity = named(&game, ACTOR);
    let first = game.capture_scene(EPOCH).unwrap();
    let frozen = first.clone();
    game.registry()
        .world
        .get::<&mut Transform>(entity)
        .unwrap()
        .origin += Vec3::X * 100.0;
    game.registry()
        .world
        .get::<&mut StudioAnim>(entity)
        .unwrap()
        .cycle = 0.15;
    let second = game.capture_scene(EPOCH).unwrap();
    assert_ne!(
        studio(&second, entity).transform,
        studio(&first, entity).transform
    );
    assert_ne!(studio(&second, entity).pose, studio(&first, entity).pose);
    assert_eq!(first, frozen);
}

#[test]
fn full_replacement_removes_hidden_pickups_absent_actors_and_held_geometry() {
    let (mut game, assets) = game();
    // Arena setup classifies the existing pickups without advancing a tick.
    game.start_skirmish(
        &assets,
        &SkirmishConfig {
            bots: 0,
            ..SkirmishConfig::default()
        },
    )
    .unwrap();
    let pickup = named(&game, PICKUP);
    let actor = named(&game, ACTOR);
    let initial = game.capture_scene(EPOCH).unwrap();
    let mut replica = accepted(&initial);
    game.registry()
        .world
        .get::<&mut Pickup>(pickup)
        .unwrap()
        .taken = true;
    let hidden = game.capture_scene(EPOCH).unwrap();
    assert!(
        !hidden
            .studios()
            .iter()
            .any(|record| record.identity.local_id() == entity_id(pickup))
    );
    replica.apply(restamp(&hidden, 1, hidden.time())).unwrap();
    assert_eq!(replica.frame().unwrap().studios().len(), 1);

    let mut body = studio(&hidden, actor).clone();
    body.held = Some(HeldModelRecord {
        model: body.model.clone(),
        pose: body.pose.clone(),
        body: vec![0],
        skin: 0,
    });
    replica
        .apply(SceneFrame::new(
            hidden.catalog().clone(),
            EPOCH,
            2,
            0.0,
            hidden.brushes().to_vec(),
            vec![body.clone()],
        ))
        .unwrap();
    body.held = None;
    replica
        .apply(SceneFrame::new(
            hidden.catalog().clone(),
            EPOCH,
            3,
            0.0,
            hidden.brushes().to_vec(),
            vec![body],
        ))
        .unwrap();
    assert!(replica.frame().unwrap().studios()[0].held.is_none());
    replica
        .apply(SceneFrame::new(
            hidden.catalog().clone(),
            EPOCH,
            4,
            0.0,
            Vec::new(),
            Vec::new(),
        ))
        .unwrap();
    assert!(replica.frame().unwrap().studios().is_empty());
    assert!(replica.frame().unwrap().brushes().is_empty());
    // Presentation removal did not despawn the authority's still-live actor.
    assert!(game.registry().world.contains(actor));
}

#[test]
fn recycled_generations_get_distinct_epoch_qualified_local_identities() {
    let (game, _) = game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let mut world = World::new();
    let first = world.spawn(());
    let mut body = frame.studios()[0].clone();
    body.identity = SceneIdentity::from_entity(EPOCH, first);
    let initial = SceneFrame::new(
        frame.catalog().clone(),
        EPOCH,
        0,
        0.0,
        Vec::new(),
        vec![body.clone()],
    );
    let mut replica = accepted(&initial);
    world.despawn(first).unwrap();
    let second = world.spawn(());
    assert_eq!(first.id(), second.id());
    let previous = body.identity;
    body.identity = SceneIdentity::from_entity(EPOCH, second);
    assert_ne!(body.identity, previous);
    replica
        .apply(SceneFrame::new(
            frame.catalog().clone(),
            EPOCH,
            1,
            0.0,
            Vec::new(),
            vec![body.clone()],
        ))
        .unwrap();
    assert_eq!(
        replica.frame().unwrap().studios()[0].identity,
        body.identity
    );
    assert_ne!(replica.frame().unwrap().studios()[0].identity, previous);
}

#[test]
fn identical_independent_loads_never_share_catalog_or_handle_associations() {
    let (first, _) = game();
    let (second, _) = game();
    assert_ne!(first.scene_catalog(), second.scene_catalog());
    let initial = first.capture_scene(EPOCH).unwrap();
    let mut replica = accepted(&initial);
    assert_rejected(
        &mut replica,
        second.capture_scene(EPOCH).unwrap(),
        SceneError::WrongCatalog,
    );
    let mut studios = initial.studios().to_vec();
    studios[0].model = second
        .scene_catalog()
        .studio_handle(studios[0].model.local_index())
        .unwrap();
    assert_rejected(
        &mut replica,
        records(&initial, initial.brushes().to_vec(), studios),
        SceneError::UnknownResource,
    );
}

#[test]
fn catalog_growth_and_level_change_invalidate_old_associations_atomically() {
    let (mut game, assets) = game();
    let initial = game.capture_scene(EPOCH).unwrap();
    let mut old = accepted(&initial);
    game.start_skirmish(
        &assets,
        &SkirmishConfig {
            bots: 0,
            ..SkirmishConfig::default()
        },
    )
    .unwrap();
    let grown = game.capture_scene(EPOCH).unwrap();
    assert_ne!(initial.catalog(), grown.catalog());
    assert_rejected(&mut old, grown.clone(), SceneError::WrongCatalog);
    let mut new = accepted(&grown);
    let mut studios = grown.studios().to_vec();
    studios[0].model = initial.studios()[0].model.clone();
    assert_rejected(
        &mut new,
        records(&grown, grown.brushes().to_vec(), studios),
        SceneError::UnknownResource,
    );

    let (mut campaign, assets) = self::game();
    let before = campaign.capture_scene(EPOCH).unwrap();
    campaign.change_level(&assets, NEXT_MAP, LANDMARK).unwrap();
    let after = campaign.capture_scene(EPOCH).unwrap();
    assert_ne!(before.catalog(), after.catalog());
    let mut next = accepted(&after);
    assert_rejected(&mut next, before, SceneError::WrongCatalog);
}

#[test]
fn epoch_freshness_and_time_rejections_preserve_the_previous_frame() {
    let (game, _) = game();
    let frame = restamp(&game.capture_scene(EPOCH).unwrap(), 10, 2.0);
    let mut replica = accepted(&frame);
    assert_rejected(
        &mut replica,
        restamp(&frame, 10, 3.0),
        SceneError::StaleTick,
    );
    assert_rejected(&mut replica, restamp(&frame, 9, 3.0), SceneError::StaleTick);
    assert_rejected(
        &mut replica,
        restamp(&frame, 11, 1.5),
        SceneError::StaleTime,
    );
    let wrong_epoch = SceneFrame::new(
        frame.catalog().clone(),
        SceneEpoch::new(74),
        11,
        3.0,
        frame.brushes().to_vec(),
        frame.studios().to_vec(),
    );
    assert_rejected(&mut replica, wrong_epoch, SceneError::WrongEpoch);
    for time in [f32::NAN, f32::INFINITY, -1.0] {
        assert_rejected(
            &mut replica,
            restamp(&frame, 11, time),
            SceneError::InvalidTime,
        );
    }
    replica.apply(restamp(&frame, 11, 2.0)).unwrap();
    assert_eq!(replica.frame().unwrap().tick(), 11);
}

#[test]
fn duplicate_invalid_and_wrong_epoch_identities_fail_atomically() {
    let (game, _) = game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let mut replica = accepted(&frame);
    let body = frame.studios()[0].clone();
    assert_rejected(
        &mut replica,
        records(&frame, Vec::new(), vec![body.clone(), body]),
        SceneError::DuplicateIdentity,
    );
    let mut cross_family = frame.studios().to_vec();
    cross_family[0].identity = frame.brushes()[0].identity;
    assert_rejected(
        &mut replica,
        records(&frame, frame.brushes().to_vec(), cross_family),
        SceneError::DuplicateIdentity,
    );
    for (identity, expected) in [
        (
            SceneIdentity::from_local_id(EPOCH, EntityId(0)),
            SceneError::InvalidIdentity,
        ),
        (
            SceneIdentity::from_local_id(
                SceneEpoch::new(74),
                frame.studios()[0].identity.local_id(),
            ),
            SceneError::WrongEpoch,
        ),
    ] {
        let mut studios = frame.studios().to_vec();
        studios[0].identity = identity;
        assert_rejected(
            &mut replica,
            records(&frame, frame.brushes().to_vec(), studios),
            expected,
        );
    }
}

#[test]
fn unknown_and_foreign_resource_handles_are_rejected_without_state_changes() {
    let (game, _) = game();
    let (other, _) = self::game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let mut replica = accepted(&frame);
    assert_eq!(
        game.scene_catalog().brush_handle(u32::MAX).unwrap_err(),
        SceneError::UnknownResource
    );
    assert_eq!(
        game.scene_catalog().studio_handle(usize::MAX).unwrap_err(),
        SceneError::UnknownResource
    );
    let foreign = other.scene_catalog().studio_handles().next().unwrap();
    assert_eq!(
        game.scene_catalog().studio_bone_count(&foreign),
        Err(SceneError::UnknownResource)
    );
    let mut brushes = frame.brushes().to_vec();
    brushes[0].model = other.scene_catalog().brush_handles().next().unwrap();
    assert_rejected(
        &mut replica,
        records(&frame, brushes, frame.studios().to_vec()),
        SceneError::UnknownResource,
    );
    let mut studios = frame.studios().to_vec();
    studios[0].held = Some(HeldModelRecord {
        model: foreign,
        pose: studios[0].pose.clone(),
        body: vec![0],
        skin: 0,
    });
    assert_rejected(
        &mut replica,
        records(&frame, frame.brushes().to_vec(), studios),
        SceneError::UnknownResource,
    );
}

#[test]
fn nonfinite_state_and_incompatible_pose_layouts_fail_atomically() {
    let (game, _) = game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let mut replica = accepted(&frame);
    let mutations: [fn(&mut StudioRecord); 6] = [
        |record| record.transform[12] = f32::NAN,
        |record| record.ambient[0] = f32::INFINITY,
        |record| record.animation.cycle = f32::NAN,
        |record| record.animation.frame_rate = f32::INFINITY,
        |record| record.pose.matrices[0][0] = f32::NAN,
        |record| {
            record.animation.gait = Some(SceneGait {
                sequence: Some(0),
                cycle: f32::NAN,
                frame_rate: 1.0,
            });
        },
    ];
    for mutate in mutations {
        let mut studios = frame.studios().to_vec();
        mutate(&mut studios[0]);
        assert_rejected(
            &mut replica,
            records(&frame, frame.brushes().to_vec(), studios),
            SceneError::NonFiniteState,
        );
    }
    let mut brushes = frame.brushes().to_vec();
    brushes[0].transform[0] = f32::NEG_INFINITY;
    assert_rejected(
        &mut replica,
        records(&frame, brushes, frame.studios().to_vec()),
        SceneError::NonFiniteState,
    );
    let mut studios = frame.studios().to_vec();
    studios[0].pose.matrices.pop();
    assert_rejected(
        &mut replica,
        records(&frame, frame.brushes().to_vec(), studios),
        SceneError::PoseLayout,
    );
    let mut studios = frame.studios().to_vec();
    studios[0].body = vec![0; MAX_SCENE_BODY_SELECTIONS + 1];
    assert_rejected(
        &mut replica,
        records(&frame, frame.brushes().to_vec(), studios),
        SceneError::BodyLimit,
    );
}

#[test]
fn hard_and_reduced_instance_bone_and_held_budgets_are_atomic() {
    let (game, _) = game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let empty = SceneFrame::new(
        frame.catalog().clone(),
        EPOCH,
        0,
        0.0,
        Vec::new(),
        Vec::new(),
    );
    let mut limited = SceneReplica::with_limits(
        frame.catalog().clone(),
        EPOCH,
        SceneLimits {
            max_instances: 0,
            max_bones: 0,
        },
    )
    .unwrap();
    limited.apply(empty.clone()).unwrap();
    assert_rejected(
        &mut limited,
        restamp(&frame, 1, 0.0),
        SceneError::InstanceLimit,
    );
    for limits in [
        SceneLimits {
            max_instances: MAX_SCENE_INSTANCES + 1,
            max_bones: 0,
        },
        SceneLimits {
            max_instances: 0,
            max_bones: MAX_SCENE_BONES + 1,
        },
    ] {
        assert_eq!(
            SceneReplica::with_limits(frame.catalog().clone(), EPOCH, limits).unwrap_err(),
            SceneError::InvalidLimits
        );
    }
    let mut replica = accepted(&frame);
    assert_rejected(
        &mut replica,
        records(
            &frame,
            vec![frame.brushes()[0].clone(); MAX_SCENE_INSTANCES + 1],
            Vec::new(),
        ),
        SceneError::InstanceLimit,
    );
    let mut body = frame.studios()[0].clone();
    let bones = body.pose.matrices.len();
    body.held = Some(HeldModelRecord {
        model: body.model.clone(),
        pose: body.pose.clone(),
        body: vec![0],
        skin: 0,
    });
    let held = SceneFrame::new(
        frame.catalog().clone(),
        EPOCH,
        1,
        0.0,
        Vec::new(),
        vec![body],
    );
    for (limits, expected) in [
        (
            SceneLimits {
                max_instances: 1,
                max_bones: MAX_SCENE_BONES,
            },
            SceneError::InstanceLimit,
        ),
        (
            SceneLimits {
                max_instances: MAX_SCENE_INSTANCES,
                max_bones: bones,
            },
            SceneError::BoneLimit,
        ),
    ] {
        let mut replica =
            SceneReplica::with_limits(frame.catalog().clone(), EPOCH, limits).unwrap();
        replica.apply(empty.clone()).unwrap();
        assert_rejected(&mut replica, held.clone(), expected);
    }
}

#[test]
fn finite_animation_body_and_skin_fallbacks_remain_effective() {
    let (game, _) = game();
    let actor = named(&game, ACTOR);
    let bind = game.capture_scene(EPOCH).unwrap();
    game.registry()
        .world
        .get::<&mut StudioAnim>(actor)
        .unwrap()
        .cycle = 0.15;
    let active = game.capture_scene(EPOCH).unwrap();
    assert_ne!(studio(&active, actor).pose, studio(&bind, actor).pose);
    {
        let mut anim = game.registry().world.get::<&mut StudioAnim>(actor).unwrap();
        anim.sequence = usize::MAX;
        anim.body = u32::MAX;
        anim.skin = usize::MAX;
    }
    let fallback = game.capture_scene(EPOCH).unwrap();
    assert_eq!(studio(&fallback, actor).pose, studio(&bind, actor).pose);
    assert_eq!(studio(&fallback, actor).animation.sequence, usize::MAX);
    assert_eq!(studio(&fallback, actor).body, [u32::MAX]);
    assert_eq!(studio(&fallback, actor).skin, usize::MAX);
    accepted(&fallback);
}

#[test]
fn capture_uses_effective_render_properties_and_rejects_nonfinite_state() {
    let (game, _) = game();
    let actor = named(&game, ACTOR);
    {
        let mut props = game
            .registry()
            .world
            .get::<&mut ohl_game::keyvalues::RenderProps>(actor)
            .unwrap();
        props.mode = 2;
        props.amt = 128;
        props.color = [7, 19, 31];
    }
    let frame = game.capture_scene(EPOCH).unwrap();
    let props = studio(&frame, actor).render;
    assert_eq!(props.mode, RenderMode::Texture);
    assert_eq!(props.amount, 128);
    assert_eq!(props.color, [7, 19, 31]);
    game.registry()
        .world
        .get::<&mut StudioAnim>(actor)
        .unwrap()
        .cycle = f32::NAN;
    assert_eq!(game.capture_scene(EPOCH), Err(SceneError::NonFiniteState));
    assert_eq!(game.elapsed(), 0.0);
}

#[test]
fn automatic_debug_and_errors_redact_local_ids_resources_and_poses() {
    let (game, _) = game();
    let frame = game.capture_scene(EPOCH).unwrap();
    let mut different = frame.studios()[0].clone();
    different.transform[12] = 123_456.75;
    different.animation.cycle = 123_456.75;
    different.identity =
        SceneIdentity::from_local_id(SceneEpoch::new(1_234_567_891), EntityId(1_234_567_891));
    assert_eq!(
        format!("{:?}", frame.studios()[0]),
        format!("{different:?}")
    );
    assert_eq!(
        format!("{EPOCH:?}"),
        format!("{:?}", different.identity.epoch())
    );
    assert_eq!(
        format!("{:?}", frame.studios()[0].identity),
        format!("{:?}", different.identity)
    );
    assert_eq!(
        format!("{frame:?}"),
        format!("{:?}", restamp(&frame, 1_234_567_891, 123_456.75))
    );
    let replica = accepted(&frame);
    for text in [
        format!("{replica:?}"),
        format!("{:?}", frame.catalog()),
        format!("{:?}", frame.brushes()[0]),
        format!("{:?}", frame.brushes()[0].model),
        format!("{:?}", different.model),
        format!("{:?}", different.animation),
        format!(
            "{:?}",
            SceneGait {
                sequence: Some(1_234_567_891),
                cycle: 123_456.75,
                frame_rate: 1.0
            }
        ),
        format!(
            "{:?}",
            HeldModelRecord {
                model: different.model,
                pose: different.pose,
                body: vec![1_234_567_891],
                skin: 1_234_567_891
            }
        ),
    ] {
        assert!(!text.contains("123456"));
        assert!(!text.contains("local_index"));
        assert!(!text.contains("matrices"));
    }
}

fn render_pixels(
    game: &mut Game,
    context: &GpuContext,
    target: &OffscreenTarget,
    replica: Option<&SceneReplica>,
    camera: &FreeFlyCamera,
) -> Vec<u8> {
    let draw = RenderTarget {
        view: target.view(),
        width: 192,
        height: 144,
        format: OFFSCREEN_FORMAT,
    };
    if let Some(replica) = replica {
        game.render_scene(context, draw, replica, camera).unwrap();
    } else {
        game.render(context, draw).unwrap();
    }
    target.read_rgba(context).unwrap()
}

#[test]
fn accepted_scene_frames_reach_pixels_without_ticks_and_reuse_resources_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let context = GpuContext::headless().expect("explicit opt-in requires a graphics adapter");
    let (mut game, _) = game();
    game.set_viewpoint([0.0, -150.0, 50.0], 0.0, 90.0);
    let camera = *game.camera();
    let frame = game.capture_scene(EPOCH).unwrap();
    let authority = game.save_bytes(42).unwrap();
    let mut replica = accepted(&frame);
    let target = OffscreenTarget::new(&context, 192, 144).unwrap();
    let ordinary = render_pixels(&mut game, &context, &target, None, &camera);
    let prepared = game.render_resource_stats();
    assert_eq!(
        render_pixels(&mut game, &context, &target, Some(&replica), &camera),
        ordinary
    );
    let mut fallbacks = frame.studios().to_vec();
    for record in &mut fallbacks {
        record.body = vec![u32::MAX];
        record.skin = usize::MAX;
    }
    replica
        .apply(records(&frame, frame.brushes().to_vec(), fallbacks))
        .unwrap();
    assert_eq!(
        render_pixels(&mut game, &context, &target, Some(&replica), &camera),
        ordinary
    );
    replica
        .apply(SceneFrame::new(
            frame.catalog().clone(),
            EPOCH,
            2,
            0.0,
            Vec::new(),
            Vec::new(),
        ))
        .unwrap();
    assert_ne!(
        render_pixels(&mut game, &context, &target, Some(&replica), &camera),
        ordinary
    );
    for _ in 0..3 {
        let _ = render_pixels(&mut game, &context, &target, Some(&replica), &camera);
        assert_eq!(game.render_resource_stats(), prepared);
    }
    assert_eq!(game.save_bytes(42).unwrap(), authority);
    assert_eq!(game.capture_scene(EPOCH).unwrap(), frame);
    let (mut other, _) = self::game();
    let target = RenderTarget {
        view: target.view(),
        width: 192,
        height: 144,
        format: OFFSCREEN_FORMAT,
    };
    assert_eq!(
        other.render_scene(&context, target, &replica, &camera),
        Err(SceneError::WrongCatalog)
    );
    assert_eq!(
        other.render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
}
