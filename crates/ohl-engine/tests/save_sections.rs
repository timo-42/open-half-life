//! M7.9 P4b: the five additive save sections (`SECTION_INVENTORY` 23,
//! `SECTION_ENTITY_COMBAT` 24, `SECTION_AI` 25, `SECTION_PROJECTILES` 26,
//! `SECTION_RNG` 27), exercised through the full `Game` loop.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_combat::{ProjectileKind, WeaponId, hud_slot};
use ohl_engine::test_support::{AI_MAP, ai_room_bsp, monster_entities, queue_monster_damage};
use ohl_engine::{EngineError, Game, Input, MemoryAssets, TICK_SECONDS};

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
