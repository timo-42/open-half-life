//! M7.11: scripted sequences, talk-monster following and
//! `scripted_sentence`, over a synthetic room.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. Every keyvalue and spawnflag the
//! entity blocks below use is a published one recorded in
//! `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters".

use ohl_engine::test_support::{
    SCRIPT_MAP, actor_origin, entity_block, entity_of_classname, queue_monster_damage, script_game,
    script_room_bsp, script_room_entities, strip_monster_ai, use_input,
};
use ohl_engine::{Game, GameEvent, Input, MemoryAssets, StudioAnim, TICK_SECONDS};

/// A `trigger_auto` that fires `target` as soon as the map has loaded.
fn trigger_auto(target: &str) -> String {
    entity_block("trigger_auto", [0.0, 0.0, 0.0], 0.0, &[("target", target)])
}

/// A `trigger_changelevel` a script's `target` can name; firing it is
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

/// Steps `game`, collecting how many `GameEvent::LevelChange`s it produced.
fn tick_counting_level_changes(game: &mut Game, ticks: usize) -> usize {
    let input = Input::default();
    let mut fired = 0;
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired += 1;
            }
        }
    }
    fired
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

/// A `trigger_auto` starts a walking `scripted_sequence`: the guard leaves
/// its spawn, reaches the script's mark, and the script fires its `target`
/// exactly once when the action animation finishes.
#[test]
fn a_triggered_script_walks_its_monster_to_the_mark_and_fires_once() {
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
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let spawn = actor_origin(&game, guard);

    // The auto trigger fires on the first tick, so the script takes the
    // guard over almost immediately.
    tick(&mut game, 5);
    assert_eq!(
        game.active_script_count(),
        1,
        "the script possesses the guard"
    );
    assert_eq!(game.script_start_count(), 1);

    // 160 units at the walking speed is a few seconds; give it plenty.
    let fired = tick_counting_level_changes(&mut game, 1_200);
    let arrived = actor_origin(&game, guard);
    assert!(
        arrived.x > spawn.x + 64.0,
        "the guard walked toward the mark"
    );
    assert!(
        (arrived.x - 160.0).abs() <= 32.0,
        "the guard stopped at the mark"
    );
    assert_eq!(fired, 1, "the script's target fired exactly once");
    assert_eq!(game.script_completion_count(), 1);
    assert_eq!(game.active_script_count(), 0, "the script let the guard go");

    // A spent, non-repeatable script never fires again.
    assert_eq!(tick_counting_level_changes(&mut game, 600), 0);
    assert_eq!(game.script_completion_count(), 1);
}

/// A script with `No Interruptions` (spawnflag 32) keeps its monster
/// through damage that would otherwise abandon an ordinary script.
#[test]
fn a_no_interruptions_script_ignores_damage_applied_mid_script() {
    let script = |flags: &str| {
        script_room_entities(
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
                        ("m_fMoveTo", "1"),
                        ("spawnflags", flags),
                    ],
                ),
                trigger_auto("ohl_script"),
            ),
        )
    };

    let mut protected = script_game(&script("32"));
    let mut ordinary = script_game(&script("0"));
    for game in [&mut protected, &mut ordinary] {
        tick(game, 10);
        assert_eq!(game.active_script_count(), 1);
        let guard = entity_of_classname(game, "monster_barney").expect("the guard spawned");
        ohl_engine::test_support::queue_monster_damage(game, guard, None, 5.0);
        tick(game, 5);
    }

    assert_eq!(
        protected.active_script_count(),
        1,
        "No Interruptions keeps the guard through damage"
    );
    assert_eq!(
        ordinary.active_script_count(),
        0,
        "an ordinary script lets go when its monster is hurt"
    );
}

/// A `scripted_sequence` with "Move to Position" = "Instantaneous" warps
/// its monster onto the mark instead of walking.
#[test]
fn an_instantaneous_script_warps_its_monster_onto_the_mark() {
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
                [96.0, 96.0, 36.0],
                180.0,
                &[
                    ("targetname", "ohl_script"),
                    ("m_iszEntity", "ohl_guard"),
                    ("m_fMoveTo", "4"),
                ],
            ),
            trigger_auto("ohl_script"),
        ),
    );
    let mut game = script_game(&entities);
    tick(&mut game, 10);
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let origin = actor_origin(&game, guard);
    assert!((origin.x - 96.0).abs() < 1.0 && (origin.y - 96.0).abs() < 1.0);
}

/// A script that names a *classname* rather than a `targetname` picks a
/// monster inside its search radius, and one that is out of radius is left
/// alone.
#[test]
fn a_classname_script_only_reaches_inside_its_search_radius() {
    let build = |radius: &str| {
        script_room_entities(
            [-192.0, -192.0, 36.0],
            &format!(
                "{}{}{}",
                entity_block("monster_barney", [0.0, 0.0, 36.0], 0.0, &[]),
                entity_block(
                    "scripted_sequence",
                    [200.0, 0.0, 36.0],
                    0.0,
                    &[
                        ("targetname", "ohl_script"),
                        ("m_iszEntity", "monster_barney"),
                        ("m_flRadius", radius),
                        ("m_fMoveTo", "0"),
                    ],
                ),
                trigger_auto("ohl_script"),
            ),
        )
    };
    let mut in_range = script_game(&build("512"));
    let mut out_of_range = script_game(&build("32"));
    tick(&mut in_range, 5);
    tick(&mut out_of_range, 5);
    assert_eq!(in_range.script_start_count(), 1);
    assert_eq!(out_of_range.script_start_count(), 0);
}

/// The player brings a scientist into their group with `use`, and sends it
/// away with a second `use`.
#[test]
fn a_scientist_follows_after_use_and_stops_after_a_second_use() {
    let entities = script_room_entities(
        [0.0, 0.0, 36.0],
        &entity_block("monster_scientist", [32.0, 0.0, 36.0], 180.0, &[]),
    );
    let mut game = script_game(&entities);
    let scientist = entity_of_classname(&game, "monster_scientist").expect("it spawned");
    assert!(game.followers().is_empty());

    game.tick(TICK_SECONDS, &use_input());
    tick(&mut game, 2);
    assert_eq!(game.followers(), &[scientist], "one use starts following");

    game.tick(TICK_SECONDS, &use_input());
    tick(&mut game, 2);
    assert!(
        game.followers().is_empty(),
        "a second use sends the scientist away"
    );
}

/// Project-authored reach is measured to the body volume. This ordinary Use
/// scene is outside the old center sphere but inside the unchanged hull reach.
#[test]
fn ordinary_use_recruits_a_scientist_within_hull_reach_but_beyond_center_reach() {
    use ohl_ai::{Actor, Follower};
    use ohl_engine::ai::TALK_USE_RADIUS;
    use ohl_game::hecs::Entity;
    use ohl_physics::{Hull, Vec3};

    let entities = script_room_entities(
        [72.0, 0.0, 36.0 + ohl_physics::DIST_EPSILON],
        &entity_block("monster_scientist", [0.0, 0.0, 0.0], 0.0, &[]),
    );
    let mut game = script_game(&entities);
    let scientist = entity_of_classname(&game, "monster_scientist").expect("it spawned");
    let qualify = |game: &Game, eye: Vec3| {
        assert!(game.followers().is_empty());
        assert!(game.player_health() > 0.0 && game.player_on_ground());
        let player = game
            .registry()
            .world
            .get::<&Actor>(game.player_entity())
            .unwrap();
        assert_eq!(player.hull, Hull::Standing);
        let center = Vec3::from_array(game.player_origin());
        let trace = game
            .collision()
            .unwrap()
            .trace(Hull::Standing, center, center);
        assert!(!trace.start_solid && !trace.all_solid);
        assert!(
            ohl_game::find_usable_within(game.registry(), eye, ohl_engine::USE_RADIUS).is_none(),
            "the ordinary dispatcher must offer this use to talk monsters"
        );
        let pairs: Vec<_> = game
            .registry()
            .world
            .query::<(Entity, &Actor, &Follower)>()
            .iter()
            .map(|(entity, _, _)| entity)
            .collect();
        assert_eq!(
            pairs,
            vec![scientist],
            "one candidate; no selection ambiguity"
        );
        let actor = game.registry().world.get::<&Actor>(scientist).unwrap();
        let follower = game.registry().world.get::<&Follower>(scientist).unwrap();
        assert!(actor.alive && actor.health.is_finite() && actor.health > 0.0);
        assert!(follower.can_follow && !follower.following);
        let (min, max) = actor.body_frame.world_bounds(actor.hull, actor.origin);
        assert!(eye.is_finite() && min.is_finite() && max.is_finite() && min.cmple(max).all());
        let body_distance = eye.distance(eye.clamp(min, max));
        let center_distance = eye.distance(actor.query_origin());
        assert!(body_distance.is_finite() && center_distance.is_finite());
        assert!(
            body_distance < TALK_USE_RADIUS && center_distance > TALK_USE_RADIUS,
            "real attached geometry distinguishes body reach from center reach"
        );
    };
    qualify(&game, Vec3::from_array(game.eye_position()));
    game.tick(TICK_SECONDS, &use_input());
    // Phase 12 queued this actual camera eye; the next ordinary AI phase
    // consumes it. No script, map-use target or competing talk actor intervenes.
    let dispatched_eye = Vec3::from_array(game.eye_position());
    qualify(&game, dispatched_eye);
    game.tick(TICK_SECONDS, &Input::default());
    assert_eq!(
        game.followers(),
        &[scientist],
        "ordinary use reaches the nearby body"
    );
    assert!(
        game.registry()
            .world
            .get::<&Follower>(scientist)
            .unwrap()
            .following
    );
}

/// A `Pre-Disaster` scientist (spawnflag 256) refuses to follow.
#[test]
fn a_pre_disaster_scientist_never_joins_the_player() {
    let entities = script_room_entities(
        [0.0, 0.0, 36.0],
        &entity_block(
            "monster_scientist",
            [32.0, 0.0, 36.0],
            180.0,
            &[("spawnflags", "256")],
        ),
    );
    let mut game = script_game(&entities);
    game.tick(TICK_SECONDS, &use_input());
    tick(&mut game, 2);
    assert!(game.followers().is_empty());
}

/// A `scripted_sentence` resolves its speaker and emits one voice cue,
/// spatialised where that speaker stands, then fires its `target`. This
/// fixture publishes no `sentences.txt`, so the sentence group resolves to
/// no word samples and the cue names nothing playable — exactly what the
/// composition root drops silently.
#[test]
fn a_scripted_sentence_speaks_through_a_cue_on_the_speakers_voice_channel() {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            entity_block(
                "monster_scientist",
                [0.0, 0.0, 36.0],
                0.0,
                &[("targetname", "ohl_speaker")],
            ),
            entity_block(
                "scripted_sentence",
                [0.0, 0.0, 36.0],
                0.0,
                &[
                    ("targetname", "ohl_line"),
                    ("sentence", "OHL_GREETING"),
                    ("entity", "ohl_speaker"),
                    ("spawnflags", "1"),
                    ("target", "ohl_after"),
                ],
            ),
            trigger_auto("ohl_line") + &exit_trigger("ohl_after"),
        ),
    );
    let mut game = script_game(&entities);

    let mut cues = 0;
    let mut level_changes = 0;
    let input = Input::default();
    for _ in 0..120 {
        for event in game.tick(TICK_SECONDS, &input) {
            match event {
                GameEvent::Sound(cue) => {
                    assert_eq!(cue.class, ohl_engine::ChannelClass::Voice);
                    assert!(
                        cue.origin.is_some(),
                        "a spoken line is heard where the speaker stands"
                    );
                    assert!(
                        cue.asset.is_unresolved(),
                        "this fixture publishes no sentences.txt to resolve against"
                    );
                    cues += 1;
                }
                GameEvent::LevelChange { .. } => level_changes += 1,
                _ => {}
            }
        }
    }
    assert_eq!(cues, 1, "a Fire Once sentence speaks exactly once");
    assert_eq!(level_changes, 1, "and fires its target");
}

/// With a `sentences.txt` to resolve against, the same cue carries the
/// sentence's word samples, in speaking order, each a `sound/`-relative WAV
/// — and it is heard where the *speaker* stands, not where the
/// `scripted_sentence` that directed it was placed.
///
/// The `sentences.txt` line, the sentence name and both words are
/// project-authored; the file's published shape (a name, then word tokens
/// naming `sound/`-relative samples) is recorded in
/// `docs/FORMAT_SOURCES.md`.
#[test]
fn a_scripted_sentence_names_its_words_and_is_heard_where_the_speaker_stands() {
    let speaker_at = [96.0, 64.0, 36.0];
    let director_at = [-96.0, -64.0, 36.0];
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            entity_block(
                "monster_scientist",
                speaker_at,
                0.0,
                &[("targetname", "ohl_speaker")],
            ),
            entity_block(
                "scripted_sentence",
                director_at,
                0.0,
                &[
                    ("targetname", "ohl_line"),
                    ("sentence", "OHL_GREETING"),
                    ("entity", "ohl_speaker"),
                    ("spawnflags", "1"),
                ],
            ),
            trigger_auto("ohl_line"),
        ),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    assets.insert(
        "sound/sentences.txt",
        b"OHL_GREETING ohl/hello ohl/there\n".to_vec(),
    );
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");
    let speaker = entity_of_classname(&game, "monster_scientist").expect("the speaker spawns");

    let input = Input::default();
    let mut spoken = Vec::new();
    for _ in 0..120 {
        for event in game.tick(TICK_SECONDS, &input) {
            if let GameEvent::Sound(cue) = event {
                spoken.push((cue, actor_origin(&game, speaker)));
            }
        }
    }
    assert_eq!(spoken.len(), 1, "a Fire Once sentence speaks exactly once");
    let (cue, speaker_origin) = &spoken[0];
    assert_eq!(cue.class, ohl_engine::ChannelClass::Voice);
    // No `volume` or `attenuation` key: full volume, and "Sound Radius" `0`
    // ("Small Radius"), the fastest published falloff.
    assert!((cue.volume - 1.0).abs() < 1e-6, "{}", cue.volume);
    assert!(
        (cue.attenuation - ohl_engine::ATTN_IDLE).abs() < 1e-6,
        "{}",
        cue.attenuation
    );
    assert_eq!(
        cue.asset,
        ohl_engine::SoundAsset::sentence(vec![
            "sound/ohl/hello.wav".to_string(),
            "sound/ohl/there.wav".to_string(),
        ]),
        "the group's words, in order, as `sound/`-relative samples"
    );
    let origin = cue.origin.expect("a spoken line is spatialised");
    let distance = |to: [f32; 3]| {
        origin
            .iter()
            .zip(to)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt()
    };
    assert!(
        distance(speaker_origin.to_array()) < 1.0,
        "heard at the speaker: {origin:?} vs {speaker_origin:?}"
    );
    assert!(
        distance(director_at) > 100.0,
        "not at the scripted_sentence that directed it: {origin:?}"
    );
}

/// The `sentences.txt` line every test below speaks from. Project-authored.
const GREETING_SENTENCES: &[u8] = b"OHL_GREETING ohl/hello ohl/there\n";

/// A scientist named `ohl_speaker` at `speaker_at`, and a Fire Once
/// `scripted_sentence` telling it to say `OHL_GREETING` with `keys` added,
/// started by a `trigger_auto` `delay` seconds after the map loads.
fn greeting_room(speaker_at: [f32; 3], delay: &str, keys: &[(&str, &str)]) -> String {
    let mut sentence_keys = vec![
        ("targetname", "ohl_line"),
        ("sentence", "OHL_GREETING"),
        ("entity", "ohl_speaker"),
        ("spawnflags", "1"),
    ];
    sentence_keys.extend_from_slice(keys);
    script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}{}",
            entity_block(
                "monster_scientist",
                speaker_at,
                0.0,
                &[("targetname", "ohl_speaker")],
            ),
            entity_block("scripted_sentence", [0.0, 0.0, 36.0], 0.0, &sentence_keys),
            entity_block(
                "trigger_auto",
                [0.0, 0.0, 0.0],
                0.0,
                &[("target", "ohl_line"), ("delay", delay)],
            ),
        ),
    )
}

/// Every voice cue `ticks` steps of `game` produce.
fn voice_cues(game: &mut Game, ticks: usize) -> Vec<ohl_engine::SoundCue> {
    let input = Input::default();
    let mut cues = Vec::new();
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, &input) {
            if let GameEvent::Sound(cue) = event
                && cue.class == ohl_engine::ChannelClass::Voice
            {
                cues.push(cue);
            }
        }
    }
    cues
}

/// The words `GREETING_SENTENCES` names, as the cue carries them.
fn greeting_words() -> ohl_engine::SoundAsset {
    ohl_engine::SoundAsset::sentence(vec![
        "sound/ohl/hello.wav".to_string(),
        "sound/ohl/there.wav".to_string(),
    ])
}

/// The published `volume` ("Range: 0 - 10") and "Sound Radius"
/// (`attenuation`: `3` is "Play Everywhere") reach the cue.
#[test]
fn a_scripted_sentence_speaks_at_its_published_volume_and_radius() {
    let entities = greeting_room(
        [0.0, 96.0, 36.0],
        "0",
        &[("volume", "5"), ("attenuation", "3")],
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    assets.insert("sound/sentences.txt", GREETING_SENTENCES.to_vec());
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");

    let cues = voice_cues(&mut game, 60);
    assert_eq!(cues.len(), 1);
    assert!((cues[0].volume - 0.5).abs() < 1e-6, "{}", cues[0].volume);
    assert!(
        (cues[0].attenuation - ohl_engine::ATTN_NONE).abs() < 1e-6,
        "{}",
        cues[0].attenuation
    );
}

/// A speaker that has died says nothing: a corpse keeps its `Actor`, but
/// not its voice. Killed before its line is due, the line never comes.
#[test]
fn a_dead_speaker_says_nothing() {
    let entities = greeting_room([0.0, 96.0, 36.0], "1.0", &[]);
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    assets.insert("sound/sentences.txt", GREETING_SENTENCES.to_vec());

    // Alive, it speaks: the control for the case below.
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");
    assert_eq!(voice_cues(&mut game, 120).len(), 1);

    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");
    let speaker = entity_of_classname(&game, "monster_scientist").expect("the speaker spawns");
    // Enough to kill it, not enough to gib it: a corpse that is still there,
    // still named, and still carrying its `Actor` is the case at issue.
    queue_monster_damage(&mut game, speaker, None, 25.0);
    tick(&mut game, 2);
    let actor = game
        .registry()
        .world
        .get::<&ohl_ai::Actor>(speaker)
        .map(|actor| actor.alive);
    assert_eq!(
        actor,
        Ok(false),
        "the fixture's speaker is a corpse, not gone, before its line"
    );
    assert!(
        voice_cues(&mut game, 120).is_empty(),
        "a corpse does not speak"
    );
}

/// The payload's `sentences.txt` is read once, when the game loads, and
/// every map after a level change speaks from it. The first map here does
/// nothing but leave; the second has the speaker.
#[test]
fn a_sentence_is_still_spoken_after_a_level_change() {
    const SECOND_MAP: &str = "ohlscriptsynthb";
    let leaving = script_room_entities(
        [-192.0, -192.0, 36.0],
        &format!(
            "{}{}",
            entity_block(
                "trigger_changelevel",
                [0.0, 0.0, 0.0],
                0.0,
                &[
                    ("targetname", "ohl_leave"),
                    ("map", SECOND_MAP),
                    ("landmark", "ohl_landmark"),
                ],
            ),
            trigger_auto("ohl_leave"),
        ),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), script_room_bsp(&leaving));
    assets.insert(
        &format!("maps/{SECOND_MAP}.bsp"),
        script_room_bsp(&greeting_room([0.0, 96.0, 36.0], "0.5", &[])),
    );
    assets.insert("sound/sentences.txt", GREETING_SENTENCES.to_vec());
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the first room loads");

    let input = Input::default();
    let mut changed = false;
    for _ in 0..60 {
        for event in game.tick(TICK_SECONDS, &input) {
            if let GameEvent::LevelChange { map, landmark } = event {
                game.change_level(&assets, &map, &landmark)
                    .expect("the second room loads");
                changed = true;
            }
        }
        if changed {
            break;
        }
    }
    assert!(changed, "the first room's trigger_changelevel fired");
    assert_eq!(game.map(), SECOND_MAP);

    let cues = voice_cues(&mut game, 120);
    assert_eq!(cues.len(), 1, "the second room's line is spoken");
    assert_eq!(
        cues[0].asset,
        greeting_words(),
        "spoken from the same sentences.txt the game loaded with"
    );
}

/// The same after a save is loaded: the loaded game reads the table again.
#[test]
fn a_sentence_is_still_spoken_after_a_save_is_loaded() {
    let entities = greeting_room([0.0, 96.0, 36.0], "1.0", &[]);
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    assets.insert("sound/sentences.txt", GREETING_SENTENCES.to_vec());
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");
    // Saved after the trigger_auto has scheduled the line and before it is
    // due, so the line is spoken by the loaded game.
    tick(&mut game, 10);
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");

    let mut loaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    let cues = voice_cues(&mut loaded, 120);
    assert_eq!(cues.len(), 1, "the loaded game speaks the line");
    assert_eq!(cues[0].asset, greeting_words());
}

/// Loading the same fixture twice with the same inputs reproduces the same
/// AI state hash, scripts and followers included.
#[test]
fn scripting_stays_deterministic() {
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
                    ("m_fMoveTo", "2"),
                    ("spawnflags", "4"),
                    ("m_flRepeat", "1"),
                ],
            ),
            trigger_auto("ohl_script"),
        ),
    );
    let bytes = script_room_bsp(&entities);
    let mut assets = ohl_engine::MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());

    let mut first = Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("loads");
    let mut second = Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("loads");
    let inputs: Vec<Input> = (0..600)
        .map(|step| {
            if step % 97 == 0 {
                use_input()
            } else {
                Input::default()
            }
        })
        .collect();
    ohl_engine::test_support::run_script(&mut first, &inputs);
    ohl_engine::test_support::run_script(&mut second, &inputs);
    assert_eq!(first.ai_state_hash(), second.ai_state_hash());
    assert_eq!(
        first.script_completion_count(),
        second.script_completion_count()
    );
    assert!(
        first.script_completion_count() >= 2,
        "a repeatable script with a repeat rate runs again on its own"
    );
}

/// The regression guard for the review's blocker: an **untriggered**
/// `scripted_sequence` that names a monster must leave that monster
/// entirely alone. A recruited scientist with a dormant script pointed at
/// it travels the same distance as an identical scientist with no script in
/// the map at all.
#[test]
fn a_dormant_script_does_not_immobilise_the_monster_it_names() {
    let room = |with_script: bool| {
        let mut extra = entity_block(
            "monster_scientist",
            [32.0, 0.0, 36.0],
            180.0,
            &[("targetname", "ohl_doc")],
        );
        if with_script {
            // A script that names the scientist and is never triggered:
            // no `trigger_auto`, nothing pointing at it.
            extra.push_str(&entity_block(
                "scripted_sequence",
                [-200.0, -200.0, 36.0],
                0.0,
                &[
                    ("targetname", "ohl_never_fired"),
                    ("m_iszEntity", "ohl_doc"),
                    ("m_iszPlay", "ohl_action"),
                    ("m_iszIdle", "ohl_wait"),
                    ("m_fMoveTo", "1"),
                ],
            ));
        }
        script_room_entities([0.0, 0.0, 36.0], &extra)
    };

    let travelled = |with_script: bool| {
        let mut game = script_game(&room(with_script));
        let doc = entity_of_classname(&game, "monster_scientist").expect("it spawned");
        // Recruit it, then walk the player away so the follower has
        // somewhere to go.
        game.tick(TICK_SECONDS, &use_input());
        tick(&mut game, 2);
        assert_eq!(game.followers(), &[doc], "the scientist joined");
        game.set_viewpoint([-224.0, 0.0, 36.0], 0.0, 0.0);
        let before = actor_origin(&game, doc);
        tick(&mut game, 600);
        (actor_origin(&game, doc) - before).length()
    };

    let control = travelled(false);
    let scripted = travelled(true);
    assert!(
        control > 32.0,
        "the control scientist actually followed the player ({control})"
    );
    assert!(
        (scripted - control).abs() < 1.0,
        "a dormant script must not slow its monster down \
         (control {control}, scripted {scripted})"
    );
}

/// A monster whose script was interrupted mid-sequence is handed back to
/// its own brain and moves again.
#[test]
fn an_interrupted_monster_can_move_again_afterwards() {
    let entities = script_room_entities(
        [0.0, 0.0, 36.0],
        &format!(
            "{}{}{}",
            entity_block(
                "monster_scientist",
                [32.0, 0.0, 36.0],
                180.0,
                &[("targetname", "ohl_doc")],
            ),
            entity_block(
                "scripted_sequence",
                [200.0, 0.0, 36.0],
                0.0,
                &[
                    ("targetname", "ohl_script"),
                    ("m_iszEntity", "ohl_doc"),
                    ("m_iszPlay", "ohl_action"),
                    ("m_fMoveTo", "1"),
                ],
            ),
            trigger_auto("ohl_script"),
        ),
    );
    let mut game = script_game(&entities);
    let doc = entity_of_classname(&game, "monster_scientist").expect("it spawned");

    game.tick(TICK_SECONDS, &use_input());
    tick(&mut game, 10);
    assert_eq!(game.followers().len(), 1, "the scientist joined");
    assert_eq!(game.active_script_count(), 1, "the script took the doctor");

    ohl_engine::test_support::queue_monster_damage(&mut game, doc, None, 1.0);
    tick(&mut game, 5);
    assert_eq!(game.active_script_count(), 0, "the damage interrupted it");

    game.set_viewpoint([-224.0, 0.0, 36.0], 0.0, 0.0);
    let before = actor_origin(&game, doc);
    tick(&mut game, 600);
    let travelled = (actor_origin(&game, doc) - before).length();
    assert!(
        travelled > 32.0,
        "an interrupted monster moves again ({travelled})"
    );
}

/// The player's group does not survive a level change: the entities in it
/// belong to the level that was unloaded.
#[test]
fn the_player_group_is_cleared_by_a_level_change() {
    let here = script_room_entities(
        [0.0, 0.0, 36.0],
        &format!(
            "{}{}",
            entity_block("monster_scientist", [32.0, 0.0, 36.0], 180.0, &[]),
            entity_block(
                "info_landmark",
                [0.0, 0.0, 0.0],
                0.0,
                &[("targetname", "ohl_landmark")],
            ),
        ),
    );
    let there = script_room_entities(
        [0.0, 0.0, 36.0],
        &entity_block(
            "info_landmark",
            [0.0, 0.0, 0.0],
            0.0,
            &[("targetname", "ohl_landmark")],
        ),
    );
    let mut assets = ohl_engine::MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), script_room_bsp(&here));
    assets.insert("maps/ohlscriptsynth2.bsp", script_room_bsp(&there));

    let mut game = Game::load(&assets, SCRIPT_MAP).expect("the script room loads");
    game.tick(TICK_SECONDS, &use_input());
    tick(&mut game, 2);
    assert_eq!(game.followers().len(), 1, "the scientist joined");

    game.change_level(&assets, "ohlscriptsynth2", "ohl_landmark")
        .expect("the destination map loads");
    assert!(
        game.followers().is_empty(),
        "a level change empties the player's group"
    );
    tick(&mut game, 10);
    assert!(game.followers().is_empty());
}

/// M7.11 follow-up ("Scripted-sequence probe", recorded in local
/// investigation notes and not part of the repository):
/// a walk-mode script bound to a monster that never moves — here made
/// inert by stripping its `MonsterAi` outright, the worst case of "cannot
/// reach the mark" — must still let go within
/// `ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS`, not hold the monster
/// forever. Regression guard for the round-3 probe's finding that
/// `active_script_count` could rise above zero and never return to it.
#[test]
fn a_walk_script_bound_to_an_inert_monster_does_not_stall_forever() {
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
                [800.0, 0.0, 36.0],
                90.0,
                &[
                    ("targetname", "ohl_script"),
                    ("m_iszEntity", "ohl_guard"),
                    ("m_iszPlay", "ohl_action"),
                    ("m_fMoveTo", "1"),
                    ("target", "ohl_after"),
                ],
            ),
            trigger_auto("ohl_script") + &exit_trigger("ohl_after"),
        ),
    );
    let mut game = script_game(&entities);
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let spawn = actor_origin(&game, guard);

    // The auto trigger fires on the first tick, so the script takes the
    // guard over almost immediately.
    tick(&mut game, 5);
    assert_eq!(
        game.active_script_count(),
        1,
        "the script possesses the guard"
    );

    // Now make the guard inert: no `MonsterAi` at all, so nothing ever
    // drives it toward the mark, however long the script waits.
    strip_monster_ai(&mut game, guard);

    // One tick short of the documented bound: still trying.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a tick count derived from two small, positive, fixed constants"
    )]
    let timeout_ticks = (f64::from(ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS)
        / f64::from(TICK_SECONDS)) as usize;
    tick(&mut game, timeout_ticks.saturating_sub(50));
    assert_eq!(
        game.active_script_count(),
        1,
        "still within the documented bound"
    );
    assert_eq!(game.script_timeout_count(), 0);

    // Comfortably past the bound: the script must have given up.
    tick(&mut game, 200);
    assert_eq!(
        game.active_script_count(),
        0,
        "an inert monster's script must not stall forever"
    );
    assert_eq!(game.script_timeout_count(), 1);
    assert_eq!(
        game.script_completion_count(),
        0,
        "a give-up is not a completion, so `target` never fires"
    );
    assert!(
        (actor_origin(&game, guard) - spawn).length() < 8.0,
        "an inert monster never walked toward the mark on its own"
    );
}

/// The M7.11 review's item 21 follow-up: a dormant `scripted_sequence`'s
/// `m_iszIdle` plays on its named monster while that monster's own AI is
/// idle, without ever possessing it; it steps aside the moment the AI
/// itself has something else to do; and once triggered, the ordinary
/// possessed path still takes over and hands the guard back to its own
/// idle afterwards — exactly the precedence `docs/FORMAT_SOURCES.md`'s
/// `TODO(black-box)` item 21 documents.
///
/// The monster's model publishes two sequences: `"idle"` (index 0) is what
/// `ohl_ai::Activity::Idle` resolves to under this crate's own vocabulary
/// name — the sequence the guard's own AI would pick with no script
/// involved at all — and `"ohl_wait"` (index 1) is the script's own
/// `m_iszIdle`. Asserting the *index* is what tells the two apart: a
/// single-sequence fixture could not distinguish "the AI's own idle
/// happened to land on the same slot" from "the script's idle actually
/// won".
#[test]
fn a_dormant_scripts_idle_animation_plays_while_its_monster_is_idle_and_yields_otherwise() {
    const IDLE_SEQUENCE: usize = 0;
    const SCRIPT_IDLE_SEQUENCE: usize = 1;

    let build_entities = |with_trigger: bool| {
        let mut extra = format!(
            "{}{}",
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
        );
        if with_trigger {
            extra.push_str(&trigger_auto("ohl_script"));
            extra.push_str(&exit_trigger("ohl_after"));
        }
        script_room_entities([-192.0, -192.0, 36.0], &extra)
    };

    let build_game = |with_trigger: bool| {
        let entities = build_entities(with_trigger);
        let bytes = script_room_bsp(&entities);
        let (mdl_bytes, _layout) =
            ohl_formats::test_support::build_minimal_mdl10_with_sequences(&["idle", "ohl_wait"]);
        let mut assets = MemoryAssets::new();
        assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());
        assets.insert("models/barney.mdl", mdl_bytes);
        Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("the room loads")
    };

    let sequence_of = |game: &Game, entity: ohl_game::hecs::Entity| -> usize {
        game.registry()
            .world
            .get::<&StudioAnim>(entity)
            .expect("the guard drew a model")
            .sequence
    };

    // --- Dormant: the pre-trigger idle plays, without possession. ---
    let mut game = build_game(false);
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let spawn = actor_origin(&game, guard);

    // Untriggered and idle: the script's idle plays, and the guard never
    // moves — the same "a dormant script never touches its monster's
    // movement" contract the other dormant-script test in this file
    // covers, checked again here alongside the animation.
    tick(&mut game, 10);
    assert_eq!(game.active_script_count(), 0, "the script never took over");
    assert_eq!(game.script_start_count(), 0);
    assert_eq!(
        sequence_of(&game, guard),
        SCRIPT_IDLE_SEQUENCE,
        "the pre-trigger idle plays while the guard's own AI is idle"
    );
    assert!(
        (actor_origin(&game, guard) - spawn).length() < 1.0,
        "the dormant script never moved the guard"
    );

    // Give the guard an enemy: its own AI stops being idle, so the
    // script's pre-trigger idle must step aside rather than fight the
    // AI's own activity selection for the sequence slot.
    let player = game.player_entity();
    queue_monster_damage(&mut game, guard, Some(player), 5.0);
    tick(&mut game, 5);
    assert_ne!(
        sequence_of(&game, guard),
        SCRIPT_IDLE_SEQUENCE,
        "an idle monster's own AI wins once it is no longer idle"
    );
    assert_eq!(
        game.active_script_count(),
        0,
        "the script still never took over: only its idle animation is shared"
    );

    // --- Triggered: the ordinary possessed path still takes over, and the
    // --- guard settles back on its own idle once the script is done. ---
    let mut game = build_game(true);
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let fired = tick_counting_level_changes(&mut game, 1_200);
    assert_eq!(fired, 1, "a triggered script still fires its target once");
    assert_eq!(game.script_completion_count(), 1);
    assert_eq!(game.active_script_count(), 0, "the script let the guard go");
    assert_eq!(
        sequence_of(&game, guard),
        IDLE_SEQUENCE,
        "released back to its own brain, the guard settles back on its own idle"
    );
}

/// Wave 1 batch A: the two scripted-prop kinds (`monster_generic`,
/// `monster_furniture`) are monsters a script can possess, so a
/// `scripted_sequence` naming one walks it to the mark and fires its
/// `target` exactly like a guard — where before this pass neither had an
/// `Actor` for the script to find and the sequence never started.
#[test]
fn a_script_possesses_a_generic_monster_and_a_piece_of_furniture() {
    for classname in ["monster_generic", "monster_furniture"] {
        let entities = script_room_entities(
            [-192.0, -192.0, 36.0],
            &format!(
                "{}{}{}",
                entity_block(
                    classname,
                    [0.0, 0.0, 36.0],
                    0.0,
                    &[("targetname", "ohl_prop")]
                ),
                entity_block(
                    "scripted_sequence",
                    [160.0, 0.0, 36.0],
                    90.0,
                    &[
                        ("targetname", "ohl_script"),
                        ("m_iszEntity", "ohl_prop"),
                        ("m_iszPlay", "ohl_action"),
                        ("m_fMoveTo", "1"),
                        ("target", "ohl_after"),
                    ],
                ),
                trigger_auto("ohl_script") + &exit_trigger("ohl_after"),
            ),
        );
        let mut game = script_game(&entities);
        let prop = entity_of_classname(&game, classname).expect("the prop spawned");
        let spawn = actor_origin(&game, prop);
        assert!(
            game.registry()
                .world
                .get::<&ohl_ai::MonsterAi>(prop)
                .is_ok(),
            "{classname} thinks (has a MonsterAi) rather than being an inert actor"
        );

        tick(&mut game, 5);
        assert_eq!(
            game.active_script_count(),
            1,
            "{classname}: the script possesses the prop"
        );
        assert_eq!(game.script_start_count(), 1);

        let fired = tick_counting_level_changes(&mut game, 1_200);
        let arrived = actor_origin(&game, prop);
        assert!(
            arrived.x > spawn.x + 64.0,
            "{classname}: the prop walked toward the mark"
        );
        assert_eq!(
            fired, 1,
            "{classname}: the script's target fired exactly once"
        );
        assert_eq!(game.script_completion_count(), 1);
        assert_eq!(game.active_script_count(), 0);

        // Released, a prop's own brain never walks it anywhere.
        let released = actor_origin(&game, prop);
        tick(&mut game, 600);
        assert!(
            (actor_origin(&game, prop) - released).length() < 1.0,
            "{classname}: a released prop stays put"
        );
    }
}

/// Two ordinary map activations must not let a later script release the
/// actor while an earlier script still owns its unfinished approach.
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "keep ordinary activation and geometry prerequisites before the ownership oracle"
)]
fn a_contending_script_keeps_the_incumbent_route_and_hold() {
    use ohl_ai::scripts::{ScriptHold, ScriptPhase};
    use ohl_ai::{Actor, MonsterAi};
    use ohl_game::scripts::ScriptActivation;

    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &[
            entity_block(
                "monster_barney",
                [0.0; 3],
                0.0,
                &[("targetname", "ohl_overlap_actor"), ("spawnflags", "16")],
            ),
            entity_block(
                "scripted_sequence",
                [160.0, 0.0, 0.0],
                0.0,
                &[
                    ("targetname", "ohl_overlap_walk"),
                    ("m_iszEntity", "ohl_overlap_actor"),
                    ("m_fMoveTo", "1"),
                    ("spawnflags", "32"),
                    ("target", "ohl_overlap_after_walk"),
                ],
            ),
            entity_block(
                "scripted_sequence",
                [0.0; 3],
                0.0,
                &[
                    ("targetname", "ohl_overlap_wait"),
                    ("m_iszEntity", "ohl_overlap_actor"),
                    ("m_fMoveTo", "0"),
                    ("spawnflags", "32"),
                    ("target", "ohl_overlap_after_wait"),
                ],
            ),
            trigger_auto("ohl_overlap_walk"),
            entity_block(
                "trigger_auto",
                [0.0; 3],
                0.0,
                &[("target", "ohl_overlap_wait"), ("delay", "0.1")],
            ),
            exit_trigger("ohl_overlap_after_walk"),
            exit_trigger("ohl_overlap_after_wait"),
        ]
        .concat(),
    );
    let bytes = script_room_bsp(&entities);
    let limits = ohl_formats::bsp30::Limits::default();
    let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &limits).unwrap();
    let collision = ohl_physics::CollisionModel::from_bsp(&bsp, &limits).unwrap();
    let mut game = script_game(&entities);
    let actor = game.registry().find("ohl_overlap_actor")[0];
    let walk = game.registry().find("ohl_overlap_walk")[0];
    let wait = game.registry().find("ohl_overlap_wait")[0];
    let actor_index = game
        .registry()
        .entities
        .iter()
        .position(|entity| *entity == actor)
        .unwrap();
    let actor_index = u32::try_from(actor_index).expect("bounded authored entity list");
    let script_state = |game: &Game, script: ohl_game::hecs::Entity| {
        let index = game
            .registry()
            .entities
            .iter()
            .position(|entity| *entity == script)
            .unwrap();
        game.to_save(0).mover_state.unwrap()[index]
            .as_ref()
            .unwrap()
            .script
            .unwrap()
    };
    let initial = actor_origin(&game, actor);
    let mut delivered = false;
    let mut early_targets = 0;
    for _ in 0..64 {
        for event in game.tick(TICK_SECONDS, &Input::default()) {
            early_targets += usize::from(matches!(event, GameEvent::LevelChange { .. }));
        }
        if game
            .registry()
            .world
            .get::<&ScriptActivation>(wait)
            .unwrap()
            .pending
            > 0
        {
            delivered = true;
            break;
        }
    }
    assert!(delivered, "the delayed ordinary trigger delivered B");
    assert_eq!(early_targets, 0);
    assert_eq!(game.script_start_count(), 1, "A actually started before B");
    let before = script_state(&game, walk);
    assert_eq!(before.actor, Some(actor_index));
    assert_eq!(before.phase_tag, ScriptPhase::Moving.tag());
    assert!(before.was_active && before.moving_elapsed > 0.0);
    {
        let body = game.registry().world.get::<&Actor>(actor).unwrap();
        assert!(body.alive && body.health > 0.0 && body.origin.is_finite());
        assert!(
            body.origin.x > initial.x,
            "ordinary A approach actually moved"
        );
        assert!(
            (160.0 - body.origin.x).abs() > 32.0,
            "A is still far from its mark"
        );
        let query = body.query_origin();
        let clear = collision.trace(body.hull, query, query);
        assert!(!clear.start_solid && !clear.all_solid);
        let ai = game.registry().world.get::<&MonsterAi>(actor).unwrap();
        assert!(!ai.route.is_finished() && ai.move_speed > 0.0);
    }
    assert!(game.registry().world.get::<&ScriptHold>(actor).is_ok());

    // First tick consumes B and enters Play; second tick completes its
    // unspecified action on old source. A remains well short of its mark.
    for _ in 0..2 {
        for event in game.tick(TICK_SECONDS, &Input::default()) {
            early_targets += usize::from(matches!(event, GameEvent::LevelChange { .. }));
        }
    }
    let incumbent = script_state(&game, walk);
    let contender = script_state(&game, wait);
    assert_eq!(incumbent.actor, Some(actor_index));
    assert_eq!(incumbent.phase_tag, ScriptPhase::Moving.tag());
    assert!(incumbent.was_active);
    assert_eq!(contender.actor, Some(actor_index));
    let completed = contender.phase_tag == ScriptPhase::Done.tag();
    if completed {
        assert_eq!(
            contender.completions, 1,
            "B genuinely completed on the same actor"
        );
        assert_eq!(early_targets, 1, "B's ordinary completion output fired");
    } else {
        assert_eq!(contender.phase_tag, ScriptPhase::Dormant.tag());
        assert!(contender.pending_trigger, "the exclusive policy queues B");
        assert_eq!(contender.completions, 0);
    }
    {
        let body = game.registry().world.get::<&Actor>(actor).unwrap();
        assert!(body.alive && body.health > 0.0);
        assert!((160.0 - body.origin.x).abs() > 32.0);
    }
    assert!(
        game.registry().world.get::<&ScriptHold>(actor).is_ok(),
        "A is still Moving: B must not release its actor (B completed: {completed})"
    );
    // Reinserting Hold after an overlapping completion is insufficient:
    // the contender must wait without driving, completing or firing early.
    assert_eq!(contender.phase_tag, ScriptPhase::Dormant.tag());
    assert!(contender.pending_trigger);
    assert_eq!(contender.completions, 0);
    assert_eq!(early_targets, 0);
    let ai = game.registry().world.get::<&MonsterAi>(actor).unwrap();
    assert!(!ai.route.is_finished() && ai.move_speed > 0.0);
}

fn script_owner_assets(extra: &str) -> MemoryAssets {
    let entities = script_room_entities(
        [-192.0, -192.0, 36.0],
        &(entity_block(
            "monster_barney",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "ohl_owner_actor"),
                ("spawnflags", "16"),
                ("model", "models/ohl_owner.mdl"),
            ],
        ) + extra),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    assets.insert(
        "models/ohl_owner.mdl",
        ohl_formats::test_support::build_minimal_mdl10_with_sequences(&[
            "idle", "owner", "waiting",
        ])
        .0,
    );
    assets
}

fn script_owner_def(name: &str, mode: &str, extra: &[(&str, &str)]) -> String {
    let mut fields = vec![
        ("targetname", name),
        ("m_iszEntity", "ohl_owner_actor"),
        ("m_fMoveTo", mode),
    ];
    if !extra.iter().any(|(key, _)| *key == "spawnflags") {
        fields.push(("spawnflags", "32"));
    }
    fields.extend_from_slice(extra);
    entity_block("scripted_sequence", [160.0, 0.0, 0.0], 0.0, &fields)
}

fn script_owner_output(name: &str) -> String {
    entity_block(
        "trigger_changelevel",
        [0.0; 3],
        0.0,
        &[
            ("targetname", name),
            ("map", name),
            ("landmark", "ohl_landmark"),
        ],
    )
}

fn script_owner_snapshot(game: &Game, name: &str) -> ohl_engine::save_state::ScriptRunnerSnapshot {
    let entity = game.registry().find(name)[0];
    let index = game
        .registry()
        .entities
        .iter()
        .position(|item| *item == entity)
        .unwrap();
    game.to_save(0).mover_state.unwrap()[index]
        .as_ref()
        .unwrap()
        .script
        .unwrap()
}

fn script_owner_tick(game: &mut Game) -> Vec<String> {
    game.tick(TICK_SECONDS, &Input::default())
        .into_iter()
        .filter_map(|event| match event {
            GameEvent::LevelChange { map, .. } => Some(map),
            _ => None,
        })
        .collect()
}

#[test]
fn script_owner_later_incumbent_keeps_its_route_and_idle_then_releases_in_order() {
    use ohl_ai::{Actor, MonsterAi, ScriptHold, ScriptPhase};
    let assets = script_owner_assets(
        &[
            // The earlier definition is a contender, not the active incumbent.
            script_owner_def(
                "ohl_owner_b",
                "0",
                &[("m_iszIdle", "waiting"), ("target", "ohl_owner_b_out")],
            ),
            script_owner_def(
                "ohl_owner_a",
                "1",
                &[("m_iszIdle", "owner"), ("target", "ohl_owner_a_out")],
            ),
            trigger_auto("ohl_owner_a"),
            entity_block(
                "trigger_auto",
                [0.0; 3],
                0.0,
                &[("target", "ohl_owner_b"), ("delay", "0.1")],
            ),
            script_owner_output("ohl_owner_a_out"),
            script_owner_output("ohl_owner_b_out"),
        ]
        .concat(),
    );
    let mut game = Game::load(&assets, SCRIPT_MAP).unwrap();
    let actor = game.registry().find("ohl_owner_actor")[0];
    let mut outputs = Vec::new();
    for _ in 0..30 {
        outputs.extend(script_owner_tick(&mut game));
    }
    assert_eq!(
        script_owner_snapshot(&game, "ohl_owner_a").phase_tag,
        ScriptPhase::Moving.tag()
    );
    let waiting = script_owner_snapshot(&game, "ohl_owner_b");
    assert_eq!(waiting.phase_tag, ScriptPhase::Dormant.tag());
    assert!(waiting.pending_trigger);
    assert!(outputs.is_empty());
    assert!(game.registry().world.get::<&ScriptHold>(actor).is_ok());
    assert!(game.registry().world.get::<&Actor>(actor).unwrap().origin.x > 0.0);
    let ai = game.registry().world.get::<&MonsterAi>(actor).unwrap();
    assert!(!ai.route.is_finished() && ai.move_speed > 0.0);
    drop(ai);
    assert_eq!(
        game.registry()
            .world
            .get::<&StudioAnim>(actor)
            .unwrap()
            .sequence,
        1,
        "waiting idle must not overwrite the owner's independently named sequence"
    );
    for _ in 0..1_200 {
        outputs.extend(script_owner_tick(&mut game));
        if outputs.len() == 2 {
            break;
        }
    }
    assert_eq!(outputs, ["ohl_owner_a_out", "ohl_owner_b_out"]);
    assert_eq!(script_owner_snapshot(&game, "ohl_owner_a").completions, 1);
    assert_eq!(script_owner_snapshot(&game, "ohl_owner_b").completions, 1);
    assert!(game.registry().world.get::<&ScriptHold>(actor).is_err());
    assert_eq!(game.active_script_count(), 0);
    assert_eq!(tick_counting_level_changes(&mut game, 120), 0);
}

#[test]
fn script_owner_busy_repeat_pauses_and_save_continuation_keeps_output_order() {
    use ohl_ai::ScriptPhase;
    let assets = script_owner_assets(
        &[
            script_owner_def(
                "ohl_owner_b",
                "0",
                &[
                    ("spawnflags", "36"),
                    ("m_flRepeat", "1"),
                    ("target", "ohl_owner_b_out"),
                ],
            ),
            script_owner_def("ohl_owner_a", "1", &[("target", "ohl_owner_a_out")]),
            trigger_auto("ohl_owner_b"),
            entity_block(
                "trigger_auto",
                [0.0; 3],
                0.0,
                &[("target", "ohl_owner_a"), ("delay", "0.1")],
            ),
            script_owner_output("ohl_owner_a_out"),
            script_owner_output("ohl_owner_b_out"),
        ]
        .concat(),
    );
    let mut game = Game::load(&assets, SCRIPT_MAP).unwrap();
    assert_eq!(
        tick_counting_level_changes(&mut game, 30),
        1,
        "B actually completed first"
    );
    let paused = script_owner_snapshot(&game, "ohl_owner_b");
    assert_eq!(paused.phase_tag, ScriptPhase::Repeating.tag());
    assert_eq!(paused.completions, 1);
    assert!(paused.timer > 0.0);
    assert_eq!(
        script_owner_snapshot(&game, "ohl_owner_a").phase_tag,
        ScriptPhase::Moving.tag()
    );
    assert_eq!(tick_counting_level_changes(&mut game, 60), 0);
    assert_eq!(
        script_owner_snapshot(&game, "ohl_owner_b"),
        paused,
        "busy repeat does not advance"
    );
    let saved = game.save_bytes(0).unwrap();
    let mut loaded = Game::load_bytes(&assets, &saved).unwrap();
    for name in ["ohl_owner_a", "ohl_owner_b"] {
        assert_eq!(
            script_owner_snapshot(&loaded, name),
            script_owner_snapshot(&game, name)
        );
    }
    let mut outputs = Vec::new();
    for _ in 0..1_200 {
        let actual = script_owner_tick(&mut game);
        assert_eq!(script_owner_tick(&mut loaded), actual);
        outputs.extend(actual);
        for name in ["ohl_owner_a", "ohl_owner_b"] {
            assert_eq!(
                script_owner_snapshot(&loaded, name),
                script_owner_snapshot(&game, name)
            );
        }
        let a = game.registry().find("ohl_owner_actor")[0];
        let b = loaded.registry().find("ohl_owner_actor")[0];
        assert!((actor_origin(&game, a) - actor_origin(&loaded, b)).length() < 0.001);
        if outputs.len() == 2 {
            break;
        }
    }
    assert_eq!(outputs, ["ohl_owner_a_out", "ohl_owner_b_out"]);
    assert_eq!(script_owner_snapshot(&game, "ohl_owner_b").completions, 2);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "compare the two legacy ownership phases through one real save/load boundary"
)]
fn script_owner_legacy_duplicates_queue_without_fabricating_completion() {
    use ohl_ai::{Actor, MonsterAi, ScriptHold, ScriptPhase, Vec3};
    let assets = script_owner_assets(
        &[
            script_owner_def(
                "ohl_owner_a",
                "1",
                &[("m_iszPlay", "owner"), ("target", "ohl_owner_a_out")],
            ),
            entity_block(
                "scripted_sequence",
                [160.0, 40.0, 0.0],
                0.0,
                &[
                    ("targetname", "ohl_owner_b"),
                    ("m_iszEntity", "ohl_owner_actor"),
                    ("m_fMoveTo", "1"),
                    ("m_iszPlay", "waiting"),
                    ("spawnflags", "32"),
                    ("target", "ohl_owner_b_out"),
                ],
            ),
            trigger_auto("ohl_owner_a"),
            script_owner_output("ohl_owner_a_out"),
            script_owner_output("ohl_owner_b_out"),
        ]
        .concat(),
    );
    let mut game = Game::load(&assets, SCRIPT_MAP).unwrap();
    assert_eq!(tick_counting_level_changes(&mut game, 30), 0);
    let captured = script_owner_snapshot(&game, "ohl_owner_a");
    assert_eq!(captured.phase_tag, ScriptPhase::Moving.tag());
    assert!(captured.moving_elapsed > 0.0 && captured.was_active);
    let index_of = |name: &str| {
        let entity = game.registry().find(name)[0];
        game.registry()
            .entities
            .iter()
            .position(|e| *e == entity)
            .unwrap()
    };
    let actor = game.registry().find("ohl_owner_actor")[0];
    let body = *game.registry().world.get::<&Actor>(actor).unwrap();
    let owner_goal = body
        .body_frame
        .anchor_to_query(body.hull, Vec3::new(160.0, 0.0, 0.0));
    let foreign_goal = body
        .body_frame
        .anchor_to_query(body.hull, Vec3::new(160.0, 40.0, 0.0));
    assert!((foreign_goal - owner_goal).length() > 0.0);
    assert!(
        (foreign_goal - owner_goal).length() < 80.0,
        "old refresh threshold would retain foreign route"
    );
    for phase in [ScriptPhase::Moving, ScriptPhase::Playing] {
        let mut save = game.to_save(0);
        // Explicit legacy tag28 duplicates and tag25 foreign route: no live-state injection.
        let scripts = save.mover_state.as_mut().unwrap();
        let winner = scripts[index_of("ohl_owner_a")]
            .as_mut()
            .unwrap()
            .script
            .as_mut()
            .unwrap();
        winner.phase_tag = phase.tag();
        winner.played = if phase == ScriptPhase::Playing {
            0.02
        } else {
            0.0
        };
        let expected_winner = *winner;
        let duplicate = scripts[index_of("ohl_owner_b")]
            .as_mut()
            .unwrap()
            .script
            .as_mut()
            .unwrap();
        duplicate.actor = captured.actor;
        duplicate.phase_tag = ScriptPhase::Playing.tag();
        duplicate.timer = 0.25;
        duplicate.completions = 7;
        duplicate.warped = true;
        duplicate.moving_elapsed = 0.5;
        duplicate.was_active = true;
        duplicate.played = 0.25;
        let foreign = save.ai.as_mut().unwrap()[index_of("ohl_owner_actor")]
            .as_mut()
            .unwrap();
        foreign.route_waypoints = vec![foreign_goal.to_array()];
        foreign.route_current = 0;
        foreign.route_goal = foreign_goal.to_array();
        foreign.move_speed = 40.0;
        let mut loaded = Game::load_bytes(&assets, &save.to_bytes().unwrap()).unwrap();
        let restored_actor = loaded.registry().find("ohl_owner_actor")[0];
        assert_eq!(
            script_owner_snapshot(&loaded, "ohl_owner_a"),
            expected_winner
        );
        let deferred = script_owner_snapshot(&loaded, "ohl_owner_b");
        assert_eq!(deferred.phase_tag, ScriptPhase::Dormant.tag());
        assert!(deferred.pending_trigger && !deferred.was_active && !deferred.warped);
        assert_eq!(deferred.completions, 7);
        assert_eq!(
            [deferred.timer, deferred.moving_elapsed, deferred.played].map(f32::to_bits),
            [0; 3]
        );
        {
            let ai = loaded
                .registry()
                .world
                .get::<&MonsterAi>(restored_actor)
                .unwrap();
            assert!(
                ai.route.is_finished() && ai.move_speed <= 0.0,
                "normalization must discard the losing owner's shared route"
            );
        }
        assert!(
            loaded
                .registry()
                .world
                .get::<&ScriptHold>(restored_actor)
                .is_ok()
        );
        if phase == ScriptPhase::Playing {
            assert_eq!(
                loaded
                    .registry()
                    .world
                    .get::<&StudioAnim>(restored_actor)
                    .unwrap()
                    .sequence,
                1,
                "restore canonical action even with a nonzero played clock"
            );
        }
        let before = actor_origin(&loaded, restored_actor);
        let mut outputs = script_owner_tick(&mut loaded);
        let displacement = actor_origin(&loaded, restored_actor) - before;
        if phase == ScriptPhase::Moving {
            assert!(
                displacement.x > 0.0 && displacement.y.abs() < 0.001,
                "ordinary next step follows A's goal, not nearby B's stale goal"
            );
        } else {
            assert!(displacement.length() < 0.001);
        }
        for _ in 0..1_200 {
            outputs.extend(script_owner_tick(&mut loaded));
            if outputs.len() == 2 {
                break;
            }
        }
        assert_eq!(outputs, ["ohl_owner_a_out", "ohl_owner_b_out"]);
        assert_eq!(script_owner_snapshot(&loaded, "ohl_owner_a").completions, 1);
        assert_eq!(script_owner_snapshot(&loaded, "ohl_owner_b").completions, 8);
        assert_eq!(tick_counting_level_changes(&mut loaded, 120), 0);
    }
}
