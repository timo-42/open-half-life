//! End-to-end authored-anchor tests over generated project fixtures.

use crate::Owner;
use glam::Vec3;
use ohl_ai::{Actor, BodyFrame, MonsterAi, MonsterKind};
use ohl_combat::{HitboxIndex, HitboxLimits};
use ohl_game::registry::{ClassName, Transform};

use crate::ai::AiState;
use crate::components::StudioAnim;
use crate::level::Level;
use crate::test_support::{AI_MAP, ai_room_bsp, entity_block};
use crate::{Game, Input, MemoryAssets, TICK_SECONDS};

fn model(eye: [f32; 3], bounds: ([f32; 3], [f32; 3]), posed: bool) -> Vec<u8> {
    let (mut bytes, _) = if posed {
        ohl_formats::test_support::build_minimal_mdl10_with_hitbox(bounds.0, bounds.1)
    } else {
        ohl_formats::test_support::build_minimal_mdl10()
    };
    for (base, values) in [(76, eye), (112, bounds.0), (124, bounds.1)] {
        for (axis, value) in values.into_iter().enumerate() {
            bytes[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn assets(extra: &str, mdl: Option<Vec<u8>>) -> (MemoryAssets, Vec<u8>) {
    let entities = format!(
        "{{\"classname\" \"worldspawn\"}}\n{{\"classname\" \"info_player_start\" \"origin\" \"-200 -200 36\"}}\n{extra}"
    );
    let bytes = ai_room_bsp(&entities, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    if let Some(mdl) = mdl {
        assets.insert(
            MonsterKind::Barney
                .default_model_path()
                .expect("published default"),
            mdl,
        );
    }
    (assets, bytes)
}

fn level(extra: &str, mdl: Option<Vec<u8>>) -> Level {
    let (assets, bytes) = assets(extra, mdl);
    let mut level = Level::from_bytes(&assets, AI_MAP, &bytes).expect("synthetic room");
    AiState::new(5).attach_level(
        &mut level,
        ohl_campaign::Difficulty::Easy,
        &ohl_campaign::SkillTable::default(),
    );
    level
}

#[test]
fn posed_and_clipping_boxes_share_the_render_anchor_while_only_degenerate_boxes_use_the_proxy() {
    let bounds = ([-6.0, -10.0, 3.0], [8.0, 12.0, 40.0]);
    let block = entity_block("monster_barney", [80.0, 20.0, 0.0], 90.0, &[]);
    for posed in [false, true] {
        let level = level(&block, Some(model([5.0, 1.0, 32.0], bounds, posed)));
        let entity = level
            .registry
            .world
            .query::<(ohl_game::hecs::Entity, &MonsterAi)>()
            .iter()
            .next()
            .expect("monster")
            .0;
        let actor = level.registry.world.get::<&Actor>(entity).expect("actor");
        let transform = level
            .registry
            .world
            .get::<&Transform>(entity)
            .expect("transform");
        assert_eq!(actor.origin, transform.origin);
        assert_eq!(actor.origin.z, 0.0);
        assert!(actor.eye().abs_diff_eq(Vec3::new(79.0, 25.0, 32.0), 0.001));
        let placement = ohl_render::placement(transform.origin.to_array(), transform.angles.y);
        assert_eq!(
            [placement[12], placement[13], placement[14]],
            actor.origin.to_array()
        );
        let mut index = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut index, &level);
        let entry = index
            .entries()
            .iter()
            .find(|entry| entry.id == crate::ids::entity_id(entity))
            .expect("hitbox");
        assert_eq!(entry.origin, actor.origin);
        // The generated model's first animated root translation is +10 X;
        // a clipping fallback has no bone pose. Neither receives body D.
        let animation = if posed { Vec3::X * 10.0 } else { Vec3::ZERO };
        assert_eq!(entry.boxes[0].min, Vec3::from_array(bounds.0) + animation);
        assert_eq!(entry.boxes[0].max, Vec3::from_array(bounds.1) + animation);
    }
    for mdl in [None, Some(model([0.0; 3], ([0.0; 3], [0.0; 3]), false))] {
        let level = level(&block, mdl);
        let entity = level
            .registry
            .world
            .query::<(ohl_game::hecs::Entity, &MonsterAi)>()
            .iter()
            .next()
            .expect("monster")
            .0;
        let mut index = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut index, &level);
        let entry = index
            .entries()
            .iter()
            .find(|entry| entry.id == crate::ids::entity_id(entity))
            .expect("model-less/degenerate actor is shootable");
        assert_eq!(entry.boxes[0].min, Vec3::new(-16.0, -16.0, 0.0));
        assert_eq!(entry.boxes[0].max, Vec3::new(16.0, 16.0, 72.0));
    }
}

#[test]
fn map_maker_and_restored_children_reconstruct_the_same_model_policy() {
    let direct = entity_block(
        "monster_barney",
        [128.0, 64.0, 0.0],
        90.0,
        &[("spawnflags", "16")],
    );
    let maker = entity_block(
        "monstermaker",
        [-128.0, 64.0, 0.0],
        90.0,
        &[
            ("monstertype", "monster_barney"),
            ("monstercount", "1"),
            ("delay", "0.1"),
            ("spawnflags", "1"),
        ],
    );
    for include_map_actor in [true, false] {
        let definitions = if include_map_actor {
            format!("{direct}{maker}")
        } else {
            maker.clone()
        };
        let (assets, bytes) = assets(
            &definitions,
            Some(model(
                [5.0, 1.0, 50.0],
                ([-8.0, -8.0, 0.0], [8.0, 8.0, 60.0]),
                false,
            )),
        );
        let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("map");
        for _ in 0..3 {
            game.tick(TICK_SECONDS, &Input::default());
        }
        let policies = |game: &Game| {
            let mut values: Vec<_> = game
                .registry()
                .world
                .query::<(
                    ohl_game::hecs::Entity,
                    &Actor,
                    &ClassName,
                    &Transform,
                    &StudioAnim,
                    Option<&Owner>,
                )>()
                .iter()
                .filter(|(_, _, classname, _, _, _)| classname.0 == "monster_barney")
                .map(|(_, actor, _, transform, _, owner)| {
                    assert_eq!(actor.origin, transform.origin);
                    assert_eq!(actor.body_frame, BodyFrame::Feet);
                    assert_eq!(actor.view_ofs, Vec3::new(5.0, 1.0, 50.0));
                    (
                        owner.is_some(),
                        actor.origin,
                        actor.body_frame,
                        actor.view_ofs,
                    )
                })
                .collect();
            values.sort_by_key(|entry| entry.0);
            values
        };
        let expected = policies(&game);
        assert_eq!(
            expected.len(),
            if include_map_actor { 2 } else { 1 },
            "maker-only maps also preload the child model"
        );
        assert!(expected.iter().all(|entry| entry.1.z == 0.0));
        let saved = game.save_bytes(1).expect("save");
        let restored = Game::load_bytes(&assets, &saved).expect("restore");
        assert_eq!(policies(&restored), expected);
    }
}

#[test]
fn ducking_player_actor_keeps_controller_center_and_current_floor_goal() {
    let (assets, bytes) = assets(
        &entity_block("monster_barney", [-160.0, -200.0, 0.0], 180.0, &[]),
        None,
    );
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("map");
    game.tick(TICK_SECONDS, &crate::test_support::use_input());
    // Phase 12 queues use; following consumes it in the next AI phase.
    game.tick(TICK_SECONDS, &Input::default());
    let follower =
        crate::test_support::entity_of_classname(&game, "monster_barney").expect("follower");
    assert_eq!(game.followers(), &[follower]);
    for duck in [false, true, false] {
        for _ in 0..50 {
            game.tick(
                TICK_SECONDS,
                &Input {
                    duck,
                    ..Input::default()
                },
            );
        }
        let actor = game
            .registry()
            .world
            .get::<&Actor>(game.player_entity())
            .expect("player actor");
        assert_eq!(
            actor.hull,
            if duck {
                ohl_physics::Hull::Crouched
            } else {
                ohl_physics::Hull::Standing
            }
        );
        assert_eq!(actor.origin.to_array(), game.player_origin());
        let following = game
            .registry()
            .world
            .get::<&MonsterAi>(follower)
            .expect("follower AI");
        assert_eq!(following.move_target, Some(actor.navigation_anchor()));
        assert!(following.move_target.expect("floor goal").z.abs() < 0.1);
        assert!(
            actor.navigation_anchor().z.abs() < 0.1,
            "stance does not raise the pursuit goal"
        );
    }
}

#[test]
fn restored_visible_pursuit_replans_a_legacy_center_goal_without_shifting_actor_storage() {
    let entities = format!(
        "{}{}",
        entity_block("monster_zombie", [100.0, 0.0, 0.0], 180.0, &[]),
        entity_block("monster_barney", [-100.0, 0.0, 0.0], 0.0, &[])
    );
    let (assets, bytes) = assets(&entities, None);
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("map");
    let entity = crate::test_support::entity_of_classname(&game, "monster_zombie").expect("actor");
    let target = crate::test_support::entity_of_classname(&game, "monster_barney").expect("target");
    let anchor = Vec3::new(100.0, 0.0, 0.0);
    {
        let mut ai = game
            .registry()
            .world
            .get::<&mut MonsterAi>(entity)
            .expect("AI");
        let legacy_goal = Vec3::new(-100.0, 0.0, 36.0);
        ai.state = ohl_ai::MonsterState::Combat;
        ai.memory = Some(ohl_ai::EnemyMemory {
            entity: target,
            last_known_position: legacy_goal,
            time_since_seen: 0.0,
            occluded: false,
            last_known_distance: 200.0,
        });
        ai.route = ohl_ai::Route::straight_line(legacy_goal);
        ai.move_target = Some(legacy_goal);
        ai.move_speed = 40.0;
        ai.runner = ohl_ai::ScheduleRunner::restore(ohl_ai::brain::CHASE_ENEMY.name, 3, true, 0.0);
    }
    let saved = game.save_bytes(1).expect("legacy-coordinate save");
    let mut loaded = Game::load_bytes(&assets, &saved).expect("load");
    let entity =
        crate::test_support::entity_of_classname(&loaded, "monster_zombie").expect("actor");
    assert_eq!(
        loaded
            .registry()
            .world
            .get::<&Actor>(entity)
            .expect("actor")
            .origin,
        anchor
    );
    for _ in 0..3 {
        loaded.tick(TICK_SECONDS, &Input::default());
    }
    let ai = loaded
        .registry()
        .world
        .get::<&MonsterAi>(entity)
        .expect("AI");
    assert!(!ai.route.is_finished());
    assert!(
        ai.route.goal.z.abs() < 0.1,
        "identified live pursuit replans to the authored anchor"
    );
    let actor = loaded
        .registry()
        .world
        .get::<&Actor>(entity)
        .expect("actor");
    let transform = loaded
        .registry()
        .world
        .get::<&Transform>(entity)
        .expect("transform");
    assert_eq!(actor.origin, transform.origin);
    assert!(actor.origin.z.abs() < 0.1);
}

#[test]
fn a_toggled_wall_waits_for_a_feet_actors_upper_body_then_solidifies() {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    let text = "{\"classname\" \"worldspawn\"}\n{\"classname\" \"info_player_start\" \"origin\" \"-200 0 36\"}\n{\"classname\" \"monster_barney\" \"origin\" \"0 0 0\" \"spawnflags\" \"16\"}\n{\"classname\" \"func_wall_toggle\" \"model\" \"*1\" \"targetname\" \"ohl_block\" \"spawnflags\" \"1\"}\n{\"classname\" \"trigger_auto\" \"target\" \"ohl_block\"}";
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(text);
    let heads = builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
    builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], heads, 2, 0, 0);
    let min = [-8.0, -8.0, 48.0];
    let max = [8.0, 8.0, 64.0];
    let heads = builder.push_collision_hulls(&[CollisionBrush::box_brush(min, max)]);
    builder.push_model(min, max, [0.0; 3], heads, 2, 0, 0);
    let mut game =
        Game::from_map_bytes(&MemoryAssets::new(), AI_MAP, &builder.build()).expect("map");
    for _ in 0..5 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    let probe = Vec3::new(0.0, 0.0, 56.0);
    let solid = |game: &Game| {
        game.monster_collision()
            .expect("collision")
            .trace(ohl_physics::Hull::Point, probe, probe)
            .start_solid
    };
    assert!(!solid(&game), "upper-body occupant suspends the wall");
    let entity = crate::test_support::entity_of_classname(&game, "monster_barney").expect("actor");
    game.registry()
        .world
        .get::<&mut Actor>(entity)
        .expect("actor")
        .origin
        .x = 100.0;
    game.registry()
        .world
        .get::<&mut Transform>(entity)
        .expect("transform")
        .origin
        .x = 100.0;
    for _ in 0..3 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert!(solid(&game), "vacated wall becomes solid");
}

#[test]
fn a_fresh_save_preserves_unseen_enemy_memory_and_historical_pursuit() {
    // Inside the existing 256-unit occluded-memory range, on opposite sides
    // of the generated wall. The historical goal differs from the player.
    let entities = "{\"classname\" \"worldspawn\"}\n{\"classname\" \"info_player_start\" \"origin\" \"-200 -200 36\"}\n{\"classname\" \"monster_zombie\" \"origin\" \"100 0 0\" \"angle\" \"180\"}\n{\"classname\" \"monster_barney\" \"origin\" \"-100 0 0\" \"spawnflags\" \"16\"}";
    let bytes = ai_room_bsp(entities, true);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    for schedule in [&ohl_ai::brain::HUNT_ENEMY, &ohl_ai::brain::CHASE_ENEMY] {
        let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("map");
        let entity = game
            .registry()
            .world
            .query::<(ohl_game::hecs::Entity, &ClassName)>()
            .iter()
            .find(|(_, classname)| classname.0 == "monster_zombie")
            .expect("monster")
            .0;
        let target = game
            .registry()
            .world
            .query::<(ohl_game::hecs::Entity, &ClassName)>()
            .iter()
            .find(|(_, classname)| classname.0 == "monster_barney")
            .expect("registry-indexed target")
            .0;
        let remembered = Vec3::new(150.0, 100.0, 0.0);
        {
            let mut ai = game
                .registry()
                .world
                .get::<&mut MonsterAi>(entity)
                .expect("AI");
            ai.state = ohl_ai::MonsterState::Combat;
            ai.memory = Some(ohl_ai::EnemyMemory {
                entity: target,
                last_known_position: remembered,
                time_since_seen: 0.5,
                occluded: true,
                last_known_distance: 100.0,
            });
            ai.route = ohl_ai::Route::straight_line(remembered);
            ai.move_target = Some(remembered);
            ai.move_speed = 50.0;
            ai.runner = ohl_ai::ScheduleRunner::restore(schedule.name, 3, true, 0.0);
        }
        let bytes = game.save_bytes(1).expect("fresh save");
        let mut restored = Game::load_bytes(&assets, &bytes).expect("restore");
        game.tick(TICK_SECONDS, &Input::default());
        restored.tick(TICK_SECONDS, &Input::default());
        let restored_entity = restored
            .registry()
            .world
            .query::<(ohl_game::hecs::Entity, &ClassName)>()
            .iter()
            .find(|(_, classname)| classname.0 == "monster_zombie")
            .expect("restored monster")
            .0;
        let original = game
            .registry()
            .world
            .get::<&MonsterAi>(entity)
            .expect("original AI");
        let loaded = restored
            .registry()
            .world
            .get::<&MonsterAi>(restored_entity)
            .expect("loaded AI");
        assert_eq!(
            loaded
                .memory
                .expect("historical memory")
                .last_known_position,
            remembered,
            "save reconstruction must not disclose the unseen target's live location"
        );
        assert_eq!(
            loaded.route, original.route,
            "fresh save continues the same route"
        );
        assert_eq!(loaded.runner.task_index(), original.runner.task_index());
        assert_eq!(
            restored
                .registry()
                .world
                .get::<&Actor>(restored_entity)
                .expect("loaded actor")
                .origin,
            game.registry()
                .world
                .get::<&Actor>(entity)
                .expect("original actor")
                .origin
        );
    }
}
