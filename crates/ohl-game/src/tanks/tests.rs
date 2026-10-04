//! Synthetic parsing/pose/use contracts. No map assets or retail observations.
#![allow(clippy::float_cmp)]

use super::*;
use crate::keyvalues::{Limits, parse_entity};

fn entity(classname: &str, values: &[(&str, &str)]) -> EntityDef {
    let mut raw = values
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<std::collections::BTreeMap<_, _>>();
    raw.insert("classname".to_owned(), classname.to_owned());
    parse_entity(&raw, &Limits::default())
}

fn def(values: &[(&str, &str)]) -> TankDef {
    TankDef::from_entity(&entity("func_tank", values)).unwrap()
}

#[test]
fn parses_all_variants_and_controls_without_promoting_other_brushes() {
    for (name, kind) in [
        ("func_tank", TankVariant::Bullets),
        ("func_tankrocket", TankVariant::Rocket),
        ("func_tanklaser", TankVariant::Laser),
        ("func_tankmortar", TankVariant::Mortar),
    ] {
        assert_eq!(
            TankDef::from_entity(&entity(name, &[])).unwrap().variant,
            kind
        );
    }
    assert!(TankDef::from_entity(&entity("func_wall", &[])).is_none());
    let controls = TankControls::from_entity(&entity(
        "func_tankcontrols",
        &[("target", "synthetic_tank")],
    ))
    .unwrap();
    assert_eq!(controls.target, "synthetic_tank");
    assert!(TankControls::from_entity(&entity("func_tankcontrols", &[])).is_none());
    assert!(
        TankControls::from_entity(&entity("func_wall", &[("target", "synthetic_tank")])).is_none()
    );
}

#[test]
fn published_keys_keep_explicit_zero_damage_and_legacy_persistence_precedence() {
    let tank = def(&[
        ("spawnflags", "49"),
        ("angles", "30 90 10"),
        ("barrel", "12"),
        ("barrely", "3"),
        ("barrelz", "7"),
        ("bullet", "3"),
        ("bullet_damage", "0"),
        ("persistance", "2"),
        ("persistence", "9"),
        ("target", "synthetic_shot"),
        ("master", "synthetic_gate"),
    ]);
    assert!(tank.starts_active && tank.only_direct && tank.controllable);
    assert_eq!(tank.authored_angles, Vec3::new(30.0, 90.0, 10.0));
    assert_eq!(tank.barrel, Vec3::new(12.0, 3.0, 7.0));
    assert_eq!(tank.bullet, TankBullet::TwelveMillimeter);
    assert_eq!(tank.damage, 0.0);
    assert!(tank.damage_key_present);
    assert_eq!(tank.persistence, 2.0);
    assert_eq!(def(&[("persistence", "3")]).persistence, 3.0);
    assert_eq!(tank.target.as_deref(), Some("synthetic_shot"));
    assert_eq!(tank.master.as_deref(), Some("synthetic_gate"));
}

#[test]
fn published_defaults_distinguish_omitted_damage_from_explicit_zero() {
    let tank = def(&[]);
    assert!(tank.is_valid());
    assert_eq!(tank.damage, 0.0);
    assert!(!tank.damage_key_present);
    assert_eq!(tank.bullet, TankBullet::None);
    assert_eq!(
        (tank.yaw_rate, tank.yaw_range, tank.yaw_tolerance),
        (30.0, 180.0, 15.0)
    );
    assert_eq!(
        (tank.pitch_rate, tank.pitch_range, tank.pitch_tolerance),
        (0.0, 0.0, 5.0)
    );
    assert_eq!(tank.persistence, 1.0);
    assert_eq!(tank.fire_rate, 1.0);
    assert_eq!(def(&[("bullet_damage", "0")]).damage, 0.0);
    assert!(def(&[("bullet_damage", "0")]).damage_key_present);
    assert_eq!(def(&[("bullet_damage", "23")]).damage, 23.0);
    assert_eq!(tank.magnitude, 100.0);
    assert_eq!(def(&[("iMagnitude", "0")]).magnitude, 0.0);
    assert_eq!(def(&[("iMagnitude", "NaN")]).magnitude, 0.0);
}

#[test]
fn malformed_numbers_and_long_utf8_references_stay_bounded() {
    let text = "é".repeat(200);
    let tank = def(&[
        ("firerate", "inf"),
        ("yawrate", "NaN"),
        ("pitchrate", "-5"),
        ("yawrange", "900"),
        ("bullet_damage", "1e30"),
        ("iMagnitude", "-4"),
        ("barrel", "inf"),
        ("minRange", "900"),
        ("maxRange", "128"),
        ("firespread", "999"),
        ("target", &text),
    ]);
    assert!(tank.is_valid());
    assert_eq!(tank.fire_rate, 1.0);
    assert_eq!(tank.yaw_rate, 30.0);
    assert_eq!(tank.pitch_rate, 0.0);
    assert_eq!(tank.yaw_range, 180.0);
    assert_eq!(tank.damage, MAX_DAMAGE);
    assert_eq!(tank.magnitude, 0.0);
    assert_eq!(tank.barrel.x, 0.0);
    assert_eq!(tank.min_range, 128.0);
    assert_eq!(tank.max_range, 128.0);
    assert_eq!(tank.spread, 4);
    assert_eq!(tank.target.as_ref().unwrap().len(), MAX_REFERENCE_BYTES);
    assert_eq!(def(&[("maxRange", "0")]).max_range, MAX_RANGE);
}

#[test]
fn one_nonzero_authored_pose_places_barrel_and_compiled_geometry() {
    let tank = def(&[
        ("angles", "30 90 0"),
        ("barrel", "10"),
        ("barrely", "3"),
        ("barrelz", "4"),
    ]);
    let state = TankState::spawn(&tank, 1);
    let origin = Vec3::new(20.0, 30.0, 40.0);
    let pivot = Vec3::new(2.0, 4.0, 6.0);
    let pose = TankPose::new(&tank, &state, origin, pivot).unwrap();
    let c = 3.0_f32.sqrt() / 2.0;
    assert!(pose.forward().abs_diff_eq(Vec3::new(0.0, c, -0.5), 1.0e-5));
    let expected = origin + pivot + Vec3::new(3.0, 10.0 * c + 2.0, -5.0 + 4.0 * c);
    assert!(pose.muzzle(&tank).abs_diff_eq(expected, 1.0e-5));
    assert!(pose.world_point(pivot).abs_diff_eq(origin + pivot, 1.0e-5));
    assert!(
        pose.world_point(pivot + Vec3::new(10.0, -3.0, 4.0))
            .abs_diff_eq(expected, 1.0e-5)
    );
    let (axis, degrees, compiled_pivot) = pose.axis_angle();
    assert_eq!(compiled_pivot, pivot);
    let rebuilt = Quat::from_axis_angle(axis, degrees.to_radians());
    assert!((rebuilt * Vec3::X).abs_diff_eq(pose.forward(), 1.0e-5));
    assert_eq!(tank.authored_angles, Vec3::new(30.0, 90.0, 0.0));
}

#[test]
fn live_offsets_and_roll_do_not_reapply_authored_yaw_or_move_the_pivot() {
    let tank = def(&[("angles", "0 90 90"), ("barrely", "4")]);
    let mut state = TankState::spawn(&tank, 1);
    state.relative_yaw = 90.0;
    let pose = TankPose::new(&tank, &state, Vec3::ZERO, Vec3::ZERO).unwrap();
    assert!(pose.forward().abs_diff_eq(-Vec3::X, 1.0e-5));
    assert!(pose.muzzle(&tank).abs_diff_eq(-Vec3::Z * 4.0, 1.0e-5));
    assert_eq!(wrap_degrees(370.0), 10.0);
    assert_eq!(wrap_degrees(f32::NAN), 0.0);
    assert!(TankPose::new(&tank, &state, Vec3::splat(f32::NAN), Vec3::ZERO).is_none());
    assert!(TankPose::new(&tank, &state, Vec3::splat(f32::MAX), Vec3::splat(f32::MAX)).is_none());
}

#[test]
fn only_real_player_use_requests_remote_control_and_master_denial_is_inert() {
    let tank = def(&[("spawnflags", "32")]);
    let mut state = TankState::spawn(&tank, 1);
    let mut world = hecs::World::new();
    let source = world.spawn(());
    let player = world.spawn(());
    let foreign = world.spawn(());
    assert!(
        state
            .use_by(
                &tank,
                source,
                TriggerUse::On,
                Some(foreign),
                Some(player),
                true
            )
            .is_none()
    );
    assert!(state.active);
    let request = state
        .use_by(
            &tank,
            source,
            TriggerUse::Toggle,
            Some(player),
            Some(player),
            true,
        )
        .unwrap();
    assert_eq!(request.player, player);
    assert_eq!(request.tank, source);
    assert!(state.active);
    assert!(
        state
            .use_by(
                &tank,
                source,
                TriggerUse::Off,
                Some(player),
                Some(player),
                false
            )
            .is_none()
    );
    assert!(state.active);
    assert!(
        state
            .use_by(&tank, source, TriggerUse::Off, None, Some(player), true)
            .is_none()
    );
    assert!(!state.active);
}

#[test]
fn continuation_sanitization_discards_nonfinite_and_expired_memory() {
    let tank = def(&[
        ("persistence", "2"),
        ("pitchrange", "20"),
        ("yawrange", "40"),
    ]);
    let mut state = TankState::spawn(&tank, 0);
    state.relative_pitch = f32::INFINITY;
    state.relative_yaw = 1000.0;
    state.shot_wait = f64::NAN;
    state.memory = Some(TankMemory {
        point: Vec3::ZERO,
        remaining: f32::INFINITY,
    });
    state.rng = 0;
    state.sanitize(&tank);
    assert_eq!(state.relative_pitch, 0.0);
    assert_eq!(state.relative_yaw, 40.0);
    assert_eq!(state.shot_wait, 0.0);
    assert!(state.memory.is_none());
    assert_eq!(state.rng, 1);
    state.memory = Some(TankMemory {
        point: Vec3::X,
        remaining: 30.0,
    });
    state.sanitize(&tank);
    assert_eq!(state.memory.unwrap().remaining, 2.0);
    state.sanitize(&TankDef {
        pitch_range: f32::NAN,
        ..tank
    });
    assert!(!state.active);
    assert_eq!(state.relative_pitch, 0.0);
    assert_eq!(state.relative_yaw, 0.0);
    assert_eq!(state.shot_wait, 0.0);
    assert!(state.memory.is_none());
}

fn registry(definitions: &[EntityDef]) -> Registry {
    let bounds = std::collections::BTreeMap::from([
        (1, ([4.0, -2.0, -2.0], [12.0, 2.0, 2.0])),
        (2, ([-4.0, -4.0, -4.0], [4.0, 4.0, 4.0])),
    ]);
    Registry::build(definitions, &bounds, &Limits::default())
}

#[test]
fn registry_keeps_all_turret_variants_visible_solid_and_controls_invisible_nonblocking() {
    let mut definitions: Vec<_> = [
        "func_tank",
        "func_tankrocket",
        "func_tanklaser",
        "func_tankmortar",
    ]
    .into_iter()
    .map(|name| entity(name, &[("model", "*1")]))
    .collect();
    definitions.push(entity(
        "func_tankcontrols",
        &[("model", "*2"), ("target", "synthetic_tank")],
    ));
    let registry = registry(&definitions);
    let controls = registry.entities[4];
    assert!(registry.world.get::<&TankControls>(controls).is_ok());
    for id in &registry.entities[..4] {
        assert!(registry.world.get::<&TankDef>(*id).is_ok());
        assert!(registry.world.get::<&TankState>(*id).is_ok());
    }
    for instances in [
        crate::brush::model_instances(&registry),
        crate::brush::solid_model_instances(&registry),
        crate::brush::monster_solid_model_instances(&registry),
    ] {
        assert_eq!(instances.len(), 4);
        assert!(instances.iter().all(|instance| instance.entity != controls));
    }
    assert_eq!(
        crate::find_usable_within(&registry, Vec3::ZERO, 16.0),
        Some(controls)
    );
}

#[test]
fn shared_brush_pose_and_center_read_live_tank_aim_without_changing_authored_transform() {
    let registry = registry(&[entity(
        "func_tank",
        &[
            ("model", "*1"),
            ("origin", "20 30 40"),
            ("angles", "0 90 0"),
            ("barrel", "10"),
        ],
    )]);
    let tank = registry.entities[0];
    registry
        .world
        .get::<&mut TankState>(tank)
        .unwrap()
        .relative_yaw = 90.0;
    let (axis, angle, pivot) = crate::pose::brush_pose_rotation(&registry, tank);
    let rotation = Quat::from_axis_angle(axis, angle.to_radians());
    assert!((rotation * Vec3::X).abs_diff_eq(-Vec3::X, 1.0e-5));
    assert_eq!(pivot, Vec3::ZERO);
    assert!(
        crate::pose::brush_center(&registry, tank)
            .unwrap()
            .abs_diff_eq(Vec3::new(12.0, 30.0, 40.0), 1.0e-5)
    );
    let pose = tank_pose(&registry, tank).unwrap();
    let def = registry.world.get::<&TankDef>(tank).unwrap();
    assert!(
        pose.muzzle(&def)
            .abs_diff_eq(Vec3::new(10.0, 30.0, 40.0), 1.0e-5)
    );
    let transform = registry.world.get::<&Transform>(tank).unwrap();
    assert_eq!(transform.origin, Vec3::new(20.0, 30.0, 40.0));
    assert_eq!(transform.angles, Vec3::new(0.0, 90.0, 0.0));
}

#[test]
fn real_player_relay_use_queues_control_without_firing_the_tanks_shot_target() {
    let mut registry = registry(&[
        entity(
            "func_tank",
            &[
                ("targetname", "synthetic_tank"),
                ("target", "synthetic_shot"),
                ("spawnflags", "32"),
            ],
        ),
        entity(
            "trigger_relay",
            &[
                ("targetname", "synthetic_relay"),
                ("target", "synthetic_tank"),
            ],
        ),
        entity("info_target", &[("targetname", "synthetic_player")]),
    ]);
    let tank = registry.entities[0];
    let relay = registry.entities[1];
    let player = registry.entities[2];
    let mut simulation = crate::Simulation::new();
    simulation.set_effect_player(Some(crate::effects::EffectPlayer {
        entity: player,
        origin: Vec3::ZERO,
        grounded: true,
    }));
    simulation.use_entity(&mut registry, relay, Some(player), &mut Vec::new());
    simulation.tick(&mut registry, 0.01);
    let intent = simulation.take_tank_control_intent().unwrap();
    assert_eq!(intent.tank, tank);
    assert_eq!(intent.player, player);
    assert!(simulation.snapshot().pending.is_empty());
    assert!(!registry.world.get::<&TankState>(tank).unwrap().active);
    // A missing authoritative player cannot turn even a supplied handle into
    // a player mount request. It follows ordinary automatic on/off use.
    simulation.set_effect_player(None);
    simulation.use_entity(&mut registry, tank, Some(player), &mut Vec::new());
    assert!(simulation.take_tank_control_intent().is_none());
    assert!(registry.world.get::<&TankState>(tank).unwrap().active);
}

#[test]
fn controls_use_obeys_tank_master_and_legacy_simulation_restore_drops_pending_mount() {
    let mut registry = registry(&[
        entity(
            "func_tank",
            &[
                ("targetname", "synthetic_tank"),
                ("spawnflags", "32"),
                ("master", "synthetic_gate"),
            ],
        ),
        entity("func_tankcontrols", &[("target", "synthetic_tank")]),
        entity("multisource", &[("targetname", "synthetic_gate")]),
        entity("info_target", &[("targetname", "synthetic_player")]),
    ]);
    let (tank, controls, gate, player) = (
        registry.entities[0],
        registry.entities[1],
        registry.entities[2],
        registry.entities[3],
    );
    let mut simulation = crate::Simulation::new();
    simulation.set_effect_player(Some(crate::effects::EffectPlayer {
        entity: player,
        origin: Vec3::ZERO,
        grounded: true,
    }));
    assert!(!simulation.master_is_active(&registry, tank));
    simulation.use_entity(&mut registry, controls, Some(player), &mut Vec::new());
    assert!(simulation.tank_control_intent().is_none());
    simulation.use_entity(&mut registry, gate, Some(player), &mut Vec::new());
    assert!(simulation.master_is_active(&registry, tank));
    simulation.use_entity(&mut registry, controls, Some(player), &mut Vec::new());
    assert_eq!(simulation.tank_control_intent().unwrap().tank, tank);
    simulation.restore(&simulation.snapshot());
    assert!(simulation.tank_control_intent().is_none());
}
