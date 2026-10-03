//! Wave 1 batch B through the whole engine: the damage-type rules, the
//! Gonarch's trail, the Nihilanth's activation and shield, and the
//! aircraft's `Start Inactive` route.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`'s AI
//! room, entity blocks written below); every `targetname` is a synthetic
//! `ohl_*` name, and no bytes come from any game installation. See
//! `docs/CLEAN_ROOM.md`.

use ohl_ai::monsters::bigmomma::TrailPhase;
use ohl_ai::monsters::nihilanth::HEAD_OPEN_SECONDS;
use ohl_ai::monsters::table::BIGMOMMA_HEALTH_FACTOR;
use ohl_ai::{DamageKinds, GonarchTrail};
use ohl_engine::test_support::{
    AI_MAP, ai_room_bsp, monster_entities, queue_monster_damage, queue_typed_monster_damage,
};
use ohl_engine::{Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::hecs::Entity;
use std::fmt::Write as _;

/// The room's entity block: a worldspawn, a player start facing `+X`, and
/// whatever the test adds.
fn entities(extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {extra}"
    )
}

/// One entity block: `classname` at `origin` with `keys`.
fn block(classname: &str, origin: [f32; 3], keys: &[(&str, &str)]) -> String {
    let mut text = format!(
        "{{\n\"classname\" \"{classname}\"\n\"origin\" \"{} {} {}\"\n",
        origin[0], origin[1], origin[2]
    );
    for (key, value) in keys {
        let _ = writeln!(text, "\"{key}\" \"{value}\"");
    }
    text.push_str("}\n");
    text
}

fn trigger_auto(target: &str) -> String {
    block("trigger_auto", [0.0, 0.0, 0.0], &[("target", target)])
}

/// A `trigger_changelevel` a fired name can reach; firing it is visible to
/// the host as a `GameEvent::LevelChange`.
fn exit_trigger() -> String {
    block(
        "trigger_changelevel",
        [0.0, 0.0, 0.0],
        &[
            ("targetname", "ohl_exit"),
            ("map", "ohlelsewhere"),
            ("landmark", "ohl_landmark"),
        ],
    )
}

fn assets(entities: &str) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), ai_room_bsp(entities, false));
    assets
}

fn game_from(entities: &str) -> Game {
    let bytes = ai_room_bsp(entities, false);
    Game::from_map_bytes(&assets(entities), AI_MAP, &bytes).expect("the AI room loads")
}

/// Steps `game` and reports whether any step announced a level change.
fn tick_until_level_change(game: &mut Game, ticks: usize) -> bool {
    let input = Input::default();
    let mut fired = false;
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired = true;
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

fn the_monster(game: &Game) -> Entity {
    let monsters = monster_entities(game);
    assert_eq!(monsters.len(), 1, "the room holds one monster");
    monsters[0]
}

fn health_of(game: &Game, entity: Entity) -> f32 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .expect("actor")
        .health
}

fn origin_of(game: &Game, entity: Entity) -> ohl_ai::Vec3 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .expect("actor")
        .origin
}

// --- Damage types -----------------------------------------------------------

/// The gargantua's published immunity, end to end through the engine's
/// damage queue. A hit typed as a bullet (or left untyped, as an unknown
/// source's hit is) costs no health; a blast does; enough blast kills, once.
#[test]
fn a_gargantua_shrugs_off_bullets_and_dies_to_blast() {
    let mut game = game_from(&entities(&block(
        "monster_gargantua",
        [128.0, 0.0, 36.0],
        &[("angle", "180")],
    )));
    let garg = the_monster(&game);
    let full = health_of(&game, garg);
    assert!(full >= 800.0, "the gargantua's own cited health");

    queue_typed_monster_damage(&mut game, garg, None, 500.0, DamageKinds::BULLET);
    queue_monster_damage(&mut game, garg, None, 500.0);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, garg) - full).abs() < 1e-3,
        "bullets and untyped hits cost nothing"
    );
    assert_eq!(game.monster_death_count(), 0);

    queue_typed_monster_damage(&mut game, garg, None, 100.0, DamageKinds::BLAST);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, garg) - (full - 100.0)).abs() < 1e-3,
        "a blast costs health"
    );

    queue_typed_monster_damage(&mut game, garg, None, 5_000.0, DamageKinds::BLAST);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 1, "enough blast kills, once");
    assert_eq!(game.monster_count(), 0);
}

/// The Apache's published "blast damage doubles damage": a blast costs
/// twice its amount, a bullet its own.
#[test]
fn an_apache_takes_double_from_a_blast() {
    let mut game = game_from(&entities(&block(
        "monster_apache",
        [128.0, 0.0, 160.0],
        &[],
    )));
    let apache = the_monster(&game);
    let full = health_of(&game, apache);
    queue_typed_monster_damage(&mut game, apache, None, 10.0, DamageKinds::BULLET);
    tick(&mut game, 1);
    assert!((health_of(&game, apache) - (full - 10.0)).abs() < 1e-3);
    queue_typed_monster_damage(&mut game, apache, None, 40.0, DamageKinds::BLAST);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, apache) - (full - 90.0)).abs() < 1e-3,
        "the blast cost 80"
    );
}

// --- The Gonarch --------------------------------------------------------------

/// A Gonarch whose first node is `ohl_node`: it runs there, and the node
/// names `ohl_exit` to fire, `ohl_crate` to remove, `ohl_seq` to play and
/// a health to set. `extra` is added to the room.
fn gonarch_room(extra: &str) -> String {
    entities(&format!(
        "{}{}{}{}{extra}",
        block(
            "monster_bigmomma",
            [160.0, 0.0, 36.0],
            &[
                ("angle", "180"),
                ("targetname", "ohl_mother"),
                ("netname", "ohl_node"),
            ],
        ),
        block(
            "info_bigmomma",
            [32.0, 0.0, 36.0],
            &[
                ("targetname", "ohl_node"),
                ("reachtarget", "ohl_exit"),
                ("killtarget", "ohl_crate"),
                ("reachsequence", "ohl_seq"),
                ("health", "100"),
                ("spawnflags", "1"),
            ],
        ),
        block(
            "info_target",
            [0.0, 160.0, 36.0],
            &[("targetname", "ohl_crate")]
        ),
        exit_trigger(),
    ))
}

/// Reaching a node fires its `reachtarget` by name, removes its
/// `killtarget`, starts its `reachsequence`, and sets the node's health
/// (times the difficulty's published factor); on the way it is shielded.
#[test]
fn a_gonarch_reaching_a_node_fires_removes_and_plays_what_the_node_names() {
    // The script the node names, bound to the Gonarch by name.
    // `Override AI` (64), so a Gonarch already fighting is still taken.
    let script = block(
        "scripted_sequence",
        [32.0, 0.0, 36.0],
        &[
            ("targetname", "ohl_seq"),
            ("m_iszEntity", "ohl_mother"),
            ("m_iszPlay", "ohl_action"),
            ("spawnflags", "64"),
        ],
    );
    let mut game = game_from(&gonarch_room(&script));
    let gonarch = the_monster(&game);
    assert_eq!(game.registry().find("ohl_crate").len(), 1);
    assert_eq!(game.script_start_count(), 0);

    // On the trail: shielded.
    queue_monster_damage(&mut game, gonarch, None, 10_000.0);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 0, "shielded on the way");

    assert!(
        tick_until_level_change(&mut game, 300),
        "the node's reachtarget fired"
    );
    let crate_gone = game
        .registry()
        .find("ohl_crate")
        .iter()
        .all(|entity| !game.registry().world.contains(*entity));
    assert!(crate_gone, "the node's killtarget was removed");
    tick(&mut game, 5);
    assert_eq!(
        game.script_start_count(),
        1,
        "the node's reachsequence started"
    );
    let health = health_of(&game, gonarch);
    assert!(
        BIGMOMMA_HEALTH_FACTOR
            .iter()
            .any(|factor| (health - 100.0 * factor).abs() < 1e-3),
        "the node's health, scaled: {health}"
    );
    assert_eq!(
        game.registry()
            .world
            .get::<&GonarchTrail>(gonarch)
            .expect("trail")
            .phase(),
        TrailPhase::Holding { at: 0 }
    );
}

// --- The Nihilanth ------------------------------------------------------------

fn nihilanth_room(activated: bool, extra_keys: &[(&str, &str)]) -> String {
    let mut keys = vec![("angle", "180"), ("targetname", "ohl_boss")];
    keys.extend_from_slice(extra_keys);
    entities(&format!(
        "{}{}{}",
        block("monster_nihilanth", [160.0, 0.0, 128.0], &keys),
        if activated {
            trigger_auto("ohl_boss")
        } else {
            String::new()
        },
        exit_trigger(),
    ))
}

/// "When spawned, the Nihilanth does not attack immediately": in view of
/// the player it attacks nothing until a `use` of its name activates it,
/// and then it does.
#[test]
fn a_nihilanth_attacks_nothing_until_a_trigger_activates_it() {
    let mut dormant = game_from(&nihilanth_room(false, &[]));
    tick(&mut dormant, 500);
    assert!(
        (dormant.player_health() - 100.0).abs() < f32::EPSILON,
        "a dormant boss attacked"
    );

    let mut active = game_from(&nihilanth_room(true, &[]));
    tick(&mut active, 500);
    assert!(
        active.player_health() < 100.0,
        "the activated boss zapped the player"
    );
}

/// With no crystal in the room, hits drain the reserve, the head opens
/// after its delay, and only then can the boss die — once. Its page says
/// `TriggerCondition` does not work on it, so a declared death trigger
/// fires nothing.
#[test]
fn a_nihilanth_dies_once_its_head_opens_and_ignores_its_trigger_condition() {
    let keys = [("TriggerCondition", "4"), ("TriggerTarget", "ohl_exit")];
    let mut game = game_from(&nihilanth_room(false, &keys));
    let boss = the_monster(&game);

    queue_monster_damage(&mut game, boss, None, 10_000.0);
    assert!(!tick_until_level_change(&mut game, 4));
    assert_eq!(game.monster_death_count(), 0, "the reserve took it");

    // Past the head-opening delay, watching for a level change all along.
    let mut elapsed = 0.0;
    while elapsed < HEAD_OPEN_SECONDS + 0.5 {
        assert!(!tick_until_level_change(&mut game, 1));
        elapsed += TICK_SECONDS;
    }
    queue_monster_damage(&mut game, boss, None, 10_000.0);
    let fired = tick_until_level_change(&mut game, 8);
    assert_eq!(game.monster_death_count(), 1, "the boss died exactly once");
    assert!(!fired, "its TriggerCondition is not honoured");

    // The same declaration on a monster whose page does not say so fires:
    // the exit is reachable, so the silence above is the Nihilanth's own.
    let mut headcrab = game_from(&entities(&format!(
        "{}{}",
        block(
            "monster_headcrab",
            [160.0, 0.0, 36.0],
            &[("TriggerCondition", "4"), ("TriggerTarget", "ohl_exit")],
        ),
        exit_trigger(),
    )));
    let crab = the_monster(&headcrab);
    queue_monster_damage(&mut headcrab, crab, None, 10_000.0);
    assert!(tick_until_level_change(&mut headcrab, 8));
}

// --- The aircraft -------------------------------------------------------------

/// An Apache spawned `Start Inactive` on a two-corner loop, started by a
/// `trigger_auto` when `started`.
fn apache_room(started: bool) -> String {
    entities(&format!(
        "{}{}{}{}",
        block(
            "monster_apache",
            [-160.0, 0.0, 160.0],
            &[
                ("targetname", "ohl_heli"),
                ("target", "ohl_p1"),
                ("spawnflags", "64"),
            ],
        ),
        block(
            "path_corner",
            [160.0, 160.0, 192.0],
            &[("targetname", "ohl_p1"), ("target", "ohl_p2")],
        ),
        block(
            "path_corner",
            [160.0, -160.0, 160.0],
            &[("targetname", "ohl_p2"), ("target", "ohl_p1")],
        ),
        if started {
            trigger_auto("ohl_heli")
        } else {
            String::new()
        },
    ))
}

/// `Start Inactive` (64): the aircraft hangs where it was put until a
/// `use` of its name starts it, and then flies at its route.
#[test]
fn a_start_inactive_apache_waits_for_a_use_and_then_flies_its_route() {
    let mut parked = game_from(&apache_room(false));
    let apache = the_monster(&parked);
    let spawn = origin_of(&parked, apache);
    tick(&mut parked, 100);
    assert!(
        (origin_of(&parked, apache) - spawn).length() < 1e-3,
        "it moved without being started"
    );

    let mut started = game_from(&apache_room(true));
    let apache = the_monster(&started);
    tick(&mut started, 100);
    let moved = origin_of(&started, apache);
    assert!((moved - spawn).length() > 100.0, "{moved:?}");
    assert!(moved.z > spawn.z, "climbing toward the higher corner");
}
