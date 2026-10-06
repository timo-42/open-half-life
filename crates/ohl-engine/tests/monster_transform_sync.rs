//! A monster's `ohl_ai::Actor` (sensing, navigation, attacks) and its
//! `Transform` (rendering, and what the hitbox index `crate::combat::
//! rebuild_hitbox_index` rebuilds from) must never drift apart. `Actor` is
//! the centre of the monster's hull and `Transform` its feet, so for a
//! walker the two differ by exactly its `ohl_engine::HullLift`:
//!
//! - At spawn, a walker's feet drop onto the floor under where the map put
//!   it (M9.59), and its `Actor` stands its lift above them; a flier, which
//!   has no feet, keeps the map's placement for both.
//! - Every step, phase 8b (`crate::systems::Systems::sync_monster_transforms`)
//!   copies a walking monster's freshly-moved `Actor` onto its `Transform`,
//!   so the rendered model — and the hitbox index the next step's phase 5
//!   rebuilds — actually follows the route `AiWorld::tick` (phase 8) just
//!   advanced, instead of staying pinned at wherever the monster last stood
//!   (the model, and a shot's hit test, never used to move at all).
//! - Right after `Game::restore` loads a save, `Systems::
//!   sync_actor_from_transforms` copies the just-restored `Transform` back
//!   onto `Actor`, so a reloaded monster's next think step starts from the
//!   save's own position rather than from `ohl_ai::attach_monsters`'s
//!   spawn-time default.
//!
//! Scripted walks and turns must reach `Transform` while the script still
//! holds its monster, including when a save resumes in the middle of a walk.
//!
//! Reuses `ohl_engine::test_support::ai_room_bsp`, the same fixture
//! `tests/ai_wiring.rs` builds its rooms from. No bytes here come from any
//! game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    AI_MAP, actor_origin, ai_room_bsp, entity_block, entity_of_classname, monster_entities,
};
use ohl_engine::{Game, HullLift, Input, MemoryAssets, StudioAnim};
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

/// `entity`'s `HullLift`, or `None` when it carries none.
fn lift_of(game: &Game, entity: ohl_game::hecs::Entity) -> Option<f32> {
    game.registry()
        .world
        .get::<&HullLift>(entity)
        .ok()
        .map(|lift| lift.0)
}

/// The single monster `block` spawns: its feet, its `Actor` and its lift,
/// straight after load.
fn spawned(block: &str) -> (ohl_ai::Vec3, ohl_ai::Vec3, Option<f32>) {
    let (_assets, game) = game_from(&entities(block));
    let entity = monster_entities(&game)[0];
    (
        transform_origin(&game, entity),
        actor_origin(&game, entity),
        lift_of(&game, entity),
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
    let lift = lift_of(game, entity).expect("the guard carries its lift");
    assert!(
        game.registry()
            .world
            .get::<&ohl_ai::ScriptHold>(entity)
            .is_ok(),
        "check the model while the script still possesses the guard"
    );
    assert!(
        (transform.origin + ohl_ai::Vec3::Z * lift - actor.origin).length() < 1e-3,
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

/// A monster with nowhere it needs to see the player still wanders
/// (`ai_wiring.rs`'s `a_map_without_nodes_still_moves_monsters` already
/// covers that its `Actor` moves): this asserts its `Transform` — the
/// rendered model's own position, and what the hitbox index reads — tracks
/// that move step for step rather than staying pinned at spawn.
#[test]
fn a_walking_monsters_transform_tracks_its_actor() {
    let block = entities(&monster("monster_zombie", [200.0, 0.0, 36.0], 180.0));
    let (_assets, mut game) = game_from(&block);
    let entity = monster_entities(&game)[0];
    let start = transform_origin(&game, entity);

    tick(&mut game, 400);

    let actor = actor_origin(&game, entity);
    let transform = transform_origin(&game, entity);
    let lift = lift_of(&game, entity).expect("a walker carries its lift");
    assert!(
        (transform - start).length() > 1.0,
        "the monster must actually have walked for this test to mean anything"
    );
    assert!(
        (transform + ohl_ai::Vec3::Z * lift - actor).length() < 1e-3,
        "the rendered Transform must track the AI's own Actor::origin, its \
         lift below it (Transform {transform:?}, Actor {actor:?}, lift \
         {lift}) — a route that moves the monster must move the model and \
         the hitbox index with it"
    );
    assert!(
        (transform.z - FLOOR_Z).abs() < ON_FLOOR,
        "a walker on a flat floor keeps its feet on it while it walks \
         (Transform {transform:?})"
    );
}

/// A monster that has walked away from its map spawn point, saved and
/// reloaded, comes back with `Actor::origin`/`yaw` at exactly the save's
/// `Transform`, not at `ohl_ai::attach_monsters`'s spawn-time default —
/// otherwise a reloaded monster's very next think step senses, navigates
/// and attacks from the wrong place.
#[test]
fn actor_matches_transform_immediately_after_a_save_load_boundary() {
    let block = entities(&monster("monster_zombie", [200.0, 0.0, 36.0], 180.0));
    let (assets, mut game) = game_from(&block);
    let entity = monster_entities(&game)[0];
    let spawn = actor_origin(&game, entity);

    tick(&mut game, 400);
    let actor_before_save = actor_origin(&game, entity);
    assert!(
        (actor_before_save - spawn).length() > 1.0,
        "the monster must actually have walked away from its map spawn \
         for this test to catch attach_monsters's spawn default leaking \
         back in"
    );

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the mid-walk save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the mid-walk save loads");

    let reloaded_entity = monster_entities(&reloaded)[0];
    let actor = actor_origin(&reloaded, reloaded_entity);
    let transform = transform_origin(&reloaded, reloaded_entity);
    let lift = lift_of(&reloaded, reloaded_entity).expect("the lift is re-derived at load");
    assert_eq!(
        actor,
        transform + ohl_ai::Vec3::Z * lift,
        "immediately after load, every monster's Actor::origin must stand \
         its lift above its just-restored Transform::origin, not at the \
         map's spawn point"
    );
    assert!(
        (actor - spawn).length() > 1.0,
        "the restored Actor::origin must be the save's walked-to position, \
         not silently equal to the map spawn point by coincidence"
    );
}

/// A monster the map placed above its floor stands on it: its model's feet
/// on the floor, its `Actor` a standing hull's half-height (36) above them.
/// On a real map this was guards and scientists drawn 17 units up.
#[test]
fn a_monster_placed_above_its_floor_stands_on_it() {
    let (feet, actor, lift) = spawned(&monster("monster_scientist", [100.0, 0.0, 17.0], 0.0));
    assert_eq!(
        lift,
        Some(36.0),
        "a standing hull's centre is 36 above its feet"
    );
    assert!(
        (feet.z - FLOOR_Z).abs() < ON_FLOOR,
        "the scientist's feet must be on the floor, not where the map placed them ({feet:?})"
    );
    assert!(
        (actor - (feet + ohl_ai::Vec3::Z * 36.0)).length() < 1e-3,
        "the AI reads the hull centre, 36 above the feet ({actor:?})"
    );
    assert!(
        (feet.x - 100.0).abs() < 1e-3 && feet.y.abs() < 1e-3,
        "dropping onto the floor moves a monster straight down only ({feet:?})"
    );
}

/// A monster placed hull-centred (as this project's own fixtures place
/// them) keeps exactly the AI position it always had; only its model comes
/// down onto the floor.
#[test]
fn a_hull_centred_monster_keeps_its_ai_position() {
    let (feet, actor, _) = spawned(&monster("monster_zombie", [100.0, 0.0, 36.0], 0.0));
    assert!(
        (feet.z - FLOOR_Z).abs() < ON_FLOOR,
        "feet on the floor ({feet:?})"
    );
    assert!(
        (actor.z - 36.0).abs() < ON_FLOOR,
        "AI unchanged at 36 ({actor:?})"
    );
}

/// The lift is the species' own hull's half-height: a large hull's 32, a
/// crouched hull's 18.
#[test]
fn the_lift_follows_the_species_hull() {
    let (feet, actor, lift) = spawned(&monster("monster_gargantua", [100.0, 0.0, 50.0], 0.0));
    assert_eq!(lift, Some(32.0));
    assert!((feet.z - FLOOR_Z).abs() < ON_FLOOR, "{feet:?}");
    assert!((actor.z - 32.0).abs() < ON_FLOOR, "{actor:?}");

    let (feet, actor, lift) = spawned(&monster("monster_headcrab", [100.0, 0.0, 10.0], 0.0));
    assert_eq!(lift, Some(18.0));
    assert!((feet.z - FLOOR_Z).abs() < ON_FLOOR, "{feet:?}");
    assert!((actor.z - 18.0).abs() < ON_FLOOR, "{actor:?}");
}

/// A flier has no feet: it keeps the map's placement, for its model and
/// its AI alike, and carries no lift.
#[test]
fn a_flier_keeps_its_placement() {
    let (feet, actor, lift) = spawned(&monster(
        "monster_alien_controller",
        [100.0, 0.0, 120.0],
        0.0,
    ));
    assert_eq!(lift, None);
    assert!((feet.z - 120.0).abs() < 1e-3, "{feet:?}");
    assert_eq!(feet, actor);
}

/// The barnacle hangs from its origin at the ceiling: it is neither
/// dropped nor lifted.
#[test]
fn a_barnacle_stays_on_its_ceiling() {
    let (feet, actor, lift) = spawned(&monster("monster_barnacle", [100.0, 0.0, 255.0], 0.0));
    assert_eq!(lift, None);
    assert!((feet.z - 255.0).abs() < 1e-3, "{feet:?}");
    assert_eq!(feet, actor);
}

/// A `monster_generic` showing one of the two models whose origin is their
/// centre (`ohl_engine::ai::CENTRED_ORIGIN_MODELS`) is drawn exactly where
/// the map placed it, with no lift, whatever the path's case or slashes;
/// the same prop with any other model stands on its floor.
#[test]
fn a_centred_origin_prop_keeps_its_placement() {
    let generic = |model: &str| {
        spawned(&format!(
            "{{\n\"classname\" \"monster_generic\"\n\"origin\" \"100 0 60\"\n\
             \"angle\" \"0\"\n\"model\" \"{model}\"\n}}\n"
        ))
    };
    for model in ohl_engine::ai::CENTRED_ORIGIN_MODELS
        .into_iter()
        .chain(["MODELS/Holo.mdl"])
    {
        let (feet, actor, lift) = generic(model);
        assert_eq!(lift, None, "{model}");
        assert!((feet.z - 60.0).abs() < 1e-3, "{model}: {feet:?}");
        assert_eq!(feet, actor, "{model}");
    }

    let (feet, actor, lift) = generic("models/ohl_prop.mdl");
    assert_eq!(lift, Some(36.0));
    assert!((feet.z - FLOOR_Z).abs() < ON_FLOOR, "{feet:?}");
    assert!((actor.z - 36.0).abs() < ON_FLOOR, "{actor:?}");
}
