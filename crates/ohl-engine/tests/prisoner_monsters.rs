//! The published `Prisoner` monster spawnflag (bit 16), end to end over a
//! synthetic room: a flagged monster with the player in plain view never
//! hurts them, and a `scripted_sequence` fired at it while the player is
//! standing there still takes it over. Nothing outside the AI counts it as
//! a threat either: it is not on the engine's list of monsters hostile to
//! the player, so the guard loop neither aims at it nor backs away from it.
//! And the marker, which has no save field, survives a save and load and a
//! level change that carries the monster across.
//!
//! The second half is the shape of the eleventh chain hop's failure (see
//! `docs/MILESTONES.md`): a map teleports the player in front of flagged
//! monsters and, a moment later, fires an ordinary (no `Override AI`)
//! script at them. Without the flag modelled, the monster was already in
//! combat by the time the script arrived — so the script was refused, as
//! the published `Override AI` text says it must be, and the monster kept
//! shooting a player who was only ever meant to watch it.
//!
//! Reuses `ohl_engine::test_support`'s flat AI room and the entity-block
//! helpers the scripting tests already use. No bytes here come from any
//! game installation; see `docs/CLEAN_ROOM.md`. Every keyvalue and
//! spawnflag below is a published one recorded in `docs/FORMAT_SOURCES.md`.

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, PLAN_SCRIPTED_MONSTER_MODEL, SCRIPT_MAP, SYNTHETIC_MAP, entity_block,
    entity_of_classname, plan_scripted_monster_model_bytes, script_game, script_room_bsp,
    script_room_entities, synthetic_map_bsp_with_extra_entity,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS, guard_input};

/// The published `Prisoner` spawnflag bit, restated here as the map text
/// a mapper would write rather than through `ohl_ai`'s constant, so the
/// test reads the flag the way the entity lump carries it.
const PRISONER: &str = "16";

/// A player facing `+X` with a hostile monster 128 units in front of it,
/// facing back — the closest a room can put them without either having to
/// walk — with the monster's spawnflags chosen by the caller.
fn facing_room(monster_flags: &str) -> String {
    script_room_entities(
        [-64.0, 0.0, 36.0],
        &entity_block(
            "monster_human_grunt",
            [64.0, 0.0, 36.0],
            180.0,
            &[("targetname", "ohl_captive"), ("spawnflags", monster_flags)],
        ),
    )
}

/// [`facing_room`] with the monster wearing the project's synthetic
/// hitbox model, so a shot — and with it the guard loop, which only ever
/// targets what a shot would reach — can find it.
fn hittable_facing_game(monster_flags: &str) -> Game {
    let entities = script_room_entities(
        [-64.0, 0.0, 36.0],
        &entity_block(
            "monster_human_grunt",
            [64.0, 0.0, 36.0],
            180.0,
            &[
                ("model", PLAN_SCRIPTED_MONSTER_MODEL),
                ("spawnflags", monster_flags),
            ],
        ),
    );
    let bytes = script_room_bsp(&entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());
    assets.insert(
        PLAN_SCRIPTED_MONSTER_MODEL,
        plan_scripted_monster_model_bytes(),
    );
    Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("the room loads")
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

fn is_prisoner(game: &Game) -> bool {
    is_prisoner_of(game, "monster_human_grunt")
}

fn is_prisoner_of(game: &Game, classname: &str) -> bool {
    let monster = entity_of_classname(game, classname).expect("the monster spawned");
    game.registry()
        .world
        .get::<&ohl_ai::Prisoner>(monster)
        .is_ok()
}

/// "When enabled, the monster won't attack the player." The same room,
/// with and without the flag: the ordinary monster hurts the player
/// within the window `tests/monster_attacks_player.rs` already relies on,
/// and the prisoner never does.
#[test]
fn a_prisoner_with_the_player_in_view_never_hurts_them() {
    let mut ordinary = script_game(&facing_room("0"));
    let mut captive = script_game(&facing_room(PRISONER));
    assert!(!is_prisoner(&ordinary));
    assert!(is_prisoner(&captive), "the spawnflag became the marker");

    tick(&mut ordinary, 3_000);
    tick(&mut captive, 3_000);

    assert!(
        ordinary.player_health() < 100.0,
        "the unflagged monster is the control: it attacks"
    );
    assert!(
        (captive.player_health() - 100.0).abs() < f32::EPSILON,
        "a prisoner never attacks the player"
    );
    assert_eq!(captive.player_damage_event_count(), 0);
}

/// A script without `Override AI` fired at a monster that is already in
/// combat is refused (published: only `Override AI` "will possess its
/// target even when the monster is in the combat state"). A prisoner is
/// never in combat, so the same script, fired a second after the player
/// appears in front of it, takes it over exactly as the map intends.
#[test]
fn a_script_fired_at_a_prisoner_in_the_players_view_still_possesses_it() {
    let room = |flags: &str| {
        let script = entity_block(
            "scripted_sequence",
            [64.0, 0.0, 36.0],
            180.0,
            &[
                ("targetname", "ohl_script"),
                ("m_iszEntity", "ohl_captive"),
                ("m_iszPlay", "ohl_action"),
                ("m_fMoveTo", "0"),
            ],
        );
        // A second's delay: long enough for an unflagged monster to have
        // seen the player and gone to combat, which is exactly the window
        // a map's own chain of delays opens.
        let starter = entity_block(
            "trigger_auto",
            [0.0, 0.0, 0.0],
            0.0,
            &[("target", "ohl_script"), ("delay", "1")],
        );
        facing_room(flags) + &script + &starter
    };

    let mut ordinary = script_game(&room("0"));
    let mut captive = script_game(&room(PRISONER));
    for game in [&mut ordinary, &mut captive] {
        // Past the auto trigger's delay, with a margin for the fire to
        // reach the script and the script to take its first step.
        tick(game, 150);
    }

    assert_eq!(
        ordinary.script_start_count(),
        0,
        "the control: a monster already fighting refuses an ordinary script"
    );
    assert_eq!(
        captive.script_start_count(),
        1,
        "a prisoner is never fighting, so the script possesses it"
    );
    assert_eq!(captive.active_script_count(), 1);
    assert_eq!(captive.player_damage_event_count(), 0);
}

/// Nothing outside the AI gets to call a prisoner a threat either. The
/// engine's list of monsters hostile to the player — what a planned route
/// decides to guard against, and what the guard loop aims at and backs
/// away from — reads hostility through the same rule a monster's own enemy
/// acquisition does, so the monster that will never take the player as its
/// enemy is not on it.
#[test]
fn a_prisoner_is_not_on_the_list_of_monsters_hostile_to_the_player() {
    let ordinary = script_game(&facing_room("0"));
    let captive = script_game(&facing_room(PRISONER));
    assert_eq!(
        ordinary.hostile_monster_eyes().len(),
        1,
        "the control: the same monster unflagged is hostile"
    );
    assert!(captive.hostile_monster_eyes().is_empty());
}

/// What that list is for, end to end: an empty-handed guard in front of an
/// ordinary hostile monster backs away from it, and in front of a prisoner
/// it holds its ground — a route that is waiting for a map to run a script
/// on that monster does not walk off its spot to flee from it.
#[test]
fn an_empty_handed_guard_holds_its_ground_in_front_of_a_prisoner() {
    let mut ordinary = hittable_facing_game("0");
    let mut captive = hittable_facing_game(PRISONER);
    let start = captive.player_origin();
    let mut ordinary_moved = false;
    for _ in 0..120 {
        let input = guard_input(&ordinary);
        ordinary_moved |= (input.forward, input.right) != (0, 0);
        ordinary.tick(TICK_SECONDS, &input);

        let input = guard_input(&captive);
        assert_eq!(
            (input.forward, input.right, input.up),
            (0, 0, 0),
            "nothing to retreat from"
        );
        assert!(!input.jump && !input.attack);
        captive.tick(TICK_SECONDS, &input);
    }
    assert!(
        ordinary_moved,
        "the control: an unarmed guard backs away from a hostile monster"
    );
    let end = captive.player_origin();
    let moved = (0..2)
        .map(|axis| (end[axis] - start[axis]).abs())
        .fold(0.0_f32, f32::max);
    assert!(moved < 1.0, "the guard never walks away from its spot");
}

/// The marker has no save field of its own: it is rebuilt from the map's
/// own definition whenever a level is built, and a save is restored onto a
/// freshly built level. So a prisoner saved is a prisoner loaded, and an
/// ordinary monster stays ordinary.
#[test]
fn a_prisoner_is_still_a_prisoner_after_a_save_and_load() {
    for (flags, expected) in [(PRISONER, true), ("0", false)] {
        let entities = facing_room(flags);
        let mut game = script_game(&entities);
        tick(&mut game, 30);
        let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
        let mut assets = MemoryAssets::new();
        assets.insert(
            &format!("maps/{SCRIPT_MAP}.bsp"),
            script_room_bsp(&entities),
        );
        let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
        assert_eq!(is_prisoner(&reloaded), expected, "spawnflags {flags}");
    }
}

/// A prisoner the destination map does not declare arrives as one: a
/// carried monster is re-created from its own carried keyvalues,
/// `spawnflags` among them, and a save made after the change keeps it.
#[test]
fn a_carried_prisoner_arrives_a_prisoner_and_stays_one_across_a_save() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{SYNTHETIC_MAP}.bsp"),
        synthetic_map_bsp_with_extra_entity(
            NEXT_MAP,
            &format!(
                "{{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"ohl_captive\"\n\
                 \"spawnflags\" \"{PRISONER}\"\n\"origin\" \"48 0 32\"\n}}\n"
            ),
        ),
    );
    assets.insert(
        &format!("maps/{NEXT_MAP}.bsp"),
        synthetic_map_bsp_with_extra_entity(SYNTHETIC_MAP, ""),
    );
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the source map loads");
    tick(&mut game, 5);
    assert!(is_prisoner_of(&game, "monster_scientist"));

    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("the destination map loads");
    assert_eq!(game.map(), NEXT_MAP);
    assert!(
        is_prisoner_of(&game, "monster_scientist"),
        "the carried monster arrives a prisoner"
    );

    tick(&mut game, 5);
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert!(
        is_prisoner_of(&reloaded, "monster_scientist"),
        "and is still one once that level is saved and loaded"
    );
}
