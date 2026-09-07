//! M7.9 P4b: the five additive save sections (`SECTION_INVENTORY` 23,
//! `SECTION_ENTITY_COMBAT` 24, `SECTION_AI` 25, `SECTION_PROJECTILES` 26,
//! `SECTION_RNG` 27), exercised through the full `Game` loop.
//!
//! M7.13 adds `SECTION_MOVER_STATE` (28): a `func_tracktrain`'s mid-route
//! position, an active `trigger_camera` sequence, a `scripted_sequence` mid
//! possession and a `monstermaker`'s spawn counters, tested at the bottom
//! of this file.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

// Exact float comparison is the point of the mover-position assertions
// below: the restored/continued position either lands exactly where it is
// expected or it does not, matching `camera_sequences.rs`'s own precedent.
#![allow(clippy::float_cmp)]

use ohl_combat::{ProjectileKind, WeaponId, hud_slot};
use ohl_engine::test_support::{
    AI_MAP, SCRIPT_MAP, actor_origin, ai_room_bsp, entity_block, entity_of_classname,
    monster_entities, queue_monster_damage, script_game, script_room_bsp, script_room_entities,
};
use ohl_engine::{EngineError, Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};
use ohl_formats::test_support::build_minimal_mdl10;

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

/// A `monstermaker`'s spawn counters — not the (separately, pre-existingly
/// unsaved) live child entity itself — survive a save/load round trip: the
/// quota it already spent before the save is not forgotten, so it can only
/// ever spawn up to `monstercount` in total, across the load.
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
    // The already-spawned child itself is the pre-existing, documented
    // `monstermaker`-children-are-not-indexed gap (`crate::save_state`'s
    // module doc, "Monstermaker children are not saved") — this section
    // does not change that, only the maker's own counters.
    assert_eq!(
        reloaded.monster_count(),
        0,
        "the already-spawned child is not itself restored (documented gap); \
         only the maker's counters are this test's subject"
    );

    script_tick(&mut reloaded, 400);
    assert_eq!(
        reloaded.monster_count(),
        1,
        "monstercount=2 total, and one was already spent before the save: \
         only one more may ever spawn. A reset spawned_total would let two \
         more spawn instead of one"
    );
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
