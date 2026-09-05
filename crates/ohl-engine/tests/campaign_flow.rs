//! Campaign flow (M8.2): level transitions, save/load, chapter titles,
//! HUD messages and the difficulty-selected `skill.cfg` table.
//!
//! Every fixture here is project-authored synthetic data (a two-map
//! campaign built from `ohl_engine::test_support`'s room, plus small
//! invented `titles.txt`/`skill.cfg` files); nothing comes from any game
//! installation. See `docs/CLEAN_ROOM.md`.

use ohl_campaign::Difficulty;
use ohl_engine::test_support::synthetic_map_bsp_with_entities;
use ohl_engine::{Game, GameConfig, GameEvent, Input, MemoryAssets};
use ohl_game::hecs::Entity;
use ohl_game::registry::{Light, TargetName, Transform};

/// The landmark both maps of the fixture campaign declare.
const LANDMARK: &str = "ohl_lm";

/// The named entity that travels between them.
const CARRIED: &str = "ohl_lamp";

/// Map A: a player start, the landmark at `16 0 0`, a named light at
/// `48 0 16` (inside the default carry radius), and a `trigger_changelevel`
/// to map B.
fn map_a_entities(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 0\"\n}}\n\
         {{\n\"classname\" \"light\"\n\"targetname\" \"{CARRIED}\"\n\
         \"origin\" \"48 0 16\"\n\"_light\" \"200\"\n\"style\" \"3\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_exit\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n}}\n"
    )
}

/// Map B: the same landmark at a different world position, so a correct
/// transition is visible as a translation.
fn map_b_entities(newunit: bool) -> String {
    let newunit = if newunit { "\"newunit\" \"1\"\n" } else { "" };
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{newunit}}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"100 0 0\"\n}}\n"
    )
}

/// A map with a button the player spawns next to, wired to an
/// `env_message` naming a `titles.txt` entry.
fn message_map_entities() -> String {
    "{\n\"classname\" \"worldspawn\"\n}\n\
     {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"0\"\n}\n\
     {\n\"classname\" \"func_button\"\n\"target\" \"ohl_msg\"\n\
     \"origin\" \"0 40 32\"\n\"speed\" \"50\"\n\"wait\" \"1\"\n\"delay\" \"0\"\n}\n\
     {\n\"classname\" \"env_message\"\n\"targetname\" \"ohl_msg\"\n\
     \"message\" \"OHL_TEST\"\n}\n"
        .to_string()
}

/// A project-authored `titles.txt` exercising the directive state the HUD
/// timing comes from.
const TITLES: &[u8] = b"$fadein 0.5\n$fadeout 1.5\n$holdtime 3\n$color 255 200 100\n\
OHL_TEST\n{\nRadiation levels nominal.\n}\n";

/// A project-authored `skill.cfg` with one cvar at all three difficulties.
const SKILL_CFG: &[u8] = b"sk_ohl_probe_health1 \"10\"\n\
sk_ohl_probe_health2 \"20\"\n\
sk_ohl_probe_health3 \"30\"\n";

/// The two-map fixture campaign, published as an asset source.
fn campaign(newunit: bool) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(
        "maps/ohl_a.bsp",
        synthetic_map_bsp_with_entities(&map_a_entities("ohl_b")),
    );
    assets.insert(
        "maps/ohl_b.bsp",
        synthetic_map_bsp_with_entities(&map_b_entities(newunit)),
    );
    assets.insert("skill.cfg", SKILL_CFG.to_vec());
    assets.insert("titles.txt", TITLES.to_vec());
    assets
}

/// The named entity's world position in the current level, if it is there.
fn carried_origin(game: &Game) -> Option<[f32; 3]> {
    let registry = game.registry();
    for (name, transform) in &mut registry.world.query::<(&TargetName, &Transform)>() {
        if name.0 == CARRIED {
            return Some(transform.origin.to_array());
        }
    }
    None
}

fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected:?}, got {actual:?}"
        );
    }
}

#[test]
fn a_transition_places_the_player_and_a_carried_entity_relative_to_the_landmarks() {
    let assets = campaign(false);
    let mut game = Game::load(&assets, "ohl_a").expect("map A loads");
    let before = game.eye_position();

    game.change_level(&assets, "ohl_b", LANDMARK)
        .expect("map B loads");

    assert_eq!(game.map(), "ohl_b");
    // Both landmarks are on the X axis, 84 units apart.
    assert_close(
        game.eye_position(),
        [before[0] + 84.0, before[1], before[2]],
    );
    // The light stood 32 units in front of map A's landmark; it arrives 32
    // units in front of map B's.
    assert_close(
        carried_origin(&game).expect("the named light travelled"),
        [132.0, 0.0, 16.0],
    );
}

#[test]
fn a_carried_entity_keeps_its_component_state() {
    let assets = campaign(false);
    let mut game = Game::load(&assets, "ohl_a").expect("map A loads");
    game.change_level(&assets, "ohl_b", LANDMARK)
        .expect("map B loads");

    let registry = game.registry();
    let mut style = None;
    for (name, light) in &mut registry.world.query::<(&TargetName, &Light)>() {
        if name.0 == CARRIED {
            style = Some(light.style);
        }
    }
    assert_eq!(style, Some(3), "the light's style keyvalue travelled");
}

#[test]
fn a_newunit_destination_drops_the_carried_state() {
    let assets = campaign(true);
    let mut game = Game::load(&assets, "ohl_a").expect("map A loads");
    let before = game.eye_position();

    game.change_level(&assets, "ohl_b", LANDMARK)
        .expect("map B loads");

    assert!(
        carried_origin(&game).is_none(),
        "newunit discards carried entities"
    );
    // The player still arrives relative to the landmark.
    assert_close(
        game.eye_position(),
        [before[0] + 84.0, before[1], before[2]],
    );
}

#[test]
fn save_load_save_is_byte_identical() {
    let assets = campaign(false);
    let mut game = Game::load(&assets, "ohl_a").expect("map A loads");
    // Advance a little so the save holds a non-trivial simulation state.
    for _ in 0..8 {
        game.tick(1.0 / 60.0, &Input::default());
    }

    let first = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &first).expect("the save is read back");
    let second = reloaded
        .save_bytes(1_700_000_000)
        .expect("the reloaded game saves again");

    assert_eq!(first, second, "a save round trip is byte identical");
    assert_eq!(reloaded.map(), "ohl_a");
    assert_close(reloaded.eye_position(), game.eye_position());
}

#[test]
fn a_save_slot_round_trips_through_the_filesystem() {
    let assets = campaign(false);
    let game = Game::load(&assets, "ohl_a").expect("map A loads");
    let dir = tempfile::tempdir().expect("a temporary directory");
    let slot = ohl_save::SaveSlot::new(dir.path());

    game.save_slot(&slot, "ohl-test", 42)
        .expect("the slot writes");
    let loaded = Game::load_slot(&assets, &slot, "ohl-test").expect("the slot loads");

    assert_eq!(loaded.map(), game.map());
    assert_eq!(
        loaded.save_bytes(42).expect("re-save"),
        game.save_bytes(42).expect("save")
    );
}

#[test]
fn loading_a_campaign_map_announces_its_chapter_title() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{}.bsp", ohl_campaign::STARTMAP),
        synthetic_map_bsp_with_entities(&map_b_entities(false)),
    );
    let mut game = Game::load(&assets, ohl_campaign::STARTMAP).expect("the start map loads");

    let events = game.tick(1.0 / 60.0, &Input::default());
    let title = events.iter().find_map(|event| match event {
        GameEvent::ChapterTitle(title) => Some(title.clone()),
        _ => None,
    });
    assert_eq!(title.as_deref(), Some("Black Mesa Inbound"));
}

#[test]
fn a_map_outside_the_chapter_table_announces_nothing() {
    let assets = campaign(false);
    let mut game = Game::load(&assets, "ohl_a").expect("map A loads");
    let events = game.tick(1.0 / 60.0, &Input::default());
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, GameEvent::ChapterTitle(_)))
    );
}

#[test]
fn an_env_message_resolves_its_titles_block_and_timings() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        "maps/ohl_msg.bsp",
        synthetic_map_bsp_with_entities(&message_map_entities()),
    );
    assets.insert("titles.txt", TITLES.to_vec());
    let mut game = Game::load(&assets, "ohl_msg").expect("the map loads");

    let mut block = None;
    let mut input = Input {
        use_pressed: true,
        ..Input::default()
    };
    for _ in 0..16 {
        for event in game.tick(1.0 / 60.0, &input) {
            if let GameEvent::Message { block: message } = event {
                block = Some(message);
            }
        }
        input.use_pressed = false;
        if block.is_some() {
            break;
        }
    }

    let block = block.expect("the button's env_message fired");
    assert_eq!(block.text, "Radiation levels nominal.");
    assert!((block.fadein - 0.5).abs() < f32::EPSILON);
    assert!((block.fadeout - 1.5).abs() < f32::EPSILON);
    assert!((block.holdtime - 3.0).abs() < f32::EPSILON);
    assert!((block.total_seconds() - 5.0).abs() < 0.001);
    assert_eq!(block.color, [255, 200, 100]);
}

#[test]
fn the_skill_table_is_read_at_the_selected_difficulty() {
    let assets = campaign(false);
    for (difficulty, expected) in [
        (Difficulty::Easy, "10"),
        (Difficulty::Medium, "20"),
        (Difficulty::Hard, "30"),
    ] {
        let game =
            Game::load_with(&assets, "ohl_a", &GameConfig { difficulty }).expect("map A loads");
        assert_eq!(game.skill("sk_ohl_probe_health"), Some(expected));
        assert_eq!(game.difficulty(), difficulty);
    }
}

#[test]
fn a_sentence_lookup_names_one_asset_per_word() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        "maps/ohl_a.bsp",
        synthetic_map_bsp_with_entities(&map_a_entities("ohl_b")),
    );
    assets.insert(
        "sound/sentences.txt",
        b"OHL_HELLO vox/hello vox/there\n".to_vec(),
    );
    let game = Game::load(&assets, "ohl_a").expect("map A loads");

    let words: Vec<String> = game
        .sentences()
        .words("OHL_HELLO")
        .into_iter()
        .map(|path| path.0)
        .collect();
    assert_eq!(words, ["sound/vox/hello.wav", "sound/vox/there.wav"]);
    assert!(game.sentences().words("OHL_MISSING").is_empty());
}

#[test]
fn the_registry_snapshot_covers_every_entity_in_spawn_order() {
    let assets = campaign(false);
    let game = Game::load(&assets, "ohl_a").expect("map A loads");
    let save = game.to_save(0);
    assert_eq!(save.entities.len(), game.registry().entities.len());
    // Every entity in this fixture has a transform, so none of the
    // snapshots may be empty.
    assert!(
        save.entities
            .iter()
            .all(|entity| entity.transform.is_some())
    );
    let _: Vec<Entity> = game.registry().entities.clone();
}
