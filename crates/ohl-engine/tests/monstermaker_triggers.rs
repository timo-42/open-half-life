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
//! *retrigger* semantics under test (toggle, for a `Cyclic` maker and a
//! non-`Cyclic` one alike) are this milestone's own product decision,
//! documented beside `ohl_ai::Spawner::trigger` and appended to
//! `docs/FORMAT_SOURCES.md`'s "Monster definitions" entry.

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

/// `docs/FORMAT_SOURCES.md`'s "Monster definitions" citation for the
/// `Cyclic` spawnflag: "keep spawning rather than stopping after one
/// quota". A single trigger must start a continuous, `delay`-paced loop
/// that produces every child `monstercount` allows on its own, without
/// needing a separate trigger per child.
#[test]
fn a_cyclic_maker_keeps_spawning_on_delay_after_one_trigger() {
    const SPAWNFLAG_CYCLIC: u32 = 4;
    const DELAY: f32 = 0.3;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &format!(
            "{}{}",
            trigger_auto("maker1", 0.0),
            maker_block(3, DELAY, 0, SPAWNFLAG_CYCLIC),
        ),
    );
    let mut game = script_game(&entities);

    // The very first child is immediate (the maker's own timer starts at
    // zero), from the one trigger_auto alone.
    tick(&mut game, ticks_for(0.05));
    assert_eq!(game.monster_count(), 1, "one trigger starts the loop");

    // No second trigger anywhere in this fixture: each further child must
    // still appear, `delay` apart, entirely on its own.
    for expected in 2..=3usize {
        tick(&mut game, ticks_for(DELAY));
        assert_eq!(
            game.monster_count(),
            expected,
            "child #{expected} must appear on its own, `delay` after the last"
        );
    }

    // monstercount (3) is still a hard cap: running well past it produces
    // no more.
    tick(&mut game, ticks_for(DELAY) * 3);
    assert_eq!(game.monster_count(), 3);
}

/// `Start On` and `Cyclic` together start the same continuous loop
/// immediately, with no trigger needed at all.
#[test]
fn start_on_and_cyclic_together_spawn_continuously_without_a_trigger() {
    const SPAWNFLAG_START_ON: u32 = 1;
    const SPAWNFLAG_CYCLIC: u32 = 4;
    const DELAY: f32 = 0.2;
    let entities = script_room_entities(
        PLAYER_ORIGIN,
        &maker_block(2, DELAY, 0, SPAWNFLAG_START_ON | SPAWNFLAG_CYCLIC),
    );
    let mut game = script_game(&entities);

    tick(&mut game, ticks_for(0.05));
    assert_eq!(game.monster_count(), 1, "Start On begins spawning at once");
    tick(&mut game, ticks_for(DELAY));
    assert_eq!(
        game.monster_count(),
        2,
        "the second child needs no trigger either"
    );
}

/// `monstercount` bounds a `Cyclic` maker exactly as it bounds a
/// non-cyclic one, even across several explicit retriggers (each of which
/// toggles the maker on/off, per this milestone's own retrigger decision).
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

/// An ordinary delayed maker, or its directly declared talk counterpart.
fn maker_talk_fixture(classname: &str, made: bool, flags: u32) -> (Game, ohl_engine::MemoryAssets) {
    use ohl_engine::test_support::{SCRIPT_MAP, script_room_bsp};
    const DELAY: f32 = 0.3;
    let origin = [32.0, 0.0, 0.0];
    let extra = if made {
        format!(
            "{}{}",
            trigger_auto("talk_maker", 0.0),
            entity_block(
                "monstermaker",
                origin,
                180.0,
                &[
                    ("targetname", "talk_maker"),
                    ("monstertype", classname),
                    ("monstercount", "1"),
                    ("delay", &DELAY.to_string()),
                    ("spawnflags", &flags.to_string()),
                ],
            ),
        )
    } else {
        entity_block(
            classname,
            origin,
            180.0,
            &[("spawnflags", &flags.to_string())],
        )
    };
    let entities = script_room_entities([0.0, 0.0, 36.0], &extra);
    let mut assets = ohl_engine::MemoryAssets::new();
    assets.insert(
        &format!("maps/{SCRIPT_MAP}.bsp"),
        script_room_bsp(&entities),
    );
    let mut game = Game::load(&assets, SCRIPT_MAP).expect("generated talk room");
    if made {
        assert!(
            monster_entities(&game).is_empty(),
            "no child before activation"
        );
        tick(&mut game, 1);
        assert!(
            monster_entities(&game).is_empty(),
            "activation still respects delay"
        );
    }
    tick(&mut game, ticks_for(DELAY));
    (game, assets)
}

/// Do not require Follower presence here: its absence must be detected by
/// real recruitment, after independent spawn/life/geometry prerequisites.
fn sole_nearby_talk_actor(game: &Game, classname: &str) -> ohl_game::hecs::Entity {
    use ohl_ai::{Actor, Follower};
    use ohl_game::hecs::Entity;
    let actor_id = ohl_engine::test_support::entity_of_classname(game, classname).unwrap();
    assert_eq!(
        monster_entities(game),
        vec![actor_id],
        "no competing monster"
    );
    let actor = game.registry().world.get::<&Actor>(actor_id).unwrap();
    assert!(actor.alive && actor.health > 0.0 && game.player_health() > 0.0);
    let point = actor.query_origin();
    let eye = ohl_ai::Vec3::from_array(game.eye_position());
    assert!(point.is_finite() && eye.is_finite());
    assert!(point.distance(eye) < ohl_engine::ai::TALK_USE_RADIUS);
    assert!(
        game.registry()
            .world
            .query::<(Entity, &Actor, &Follower)>()
            .iter()
            .all(|(id, _, _)| id == actor_id),
        "no other follow candidate"
    );
    actor_id
}

fn use_talk_and_consume(game: &mut Game) {
    game.tick(TICK_SECONDS, &ohl_engine::test_support::use_input());
    tick(game, 2);
}

#[test]
fn maker_talk_child_recruits_and_stops_after_real_use() {
    for classname in ohl_engine::ai::TALK_MONSTER_CLASSNAMES {
        let (mut game, _) = maker_talk_fixture(classname, true, 0);
        let child = sole_nearby_talk_actor(&game, classname);
        assert!(game.followers().is_empty());
        use_talk_and_consume(&mut game);
        assert_eq!(
            game.followers(),
            &[child],
            "ordinary use recruits the maker child"
        );
        use_talk_and_consume(&mut game);
        assert!(
            game.followers().is_empty(),
            "the second use stops the child"
        );
        assert!(
            !game
                .registry()
                .world
                .get::<&ohl_ai::follow::Follower>(child)
                .unwrap()
                .following
        );
    }
}

#[test]
fn maker_talk_child_declared_controls_keep_pre_disaster_behavior() {
    use ohl_ai::follow::SPAWNFLAG_TALK_PRE_DISASTER;
    for classname in ohl_engine::ai::TALK_MONSTER_CLASSNAMES {
        for flags in [0, SPAWNFLAG_TALK_PRE_DISASTER] {
            let (mut game, _) = maker_talk_fixture(classname, false, flags);
            let declared = sole_nearby_talk_actor(&game, classname);
            use_talk_and_consume(&mut game);
            if flags == 0 {
                assert_eq!(
                    game.followers(),
                    &[declared],
                    "declared default still follows"
                );
                use_talk_and_consume(&mut game);
                assert!(game.followers().is_empty());
            } else {
                assert!(
                    game.followers().is_empty(),
                    "declared Pre-Disaster still refuses"
                );
            }
        }
    }
}

#[test]
fn maker_talk_child_save_rebuilds_eligibility_without_inventing_roster_state() {
    // The existing save format records no Follower/roster data. A save
    // before recruitment also represents an older maker child without that
    // component; after recruitment, the direct and maker cases both reset
    // the roster on load. This is a retained limitation, not new fidelity.
    for made in [false, true] {
        for recruited in [false, true] {
            let classname = "monster_scientist";
            let (mut game, assets) = maker_talk_fixture(classname, made, 0);
            let actor = sole_nearby_talk_actor(&game, classname);
            if recruited {
                use_talk_and_consume(&mut game);
                assert_eq!(game.followers(), &[actor]);
            }
            let bytes = game
                .save_bytes(1_700_000_000)
                .expect("write generated save");
            let mut loaded = Game::load_bytes(&assets, &bytes).expect("load generated save");
            let restored = sole_nearby_talk_actor(&loaded, classname);
            assert!(
                loaded.followers().is_empty(),
                "existing roster reset remains explicit"
            );
            use_talk_and_consume(&mut loaded);
            assert_eq!(
                loaded.followers(),
                &[restored],
                "restored actor is recruitable"
            );
            use_talk_and_consume(&mut loaded);
            assert!(
                loaded.followers().is_empty(),
                "restored actor can stop following"
            );
        }
    }
}
