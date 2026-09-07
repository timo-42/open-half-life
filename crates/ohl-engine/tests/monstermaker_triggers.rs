//! `monstermaker` activation by `targetname`, over the synthetic script
//! room.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. Every keyvalue and spawnflag the
//! entity blocks below use is a published one recorded in
//! `docs/FORMAT_SOURCES.md`, "Monster definitions". Retriggering is driven
//! by extra, staggered `trigger_auto` entities (`delay` is a published
//! `trigger_auto` keyvalue), exactly like `camera_sequences.rs`'s "a second
//! trigger_auto" fixture, since there is no other public seam that fires a
//! `targetname` a fixed number of seconds into a running level. The exact
//! *retrigger* semantics under test (toggle for a non-cyclic maker, one
//! discrete spawn per trigger for a cyclic one) are this milestone's own
//! product decision, documented beside `ohl_ai::Spawner::trigger`.

use ohl_engine::test_support::{entity_block, monster_entities, script_game, script_room_entities};
use ohl_engine::{Game, Input, TICK_SECONDS};

const PLAYER_ORIGIN: [f32; 3] = [0.0, 0.0, 32.0];
const MAKER_ORIGIN: [f32; 3] = [96.0, -96.0, 32.0];

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

/// The number of `TICK_SECONDS` ticks `seconds` covers, rounded up, plus a
/// small slack margin, matching `camera_sequences.rs`'s `ticks_for`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ticks_for(seconds: f32) -> usize {
    (seconds / TICK_SECONDS).ceil() as usize + 8
}

/// A `trigger_auto` that fires `target` `delay` seconds after the map has
/// loaded (`delay` defaults to `0` when omitted).
fn trigger_auto(target: &str, delay: f32) -> String {
    entity_block(
        "trigger_auto",
        [0.0, 0.0, 0.0],
        0.0,
        &[("target", target), ("delay", &delay.to_string())],
    )
}

/// `count` `trigger_auto` entities all targeting `target`, firing
/// `interval`, `2 * interval`, `3 * interval`, ... seconds apart (the
/// first at `interval` itself, since a `0`-delay trigger would collide
/// with a maker's own `Start On`less first activation in the same test).
fn staggered_triggers(target: &str, count: usize, interval: f32) -> String {
    (1..=count)
        .map(|n| {
            #[allow(clippy::cast_precision_loss)]
            trigger_auto(target, interval * n as f32)
        })
        .collect()
}

/// A `monstermaker` with a `targetname`, not `Start On` unless `spawnflags`
/// says so.
fn maker_block(monstercount: i32, delay: f32, max_live: u32, spawnflags: u32) -> String {
    entity_block(
        "monstermaker",
        MAKER_ORIGIN,
        0.0,
        &[
            ("targetname", "maker1"),
            ("monstertype", "monster_headcrab"),
            ("monstercount", &monstercount.to_string()),
            ("delay", &delay.to_string()),
            ("m_imaxlivechildren", &max_live.to_string()),
            ("spawnflags", &spawnflags.to_string()),
        ],
    )
}

/// A `trigger_auto` targeting a `monstermaker` without `Start On` spawns
/// its first child only after the configured `delay`, not before.
#[test]
fn a_trigger_auto_starts_a_non_start_on_maker_after_its_delay() {
    const DELAY: f32 = 0.3;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(1, DELAY, 0, 0)
        ),
    );
    let mut game = script_game(&entities);
    assert_eq!(game.monster_count(), 0, "nothing spawns before triggering");

    // One tick: the `trigger_auto` fires and the map-logic simulation
    // activates the maker, but the spawn itself is still `delay` seconds
    // away.
    tick(&mut game, 1);
    assert_eq!(
        game.monster_count(),
        0,
        "the trigger only starts the maker; the configured delay must still elapse"
    );

    tick(&mut game, ticks_for(DELAY));
    assert_eq!(
        game.monster_count(),
        1,
        "the maker must have spawned its first child once the delay elapsed"
    );
}

/// Triggering an already-active, non-cyclic maker a second time toggles it
/// off: no further children ever appear, even though `monstercount` was
/// never exhausted.
#[test]
fn a_second_trigger_stops_a_non_cyclic_maker_from_spawning_further() {
    const INTERVAL: f32 = 0.4;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(100, 0.02, 0, 0),
            // A second trigger_auto, fired well after the maker has
            // started spawning, toggles it back off.
            staggered_triggers("maker1", 1, INTERVAL),
        ),
    );
    let mut game = script_game(&entities);

    tick(&mut game, ticks_for(0.1));
    assert!(
        game.monster_count() >= 1,
        "the first trigger must have started spawning"
    );

    // Let the second trigger_auto fire and toggle the maker off, then run
    // well past what `monstercount`'s worth of ticks would otherwise
    // produce: nothing more may ever appear.
    tick(&mut game, ticks_for(INTERVAL));
    let stopped_count = game.monster_count();
    tick(&mut game, 400);
    assert_eq!(
        game.monster_count(),
        stopped_count,
        "a toggled-off maker must never spawn again, even with quota left"
    );
    assert!(
        stopped_count < 100,
        "the toggle must have actually cut spawning short of monstercount"
    );
}

/// A `Cyclic` maker (spawnflag bit `4`) spawns exactly one child per
/// trigger event, and this is repeatable.
#[test]
fn a_cyclic_maker_spawns_one_child_per_trigger_repeatably() {
    const SPAWNFLAG_CYCLIC: u32 = 4;
    const INTERVAL: f32 = 0.3;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(100, 0.0, 0, SPAWNFLAG_CYCLIC),
            staggered_triggers("maker1", 3, INTERVAL),
        ),
    );
    let mut game = script_game(&entities);

    tick(&mut game, ticks_for(0.05));
    assert_eq!(
        game.monster_count(),
        1,
        "one trigger, from the initial trigger_auto, is worth exactly one child"
    );

    for expected in 2..=4usize {
        tick(&mut game, ticks_for(INTERVAL));
        assert_eq!(
            game.monster_count(),
            expected,
            "trigger #{expected} must add exactly one more child"
        );
    }
}

/// `monstercount` bounds a `Cyclic` maker exactly as it bounds a
/// non-cyclic one, across many triggers.
#[test]
fn monstercount_is_respected_across_many_triggers_of_a_cyclic_maker() {
    const SPAWNFLAG_CYCLIC: u32 = 4;
    const INTERVAL: f32 = 0.1;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(2, 0.0, 0, SPAWNFLAG_CYCLIC),
            staggered_triggers("maker1", 10, INTERVAL),
        ),
    );
    let mut game = script_game(&entities);
    tick(&mut game, ticks_for(11.0 * INTERVAL));

    assert_eq!(
        game.monster_count(),
        2,
        "monstercount caps a cyclic maker across any number of triggers"
    );
}

/// `monstercount` and `m_imaxlivechildren` both still apply when the
/// maker's spawns come from repeated triggers rather than `Start On`.
#[test]
fn monstercount_and_live_cap_hold_across_repeated_triggers() {
    const INTERVAL: f32 = 0.05;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(3, 0.0, 1, 0),
            // Retrigger (toggle on/off) many times; toggling never lets
            // more than one live child exist, and never more than three
            // total.
            staggered_triggers("maker1", 20, INTERVAL),
        ),
    );
    let mut game = script_game(&entities);
    for _ in 0..20 {
        tick(&mut game, ticks_for(INTERVAL));
        assert!(
            game.monster_count() <= 1,
            "m_imaxlivechildren must cap live children even while retriggered"
        );
    }
    for entity in monster_entities(&game) {
        ohl_engine::test_support::queue_monster_damage(&mut game, entity, None, 1_000.0);
    }
    tick(&mut game, 60);
    assert!(
        game.monster_count() <= 3,
        "monstercount must never be exceeded regardless of trigger toggling"
    );
}
