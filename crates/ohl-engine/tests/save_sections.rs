//! M7.9 P4b: the five additive save sections (`SECTION_INVENTORY` 23,
//! `SECTION_ENTITY_COMBAT` 24, `SECTION_AI` 25, `SECTION_PROJECTILES` 26,
//! `SECTION_RNG` 27), exercised through the full `Game` loop.
//!
//! M7.13 adds `SECTION_MOVER_STATE` (28): a `func_tracktrain`'s mid-route
//! position, an active `trigger_camera` sequence, a `scripted_sequence` mid
//! possession and a `monstermaker`'s spawn counters, tested at the bottom
//! of this file, plus a `func_rotating`'s spin state — which
//! `SECTION_ENTITY_REGISTRY` (18) also carries, redundantly, as part of the
//! whole `Rotator` component. Tag 28's wire shape (including that
//! redundant copy, which is frozen in place) is pinned by
//! `crates/ohl-engine/tests/save_format_frozen.rs`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

// Exact float comparison is the point of the mover-position assertions
// below: the restored/continued position either lands exactly where it is
// expected or it does not, matching `camera_sequences.rs`'s own precedent.
#![allow(clippy::float_cmp)]

use ohl_combat::{ProjectileKind, WeaponId, hud_slot};
use ohl_engine::test_support::{
    AI_MAP, BREAKABLE_MAP, BREAKABLE_OBSTACLE_MAXS, MOMENTARY_DOOR_MAP, MOMENTARY_DOOR_NAME,
    OBSTACLE_NAME, PLATROT_MAP, PLATROT_NAME, ROT_BUTTON_MAP, ROT_BUTTON_NAME, ROTATING_DOOR_MAP,
    SCRIPT_MAP, actor_origin, ai_room_bsp, breakable_corridor_entities, entity_block,
    entity_of_classname, momentary_door_bsp, momentary_door_entities, monster_entities,
    obstacle_corridor_bsp, platrot_entities, queue_monster_damage, rot_button_bsp,
    rotating_door_bsp, rotating_door_entities, rotating_platform_bsp, script_game, script_room_bsp,
    script_room_entities,
};
use ohl_engine::{AssetSource, EngineError, Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};
use ohl_formats::test_support::build_minimal_mdl10;
use ohl_game::registry::{MoverState, PlatRot};

/// A room with a player start, a weapon and its ammo within pickup range of
/// the spawn, and one monster the test kills directly (`queue_monster_damage`)
/// rather than by aiming a shot at it.
fn entities() -> String {
    "{\n\"classname\" \"worldspawn\"\n}\n\
     {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 36\"\n\"angle\" \"0\"\n}\n\
     {\n\"classname\" \"weapon_357\"\n\"origin\" \"8 0 36\"\n}\n\
     {\n\"classname\" \"ammo_357\"\n\"origin\" \"-8 0 36\"\n}\n\
     {\n\"classname\" \"monster_human_grunt\"\n\"origin\" \"-160 160 36\"\n\"angle\" \"180\"\n}\n"
        .to_string()
}

fn game() -> Game {
    let bytes = ai_room_bsp(&entities(), false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
}

fn tick(game: &mut Game, input: &Input) {
    game.tick(TICK_SECONDS, input);
}

/// Drives `game` through firing a weapon, killing a monster, picking up
/// ammo and leaving a live projectile in flight, ready for a save.
fn play_out_a_combat_scenario(game: &mut Game) {
    // Phase 11 (pickups) runs every step, so the first tick already picks
    // up the weapon and the loose ammo sitting at the spawn's own origin.
    tick(game, &Input::default());
    assert!(game.inventory().has_weapon(WeaponId::Python));
    assert!(
        game.pickup_count() >= 2,
        "the weapon and the ammo box both count"
    );

    // Select the weapon (a fresh pickup's clip starts empty, matching a
    // real loadout: nothing auto-loads it), reload, then fire: the first
    // attack tick only draws the weapon (`Holstered` -> `Idle`), the
    // second is the one that actually cycles.
    tick(
        game,
        &Input {
            select_slot: Some(hud_slot(WeaponId::Python).slot),
            ..Input::default()
        },
    );
    tick(
        game,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    // Comfortably longer than the revolver's published reload time.
    for _ in 0..600 {
        tick(game, &Input::default());
    }
    assert!(
        game.inventory().clip(WeaponId::Python) > 0,
        "the reload must have loaded the clip"
    );
    tick(
        game,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    tick(
        game,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    assert!(game.weapon_fired_count() >= 1, "the weapon must have fired");

    // Kill the monster directly (no line of sight is set up between the
    // player and it), so this test's "killed a monster" leg is independent
    // of the hitscan aiming the fired shot above did.
    let monster = *monster_entities(game)
        .first()
        .expect("the fixture places exactly one monster");
    queue_monster_damage(game, monster, None, 10_000.0);
    tick(game, &Input::default());
    assert_eq!(game.monster_death_count(), 1);

    // A live projectile: nothing in this tree yet drives a weapon's
    // `SpawnProjectile` action or a monster's projectile attack end to
    // end (a known gap between the independent P1/P2/P3 packages, out of
    // this section's scope), so the test-only hook stands in for whichever
    // one eventually fills it.
    let spawned =
        game.debug_spawn_projectile(ProjectileKind::Rocket, [0.0, 0.0, 40.0], [200.0, 0.0, 0.0]);
    assert!(spawned.is_some());
    assert!(game.projectile_count() >= 1);
}

/// The core M7.9 P4b acceptance test: a save -> load -> save chain is
/// byte-identical after a run that fired a weapon, killed a monster,
/// picked up ammo and left a live projectile in flight.
#[test]
fn save_load_save_is_byte_identical_after_combat_pickup_and_projectile() {
    let mut game = game();
    play_out_a_combat_scenario(&mut game);

    let first = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&game_assets(), &first).expect("the save is read back");
    let second = reloaded
        .save_bytes(1_700_000_000)
        .expect("the reloaded game saves again");
    assert_eq!(first, second, "a save round trip is byte identical");

    // The typed sections actually carried the state, not just agreed with
    // themselves on re-encoding: a fresh, unrelated game would not.
    assert!(reloaded.inventory().has_weapon(WeaponId::Python));
    assert_eq!(reloaded.projectile_count(), game.projectile_count());
    assert_eq!(
        reloaded.monster_death_count(),
        0,
        "death count itself is not carried, only the entity/AI state is"
    );
}

fn game_assets() -> MemoryAssets {
    let bytes = ai_room_bsp(&entities(), false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes);
    assets
}

/// A save missing tags 23-27 entirely (a pre-M7.9-P4b file) still loads,
/// with every new section reading as its documented default.
#[test]
fn a_save_missing_the_new_sections_still_loads() {
    let mut game = game();
    play_out_a_combat_scenario(&mut game);
    let mut save = game.to_save(1_700_000_000);

    save.inventory = None;
    save.entity_combat = None;
    save.ai = None;
    save.projectiles = None;
    save.rng = None;

    let bytes = save
        .to_bytes()
        .expect("a save missing the new sections still encodes");
    let reloaded =
        Game::load_bytes(&game_assets(), &bytes).expect("an old-shaped save still loads");
    // The legacy `SECTION_PLAYER_CARRY` blob is still what restores the
    // weapon in this case, exactly as it did before M7.9 P4b existed.
    assert!(reloaded.inventory().has_weapon(WeaponId::Python));
    assert_eq!(
        reloaded.projectile_count(),
        0,
        "no SECTION_PROJECTILES: nothing to restore"
    );
}

/// Two `monster_generic` props naming the exact citable model paths
/// [`DeployableKind::Satchel`]/[`DeployableKind::Tripmine`] resolve to
/// (`default_deployable_model_path` in `crate::projectiles`, not
/// reachable directly from an integration test, but the literals are
/// published in `docs/FORMAT_SOURCES.md`): once the payload actually
/// publishes those `.mdl` assets, `ProjectileSystem::configure_models`
/// (run by `Systems::attach_level`, on every fresh load, exactly as it
/// would for a real map with a loaded satchel/tripmine model) wires them
/// up on its own, no test-only override needed.
fn entities_with_deployable_models() -> String {
    format!(
        "{}\
         {{\n\"classname\" \"monster_generic\"\n\"model\" \"models/w_satchel.mdl\"\n\
         \"origin\" \"-200 -200 -200\"\n}}\n\
         {{\n\"classname\" \"monster_generic\"\n\"model\" \"models/v_tripmine.mdl\"\n\
         \"origin\" \"-200 -220 -200\"\n}}\n",
        entities()
    )
}

fn game_assets_with_deployable_models() -> MemoryAssets {
    let bytes = ai_room_bsp(&entities_with_deployable_models(), false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes);
    let (satchel_mdl, _) = build_minimal_mdl10();
    let (tripmine_mdl, _) = build_minimal_mdl10();
    assets.insert("models/w_satchel.mdl", satchel_mdl);
    assets.insert("models/v_tripmine.mdl", tripmine_mdl);
    assets
}

fn game_with_deployable_models() -> Game {
    let bytes = ai_room_bsp(&entities_with_deployable_models(), false);
    let assets = game_assets_with_deployable_models();
    Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
}

/// M7.9 P4b's `SECTION_PROJECTILES` restores the plain `DeployableSet`/
/// `ProjectileSet` data; the model-backed stand-in entities (drawn,
/// damageable) are re-created by `restore_snapshot` itself, never
/// serialized. Places one satchel and one tripmine on a map that actually
/// published both kinds' models, saves, reloads, and checks the stand-ins
/// came back one-for-one with the restored deployables (drawn again, and
/// reachable by the player's own hitscan again), and that a save -> load
/// -> save chain including them is still byte-identical.
#[test]
fn deployable_stand_ins_survive_a_save_load_boundary() {
    let mut game = game_with_deployable_models();
    tick(&mut game, &Input::default());

    let satchel = game.debug_place_satchel([0.0, 40.0, 36.0]);
    assert!(satchel.is_some(), "the satchel must place");
    let tripmine = game.debug_place_tripmine([0.0, -40.0, 36.0], [0.0, 0.0, -1.0]);
    assert!(
        tripmine.is_some(),
        "the placement trace must find the floor"
    );

    assert_eq!(game.deployable_count(), 2);
    assert_eq!(
        game.deployable_stand_in_count(),
        2,
        "both placed deployables must have a stand-in entity, this map having \
         actually loaded both kinds' models"
    );

    let first = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&game_assets_with_deployable_models(), &first)
        .expect("the save is read back");

    assert_eq!(reloaded.deployable_count(), 2, "both deployables restore");
    assert_eq!(
        reloaded.deployable_stand_in_count(),
        2,
        "a restore must re-create a stand-in for every restored deployable, \
         not leave them undrawn and undamageable"
    );

    let second = reloaded
        .save_bytes(1_700_000_000)
        .expect("the reloaded game saves again");
    assert_eq!(
        first, second,
        "a save -> load -> save chain with placed deployables is byte-identical"
    );
}

/// A save written by a build that predates this fix (an ordinary
/// `SECTION_PROJECTILES` payload — the wire format never changed, since a
/// stand-in was never serialized to begin with) still loads, and now
/// additionally gets its stand-ins re-created where a pre-fix build would
/// have left the restored deployable undrawn and undamageable.
#[test]
fn a_pre_fix_save_still_loads_and_now_gets_its_stand_ins_back() {
    let mut game = game_with_deployable_models();
    tick(&mut game, &Input::default());
    game.debug_place_tripmine([0.0, -40.0, 36.0], [0.0, 0.0, -1.0])
        .expect("the placement trace finds the floor");

    // `to_save`/`SECTION_PROJECTILES` predates this fix and is unchanged by
    // it (see this module's own doc): the bytes below are exactly what a
    // pre-fix build would also have written.
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&game_assets_with_deployable_models(), &bytes)
        .expect("a save from before this fix still loads");
    assert_eq!(reloaded.deployable_count(), 1);
    assert_eq!(
        reloaded.deployable_stand_in_count(),
        1,
        "this fix re-creates the stand-in a pre-fix build would have left missing"
    );
}

/// A save whose `SECTION_AI` is present but fails to decode is rejected
/// outright rather than silently loading with defaults.
#[test]
fn a_corrupted_ai_section_fails_closed() {
    let game = game();
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let save = ohl_engine::GameSave::from_bytes(&bytes).expect("the original save reads back");

    // Re-encode a container with an intentionally malformed `SECTION_AI`
    // (25) so the resulting bytes still open (magic, table, digests all
    // consistent) but that one section cannot deserialize.
    let header = ohl_save::Header {
        game_version: String::new(),
        created_at_unix_secs: 1_700_000_000,
        map_identity: save.header.map.clone(),
        title: save.header.map.clone(),
        thumbnail: Vec::new(),
    };
    let mut writer = ohl_save::SaveWriter::begin(header);
    writer
        .add_section_serde(ohl_engine::save::SECTION_ENGINE_HEADER, &save.header)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_PLAYER_CARRY, &save.player)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_ENTITY_REGISTRY, &save.entities)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_SIMULATION, &save.simulation)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_GLOBAL_STATE, &save.globals)
        .unwrap();
    writer
        .add_section_serde(
            ohl_engine::save::SECTION_LIGHT_STYLE_TIME,
            &save.light_style_time,
        )
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_VIEW, &save.view)
        .unwrap();
    // A byte string that is not a valid `postcard` encoding of
    // `Vec<Option<AiSnapshot>>`.
    writer
        .add_section(ohl_engine::save::SECTION_AI, &[0xFF; 64])
        .unwrap();
    let corrupted = writer
        .finish(&ohl_save::Limits::default())
        .expect("the container still assembles");

    let result = Game::load_bytes(&game_assets(), &corrupted);
    assert!(matches!(result, Err(EngineError::SaveUnreadable)));
}

/// A fixed-seed scripted run continued after a save/load boundary produces
/// the same `ai_state_hash` as ticking the same total number of steps
/// uninterrupted.
#[test]
fn ai_state_hash_matches_across_a_save_load_boundary() {
    let total_ticks = 400;
    let split_at = 180;

    let mut uninterrupted = game();
    for _ in 0..total_ticks {
        uninterrupted.tick(TICK_SECONDS, &Input::default());
    }

    let mut continued = game();
    for _ in 0..split_at {
        continued.tick(TICK_SECONDS, &Input::default());
    }
    let bytes = continued
        .save_bytes(1_700_000_000)
        .expect("the mid-run save is written");
    let mut reloaded = Game::load_bytes(&game_assets(), &bytes).expect("the mid-run save loads");
    assert_eq!(
        reloaded.ai_state_hash(),
        continued.ai_state_hash(),
        "the AI state must already match right at the load boundary, before any further tick"
    );
    for _ in 0..(total_ticks - split_at) {
        reloaded.tick(TICK_SECONDS, &Input::default());
    }

    assert_eq!(
        reloaded.ai_state_hash(),
        uninterrupted.ai_state_hash(),
        "continuing a scripted run after a save/load must reproduce the same AI state"
    );
}

/// A restored route cursor at exactly the waypoint count is `Route`'s own
/// valid "finished" state (`ohl_ai::movement::Route::is_finished`), not an
/// out-of-range value: a fixed regression for a clamp that used to rewind
/// it to `len - 1`, un-finishing a completed route on every load and
/// breaking the save -> load -> save byte identity.
#[test]
fn a_finished_route_cursor_survives_a_save_load_boundary() {
    let mut game = game();
    game.tick(TICK_SECONDS, &Input::default());

    let monster = *monster_entities(&game)
        .first()
        .expect("the fixture places exactly one monster");
    let index = game
        .registry()
        .entities
        .iter()
        .position(|entity| *entity == monster)
        .expect("the monster is a registry entity");

    let mut save = game.to_save(1_700_000_000);
    let ai = save
        .ai
        .as_mut()
        .expect("SECTION_AI is always populated by this build")
        .get_mut(index)
        .expect("the monster's own slot")
        .as_mut()
        .expect("a thinking monster always has an AiSnapshot");
    // Two waypoints, cursor at `2`: the route has just finished, one step
    // past its last waypoint.
    ai.route_waypoints = vec![[0.0, 0.0, 0.0], [64.0, 0.0, 0.0]];
    ai.route_current = 2;

    let first = save.to_bytes().expect("the edited save still encodes");
    let reloaded = Game::load_bytes(&game_assets(), &first).expect("it reads back");
    let second = reloaded
        .save_bytes(1_700_000_000)
        .expect("the reloaded game saves again");
    assert_eq!(
        first, second,
        "a finished route's cursor must round-trip exactly, not rewind"
    );

    let restored_ai = reloaded
        .registry()
        .world
        .get::<&ohl_ai::MonsterAi>(monster)
        .expect("the monster still carries a MonsterAi");
    assert_eq!(restored_ai.route.current, 2);
    assert!(
        restored_ai.route.is_finished(),
        "the route must still report finished after the load"
    );
}

// --- M7.13: `SECTION_MOVER_STATE` (28) ------------------------------------

/// A `trigger_auto` that fires `target` as soon as the map has loaded,
/// matching every other test file's own copy of this fixture
/// (`camera_sequences.rs`, `scripted_sequences.rs`).
fn trigger_auto(target: &str) -> String {
    entity_block("trigger_auto", [0.0, 0.0, 0.0], 0.0, &[("target", target)])
}

/// A `trigger_changelevel` a completion `target` can name; firing it is
/// visible to the host as a `GameEvent::LevelChange`.
fn exit_trigger(name: &str) -> String {
    entity_block(
        "trigger_changelevel",
        [0.0, 0.0, 0.0],
        0.0,
        &[
            ("targetname", name),
            ("map", "ohlelsewhere"),
            ("landmark", "ohl_landmark"),
        ],
    )
}

/// The [`MemoryAssets`] a [`script_game`]-built `Game` (or a save loaded
/// against the same map) reads from.
fn script_game_assets(entities: &str) -> MemoryAssets {
    let bytes = script_room_bsp(entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes);
    assets
}

fn script_tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

/// A `func_tracktrain` on a straight three-node `path_track` chain, started
/// by a `trigger_auto`.
fn track_train_entities() -> String {
    format!(
        "{}{}{}{}{}",
        trigger_auto("tram"),
        entity_block(
            "func_tracktrain",
            [0.0, 96.0, 64.0],
            0.0,
            &[
                ("targetname", "tram"),
                ("target", "node1"),
                ("speed", "150"),
                ("height", "0"),
            ],
        ),
        entity_block(
            "path_track",
            [0.0, 96.0, 64.0],
            0.0,
            &[("targetname", "node1"), ("target", "node2")],
        ),
        entity_block(
            "path_track",
            [300.0, 96.0, 64.0],
            0.0,
            &[("targetname", "node2"), ("target", "node3")],
        ),
        entity_block(
            "path_track",
            [600.0, 96.0, 64.0],
            0.0,
            &[("targetname", "node3")],
        ),
    )
}

/// `entity`'s `TrackTrainState`-reported world position.
fn train_position(game: &Game, entity: ohl_game::hecs::Entity) -> [f32; 3] {
    game.registry()
        .world
        .get::<&ohl_game::TrackTrainState>(entity)
        .expect("the tracktrain still carries its runtime state")
        .position()
        .to_array()
}

/// A moving train mid-segment: save, load, and the resolved position round
/// trips exactly and the simulation continues along the exact same
/// trajectory afterward, not merely a byte-identical save.
#[test]
fn a_moving_train_round_trips_its_mid_segment_state_and_continues() {
    let entities = script_room_entities([-192.0, -192.0, 36.0], &track_train_entities());

    let mut uninterrupted = script_game(&entities);
    script_tick(&mut uninterrupted, 400);

    let mut continued = script_game(&entities);
    script_tick(&mut continued, 180);
    let train = entity_of_classname(&continued, "func_tracktrain").expect("the train spawned");
    let mid_position = train_position(&continued, train);
    // Confirms the fixture actually reached a moving, mid-segment state
    // before the save (not still sitting at node1).
    assert!(mid_position[0] > 1.0, "the train must already be moving");

    let bytes = continued
        .save_bytes(1_700_000_000)
        .expect("the mid-route save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the mid-route save loads");
    let train_r = entity_of_classname(&reloaded, "func_tracktrain").expect("the train restores");
    assert_eq!(
        train_position(&reloaded, train_r),
        mid_position,
        "the resolved position must round trip exactly across the load, \
         before any further tick"
    );

    let second = reloaded
        .save_bytes(1_700_000_000)
        .expect("the reloaded game saves again");
    assert_eq!(
        bytes, second,
        "a save -> load -> save chain with a moving train is byte-identical"
    );

    script_tick(&mut reloaded, 220);
    assert_eq!(
        train_position(&reloaded, train_r),
        train_position(
            &uninterrupted,
            entity_of_classname(&uninterrupted, "func_tracktrain")
                .expect("the uninterrupted run's train exists too")
        ),
        "continuing after the load must reproduce the same trajectory an \
         uninterrupted run takes, not just match the saved bytes"
    );
}

/// An active `trigger_camera` sequence saved mid-hold resumes active after
/// the load and fires its completion target exactly once across the whole
/// run (not zero, not twice).
#[test]
fn an_active_camera_sequence_round_trips_and_fires_its_completion_target_once() {
    const HOLD_SECONDS: f32 = 1.0;
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            trigger_auto("cam1"),
            entity_block(
                "trigger_camera",
                [96.0, -96.0, 64.0],
                0.0,
                &[
                    ("targetname", "cam1"),
                    ("target", "after_cam"),
                    ("wait", &HOLD_SECONDS.to_string()),
                ],
            ),
            exit_trigger("after_cam"),
        ),
    );
    let mut game = script_game(&entities);
    let mut fired = 0usize;
    for _ in 0..30 {
        for event in game.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    assert!(
        game.camera_sequence_active(),
        "the sequence must still be mid-hold before the save"
    );
    assert_eq!(fired, 0, "the hold has not elapsed yet");

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the mid-hold save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the mid-hold save loads");
    assert!(
        reloaded.camera_sequence_active(),
        "the sequence must resume active right after the load, not reset to dormant"
    );

    for _ in 0..400 {
        for event in reloaded.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    assert_eq!(
        fired, 1,
        "the completion target must fire exactly once across the whole run"
    );
    // Run well past completion: it must never fire again.
    for _ in 0..200 {
        for event in reloaded.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    assert_eq!(fired, 1);
}

/// A `scripted_sequence` saved mid-`Moving` resumes still possessing its
/// monster (not reset to dormant) from the same position, and still
/// completes, firing its target exactly once across the whole run.
#[test]
fn a_script_mid_moving_round_trips_and_still_completes_exactly_once() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            entity_block(
                "monster_barney",
                [0.0, 0.0, 36.0],
                0.0,
                &[("targetname", "ohl_guard")],
            ),
            entity_block(
                "scripted_sequence",
                [160.0, 0.0, 36.0],
                90.0,
                &[
                    ("targetname", "ohl_script"),
                    ("m_iszEntity", "ohl_guard"),
                    ("m_iszPlay", "ohl_action"),
                    ("m_iszIdle", "ohl_wait"),
                    ("m_fMoveTo", "1"),
                    ("target", "ohl_after"),
                ],
            ),
            trigger_auto("ohl_script") + &exit_trigger("ohl_after"),
        ),
    );
    let mut game = script_game(&entities);
    script_tick(&mut game, 5);
    assert_eq!(
        game.active_script_count(),
        1,
        "the script must possess the guard"
    );

    // Partway through the walk: comfortably before the guard reaches the
    // mark (160 units away), so the runner is still in `ScriptPhase::Moving`.
    script_tick(&mut game, 60);
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let mid_position = actor_origin(&game, guard).to_array();
    assert!(
        mid_position[0] > 0.5 && mid_position[0] < 160.0 - 32.0,
        "the guard must still be en route, not at its spawn or already at the mark: {mid_position:?}"
    );

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the mid-walk save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the mid-walk save loads");
    assert_eq!(
        reloaded.active_script_count(),
        1,
        "the script must still possess the guard right after the load"
    );
    // `mid_position` is otherwise unused directly (a monster's `Actor`
    // position is a separate, pre-existing gap this section does not
    // touch — see `crate::save_state`'s own "Monstermaker children are not
    // saved" precedent for the same class of documented limitation); what
    // this test actually exercises is that the *script's own* phase and
    // binding come back active rather than reset to `Dormant`, asserted
    // above, and that it still completes below.
    let _ = mid_position;

    let mut fired = 0usize;
    for _ in 0..1_200 {
        for event in reloaded.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    assert_eq!(
        fired, 1,
        "the resumed script must complete and fire its target exactly once"
    );
    assert_eq!(reloaded.script_completion_count(), 1);
}

/// A `monstermaker`'s spawn counters, and (as of M9.5,
/// `SECTION_MAKER_CHILDREN`, tag 29) the already-spawned live child entity
/// itself, both survive a save/load round trip: the quota it already spent
/// before the save is not forgotten, so it can only ever spawn up to
/// `monstercount` in total, across the load, and the child that already
/// existed is not lost either.
#[test]
fn a_monstermakers_counters_survive_a_save_load_round_trip() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "monstermaker",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("monstertype", "monster_headcrab"),
                ("monstercount", "2"),
                ("delay", "0.05"),
                ("spawnflags", "1"), // "Start On": see docs/FORMAT_SOURCES.md, "Monster definitions".
            ],
        ),
    );

    let mut game = script_game(&entities);
    // One tick already spawns the first child (a fresh `Spawner`'s timer
    // starts at zero); saving right here is deliberately *before* the
    // second `delay` interval elapses, so the counters this test is
    // actually about (`spawned_total` in particular) are the only thing
    // that can tell a correct restore apart from one that reset them.
    script_tick(&mut game, 1);
    assert_eq!(game.monster_count(), 1, "the first child must have spawned");

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    // `SECTION_MAKER_CHILDREN` (29, M9.5) recreates the already-spawned
    // child, ahead of the entity/combat/AI sections that go on to restore
    // its exact transform/health/AI state.
    assert_eq!(
        reloaded.monster_count(),
        1,
        "the already-spawned child is itself restored (SECTION_MAKER_CHILDREN, tag 29)"
    );

    script_tick(&mut reloaded, 400);
    assert_eq!(
        reloaded.monster_count(),
        2,
        "monstercount=2 total, and one was already spent before the save: \
         only one more may ever spawn, on top of the one already restored. \
         A reset spawned_total would let two more spawn instead of one, \
         reaching 3"
    );
}

/// A `monstermaker` with `m_imaxlivechildren=1` (only one live child
/// allowed at a time) that has already spawned its one allowed child before
/// the save must still respect that cap after a load: this is what tells a
/// correct restore — which relinks the recreated child onto
/// `ohl_ai::Spawner::children` (`ohl_ai::Spawner::restore_child`,
/// `crate::ai::AiState::finalize_maker_children`) — apart from one that
/// recreates the child entity but forgets to relink it, which would let a
/// second child spawn immediately after the load despite the live one
/// still being alive.
#[test]
fn a_makers_live_child_cap_is_still_respected_after_a_load() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "monstermaker",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("monstertype", "monster_headcrab"),
                ("monstercount", "3"),
                ("m_imaxlivechildren", "1"),
                ("delay", "0.05"),
                ("spawnflags", "1"), // "Start On".
            ],
        ),
    );

    let mut game = script_game(&entities);
    script_tick(&mut game, 1);
    assert_eq!(
        game.monster_count(),
        1,
        "the one allowed live child must have spawned"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert_eq!(
        reloaded.monster_count(),
        1,
        "the already-spawned child is restored"
    );

    // Enough ticks for many `delay` intervals to elapse. If the restored
    // child were not relinked onto the maker's own live-child list, the
    // maker would (wrongly) believe it has room and spawn a second child
    // immediately, growing the count past 1 even though the first child is
    // still alive and `m_imaxlivechildren` is 1.
    script_tick(&mut reloaded, 400);
    assert_eq!(
        reloaded.monster_count(),
        1,
        "m_imaxlivechildren=1 and the restored child is still alive: no \
         second child may spawn until it dies"
    );
}

/// A `monstermaker` that has spawned two children, one of them already
/// dead (a corpse, not gibbed) by save time, round-trips both correctly:
/// the live one keeps existing (and thinking — it still carries
/// `ohl_ai::MonsterAi`) at its saved health, and the dead one does not
/// come back as a second thinking monster (matching `Self::retire`'s own
/// rule that a corpse loses its `MonsterAi` but not the rest of the
/// entity).
#[test]
fn a_makers_two_children_one_dead_round_trip_correctly() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "monstermaker",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("monstertype", "monster_headcrab"),
                ("monstercount", "2"),
                ("m_imaxlivechildren", "2"),
                ("delay", "0.05"),
                ("spawnflags", "1"), // "Start On".
            ],
        ),
    );

    let mut game = script_game(&entities);
    // `delay=0.05` and `TICK_SECONDS` (see `crate::TICK_SECONDS`) together
    // need a handful of ticks for both children to spawn; `m_imaxlivechildren=2`
    // lets both exist at once.
    script_tick(&mut game, 20);
    assert_eq!(
        game.monster_count(),
        2,
        "monstercount=2, m_imaxlivechildren=2: both children must have spawned"
    );

    let children = monster_entities(&game);
    assert_eq!(children.len(), 2);
    queue_monster_damage(&mut game, children[0], None, 1_000.0);
    // One more tick applies the queued damage and runs `AiState::retire`,
    // turning the killed child into a corpse (its `MonsterAi` removed, the
    // rest of the entity — including `Owner`/`ClassName`, which is what
    // `SECTION_MAKER_CHILDREN` keys off — left alone).
    script_tick(&mut game, 1);
    assert_eq!(
        game.monster_count(),
        1,
        "the killed child is now a corpse: it no longer carries MonsterAi"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert_eq!(
        reloaded.monster_count(),
        1,
        "only the still-alive child round-trips as a thinking monster; the \
         corpse does not come back as a second one"
    );

    // The maker's own quota (`monstercount=2`) was already exhausted
    // before the save (both children were ever spawned), so ticking a lot
    // more must not create a third monster no matter how the corpse round
    // trips.
    script_tick(&mut reloaded, 400);
    assert_eq!(
        reloaded.monster_count(),
        1,
        "monstercount=2 is already exhausted; no further child may spawn"
    );
}

/// A save written before `SECTION_MAKER_CHILDREN` (29, M9.5) existed —
/// tags up to 28 only — still loads, with the pre-M9.5 behaviour: the
/// maker's own counters restore (tag 28 already covered those), but its
/// already-spawned child is not recreated, since no record of it exists in
/// a save this old.
#[test]
fn a_pre_tag_29_save_still_loads_without_its_maker_children() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "monstermaker",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("monstertype", "monster_headcrab"),
                ("monstercount", "2"),
                ("delay", "0.05"),
                ("spawnflags", "1"), // "Start On".
            ],
        ),
    );

    let mut game = script_game(&entities);
    script_tick(&mut game, 1);
    assert_eq!(game.monster_count(), 1);

    let mut save = game.to_save(1_700_000_000);
    // Simulate a save written by a build before M9.5: tag 29 simply never
    // existed.
    save.maker_children = None;
    let bytes = save.to_bytes().expect("the save is written");

    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert_eq!(
        reloaded.monster_count(),
        0,
        "no SECTION_MAKER_CHILDREN section at all: the pre-M9.5 behaviour, \
         the child is not recreated, exactly like `a_save_missing_the_new_sections_still_loads` \
         exercises for tags 23-27"
    );

    // The maker's own counters (tag 28, pre-existing) still restore
    // correctly on top of this, so the quota is still respected.
    script_tick(&mut reloaded, 400);
    assert_eq!(
        reloaded.monster_count(),
        1,
        "monstercount=2 total, one already spent before the save: only one \
         more may ever spawn"
    );
}

/// A `func_rotating`'s spin state (`spinning`/`angle_deg`) round trips
/// through a save/load. Two sections carry it: `SECTION_MOVER_STATE`
/// (28)'s `RotatorSnapshot`, and — redundantly, and in fact first —
/// `SECTION_ENTITY_REGISTRY` (18), whose
/// `ohl_engine::transition::EntitySnapshot` captures and re-applies the
/// whole `ohl_game::registry::Rotator` component in spawn order, the same
/// mechanism that carries a `func_door`'s or `func_button`'s own state
/// machine. The duplication is a frozen wire shape being honoured rather
/// than a design (`docs/FORMAT_SOURCES.md` `TODO(black-box)` item 28); that
/// tag 18 alone suffices is shown by
/// `a_spinning_rotator_is_carried_by_the_entity_registry_section` below.
/// Without either, a spinning `func_rotating` would revert to its
/// spawnflag default (spinning per "Start On", `angle_deg: 0.0`) on load.
/// "Start On" is set so `spinning` alone cannot distinguish a correct
/// restore from a reset to the spawn default — only `angle_deg` can, so
/// this saves *mid-spin*, after a nonzero angle has already accumulated,
/// and checks that the reloaded angle is the accumulated one, not zero.
#[test]
fn a_spinning_rotator_round_trips_its_spin_state_and_continues() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "func_rotating",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("targetname", "fan1"),
                ("speed", "180"),
                ("spawnflags", "1"), // "Start On": see docs/FORMAT_SOURCES.md, "Entity keyvalues and map logic".
            ],
        ),
    );

    let mut game = script_game(&entities);
    let entity = entity_of_classname(&game, "func_rotating").expect("the fan spawned");
    assert!(
        game.registry()
            .world
            .get::<&ohl_game::registry::Rotator>(entity)
            .expect("the fan carries a Rotator")
            .spinning,
        "'Start On' must have it spinning already"
    );

    // 180 degrees/second for half a second is 90 degrees.
    script_tick(&mut game, 50);

    let (spinning_before_save, angle_before_save) = {
        let rotator = game
            .registry()
            .world
            .get::<&ohl_game::registry::Rotator>(entity)
            .expect("the fan carries a Rotator");
        (rotator.spinning, rotator.angle_deg)
    };
    assert!(spinning_before_save);
    assert!(
        (angle_before_save - 90.0).abs() < 1e-2,
        "angle was {angle_before_save}"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity = entity_of_classname(&reloaded, "func_rotating").expect("the fan reloads");
    {
        let rotator = reloaded
            .registry()
            .world
            .get::<&ohl_game::registry::Rotator>(reloaded_entity)
            .expect("the reloaded fan still carries a Rotator");
        assert!(rotator.spinning);
        assert!(
            (rotator.angle_deg - angle_before_save).abs() < 1e-6,
            "angle_deg was {} but must round-trip the mid-spin value {angle_before_save}, \
             not reset to the spawn default 0.0",
            rotator.angle_deg
        );
    }

    // Still spinning after the load: another half second accumulates
    // another 90 degrees on top of the restored angle, not from zero.
    script_tick(&mut reloaded, 50);
    let angle_after = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::Rotator>(reloaded_entity)
        .expect("the fan is still there")
        .angle_deg;
    assert!(
        (angle_after - 180.0).abs() < 1e-1,
        "angle was {angle_after}, expected the restored 90 degrees plus \
         another 90 accumulated post-load"
    );
}

/// `SECTION_ENTITY_REGISTRY` (18) carries a `func_rotating`'s spin state on
/// its own: the same mid-spin save as the test above, with
/// `SECTION_MOVER_STATE` (28) dropped from the file entirely, still restores
/// the accumulated angle. That is what makes tag 28's own `RotatorSnapshot`
/// redundant — it is kept only because tag 28's wire shape is frozen with it
/// in place, never because a restore needs it (`docs/FORMAT_SOURCES.md`
/// `TODO(black-box)` item 28).
#[test]
fn a_spinning_rotator_is_carried_by_the_entity_registry_section() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "func_rotating",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("targetname", "fan1"),
                ("speed", "180"),
                ("spawnflags", "1"), // "Start On": see docs/FORMAT_SOURCES.md, "Entity keyvalues and map logic".
            ],
        ),
    );

    let mut game = script_game(&entities);
    script_tick(&mut game, 50);
    let entity = entity_of_classname(&game, "func_rotating").expect("the fan spawned");
    let angle_before_save = game
        .registry()
        .world
        .get::<&ohl_game::registry::Rotator>(entity)
        .expect("the fan carries a Rotator")
        .angle_deg;
    assert!(angle_before_save > 1.0, "the fan must have spun measurably");

    let mut save = game.to_save(1_700_000_000);
    save.mover_state = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_MOVER_STATE still encodes");

    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity = entity_of_classname(&reloaded, "func_rotating").expect("the fan reloads");
    let rotator = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::Rotator>(reloaded_entity)
        .expect("the reloaded fan still carries a Rotator");
    assert!(rotator.spinning);
    assert!(
        (rotator.angle_deg - angle_before_save).abs() < 1e-6,
        "angle_deg was {} but tag 18 alone must round-trip the mid-spin \
         value {angle_before_save}",
        rotator.angle_deg
    );
}

/// The side a `func_door_rotating` chose to swing away from its activator
/// survives a save/load: the choice is written into `Door::rotation_axis`'s
/// own sign (`ohl_game::registry::RotatingDoorSwing`,
/// `docs/FORMAT_SOURCES.md` item 26), and that field already round-trips
/// with the rest of the `Door` component in `SECTION_ENTITY_REGISTRY`'s
/// per-entity snapshot — so a reloaded door keeps opening the way it was
/// opened, rather than reverting to its spawnflag-chosen direction.
#[test]
fn a_rotating_doors_chosen_swing_side_survives_a_save_load() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "func_door_rotating",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("targetname", "rotdoor1"),
                ("speed", "90"),
                ("distance", "90"),
                ("wait", "-1"),
            ],
        ),
    );

    let mut game = script_game(&entities);
    let entity = entity_of_classname(&game, "func_door_rotating").expect("the door spawned");
    {
        let mut door = game
            .registry()
            .world
            .get::<&mut ohl_game::registry::Door>(entity)
            .expect("the door carries a Door");
        // The spawnflags alone chose `+Z`; an activator standing on that
        // side is what flips it (unit-tested in `ohl_game::logic`), and
        // this is the state that has to survive the save.
        assert_eq!(door.rotation_axis, Some(glam::Vec3::Z));
        door.rotation_axis = Some(-glam::Vec3::Z);
    }
    script_tick(&mut game, 10);

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity =
        entity_of_classname(&reloaded, "func_door_rotating").expect("the door reloads");
    let door = *reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::Door>(reloaded_entity)
        .expect("the reloaded door still carries a Door");
    assert_eq!(
        door.rotation_axis,
        Some(-glam::Vec3::Z),
        "the reloaded door reverted to its spawnflag-chosen swing side"
    );
}

/// The other half of `SECTION_ROTATING_MOVER_STATE` (tag 30): the
/// `func_rot_button` *touch-edge* bookkeeping
/// (`ohl_game::logic::Simulation::rot_button_touch_snapshot`), which is
/// keyed by entity bit pattern rather than spawn index and so rides
/// alongside the section's entity-indexed `movers` vector.
///
/// The fixture is a "Toggle" + "Touch activates" `func_rot_button` whose
/// brush volume overlaps the player's own standing hull at the spawn point,
/// so the player is *still standing in it* when the save is taken. Touching
/// is edge-triggered (`Simulation::touch_rot_buttons` only activates on the
/// not-overlapping -> overlapping transition), so the restored map either
/// remembers that the player was already inside — and leaves the button
/// alone — or forgets, sees a fresh rising edge on the very first tick after
/// the load, and toggles the pressed button straight back closed. That is
/// what makes this test discriminating: no-oping `Game::restore`'s
/// `restore_rot_button_touch` call fails the final assertion, rather than
/// leaving the suite green.
#[test]
fn a_rot_button_the_player_is_standing_in_does_not_re_fire_after_a_load() {
    // `spawnflags`: 32 ("Toggle") | 256 ("Touch activates"), the documented
    // bits `ohl_game::registry::SPAWNFLAG_ROT_BUTTON_TOGGLE`/`_TOUCH` name.
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -24 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_rot_button\"\n\"targetname\" \"{ROT_BUTTON_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"spawnflags\" \"288\"\n\"origin\" \"0 0 0\"\n}}\n"
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{ROT_BUTTON_MAP}.bsp"),
        rot_button_bsp(&entities),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROT_BUTTON_MAP).expect("the fixture loads");

    let button_state = |game: &Game| {
        let registry = game.registry();
        let entity = *registry
            .find(ROT_BUTTON_NAME)
            .first()
            .expect("the fixture declares one named rot button");
        *registry
            .world
            .get::<&ohl_game::registry::RotButton>(entity)
            .expect("the named entity is a rot button")
    };

    // The player spawns inside the button's volume, so the first tick is the
    // rising edge that presses it; a quarter turn at 360 deg/s takes a
    // quarter second, and "Toggle" then holds it open indefinitely.
    script_tick(&mut game, 30);
    assert_eq!(
        button_state(&game).state,
        ohl_game::registry::MoverState::Open,
        "the touch press must have completed before the save"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert_eq!(
        button_state(&reloaded).state,
        ohl_game::registry::MoverState::Open,
        "tag 30's entity-indexed half must restore the pressed button itself"
    );

    // The load put the player back inside the button's volume. With the
    // touch-edge state restored there is no rising edge, so nothing happens;
    // without it, this tick toggles the button back to `Closing`.
    script_tick(&mut reloaded, 1);
    assert_eq!(
        button_state(&reloaded).state,
        ohl_game::registry::MoverState::Open,
        "the reloaded map saw a spurious touch rising edge and toggled the \
         button back: SECTION_ROTATING_MOVER_STATE's rot_button_touch half \
         did not restore"
    );
}

/// A `func_rot_button` mid-press round-trips its `RotButton` state through
/// `SECTION_ROTATING_MOVER_STATE` (tag 30, not `Door`/`Rotator`'s own tag
/// 28 — see `ohl_engine::save_state::MoverSnapshot`'s own doc comment for
/// why): pressed through the real `use_pressed` input path (see
/// `crates/ohl-engine/tests/rot_button.rs` for the same path proving the
/// button unblocks its target), saved partway through its swing, the
/// reloaded button is still `Opening` at the same timer, not reset to
/// `Closed`.
#[test]
fn a_pressed_rot_button_round_trips_its_press_state_and_continues() {
    // The button sits at the player's own spawn point (no brush model, so
    // no collision to embed in) — well inside `ohl_engine::USE_RADIUS`
    // regardless of the eye-height offset `find_usable_within` measures
    // from.
    let spawn = [-192.0, -192.0, 36.0];
    let entities = script_room_entities(
        spawn,
        &entity_block(
            "func_rot_button",
            spawn,
            0.0,
            &[
                ("targetname", "btn1"),
                ("speed", "45"),
                ("distance", "90"),
                ("wait", "5"),
            ],
        ),
    );
    let mut game = script_game(&entities);
    let entity = entity_of_classname(&game, "func_rot_button").expect("the button spawned");

    game.tick(
        TICK_SECONDS,
        &Input {
            use_pressed: true,
            ..Input::default()
        },
    );
    // 90 degrees at 45 degrees/second takes 2 seconds; tick 1 more second
    // so it is saved mid-`Opening`, not yet `Open`.
    script_tick(&mut game, 60);
    let (state_before_save, timer_before_save) = {
        let button = game
            .registry()
            .world
            .get::<&ohl_game::registry::RotButton>(entity)
            .expect("the button carries a RotButton");
        (button.state, button.timer)
    };
    assert_eq!(state_before_save, ohl_game::registry::MoverState::Opening);

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity =
        entity_of_classname(&reloaded, "func_rot_button").expect("the button reloads");
    let button = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::RotButton>(reloaded_entity)
        .expect("the reloaded button still carries a RotButton");
    assert_eq!(button.state, state_before_save);
    assert!(
        (button.timer - timer_before_save).abs() < 1e-6,
        "timer was {} but must round-trip the mid-press value {timer_before_save}",
        button.timer
    );
}

/// A `momentary_rot_button` held partway (through the real `use_held` input
/// path) round-trips its `fraction`, not resetting to `0.0` on reload.
#[test]
fn a_held_momentary_rot_button_round_trips_its_fraction() {
    let spawn = [-192.0, -192.0, 36.0];
    let entities = script_room_entities(
        spawn,
        &entity_block(
            "momentary_rot_button",
            spawn,
            0.0,
            &[
                ("targetname", "valve1"),
                ("speed", "45"),
                ("distance", "90"),
            ],
        ),
    );
    let mut game = script_game(&entities);
    let entity = entity_of_classname(&game, "momentary_rot_button").expect("the valve spawned");

    let held = Input {
        use_held: true,
        ..Input::default()
    };
    for _ in 0..30 {
        game.tick(TICK_SECONDS, &held);
    }
    let fraction_before_save = game
        .registry()
        .world
        .get::<&ohl_game::registry::MomentaryRotButton>(entity)
        .expect("the valve carries a MomentaryRotButton")
        .fraction;
    assert!(
        fraction_before_save > 0.0 && fraction_before_save < 1.0,
        "fraction was {fraction_before_save}, expected partway through the sweep"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity =
        entity_of_classname(&reloaded, "momentary_rot_button").expect("the valve reloads");
    let fraction_after_load = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::MomentaryRotButton>(reloaded_entity)
        .expect("the reloaded valve still carries a MomentaryRotButton")
        .fraction;
    assert!(
        (fraction_after_load - fraction_before_save).abs() < 1e-6,
        "fraction was {fraction_after_load} but must round-trip {fraction_before_save}, \
         not reset to 0.0"
    );
}

/// A `func_pendulum` mid-swing round-trips its `angle_deg`/`elapsed`, not
/// resetting to rest on reload.
#[test]
fn a_swinging_pendulum_round_trips_its_pose_and_continues() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &entity_block(
            "func_pendulum",
            [96.0, -96.0, 36.0],
            0.0,
            &[
                ("targetname", "swing1"),
                ("distance", "30"),
                ("speed", "180"),
                ("spawnflags", "1"), // "Start ON": docs/FORMAT_SOURCES.md, "Entity keyvalues and map logic".
            ],
        ),
    );
    let mut game = script_game(&entities);
    let entity = entity_of_classname(&game, "func_pendulum").expect("the pendulum spawned");
    assert!(
        game.registry()
            .world
            .get::<&ohl_game::registry::Pendulum>(entity)
            .expect("the pendulum carries a Pendulum")
            .swinging,
        "'Start ON' must have it swinging already"
    );
    script_tick(&mut game, 10);
    let angle_before_save = game
        .registry()
        .world
        .get::<&ohl_game::registry::Pendulum>(entity)
        .expect("the pendulum carries a Pendulum")
        .angle_deg;
    assert!(
        angle_before_save.abs() > 0.0,
        "the pendulum should have moved off rest"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity =
        entity_of_classname(&reloaded, "func_pendulum").expect("the pendulum reloads");
    let pendulum = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::Pendulum>(reloaded_entity)
        .expect("the reloaded pendulum still carries a Pendulum");
    assert!(pendulum.swinging);
    assert!(
        (pendulum.angle_deg - angle_before_save).abs() < 1e-6,
        "angle_deg was {} but must round-trip the mid-swing value {angle_before_save}",
        pendulum.angle_deg
    );
}

/// The exact regression `SECTION_ROTATING_MOVER_STATE` (30) exists to rule
/// out: a save written by a build before this section existed (tag 30
/// simply absent — the pre-PR writer's own shape, reproduced here by
/// clearing `GameSave::rotating_movers` before encoding, the same
/// technique [`a_pre_tag_28_save_still_loads`]/
/// [`a_pre_tag_29_save_still_loads_without_its_maker_children`] already use
/// for their own tags) must still load. Built on
/// `ohl_engine::test_support::rotating_door_bsp` — a real, solid rotating
/// brush mover, not a bare point entity — precisely because that fixture
/// is what the M9.6 review found broken: adding fields to `EntitySnapshot`
/// (tag 18, required) and `SimulationState` (tag 19, required) made a
/// save built from this exact fixture at `origin/main` (`424ac85`) fail
/// to load at that revision's head with `EngineError::SaveUnreadable`.
#[test]
fn a_save_from_before_section_30_existed_still_loads() {
    let bytes = rotating_door_bsp(&rotating_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");

    let mut save = game.to_save(1_700_000_000);
    // Simulate a save written by a build before M9.6: tag 30 simply never
    // existed, exactly like `EntitySnapshot`/`SimulationState` never
    // carried this state on `main` either.
    save.rotating_movers = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_ROTATING_MOVER_STATE still encodes");

    // The regression this guards: at the review revision, this exact
    // fixture failed here with `EngineError::SaveUnreadable`, because
    // `EntitySnapshot` (tag 18, required) and `SimulationState` (tag 19,
    // required) had gained fields no pre-existing save's bytes carry —
    // `postcard` is not self-describing, so those two required sections
    // fail closed rather than defaulting. Tag 18/19 carry no new fields on
    // this revision, so this must succeed regardless of tag 30's presence.
    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-30 save still loads");
    let entity = *reloaded
        .registry()
        .find(ohl_engine::test_support::ROTATING_DOOR_NAME)
        .first()
        .expect("the door reloads");
    assert!(
        reloaded
            .registry()
            .world
            .get::<&ohl_game::registry::Door>(entity)
            .is_ok(),
        "the reloaded fixture is a live Game, not a placeholder"
    );
}

/// A `momentary_door` pushed partway open (through the real `use_held`
/// input path driving its `momentary_rot_button`) round-trips its
/// `fraction`, not resetting to `0.0` on reload — the discriminating case
/// `SECTION_MOMENTARY_DOOR_STATE` (31, M9.8, `docs/FORMAT_SOURCES.md` item
/// 29) exists for: the default `MomentaryDoor::fraction` a fresh
/// `attach_level` spawns is `0.0`, so this fails unless tag 31's own
/// restore call actually runs.
#[test]
fn a_momentary_door_round_trips_its_fraction() {
    let bytes = momentary_door_bsp(&momentary_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MOMENTARY_DOOR_MAP}.bsp"), bytes);
    let mut game =
        Game::load(&assets as &dyn AssetSource, MOMENTARY_DOOR_MAP).expect("the fixture loads");

    let held = Input {
        use_held: true,
        ..Input::default()
    };
    // A partial hold: well short of the button's full 30-tick sweep.
    for _ in 0..7 {
        game.tick(TICK_SECONDS, &held);
    }
    let entity = *game
        .registry()
        .find(MOMENTARY_DOOR_NAME)
        .first()
        .expect("the fixture declares one named momentary_door");
    let fraction_before_save = game
        .registry()
        .world
        .get::<&ohl_game::registry::MomentaryDoor>(entity)
        .expect("the named entity is a momentary_door")
        .fraction;
    assert!(
        fraction_before_save > 0.0 && fraction_before_save < 1.0,
        "fraction was {fraction_before_save}, expected partway through the sweep"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity = *reloaded
        .registry()
        .find(MOMENTARY_DOOR_NAME)
        .first()
        .expect("the fixture's momentary_door reloads");
    let fraction_after_load = reloaded
        .registry()
        .world
        .get::<&ohl_game::registry::MomentaryDoor>(reloaded_entity)
        .expect("the reloaded entity still carries a MomentaryDoor")
        .fraction;
    assert!(
        (fraction_after_load - fraction_before_save).abs() < 1e-6,
        "fraction was {fraction_after_load} but must round-trip {fraction_before_save}, \
         not reset to 0.0"
    );
}

/// The exact regression `SECTION_MOMENTARY_DOOR_STATE` (31) exists to rule
/// out: a save written by a build before this section existed (tag 31
/// simply absent, reproduced here by clearing `GameSave::momentary_doors`
/// before encoding, the same technique
/// [`a_save_from_before_section_30_existed_still_loads`] already uses for
/// tag 30) must still load, with the `momentary_door` defaulting to
/// `fraction = 0.0`, its spawn-time resting position.
#[test]
fn a_save_from_before_section_31_existed_still_loads() {
    let bytes = momentary_door_bsp(&momentary_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{MOMENTARY_DOOR_MAP}.bsp"), bytes);
    let game =
        Game::load(&assets as &dyn AssetSource, MOMENTARY_DOOR_MAP).expect("the fixture loads");

    let mut save = game.to_save(1_700_000_000);
    save.momentary_doors = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_MOMENTARY_DOOR_STATE still encodes");

    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-31 save still loads");
    let entity = *reloaded
        .registry()
        .find(MOMENTARY_DOOR_NAME)
        .first()
        .expect("the fixture's momentary_door reloads");
    assert_eq!(
        reloaded
            .registry()
            .world
            .get::<&ohl_game::registry::MomentaryDoor>(entity)
            .expect("the reloaded entity still carries a MomentaryDoor")
            .fraction,
        0.0,
        "a pre-tag-31 save must leave the momentary_door at its spawn-time resting fraction"
    );
}

/// A `func_breakable` that has taken damage but not broken round-trips its
/// remaining hit points, and one that has broken stays broken — the
/// discriminating case `SECTION_BREAKABLE_STATE` (33, M9.10,
/// `docs/FORMAT_SOURCES.md` item 32) exists for: a fresh `attach_level`
/// spawns every breakable back at its authored `health`, unbroken, so both
/// halves fail unless tag 33's own restore call actually runs. Every hit
/// here is a real shot through the `Game` loop, never a forced component.
#[test]
fn a_damaged_and_a_broken_breakable_both_round_trip() {
    let assets = breakable_assets();
    let mut game =
        Game::load(&assets as &dyn AssetSource, BREAKABLE_MAP).expect("the fixture loads");
    arm_with_the_spawn_point_weapon(&mut game);

    // One published 40-damage `.357` shot against the fixture's 100 hit
    // points: damaged, not broken.
    game.tick(
        TICK_SECONDS,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    let damaged = breakable_of(&game);
    assert!(
        !damaged.broken && damaged.health < BREAKABLE_TOUGH_HEALTH && damaged.health > 0.0,
        "one shot left the breakable at {damaged:?}"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let restored = breakable_of(&reloaded);
    assert!(
        !restored.broken && (restored.health - damaged.health).abs() < 1e-6,
        "a damaged breakable came back as {restored:?}, not at {}",
        damaged.health
    );

    // Keep firing until the remaining hit points are gone.
    for _ in 0..400 {
        game.tick(
            TICK_SECONDS,
            &Input {
                attack: true,
                ..Input::default()
            },
        );
    }
    assert!(breakable_of(&game).broken, "sustained fire never broke it");
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert!(
        breakable_of(&reloaded).broken,
        "a breakable broken before the save must still be broken after the load"
    );
}

/// The exact regression `SECTION_BREAKABLE_STATE` (33) exists to rule out: a
/// save written by a build before this section existed (tag 33 simply
/// absent, reproduced by clearing `GameSave::breakables` before encoding,
/// the same technique the tag-30 and tag-31 regressions above already use)
/// must still load, with the breakable back at its authored `health` and
/// unbroken.
#[test]
fn a_save_from_before_section_33_existed_still_loads() {
    let assets = breakable_assets();
    let game = Game::load(&assets as &dyn AssetSource, BREAKABLE_MAP).expect("the fixture loads");
    let mut save = game.to_save(1_700_000_000);
    save.breakables = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_BREAKABLE_STATE still encodes");

    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-33 save still loads");
    let restored = breakable_of(&reloaded);
    assert!(
        !restored.broken && (restored.health - BREAKABLE_TOUGH_HEALTH).abs() < f32::EPSILON,
        "a pre-tag-33 save must leave the breakable intact at its authored health, got {restored:?}"
    );
}

/// The fixture breakable's `health`: more than one published 40-damage
/// `.357` shot, so "damaged but not broken" is observable through real
/// shots alone.
const BREAKABLE_TOUGH_HEALTH: f32 = 100.0;

/// Picks up the `weapon_357` the corridor fixture leaves on the spawn point
/// and loads its clip, all through ordinary inputs.
fn arm_with_the_spawn_point_weapon(game: &mut Game) {
    game.tick(TICK_SECONDS, &Input::default());
    game.tick(
        TICK_SECONDS,
        &Input {
            select_slot: Some(2),
            ..Input::default()
        },
    );
    game.tick(
        TICK_SECONDS,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    for _ in 0..400 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    assert!(
        game.inventory().clip(WeaponId::Python) > 0,
        "reload must have loaded the clip before the shots"
    );
}

/// The `func_breakable` corridor fixture, as an asset source both a fresh
/// load and a save/load round trip can read.
fn breakable_assets() -> MemoryAssets {
    let bytes = obstacle_corridor_bsp(
        &breakable_corridor_entities(BREAKABLE_TOUGH_HEALTH, 0),
        BREAKABLE_OBSTACLE_MAXS,
        false,
        false,
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{BREAKABLE_MAP}.bsp"), bytes);
    assets
}

/// The fixture's one `func_breakable` component.
fn breakable_of(game: &Game) -> ohl_game::registry::Breakable {
    let entity = *game
        .registry()
        .find(OBSTACLE_NAME)
        .first()
        .expect("the fixture's breakable reloads");
    *game
        .registry()
        .world
        .get::<&ohl_game::registry::Breakable>(entity)
        .expect("the reloaded entity still carries a Breakable")
}

/// A save written before `SECTION_MOVER_STATE` (28) existed (no tag 28 at
/// all) still loads, with every mover/camera/script/maker defaulting to
/// fresh/inactive state.
#[test]
fn a_pre_tag_28_save_still_loads() {
    let entities = script_room_entities([-192.0, -192.0, 36.0], &track_train_entities());
    let mut game = script_game(&entities);
    script_tick(&mut game, 60);
    let mut save = game.to_save(1_700_000_000);
    save.mover_state = None;

    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_MOVER_STATE still encodes");
    let assets = script_game_assets(&entities);
    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-28 save still loads");
    let train = entity_of_classname(&reloaded, "func_tracktrain").expect("the train restores");
    assert_eq!(
        train_position(&reloaded, train),
        [0.0, 96.0, 64.0],
        "with no SECTION_MOVER_STATE at all, the train defaults to its \
         freshly attach-level-spawned (dormant, at node1) state"
    );
}

/// A save whose `SECTION_MOVER_STATE` is present but fails to decode is
/// rejected outright rather than silently loading with defaults or
/// panicking, matching every other section's "fail closed" rule.
#[test]
fn a_corrupted_mover_state_section_fails_closed() {
    let game = game();
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let save = ohl_engine::GameSave::from_bytes(&bytes).expect("the original save reads back");

    let header = ohl_save::Header {
        game_version: String::new(),
        created_at_unix_secs: 1_700_000_000,
        map_identity: save.header.map.clone(),
        title: save.header.map.clone(),
        thumbnail: Vec::new(),
    };
    let mut writer = ohl_save::SaveWriter::begin(header);
    writer
        .add_section_serde(ohl_engine::save::SECTION_ENGINE_HEADER, &save.header)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_PLAYER_CARRY, &save.player)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_ENTITY_REGISTRY, &save.entities)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_SIMULATION, &save.simulation)
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_GLOBAL_STATE, &save.globals)
        .unwrap();
    writer
        .add_section_serde(
            ohl_engine::save::SECTION_LIGHT_STYLE_TIME,
            &save.light_style_time,
        )
        .unwrap();
    writer
        .add_section_serde(ohl_engine::save::SECTION_VIEW, &save.view)
        .unwrap();
    // A byte string that is not a valid `postcard` encoding of
    // `Vec<Option<MoverSnapshot>>`.
    writer
        .add_section(ohl_engine::save::SECTION_MOVER_STATE, &[0xFF; 64])
        .unwrap();
    let corrupted = writer
        .finish(&ohl_save::Limits::default())
        .expect("the container still assembles");

    let result = Game::load_bytes(&game_assets(), &corrupted);
    assert!(matches!(result, Err(EngineError::SaveUnreadable)));
}

/// A trigger that has fired but not yet reached the `Spawner` — the
/// one-tick window between `Simulation::activate` (phase 12, the last
/// phase of a tick) bumping `MakerActivation::pending` and
/// `AiState::tick_makers` (phase 10) draining it the *following* tick —
/// still survives a save taken right in that window, and the maker still
/// spawns once the reloaded game resumes ticking.
#[test]
fn a_pending_maker_activation_at_the_phase_boundary_survives_a_save_load() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}",
            trigger_auto("maker1"),
            entity_block(
                "monstermaker",
                [96.0, -96.0, 36.0],
                0.0,
                &[
                    ("targetname", "maker1"),
                    ("monstertype", "monster_headcrab"),
                    ("monstercount", "1"),
                    ("delay", "0"),
                ],
            ),
        ),
    );
    let mut game = script_game(&entities);
    // Exactly one tick: phase 12's `Simulation::tick` fires the
    // `trigger_auto` and bumps `MakerActivation::pending` to 1, but this
    // same tick's own phase 10 already ran *before* phase 12, so the
    // activation is still sitting unspent right here.
    script_tick(&mut game, 1);
    assert_eq!(
        game.monster_count(),
        0,
        "the trigger has fired but not yet reached the Spawner"
    );

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the phase-boundary save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the phase-boundary save loads");

    script_tick(&mut reloaded, 5);
    assert_eq!(
        reloaded.monster_count(),
        1,
        "the pending activation must have survived the save and still \
         reach the Spawner, or the maker never spawns at all"
    );
}

/// Confirms, against the actual restore-order code (not just asserted),
/// what happens to a `trigger_auto` with the published `Remove On fire`
/// spawnflag set once it has already fired (and so despawned itself, per
/// `ohl_game::logic::Simulation::fire_auto_triggers`) before a save: it
/// has no live `AutoTrigger` component left for `SECTION_MOVER_STATE`'s
/// `auto_trigger_fired` to read `fired` off at all (`snapshot_movers`
/// records `None` for that slot), but the entity does **not** come back
/// after a load either — `SECTION_ENTITY_COMBAT` (tag 24, pre-existing,
/// unrelated to this section) already despawns any spawn-index entity
/// that was not live in the world at save time, a `Remove On fire`
/// `trigger_auto` included, before this section's own restore ever runs.
/// It therefore does not, and cannot, refire after the load.
#[test]
fn a_remove_on_fire_trigger_auto_that_already_fired_does_not_refire_after_a_load() {
    const SPAWNFLAG_REMOVE_ON_FIRE: u32 = 1;
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}",
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[
                    ("target", "after"),
                    ("spawnflags", &SPAWNFLAG_REMOVE_ON_FIRE.to_string()),
                ],
            ),
            exit_trigger("after"),
        ),
    );
    let mut game = script_game(&entities);
    let mut fired = 0usize;
    for _ in 0..5 {
        for event in game.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    assert_eq!(
        fired, 1,
        "the auto trigger must have fired exactly once already"
    );

    let bytes = game
        .save_bytes(1_700_000_000)
        .expect("the post-fire save is written");
    let assets = script_game_assets(&entities);
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the post-fire save loads");

    let mut refired = 0usize;
    for _ in 0..5 {
        for event in reloaded.tick(TICK_SECONDS, &Input::default()) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                refired += 1;
            }
        }
    }
    assert_eq!(
        refired, 0,
        "a Remove-On-fire trigger_auto that had already fired and despawned \
         itself before the save must stay gone after the load (via \
         SECTION_ENTITY_COMBAT's own despawn-if-not-live rule), not fire \
         again"
    );
    let entity_after_load = entity_of_classname(&reloaded, "trigger_auto");
    assert!(
        entity_after_load.is_none(),
        "the entity itself must stay despawned, confirming *why* it cannot refire"
    );
}

/// A `func_platrot` caught part-way through its trip round-trips its state
/// and its timer — the discriminating case `SECTION_PLATROT_STATE` (37,
/// M9.33, `docs/FORMAT_SOURCES.md`, `func_platrot`) exists for: a fresh
/// `attach_level` spawns every platform back at `MoverState::Closed` with a
/// zero timer, so both halves of the platform's pose (its travel *and* its
/// rotation, which are derived from exactly this pair) come back wrong
/// unless tag 37's own restore call actually runs.
///
/// The platform is started the way the cited page says it is started — by
/// the player standing on it — through the real `Game` loop, never by
/// forcing a component.
#[test]
fn a_func_platrot_round_trips_its_travel_and_its_spin() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLATROT_MAP}.bsp"),
        rotating_platform_bsp(&platrot_entities()),
    );
    let mut game = Game::load(&assets as &dyn AssetSource, PLATROT_MAP).expect("the fixture loads");

    // Part-way up: well short of the fixture's own two-second trip.
    for _ in 0..60 {
        game.tick(TICK_SECONDS, &Input::default());
    }
    let entity = *game
        .registry()
        .find(PLATROT_NAME)
        .first()
        .expect("the fixture declares one named func_platrot");
    let before = *game
        .registry()
        .world
        .get::<&PlatRot>(entity)
        .expect("the named entity is a func_platrot");
    assert_eq!(
        before.state,
        MoverState::Opening,
        "the player standing on it should have it part-way up"
    );
    assert!(before.timer > 0.0, "timer was {}", before.timer);
    let pose_before = ohl_game::pose::platrot_degrees(game.registry(), entity);
    assert!(
        pose_before.1 > 0.0,
        "it should have turned some way already"
    );

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let reloaded_entity = *reloaded
        .registry()
        .find(PLATROT_NAME)
        .first()
        .expect("the fixture's func_platrot reloads");
    let after = *reloaded
        .registry()
        .world
        .get::<&PlatRot>(reloaded_entity)
        .expect("the reloaded entity still carries a PlatRot");
    assert_eq!(after.state, before.state);
    assert!((after.timer - before.timer).abs() < 1e-6);
    assert_eq!(
        ohl_game::pose::platrot_degrees(reloaded.registry(), reloaded_entity),
        pose_before,
        "the whole pose follows from the state and the timer"
    );
}

/// A save written by a build before `SECTION_PLATROT_STATE` (37) existed —
/// the tag simply absent, reproduced by clearing `GameSave::platrots`
/// before encoding, the same technique the pre-tag-30 and pre-tag-31 tests
/// above already use — must still load, with the platform back at its
/// spawn-time resting pose.
#[test]
fn a_save_from_before_section_37_existed_still_loads() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLATROT_MAP}.bsp"),
        rotating_platform_bsp(&platrot_entities()),
    );
    let mut game = Game::load(&assets as &dyn AssetSource, PLATROT_MAP).expect("the fixture loads");
    for _ in 0..60 {
        game.tick(TICK_SECONDS, &Input::default());
    }

    let mut save = game.to_save(1_700_000_000);
    save.platrots = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing SECTION_PLATROT_STATE still encodes");

    let reloaded = Game::load_bytes(&assets, &bytes).expect("a pre-tag-37 save still loads");
    let entity = *reloaded
        .registry()
        .find(PLATROT_NAME)
        .first()
        .expect("the fixture's func_platrot reloads");
    let platrot = *reloaded
        .registry()
        .world
        .get::<&PlatRot>(entity)
        .expect("the reloaded entity still carries a PlatRot");
    assert_eq!(
        platrot.state,
        MoverState::Closed,
        "a pre-tag-37 save must leave the platform at its spawn-time resting pose"
    );
    assert_eq!(platrot.timer, 0.0);
}
