//! `ambient_generic`: the level's own soundscape, reaching the host as
//! `GameEvent::Sound` over a synthetic room.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes, and no asset path, come from any game installation. The
//! `message` values below are synthetic names this project invented for
//! its own fixture. Every keyvalue and spawnflag used is a published one
//! recorded in `docs/FORMAT_SOURCES.md`, "`ambient_generic`".

use ohl_engine::test_support::{entity_block, script_game, script_room_entities};
use ohl_engine::{ChannelClass, GameEvent, Input, SoundAsset, TICK_SECONDS};

/// The synthetic, project-authored sound name the fixtures name.
const SYNTHETIC_WAV: &str = "ohl/synthetic.wav";

/// A `trigger_auto` that fires `target` as soon as the map has loaded.
fn trigger_auto(target: &str) -> String {
    entity_block("trigger_auto", [0.0, 0.0, 0.0], 0.0, &[("target", target)])
}

fn ambient(name: &str, origin: [f32; 3], keys: &[(&str, &str)]) -> String {
    let mut all: Vec<(&str, &str)> = vec![("targetname", name), ("message", SYNTHETIC_WAV)];
    all.extend_from_slice(keys);
    entity_block("ambient_generic", origin, 0.0, &all)
}

/// Every sound cue `ticks` steps produce, in order.
fn sound_cues(game: &mut ohl_engine::Game, ticks: usize) -> Vec<ohl_engine::SoundCue> {
    let input = Input::default();
    let mut cues = Vec::new();
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, &input) {
            if let GameEvent::Sound(cue) = event {
                cues.push(cue);
            }
        }
    }
    cues
}

/// The published default: without "Start silent" the sound "will play as
/// soon as the map has loaded". It is announced once, on the static
/// channel, at the entity's own position, and never re-announced while it
/// keeps playing.
#[test]
fn an_ambient_generic_announces_itself_once_when_the_map_loads() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &ambient("ohl_hum", [64.0, 0.0, 48.0], &[]),
    );
    let mut game = script_game(&entities);

    let cues = sound_cues(&mut game, 30);
    assert_eq!(cues.len(), 1, "one start, and no repeats: {cues:?}");
    let cue = &cues[0];
    assert_eq!(cue.class, ChannelClass::Static);
    assert!(!cue.stop);
    assert_eq!(cue.origin, Some([64.0, 0.0, 48.0]));
    assert_eq!(
        cue.asset,
        SoundAsset::file(format!("sound/{SYNTHETIC_WAV}")),
        "the published `message` is `sound/`-relative"
    );
}

/// The published "Start silent" spawnflag (16): the entity "must be
/// triggered to work". Nothing sounds until the map's own `trigger_auto`
/// reaches it by name, and the sound then arrives at the volume and pitch
/// the published `health`/`pitch` keyvalues asked for.
#[test]
fn a_start_silent_ambient_generic_sounds_only_once_it_is_triggered() {
    let silent = ambient(
        "ohl_alarm",
        [0.0, 0.0, 36.0],
        &[("spawnflags", "16"), ("health", "5"), ("pitch", "120")],
    );
    let mut game = script_game(&script_room_entities([-192.0, -192.0, 36.0], &silent));
    assert!(
        sound_cues(&mut game, 10).is_empty(),
        "a start-silent ambient makes no sound of its own"
    );

    let triggered = format!("{silent}{}", trigger_auto("ohl_alarm"));
    let mut game = script_game(&script_room_entities([-192.0, -192.0, 36.0], &triggered));
    let cues = sound_cues(&mut game, 30);
    assert_eq!(cues.len(), 1, "the trigger_auto starts it exactly once");
    // The published `health` is a volume "in a range from 0 (not audible)
    // to 10 (normal)", and `pitch` is "sound playback speed, in per-cent".
    assert!((cues[0].volume - 0.5).abs() < 1e-6, "{:?}", cues[0].volume);
    assert!((cues[0].pitch - 1.2).abs() < 1e-6, "{:?}", cues[0].pitch);
}

/// A looped `ambient_generic` toggles: the second activation stops it, and
/// the host is told so with a stop cue naming the same entity and channel
/// the start cue named.
#[test]
fn toggling_a_looped_ambient_generic_off_emits_a_stop_cue() {
    // Two `trigger_auto`s with different delays: the first starts the
    // sound, the second toggles it back off.
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            ambient("ohl_hum", [0.0, 0.0, 36.0], &[("spawnflags", "16")]),
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[("target", "ohl_hum"), ("delay", "0.1")],
            ),
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[("target", "ohl_hum"), ("delay", "1.0")],
            ),
        ),
    );
    let mut game = script_game(&entities);

    let cues = sound_cues(&mut game, 120);
    assert_eq!(cues.len(), 2, "one start and one stop: {cues:?}");
    assert!(!cues[0].stop);
    assert!(cues[1].stop, "the second activation switches it off");
    assert_eq!(cues[0].entity, cues[1].entity);
    assert_eq!(cues[1].class, ChannelClass::Static);
    assert!(
        cues[1].asset.is_unresolved(),
        "a stop cue names nothing to play"
    );
}

/// The published "Is NOT looped" spawnflag (32) makes the entity
/// "interpret each call as 'turn on' instead of 'toggle state'": a second
/// activation restarts the sound, which the host sees as a second start
/// cue rather than a stop.
#[test]
fn an_unlooped_ambient_generic_restarts_rather_than_stopping() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            ambient("ohl_beep", [0.0, 0.0, 36.0], &[("spawnflags", "48")]),
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[("target", "ohl_beep"), ("delay", "0.1")],
            ),
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[("target", "ohl_beep"), ("delay", "1.0")],
            ),
        ),
    );
    let mut game = script_game(&entities);

    let cues = sound_cues(&mut game, 120);
    assert_eq!(cues.len(), 2, "two starts: {cues:?}");
    assert!(cues.iter().all(|cue| !cue.stop));
}

/// The published "Play everywhere" spawnflag (1) is the one radius reading
/// that never attenuates; a "Small radius" one falls off fastest. Both are
/// carried on the cue so the composition root does not have to know the
/// spawnflags at all.
#[test]
fn the_radius_spawnflags_choose_the_cues_attenuation() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}",
            ambient("ohl_everywhere", [0.0, 0.0, 36.0], &[("spawnflags", "1")]),
            ambient("ohl_small", [32.0, 0.0, 36.0], &[("spawnflags", "2")]),
        ),
    );
    let mut game = script_game(&entities);

    let mut cues = sound_cues(&mut game, 30);
    assert_eq!(cues.len(), 2);
    cues.sort_by(|a, b| a.attenuation.total_cmp(&b.attenuation));
    assert!(
        (cues[0].attenuation - ohl_engine::ATTN_NONE).abs() < 1e-6,
        "play everywhere never attenuates"
    );
    assert!(
        (cues[1].attenuation - ohl_engine::ATTN_IDLE).abs() < 1e-6,
        "a small radius is the fastest published falloff"
    );
}

/// An `ambient_generic` whose `message` names nothing, or names a
/// `!SENTENCE` this payload does not publish, still reaches the host as a
/// cue — it just carries nothing playable, so the composition root drops
/// it. Nothing here may invent an asset path to fill the gap with.
#[test]
fn an_ambient_generic_with_no_resolvable_message_cues_nothing_playable() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}",
            entity_block(
                "ambient_generic",
                [0.0, 0.0, 36.0],
                0.0,
                &[("targetname", "ohl_empty"), ("message", "")],
            ),
            entity_block(
                "ambient_generic",
                [16.0, 0.0, 36.0],
                0.0,
                &[("targetname", "ohl_spoken"), ("message", "!OHL_NOTHING")],
            ),
        ),
    );
    let mut game = script_game(&entities);

    let cues = sound_cues(&mut game, 30);
    assert_eq!(cues.len(), 2);
    assert!(cues.iter().all(|cue| cue.asset.is_unresolved()));
}
