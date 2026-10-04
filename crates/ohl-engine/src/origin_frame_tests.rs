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
        assert_eq!(
            actor.origin.to_array().map(f32::to_bits),
            game.player_origin().map(f32::to_bits)
        );
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
