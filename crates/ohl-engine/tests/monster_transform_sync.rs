//! Generated authored-spawn and held-script synchronization controls.
//! Actor and Transform keep the same authored model anchor. The derived
//! BodyFrame enters only hull/navigation queries; it is not a second position.
//! Main's held walk/turn/save cases remain covered under that single convention.

use ohl_engine::test_support::{
    AI_MAP, actor_origin, ai_room_bsp, entity_block, entity_of_classname, monster_entities,
};
use ohl_engine::{Game, Input, MemoryAssets, StudioAnim};
use ohl_game::registry::Transform;

/// The room's floor height (`ai_room_bsp`).
const FLOOR_Z: f32 = 0.0;

/// Existing movement checks allow the collision trace epsilon above the floor.
/// Grounded fixtures author that clearance directly; spawning does not settle.
const ON_FLOOR: f32 = 0.05;

/// The room's entity block: a worldspawn, a player start facing `+X`, and
/// whatever the test adds.
fn entities(extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {extra}"
    )
}

/// A `monster_*` entity block at `origin`, facing `yaw`.
fn monster(classname: &str, origin: [f32; 3], yaw: f32) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"{yaw}\"\n}}\n",
        origin[0], origin[1], origin[2]
    )
}

fn game_from(block: &str) -> (MemoryAssets, Game) {
    let bytes = ai_room_bsp(block, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads");
    (assets, game)
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(ohl_engine::TICK_SECONDS, &input);
    }
}

fn transform_origin(game: &Game, entity: ohl_game::hecs::Entity) -> ohl_ai::Vec3 {
    game.registry()
        .world
        .get::<&Transform>(entity)
        .map(|transform| transform.origin)
        .unwrap_or_default()
}

/// The single monster's model anchor, Actor anchor and derived query center.
fn spawned(block: &str) -> (ohl_ai::Vec3, ohl_ai::Vec3, ohl_ai::Vec3) {
    let (_assets, game) = game_from(&entities(block));
    let entity = monster_entities(&game)[0];
    let query = game
        .registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .unwrap()
        .query_origin();
    (
        transform_origin(&game, entity),
        actor_origin(&game, entity),
        query,
    )
}

/// A model-backed guard whose script starts automatically, using only
/// project-authored room and model bytes.
fn scripted_game(classname: &str, move_to: &str) -> (MemoryAssets, Game) {
    let block = entities(&format!(
        "{}{}{}",
        entity_block(
            "monster_barney",
            [0.0, 0.0, ohl_physics::DIST_EPSILON],
            90.0,
            &[("targetname", "ohl_guard")],
        ),
        entity_block(
            classname,
            [160.0, 0.0, 0.0],
            180.0,
            &[
                ("targetname", "ohl_script"),
                ("m_iszEntity", "ohl_guard"),
                ("m_iszIdle", "ohl_wait"),
                ("m_iszPlay", "ohl_action"),
                ("m_fMoveTo", move_to),
            ],
        ),
        entity_block("trigger_auto", [0.0; 3], 0.0, &[("target", "ohl_script")]),
    ));
    let bytes = ai_room_bsp(&block, false);
    let (model, _) = ohl_formats::test_support::build_minimal_mdl10_with_sequences(&[
        "idle",
        "ohl_wait",
        "ohl_action",
    ]);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    assets.insert("models/barney.mdl", model);
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the script room loads");
    (assets, game)
}

fn assert_held_transform_matches_actor(game: &Game, entity: ohl_game::hecs::Entity) {
    let actor = game.registry().world.get::<&ohl_ai::Actor>(entity).unwrap();
    let transform = game.registry().world.get::<&Transform>(entity).unwrap();
    assert!(
        game.registry()
            .world
            .get::<&ohl_ai::ScriptHold>(entity)
            .is_ok(),
        "check the model while the script still possesses the guard"
    );
    assert!(
        (transform.origin - actor.origin).length() < 1e-3,
        "a held monster's rendered position must follow its current AI position"
    );
    assert!(
        (transform.angles.y - actor.yaw).abs() < 1e-3,
        "the model must turn too"
    );
}

#[test]
fn scripted_walks_and_runs_move_the_model_while_possessed() {
    for classname in ["scripted_sequence", "aiscripted_sequence"] {
        for move_to in ["1", "2"] {
            let (_assets, mut game) = scripted_game(classname, move_to);
            let guard = entity_of_classname(&game, "monster_barney").unwrap();
            let spawn = transform_origin(&game, guard);
            tick(&mut game, 5);
            for _ in 0..50 {
                tick(&mut game, 1);
                assert_held_transform_matches_actor(&game, guard);
            }
            let position = transform_origin(&game, guard);
            assert!(position.x > spawn.x + 16.0, "the model visibly advances");
            assert!((position.z - FLOOR_Z).abs() < ON_FLOOR);
            assert!(
                game.registry()
                    .world
                    .get::<&StudioAnim>(guard)
                    .unwrap()
                    .cycle
                    > 0.0,
                "the model's animation advances alongside its placement"
            );
        }
    }
}

#[test]
fn a_scripted_turn_updates_the_model_while_possessed() {
    let (_assets, mut game) = scripted_game("scripted_sequence", "5");
    let guard = entity_of_classname(&game, "monster_barney").unwrap();
    let spawn = transform_origin(&game, guard);
    tick(&mut game, 10);
    assert_held_transform_matches_actor(&game, guard);
    let transform = game.registry().world.get::<&Transform>(guard).unwrap();
    assert!(transform.angles.y > 100.0, "the model visibly turns");
    assert_eq!(transform.origin, spawn, "turning keeps the feet in place");
}

#[test]
fn a_save_during_a_scripted_walk_preserves_the_moving_placement() {
    let (assets, mut game) = scripted_game("scripted_sequence", "1");
    let guard = entity_of_classname(&game, "monster_barney").unwrap();
    let spawn = actor_origin(&game, guard);
    tick(&mut game, 100);
    let before_save = actor_origin(&game, guard);
    assert!(before_save.x > spawn.x + 16.0, "the guard has walked");
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let mut loaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let restored = entity_of_classname(&loaded, "monster_barney").unwrap();
    assert_eq!(actor_origin(&loaded, restored), before_save);
    assert_eq!(
        transform_origin(&loaded, restored),
        transform_origin(&game, guard)
    );
    assert_eq!(loaded.active_script_count(), 1);
    for _ in 0..50 {
        tick(&mut loaded, 1);
        assert_held_transform_matches_actor(&loaded, restored);
    }
    assert!(actor_origin(&loaded, restored).x > before_save.x + 16.0);
}

#[test]
fn a_walking_monsters_transform_tracks_its_actor() {
    let (_assets, mut game) = game_from(&entities(&monster(
        "monster_zombie",
        [200.0, 0.0, ohl_physics::DIST_EPSILON],
        180.0,
    )));
    let entity = monster_entities(&game)[0];
    let start = transform_origin(&game, entity);
    tick(&mut game, 400);
    let anchor = transform_origin(&game, entity);
    assert!(
        (anchor - start).length() > 1.0,
        "the monster actually walked"
    );
    assert_eq!(
        anchor,
        actor_origin(&game, entity),
        "moving model and AI share one anchor"
    );
    assert!((anchor.z - FLOOR_Z).abs() < ON_FLOOR);
}

#[test]
fn actor_matches_transform_immediately_after_a_save_load_boundary() {
    let (assets, mut game) = game_from(&entities(&monster(
        "monster_zombie",
        [200.0, 0.0, ohl_physics::DIST_EPSILON],
        180.0,
    )));
    let entity = monster_entities(&game)[0];
    let spawn = actor_origin(&game, entity);
    tick(&mut game, 400);
    let before = actor_origin(&game, entity);
    assert!(
        (before - spawn).length() > 1.0,
        "the saved actor moved from spawn"
    );
    let bytes = game.save_bytes(1_700_000_000).expect("save");
    let loaded = Game::load_bytes(&assets, &bytes).expect("load");
    let restored = monster_entities(&loaded)[0];
    assert_eq!(actor_origin(&loaded, restored), before);
    assert_eq!(transform_origin(&loaded, restored), before);
}

#[test]
fn a_monster_placed_above_its_floor_retains_its_authored_anchor() {
    let (anchor, actor, query) = spawned(&monster("monster_scientist", [100.0, 0.0, 17.0], 0.0));
    assert_eq!(anchor, actor, "spawning never writes a lifted Actor");
    assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 17.0));
    assert!((query.z - anchor.z - 36.0).abs() < 1e-3, "one query offset");
    assert!((anchor.x - 100.0).abs() < 1e-3 && anchor.y.abs() < 1e-3);
}

#[test]
fn an_authored_raised_anchor_is_not_a_second_ai_position() {
    let (anchor, actor, query) = spawned(&monster("monster_zombie", [100.0, 0.0, 36.0], 0.0));
    assert_eq!(anchor, actor);
    assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 36.0));
    assert!((query.z - anchor.z - 36.0).abs() < ON_FLOOR);
}

#[test]
fn the_query_offset_follows_the_species_hull() {
    for (name, raised, half) in [
        ("monster_gargantua", 50.0, 32.0),
        ("monster_headcrab", 10.0, 18.0),
    ] {
        let (anchor, actor, query) = spawned(&monster(name, [100.0, 0.0, raised], 0.0));
        assert_eq!(anchor, actor);
        assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, raised));
        assert!((query.z - anchor.z - half).abs() < 1e-3);
    }
}

#[test]
fn a_flier_keeps_its_placement() {
    let (anchor, actor, query) = spawned(&monster(
        "monster_alien_controller",
        [100.0, 0.0, 120.0],
        0.0,
    ));
    assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 120.0));
    assert_eq!(anchor, actor);
    assert_eq!(query, actor);
}

#[test]
fn a_barnacle_stays_on_its_ceiling() {
    let (anchor, actor, query) = spawned(&monster("monster_barnacle", [100.0, 0.0, 255.0], 0.0));
    assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 255.0));
    assert_eq!(anchor, actor);
    assert!(
        query.z < actor.z,
        "ceiling proxy ends at the unchanged anchor"
    );
}

#[test]
fn a_centred_origin_prop_keeps_its_placement() {
    let generic = |model: &str| {
        spawned(&entity_block(
            "monster_generic",
            [100.0, 0.0, 60.0],
            0.0,
            &[("model", model)],
        ))
    };
    for model in ohl_engine::ai::CENTRED_ORIGIN_MODELS
        .into_iter()
        .chain(["MODELS/Holo.mdl"])
    {
        let (anchor, actor, _) = generic(model);
        assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 60.0));
        assert_eq!(anchor, actor);
    }
    let (anchor, actor, query) = generic("models/ohl_prop.mdl");
    assert_eq!(anchor, actor);
    assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 60.0));
    assert!((query.z - anchor.z - 36.0).abs() < 1e-3);
}

#[test]
fn a_centred_origin_prop_uses_its_anchor_as_the_query_center() {
    for model in ohl_engine::ai::CENTRED_ORIGIN_MODELS
        .into_iter()
        .chain(["MODELS/Holo.mdl"])
    {
        let (assets, game) = game_from(&entities(&entity_block(
            "monster_generic",
            [100.0, 0.0, 60.0],
            0.0,
            &[("model", model)],
        )));
        let assert_centered = |game: &Game| {
            let entity = entity_of_classname(game, "monster_generic").expect("the prop spawned");
            let actor = game.registry().world.get::<&ohl_ai::Actor>(entity).unwrap();
            let anchor = transform_origin(game, entity);
            assert_eq!(anchor, ohl_ai::Vec3::new(100.0, 0.0, 60.0));
            assert_eq!(actor.origin, anchor);
            assert_eq!(
                actor.query_origin(),
                anchor,
                "the named prop has a centered query"
            );
            assert_eq!(actor.fallback_damage_bounds(), actor.hull.bounds());
        };
        assert_centered(&game);
        let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
        let loaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
        assert_centered(&loaded);
    }
}

/// Independent metadata: one valid model eye and a custom model-bottom frame.
fn authored_anchor_model(bottom: f32) -> Vec<u8> {
    let (mut model, _) = ohl_formats::test_support::build_minimal_mdl10();
    for (offset, values) in [
        (76, [3.0_f32, 4.0, 40.0]),
        (88, [-16.0, -16.0, bottom]),
        (100, [16.0, 16.0, bottom + 72.0]),
    ] {
        for (axis, value) in values.into_iter().enumerate() {
            model[offset + axis * 4..offset + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    model
}

/// Qualify the independently authored placement before asserting retention.
/// Constructing this query value never writes the actual actor or its state.
fn authored_anchor_prerequisites(
    game: &Game,
    entity: ohl_game::hecs::Entity,
    anchor: ohl_ai::Vec3,
) {
    use ohl_ai::Vec3;
    let actual = *game.registry().world.get::<&ohl_ai::Actor>(entity).unwrap();
    assert!(actual.alive);
    let authored = ohl_ai::Actor {
        origin: anchor,
        ..actual
    };
    let collision = game.monster_collision().unwrap();
    let query = authored.query_origin();
    let occupancy = collision.trace(authored.hull, query, query);
    assert!(
        !occupancy.start_solid && !occupancy.all_solid,
        "authored proxy is clear"
    );
    let fall = collision.trace(authored.hull, query, query - Vec3::Z * 200.0);
    assert!(!fall.start_solid && !fall.all_solid && fall.fraction < 1.0);
    assert!(
        fall.plane_normal.z > 0.9 && fall.end_pos.z < query.z - 1.0,
        "an ordinary lower floor is genuinely reachable"
    );
    let ai = game
        .registry()
        .world
        .get::<&ohl_ai::MonsterAi>(entity)
        .unwrap();
    assert!(ai.route.is_finished() && ai.route.waypoints.is_empty());
    assert!(
        ai.move_speed.abs() <= 0.0,
        "no ordinary movement supplied the anchor"
    );
    assert_eq!(actual.view_ofs, Vec3::new(3.0, 4.0, 40.0));
}

fn assert_authored_anchor(game: &Game, entity: ohl_game::hecs::Entity, anchor: ohl_ai::Vec3) {
    assert_eq!(
        actor_origin(game, entity),
        anchor,
        "AUTHORED SPAWN PRIMARY: retain the map or maker anchor"
    );
    assert_eq!(transform_origin(game, entity), anchor);
}

#[test]
fn authored_spawn_map_keeps_raised_feet_and_custom_bottoms_through_idle_and_save() {
    use ohl_ai::{BodyFrame, Vec3};
    for (classname, bottom, height) in [
        ("monster_barney", 0.0_f32, 40.0),
        ("monster_generic", 0.0, 80.0),
        ("monster_generic", -36.0, 80.0),
        ("monster_generic", -52.0, 80.0),
    ] {
        let anchor = Vec3::new(128.0, 64.0, height);
        let block = entities(&entity_block(
            classname,
            anchor.to_array(),
            0.0,
            &[("model", "models/ohl-authored-anchor.mdl")],
        ));
        let bytes = ai_room_bsp(&block, false);
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes);
        assets.insert(
            "models/ohl-authored-anchor.mdl",
            authored_anchor_model(bottom),
        );
        let mut game = Game::load(&assets, AI_MAP).unwrap();
        let entity = entity_of_classname(&game, classname).unwrap();
        authored_anchor_prerequisites(&game, entity, anchor);
        let actor = *game.registry().world.get::<&ohl_ai::Actor>(entity).unwrap();
        let frame = if classname == "monster_generic" {
            BodyFrame::ModelBottom(bottom)
        } else {
            BodyFrame::Feet
        };
        assert_eq!(actor.body_frame, frame);
        let (min, _) = actor.hull.bounds();
        let expected_query = anchor + Vec3::Z * (bottom - min.z);
        let authored = ohl_ai::Actor {
            origin: anchor,
            ..actor
        };
        assert_eq!(authored.query_origin(), expected_query);
        assert!(
            (frame.world_bounds(actor.hull, anchor).0.z - (height + bottom)).abs() <= f32::EPSILON
        );
        assert_authored_anchor(&game, entity, anchor);
        tick(&mut game, 1);
        assert_authored_anchor(&game, entity, anchor);
        let saved = game.save_bytes(1_700_000_000).unwrap();
        let mut loaded = Game::load_bytes(&assets, &saved).unwrap();
        let restored = entity_of_classname(&loaded, classname).unwrap();
        assert_authored_anchor(&loaded, restored, anchor);
        tick(&mut loaded, 1);
        assert_authored_anchor(&loaded, restored, anchor);
        assert_eq!(
            loaded
                .registry()
                .world
                .get::<&ohl_ai::Actor>(restored)
                .unwrap()
                .view_ofs,
            actor.view_ofs
        );
    }
}

#[test]
fn authored_spawn_maker_use_keeps_the_child_anchor_through_idle_and_save() {
    use ohl_ai::Vec3;
    use ohl_game::registry::{Button, MoverState};
    let anchor = Vec3::new(128.0, 64.0, 48.0);
    let block = entities(&format!(
        "{}{}",
        entity_block(
            "func_button",
            [-64.0, 0.0, 36.0],
            0.0,
            &[
                ("targetname", "anchor_button"),
                ("target", "anchor_maker"),
                ("speed", "100"),
                ("wait", "10")
            ]
        ),
        entity_block(
            "monstermaker",
            anchor.to_array(),
            0.0,
            &[
                ("targetname", "anchor_maker"),
                ("monstertype", "monster_barney"),
                ("monstercount", "1"),
                ("m_imaxlivechildren", "1"),
                ("delay", "0.01")
            ]
        ),
    ));
    let bytes = ai_room_bsp(&block, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes);
    assets.insert("models/barney.mdl", authored_anchor_model(0.0));
    let mut game = Game::load(&assets, AI_MAP).unwrap();
    let button = game.registry().find("anchor_button")[0];
    assert_eq!(
        game.registry().world.get::<&Button>(button).unwrap().state,
        MoverState::Closed
    );
    tick(&mut game, 5);
    assert_eq!(game.monster_count(), 0, "the maker is not auto-activated");
    game.tick(
        ohl_engine::TICK_SECONDS,
        &Input {
            use_pressed: true,
            ..Input::default()
        },
    );
    assert_ne!(
        game.registry().world.get::<&Button>(button).unwrap().state,
        MoverState::Closed,
        "ordinary player use really activated the button"
    );
    for _ in 0..100 {
        if game.monster_count() != 0 {
            break;
        }
        tick(&mut game, 1);
    }
    assert_eq!(
        game.monster_count(),
        1,
        "the ordinary target chain creates one child"
    );
    let child = entity_of_classname(&game, "monster_barney").unwrap();
    authored_anchor_prerequisites(&game, child, anchor);
    assert_eq!(
        game.registry()
            .world
            .get::<&ohl_ai::Actor>(child)
            .unwrap()
            .body_frame,
        ohl_ai::BodyFrame::Feet
    );
    assert_authored_anchor(&game, child, anchor);
    tick(&mut game, 1);
    assert_authored_anchor(&game, child, anchor);
    let saved = game.save_bytes(1_700_000_000).unwrap();
    let mut loaded = Game::load_bytes(&assets, &saved).unwrap();
    let restored = entity_of_classname(&loaded, "monster_barney").unwrap();
    assert_authored_anchor(&loaded, restored, anchor);
    tick(&mut loaded, 1);
    assert_authored_anchor(&loaded, restored, anchor);
    assert_eq!(loaded.monster_count(), 1);
}
