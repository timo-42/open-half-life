//! Authority tests use only the project's synthetic arena and project-defined
//! commands. They make no claims about a proprietary engine's wire encoding.

use glam::Vec3;
use ohl_engine::deathmatch::{
    Deathmatch, DeathmatchConfig, DeathmatchError, MAX_QUEUED_COMMANDS, PlayerButtons,
    PlayerCommand, SessionId,
};
use ohl_engine::test_support::{AI_MAP, deathmatch_room_bsp};
use ohl_engine::{GameEvent, MemoryAssets, SkirmishConfig, TICK_SECONDS};

const SPAWNS: [(f32, f32); 4] = [
    (-192.0, -192.0),
    (192.0, -192.0),
    (-192.0, 192.0),
    (192.0, 192.0),
];

fn authority(config: DeathmatchConfig) -> Deathmatch {
    let bytes = deathmatch_room_bsp(&SPAWNS, false, "");
    Deathmatch::from_map_bytes(&MemoryAssets::new(), AI_MAP, &bytes, &config).unwrap()
}

fn config() -> DeathmatchConfig {
    DeathmatchConfig {
        rules: SkirmishConfig {
            bots: 0,
            frag_limit: 0,
            time_limit_seconds: 0.0,
            ..SkirmishConfig::default()
        },
        ..DeathmatchConfig::default()
    }
}

#[test]
fn a_dedicated_match_has_no_phantom_local_player() {
    let mut game = authority(config());
    for _ in 0..200 {
        game.tick(TICK_SECONDS);
    }
    let state = game.snapshot();
    assert!(state.players.is_empty());
    assert!(state.status.scoreboard.is_empty());
    assert!(!state.status.human_dead);
    assert!(game.game().player_health().abs() < f32::EPSILON);
    assert!(
        !game
            .game()
            .registry()
            .world
            .get::<&ohl_ai::Actor>(game.game().player_entity())
            .unwrap()
            .alive
    );
}

#[test]
fn human_capacity_is_32_and_reused_slots_expire_old_identities() {
    let mut game = authority(DeathmatchConfig {
        max_players: 32,
        ..config()
    });
    let first = game.join("First").unwrap();
    for index in 1..32 {
        game.join(&format!("Guest {index}")).unwrap();
    }
    assert_eq!(game.snapshot().players.len(), 32);
    assert_eq!(game.join("Overflow"), Err(DeathmatchError::Full));
    game.leave(first).unwrap();
    let replacement = game.join("Replacement").unwrap();
    assert_eq!(first.slot, replacement.slot);
    assert_ne!(first.generation, replacement.generation);
    assert_eq!(
        game.submit(first, PlayerCommand::default()),
        Err(DeathmatchError::UnknownPlayer)
    );
    assert_eq!(game.leave(first), Err(DeathmatchError::UnknownPlayer));
    game.submit(replacement, PlayerCommand::default()).unwrap();
    let foreign = ohl_engine::deathmatch::PlayerId {
        session: SessionId(999),
        ..replacement
    };
    assert_eq!(
        game.submit(foreign, PlayerCommand::default()),
        Err(DeathmatchError::UnknownPlayer)
    );
}

#[test]
fn bots_share_capacity_without_reserving_an_extra_human_slot() {
    let mut game = authority(DeathmatchConfig {
        max_players: 3,
        rules: SkirmishConfig {
            bots: 2,
            ..config().rules
        },
        ..config()
    });
    assert_eq!(game.snapshot().players.len(), 2);
    assert!(game.snapshot().players.iter().all(|player| player.is_bot));
    game.join("Human").unwrap();
    assert_eq!(game.join("Overflow"), Err(DeathmatchError::Full));
    assert_eq!(game.snapshot().status.scoreboard.len(), 3);
}

#[test]
fn malformed_replayed_and_excess_commands_do_not_enter_the_queue() {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    for command in [
        PlayerCommand {
            duration_millis: 0,
            ..PlayerCommand::default()
        },
        PlayerCommand {
            duration_millis: 101,
            ..PlayerCommand::default()
        },
        PlayerCommand {
            movement: [f32::NAN, 0.0, 0.0],
            ..PlayerCommand::default()
        },
        PlayerCommand {
            movement: [1.01, 0.0, 0.0],
            ..PlayerCommand::default()
        },
        PlayerCommand {
            view_angles: [90.0, 0.0, 0.0],
            ..PlayerCommand::default()
        },
        PlayerCommand {
            view_angles: [0.0, f32::INFINITY, 0.0],
            ..PlayerCommand::default()
        },
    ] {
        assert_eq!(
            game.submit(player, command),
            Err(DeathmatchError::InvalidCommand)
        );
    }
    game.submit(
        player,
        PlayerCommand {
            sequence: u32::MAX,
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(player, PlayerCommand::default()).unwrap();
    assert_eq!(
        game.submit(player, PlayerCommand::default()),
        Err(DeathmatchError::StaleCommand)
    );
    assert_eq!(
        game.submit(
            player,
            PlayerCommand {
                sequence: u32::MAX - 1,
                ..PlayerCommand::default()
            }
        ),
        Err(DeathmatchError::StaleCommand)
    );
    for sequence in 1..u32::try_from(MAX_QUEUED_COMMANDS - 1).unwrap() {
        game.submit(
            player,
            PlayerCommand {
                sequence,
                ..PlayerCommand::default()
            },
        )
        .unwrap();
    }
    assert_eq!(
        game.submit(
            player,
            PlayerCommand {
                sequence: 1000,
                ..PlayerCommand::default()
            }
        ),
        Err(DeathmatchError::CommandQueueFull)
    );
    assert_eq!(game.snapshot().tick, 0);
}

#[test]
fn short_button_pulses_survive_zero_and_fractional_tick_frames_once() {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    // Finish the shared weapon's ordinary deployment before tapping fire.
    for _ in 0..100 {
        game.tick(TICK_SECONDS);
    }
    let loaded = game.snapshot().players[0].clip;
    game.submit(
        player,
        PlayerCommand {
            sequence: 1,
            duration_millis: 1,
            buttons: PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 2,
            duration_millis: 9,
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(0.0);
    game.tick(TICK_SECONDS * 0.5);
    assert_eq!(game.snapshot().players[0].clip, loaded);
    game.tick(TICK_SECONDS * 0.5);
    assert_eq!(game.snapshot().players[0].clip, loaded - 1);
    assert_eq!(game.snapshot().players[0].last_processed_sequence, Some(2));
    for _ in 0..20 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.snapshot().players[0].clip, loaded - 1);
}

fn move_actor(scale: f32) -> f32 {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    let origin = Vec3::from_array(game.snapshot().players[0].origin);
    let yaw = (-origin.y).atan2(-origin.x).to_degrees();
    game.submit(
        player,
        PlayerCommand {
            duration_millis: 100,
            view_angles: [0.0, yaw, 0.0],
            movement: [scale, 0.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    for _ in 0..10 {
        game.tick(TICK_SECONDS);
    }
    Vec3::from_array(game.snapshot().players[0].origin).distance(origin)
}

#[test]
fn movement_keeps_analog_magnitude_and_client_duration_does_not_advance_time() {
    let slow = move_actor(0.25);
    let fast = move_actor(1.0);
    assert!(
        slow > 0.0 && fast > slow * 2.0,
        "analog motion: slow {slow}, fast {fast}"
    );
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    for sequence in 0..64 {
        game.submit(
            player,
            PlayerCommand {
                sequence,
                duration_millis: 100,
                movement: [1.0, 0.0, 0.0],
                ..PlayerCommand::default()
            },
        )
        .unwrap();
    }
    let initial = game.snapshot();
    game.tick(0.0);
    assert_eq!(game.snapshot(), initial);
    game.tick(TICK_SECONDS);
    assert_eq!(game.snapshot().tick, 1);
    assert!((game.snapshot().elapsed - TICK_SECONDS).abs() < f32::EPSILON);
}

#[test]
fn humans_damage_each_other_score_and_respawn_through_shared_gameplay() {
    let mut game = authority(config());
    let shooter = game.join("Shooter").unwrap();
    let victim = game.join("Victim").unwrap();
    let mut frag = false;
    for sequence in 0..1000 {
        let state = game.snapshot();
        if !state.players[1].alive {
            break;
        }
        let delta = Vec3::from_array(state.players[1].origin)
            - (Vec3::from_array(state.players[0].origin) + Vec3::Z * 28.0);
        let yaw = delta.y.atan2(delta.x).to_degrees();
        let pitch = (-delta.z).atan2(delta.truncate().length()).to_degrees();
        game.submit(
            shooter,
            PlayerCommand {
                sequence,
                view_angles: [pitch, yaw, 0.0],
                buttons: PlayerButtons {
                    attack: sequence % 2 == 0,
                    ..PlayerButtons::default()
                },
                ..PlayerCommand::default()
            },
        )
        .unwrap();
        let events = game.tick(TICK_SECONDS);
        frag |= events.iter().any(|event| matches!(event, GameEvent::Frag { killer: Some(killer), victim, .. } if killer == "Shooter" && victim == "Victim"));
    }
    let state = game.snapshot();
    assert!(!state.players[1].alive, "shots reached the other human");
    assert!(frag);
    assert_eq!((state.players[0].frags, state.players[1].deaths), (1, 1));
    for _ in 0..250 {
        game.tick(TICK_SECONDS);
    }
    assert!(
        !game.snapshot().players[1].alive,
        "human respawn requires a click"
    );
    game.submit(
        victim,
        PlayerCommand {
            buttons: PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    assert!(game.snapshot().players[1].alive);
    assert!((game.snapshot().players[1].health - 100.0).abs() < f32::EPSILON);
}

#[test]
fn queued_reload_and_flashlight_edges_survive_release_before_a_step() {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    for _ in 0..100 {
        game.tick(TICK_SECONDS);
    }
    let loaded = game.snapshot().players[0].clip;
    game.submit(
        player,
        PlayerCommand {
            sequence: 1,
            buttons: PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    for _ in 0..50 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.snapshot().players[0].clip, loaded - 1);
    game.submit(
        player,
        PlayerCommand {
            sequence: 2,
            duration_millis: 1,
            buttons: PlayerButtons {
                reload: true,
                flashlight: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 3,
            duration_millis: 9,
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS * 0.25);
    assert!(!game.snapshot().players[0].flashlight_on);
    game.tick(TICK_SECONDS * 0.75);
    assert!(game.snapshot().players[0].flashlight_on);
    for _ in 0..300 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.snapshot().players[0].clip, loaded);
    game.submit(
        player,
        PlayerCommand {
            sequence: 4,
            buttons: PlayerButtons {
                flashlight: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    assert!(!game.snapshot().players[0].flashlight_on);
    game.submit(
        player,
        PlayerCommand {
            sequence: 5,
            buttons: PlayerButtons {
                flashlight: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    assert!(
        !game.snapshot().players[0].flashlight_on,
        "a held button is not another toggle"
    );
}

#[test]
fn dedicated_bots_keep_thinking_moving_and_firing() {
    let mut game = authority(DeathmatchConfig {
        max_players: 2,
        rules: SkirmishConfig {
            bots: 2,
            bot_skill: ohl_engine::BotSkill::Hard,
            ..config().rules
        },
        ..config()
    });
    let initial = game.snapshot();
    let mut fired = false;
    let mut moved = false;
    for _ in 0..700 {
        game.tick(TICK_SECONDS);
        let state = game.snapshot();
        fired |= state.players.iter().any(|player| player.clip < 17);
        moved |= state
            .players
            .iter()
            .zip(&initial.players)
            .any(|(player, previous)| {
                Vec3::from_array(player.origin).distance(Vec3::from_array(previous.origin)) > 0.01
            });
    }
    assert!(fired && moved);
    assert_eq!(game.snapshot().status.scoreboard.len(), 2);
}

#[test]
fn no_spectator_takes_pickups_and_human_pickups_share_respawn_timers() {
    let bytes = deathmatch_room_bsp(
        &[(0.0, 0.0)],
        false,
        "{\n\"classname\" \"weapon_shotgun\"\n\"origin\" \"0 0 36\"\n}\n",
    );
    let mut game =
        Deathmatch::from_map_bytes(&MemoryAssets::new(), AI_MAP, &bytes, &config()).unwrap();
    for _ in 0..100 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.game().pickup_count(), 0);
    let player = game.join("Human").unwrap();
    game.tick(TICK_SECONDS);
    assert_eq!(game.game().pickup_count(), 1);
    game.submit(
        player,
        PlayerCommand {
            weapon: Some(ohl_combat::WeaponId::Shotgun),
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    assert_eq!(
        game.snapshot().players[0].weapon,
        Some(ohl_combat::WeaponId::Shotgun)
    );
    for _ in 0..2100 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.game().pickup_count(), 2);
}

#[test]
fn time_limit_winner_is_a_real_player_and_intermission_freezes_remote_input() {
    let mut game = authority(DeathmatchConfig {
        rules: SkirmishConfig {
            time_limit_seconds: 0.1,
            ..config().rules
        },
        ..config()
    });
    // An empty dedicated match never invents a winner or spectator frag.
    for _ in 0..20 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(game.snapshot().status.winner, None);
    let player = game.join("Human").unwrap();
    game.submit(
        player,
        PlayerCommand {
            movement: [1.0, 0.0, 0.0],
            duration_millis: 100,
            buttons: PlayerButtons {
                attack: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    assert_eq!(game.snapshot().status.winner.as_deref(), Some("Human"));
    // A queued wish must not persist through the match-over idle path.
    for _ in 0..100 {
        game.tick(TICK_SECONDS);
    }
    let stopped = Vec3::from_array(game.snapshot().players[0].origin);
    for _ in 0..100 {
        game.tick(TICK_SECONDS);
    }
    let later = Vec3::from_array(game.snapshot().players[0].origin);
    assert!(later.distance(stopped) < 0.01);
    assert_eq!(game.snapshot().status.scoreboard.len(), 1);
}

#[test]
fn queued_use_press_and_release_activate_a_real_map_button_once() {
    let bytes = deathmatch_room_bsp(
        &[(0.0, 0.0)],
        false,
        "{\n\"classname\" \"func_button\"\n\"origin\" \"0 0 64\"\n\"wait\" \"5\"\n}\n",
    );
    let mut game =
        Deathmatch::from_map_bytes(&MemoryAssets::new(), AI_MAP, &bytes, &config()).unwrap();
    let player = game.join("Human").unwrap();
    let button_state = |game: &Deathmatch| {
        game.game()
            .registry()
            .world
            .query::<&ohl_game::registry::Button>()
            .iter()
            .next()
            .unwrap()
            .state
    };
    assert_eq!(button_state(&game), ohl_game::registry::MoverState::Closed);
    game.submit(
        player,
        PlayerCommand {
            sequence: 1,
            duration_millis: 1,
            buttons: PlayerButtons {
                use_held: true,
                ..PlayerButtons::default()
            },
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 2,
            duration_millis: 9,
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS * 0.5);
    assert_eq!(button_state(&game), ohl_game::registry::MoverState::Closed);
    game.tick(TICK_SECONDS * 0.5);
    assert_eq!(button_state(&game), ohl_game::registry::MoverState::Opening);
    for _ in 0..200 {
        game.tick(TICK_SECONDS);
    }
    assert_eq!(button_state(&game), ohl_game::registry::MoverState::Open);
}

#[test]
fn command_window_orients_each_wish_before_averaging_and_keeps_final_aim() {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    let mut idle = authority(config());
    idle.join("Human").unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 1,
            duration_millis: 5,
            view_angles: [0.0, 0.0, 0.0],
            movement: [1.0, 0.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 2,
            duration_millis: 5,
            view_angles: [0.0, 180.0, 0.0],
            movement: [1.0, 0.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.tick(TICK_SECONDS);
    idle.tick(TICK_SECONDS);
    let moved = &game.snapshot().players[0];
    let stationary = &idle.snapshot().players[0];
    assert!(
        Vec3::from_array(moved.origin).distance(Vec3::from_array(stationary.origin)) < 0.0001,
        "opposed world wishes cancel within the project-defined window"
    );
    assert!((moved.view_angles[1] - 180.0).abs() < f32::EPSILON);
    assert_eq!(moved.last_processed_sequence, Some(2));
}

#[test]
fn command_window_caps_each_diagonal_before_averaging() {
    let mut game = authority(config());
    let player = game.join("Human").unwrap();
    let mut reference = authority(config());
    let reference_player = reference.join("Human").unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 1,
            duration_millis: 5,
            movement: [1.0, 1.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    game.submit(
        player,
        PlayerCommand {
            sequence: 2,
            duration_millis: 5,
            movement: [1.0, -1.0, 0.0],
            ..PlayerCommand::default()
        },
    )
    .unwrap();
    reference
        .submit(
            reference_player,
            PlayerCommand {
                movement: [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0],
                ..PlayerCommand::default()
            },
        )
        .unwrap();
    game.tick(TICK_SECONDS);
    reference.tick(TICK_SECONDS);
    let aggregate = &game.snapshot().players[0];
    let expected = &reference.snapshot().players[0];
    assert!(
        Vec3::from_array(aggregate.origin).distance(Vec3::from_array(expected.origin)) < 0.0001
    );
    assert!(
        Vec3::from_array(aggregate.velocity).distance(Vec3::from_array(expected.velocity)) < 0.0001
    );
}
