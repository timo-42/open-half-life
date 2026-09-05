//! The player systems, driven both directly and from a real `ohl-physics`
//! movement step over this project's synthetic fixtures.

use ohl_physics::test_support::{
    build_flat_floor_bsp, build_liquid_room_bsp, collision_model_from,
};
use ohl_physics::{
    LiquidKind, MoveConfig, MoveInput, Vec3, WaterLevel, contents, player_move_events,
};
use ohl_player::damage::{DAMAGE_PER_EXCESS_FALL_SPEED, SAFE_FALL_SPEED, absorb, fall_damage};
use ohl_player::{
    DamageKind, EmptyWorld, HurtInput, PhysicsOutput, Player, PlayerEvent, PlayerInput,
    PlayerSystems, SuitOccasion,
};

const TICK: f32 = 1.0 / 100.0;

fn suited() -> Player {
    let mut player = Player::default();
    let mut events = Vec::new();
    player.equip_suit(&mut events);
    player
}

fn damage_events(events: &[PlayerEvent]) -> Vec<(i32, DamageKind)> {
    events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Damaged { amount, kind } => Some((*amount, *kind)),
            _ => None,
        })
        .collect()
}

fn suit_occasions(events: &[PlayerEvent]) -> Vec<SuitOccasion> {
    events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Suit(suit) => Some(suit.occasion),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------
// Fall damage
// ---------------------------------------------------------------------

#[test]
fn fall_damage_follows_the_published_curve() {
    // Below the published safe speed nothing happens at all.
    for speed in [SAFE_FALL_SPEED, 0.0, f32::NAN, -1.0] {
        assert!(fall_damage(speed).abs() < f32::EPSILON, "at {speed}");
    }

    // The documented worked example: 1024 ups is exactly lethal to a
    // 100-health player.
    let lethal = fall_damage(1024.0);
    assert!((lethal - 100.0).abs() < 0.5, "{lethal}");

    // And it is linear in the excess speed.
    let excess = fall_damage(SAFE_FALL_SPEED + 111.0);
    assert!((excess - 111.0 * DAMAGE_PER_EXCESS_FALL_SPEED).abs() < 1e-3);
}

#[test]
fn a_landing_costs_health_and_ignores_armor() {
    let mut player = suited();
    let mut ignored = Vec::new();
    player.add_armor(100.0, &mut ignored);

    let physics = PhysicsOutput {
        landed_speed: Some(1000.0),
        ..PhysicsOutput::default()
    };
    let events = player.tick(TICK, &PlayerInput::default(), &physics, &EmptyWorld);
    let damage = damage_events(&events);
    assert_eq!(damage.len(), 1);
    assert_eq!(damage[0].1, DamageKind::Fall);
    // Published: fall damage is absorbed directly into body integrity, so
    // the armor is untouched.
    assert!((player.state.armor - 100.0).abs() < f32::EPSILON);
    assert!(player.state.health < 100.0);
}

#[test]
fn a_step_down_does_no_damage() {
    let mut player = suited();
    let physics = PhysicsOutput {
        landed_speed: Some(120.0),
        ..PhysicsOutput::default()
    };
    let events = player.tick(TICK, &PlayerInput::default(), &physics, &EmptyWorld);
    assert!(damage_events(&events).is_empty());
    assert!((player.state.health - 100.0).abs() < f32::EPSILON);
}

#[test]
fn a_real_fall_through_the_physics_step_hurts_and_a_short_one_does_not() {
    let model = collision_model_from(&build_flat_floor_bsp());
    let move_config = MoveConfig::default();

    for (height, expect_damage) in [(700.0f32, true), (100.0f32, false)] {
        let mut player = suited();
        let mut state = ohl_physics::PlayerState::at(Vec3::new(0.0, 0.0, 36.0 + height));
        let mut hurt = false;
        for _ in 0..600 {
            let events = player_move_events(
                &model,
                &mut state,
                &MoveInput::default(),
                &move_config,
                TICK,
            );
            let physics = PhysicsOutput::from_move(&state, &move_config, &events);
            let player_events = player.tick(TICK, &PlayerInput::default(), &physics, &model);
            if !damage_events(&player_events).is_empty() {
                hurt = true;
            }
            if state.on_ground {
                break;
            }
        }
        assert_eq!(hurt, expect_damage, "falling {height} units");
    }
}

// ---------------------------------------------------------------------
// Drowning
// ---------------------------------------------------------------------

#[test]
fn air_runs_out_under_water_and_recovers_on_the_surface() {
    let mut player = suited();
    let submerged = PhysicsOutput {
        water_level: WaterLevel::Eyes,
        liquid: LiquidKind::Water,
        ..PhysicsOutput::default()
    };

    let mut started = 0;
    let mut hits = 0;
    // 15 seconds at 100 Hz, comfortably past the default air capacity.
    for _ in 0..1500 {
        let events = player.tick(TICK, &PlayerInput::default(), &submerged, &EmptyWorld);
        for event in &events {
            match event {
                PlayerEvent::DrowningStarted => started += 1,
                PlayerEvent::Damaged {
                    kind: DamageKind::Drown,
                    ..
                } => hits += 1,
                _ => {}
            }
        }
    }
    assert_eq!(started, 1, "drowning should start exactly once");
    assert!(hits >= 2, "only {hits} drowning hits");
    assert!(player.state.air_time <= 0.0);
    assert!(player.state.health < 100.0);
    assert_eq!(player.state.waterlevel, 3);

    // Surfacing recovers the air and stops the damage.
    let dry = PhysicsOutput::default();
    let mut surfaced = 0;
    for _ in 0..500 {
        let events = player.tick(TICK, &PlayerInput::default(), &dry, &EmptyWorld);
        surfaced += events
            .iter()
            .filter(|event| matches!(event, PlayerEvent::Surfaced))
            .count();
    }
    assert_eq!(surfaced, 1);
    assert!((player.state.air_time - player.config.air_capacity_seconds).abs() < 1e-3);
}

#[test]
fn wading_waist_deep_never_drowns() {
    let mut player = suited();
    let waist = PhysicsOutput {
        water_level: WaterLevel::Waist,
        liquid: LiquidKind::Water,
        ..PhysicsOutput::default()
    };
    for _ in 0..3000 {
        player.tick(TICK, &PlayerInput::default(), &waist, &EmptyWorld);
    }
    assert!((player.state.health - 100.0).abs() < f32::EPSILON);
    assert_eq!(player.state.waterlevel, 2);
}

// ---------------------------------------------------------------------
// trigger_hurt, slime and lava
// ---------------------------------------------------------------------

#[test]
fn a_trigger_hurt_volume_hits_on_the_documented_half_second_cadence() {
    let mut player = suited();
    let mut input = PlayerInput::default();
    input.push_hurt(HurtInput {
        damage_per_second: 20.0,
        damage_type: 0,
    });
    let physics = PhysicsOutput::default();

    let mut hits = 0;
    // Two seconds at 100 Hz.
    for _ in 0..200 {
        let events = player.tick(TICK, &input, &physics, &EmptyWorld);
        hits += damage_events(&events).len();
    }
    // A hit every half second: four in two seconds (plus the one on the
    // very first tick, when the timer starts at zero).
    assert!((4..=5).contains(&hits), "{hits} hits in two seconds");
}

#[test]
fn a_negative_trigger_hurt_heals() {
    let mut player = suited();
    let mut events = Vec::new();
    player.apply_damage(50.0, DamageKind::Generic, &mut events);
    let hurt_health = player.state.health;

    let mut input = PlayerInput::default();
    input.push_hurt(HurtInput {
        damage_per_second: -20.0,
        damage_type: 0,
    });
    for _ in 0..200 {
        player.tick(TICK, &input, &PhysicsOutput::default(), &EmptyWorld);
    }
    assert!(player.state.health > hurt_health);
    assert!(player.state.health <= player.config.max_health);
}

#[test]
fn a_trigger_hurt_damage_type_is_classified_from_the_documented_bits() {
    // 24 = 8 (burn) + 16 (freeze); burn wins the fixed ordering.
    let hurt = HurtInput {
        damage_per_second: 10.0,
        damage_type: 24,
    };
    assert_eq!(hurt.kind(), DamageKind::Burn);
    assert_eq!(
        HurtInput {
            damage_per_second: 10.0,
            damage_type: 16
        }
        .kind(),
        DamageKind::Freeze
    );
    assert_eq!(
        HurtInput {
            damage_per_second: 10.0,
            damage_type: 0
        }
        .kind(),
        DamageKind::Generic
    );
}

#[test]
fn slime_and_lava_burn_and_water_does_not() {
    for (liquid, expect_kind) in [
        (LiquidKind::Slime, Some(DamageKind::Acid)),
        (LiquidKind::Lava, Some(DamageKind::Burn)),
        (LiquidKind::Water, None),
    ] {
        let mut player = suited();
        let physics = PhysicsOutput {
            water_level: WaterLevel::Waist,
            liquid,
            ..PhysicsOutput::default()
        };
        let mut kinds = Vec::new();
        for _ in 0..100 {
            let events = player.tick(TICK, &PlayerInput::default(), &physics, &EmptyWorld);
            kinds.extend(damage_events(&events).into_iter().map(|(_, kind)| kind));
        }
        match expect_kind {
            Some(kind) => assert!(kinds.iter().all(|seen| *seen == kind) && !kinds.is_empty()),
            None => assert!(kinds.is_empty(), "water hurt the player: {kinds:?}"),
        }
    }
}

#[test]
fn a_lava_pool_built_from_the_physics_fixture_damages_the_player() {
    let model = collision_model_from(&build_liquid_room_bsp(contents::LAVA));
    let move_config = MoveConfig::default();
    let mut player = suited();
    let mut state = ohl_physics::PlayerState::at(Vec3::new(0.0, 0.0, 100.0));
    for _ in 0..200 {
        let events = player_move_events(
            &model,
            &mut state,
            &MoveInput::default(),
            &move_config,
            TICK,
        );
        let physics = PhysicsOutput::from_move(&state, &move_config, &events);
        assert_eq!(physics.liquid, LiquidKind::Lava);
        player.tick(TICK, &PlayerInput::default(), &physics, &model);
    }
    assert!(player.state.health < 100.0);
}

// ---------------------------------------------------------------------
// Armor
// ---------------------------------------------------------------------

#[test]
fn armor_absorbs_the_documented_share_and_runs_out() {
    // With armor left: health takes a fifth, armor drains two fifths.
    let absorbed = absorb(50.0, 100.0, DamageKind::Generic);
    assert!((absorbed.health_loss - 10.0).abs() < 1e-3);
    assert!((absorbed.armor_left - 80.0).abs() < 1e-3);

    // Without armor: everything lands on health.
    let absorbed = absorb(50.0, 0.0, DamageKind::Generic);
    assert!((absorbed.health_loss - 50.0).abs() < 1e-3);

    // Armor exhausted mid-hit: the rest lands on health, continuously with
    // the case above.
    let absorbed = absorb(50.0, 20.0, DamageKind::Generic);
    assert!((absorbed.armor_left - 0.0).abs() < 1e-3);
    assert!(absorbed.health_loss > 0.0 && absorbed.health_loss < 50.0);
}

#[test]
fn armor_only_works_with_the_suit() {
    let mut player = Player::default();
    let mut events = Vec::new();
    player.add_armor(100.0, &mut events);
    assert!((player.state.armor - 0.0).abs() < f32::EPSILON);
    player.apply_damage(30.0, DamageKind::Generic, &mut events);
    assert!((player.state.health - 70.0).abs() < 1e-3);
}

// ---------------------------------------------------------------------
// HEV suit voice
// ---------------------------------------------------------------------

#[test]
fn a_suit_condition_speaks_once_per_cooldown() {
    let mut player = suited();
    let mut events = Vec::new();
    player.add_armor(10.0, &mut events);
    events.clear();

    // One hit exhausts the armor: armor-gone fires.
    player.apply_damage(40.0, DamageKind::Generic, &mut events);
    assert!(suit_occasions(&events).contains(&SuitOccasion::ArmorGone));

    // Draining it again immediately does not repeat the line.
    events.clear();
    player.add_armor(10.0, &mut events);
    player.apply_damage(40.0, DamageKind::Generic, &mut events);
    assert!(!suit_occasions(&events).contains(&SuitOccasion::ArmorGone));

    // After the cooldown it may speak again.
    // 20 seconds at 100 Hz, past the suit's cooldown.
    for _ in 0..2000 {
        player.tick(
            TICK,
            &PlayerInput::default(),
            &PhysicsOutput::default(),
            &EmptyWorld,
        );
    }
    let mut events = Vec::new();
    player.add_armor(10.0, &mut events);
    player.apply_damage(40.0, DamageKind::Generic, &mut events);
    assert!(suit_occasions(&events).contains(&SuitOccasion::ArmorGone));
}

#[test]
fn the_suit_warns_about_critical_health_and_announces_pickups() {
    let mut player = suited();
    let mut events = Vec::new();
    player.apply_damage(80.0, DamageKind::Generic, &mut events);
    assert!(suit_occasions(&events).contains(&SuitOccasion::HealthCritical));

    let mut events = Vec::new();
    player.give_long_jump(&mut events);
    assert!(suit_occasions(&events).contains(&SuitOccasion::LongJumpActivated));
    assert!(player.state.longjump_owned);
}

#[test]
fn without_the_suit_nothing_speaks() {
    let mut player = Player::default();
    let mut events = Vec::new();
    player.apply_damage(95.0, DamageKind::Generic, &mut events);
    assert!(suit_occasions(&events).is_empty());
}

#[test]
fn suit_events_are_spaced_out_and_prioritised() {
    let mut player = suited();
    let mut events = Vec::new();
    // Burn damage raises both a heat warning and, at low health, a health
    // warning.
    player.apply_damage(90.0, DamageKind::Burn, &mut events);
    let suit: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            PlayerEvent::Suit(suit) => Some(*suit),
            _ => None,
        })
        .collect();
    assert!(suit.len() >= 2, "{suit:?}");
    // Each is delayed further than the one before it, so the host never
    // has two lines talking over each other.
    for pair in suit.windows(2) {
        assert!(pair[1].delay > pair[0].delay);
    }
    // The name is the stable symbolic id, not game data.
    assert_eq!(SuitOccasion::HealthCritical.name(), "health_critical",);
    assert!(SuitOccasion::NearDeath.priority() <= SuitOccasion::AmmoPickup.priority());
}

#[test]
fn dying_reports_death_once() {
    let mut player = suited();
    let mut events = Vec::new();
    player.apply_damage(500.0, DamageKind::Generic, &mut events);
    assert!((player.state.health - 0.0).abs() < f32::EPSILON);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, PlayerEvent::Died))
            .count(),
        1
    );
    let mut more = Vec::new();
    player.apply_damage(500.0, DamageKind::Generic, &mut more);
    assert!(!more.iter().any(|event| matches!(event, PlayerEvent::Died)));
}

// ---------------------------------------------------------------------
// Flashlight
// ---------------------------------------------------------------------

#[test]
fn the_flashlight_toggles_drains_and_recharges() {
    let mut player = suited();
    let press = PlayerInput {
        flashlight_pressed: true,
        ..PlayerInput::default()
    };
    let events = player.tick(TICK, &press, &PhysicsOutput::default(), &EmptyWorld);
    assert!(events.contains(&PlayerEvent::FlashlightToggled(true)));
    assert!(player.state.flashlight.on);

    for _ in 0..1000 {
        player.tick(
            TICK,
            &PlayerInput::default(),
            &PhysicsOutput::default(),
            &EmptyWorld,
        );
    }
    let drained = player.state.flashlight.charge;
    assert!(drained < 1.0, "{drained}");

    let events = player.tick(TICK, &press, &PhysicsOutput::default(), &EmptyWorld);
    assert!(events.contains(&PlayerEvent::FlashlightToggled(false)));
    for _ in 0..2000 {
        player.tick(
            TICK,
            &PlayerInput::default(),
            &PhysicsOutput::default(),
            &EmptyWorld,
        );
    }
    assert!(player.state.flashlight.charge > drained);
    assert!(player.state.flashlight.charge <= 1.0);
}

#[test]
fn the_flashlight_switches_itself_off_when_it_runs_dry() {
    let mut player = suited();
    player.config.flashlight_drain_per_second = 10.0;
    let press = PlayerInput {
        flashlight_pressed: true,
        ..PlayerInput::default()
    };
    player.tick(TICK, &press, &PhysicsOutput::default(), &EmptyWorld);
    let mut turned_off = false;
    for _ in 0..100 {
        let events = player.tick(
            TICK,
            &PlayerInput::default(),
            &PhysicsOutput::default(),
            &EmptyWorld,
        );
        if events.contains(&PlayerEvent::FlashlightToggled(false)) {
            turned_off = true;
        }
    }
    assert!(turned_off);
    assert!(!player.state.flashlight.on);
}

#[test]
fn there_is_no_flashlight_without_the_suit() {
    let mut player = Player::default();
    let press = PlayerInput {
        flashlight_pressed: true,
        ..PlayerInput::default()
    };
    let events = player.tick(TICK, &press, &PhysicsOutput::default(), &EmptyWorld);
    assert!(events.is_empty());
    assert!(!player.state.flashlight.on);
}

// ---------------------------------------------------------------------
// HUD and input helpers
// ---------------------------------------------------------------------

#[test]
fn the_hud_snapshot_mirrors_the_player_state() {
    let mut player = suited();
    let mut events = Vec::new();
    player.add_armor(50.0, &mut events);
    player.apply_damage(30.0, DamageKind::Generic, &mut events);
    player.state.waterlevel = 3;
    player.state.air_time = player.config.air_capacity_seconds / 2.0;

    let hud = player
        .state
        .hud_snapshot(player.config.air_capacity_seconds);
    assert_eq!(hud.health, player.state.display_health());
    assert_eq!(hud.armor, player.state.display_armor());
    assert_eq!(hud.waterlevel, 3);
    assert!((hud.air_fraction - 0.5).abs() < 1e-3);
    assert!(hud.suit_equipped);
    assert!(hud.damage_flash);
    assert!(player.state.damage_flags.contains(DamageKind::Generic));
}

#[test]
fn the_long_jump_combo_is_reported_from_the_input() {
    let combo = PlayerInput {
        jump: true,
        duck: true,
        ..PlayerInput::default()
    };
    assert!(combo.is_long_jump_combo());
    assert!(!PlayerInput::default().is_long_jump_combo());
}

// ---------------------------------------------------------------------
// Save/load
// ---------------------------------------------------------------------

fn save_header() -> ohl_save::Header {
    ohl_save::Header {
        game_version: "0.1.0".to_string(),
        created_at_unix_secs: 0,
        map_identity: "player-systems-test".to_string(),
        title: "Player".to_string(),
        thumbnail: Vec::new(),
    }
}

#[test]
fn a_player_round_trips_through_a_save_section() {
    let mut player = suited();
    let mut events = Vec::new();
    player.add_armor(75.0, &mut events);
    player.give_long_jump(&mut events);
    player.apply_damage(37.0, DamageKind::Shock, &mut events);
    player.toggle_flashlight(&mut events);
    player.state.waterlevel = 2;
    player.state.air_time = 3.5;

    let mut writer = ohl_save::SaveWriter::begin(save_header());
    player.write_section(&mut writer).expect("section written");
    let bytes = writer
        .finish(&ohl_save::Limits::default())
        .expect("save written");

    let reader =
        ohl_save::SaveReader::open(&bytes, &ohl_save::Limits::default()).expect("save opens");
    let mut restored = Player::default();
    restored.read_section(&reader).expect("section read");

    assert_eq!(restored.state, player.state);
    assert_eq!(restored.snapshot(), player.snapshot());

    // Saving the restored player again produces the same section bytes.
    let mut writer = ohl_save::SaveWriter::begin(save_header());
    restored
        .write_section(&mut writer)
        .expect("section written");
    let again = writer
        .finish(&ohl_save::Limits::default())
        .expect("save written");
    assert_eq!(bytes, again);
}

#[test]
fn the_player_save_tag_is_an_application_tag() {
    // Below `MIN_APPLICATION_TAG` the container rejects the section.
    let mut writer = ohl_save::SaveWriter::begin(save_header());
    Player::default()
        .write_section(&mut writer)
        .expect("the player tag is an application tag");
}

#[test]
fn restoring_clamps_an_out_of_range_snapshot() {
    let mut player = suited();
    let mut snapshot = player.snapshot();
    snapshot.state.health = 5_000.0;
    snapshot.state.armor = -20.0;
    snapshot.state.air_time = f32::INFINITY;
    snapshot.state.waterlevel = 200;
    snapshot.state.flashlight.charge = 40.0;

    player.restore(&snapshot);
    assert!(player.state.health <= player.config.max_health);
    assert!(player.state.armor >= 0.0);
    assert!(player.state.air_time <= player.config.air_capacity_seconds);
    assert_eq!(player.state.waterlevel, 3);
    assert!(player.state.flashlight.charge <= 1.0);
}
