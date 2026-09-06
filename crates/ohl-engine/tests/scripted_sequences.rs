//! M7.11: scripted sequences, talk-monster following and
//! `scripted_sentence`, over a synthetic room.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. Every keyvalue and spawnflag the
//! entity blocks below use is a published one recorded in
//! `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters".

use ohl_engine::test_support::{
    SCRIPT_MAP, actor_origin, entity_block, entity_of_classname, script_game, script_room_bsp,
    script_room_entities, use_input,
};
use ohl_engine::{Game, GameEvent, Input, TICK_SECONDS};

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

/// A `scripted_sentence` resolves its speaker and emits one cue whose path
/// is `None`, matching this project's sound-path policy, and fires its
/// `target` afterwards.
#[test]
fn a_scripted_sentence_speaks_through_a_cue_without_an_asset_path() {
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
                    assert_eq!(cue.path, None, "no sound asset path is ever shipped");
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
