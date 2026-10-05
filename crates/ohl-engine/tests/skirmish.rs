//! Local skirmish (deathmatch against bots) end to end, on a synthetic
//! arena: spawning at `info_player_deathmatch` points with the deathmatch
//! equipment, frags credited to whoever landed the killing hit, the
//! suicide penalty, a click (or `force_respawn`) respawning the player,
//! the frag and time limits, the published pickup respawn delay, the
//! "Not In Deathmatch" flag, and bots that find and fight each other on
//! their own.
//!
//! Reuses `ohl_engine::test_support::deathmatch_room_bsp`, a project-authored
//! room; no bytes here come from any game installation, and every name a
//! test reads back is project-authored (`docs/CLEAN_ROOM.md`).

use std::fmt::Write as _;

use glam::Vec3;
use ohl_engine::skirmish::{
    BotBody, HUMAN_NAME, INTERMISSION_SECONDS, RESPAWN_CLICK_DELAY_SECONDS, WEAPON_RESPAWN_SECONDS,
};
use ohl_engine::test_support::{AI_MAP, deathmatch_room_bsp, queue_engine_damage_from};
use ohl_engine::{
    BotSkill, EngineError, Game, GameEvent, Input, MemoryAssets, SkirmishConfig, TICK_SECONDS,
};
use ohl_game::registry::Transform;

/// Four spawn points in the corners of the room.
const CORNERS: [(f32, f32); 4] = [
    (-192.0, -192.0),
    (192.0, -192.0),
    (-192.0, 192.0),
    (192.0, 192.0),
];

fn assets(bytes: &[u8]) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.to_vec());
    assets
}

fn skirmish(spawns: &[(f32, f32)], extra: &str, config: SkirmishConfig) -> Game {
    let bytes = deathmatch_room_bsp(spawns, false, extra);
    let assets = assets(&bytes);
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the arena loads");
    game.start_skirmish(&assets, &config)
        .expect("the arena has spawn points");
    game
}

/// A quiet match: no bots unless a test asks for them, no limits.
fn quiet(bots: u8) -> SkirmishConfig {
    SkirmishConfig {
        bots,
        frag_limit: 0,
        time_limit_seconds: 0.0,
        ..SkirmishConfig::default()
    }
}

fn tick(game: &mut Game, seconds: f32, input: &Input) -> Vec<GameEvent> {
    let mut events = Vec::new();
    let mut elapsed = 0.0;
    while elapsed + TICK_SECONDS * 0.5 < seconds {
        events.extend(game.tick(TICK_SECONDS, input));
        elapsed += TICK_SECONDS;
    }
    events
}

fn origin_of(game: &Game, entity: ohl_game::hecs::Entity) -> Vec3 {
    game.registry()
        .world
        .get::<&Transform>(entity)
        .expect("a transform")
        .origin
}

fn near_a_spawn(position: Vec3) -> bool {
    CORNERS
        .iter()
        .any(|(x, y)| Vec3::new(*x, *y, position.z).distance(position) < 1.0)
}

fn frags_of(game: &Game, name: &str) -> (i32, u32) {
    let status = game.skirmish_status().expect("a skirmish");
    let row = status
        .scoreboard
        .iter()
        .find(|row| row.name == name)
        .expect("a scoreboard row");
    (row.frags, row.deaths)
}

fn bot_name(game: &Game, index: usize) -> String {
    let status = game.skirmish_status().expect("a skirmish");
    let mut bots: Vec<&str> = status
        .scoreboard
        .iter()
        .filter(|row| !row.is_human)
        .map(|row| row.name.as_str())
        .collect();
    bots.sort_unstable();
    // Bot names are assigned in order and are alphabetical in that order.
    bots[index].to_string()
}

#[test]
fn everyone_spawns_at_a_deathmatch_point_with_the_deathmatch_equipment() {
    let game = skirmish(&CORNERS, "", quiet(3));
    assert!(game.is_skirmish());
    let status = game.skirmish_status().unwrap();
    assert_eq!(status.scoreboard.len(), 4, "the player and three bots");
    assert_eq!(
        status
            .scoreboard
            .iter()
            .filter(|row| row.is_human && row.name == HUMAN_NAME)
            .count(),
        1
    );
    let mut occupied = Vec::new();
    let human = Vec3::from_array(game.player_origin());
    assert!(near_a_spawn(human), "the player stands on a spawn point");
    occupied.push(human);
    for bot in game.skirmish_bots() {
        let body = *game.registry().world.get::<&BotBody>(bot).unwrap();
        assert!(body.alive);
        let position = origin_of(&game, bot);
        assert!(near_a_spawn(position), "every bot stands on a spawn point");
        assert!(
            occupied.iter().all(|other| other.distance(position) > 64.0),
            "nobody spawns inside anyone else"
        );
        occupied.push(position);
    }
    assert!((game.player_health() - 100.0).abs() < f32::EPSILON);
    assert!(game.player_suit_equipped(), "the HEV suit is part of it");
    let inventory = game.inventory();
    assert!(inventory.has_weapon(ohl_combat::WeaponId::Crowbar));
    assert!(inventory.has_weapon(ohl_combat::WeaponId::Glock));
    assert_eq!(inventory.selected(), Some(ohl_combat::WeaponId::Glock));
    assert!(inventory.clip(ohl_combat::WeaponId::Glock) > 0, "loaded");
    assert!(game.skirmish_nav_node_count().unwrap() > 50);
}

#[test]
fn human_gunfire_names_a_playable_sample_at_the_listener() {
    let mut game = skirmish(&CORNERS, "", quiet(0));
    let events = tick(
        &mut game,
        0.5,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    let cue = events
        .iter()
        .find_map(|event| match event {
            GameEvent::Sound(cue) if cue.class == ohl_engine::ChannelClass::Weapon => Some(cue),
            _ => None,
        })
        .expect("firing emits a weapon cue");
    assert_eq!(cue.entity, game.player_entity().id());
    assert_eq!(
        cue.asset,
        ohl_engine::SoundAsset::file("sound/weapons/pl_gun3.wav")
    );
    assert!(cue.origin.is_none());
    assert!(cue.one_shot);
}

#[test]
fn bot_gunfire_reaches_the_host_from_its_own_position() {
    let mut game = skirmish(
        &CORNERS,
        "",
        SkirmishConfig {
            bot_skill: BotSkill::Hard,
            ..quiet(2)
        },
    );
    let bots = game.skirmish_bots();
    let mut heard = false;
    for _ in 0..500 {
        let events = game.tick(TICK_SECONDS, &Input::default());
        assert!(
            !events.contains(&GameEvent::ViewModel(ohl_gameplay::ViewModelAction::Fire)),
            "a bot does not animate the human's viewmodel"
        );
        for event in events {
            let GameEvent::Sound(cue) = event else {
                continue;
            };
            if cue.class != ohl_engine::ChannelClass::Weapon {
                continue;
            }
            let bot = bots
                .iter()
                .find(|bot| bot.id() == cue.entity)
                .expect("the idle human hears another combatant's weapon");
            assert!(!cue.asset.is_unresolved());
            assert!(cue.one_shot);
            let origin = Vec3::from_array(cue.origin.expect("bot sounds are spatialised"));
            assert!(
                origin
                    .truncate()
                    .distance(origin_of(&game, *bot).truncate())
                    < 0.001
            );
            assert!((cue.attenuation - ohl_engine::ATTN_NORM).abs() < f32::EPSILON);
            heard = true;
        }
        if heard {
            break;
        }
    }
    assert!(heard, "bots fighting must emit audible weapon cues");
}

#[test]
fn a_bot_taking_a_pickup_emits_its_sound_in_the_same_tick() {
    let mut extra = String::new();
    for (x, y) in CORNERS {
        let _ = write!(
            extra,
            "{{\n\"classname\" \"weapon_shotgun\"\n\"origin\" \"{x} {y} 36\"\n}}\n"
        );
    }
    let mut game = skirmish(&CORNERS, &extra, quiet(2));
    let bots = game.skirmish_bots();
    let events = game.tick(TICK_SECONDS, &Input::default());
    let cue = events
        .iter()
        .find_map(|event| match event {
            GameEvent::Sound(cue)
                if cue.class == ohl_engine::ChannelClass::Item
                    && bots.iter().any(|bot| bot.id() == cue.entity) =>
            {
                Some(cue)
            }
            _ => None,
        })
        .expect("a bot picks up the weapon at its spawn");
    assert_eq!(
        cue.asset,
        ohl_engine::SoundAsset::file("sound/items/gunpickup2.wav")
    );
    assert!(cue.origin.is_some());
    assert!(cue.one_shot);
}

#[test]
fn the_player_killing_a_bot_scores_a_frag_and_the_bot_respawns() {
    let mut game = skirmish(&CORNERS, "", quiet(1));
    let bot = game.skirmish_bots()[0];
    let name = bot_name(&game, 0);
    let player = game.player_entity();
    queue_engine_damage_from(&mut game, bot, player, 500.0);
    let events = tick(&mut game, TICK_SECONDS, &Input::default());
    assert!(events.contains(&GameEvent::Frag {
        killer: Some(HUMAN_NAME.to_string()),
        victim: name.clone(),
        weapon: Some("9mm pistol"),
        involves_player: true,
    }));
    assert_eq!(frags_of(&game, HUMAN_NAME), (1, 0));
    assert_eq!(frags_of(&game, &name), (0, 1));
    assert!(!game.registry().world.get::<&BotBody>(bot).unwrap().alive);
    // A corpse no longer stops shots.
    let corpse = origin_of(&game, bot);
    tick(&mut game, TICK_SECONDS * 2.0, &Input::default());
    assert_ne!(game.shot_would_reach(corpse), Some(bot));
    tick(&mut game, 3.0, &Input::default());
    assert!(
        game.registry().world.get::<&BotBody>(bot).unwrap().alive,
        "the bot is back after its respawn delay"
    );
}

#[test]
fn a_bot_killing_the_player_scores_and_a_click_respawns_the_player() {
    let mut game = skirmish(&CORNERS, "", quiet(1));
    let bot = game.skirmish_bots()[0];
    let name = bot_name(&game, 0);
    let player = game.player_entity();
    let fire = Input {
        attack: true,
        ..Input::default()
    };
    queue_engine_damage_from(&mut game, player, bot, 500.0);
    // The player dies with the fire button down.
    let events = tick(&mut game, TICK_SECONDS, &fire);
    assert!(events.contains(&GameEvent::PlayerDied));
    assert!(events.iter().any(|event| matches!(
        event,
        GameEvent::Frag { killer: Some(killer), victim, .. }
            if *killer == name && victim == HUMAN_NAME
    )));
    assert_eq!(frags_of(&game, &name), (1, 0));
    assert_eq!(frags_of(&game, HUMAN_NAME), (0, 1));

    let status = game.skirmish_status().unwrap();
    assert!(status.human_dead && !status.respawn_ready);
    // Holding fire through the death is not a click.
    let events = tick(&mut game, RESPAWN_CLICK_DELAY_SECONDS + 0.5, &fire);
    assert!(!events.contains(&GameEvent::PlayerRespawned));
    assert!(game.skirmish_status().unwrap().respawn_ready);
    tick(&mut game, TICK_SECONDS * 2.0, &Input::default());
    let events = tick(&mut game, TICK_SECONDS * 2.0, &fire);
    assert!(events.contains(&GameEvent::PlayerRespawned));
    assert!((game.player_health() - 100.0).abs() < f32::EPSILON);
    assert!(!game.skirmish_status().unwrap().human_dead);
    assert!(near_a_spawn(Vec3::from_array(game.player_origin())));
}

#[test]
fn a_dead_player_neither_reloads_nor_switches_weapons() {
    use ohl_combat::WeaponId;
    let mut game = skirmish(&CORNERS, "", quiet(0));
    let fire = Input {
        attack: true,
        ..Input::default()
    };
    let full = game.inventory().clip(WeaponId::Glock);
    tick(&mut game, 1.0, &fire);
    let fired = game.inventory().clip(WeaponId::Glock);
    assert!(fired < full, "a shot was fired");

    let player = game.player_entity();
    queue_engine_damage_from(&mut game, player, player, 500.0);
    tick(&mut game, TICK_SECONDS, &Input::default());
    assert!(game.skirmish_status().unwrap().human_dead);
    let reload_and_switch = Input {
        reload: true,
        select_slot: Some(1),
        ..Input::default()
    };
    tick(&mut game, TICK_SECONDS, &reload_and_switch);
    tick(&mut game, 3.0, &Input::default());
    assert_eq!(game.inventory().clip(WeaponId::Glock), fired, "no reload");
    assert_eq!(game.inventory().selected(), Some(WeaponId::Glock));
}

#[test]
fn force_respawn_brings_the_player_back_without_a_click() {
    let mut game = skirmish(
        &CORNERS,
        "",
        SkirmishConfig {
            force_respawn: true,
            ..quiet(0)
        },
    );
    let player = game.player_entity();
    queue_engine_damage_from(&mut game, player, player, 500.0);
    let events = tick(&mut game, 6.0, &Input::default());
    assert!(events.contains(&GameEvent::PlayerRespawned));
    assert!(game.player_health() > 0.0);
}

#[test]
fn a_suicide_costs_a_frag() {
    let mut game = skirmish(&CORNERS, "", quiet(0));
    let player = game.player_entity();
    queue_engine_damage_from(&mut game, player, player, 500.0);
    let events = tick(&mut game, TICK_SECONDS, &Input::default());
    assert!(events.iter().any(|event| matches!(
        event,
        GameEvent::Frag { killer: Some(killer), victim, .. } if killer == victim
    )));
    assert_eq!(frags_of(&game, HUMAN_NAME), (-1, 1));
}

#[test]
fn the_frag_limit_ends_the_match_and_freezes_it_for_the_intermission() {
    let mut game = skirmish(
        &CORNERS,
        "",
        SkirmishConfig {
            frag_limit: 1,
            ..quiet(1)
        },
    );
    let bot = game.skirmish_bots()[0];
    let player = game.player_entity();
    queue_engine_damage_from(&mut game, bot, player, 500.0);
    let events = tick(&mut game, TICK_SECONDS, &Input::default());
    assert!(events.contains(&GameEvent::MatchOver {
        winner: HUMAN_NAME.to_string()
    }));
    let status = game.skirmish_status().unwrap();
    assert_eq!(status.winner.as_deref(), Some(HUMAN_NAME));
    assert!(status.intermission_left.unwrap() > INTERMISSION_SECONDS - 0.1);
    // The match stays frozen: no bot respawns, nothing else is scored.
    tick(&mut game, 4.0, &Input::default());
    assert!(!game.registry().world.get::<&BotBody>(bot).unwrap().alive);
    tick(&mut game, INTERMISSION_SECONDS, &Input::default());
    assert!(
        game.skirmish_status()
            .unwrap()
            .intermission_left
            .is_some_and(|left| left <= 0.0),
        "the host leaves once the intermission has run out"
    );
}

#[test]
fn the_time_limit_ends_the_match() {
    let mut game = skirmish(
        &CORNERS,
        "",
        SkirmishConfig {
            time_limit_seconds: 1.0,
            ..quiet(0)
        },
    );
    let events = tick(&mut game, 1.5, &Input::default());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, GameEvent::MatchOver { .. }))
    );
    assert_eq!(game.skirmish_status().unwrap().seconds_left, Some(0.0));
}

#[test]
fn a_taken_weapon_reappears_after_its_published_delay() {
    // One spawn point, with a shotgun lying on it: the player takes it on
    // the first step, and again the moment it reappears (one pickup's ammo
    // never fills the reserve).
    let extra = "{\n\"classname\" \"weapon_shotgun\"\n\"origin\" \"0 0 36\"\n}\n";
    let mut game = skirmish(&[(0.0, 0.0)], extra, quiet(0));
    tick(&mut game, TICK_SECONDS * 2.0, &Input::default());
    assert_eq!(game.pickup_count(), 1);
    assert!(game.inventory().has_weapon(ohl_combat::WeaponId::Shotgun));
    tick(&mut game, WEAPON_RESPAWN_SECONDS - 0.5, &Input::default());
    assert_eq!(game.pickup_count(), 1, "still gone just before the delay");
    tick(&mut game, 1.0, &Input::default());
    assert_eq!(game.pickup_count(), 2, "back, and taken again");
}

#[test]
fn a_not_in_deathmatch_pickup_never_appears() {
    let extra =
        "{\n\"classname\" \"weapon_shotgun\"\n\"origin\" \"0 0 36\"\n\"spawnflags\" \"2048\"\n}\n";
    let mut game = skirmish(&[(0.0, 0.0)], extra, quiet(0));
    tick(&mut game, WEAPON_RESPAWN_SECONDS + 1.0, &Input::default());
    assert_eq!(game.pickup_count(), 0);
    assert!(!game.inventory().has_weapon(ohl_combat::WeaponId::Shotgun));
}

#[test]
fn a_skirmish_is_never_saved() {
    let game = skirmish(&CORNERS, "", quiet(1));
    assert_eq!(game.save_bytes(0), Err(EngineError::SaveUnwritable));
}

#[test]
fn a_map_with_no_spawn_point_at_all_refuses_a_skirmish() {
    let bytes = deathmatch_room_bsp(&[], false, "");
    let assets = assets(&bytes);
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("loads");
    assert_eq!(
        game.start_skirmish(&assets, &quiet(1)),
        Err(EngineError::NoSpawnPoints)
    );
    assert!(!game.is_skirmish());
}

#[test]
fn deathmatch_maps_are_recognised_by_their_spawn_points() {
    assert_eq!(
        ohl_engine::deathmatch_spawn_count(&deathmatch_room_bsp(&CORNERS, false, "")),
        Some(4)
    );
    assert_eq!(
        ohl_engine::deathmatch_spawn_count(&deathmatch_room_bsp(&[], false, "")),
        Some(0)
    );
    assert_eq!(ohl_engine::deathmatch_spawn_count(b"not a map"), None);
}

#[test]
fn bots_find_each_other_and_score_frags_on_their_own() {
    // Two rooms' worth of corners and a weapon cache: five hard bots and an
    // idle player who respawns by themself. Within a simulated minute the
    // bots must have hunted somebody down without any help.
    let extra = "{\n\"classname\" \"weapon_9mmAR\"\n\"origin\" \"0 0 36\"\n}\n\
                 {\n\"classname\" \"ammo_9mmAR\"\n\"origin\" \"64 0 36\"\n}\n";
    let mut game = skirmish(
        &CORNERS,
        extra,
        SkirmishConfig {
            bots: 5,
            bot_skill: BotSkill::Hard,
            force_respawn: true,
            ..quiet(0)
        },
    );
    let events = tick(&mut game, 60.0, &Input::default());
    let credited = events
        .iter()
        .filter(|event| {
            matches!(event, GameEvent::Frag { killer: Some(killer), victim, .. } if killer != victim)
        })
        .count();
    assert!(credited > 0, "bots scored kills on their own");
    let status = game.skirmish_status().unwrap();
    assert!(
        status
            .scoreboard
            .iter()
            .any(|row| !row.is_human && row.frags > 0),
        "a bot tops somebody"
    );
}

#[test]
fn the_same_seed_plays_the_same_match() {
    let config = SkirmishConfig {
        bots: 3,
        force_respawn: true,
        ..quiet(0)
    };
    let mut first = skirmish(&CORNERS, "", config);
    let mut second = skirmish(&CORNERS, "", config);
    let a = tick(&mut first, 20.0, &Input::default());
    let b = tick(&mut second, 20.0, &Input::default());
    assert_eq!(a, b);
    assert_eq!(first.skirmish_status(), second.skirmish_status());
    for (x, y) in first
        .skirmish_bots()
        .into_iter()
        .zip(second.skirmish_bots())
    {
        assert_eq!(origin_of(&first, x), origin_of(&second, y));
    }
}

#[test]
fn the_killing_hit_takes_the_credit_not_a_later_one_in_the_same_step() {
    let mut game = skirmish(&CORNERS, "", quiet(1));
    let bot = game.skirmish_bots()[0];
    let name = bot_name(&game, 0);
    let player = game.player_entity();
    // The bot's shot kills; the player's own blast lands on the corpse after.
    queue_engine_damage_from(&mut game, player, bot, 500.0);
    queue_engine_damage_from(&mut game, player, player, 10.0);
    tick(&mut game, TICK_SECONDS, &Input::default());
    assert_eq!(frags_of(&game, &name), (1, 0), "the bot scored");
    assert_eq!(frags_of(&game, HUMAN_NAME), (0, 1), "no suicide penalty");
}

#[test]
fn nothing_scores_once_a_frag_ends_the_match() {
    let mut game = skirmish(
        &CORNERS,
        "",
        SkirmishConfig {
            frag_limit: 1,
            ..quiet(2)
        },
    );
    let bots = game.skirmish_bots();
    let second = bot_name(&game, 1);
    let player = game.player_entity();
    // In one step: the player's frag ends the match, then a bot kills the
    // player.
    queue_engine_damage_from(&mut game, bots[0], player, 500.0);
    queue_engine_damage_from(&mut game, player, bots[1], 500.0);
    let events = tick(&mut game, TICK_SECONDS, &Input::default());
    assert!(events.contains(&GameEvent::MatchOver {
        winner: HUMAN_NAME.to_string()
    }));
    assert_eq!(frags_of(&game, HUMAN_NAME), (1, 0));
    assert_eq!(frags_of(&game, &second), (0, 0), "too late to score");
    let status = game.skirmish_status().unwrap();
    assert_eq!(
        status.scoreboard[0].name, HUMAN_NAME,
        "the winner tops the board"
    );
}
