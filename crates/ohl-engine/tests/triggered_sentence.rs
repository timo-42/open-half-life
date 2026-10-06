//! A project-authored reception-style trigger chain: walking into a volume
//! starts a delayed, explicitly named sentence. No game data is used.

use ohl_engine::test_support::{
    SCRIPT_MAP, door_behind_touch_trigger_bsp, entity_block, script_room_entities,
};
use ohl_engine::{ChannelClass, Game, GameEvent, Input, MemoryAssets, SoundAsset, TICK_SECONDS};

fn game() -> Game {
    let entities = script_room_entities(
        [-96.0, 0.0, 36.0],
        &format!(
            "{}{}{}{}",
            entity_block(
                "trigger_once",
                [0.0; 3],
                0.0,
                &[("model", "*2"), ("target", "ohl_arrival")],
            ),
            entity_block(
                "multi_manager",
                [0.0; 3],
                0.0,
                &[("targetname", "ohl_arrival"), ("ohl_line", "0.25")],
            ),
            entity_block(
                "scripted_sentence",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "ohl_line"),
                    ("sentence", "!OHL_ARRIVAL"),
                    ("entity", "ohl_receptionist"),
                    ("spawnflags", "1"),
                ],
            ),
            entity_block(
                "monster_barney",
                [160.0, 96.0, 0.0],
                180.0,
                &[("targetname", "ohl_receptionist"), ("spawnflags", "256")],
            ),
        ),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        door_behind_touch_trigger_bsp(&entities),
    );
    assets.insert(
        "sound/sentences.txt",
        b"OHL_ARRIVAL ohl/welcome ohl/visitor\n".to_vec(),
    );
    Game::load(&assets, SCRIPT_MAP).expect("the synthetic reception loads")
}

fn voice_cues(game: &mut Game, input: &Input, ticks: usize) -> Vec<SoundAsset> {
    (0..ticks)
        .flat_map(|_| game.tick(TICK_SECONDS, input))
        .filter_map(|event| match event {
            GameEvent::Sound(cue) if cue.class == ChannelClass::Voice => Some(cue.asset),
            _ => None,
        })
        .collect()
}

#[test]
fn entering_the_trigger_plays_its_named_sentence_once() {
    let mut game = game();
    assert!(voice_cues(&mut game, &Input::default(), 120).is_empty());
    assert_eq!(game.touch_trigger_count(), 0);

    let cues = voice_cues(
        &mut game,
        &Input {
            forward: 1,
            ..Input::default()
        },
        200,
    );
    assert_eq!(game.touch_trigger_count(), 1, "walking fires the trigger");
    assert_eq!(
        cues,
        [SoundAsset::sentence([
            "sound/ohl/welcome.wav".into(),
            "sound/ohl/visitor.wav".into(),
        ])],
        "the triggered line resolves to playable word samples"
    );

    // Cross the entrance again: neither the touch trigger nor its Fire
    // Once sentence may replay the greeting.
    assert!(
        voice_cues(
            &mut game,
            &Input {
                forward: -1,
                ..Input::default()
            },
            200,
        )
        .is_empty()
    );
    assert_eq!(game.touch_trigger_count(), 1);
}
