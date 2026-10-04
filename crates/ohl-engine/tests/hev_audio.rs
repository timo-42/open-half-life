//! Synthetic HEV delivery through the existing pickup and burn-trigger paths.
//! Only the reviewed sentence identifier is game data; words and fixtures are
//! project-authored. These tests do not load media or require an audio device.

use ohl_engine::test_support::{SCRIPT_MAP, entity_block, script_room_bsp, script_room_entities};
use ohl_engine::{ChannelClass, Game, GameEvent, Input, MemoryAssets, SoundAsset, TICK_SECONDS};
use ohl_player::SuitOccasion;

fn assets(sentences: Option<&[u8]>) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    let extra = format!(
        "{}{}",
        entity_block("item_suit", [-160.0, -160.0, 36.0], 0.0, &[]),
        entity_block(
            "trigger_hurt",
            [64.0, 64.0, 36.0],
            0.0,
            &[("dmg", "2"), ("damagetype", "8")],
        ),
    );
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&script_room_entities([-160.0, -160.0, 36.0], &extra)),
    );
    if let Some(sentences) = sentences {
        assets.insert("sound/sentences.txt", sentences.to_vec());
    }
    assets
}

fn enter_burn(assets: &MemoryAssets) -> Game {
    let mut game = Game::load(assets, SCRIPT_MAP).expect("synthetic room loads");
    assert!(!game.player_suit_equipped());
    game.tick(TICK_SECONDS, &Input::default());
    assert!(
        game.player_suit_equipped(),
        "ordinary pickup equips the suit before burn contact"
    );
    assert!((game.player_health() - 100.0).abs() < f32::EPSILON);
    // Existing debug placement enters the remote trigger only after pickup.
    game.set_viewpoint([64.0, 64.0, 36.0], 0.0, 0.0);
    game
}

fn first_burn_events(game: &mut Game, frame_seconds: f32) -> Vec<GameEvent> {
    let before = game.player_health();
    // The suit-acquisition tick already started the existing hurt interval.
    // Wait naturally for damage, preserving that frame's output even when
    // the audio adapter is stubbed. No producer timer is changed for setup.
    for _ in 0..100 {
        let events = game.tick(frame_seconds, &Input::default());
        if game.player_health() < before {
            return events;
        }
    }
    panic!("the synthetic burn trigger must damage the suited player");
}

#[test]
fn hev_audio_burn_game_event_is_once_per_emission_across_frame_boundaries() {
    let assets = assets(Some(b"HEV_FIRE ohl/burn_first ohl/burn_second\n"));
    let mut game = enter_burn(&assets);
    assert!(
        game.tick(0.0, &Input::default()).is_empty(),
        "zero-step frame adds no cue"
    );
    let output = first_burn_events(&mut game, TICK_SECONDS * 3.0);
    assert!(
        game.player_health() < 100.0,
        "existing burn producer actually damaged the player"
    );
    let mut sounds = Vec::new();
    let mut suits = Vec::new();
    for event in output {
        match event {
            GameEvent::Sound(cue) if cue.class == ChannelClass::Voice => sounds.push(cue),
            GameEvent::Suit(suit) => suits.push(suit),
            _ => {}
        }
    }
    assert_eq!(suits.len(), 1);
    assert_eq!(suits[0].occasion, SuitOccasion::HeatDamage);
    assert_eq!(
        sounds.len(),
        suits.len(),
        "several fixed steps do not replay the event"
    );
    assert_eq!(sounds[0].entity, game.player_entity().id());
    assert_eq!(
        sounds[0].asset,
        SoundAsset::sentence(vec![
            "sound/ohl/burn_first.wav".into(),
            "sound/ohl/burn_second.wav".into()
        ])
    );
    assert_eq!(sounds[0].origin, None);
    assert!(
        game.tick(0.0, &Input::default()).is_empty(),
        "second drain is empty"
    );
    for _ in 0..100 {
        assert!(
            !game.tick(TICK_SECONDS, &Input::default()).iter().any(
                |event| matches!(event, GameEvent::Sound(cue) if cue.class == ChannelClass::Voice)
            ),
            "existing cooldown suppresses later repeated condition"
        );
    }
}

#[test]
fn hev_audio_missing_sentence_keeps_metadata_and_one_unresolved_cue() {
    let assets = assets(None);
    let mut game = enter_burn(&assets);
    let output = first_burn_events(&mut game, TICK_SECONDS);
    let mut sounds = Vec::new();
    let mut suit_count = 0;
    for event in output {
        match event {
            GameEvent::Sound(cue) if cue.class == ChannelClass::Voice => sounds.push(cue),
            GameEvent::Suit(suit) if suit.occasion == SuitOccasion::HeatDamage => suit_count += 1,
            _ => {}
        }
    }
    assert_eq!(suit_count, 1);
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].asset, SoundAsset::Unresolved);
}
