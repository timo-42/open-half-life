//! Project-authored optional follow continuation; every scene and byte is generated.
use ohl_ai::follow::{FollowPhase, Follower};
use ohl_ai::{Actor, Conditions, MonsterAi, Vec3};
use ohl_engine::save::{
    FollowAttemptSnapshot, FollowMemberSnapshot, FollowNavigationSnapshot,
    SECTION_FOLLOW_NAVIGATION,
};
use ohl_engine::test_support::{
    SCRIPT_MAP, ai_room_bsp, entity_block, entity_of_classname, script_room_entities, use_input,
};
use ohl_engine::{Game, GameSave, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::hecs::Entity;
use ohl_physics::{DIST_EPSILON, Hull};

fn scene(wall: bool, extras: &str) -> (Game, MemoryAssets) {
    let entities = script_room_entities([-128.0, 0.0, 36.0 + DIST_EPSILON], extras);
    let bytes = ai_room_bsp(&entities, wall);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());
    (
        Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("room"),
        assets,
    )
}

fn scientist() -> String {
    entity_block("monster_scientist", [-96.0, 0.0, 0.0], 180.0, &[])
}
fn state(game: &Game, entity: Entity) -> MonsterAi {
    (*game.registry().world.get::<&MonsterAi>(entity).expect("AI")).clone()
}
fn position(game: &Game, entity: Entity) -> Vec3 {
    game.registry()
        .world
        .get::<&Actor>(entity)
        .expect("actor")
        .origin
}
fn place(game: &mut Game, x: f32, y: f32) {
    let center = Vec3::new(x, y, 36.0 + DIST_EPSILON);
    let offset = Vec3::from_array(game.eye_position()) - Vec3::from_array(game.player_origin());
    game.set_viewpoint((center + offset).to_array(), 0.0, 0.0);
    assert!(Vec3::from_array(game.player_origin()).abs_diff_eq(center, DIST_EPSILON));
    let trace = game
        .collision()
        .expect("collision")
        .trace(Hull::Standing, center, center);
    assert!(!trace.start_solid && !trace.all_solid);
}
fn use_once(game: &mut Game) {
    game.tick(TICK_SECONDS, &Input::default());
    game.tick(TICK_SECONDS, &use_input());
    game.tick(TICK_SECONDS, &Input::default());
}

#[test]
fn follow_intent_save_preserves_entered_failure_and_later_real_retarget() {
    let (mut game, assets) = scene(true, &scientist());
    let id = entity_of_classname(&game, "monster_scientist").expect("scientist");
    use_once(&mut game);
    assert_eq!(game.followers(), &[id]);
    place(&mut game, 160.0, 0.0);
    let mut consecutive = 0;
    for _ in 0..600 {
        game.tick(TICK_SECONDS, &Input::default());
        let ai = state(&game, id);
        if ai.runner.task_index() == 0
            && !ai.runner.started()
            && ai.conditions.contains(Conditions::BLOCKED)
            && ai.stuck.ticks() >= ohl_ai::movement::STUCK_TICKS
            && !ai.route.is_finished()
        {
            consecutive += 1;
            if consecutive > 25 {
                break;
            }
        } else {
            consecutive = 0;
        }
    }
    assert!(consecutive > 25);
    let accepted = state(&game, id).follow_attempt.expect("accepted");
    assert_eq!(accepted.phase, FollowPhase::Moving);
    let save = game.to_save(0);
    let bytes = save.to_bytes().expect("save");
    let decoded = GameSave::from_bytes(&bytes).expect("decode");
    assert_eq!(decoded.follow_navigation, save.follow_navigation);
    let mut loaded = Game::from_save(&assets, &decoded).expect("continuation");
    let restored = entity_of_classname(&loaded, "monster_scientist").expect("restored");
    assert_eq!(loaded.followers(), &[restored]);
    assert_eq!(state(&loaded, restored).follow_attempt, Some(accepted));
    assert_eq!(state(&loaded, restored).route, state(&game, id).route);
    assert_eq!(state(&loaded, restored).stuck, state(&game, id).stuck);
    for _ in 0..30 {
        game.tick(TICK_SECONDS, &Input::default());
        loaded.tick(TICK_SECONDS, &Input::default());
        assert_eq!(state(&loaded, restored).follow_attempt, Some(accepted));
        assert_eq!(state(&loaded, restored).stuck, state(&game, id).stuck);
        assert_eq!(loaded.ai_state_hash(), game.ai_state_hash());
    }
    let before = position(&game, id);
    for instance in [&mut game, &mut loaded] {
        place(instance, -160.0, 160.0);
        instance.tick(TICK_SECONDS, &Input::default());
    }
    let admitted = state(&game, id);
    assert_eq!(
        admitted.follow_attempt.expect("new").phase,
        FollowPhase::Preparing
    );
    assert_ne!(admitted.follow_attempt, Some(accepted));
    assert_eq!(admitted.runner.task_index(), 0);
    assert!(!admitted.runner.started() && admitted.route.waypoints.is_empty());
    assert_eq!(admitted.stuck.ticks(), 0);
    assert!(!admitted.conditions.contains(Conditions::BLOCKED));
    assert_eq!(position(&game, id), before);
    for _ in 0..158 {
        game.tick(TICK_SECONDS, &Input::default());
        loaded.tick(TICK_SECONDS, &Input::default());
        assert_eq!(
            state(&loaded, restored).follow_attempt,
            state(&game, id).follow_attempt
        );
        assert_eq!(
            state(&loaded, restored).runner.task_index(),
            state(&game, id).runner.task_index()
        );
        assert_eq!(state(&loaded, restored).stuck, state(&game, id).stuck);
        assert!(position(&loaded, restored).abs_diff_eq(position(&game, id), DIST_EPSILON));
    }
    assert!((position(&game, id) - before).length() > 32.0);
}

fn replace_follow_payload(bytes: &[u8], payload: Option<&[u8]>) -> Vec<u8> {
    let reader = ohl_save::SaveReader::open(bytes, &ohl_save::Limits::default()).expect("reader");
    let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
    for entry in reader.sections() {
        if entry.tag != SECTION_FOLLOW_NAVIGATION {
            writer
                .add_section(entry.tag, reader.section(entry.tag).expect("section"))
                .expect("add");
        }
    }
    if let Some(payload) = payload {
        writer
            .add_section(SECTION_FOLLOW_NAVIGATION, payload)
            .expect("follow");
    }
    writer
        .finish(&ohl_save::Limits::default())
        .expect("container")
}

// Keep the wire golden, legacy absence and malformed-continuation refusals together.
#[allow(clippy::too_many_lines)]
#[test]
fn follow_intent_section_gold_absence_and_malformed_state_fail_closed() {
    const GOLD: &[u8] = &[1, 1, 3, 1, 0, 0, 128, 63, 0, 0, 0, 64, 0, 0, 64, 64, 1];
    let golden = FollowNavigationSnapshot {
        version: 1,
        members: vec![FollowMemberSnapshot {
            spawn_index: 3,
            attempt: Some(FollowAttemptSnapshot {
                accepted_player_anchor: [1.0, 2.0, 3.0],
                phase: 1,
            }),
        }],
    };
    assert_eq!(postcard::to_allocvec(&golden).expect("encode"), GOLD);
    assert_eq!(
        postcard::from_bytes::<FollowNavigationSnapshot>(GOLD).expect("gold"),
        golden
    );
    let (mut game, assets) = scene(false, &scientist());
    use_once(&mut game);
    let save = game.to_save(0);
    let bytes = save.to_bytes().expect("save");
    let absent = replace_follow_payload(&bytes, None);
    let loaded = Game::load_bytes(&assets, &absent).expect("old section absent");
    assert!(loaded.followers().is_empty());
    let old_reader =
        ohl_save::SaveReader::open(&absent, &ohl_save::Limits::default()).expect("old");
    let new_reader = ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default()).expect("new");
    for entry in old_reader.sections() {
        assert_eq!(
            old_reader.section(entry.tag).expect("old payload"),
            new_reader.section(entry.tag).expect("unchanged payload")
        );
    }
    // The optional continuation rejects corrupt raw tag-25 cursor/timer fields,
    // even though the legacy loader alone would sanitize those fields.
    let member = save.follow_navigation.as_ref().expect("present").members[0].spawn_index;
    for (cursor, timer) in [
        (
            u32::try_from(ohl_ai::monsters::brains::FOLLOW_PLAYER.tasks.len())
                .expect("bounded FOLLOW task count"),
            0.0,
        ),
        (u32::MAX, 0.0),
        (0, f32::NAN),
        (0, f32::INFINITY),
        (0, -1.0),
    ] {
        let mut bad = save.clone();
        let ai = bad.ai.as_mut().expect("AI")[member as usize]
            .as_mut()
            .expect("member AI");
        ai.task_index = cursor;
        ai.schedule_timer = timer;
        assert!(Game::from_save(&assets, &bad).is_err());
    }
    let valid = save.follow_navigation.clone().expect("present");
    let mut malformed = vec![];
    let mut bad = valid.clone();
    bad.version = 2;
    malformed.push(bad);
    let mut bad = valid.clone();
    bad.members.push(bad.members[0].clone());
    malformed.push(bad);
    let mut bad = valid.clone();
    bad.members[0].spawn_index = u32::MAX;
    malformed.push(bad);
    for (phase, anchor) in [
        (255, [0.0; 3]),
        (0, [f32::NAN, 0.0, 0.0]),
        (0, [f32::MAX; 3]),
    ] {
        let mut bad = valid.clone();
        bad.members[0].attempt = Some(FollowAttemptSnapshot {
            phase,
            accepted_player_anchor: anchor,
        });
        malformed.push(bad);
    }
    for state in malformed {
        let payload = postcard::to_allocvec(&state).expect("authored malformed");
        assert!(
            Game::load_bytes(&assets, &replace_follow_payload(&bytes, Some(&payload))).is_err()
        );
    }
    let mut trailing = postcard::to_allocvec(&valid).expect("valid");
    trailing.push(0);
    for payload in [
        &[][..],
        &[1, 3][..],
        &trailing[..],
        &[1, 255, 255, 255, 255, 15][..],
    ] {
        assert!(GameSave::from_bytes(&replace_follow_payload(&bytes, Some(payload))).is_err());
    }
    // An otherwise valid later member cannot make a contradictory attempt acceptable.
    let mut contradictory = save.clone();
    contradictory
        .follow_navigation
        .as_mut()
        .expect("present")
        .members[0]
        .attempt = Some(FollowAttemptSnapshot {
        accepted_player_anchor: [0.0; 3],
        phase: 1,
    });
    assert!(Game::from_save(&assets, &contradictory).is_err());
}

#[test]
fn follow_intent_saved_roster_order_controls_next_eviction_and_stop() {
    let extra = [0.0, 64.0, 128.0]
        .into_iter()
        .map(|y| entity_block("monster_scientist", [-96.0, y, 0.0], 180.0, &[]))
        .collect::<String>();
    let (mut game, assets) = scene(false, &extra);
    let mut ids: Vec<_> = game
        .registry()
        .world
        .query::<(Entity, &Follower)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    ids.sort_by_key(|e| e.id());
    assert_eq!(ids.len(), 3);
    place(&mut game, -128.0, 64.0);
    use_once(&mut game);
    place(&mut game, -128.0, 0.0);
    use_once(&mut game);
    assert_eq!(game.followers(), &[ids[1], ids[0]]);
    let save = game.to_save(0);
    let mut loaded = Game::from_save(&assets, &save).expect("ordered restore");
    assert_eq!(loaded.followers(), &[ids[1], ids[0]]);
    place(&mut loaded, -128.0, 128.0);
    use_once(&mut loaded);
    assert_eq!(loaded.followers(), &[ids[0], ids[2]]);
    use_once(&mut loaded);
    assert_eq!(loaded.followers(), &[ids[0]]);
    assert!(state(&loaded, ids[2]).follow_attempt.is_none());
    let mut reversed = save.clone();
    reversed
        .follow_navigation
        .as_mut()
        .expect("roster")
        .members
        .reverse();
    let differently_ordered = Game::from_save(&assets, &reversed).expect("other valid order");
    let original_order = Game::from_save(&assets, &save).expect("original valid order");
    assert_ne!(
        differently_ordered.ai_state_hash(),
        original_order.ai_state_hash()
    );
}

#[test]
fn follow_intent_each_live_phase_continues_and_posture_keeps_the_raw_goal() {
    let (mut game, assets) = scene(false, &scientist());
    let id = entity_of_classname(&game, "monster_scientist").expect("scientist");
    use_once(&mut game);
    let holding = state(&game, id).follow_attempt.expect("holding");
    assert_eq!(holding.phase, FollowPhase::Holding);
    let player_anchor = |g: &Game| {
        g.registry()
            .world
            .query::<&Actor>()
            .iter()
            .find(|actor| actor.is_client)
            .expect("player Actor")
            .navigation_anchor()
    };
    let old_anchor = player_anchor(&game);
    let old_eye = game.eye_position();
    for _ in 0..10 {
        game.tick(
            TICK_SECONDS,
            &Input {
                duck: true,
                ..Input::default()
            },
        );
        assert_eq!(state(&game, id).follow_attempt, Some(holding));
        assert!(player_anchor(&game).abs_diff_eq(old_anchor, DIST_EPSILON));
    }
    assert!(
        old_eye[2] - game.eye_position()[2] > DIST_EPSILON,
        "ordinary posture actually changed the eye"
    );
    for _ in 0..10 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert_eq!(state(&game, id).follow_attempt, Some(holding));
    assert!(player_anchor(&game).abs_diff_eq(old_anchor, DIST_EPSILON));
    place(&mut game, 160.0, 0.0);
    let mut phases = [false; 4];
    for _ in 0..650 {
        game.tick(TICK_SECONDS, &Input::default());
        let before = state(&game, id);
        let phase = before.follow_attempt.expect("active").phase as usize;
        if !phases[phase] {
            let save = game.to_save(0);
            let mut loaded = Game::from_save(&assets, &save).expect("phase restore");
            let restored = entity_of_classname(&loaded, "monster_scientist").expect("restored");
            assert_eq!(
                loaded.to_save(0).ai,
                save.ai,
                "raw AI continuation at phase {phase}"
            );
            assert_eq!(
                loaded.to_save(0).follow_navigation,
                save.follow_navigation,
                "accepted history and ordered roster at phase {phase}"
            );
            // Existing restore synchronizes the player Actor in the next ordinary
            // phase 4; accepted follow history and raw AI already match above.
            game.tick(TICK_SECONDS, &Input::default());
            loaded.tick(TICK_SECONDS, &Input::default());
            let live = state(&game, id);
            let reloaded = state(&loaded, restored);
            assert_eq!(
                loaded.ai_state_hash(),
                game.ai_state_hash(),
                "post-tick phase {phase}"
            );
            assert_eq!(reloaded.follow_attempt, live.follow_attempt);
            assert_eq!(reloaded.runner.task_index(), live.runner.task_index());
            assert_eq!(reloaded.runner.started(), live.runner.started());
            assert_eq!(reloaded.stuck, live.stuck);
            assert!(position(&loaded, restored).abs_diff_eq(position(&game, id), DIST_EPSILON));
            phases[phase] = true;
        }
        if phases.iter().all(|seen| *seen) {
            break;
        }
    }
    assert!(
        phases.iter().all(|seen| *seen),
        "all four phases occurred through ordinary input"
    );
}
