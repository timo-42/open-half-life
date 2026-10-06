//! Generated spawn-floor and held-script synchronization controls.
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

/// How close to [`FLOOR_Z`] a monster standing on the floor is: a hull
/// trace stops `ohl_physics::DIST_EPSILON` (1/32) short of the plane it
/// hits, so feet dropped onto the floor rest that far above it.
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
            [0.0, 0.0, 17.0],
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
        [200.0, 0.0, 36.0],
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
        [200.0, 0.0, 36.0],
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
fn a_monster_placed_above_its_floor_stands_on_it() {
    let (anchor, actor, query) = spawned(&monster("monster_scientist", [100.0, 0.0, 17.0], 0.0));
    assert_eq!(anchor, actor, "floor landing never writes a lifted Actor");
    assert!((anchor.z - FLOOR_Z).abs() < ON_FLOOR);
    assert!((query.z - anchor.z - 36.0).abs() < 1e-3, "one query offset");
    assert!((anchor.x - 100.0).abs() < 1e-3 && anchor.y.abs() < 1e-3);
}

#[test]
fn an_authored_raised_anchor_drops_without_preserving_a_second_ai_position() {
    let (anchor, actor, query) = spawned(&monster("monster_zombie", [100.0, 0.0, 36.0], 0.0));
    assert_eq!(anchor, actor);
    assert!((anchor.z - FLOOR_Z).abs() < ON_FLOOR);
    assert!((query.z - 36.0).abs() < ON_FLOOR);
}

#[test]
fn the_query_offset_follows_the_species_hull() {
    for (name, raised, half) in [
        ("monster_gargantua", 50.0, 32.0),
        ("monster_headcrab", 10.0, 18.0),
    ] {
        let (anchor, actor, query) = spawned(&monster(name, [100.0, 0.0, raised], 0.0));
        assert_eq!(anchor, actor);
        assert!((anchor.z - FLOOR_Z).abs() < ON_FLOOR);
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
    assert!((anchor.z - FLOOR_Z).abs() < ON_FLOOR);
    assert!((query.z - anchor.z - 36.0).abs() < 1e-3);
}
