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
        assert_eq!(actor.origin.z.to_bits(), 0.0_f32.to_bits());
        assert!(actor.eye().abs_diff_eq(Vec3::new(79.0, 25.0, 32.0), 0.001));
        let placement = ohl_render::placement(transform.origin.to_array(), transform.angles.y);
        assert_eq!(
            [placement[12], placement[13], placement[14]].map(f32::to_bits),
            actor.origin.to_array().map(f32::to_bits)
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
        [128.0, 64.0, 48.0],
        90.0,
        &[("spawnflags", "16")],
    );
    let maker = entity_block(
        "monstermaker",
        [-128.0, 64.0, 48.0],
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
        assert!(
            expected.iter().all(|entry| entry.1.z.abs() < 0.05),
            "map walker and maker child both landed once"
        );
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
        assert_eq!(
            actor.origin.to_array().map(f32::to_bits),
            game.player_origin().map(f32::to_bits)
        );
        let following = game
            .registry()
            .world
            .get::<&MonsterAi>(follower)
            .expect("follower AI");
        let owner = game
            .registry()
            .world
            .get::<&Actor>(follower)
            .expect("follower actor");
        assert!(
            following.runner.schedule().is_some_and(|schedule| {
                std::ptr::eq(schedule, &raw const ohl_ai::monsters::brains::FOLLOW_PLAYER)
            }),
            "ordinary follow schedule was selected"
        );
        let attempt = following.follow_attempt.expect("ordinary follow admission");
        assert_eq!(attempt.phase, ohl_ai::follow::FollowPhase::Holding);
        // Follow owns a raw anchor separately from generic sound/movement targets.
        assert_eq!(attempt.accepted_player_anchor, actor.navigation_anchor());
        let goal = owner
            .body_frame
            .anchor_to_query(owner.hull, attempt.accepted_player_anchor);
        assert!((goal.z - owner.query_origin().z).abs() < 0.1);
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
        (ai.route.goal.z - 36.0).abs() < 0.1,
        "identified live pursuit replans to the owner query goal"
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
    for _ in 0..6 {
        assert!(
            !solid(&game),
            "upper-body occupant suspends the monster wall"
        );
        assert!(
            game.position_is_in_solid(probe.to_array()),
            "upper-body NPC occupancy preserves the unrelated player wall"
        );
        game.tick(TICK_SECONDS, &Input::default());
    }
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

// Every coordinate below is project-authored. The eye is the current
// project muzzle/aim policy, not a claim about every model's anatomy.
fn eye_projectile_model(eye: f32, target: bool) -> Vec<u8> {
    let (min, max) = if target {
        ([-12.0, -12.0, 48.0], [12.0, 12.0, 96.0])
    } else {
        ([-24.0, -24.0, 0.0], [24.0, 24.0, 128.0])
    };
    let (mut bytes, layout) = ohl_formats::test_support::build_minimal_mdl10_with_hitbox(min, max);
    // Freeze the generated root translation; eye changes must leave the
    // exact posed geometry unchanged throughout this physical flight.
    bytes[layout.anim_data_offset + 26..layout.anim_data_offset + 30].fill(0);
    for (base, values) in [
        (76, [0.0, 0.0, eye]),
        (112, [min[0], min[1], 0.0]),
        (124, max),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            bytes[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

#[derive(Debug)]
struct EyeFlight {
    acquired: bool,
    launched: usize,
    damage_events: u64,
    deaths: u64,
}

fn eye_projectile_fixture(source_eye: f32, target_eye: f32, wall_height: Option<f32>) -> Game {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    let text = format!(
        "{{\"classname\" \"worldspawn\"}}\n{{\"classname\" \"info_player_start\" \"origin\" \"-200 -200 36\"}}\n{}{}",
        entity_block("monster_bullchicken", [-96.0, 0.0, 0.0], 0.0, &[]),
        entity_block("monster_barney", [96.0, 0.0, 0.0], 180.0, &[]),
    );
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&text);
    let mut brushes = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -256.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -256.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -256.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -256.0),
    ];
    if let Some(height) = wall_height {
        brushes.push(CollisionBrush::box_brush(
            [-48.0, -256.0, 0.0],
            [-40.0, 256.0, height],
        ));
    }
    let heads = builder.push_collision_hulls(&brushes);
    builder.push_model([-256.0, -256.0, 0.0], [256.0; 3], [0.0; 3], heads, 2, 0, 0);
    let bytes = builder.build();
    let mut assets = MemoryAssets::new();
    for (kind, eye, target) in [
        (MonsterKind::Bullsquid, source_eye, false),
        (MonsterKind::Barney, target_eye, true),
    ] {
        assets.insert(
            kind.default_model_path().expect("default model"),
            eye_projectile_model(eye, target),
        );
    }
    let mut game =
        Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("project-authored flight room");
    assert_eq!(game.difficulty(), ohl_campaign::Difficulty::Medium);
    assert_eq!(
        game.systems_config().rng_seed,
        crate::systems::DEFAULT_RNG_SEED
    );
    let source =
        crate::test_support::entity_of_classname(&game, "monster_bullchicken").expect("source");
    let target = crate::test_support::entity_of_classname(&game, "monster_barney").expect("target");
    game.level_and_systems_mut()
        .0
        .registry
        .world
        .insert_one(target, ohl_ai::ScriptHold)
        .expect("stationary target");
    game.registry()
        .world
        .get::<&mut Actor>(target)
        .expect("target actor")
        .health = 5.0;
    game.registry()
        .world
        .get::<&mut Actor>(source)
        .expect("source actor")
        .health = 40.0;
    game
}

fn assert_eye_flight_geometry(
    game: &mut Game,
    source: ohl_game::hecs::Entity,
    target: ohl_game::hecs::Entity,
    expected_source_eye: Vec3,
    expected_target_eye: Vec3,
) {
    for (entity, anchor, eye, min, max) in [
        (
            source,
            Vec3::new(-96.0, 0.0, 0.0),
            expected_source_eye,
            Vec3::new(-24.0, -24.0, 0.0),
            Vec3::new(24.0, 24.0, 128.0),
        ),
        (
            target,
            Vec3::new(96.0, 0.0, 0.0),
            expected_target_eye,
            Vec3::new(-12.0, -12.0, 48.0),
            Vec3::new(12.0, 12.0, 96.0),
        ),
    ] {
        let actor = *game.registry().world.get::<&Actor>(entity).expect("actor");
        let transform = *game
            .registry()
            .world
            .get::<&Transform>(entity)
            .expect("transform");
        assert_eq!(actor.origin, anchor);
        assert_eq!(transform.origin, anchor);
        assert_eq!(actor.body_frame, BodyFrame::Feet);
        assert!(actor.eye().abs_diff_eq(eye, 0.001));
        let placement = ohl_render::placement(transform.origin.to_array(), transform.angles.y);
        assert_eq!(
            [placement[12], placement[13], placement[14]].map(f32::to_bits),
            anchor.to_array().map(f32::to_bits)
        );
        if actor.alive {
            let (_, systems) = game.level_and_systems_mut();
            let entry = systems
                .hitboxes()
                .entries()
                .iter()
                .find(|entry| entry.id == crate::ids::entity_id(entity))
                .expect("posed hitboxes");
            assert_eq!(entry.origin, anchor);
            assert_eq!(entry.boxes.len(), 1);
            assert_eq!(entry.boxes[0].min, min);
            assert_eq!(entry.boxes[0].max, max);
        }
    }
}

fn assert_eye_flight_projectiles(
    game: &Game,
    owner_ref: crate::save_state::ProjectileEntityRef,
    target_ref: crate::save_state::ProjectileEntityRef,
    expected_source_eye: Vec3,
    expected_target_eye: Vec3,
    launched: &mut std::collections::BTreeSet<u32>,
) {
    let save = game.to_save(0);
    let physical = save.projectiles.as_ref().expect("physical projectiles");
    let profiles = save
        .projectile_runtime
        .as_ref()
        .map_or(&[][..], |runtime| runtime.attacks.as_slice());
    assert!(
        physical.projectiles.len() <= 1,
        "one schedule emission, no duplicate projectile"
    );
    assert_eq!(physical.projectiles.len(), profiles.len());
    for projectile in &physical.projectiles {
        assert_eq!(projectile.kind_tag, 6, "unguided spit");
        let profile = profiles
            .iter()
            .find(|attack| attack.id == projectile.id)
            .expect("one matching profile");
        assert_eq!(profile.owner, Some(owner_ref));
        assert_eq!(profile.target, Some(target_ref));
        assert_eq!(profile.damage.to_bits(), 10.0_f32.to_bits());
        assert_eq!(profile.damage_bits, ohl_combat::DamageType::ACID.bits());
        assert_eq!(profile.blast_radius, None);
        if launched.insert(projectile.id) {
            assert_eq!(
                projectile.age.to_bits(),
                0.0_f32.to_bits(),
                "AI launch is swept on the next tick"
            );
            assert!(Vec3::from_array(projectile.position).abs_diff_eq(expected_source_eye, 0.001));
            let direction = (expected_target_eye - expected_source_eye).normalize();
            assert!(
                Vec3::from_array(projectile.velocity)
                    .normalize()
                    .abs_diff_eq(direction, 0.001)
            );
            assert_eq!(
                game.monster_damage_event_count(),
                0,
                "damage requires actual flight"
            );
        }
    }
}

fn eye_projectile_flight(source_eye: f32, target_eye: f32, wall_height: Option<f32>) -> EyeFlight {
    use crate::save_state::ProjectileEntityRef;
    let mut game = eye_projectile_fixture(source_eye, target_eye, wall_height);
    let source =
        crate::test_support::entity_of_classname(&game, "monster_bullchicken").expect("source");
    let target = crate::test_support::entity_of_classname(&game, "monster_barney").expect("target");
    let registry_ref = |entity| {
        ProjectileEntityRef::Registry(
            u32::try_from(
                game.registry()
                    .entities
                    .iter()
                    .position(|candidate| *candidate == entity)
                    .expect("registry actor"),
            )
            .expect("small fixture"),
        )
    };
    let owner_ref = registry_ref(source);
    let target_ref = registry_ref(target);
    let expected_eye = |eye: f32, height: f32| {
        if eye.is_finite() && eye != 0.0 {
            eye
        } else {
            height * (8.0 / 9.0)
        }
    };
    let expected_source_eye = Vec3::new(-96.0, 0.0, expected_eye(source_eye, 128.0));
    let expected_target_eye = Vec3::new(96.0, 0.0, expected_eye(target_eye, 96.0));
    let mut launched = std::collections::BTreeSet::new();
    let mut acquired = false;
    for _ in 0..60 {
        game.tick(TICK_SECONDS, &Input::default());
        acquired |= game
            .registry()
            .world
            .get::<&MonsterAi>(source)
            .expect("source AI")
            .enemy()
            == Some(target);
        assert_eye_flight_geometry(
            &mut game,
            source,
            target,
            expected_source_eye,
            expected_target_eye,
        );
        assert_eye_flight_projectiles(
            &game,
            owner_ref,
            target_ref,
            expected_source_eye,
            expected_target_eye,
            &mut launched,
        );
    }
    assert_eq!(
        game.projectile_count(),
        0,
        "hit or wall impact retires physical projectile"
    );
    assert!(
        game.to_save(0)
            .projectile_runtime
            .is_none_or(|runtime| runtime.attacks.is_empty())
    );
    assert_eq!(
        game.registry()
            .world
            .get::<&Actor>(source)
            .expect("shooter")
            .health
            .to_bits(),
        40.0_f32.to_bits(),
        "muzzle inside the posed owner never self-hits"
    );
    EyeFlight {
        acquired,
        launched: launched.len(),
        damage_events: game.monster_damage_event_count(),
        deaths: game.monster_death_count(),
    }
}

#[test]
fn model_eyes_separate_projectile_visibility_from_aim_and_keep_authored_geometry() {
    for source_model_eye in [false, true] {
        for target_model_eye in [false, true] {
            let source_eye = if source_model_eye { 96.0 } else { 28.0 };
            let target_eye = if target_model_eye { 64.0 } else { 28.0 };
            for wall in [None, Some(70.0), Some(128.0)] {
                let result = eye_projectile_flight(source_eye, target_eye, wall);
                let visible = wall.is_none() || (wall == Some(70.0) && source_model_eye);
                let hit = visible && target_model_eye;
                assert_eq!(
                    result.acquired, visible,
                    "source eye decides low-window acquisition: {result:?}"
                );
                assert_eq!(result.launched, usize::from(visible));
                assert_eq!(
                    result.damage_events,
                    u64::from(hit),
                    "target eye decides posed-box damage: {result:?}"
                );
                assert_eq!(result.deaths, u64::from(hit));
            }
        }
    }
    for missing_eye in [0.0, f32::NAN] {
        let result = eye_projectile_flight(missing_eye, missing_eye, Some(70.0));
        assert!(result.acquired);
        assert_eq!(result.launched, 1);
        assert_eq!(result.damage_events, 1);
        assert_eq!(result.deaths, 1);
    }
}

fn elevated_graph_script(custom_bottom: bool) -> (MemoryAssets, Game) {
    elevated_graph_script_mode(custom_bottom, "1", "0")
}

fn elevated_graph_script_mode(
    custom_bottom: bool,
    mode: &str,
    flags: &str,
) -> (MemoryAssets, Game) {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    let bottom = if custom_bottom { -8.0 } else { 0.0 };
    let classname = if custom_bottom {
        "monster_generic"
    } else {
        "monster_barney"
    };
    let mut text = format!(
        "{{\"classname\" \"worldspawn\"}}{}{}{}{}{}",
        entity_block("info_player_start", [-200.0, -200.0, 36.0], 0.0, &[]),
        entity_block(
            classname,
            [-100.0, 0.0, 48.0 - bottom],
            0.0,
            &[
                ("targetname", "ohl_descent_actor"),
                ("spawnflags", "16"),
                ("model", "models/ohl-descent.mdl")
            ]
        ),
        entity_block(
            "scripted_sequence",
            [100.0, 0.0, -bottom],
            90.0,
            &[
                ("targetname", "ohl_descent_script"),
                ("m_iszEntity", "ohl_descent_actor"),
                ("m_fMoveTo", mode),
                ("spawnflags", flags),
                ("target", "ohl_descent_done")
            ]
        ),
        entity_block(
            "trigger_auto",
            [0.0; 3],
            0.0,
            &[("target", "ohl_descent_script")]
        ),
        entity_block(
            "trigger_changelevel",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "ohl_descent_done"),
                ("map", "ohlelsewhere"),
                ("landmark", "ohl_descent_landmark")
            ]
        )
    );
    for x in [-100.0, 0.0, 100.0] {
        for y in [-96.0, 0.0, 96.0] {
            text.push_str(&entity_block("info_node", [x, y, 8.0], 0.0, &[]));
        }
    }
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&text);
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::box_brush([-8.0, -48.0, 0.0], [8.0, 48.0, 128.0]),
    ]);
    builder.push_model(
        [-512.0, -512.0, -256.0],
        [512.0; 3],
        [0.0; 3],
        heads,
        2,
        0,
        0,
    );
    let bytes = builder.build();
    let bounds = ([-8.0, -8.0, bottom], [8.0, 8.0, bottom + 60.0]);
    let mut mdl = model([0.0, 0.0, bottom + 48.0], bounds, true);
    for (base, values) in [(88, bounds.0), (100, bounds.1)] {
        for (axis, value) in values.into_iter().enumerate() {
            mdl[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    assets.insert("models/ohl-descent.mdl", mdl);
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("authored elevated graph");
    // Explicit authored runtime displacement: spawn placement now stands a
    // walker on its floor. This fixture exercises later navigation descent,
    // not the separate spawn-floor policy.
    let entity = descent_actor(&game, custom_bottom);
    let raised = Vec3::new(-100.0, 0.0, 48.0 - bottom);
    game.registry()
        .world
        .get::<&mut Actor>(entity)
        .expect("actor")
        .origin = raised;
    game.registry()
        .world
        .get::<&mut Transform>(entity)
        .expect("transform")
        .origin = raised;
    (assets, game)
}

fn descent_actor(game: &Game, custom_bottom: bool) -> ohl_game::hecs::Entity {
    crate::test_support::entity_of_classname(
        game,
        if custom_bottom {
            "monster_generic"
        } else {
            "monster_barney"
        },
    )
    .expect("walker")
}

fn assert_descent_anchor_geometry(game: &mut Game, custom_bottom: bool) -> Actor {
    let entity = descent_actor(game, custom_bottom);
    let actor = *game.registry().world.get::<&Actor>(entity).expect("actor");
    let transform = *game
        .registry()
        .world
        .get::<&Transform>(entity)
        .expect("transform");
    assert_eq!(
        actor.body_frame,
        if custom_bottom {
            BodyFrame::ModelBottom(-8.0)
        } else {
            BodyFrame::Feet
        }
    );
    assert_eq!(actor.origin, transform.origin);
    assert_eq!(actor.yaw.to_bits(), transform.angles.y.to_bits());
    let placement = ohl_render::placement(transform.origin.to_array(), transform.angles.y);
    assert_eq!(
        [placement[12], placement[13], placement[14]].map(f32::to_bits),
        actor.origin.to_array().map(f32::to_bits)
    );
    let (level, _) = game.level_and_systems_mut();
    // Inspect posed geometry when rebuilt at this post-tick anchor. The live
    // phase-5 cache precedes phase-8b movement by one fixed step; this does
    // not claim that earlier cached sample already contains this position.
    let mut index = HitboxIndex::new(HitboxLimits::default());
    crate::combat::rebuild_hitbox_index(&mut index, level);
    let entry = index
        .entries()
        .iter()
        .find(|entry| entry.id == crate::ids::entity_id(entity))
        .expect("posed body");
    assert_eq!(entry.origin, actor.origin);
    assert_eq!(entry.boxes.len(), 1);
    assert_eq!(
        entry.boxes[0].min.z.to_bits(),
        if custom_bottom { -8.0_f32 } else { 0.0_f32 }.to_bits()
    );
    assert_eq!(
        entry.boxes[0].max.z.to_bits(),
        if custom_bottom { 52.0_f32 } else { 60.0_f32 }.to_bits()
    );
    actor
}

#[test]
fn an_elevated_script_descends_completes_once_and_reconstructs_attachment_after_save() {
    for custom_bottom in [false, true] {
        let (assets, mut game) = elevated_graph_script(custom_bottom);
        let first = assert_descent_anchor_geometry(&mut game, custom_bottom);
        for _ in 0..40 {
            game.tick(TICK_SECONDS, &Input::default());
        }
        let middle = assert_descent_anchor_geometry(&mut game, custom_bottom);
        assert!(
            middle.origin.z < first.origin.z && middle.origin.z > first.origin.z - 40.0,
            "save during real bounded descent, before supported landing"
        );
        assert_eq!(middle.origin.truncate(), first.origin.truncate());
        let save = game.to_save(0);
        let mut loaded = Game::from_save(&assets, &save).expect("restore intermediate anchor");
        assert_eq!(
            assert_descent_anchor_geometry(&mut loaded, custom_bottom).origin,
            middle.origin
        );
        for game in [&mut game, &mut loaded] {
            let mut previous = middle;
            let mut completions = 0;
            let mut settled = false;
            let mut detoured = false;
            for _ in 0..1_200 {
                completions += game
                    .tick(TICK_SECONDS, &Input::default())
                    .iter()
                    .filter(|event| matches!(event, crate::GameEvent::LevelChange { .. }))
                    .count();
                let actor = assert_descent_anchor_geometry(game, custom_bottom);
                let from = previous.query_origin();
                let to = actor.query_origin();
                assert!(
                    (to - from).length() <= 40.0 * TICK_SECONDS + 0.001,
                    "no teleport slice"
                );
                assert!(to.z <= from.z + 0.001);
                assert!(to.z >= actor.hull.foot_offset());
                let (level, _) = game.level_and_systems_mut();
                let collision = level.monster_collision.as_ref().expect("world collision");
                assert!(!collision.trace(actor.hull, from, to).blocked());
                assert!(!collision.trace(actor.hull, to, to).start_solid);
                if !settled {
                    assert_eq!(actor.origin.truncate(), middle.origin.truncate());
                }
                settled |= to.z <= actor.hull.foot_offset() + 0.1;
                detoured |= actor.origin.y.abs() > 64.0;
                previous = actor;
            }
            assert!(
                settled && detoured,
                "actual support and a graph detour both occurred"
            );
            assert_eq!(completions, 1);
            assert_eq!(game.script_completion_count(), 1);
            assert_eq!(game.script_timeout_count(), 0);
            assert!(game.script_navigation_stats().graph_steps > 0);
            assert_eq!(game.script_navigation_stats().start_solid, 0);
            assert_eq!(game.script_navigation_stats().untraced_steps, 0);
            assert!(previous.origin.x > 65.0);
        }
    }
}

#[test]
fn held_teleport_no_movement_and_turn_preserve_authoritative_script_placement() {
    for (mode, flags) in [("4", "0"), ("0", "0"), ("5", "0"), ("0", "128")] {
        let (_, mut game) = elevated_graph_script_mode(false, mode, flags);
        let first = assert_descent_anchor_geometry(&mut game, false);
        let mut completions = 0;
        let mut held = false;
        for _ in 0..600 {
            completions += game
                .tick(TICK_SECONDS, &Input::default())
                .iter()
                .filter(|event| matches!(event, crate::GameEvent::LevelChange { .. }))
                .count();
            let actor = assert_descent_anchor_geometry(&mut game, false);
            held |= game.active_script_count() > 0;
            if mode != "4" {
                assert_eq!(actor.origin, first.origin);
            }
            if completions > 0 {
                assert_eq!(
                    actor.origin,
                    if mode == "4" {
                        Vec3::new(100.0, 0.0, 0.0)
                    } else {
                        first.origin
                    }
                );
                let yaw = if mode == "0" && flags == "0" {
                    0.0
                } else {
                    90.0
                };
                assert!((actor.yaw - yaw).abs() <= 5.0);
                break;
            }
        }
        assert!(held);
        assert_eq!(completions, 1);
        assert_eq!(game.script_completion_count(), 1);
        assert_eq!(game.script_timeout_count(), 0);
        assert_eq!(game.script_navigation_stats().graph_steps, 0);
    }
}

#[test]
fn lost_or_mismatched_support_keeps_a_script_pending_without_lowering_or_completing() {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    for raised_floor in [false, true] {
        let (_, mut game) = elevated_graph_script(false);
        for _ in 0..40 {
            game.tick(TICK_SECONDS, &Input::default());
        }
        let middle = assert_descent_anchor_geometry(&mut game, false);
        assert!(middle.origin.z > 20.0 && middle.origin.z < 48.0);
        let mut builder = Bsp30Builder::new();
        let mut brushes = vec![CollisionBrush::box_brush(
            [-8.0, -48.0, 0.0],
            [8.0, 48.0, 128.0],
        )];
        if raised_floor {
            brushes.push(CollisionBrush::half_space([0.0, 0.0, 1.0], 20.0));
        }
        let heads = builder.push_collision_hulls(&brushes);
        builder.push_model(
            [-512.0, -512.0, -256.0],
            [512.0; 3],
            [0.0; 3],
            heads,
            2,
            0,
            0,
        );
        let bytes = builder.build();
        let limits = ohl_formats::bsp30::Limits::default();
        let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &limits).expect("changed synthetic floor");
        game.level_and_systems_mut().0.monster_collision = Some(
            ohl_physics::CollisionModel::from_bsp(&bsp, &limits).expect("live changed collision"),
        );
        for _ in 0..500 {
            assert!(
                game.tick(TICK_SECONDS, &Input::default())
                    .iter()
                    .all(|event| !matches!(event, crate::GameEvent::LevelChange { .. }))
            );
            assert_eq!(
                assert_descent_anchor_geometry(&mut game, false).origin,
                middle.origin
            );
        }
        assert_eq!(game.script_completion_count(), 0);
        assert_eq!(game.script_timeout_count(), 0);
        assert_eq!(game.active_script_count(), 1);
        assert_eq!(game.script_navigation_stats().untraced_steps, 0);
    }
}

fn terminal_ground_script(custom_bottom: bool) -> (MemoryAssets, Game) {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    let bottom = if custom_bottom { -8.0 } else { 0.0 };
    let mut text = format!(
        "{{\"classname\" \"worldspawn\"}}{}{}{}{}{}",
        entity_block("info_player_start", [-200.0, -200.0, 36.0], 0.0, &[]),
        entity_block(
            if custom_bottom {
                "monster_generic"
            } else {
                "monster_barney"
            },
            [-100.0, 0.0, 48.0 - bottom],
            0.0,
            &[
                ("targetname", "ohl_terminal_actor"),
                ("spawnflags", "16"),
                ("model", "models/ohl-terminal.mdl")
            ]
        ),
        entity_block(
            "scripted_sequence",
            [300.0, 0.0, -bottom],
            90.0,
            &[
                ("targetname", "ohl_terminal_script"),
                ("m_iszEntity", "ohl_terminal_actor"),
                ("m_fMoveTo", "1"),
                ("target", "ohl_terminal_done")
            ]
        ),
        entity_block(
            "trigger_auto",
            [0.0; 3],
            0.0,
            &[("target", "ohl_terminal_script")]
        ),
        entity_block(
            "trigger_changelevel",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "ohl_terminal_done"),
                ("map", "ohlelsewhere"),
                ("landmark", "ohl_terminal_landmark")
            ]
        ),
    );
    // Reuse the independently authored successful detour, with a long clear
    // run before the terminal lane. This separates its earlier graph corners
    // from the outer wall's probe horizon; no nonterminal repair is assumed.
    // The last graph node precedes the mark; the outer wall lies beyond it.
    for x in [-100.0, 0.0, 100.0] {
        for y in [-96.0, 0.0, 96.0] {
            text.push_str(&entity_block("info_node", [x, y, 8.0], 0.0, &[]));
        }
    }
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&text);
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::box_brush([-8.0, -48.0, 0.0], [8.0, 48.0, 128.0]),
        CollisionBrush::box_brush([192.0, 28.0, 0.0], [340.0, 128.0, 128.0]),
        CollisionBrush::box_brush([192.0, -128.0, 0.0], [340.0, -28.0, 128.0]),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -320.0),
    ]);
    builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], heads, 2, 0, 0);
    let bytes = builder.build();
    let bounds = ([-8.0, -8.0, bottom], [8.0, 8.0, bottom + 60.0]);
    let mut mdl = model([0.0, 0.0, bottom + 48.0], bounds, true);
    for (base, values) in [(88, bounds.0), (100, bounds.1)] {
        for (axis, value) in values.into_iter().enumerate() {
            mdl[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    assets.insert("models/ohl-terminal.mdl", mdl);
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("authored terminal graph");
    (assets, game)
}

#[test]
fn terminal_ground_real_script_and_saved_continuation_complete_after_a_detour() {
    for custom_bottom in [false, true] {
        let (assets, mut game) = terminal_ground_script(custom_bottom);
        let mut detoured = false;
        let mut saved_on_graph_detour = false;
        for _ in 0..1_200 {
            game.tick(TICK_SECONDS, &Input::default());
            let actor = assert_descent_anchor_geometry(&mut game, custom_bottom);
            detoured |= actor.origin.y.abs() > 64.0;
            let goal = Vec3::new(300.0, 0.0, if custom_bottom { 8.0 } else { 0.0 });
            let query_goal = actor.body_frame.anchor_to_query(actor.hull, goal);
            let (level, _) = game.level_and_systems_mut();
            let collision = level.monster_collision.as_ref().expect("collision");
            if detoured
                && collision
                    .trace(actor.hull, actor.query_origin(), query_goal)
                    .blocked()
            {
                // Save while the real obstacle still requires graph routing.
                // Saving in the clear terminal lane could legitimately rebuild
                // a direct route, outside this deliberately graph-only slice.
                saved_on_graph_detour = true;
                break;
            }
        }
        assert!(
            detoured && saved_on_graph_detour,
            "save during the real obstructed graph detour"
        );
        assert_eq!(game.script_completion_count(), 0);
        let middle = assert_descent_anchor_geometry(&mut game, custom_bottom);
        let save = game.to_save(0);
        let mut loaded = Game::from_save(&assets, &save).expect("actual intermediate script save");
        assert_eq!(
            assert_descent_anchor_geometry(&mut loaded, custom_bottom).origin,
            middle.origin
        );
        for game in [&mut game, &mut loaded] {
            let mut previous = middle;
            let mut completions = 0;
            let mut at_terminal_lane = false;
            for _ in 0..1_200 {
                completions += game
                    .tick(TICK_SECONDS, &Input::default())
                    .iter()
                    .filter(|event| matches!(event, crate::GameEvent::LevelChange { .. }))
                    .count();
                let actor = assert_descent_anchor_geometry(game, custom_bottom);
                let from = previous.query_origin();
                let to = actor.query_origin();
                at_terminal_lane |= actor.origin.x > 252.0;
                assert!((to - from).length() <= 40.0 * TICK_SECONDS + 0.001);
                let (level, _) = game.level_and_systems_mut();
                let collision = level.monster_collision.as_ref().expect("collision");
                assert!(
                    !collision.trace(actor.hull, from, to).blocked(),
                    "every committed terminal move stays clear"
                );
                assert!(!collision.trace(actor.hull, to, to).start_solid);
                previous = actor;
            }
            assert!(
                at_terminal_lane,
                "each actual branch reaches the final lane: {:?}, {:?}",
                previous.origin,
                game.script_navigation_stats()
            );
            assert_eq!(
                completions, 1,
                "actual terminal approach completes without forcing script release"
            );
            assert_eq!(game.script_completion_count(), 1);
            assert_eq!(game.script_timeout_count(), 0);
            assert!(game.script_navigation_stats().graph_steps > 0);
            assert_eq!(game.script_navigation_stats().untraced_steps, 0);
        }
    }
}

// Project-authored wall-policy controls use solidity probes in test assertions.
// They add no production queries and do not change private observers.
// These fixtures do not reproduce or explain any private route.
mod wall_occupant_policy {
    use super::*;
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    use ohl_game::registry::WallToggle;
    use ohl_physics::Hull;

    const WALL_MIN: Vec3 = Vec3::new(-8.0, -128.0, 48.0);
    const WALL_MAX: Vec3 = Vec3::new(8.0, 128.0, 64.0);
    const PLAYER_START: Vec3 = Vec3::new(-96.0, 0.0, 36.0);
    const OCCUPANT: Vec3 = Vec3::new(0.0, 80.0, 0.0);
    const OUTSIDE: Vec3 = Vec3::new(96.0, 80.0, 0.0);
    const WALK_TICKS: usize = 80;

    fn overlaps(min: Vec3, max: Vec3) -> bool {
        min.cmplt(WALL_MAX).all() && max.cmpgt(WALL_MIN).all()
    }

    fn scene(
        npc: Option<Vec3>,
        player_start: Vec3,
        wall_class: &str,
        activate: bool,
    ) -> (MemoryAssets, Game) {
        let mut entities = String::from("{\"classname\" \"worldspawn\"}\n");
        entities.push_str(&entity_block(
            "info_player_start",
            player_start.to_array(),
            0.0,
            &[],
        ));
        entities.push_str(&entity_block(
            wall_class,
            [0.0; 3],
            0.0,
            &[
                ("model", "*1"),
                ("targetname", "ohl_wide_wall"),
                ("spawnflags", "1"),
            ],
        ));
        if activate {
            entities.push_str("{\"classname\" \"trigger_auto\" \"target\" \"ohl_wide_wall\"}");
        }
        if let Some(origin) = npc {
            entities.push_str(&entity_block(
                "monster_barney",
                origin.to_array(),
                0.0,
                &[("targetname", "ohl_lateral_npc"), ("spawnflags", "16")],
            ));
        }
        let mut builder = Bsp30Builder::new();
        builder.set_entities_text(&entities);
        let floor =
            builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
        builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], floor, 2, 0, 0);
        let wall = builder.push_collision_hulls(&[CollisionBrush::box_brush(
            WALL_MIN.to_array(),
            WALL_MAX.to_array(),
        )]);
        builder.push_model(
            WALL_MIN.to_array(),
            WALL_MAX.to_array(),
            [0.0; 3],
            wall,
            2,
            0,
            0,
        );
        let bytes = builder.build();
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
        let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("authored wide wall");
        (assets, game)
    }

    fn fixture(npc: Option<Vec3>) -> Game {
        scene(npc, PLAYER_START, "func_wall_toggle", true).1
    }

    fn solidity(game: &Game) -> (bool, bool) {
        let probe = Vec3::new(0.0, 0.0, 56.0);
        (
            game.position_is_in_solid(probe.to_array()),
            game.monster_collision()
                .expect("monster collision")
                .trace(Hull::Point, probe, probe)
                .start_solid,
        )
    }

    fn idle(game: &mut Game, steps: usize) {
        for _ in 0..steps {
            game.tick(TICK_SECONDS, &Input::default());
        }
    }

    fn vacate_npc(game: &Game) {
        // Authored occupancy control between fixed steps, not an NPC route.
        let entity = game.registry().find("ohl_lateral_npc")[0];
        game.registry()
            .world
            .get::<&mut Actor>(entity)
            .unwrap()
            .origin = OUTSIDE;
        game.registry()
            .world
            .get::<&mut Transform>(entity)
            .unwrap()
            .origin = OUTSIDE;
    }

    fn walk_player_out(game: &mut Game, npc: Vec3) {
        for _ in 0..40 {
            game.tick(
                TICK_SECONDS,
                &Input {
                    forward: 1,
                    ..Input::default()
                },
            );
            sample(game, Some(npc));
        }
        assert!(
            game.player_origin()[0] - 16.0 > WALL_MAX.x,
            "the player actually escapes the occupied wall"
        );
        idle(game, 2);
    }

    fn sample(game: &Game, npc: Option<Vec3>) -> Vec3 {
        let wall = game.registry().find("ohl_wide_wall")[0];
        assert!(
            game.registry()
                .world
                .get::<&WallToggle>(wall)
                .unwrap()
                .visible
        );
        assert!(game.has_collision() && game.player_health() > 0.0);
        assert!(game.player_on_ground(), "player remains a grounded walker");
        let position = Vec3::from_array(game.player_origin());
        assert!(position.y.abs() < 0.01, "player cannot go around the wall");
        assert!(
            (position.z - PLAYER_START.z).abs() < 1.0,
            "player cannot go over the wall"
        );
        let player = game
            .registry()
            .world
            .get::<&Actor>(game.player_entity())
            .unwrap();
        assert_eq!(player.body_frame, BodyFrame::Centered);
        assert_eq!(player.hull, Hull::Standing);
        if let Some(expected) = npc {
            let entity = game.registry().find("ohl_lateral_npc")[0];
            let actor = game.registry().world.get::<&Actor>(entity).unwrap();
            assert!(actor.alive && actor.health > 0.0);
            assert!(
                game.registry()
                    .world
                    .get::<&ohl_ai::Impervious>(entity)
                    .is_err()
            );
            assert_eq!(actor.body_frame, BodyFrame::Feet);
            assert_eq!(actor.hull, Hull::Standing);
            // Prisoner alone does not imply immobility. Check the actual state;
            // never reposition the actor or suppress its ordinary thinking.
            assert!(
                actor.origin.abs_diff_eq(expected, 0.001),
                "authored NPC stays in place"
            );
            let (min, max) = actor.body_frame.world_bounds(actor.hull, actor.origin);
            assert!(min.y > 32.0, "the NPC stays outside the player lane");
            assert_eq!(overlaps(min, max), expected == OCCUPANT);
            let (legacy_min, legacy_max) = actor.hull.bounds();
            assert!(!overlaps(
                actor.origin + legacy_min,
                actor.origin + legacy_max
            ));
        } else {
            assert!(game.registry().find("ohl_lateral_npc").is_empty());
        }
        position
    }

    // These exact authored constants are prerequisites for the fixed input interval.
    #[allow(clippy::float_cmp)]
    fn walk(npc: Option<Vec3>) -> Vec<Vec3> {
        let mut game = fixture(npc);
        for _ in 0..5 {
            game.tick(TICK_SECONDS, &Input::default());
        }
        let start = sample(&game, npc);
        let (min, max) = Hull::Standing.bounds();
        assert!(!overlaps(start + min, start + max));
        assert!(start.x + max.x < WALL_MIN.x, "initial player is outside");
        assert!(WALL_MAX.z > game.move_config().step_size);
        // The fixed 0.8 s interval exceeds gap/speed plus the default acceleration
        // ramp, independently of observed outcomes; all scenes use every input.
        assert_eq!(game.move_config().max_speed, 320.0);
        assert_eq!(game.move_config().accelerate, 10.0);
        let input = Input {
            forward: 1,
            ..Input::default()
        };
        let mut history = vec![start];
        for _ in 0..WALK_TICKS {
            game.tick(TICK_SECONDS, &input);
            history.push(sample(&game, npc));
        }
        history
    }

    #[test]
    fn upper_body_occupancy_preserves_player_blocking_under_identical_inputs() {
        let occupied = walk(Some(OCCUPANT));
        let absent = walk(None);
        let outside = walk(Some(OUTSIDE));
        let (_, max) = Hull::Standing.bounds();
        // The accepted mechanism test recorded crossing under the old shared
        // exemption. This intentionally changes that project-authored policy;
        // its lane, floor, body-overlap and actual-input prerequisites remain.
        for blocked in [&occupied, &absent, &outside] {
            assert!(
                blocked.last().unwrap().x > PLAYER_START.x + 32.0,
                "control actually approaches the wall"
            );
            assert!(
                blocked.last().unwrap().x + max.x <= WALL_MIN.x + 0.05,
                "visible wall blocks the unrelated player on its near side"
            );
        }
        assert_eq!(
            occupied, absent,
            "NPC-only occupancy does not change player history"
        );
        assert_eq!(
            absent, outside,
            "an off-lane NPC outside the wall does not change player history"
        );
    }

    #[test]
    fn simultaneous_occupants_release_each_model_independently() {
        for npc_leaves_first in [false, true] {
            let (_, mut game) = scene(
                Some(OCCUPANT),
                Vec3::new(0.0, 0.0, 36.0),
                "func_wall_toggle",
                true,
            );
            idle(&mut game, 5);
            for _ in 0..6 {
                sample(&game, Some(OCCUPANT));
                assert_eq!(
                    solidity(&game),
                    (false, false),
                    "both occupants suspend their models"
                );
                idle(&mut game, 1);
            }
            if npc_leaves_first {
                vacate_npc(&game);
                idle(&mut game, 1);
                sample(&game, Some(OUTSIDE));
                assert_eq!(
                    solidity(&game),
                    (false, true),
                    "NPC vacancy restores only the monster wall"
                );
                walk_player_out(&mut game, OUTSIDE);
            } else {
                walk_player_out(&mut game, OCCUPANT);
                for _ in 0..6 {
                    sample(&game, Some(OCCUPANT));
                    assert_eq!(
                        solidity(&game),
                        (true, false),
                        "player vacancy restores only the player wall"
                    );
                    idle(&mut game, 1);
                }
                vacate_npc(&game);
                idle(&mut game, 1);
            }
            assert_eq!(
                solidity(&game),
                (true, true),
                "both vacated models become solid"
            );
        }
    }

    #[test]
    // A skipped dead-player update must preserve the exact saved coordinate array.
    #[allow(clippy::float_cmp)]
    fn npc_only_rechecks_continue_after_player_death() {
        let mut game = fixture(Some(OCCUPANT));
        idle(&mut game, 5);
        sample(&game, Some(OCCUPANT));
        let player = game.player_entity();
        game.systems_mut()
            .damage_queue
            .push(crate::systems::QueuedDamage {
                target: player,
                info: ohl_combat::DamageInfo::new(1000.0, ohl_combat::DamageType::GENERIC),
            });
        idle(&mut game, 1);
        assert!(
            game.player_health() <= 0.0,
            "queued authored damage kills the player"
        );
        let stopped = game.player_origin();
        for _ in 0..6 {
            game.tick(
                TICK_SECONDS,
                &Input {
                    forward: 1,
                    ..Input::default()
                },
            );
            assert_eq!(
                game.player_origin(),
                stopped,
                "dead player movement stays skipped"
            );
            let npc = game.registry().find("ohl_lateral_npc")[0];
            let actor = game.registry().world.get::<&Actor>(npc).unwrap();
            assert!(actor.alive && actor.origin.abs_diff_eq(OCCUPANT, 0.001));
            assert_eq!(
                solidity(&game),
                (true, false),
                "dead-player ticks keep checking NPC occupancy"
            );
        }
        vacate_npc(&game);
        idle(&mut game, 1);
        assert_eq!(
            solidity(&game),
            (true, true),
            "NPC vacancy restores the wall after player death"
        );
    }

    #[test]
    fn restored_visible_wall_reconstructs_occupancy_and_vacancy() {
        let (assets, mut live) = scene(Some(OCCUPANT), PLAYER_START, "func_wall_toggle", true);
        idle(&mut live, 5);
        sample(&live, Some(OCCUPANT));
        let bytes = live.save_bytes(1).expect("authored on-wall save");
        let mut restored = Game::load_bytes(&assets, &bytes).expect("on-wall restore");
        for _ in 0..6 {
            idle(&mut live, 1);
            idle(&mut restored, 1);
            for game in [&live, &restored] {
                sample(game, Some(OCCUPANT));
                assert_eq!(
                    solidity(game),
                    (true, false),
                    "restored NPC-only occupancy stays model-specific"
                );
            }
        }
        for game in [&mut live, &mut restored] {
            vacate_npc(game);
            idle(game, 1);
            assert_eq!(
                solidity(game),
                (true, true),
                "restored vacancy makes both models solid"
            );
        }
    }

    #[test]
    fn off_and_ordinary_walls_keep_their_existing_solidity() {
        let (_, mut off) = scene(Some(OCCUPANT), PLAYER_START, "func_wall_toggle", false);
        let (_, mut ordinary) = scene(Some(OCCUPANT), PLAYER_START, "func_wall", false);
        for _ in 0..6 {
            idle(&mut off, 1);
            idle(&mut ordinary, 1);
            assert_eq!(
                solidity(&off),
                (false, false),
                "off walls remain absent in both models"
            );
            assert_eq!(
                solidity(&ordinary),
                (true, true),
                "ordinary walls receive no toggle exemption"
            );
        }
    }
}

/// Project-authored proxy orientation policy, not original-engine anatomy.
/// The fixture floats inside the generated room so all six blast samples are
/// clear of world geometry. No movement tick or private asset is involved.
mod actor_proxy_rotation {
    use super::*;
    use ohl_combat::{DamageType, TraceMask, trace_attack};
    use ohl_game::hecs::Entity;

    const ANCHOR: Vec3 = Vec3::new(80.0, 20.0, 32.0);

    // The authored yaw must survive model attachment without any numeric change.
    #[allow(clippy::float_cmp)]
    fn fixture(mdl: Option<Vec<u8>>) -> (Level, Entity, HitboxIndex) {
        let block = entity_block("monster_barney", ANCHOR.to_array(), 45.0, &[]);
        let level = level(&block, mdl);
        let entity = level
            .registry
            .world
            .query::<(Entity, &MonsterAi)>()
            .iter()
            .next()
            .expect("authored monster")
            .0;
        // Loading may stand the actor on the floor. This geometry fixture then
        // deliberately floats both anchors so all six blast probes stay clear.
        level
            .registry
            .world
            .get::<&mut Actor>(entity)
            .expect("actor")
            .origin = ANCHOR;
        level
            .registry
            .world
            .get::<&mut Transform>(entity)
            .expect("transform")
            .origin = ANCHOR;
        {
            let actor = level.registry.world.get::<&Actor>(entity).expect("actor");
            let transform = level
                .registry
                .world
                .get::<&Transform>(entity)
                .expect("transform");
            assert_eq!(actor.origin, ANCHOR);
            assert_eq!(transform.origin, ANCHOR);
            assert_eq!(actor.yaw, 45.0);
            assert_eq!(transform.angles, Vec3::new(0.0, 45.0, 0.0));
            assert_eq!(actor.body_frame, BodyFrame::Feet);
            assert_eq!(actor.hull, ohl_physics::Hull::Standing);
        }
        let mut index = HitboxIndex::new(HitboxLimits::default());
        crate::combat::rebuild_hitbox_index(&mut index, &level);
        (level, entity, index)
    }

    fn vertical_ray(level: &Level, index: &HitboxIndex, offset: Vec3) -> ohl_combat::AttackTrace {
        trace_attack(
            level.collision.as_ref().expect("authored room collision"),
            index,
            ANCHOR + offset + Vec3::Z * 90.0,
            ANCHOR + offset - Vec3::Z * 10.0,
            TraceMask::ENTITIES_ONLY,
        )
    }

    /// Exercises actual cached blast bounds through the real map-blast consumer.
    /// Each source is 16 units outside one analytically expected world-bound face;
    /// radius/damage 64 implies 48 damage and an inward axis direction. This avoids
    /// copying the production corner loop or exposing its private bounds cache.
    fn blast_faces(
        level: &Level,
        entity: Entity,
        index: &HitboxIndex,
        min: Vec3,
        max: Vec3,
    ) -> Vec<(f32, Vec3)> {
        let mut projectiles = crate::projectiles::ProjectileSystem::new(7);
        projectiles.update_blast_bounds(index);
        let middle = (min + max) * 0.5;
        let mut samples = Vec::new();
        for axis in 0..3 {
            for sign in [-1.0, 1.0] {
                let mut origin = middle;
                origin[axis] = if sign < 0.0 {
                    min[axis] - 16.0
                } else {
                    max[axis] + 16.0
                };
                let mut damage = Vec::new();
                projectiles.resolve_map_blast(
                    level,
                    crate::map_effects::BlastRequest {
                        origin,
                        profile: ohl_game::effects::MapBlastProfile {
                            damage: 64.0,
                            radius: 64.0,
                        },
                        kind: DamageType::BLAST,
                        attacker: level.player,
                        inflictor: level.player,
                    },
                    &mut damage,
                );
                let hits: Vec<_> = damage.iter().filter(|hit| hit.target == entity).collect();
                assert_eq!(
                    hits.len(),
                    1,
                    "one real blast result for the authored actor"
                );
                let info = hits[0].info;
                let mut inward = Vec3::ZERO;
                inward[axis] = -sign;
                assert!(
                    (info.amount - 48.0).abs() < 0.001,
                    "expected world-bound face distance"
                );
                assert!(info.direction.abs_diff_eq(inward, 0.001));
                assert_eq!(info.kind, DamageType::BLAST);
                assert_eq!(info.attacker, Some(crate::ids::entity_id(level.player)));
                assert_eq!(info.inflictor, info.attacker);
                samples.push((info.amount, info.direction));
            }
        }
        samples
    }

    #[test]
    // Zero header bounds identify this authored degenerate-model case exactly.
    #[allow(clippy::float_cmp)]
    fn missing_and_degenerate_models_share_axis_aligned_rays_and_blast_bounds() {
        let mut expected_blast = None;
        for degenerate in [false, true] {
            let mdl = degenerate.then(|| model([0.0; 3], ([0.0; 3], [0.0; 3]), false));
            let (level, entity, index) = fixture(mdl);
            if degenerate {
                let anim = level
                    .registry
                    .world
                    .get::<&StudioAnim>(entity)
                    .expect("loaded model");
                let model = &level.studio_models[anim.model];
                assert!(model.hitboxes.is_empty());
                assert_eq!(model.bounds_min, [0.0; 3]);
                assert_eq!(model.bounds_max, [0.0; 3]);
            } else {
                assert!(level.registry.world.get::<&StudioAnim>(entity).is_err());
            }
            let id = crate::ids::entity_id(entity);
            let entry = index
                .entries()
                .iter()
                .find(|entry| entry.id == id)
                .expect("actor proxy");
            assert_eq!(entry.origin, ANCHOR);
            assert_eq!(entry.boxes.len(), 1);
            assert_eq!(entry.boxes[0].min, Vec3::new(-16.0, -16.0, 0.0));
            assert_eq!(entry.boxes[0].max, Vec3::new(16.0, 16.0, 72.0));
            // At yaw 45 a rotated square reaches x=20; the axis-aligned proxy does not.
            assert_eq!(
                vertical_ray(&level, &index, Vec3::X * 20.0).entity,
                None,
                "asset availability must not rotate the invented Actor corner"
            );
            assert_eq!(vertical_ray(&level, &index, Vec3::ZERO).entity, Some(id));
            let samples = blast_faces(
                &level,
                entity,
                &index,
                ANCHOR + Vec3::new(-16.0, -16.0, 0.0),
                ANCHOR + Vec3::new(16.0, 16.0, 72.0),
            );
            if let Some(expected) = &expected_blast {
                assert_eq!(&samples, expected, "asset-independent world blast faces");
            } else {
                expected_blast = Some(samples);
            }
        }
    }

    #[test]
    fn valid_posed_and_clipping_boxes_keep_their_world_rotation() {
        let bounds = ([8.0, -2.0, 3.0], [28.0, 2.0, 40.0]);
        let rotation = glam::Quat::from_rotation_z(45.0_f32.to_radians());
        for posed in [false, true] {
            let (level, entity, index) = fixture(Some(model([0.0; 3], bounds, posed)));
            let id = crate::ids::entity_id(entity);
            let entry = index
                .entries()
                .iter()
                .find(|entry| entry.id == id)
                .expect("model box");
            let animation = if posed { Vec3::X * 10.0 } else { Vec3::ZERO };
            let min = Vec3::from_array(bounds.0) + animation;
            let max = Vec3::from_array(bounds.1) + animation;
            assert_eq!(entry.origin, ANCHOR);
            assert_eq!(entry.boxes.len(), 1);
            assert_eq!(entry.boxes[0].min, min);
            assert_eq!(entry.boxes[0].max, max);
            assert!(entry.rotation.abs_diff_eq(rotation, 0.001));
            let middle = (min + max) * 0.5;
            let ray = rotation * Vec3::new(middle.x, middle.y, 0.0);
            assert_eq!(vertical_ray(&level, &index, ray).entity, Some(id));
            assert_eq!(
                vertical_ray(&level, &index, Vec3::X * middle.x).entity,
                None
            );
            // Independent yaw-45 extrema for this asymmetric local rectangle.
            let diagonal = std::f32::consts::FRAC_1_SQRT_2;
            let world_min = ANCHOR
                + Vec3::new(
                    (min.x - max.y) * diagonal,
                    (min.x + min.y) * diagonal,
                    min.z,
                );
            let world_max = ANCHOR
                + Vec3::new(
                    (max.x - min.y) * diagonal,
                    (max.x + max.y) * diagonal,
                    max.z,
                );
            let _ = blast_faces(&level, entity, &index, world_min, world_max);
        }
    }
}

/// Legacy tag-25 record bytes, authored from its unchanged declaration-order
/// Postcard schema. Non-dyadic points and signed zero must survive literally;
/// neither the owner model bottom nor the target's present hull identifies the
/// historical source of a sound-overwritten destination or damage memory.
#[test]
fn anchor_domain_legacy_wire_and_new_save_preserve_all_point_bits() {
    const LEGACY: &[u8] = &[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 102, 102, 166, 63, 0, 0, 0, 128, 0, 0, 137, 65, 0, 0,
        0, 63, 1, 0, 0, 40, 66, 2, 0, 64, 200, 66, 154, 153, 137, 192, 205, 204, 42, 66, 0, 0, 180,
        192, 102, 102, 6, 64, 0, 0, 0, 128, 1, 0, 0, 180, 192, 102, 102, 6, 64, 0, 0, 0, 128, 1,
        154, 153, 194, 66, 0, 0, 2, 65, 0, 0, 0, 128, 1, 102, 102, 36, 194, 0, 0, 168, 64, 102,
        102, 146, 65, 0, 0, 0, 32, 66, 0, 0, 0, 0, 0, 0,
    ];
    let legacy: crate::save_state::AiSnapshot =
        postcard::from_bytes(LEGACY).expect("legacy record");
    assert_eq!(postcard::to_allocvec(&legacy).unwrap(), LEGACY);
    let defs = format!(
        "{}{}",
        entity_block(
            "monster_generic",
            [100.0, 0.0, 72.0],
            0.0,
            &[("model", "models/ohl-wire.mdl")]
        ),
        entity_block("monster_barney", [-100.0, 0.0, 0.0], 0.0, &[])
    );
    let (mut media, bytes) = assets(&defs, None);
    let mut mdl = model(
        [0.0, 0.0, 20.0],
        ([-16.0, -16.0, -3.7], [16.0, 16.0, 68.3]),
        false,
    );
    for (base, values) in [
        (88, [-16.0_f32, -16.0, -3.7]),
        (100, [16.0_f32, 16.0, 68.3]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            mdl[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    media.insert("models/ohl-wire.mdl", mdl);
    let game = Game::from_map_bytes(&media, AI_MAP, &bytes).expect("custom model game");
    let owner = crate::test_support::entity_of_classname(&game, "monster_generic").unwrap();
    let target = crate::test_support::entity_of_classname(&game, "monster_barney").unwrap();
    let mut saved = game.to_save(7);
    let owner_index = game
        .registry()
        .entities
        .iter()
        .position(|entry| *entry == owner)
        .unwrap();
    let target_index = game
        .registry()
        .entities
        .iter()
        .position(|entry| *entry == target)
        .unwrap();
    assert_eq!(
        target_index, 3,
        "fixed generated registry binds the golden enemy slot"
    );
    saved.ai.as_mut().unwrap()[owner_index] = Some(legacy);
    // Tag18 is an anchor, even when the loaded point is not on a floor.
    let anchor = Vec3::new(100.0, 0.0, 91.25);
    saved.entities[owner_index]
        .transform
        .as_mut()
        .unwrap()
        .origin = anchor;
    let encoded = saved.to_bytes().expect("legacy container");
    let loaded = Game::load_bytes(&media, &encoded).expect("legacy load");
    let owner = crate::test_support::entity_of_classname(&loaded, "monster_generic").unwrap();
    let actor = loaded.registry().world.get::<&Actor>(owner).unwrap();
    assert_eq!(actor.body_frame, BodyFrame::ModelBottom(-3.7));
    assert_eq!(actor.origin, anchor, "no post-restore floor placement");
    assert_eq!(
        loaded
            .registry()
            .world
            .get::<&Transform>(owner)
            .unwrap()
            .origin,
        anchor
    );
    let next = loaded.to_save(7);
    assert_eq!(
        postcard::to_allocvec(next.ai.as_ref().unwrap()[owner_index].as_ref().unwrap()).unwrap(),
        LEGACY,
        "tag25 positional bytes remain literal through real load/save"
    );
    let next_bytes = next.to_bytes().unwrap();
    let reader = ohl_save::SaveReader::open(&next_bytes, &ohl_save::Limits::default()).unwrap();
    assert!(
        reader.section(46).is_err(),
        "derived frames introduce no save section"
    );
    let again = Game::load_bytes(&media, &next_bytes).unwrap().to_save(7);
    assert_eq!(
        postcard::to_allocvec(&next.ai).unwrap(),
        postcard::to_allocvec(&again.ai).unwrap()
    );
    let mut absent = saved;
    absent.ai = None;
    assert!(
        Game::from_save(&media, &absent).is_ok(),
        "absent optional tag retains default reconstruction"
    );
}

#[test]
// Keep all movement branches and the terminal-arrival discriminator in one test.
#[allow(clippy::too_many_lines)]
fn anchor_domain_query_route_is_consumed_once_in_all_movement_branches() {
    use ohl_ai::{
        AiWorld, DefaultBrain, NavBridge, NavBridgeLimits, Route, ScriptHold, SightContext,
    };
    use ohl_formats::{
        bsp30::{Bsp, Limits},
        test_support::{Bsp30Builder, CollisionBrush},
    };
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\"classname\" \"worldspawn\"}");
    let heads = builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)]);
    builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], heads, 2, 0, 0);
    let bytes = builder.build();
    let bsp = Bsp::parse(&bytes, &Limits::default()).unwrap();
    let collision = ohl_physics::CollisionModel::from_bsp(&bsp, &Limits::default()).unwrap();
    for mode in 0..3 {
        let mut ai = AiWorld::new(5);
        let brain = ai.register_brain(Box::new(DefaultBrain::default()));
        let mut world = ohl_game::hecs::World::new();
        let actor = Actor::new(ohl_ai::Classification::None, Vec3::ZERO);
        let goal = actor
            .body_frame
            .anchor_to_query(actor.hull, Vec3::X * 100.0);
        assert_eq!(
            goal,
            Vec3::new(100.0, 0.0, 36.0),
            "query fixture prerequisite"
        );
        let entity = ohl_ai::spawn_monster(&mut world, actor, brain);
        world.insert_one(entity, ScriptHold).unwrap();
        {
            let mut state = world.get::<&mut MonsterAi>(entity).unwrap();
            state.route = Route::straight_line(goal);
            state.move_speed = 100.0;
        }
        if mode == 2 {
            ai.attach_navigator(NavBridge::build(
                &[],
                &collision,
                &ohl_nav::BuildLimits::default(),
                NavBridgeLimits::default(),
            ));
        }
        let context = if mode == 0 {
            SightContext::default()
        } else {
            SightContext::tracing(&collision)
        };
        for _ in 0..50 {
            ai.tick(&mut world, &context, 0.01);
        }
        let moved = world.get::<&Actor>(entity).unwrap();
        assert!(
            moved.origin.x > 40.0,
            "the selected route actually advanced"
        );
        assert!(
            moved.origin.z.abs() < 0.1,
            "stored query waypoint is not lifted a second time"
        );
        assert_eq!(world.get::<&MonsterAi>(entity).unwrap().route.goal, goal);
    }

    // A nonzero custom frame within the real terminal arrival radius exposes
    // a second goal translation. Point fliers cannot: their frame offset is zero.
    let mut ai = AiWorld::new(5);
    let brain = ai.register_brain(Box::new(DefaultBrain::default()));
    let mut world = ohl_game::hecs::World::new();
    let mut actor = Actor::new(ohl_ai::Classification::None, Vec3::new(0.0, 0.0, 20.0));
    actor.body_frame = ohl_ai::BodyFrame::ModelBottom(-20.0);
    assert_eq!(actor.query_origin(), Vec3::new(0.0, 0.0, 36.0));
    let goal = Vec3::new(20.0, 0.0, 36.0);
    assert!(
        !collision
            .trace(actor.hull, actor.query_origin(), goal)
            .blocked()
    );
    let entity = ohl_ai::spawn_monster(&mut world, actor, brain);
    world.insert_one(entity, ScriptHold).unwrap();
    {
        let mut state = world.get::<&mut MonsterAi>(entity).unwrap();
        state.route = Route::straight_line(goal);
        state.move_speed = 100.0;
    }
    ai.attach_navigator(NavBridge::build(
        &[],
        &collision,
        &ohl_nav::BuildLimits::default(),
        NavBridgeLimits::default(),
    ));
    for _ in 0..32 {
        ai.tick(&mut world, &SightContext::tracing(&collision), 0.01);
    }
    let moved = world.get::<&Actor>(entity).unwrap();
    assert!(ai.script_navigation_stats().direct_steps > 0);
    assert!(
        moved.origin.x > 10.0,
        "the terminal route actually advanced"
    );
    let state = world.get::<&MonsterAi>(entity).unwrap();
    assert!(
        state.route.is_finished(),
        "the terminal route actually arrived"
    );
    assert_eq!(state.route.goal, goal);
    assert!(
        (moved.origin.z - 20.0).abs() < 0.1,
        "terminal query goal is not lifted a second time"
    );
}

#[test]
fn anchor_domain_sight_projects_for_the_owner_and_preserves_stance_and_literal_points() {
    use ohl_ai::{AiWorld, Conditions, DefaultBrain, SightContext};
    for hull in [ohl_physics::Hull::Standing, ohl_physics::Hull::Crouched] {
        for stance in [ohl_physics::Hull::Standing, ohl_physics::Hull::Crouched] {
            let mut ai = AiWorld::new(9);
            let brain = ai.register_brain(Box::new(DefaultBrain::default()));
            let mut world = ohl_game::hecs::World::new();
            let mut owner_actor = Actor::new(ohl_ai::Classification::HumanMilitary, Vec3::ZERO);
            owner_actor.hull = hull;
            let owner = ohl_ai::spawn_monster(&mut world, owner_actor, brain);
            world.insert_one(owner, ohl_ai::ScriptHold).unwrap();
            let mut target_actor = Actor::new(
                ohl_ai::Classification::Player,
                Vec3::new(100.0, 0.0, stance.foot_offset()),
            )
            .as_client();
            target_actor.hull = stance;
            let target = ohl_ai::spawn_actor(&mut world, target_actor);
            ai.tick(&mut world, &SightContext::default(), 0.01);
            let state = world.get::<&MonsterAi>(owner).unwrap();
            assert!(
                state.conditions.contains(Conditions::SEE_ENEMY),
                "real sight acquired the generated target"
            );
            let memory = state.memory.expect("visible target");
            assert_eq!(memory.entity, target);
            let expected = owner_actor
                .body_frame
                .anchor_to_query(hull, target_actor.navigation_anchor());
            assert_eq!(
                memory.last_known_position, expected,
                "fresh sight uses the receiving hull, not target center"
            );
            assert_eq!(
                state.route,
                ohl_ai::Route::new(),
                "held actor has no invented route"
            );
            drop(state);
            // A sound can overwrite move_target while an unrelated route survives.
            let literal = Vec3::new(11.3, -8.125, -0.0);
            let route = ohl_ai::Route::straight_line(expected);
            world.get::<&mut MonsterAi>(owner).unwrap().route = route.clone();
            ai.emit_sound(ohl_ai::SoundEvent::new(
                ohl_ai::SoundKind::Danger,
                literal,
                512.0,
            ));
            ai.tick(&mut world, &SightContext::default(), 0.01);
            let state = world.get::<&MonsterAi>(owner).unwrap();
            assert_eq!(
                state.move_target.unwrap().to_array().map(f32::to_bits),
                literal.to_array().map(f32::to_bits)
            );
            assert_eq!(
                state.route, route,
                "sound does not identify route provenance"
            );
            drop(state);
            // A damage source with no Actor remains an absolute literal point.
            world.get::<&mut Actor>(target).unwrap().alive = false;
            world.get::<&mut MonsterAi>(owner).unwrap().memory = None;
            let source = world.spawn(());
            assert!(ai.apply_damage(ohl_ai::DamageEvent::new(owner, source, 1.0, literal)));
            ai.tick(&mut world, &SightContext::default(), 0.01);
            assert_eq!(
                world
                    .get::<&MonsterAi>(owner)
                    .unwrap()
                    .memory
                    .unwrap()
                    .last_known_position
                    .to_array()
                    .map(f32::to_bits),
                literal.to_array().map(f32::to_bits)
            );
        }
    }
}
