//! Synthetic real-input continuation and an independent literal tag44 schema.

use ohl_ai::Actor;
use ohl_combat::WeaponId;

use super::live_tests::{equip, fixture, tick};
use super::save::*;
use crate::save::{GameSave, SECTION_TANKS, SavedUseType};
use crate::{Game, Input, TICK_SECONDS};

fn state(game: &Game) -> TanksSnapshot {
    game.to_save(0).tanks.expect("fixture tank")
}

fn health(game: &Game) -> f32 {
    game.registry()
        .world
        .get::<&Actor>(game.registry().find("victim")[0])
        .unwrap()
        .health
}

fn idle(game: &mut Game, count: usize) {
    for _ in 0..count {
        tick(game, false, false);
    }
}

fn replace_section(bytes: &[u8], replacement: &[u8]) -> Vec<u8> {
    let limits = ohl_save::Limits::default();
    let reader = ohl_save::SaveReader::open(bytes, &limits).unwrap();
    let mut writer = ohl_save::SaveWriter::begin(reader.header().clone());
    for entry in reader.sections() {
        writer
            .add_section(
                entry.tag,
                if entry.tag == SECTION_TANKS {
                    replacement
                } else {
                    reader.section(entry.tag).unwrap()
                },
            )
            .unwrap();
    }
    writer.finish(&limits).unwrap()
}

fn golden_value() -> TanksSnapshot {
    TanksSnapshot {
        states: vec![TankStateSnapshot {
            tank: 3,
            active: true,
            relative_pitch: 1.0,
            relative_yaw: -2.0,
            shot_wait: 0.5,
            memory: Some(TankMemorySnapshot {
                point: [3.0, 4.0, 5.0],
                remaining: 0.25,
            }),
            rng: 300,
        }],
        mounted: Some(MountedTankSnapshot {
            tank: 3,
            controls: Some(4),
            player: TankEntityRef::Player,
            anchor: [6.0, 7.0, 8.0],
        }),
        pending_remote: Some(TankIntentSnapshot {
            tank: 9,
            player: TankEntityRef::Player,
            use_type: SavedUseType::On,
        }),
        pending_use: Some(TankIntentSnapshot {
            tank: 10,
            player: TankEntityRef::Registry(11),
            use_type: SavedUseType::Toggle,
        }),
        projectiles: vec![
            TankProjectileSnapshot {
                projectile: 301,
                source: 3,
                attacker: TankEntityRef::Player,
            },
            TankProjectileSnapshot {
                projectile: 7,
                source: 9,
                attacker: TankEntityRef::Registry(12),
            },
        ],
    }
}

#[test]
fn tag44_literal_nonempty_golden_round_trips_through_real_container() {
    // Independently specified postcard field order, including f64 cadence,
    // every optional field and both identity variants. No asset-derived bytes.
    const GOLDEN: &[u8] = &[
        1, 3, 1, 0, 0, 128, 63, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 224, 63, 1, 0, 0, 64, 64, 0, 0,
        128, 64, 0, 0, 160, 64, 0, 0, 128, 62, 172, 2, 1, 3, 1, 4, 0, 0, 0, 192, 64, 0, 0, 224, 64,
        0, 0, 0, 65, 1, 9, 0, 1, 1, 10, 1, 11, 2, 2, 173, 2, 3, 0, 7, 9, 1, 12,
    ];
    let (game, _) = fixture("func_tank", &[], "");
    let expected = golden_value();
    let mut save = game.to_save(123);
    save.tanks = Some(expected.clone());
    let encoded = save.to_bytes().unwrap();
    let reader = ohl_save::SaveReader::open(&encoded, &ohl_save::Limits::default()).unwrap();
    assert_eq!(reader.section(SECTION_TANKS).unwrap(), GOLDEN);
    let decoded = GameSave::from_bytes(&replace_section(&encoded, GOLDEN)).unwrap();
    assert_eq!(decoded.tanks, Some(expected));
    assert_eq!(decoded.to_bytes().unwrap(), encoded);
    assert!(GameSave::from_bytes(&replace_section(&encoded, &[GOLDEN, &[0]].concat())).is_err());
}

#[test]
fn tag44_caps_bound_both_vectors_before_restore_and_writer_rejects_nonfinite() {
    let (game, _) = fixture("func_tank", &[], "");
    let baseline = game.to_save(0);
    let encoded = baseline.to_bytes().unwrap();
    let mut too_many_tanks = golden_value();
    too_many_tanks.states = vec![too_many_tanks.states[0].clone(); 257];
    let mut too_many_rockets = golden_value();
    too_many_rockets.projectiles = vec![too_many_rockets.projectiles[0]; 129];
    for invalid in [too_many_tanks, too_many_rockets] {
        let malicious = postcard::to_allocvec(&invalid).unwrap();
        assert!(GameSave::from_bytes(&replace_section(&encoded, &malicious)).is_err());
        let mut save = baseline.clone();
        save.tanks = Some(invalid);
        assert!(save.to_bytes().is_err());
    }
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        let mut save = baseline.clone();
        save.tanks.as_mut().unwrap().states[0].shot_wait = value;
        assert!(save.to_bytes().is_err());
    }
    let mut invalid = baseline;
    invalid.tanks.as_mut().unwrap().mounted = Some(MountedTankSnapshot {
        tank: 0,
        controls: None,
        player: TankEntityRef::Player,
        anchor: [f32::NAN; 3],
    });
    assert!(invalid.to_bytes().is_err());
}

#[test]
fn old_absent44_preserves_frozen_sections_and_resets_authored_turret_state() {
    let (mut live, assets) = fixture("func_tankrocket", &[], "");
    tick(&mut live, true, true);
    idle(&mut live, 3);
    let modern = live.to_save(0);
    assert!(modern.tanks.as_ref().unwrap().mounted.is_some());
    assert_eq!(modern.tanks.as_ref().unwrap().projectiles.len(), 1);
    let modern_bytes = modern.to_bytes().unwrap();
    let mut legacy = modern;
    legacy.tanks = None;
    let legacy_bytes = legacy.to_bytes().unwrap();
    let limits = ohl_save::Limits::default();
    let new_reader = ohl_save::SaveReader::open(&modern_bytes, &limits).unwrap();
    let old_reader = ohl_save::SaveReader::open(&legacy_bytes, &limits).unwrap();
    for tag in [26, 42, 43] {
        assert_eq!(new_reader.section(tag), old_reader.section(tag));
    }
    assert!(GameSave::from_bytes(&legacy_bytes).unwrap().tanks.is_none());
    let loaded = Game::load_bytes(&assets, &legacy_bytes).unwrap();
    let restored = state(&loaded);
    assert!(
        restored.mounted.is_none()
            && restored.pending_remote.is_none()
            && restored.pending_use.is_none()
    );
    assert!(
        restored.projectiles.is_empty(),
        "absence cannot guess operator credit"
    );
    assert!(!restored.states[0].active);
    assert!(restored.states[0].shot_wait.abs() < f64::EPSILON);
    assert_eq!(
        loaded.projectile_count(),
        1,
        "legacy physical rocket remains live"
    );
}

#[test]
fn mounted_cadence_rng_and_real_damage_continue_from_uninterrupted_live_game() {
    let (mut live, assets) = fixture("func_tank", &[("firerate", "7"), ("firespread", "1")], "");
    tick(&mut live, true, true);
    for _ in 0..6 {
        tick(&mut live, true, false);
    }
    let checkpoint = state(&live);
    assert!(checkpoint.states[0].shot_wait > 0.05);
    assert!(health(&live) < 1000.0);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    assert_eq!(checkpoint, state(&loaded));
    for _ in 0..60 {
        tick(&mut live, true, false);
        tick(&mut loaded, true, false);
        assert_eq!(state(&live), state(&loaded));
        assert!((health(&live) - health(&loaded)).abs() < 0.001);
        assert_eq!(
            live.to_save(0).simulation.pending,
            loaded.to_save(0).simulation.pending
        );
    }
    assert!(
        health(&live) < 950.0,
        "continuation includes subsequent real hits"
    );
}

#[test]
fn automatic_memory_aim_and_cadence_continue_against_live_player() {
    let (mut live, assets) = fixture(
        "func_tank",
        &[
            ("spawnflags", "1"),
            ("firerate", "3"),
            ("bullet_damage", "1"),
        ],
        "",
    );
    idle(&mut live, 40);
    assert!(live.player_health() < 100.0);
    let checkpoint = state(&live);
    assert!(checkpoint.states[0].memory.is_some());
    assert!(checkpoint.states[0].relative_yaw.abs() > 30.0);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    assert_eq!(state(&loaded), checkpoint);
    for _ in 0..70 {
        tick(&mut live, false, false);
        tick(&mut loaded, false, false);
        assert_eq!(state(&live), state(&loaded));
        assert!(
            (live.player_health() - loaded.player_health()).abs() < 0.001,
            "live health {} at {:?}, loaded health {} at {:?}",
            live.player_health(),
            live.player_origin(),
            loaded.player_health(),
            loaded.player_origin()
        );
    }
}

#[test]
fn zero_persistence_automatic_fire_saves_and_stops_when_player_becomes_occluded() {
    for key in ["persistence", "persistance"] {
        let (mut live, assets) = fixture(
            "func_tank",
            &[
                ("spawnflags", "1"),
                (key, "0"),
                ("firerate", "10"),
                ("bullet_damage", "1"),
            ],
            "",
        );
        tick(&mut live, false, false);
        assert!(state(&live).states[0].memory.is_none());
        assert!(live.save_bytes(0).is_ok(), "first visible tick is savable");
        idle(&mut live, 40);
        assert!(live.player_health() < 100.0, "visible aim still fires");
        assert!(state(&live).states[0].relative_yaw.abs() > 30.0);
        let bytes = live
            .save_bytes(0)
            .expect("zero persistence remains savable");
        let mut loaded = Game::load_bytes(&assets, &bytes).unwrap();
        assert_eq!(state(&live), state(&loaded));
        let shots = live.to_save(0).simulation.pending.len();
        assert!(shots > 0);
        // The fixture's independent world wall spans x480..512. Moving the
        // player beyond it removes LOS, independently of the turret source.
        for game in [&mut live, &mut loaded] {
            game.set_viewpoint([600.0, 0.0, 36.0], 0.0, 0.0);
        }
        let health = live.player_health();
        for _ in 0..30 {
            tick(&mut live, false, false);
            tick(&mut loaded, false, false);
            assert!(state(&live).states[0].memory.is_none());
            assert_eq!(state(&live), state(&loaded));
            assert_eq!(live.to_save(0).simulation.pending.len(), shots);
            assert_eq!(loaded.to_save(0).simulation.pending.len(), shots);
            assert!((live.player_health() - health).abs() < 0.001);
            assert!((loaded.player_health() - health).abs() < 0.001);
        }
        assert!(live.save_bytes(0).is_ok());
        assert!(loaded.save_bytes(0).is_ok());
    }
}

#[test]
fn positive_persistence_continues_occluded_memory_then_expires_after_restore() {
    let (mut live, assets) = fixture(
        "func_tank",
        &[
            ("spawnflags", "1"),
            ("persistence", "0.5"),
            ("firerate", "10"),
            ("bullet_damage", "1"),
        ],
        "",
    );
    idle(&mut live, 40);
    assert!(live.player_health() < 100.0);
    let seen = state(&live).states[0].memory.unwrap();
    assert!((seen.remaining - 0.5).abs() < 0.001);
    live.set_viewpoint([600.0, 0.0, 36.0], 0.0, 0.0);
    idle(&mut live, 5);
    let checkpoint = state(&live);
    let memory = checkpoint.states[0].memory.unwrap();
    assert_eq!(memory.point.map(f32::to_bits), seen.point.map(f32::to_bits));
    assert!(memory.remaining > 0.4 && memory.remaining < 0.5);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    assert_eq!(state(&loaded), checkpoint);
    let health = live.player_health();
    let shots = live.to_save(0).simulation.pending.len();
    for step in 0..60 {
        tick(&mut live, false, false);
        tick(&mut loaded, false, false);
        assert_eq!(state(&live), state(&loaded));
        assert_eq!(
            live.to_save(0).simulation.pending,
            loaded.to_save(0).simulation.pending
        );
        assert!((live.player_health() - health).abs() < 0.001);
        assert!((loaded.player_health() - health).abs() < 0.001);
        if step == 20 {
            assert!(state(&live).states[0].memory.is_some());
            assert!(live.to_save(0).simulation.pending.len() > shots);
        }
    }
    assert!(state(&live).states[0].memory.is_none());
    let stopped = live.to_save(0).simulation.pending.len();
    for game in [&mut live, &mut loaded] {
        idle(game, 20);
        assert_eq!(game.to_save(0).simulation.pending.len(), stopped);
        assert!((game.player_health() - health).abs() < 0.001);
    }
}

#[test]
fn rocket_physics_and_operator_mapping_continue_after_actual_launch_and_restore() {
    let (mut live, assets) = fixture("func_tankrocket", &[("bullet_damage", "100")], "");
    tick(&mut live, true, true);
    idle(&mut live, 4);
    let checkpoint = state(&live);
    assert_eq!(checkpoint.projectiles.len(), 1);
    assert_eq!(checkpoint.projectiles[0].attacker, TankEntityRef::Player);
    assert_eq!(checkpoint.projectiles[0].source, checkpoint.states[0].tank);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    for _ in 0..25 {
        assert_eq!(state(&live), state(&loaded));
        assert_eq!(live.to_save(0).projectiles, loaded.to_save(0).projectiles);
        tick(&mut live, false, false);
        tick(&mut loaded, false, false);
        assert!((health(&live) - health(&loaded)).abs() < 0.001);
        assert!(
            (live.player_health() - loaded.player_health()).abs() < 0.001,
            "live health {} at {:?}, loaded health {} at {:?}",
            live.player_health(),
            live.player_origin(),
            loaded.player_health(),
            loaded.player_origin()
        );
    }
    assert!(health(&live) < 1000.0);
    assert!(
        live.player_health() < 100.0,
        "operator remains a splash candidate"
    );
    assert!(
        state(&live).projectiles.is_empty(),
        "terminal overlay retires with rocket"
    );
}

#[test]
fn malformed_duplicate_orphan_rows_cannot_restore_or_upgrade_ownership() {
    let (mut game, assets) = fixture("func_tankrocket", &[], "");
    tick(&mut game, true, true);
    let mut save = game.to_save(0);
    let snapshot = save.tanks.as_mut().unwrap();
    let good_state = snapshot.states[0].clone();
    snapshot.states[0].relative_yaw = f32::NAN;
    snapshot.states.push(good_state);
    let good_projectile = snapshot.projectiles[0];
    snapshot.projectiles[0].attacker = TankEntityRef::Registry(u32::MAX);
    snapshot.projectiles.push(good_projectile);
    snapshot.projectiles.push(TankProjectileSnapshot {
        projectile: u32::MAX,
        ..good_projectile
    });
    snapshot.mounted.as_mut().unwrap().player = TankEntityRef::Registry(snapshot.states[0].tank);
    let loaded = Game::from_save(&assets, &save).unwrap();
    let restored = state(&loaded);
    assert!(
        restored.states[0].shot_wait.abs() < f64::EPSILON,
        "invalid first state reserves its key"
    );
    assert!(
        restored.projectiles.is_empty(),
        "invalid first owner reserves its id"
    );
    assert!(
        restored.mounted.is_none(),
        "registry tank cannot become a player"
    );
    assert_eq!(loaded.projectile_count(), 1);
}

#[test]
fn mounted_restore_revalidates_player_death_controls_and_distance() {
    let (mut live, assets) = fixture("func_tank", &[], "");
    tick(&mut live, false, true);
    let baseline = live.to_save(0);
    for case in 0..3 {
        let mut save = baseline.clone();
        match case {
            0 => save.player.health = 0.0,
            1 => {
                save.tanks
                    .as_mut()
                    .unwrap()
                    .mounted
                    .as_mut()
                    .unwrap()
                    .controls = Some(u32::MAX);
            }
            _ => save.view.position = [300.0, -200.0, 36.0],
        }
        let mut loaded = Game::from_save(&assets, &save).unwrap();
        assert!(loaded.systems_mut().tanks.mounted().is_none());
    }
}

#[test]
fn mounted_restore_cancels_inconsistent_saved_charge_without_refunding_ammo() {
    let (mut game, assets) = fixture("func_tank", &[], "");
    equip(&mut game, WeaponId::Gauss);
    for _ in 0..20 {
        game.tick(
            TICK_SECONDS,
            &Input {
                attack2: true,
                ..Input::default()
            },
        );
    }
    let charged = game.to_save(0).inventory.unwrap().firing.unwrap();
    assert_eq!(charged.state_tag, 3);
    tick(&mut game, false, true);
    let ammo = game.inventory_totals();
    let mut save = game.to_save(0);
    save.inventory.as_mut().unwrap().firing = Some(charged);
    let mut loaded = Game::from_save(&assets, &save).unwrap();
    assert!(loaded.systems_mut().tanks.mounted().is_some());
    assert_eq!(
        loaded
            .to_save(0)
            .inventory
            .unwrap()
            .firing
            .unwrap()
            .state_tag,
        0
    );
    assert_eq!(loaded.inventory_totals(), ammo);
    let before = loaded.weapon_fired_count();
    tick(&mut loaded, true, false);
    assert_eq!(loaded.weapon_fired_count(), before);
    assert_eq!(loaded.inventory_totals(), ammo);
    assert!(health(&loaded) < 1000.0);
}

#[test]
fn cosmetic_laser_pulses_clear_on_restore_without_replaying_damage() {
    let (mut game, assets) = fixture("func_tanklaser", &[], "");
    tick(&mut game, true, true);
    assert_eq!(game.systems_mut().tanks.laser_pulses().len(), 1);
    let mut loaded = Game::load_bytes(&assets, &game.save_bytes(0).unwrap()).unwrap();
    assert!(loaded.systems_mut().tanks.laser_pulses().is_empty());
    assert!((health(&loaded) - 977.0).abs() < 0.001);
    tick(&mut loaded, true, false);
    assert!(loaded.systems_mut().tanks.laser_pulses().is_empty());
    assert!((health(&loaded) - 977.0).abs() < 0.001);
}

fn remote_fixture() -> (Game, crate::MemoryAssets) {
    let extra = crate::test_support::entity_block(
        "func_button",
        [0.0, -200.0, 64.0],
        0.0,
        &[
            ("targetname", "remote_switch"),
            ("target", "remote_relay"),
            ("wait", "-1"),
        ],
    ) + &crate::test_support::entity_block(
        "trigger_relay",
        [0.0; 3],
        0.0,
        &[
            ("targetname", "remote_relay"),
            ("target", "tank"),
            ("triggerstate", "1"),
        ],
    );
    let (mut game, assets) = fixture("func_tank", &[], &extra);
    equip(&mut game, WeaponId::Glock);
    // Weapon pickup supplies reserve rounds. The Glock requires ordinary
    // Reload input, unlike the single-use weapons' automatic chambering.
    game.tick(
        TICK_SECONDS,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    idle(&mut game, 160);
    assert_eq!(game.inventory().clip(WeaponId::Glock), 17);
    game.set_viewpoint([0.0, -200.0, 36.0], 0.0, 0.0);
    tick(&mut game, false, true);
    for _ in 0..100 {
        if state(&game).pending_use.is_some() {
            break;
        }
        tick(&mut game, false, false);
    }
    assert!(state(&game).mounted.is_none());
    assert_eq!(
        state(&game).pending_use.unwrap().player,
        TankEntityRef::Player
    );
    (game, assets)
}

#[test]
fn real_button_relay_remote_claim_survives_save_and_owns_next_attack() {
    let (mut live, assets) = remote_fixture();
    let checkpoint = state(&live);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    assert_eq!(state(&loaded), checkpoint);
    for game in [&mut live, &mut loaded] {
        let ammo = game.inventory_totals();
        let fired = game.weapon_fired_count();
        tick(game, true, false);
        assert!(state(game).mounted.is_some());
        assert!(state(game).pending_use.is_none());
        assert_eq!(game.inventory_totals(), ammo);
        assert_eq!(game.weapon_fired_count(), fired);
        assert!((health(game) - 977.0).abs() < 0.001);
    }
    assert_eq!(state(&live), state(&loaded));
}

#[test]
fn foreign_or_orphan_pending_claim_never_substitutes_local_player() {
    let (live, assets) = remote_fixture();
    let baseline = live.to_save(0);
    for case in 0..2 {
        let mut save = baseline.clone();
        let intent = save.tanks.as_mut().unwrap().pending_use.as_mut().unwrap();
        if case == 0 {
            intent.player = TankEntityRef::Registry(intent.tank);
        } else {
            intent.tank = u32::MAX;
        }
        let mut loaded = Game::from_save(&assets, &save).unwrap();
        assert!(state(&loaded).pending_use.is_none());
        let fired = loaded.weapon_fired_count();
        tick(&mut loaded, true, false);
        assert!(loaded.systems_mut().tanks.mounted().is_none());
        assert!(
            loaded.weapon_fired_count() > fired,
            "ordinary input remains functional: inventory {:?}",
            loaded.to_save(0).inventory
        );
    }
}

#[test]
fn mounted_restore_checks_current_master_after_all_master_overlays() {
    let extra = crate::test_support::entity_block(
        "multisource",
        [0.0; 3],
        0.0,
        &[("targetname", "tank_gate")],
    ) + &crate::test_support::entity_block(
        "func_button",
        [-200.0, -200.0, 64.0],
        0.0,
        &[
            ("targetname", "gate_switch"),
            ("target", "tank_gate"),
            ("wait", "-1"),
        ],
    );
    let (mut game, assets) = fixture("func_tank", &[("master", "tank_gate")], &extra);
    tick(&mut game, true, true);
    assert!(
        game.systems_mut().tanks.mounted().is_none(),
        "closed master denies actual Use"
    );
    assert!((health(&game) - 1000.0).abs() < 0.001);
    game.set_viewpoint([-200.0, -200.0, 36.0], 0.0, 0.0);
    tick(&mut game, false, true);
    idle(&mut game, 30);
    game.set_viewpoint([-48.0, -48.0, 36.0], 0.0, 0.0);
    tick(&mut game, false, true);
    assert!(game.systems_mut().tanks.mounted().is_some());
    let mut saved = game.to_save(0);
    let mut valid = Game::from_save(&assets, &saved).unwrap();
    assert!(valid.systems_mut().tanks.mounted().is_some());
    saved.teleport_state.as_mut().unwrap().master_fires.clear();
    let mut locked = Game::from_save(&assets, &saved).unwrap();
    assert!(locked.systems_mut().tanks.mounted().is_none());
}

#[test]
fn dead_operator_keeps_credit_for_already_launched_rocket_but_cannot_remount() {
    let (mut live, assets) = fixture("func_tankrocket", &[], "");
    tick(&mut live, true, true);
    let mut saved = live.to_save(0);
    saved.player.health = 0.0;
    let mut loaded = Game::from_save(&assets, &saved).unwrap();
    assert!(loaded.systems_mut().tanks.mounted().is_none());
    assert_eq!(
        state(&loaded).projectiles[0].attacker,
        TankEntityRef::Player
    );
    idle(&mut loaded, 25);
    assert!(
        health(&loaded) < 1000.0,
        "an existing attack survives operator death"
    );
}

#[test]
fn distinct_runtime_and_phase12_pending_intents_round_trip_without_early_consumption() {
    let (mut live, assets) = remote_fixture();
    let (level, systems) = live.level_and_systems_mut();
    let mut older = level.simulation.tank_control_intent().unwrap();
    older.use_type = ohl_game::registry::TriggerUse::Off;
    systems.tanks.queue_remote(older);
    let checkpoint = state(&live);
    assert_eq!(
        checkpoint.pending_remote.unwrap().use_type,
        SavedUseType::Off
    );
    assert_eq!(checkpoint.pending_use.unwrap().use_type, SavedUseType::On);
    let mut loaded = Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    assert_eq!(
        state(&loaded),
        checkpoint,
        "restore must not execute either queued request"
    );
    for game in [&mut live, &mut loaded] {
        tick(game, true, false);
        let after = state(game);
        assert!(
            after.mounted.is_some(),
            "latest phase12 request wins next tick"
        );
        assert!(after.pending_remote.is_none() && after.pending_use.is_none());
        assert!((health(game) - 977.0).abs() < 0.001);
    }
}

#[test]
fn valid_but_mismatched_turret_source_cannot_replace_physical_rocket_owner() {
    let extra = crate::test_support::entity_block(
        "func_tankrocket",
        [-256.0, 256.0, 64.0],
        0.0,
        &[
            ("model", "*3"),
            ("targetname", "other_tank"),
            ("spawnflags", "32"),
        ],
    );
    let (mut game, assets) = fixture("func_tankrocket", &[], &extra);
    tick(&mut game, true, true);
    let baseline = game.to_save(0);
    let physical_owner = baseline.projectiles.as_ref().unwrap().projectiles[0]
        .owner
        .unwrap();
    let other = game.registry().find("other_tank")[0];
    assert!(
        game.registry()
            .world
            .get::<&ohl_game::tanks::TankDef>(other)
            .is_ok()
    );
    let other_index = u32::try_from(
        game.registry()
            .entities
            .iter()
            .position(|entity| *entity == other)
            .unwrap(),
    )
    .unwrap();
    assert_ne!(physical_owner, other_index);
    let positive = Game::load_bytes(&assets, &baseline.to_bytes().unwrap()).unwrap();
    assert_eq!(state(&positive).projectiles.len(), 1);
    assert_eq!(state(&positive).projectiles[0].source, physical_owner);
    let mut forged = baseline;
    forged.tanks.as_mut().unwrap().projectiles[0].source = other_index;
    let restored = Game::load_bytes(&assets, &forged.to_bytes().unwrap()).unwrap();
    let after = restored.to_save(0);
    assert!(
        after.tanks.unwrap().projectiles.is_empty(),
        "a valid tank is still not this rocket's physical owner"
    );
    assert_eq!(
        after.projectiles.unwrap().projectiles[0].owner,
        Some(physical_owner)
    );
    assert_eq!(
        after.projectile_runtime.unwrap().attacks[0].owner,
        Some(crate::save_state::ProjectileEntityRef::Registry(
            physical_owner
        ))
    );
}
