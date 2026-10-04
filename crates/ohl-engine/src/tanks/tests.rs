//! Synthetic value-boundary tests; real Game input/damage integration is pending.
#![allow(clippy::float_cmp)]

use super::*;
use ohl_game::keyvalues::{Limits, parse_entity};

fn def(classname: &str, values: &[(&str, &str)]) -> TankDef {
    let mut raw = [
        ("classname".to_owned(), classname.to_owned()),
        ("spawnflags".to_owned(), "1".to_owned()),
        ("bullet".to_owned(), "1".to_owned()),
        ("bullet_damage".to_owned(), "20".to_owned()),
        ("firerate".to_owned(), "10".to_owned()),
        ("target".to_owned(), "synthetic_shot".to_owned()),
    ]
    .into_iter()
    .collect::<std::collections::BTreeMap<_, _>>();
    raw.extend(
        values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    TankDef::from_entity(&parse_entity(&raw, &Limits::default())).unwrap()
}

struct Fixture {
    _world: ohl_game::hecs::World,
    tank: Entity,
    player: Entity,
    controls: Entity,
}

impl Fixture {
    fn new() -> Self {
        let mut world = ohl_game::hecs::World::new();
        let tank = world.spawn(());
        let player = world.spawn(());
        let controls = world.spawn(());
        Self {
            _world: world,
            tank,
            player,
            controls,
        }
    }

    fn tick(&self, dt: f32) -> TankTick {
        TankTick {
            tank: self.tank,
            placement_origin: Vec3::ZERO,
            pivot_local: Vec3::ZERO,
            dt,
            master_open: true,
            player: Some(PlayerTarget {
                entity: self.player,
                eye: Vec3::X * 128.0,
                alive: true,
            }),
            controlled: None,
        }
    }

    fn input(&self) -> ControlInput {
        ControlInput {
            player: self.player,
            position: Vec3::ZERO,
            view_direction: Vec3::X,
            alive: true,
            use_pressed: false,
            attack: true,
        }
    }

    fn candidate(&self) -> ControlCandidate {
        ControlCandidate {
            tank: self.tank,
            controls: Some(self.controls),
            controllable: true,
            master_open: true,
            bounds: Some(ControlBounds {
                min: Vec3::splat(-4.0),
                max: Vec3::splat(4.0),
            }),
        }
    }
}

/// A project-authored plane and sphere, sufficient to distinguish visibility
/// from the barrel ray hitting the actual target. No canned damage/hit counters.
struct Geometry {
    wall_x: Option<f32>,
    player_radius: f32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            wall_x: None,
            player_radius: 4.0,
        }
    }
}

impl Geometry {
    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        self.wall_x.is_none_or(|x| (from.x - x) * (to.x - x) > 0.0)
    }
}

impl TankQueries for Geometry {
    fn sees_player(&self, _: Entity, from: Vec3, player: PlayerTarget) -> bool {
        self.clear(from, player.eye)
    }

    fn barrel_reaches_player(
        &self,
        _: Entity,
        muzzle: Vec3,
        direction: Vec3,
        range: f32,
        player: PlayerTarget,
    ) -> bool {
        let t = (player.eye - muzzle).dot(direction);
        t >= 0.0
            && t <= range
            && self.clear(muzzle, player.eye)
            && (muzzle + direction * t).distance_squared(player.eye)
                <= self.player_radius * self.player_radius
    }
}

#[test]
fn same_tick_use_and_attack_claims_turret_and_release_does_not_leak_attack() {
    let fixture = Fixture::new();
    let mut system = TankSystem::default();
    let input = ControlInput {
        use_pressed: true,
        ..fixture.input()
    };
    let claimed = system.resolve_controls(input, &[fixture.candidate()], Some(fixture.controls));
    assert!(claimed.consume_use && claimed.suppress_weapons);
    assert!(claimed.cancel_handheld_actions);
    assert_eq!(claimed.controlled.unwrap().tank, fixture.tank);
    assert!(claimed.controlled.unwrap().attack);
    let held = system.resolve_controls(fixture.input(), &[fixture.candidate()], None);
    assert!(held.suppress_weapons && !held.consume_use);
    assert!(!held.cancel_handheld_actions);
    system.queue_remote(TankControlIntent {
        tank: fixture.tank,
        player: fixture.player,
        use_type: TriggerUse::On,
    });
    let remote = ControlCandidate {
        controls: None,
        bounds: None,
        ..fixture.candidate()
    };
    let repeated_on =
        system.resolve_controls(fixture.input(), &[fixture.candidate(), remote], None);
    assert!(!repeated_on.cancel_handheld_actions);
    assert_eq!(system.mounted().unwrap().controls, Some(fixture.controls));
    let released = system.resolve_controls(input, &[fixture.candidate()], Some(fixture.controls));
    assert!(released.consume_use && released.suppress_weapons);
    assert!(!released.cancel_handheld_actions);
    assert!(released.controlled.is_none());
    assert!(
        !system
            .resolve_controls(fixture.input(), &[fixture.candidate()], None)
            .suppress_weapons
    );
}

#[test]
fn mount_revalidates_master_existence_death_and_control_area() {
    let fixture = Fixture::new();
    for invalidation in 0..4 {
        let mut system = TankSystem::default();
        let candidate = fixture.candidate();
        system.resolve_controls(
            ControlInput {
                use_pressed: true,
                ..fixture.input()
            },
            &[candidate],
            Some(fixture.controls),
        );
        let mut input = fixture.input();
        let mut candidates = vec![candidate];
        match invalidation {
            0 => candidates[0].master_open = false,
            1 => candidates.clear(),
            2 => input.alive = false,
            _ => input.position = Vec3::X * (CONTROL_MARGIN + 5.0),
        }
        let released = system.resolve_controls(input, &candidates, None);
        assert!(released.controlled.is_none() && released.suppress_weapons);
        assert!(system.mounted().is_none());
    }
}

#[test]
fn remote_use_keeps_player_identity_and_never_teleports_or_mounts_foreign_operator() {
    let fixture = Fixture::new();
    let mut system = TankSystem::default();
    let remote = ControlCandidate {
        controls: None,
        bounds: None,
        ..fixture.candidate()
    };
    let input = ControlInput {
        position: Vec3::Y * 300.0,
        ..fixture.input()
    };
    system.queue_remote(TankControlIntent {
        tank: fixture.tank,
        player: fixture.controls,
        use_type: TriggerUse::On,
    });
    assert!(
        !system
            .resolve_controls(input, &[remote], None)
            .suppress_weapons
    );
    system.queue_remote(TankControlIntent {
        tank: fixture.tank,
        player: fixture.player,
        use_type: TriggerUse::On,
    });
    let mounted = system.resolve_controls(input, &[remote], None);
    assert!(mounted.suppress_weapons && !mounted.consume_use);
    assert!(mounted.cancel_handheld_actions);
    assert_eq!(system.mounted().unwrap().anchor, input.position);
    system.queue_remote(TankControlIntent {
        tank: fixture.tank,
        player: fixture.player,
        use_type: TriggerUse::On,
    });
    let held = system.resolve_controls(input, &[remote], None);
    assert!(held.suppress_weapons && !held.cancel_handheld_actions);
    let moved = ControlInput {
        position: input.position + Vec3::X * (CONTROL_MARGIN + 1.0),
        ..input
    };
    assert!(
        system
            .resolve_controls(moved, &[remote], None)
            .controlled
            .is_none()
    );
    system.queue_remote(TankControlIntent {
        tank: fixture.tank,
        player: fixture.player,
        use_type: TriggerUse::On,
    });
    system.clear();
    assert!(
        !system
            .resolve_controls(input, &[remote], None)
            .suppress_weapons
    );
}

#[test]
fn automatic_tank_turns_at_authored_rate_and_fires_only_in_tolerance() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("yawrate", "45"), ("yawtolerance", "0")]);
    let mut state = TankState::spawn(&tank, 1);
    let mut tick = fixture.tick(0.1);
    tick.player.as_mut().unwrap().eye = Vec3::Y * 128.0;
    for _ in 0..19 {
        assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
    }
    assert_eq!(state.relative_yaw, 85.5);
    let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
    assert!(shot.direction.abs_diff_eq(Vec3::Y, 1.0e-5));
    assert_eq!(state.relative_yaw, 90.0);
    assert_eq!(shot.source, fixture.tank);
    assert_eq!(shot.attacker, fixture.tank);
}

#[test]
fn automatic_range_and_master_gates_do_not_accumulate_backlog() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("minRange", "64"), ("maxRange", "256")]);
    let mut state = TankState::spawn(&tank, 1);
    let mut tick = fixture.tick(0.1);
    for distance in [32.0, 512.0] {
        tick.player.as_mut().unwrap().eye = Vec3::X * distance;
        for _ in 0..20 {
            assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
        }
    }
    tick.player.as_mut().unwrap().eye = Vec3::X * 128.0;
    tick.master_open = false;
    assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
    tick.master_open = true;
    tick.dt = 0.01;
    assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_some());
    assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
}

#[test]
fn controlled_fire_ignores_automatic_target_ranges_but_keeps_mechanical_stops() {
    let fixture = Fixture::new();
    let tank = def(
        "func_tank",
        &[
            ("spawnflags", "32"),
            ("minRange", "8"),
            ("maxRange", "16"),
            ("yawrange", "0"),
        ],
    );
    let mut state = TankState::spawn(&tank, 1);
    let mut tick = fixture.tick(0.01);
    tick.controlled = Some(ControlledAim {
        tank: fixture.tank,
        player: fixture.player,
        direction: Vec3::Y,
        attack: true,
    });
    let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
    assert_eq!(shot.range, MAX_RANGE);
    assert!(shot.direction.abs_diff_eq(Vec3::X, 1.0e-5));
    assert_eq!(state.relative_yaw, 0.0);
    assert_eq!(shot.attacker, fixture.player);
    tick.controlled.as_mut().unwrap().tank = fixture.controls;
    assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
}

#[test]
fn only_direct_uses_barrel_hit_instead_of_tolerance_and_still_obeys_walls() {
    let fixture = Fixture::new();
    let tank = def(
        "func_tank",
        &[
            ("spawnflags", "17"),
            ("yawrange", "0"),
            ("yawtolerance", "0"),
            ("persistence", "1"),
        ],
    );
    let mut state = TankState::spawn(&tank, 1);
    let mut tick = fixture.tick(0.1);
    tick.player.as_mut().unwrap().eye = Vec3::new(128.0, 32.0, 0.0);
    let geometry = Geometry {
        wall_x: None,
        player_radius: 40.0,
    };
    assert!(advance_tank(&tank, &mut state, tick, &geometry).is_some());
    let normal = TankDef {
        only_direct: false,
        ..tank.clone()
    };
    assert!(advance_tank(&normal, &mut TankState::spawn(&normal, 1), tick, &geometry).is_none());
    let wall = Geometry {
        wall_x: Some(64.0),
        ..geometry
    };
    assert!(advance_tank(&tank, &mut state, tick, &wall).is_none());
    let miss = Geometry {
        wall_x: None,
        player_radius: 1.0,
    };
    assert!(advance_tank(&tank, &mut state, tick, &miss).is_none());
}

#[test]
fn remembered_aim_expires_and_still_emits_a_collidable_barrel_ray() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("persistence", "0.2")]);
    let mut state = TankState::spawn(&tank, 1);
    let mut tick = fixture.tick(0.01);
    assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_some());
    tick.dt = 0.11;
    let wall = Geometry {
        wall_x: Some(64.0),
        player_radius: 4.0,
    };
    let shot = advance_tank(&tank, &mut state, tick, &wall).unwrap();
    assert!(shot.direction.abs_diff_eq(Vec3::X, 1.0e-5));
    assert!(!wall.clear(shot.muzzle, shot.muzzle + shot.direction * shot.range));
    assert!(advance_tank(&tank, &mut state, tick, &wall).is_none());
    assert!(state.memory.is_none());
}

#[test]
fn cadence_is_elapsed_time_based_for_fixed_and_alternating_step_sizes() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[]);
    for steps in [
        vec![0.1; 10],
        vec![0.01; 100],
        vec![0.001; 1000],
        [0.07, 0.03].repeat(10),
    ] {
        let mut state = TankState::spawn(&tank, 1);
        let shots: Vec<_> = steps
            .into_iter()
            .filter_map(|dt| {
                advance_tank(&tank, &mut state, fixture.tick(dt), &Geometry::default())
            })
            .collect();
        assert_eq!(shots.len(), 10);
        assert!(shots.iter().all(|shot| shot.kind
            == TankShotKind::Bullet {
                bullet: TankBullet::NineMillimeter,
                damage: 20.0
            }));
    }
}

#[test]
fn oversized_and_zero_steps_cannot_replay_missed_shots() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[]);
    let mut state = TankState::spawn(&tank, 1);
    assert!(advance_tank(&tank, &mut state, fixture.tick(1.0), &Geometry::default()).is_some());
    let after = state.clone();
    assert!(advance_tank(&tank, &mut state, fixture.tick(0.0), &Geometry::default()).is_none());
    assert_eq!(state, after);
    assert!(advance_tank(&tank, &mut state, fixture.tick(0.01), &Geometry::default()).is_some());
    assert!(advance_tank(&tank, &mut state, fixture.tick(0.01), &Geometry::default()).is_none());
    let mut fractional = TankState::spawn(&tank, 1);
    assert!(
        advance_tank(
            &tank,
            &mut fractional,
            fixture.tick(0.27),
            &Geometry::default()
        )
        .is_some()
    );
    assert!(
        advance_tank(
            &tank,
            &mut fractional,
            fixture.tick(0.01),
            &Geometry::default()
        )
        .is_none()
    );
}

#[test]
fn all_variants_emit_distinct_commands_and_keep_turret_owner_apart_from_operator() {
    let fixture = Fixture::new();
    for name in [
        "func_tank",
        "func_tankrocket",
        "func_tanklaser",
        "func_tankmortar",
    ] {
        let tank = def(name, &[("iMagnitude", "80")]);
        let mut state = TankState::spawn(&tank, 1);
        let tick = TankTick {
            controlled: Some(ControlledAim {
                tank: fixture.tank,
                player: fixture.player,
                direction: Vec3::X,
                attack: true,
            }),
            ..fixture.tick(0.01)
        };
        let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
        assert_eq!(shot.source, fixture.tank);
        assert_eq!(shot.attacker, fixture.player);
        assert_eq!(shot.target.as_deref(), Some("synthetic_shot"));
        match (name, shot.kind) {
            ("func_tank", TankShotKind::Bullet { damage, .. }) => assert_eq!(damage, 20.0),
            (
                "func_tankrocket",
                TankShotKind::Rocket {
                    damage,
                    speed,
                    radius,
                },
            ) => {
                assert_eq!(damage, 20.0);
                assert!(speed > 0.0 && radius > 0.0);
            }
            ("func_tanklaser", TankShotKind::Laser { damage, appearance }) => {
                assert_eq!(damage, 20.0);
                assert!(appearance.is_none());
            }
            ("func_tankmortar", TankShotKind::Mortar { magnitude }) => assert_eq!(magnitude, 80.0),
            _ => panic!("wrong synthetic variant command"),
        }
    }
}

#[test]
fn none_bullet_still_schedules_target_without_damage_and_missing_assets_do_not_gate_laser() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("bullet", "0")]);
    let shot = advance_tank(
        &tank,
        &mut TankState::spawn(&tank, 1),
        fixture.tick(0.01),
        &Geometry::default(),
    )
    .unwrap();
    assert_eq!(
        shot.kind,
        TankShotKind::Bullet {
            bullet: TankBullet::None,
            damage: 0.0
        }
    );
    assert_eq!(shot.target.as_deref(), Some("synthetic_shot"));
    let laser = def("func_tanklaser", &[]);
    let shot = advance_tank(
        &laser,
        &mut TankState::spawn(&laser, 1),
        fixture.tick(0.01),
        &Geometry::default(),
    )
    .unwrap();
    assert_eq!(shot.kind.damage_type(), DamageType::ENERGYBEAM);
}

#[test]
fn damage_zero_override_and_omission_remain_distinct_in_outgoing_commands() {
    let fixture = Fixture::new();
    for (name, omitted_expected) in [
        ("func_tank", 0.0),
        ("func_tankrocket", 100.0),
        ("func_tanklaser", 0.0),
    ] {
        for (present, authored, expected) in [
            (false, 0.0, omitted_expected),
            (true, 0.0, 0.0),
            (true, 23.0, 23.0),
        ] {
            let tank = TankDef {
                damage: authored,
                damage_key_present: present,
                ..def(name, &[])
            };
            let shot = advance_tank(
                &tank,
                &mut TankState::spawn(&tank, 1),
                fixture.tick(0.01),
                &Geometry::default(),
            )
            .unwrap();
            let damage = match shot.kind {
                TankShotKind::Bullet { damage, .. }
                | TankShotKind::Rocket { damage, .. }
                | TankShotKind::Laser { damage, .. } => damage,
                TankShotKind::Mortar { .. } => panic!("unexpected mortar command"),
            };
            assert_eq!(damage, expected);
            assert_eq!(shot.target.as_deref(), Some("synthetic_shot"));
        }
    }
    for (value, expected) in [(None, 100.0), (Some("0"), 0.0), (Some("23"), 23.0)] {
        let values: Vec<_> = value
            .map(|value| ("iMagnitude", value))
            .into_iter()
            .collect();
        let tank = def("func_tankmortar", &values);
        let shot = advance_tank(
            &tank,
            &mut TankState::spawn(&tank, 1),
            fixture.tick(0.01),
            &Geometry::default(),
        )
        .unwrap();
        assert_eq!(
            shot.kind,
            TankShotKind::Mortar {
                magnitude: expected
            }
        );
    }
}

#[test]
fn dead_or_removed_player_clears_persistence_before_any_further_shot() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("persistence", "10")]);
    for removed in [false, true] {
        let mut state = TankState::spawn(&tank, 1);
        let mut tick = fixture.tick(0.1);
        assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_some());
        assert!(state.memory.is_some());
        if removed {
            tick.player = None;
        } else {
            tick.player.as_mut().unwrap().alive = false;
        }
        assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
        assert!(state.memory.is_none());
    }
}

#[test]
fn nonzero_authored_angles_are_not_applied_twice_and_malformed_frames_are_inert() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("angles", "30 170 0")]);
    let mut state = TankState::spawn(&tank, 1);
    let pose = TankPose::new(&tank, &state, Vec3::ZERO, Vec3::ZERO).unwrap();
    let mut tick = fixture.tick(0.01);
    tick.player.as_mut().unwrap().eye = pose.forward() * 128.0;
    let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
    assert!(shot.direction.abs_diff_eq(pose.forward(), 1.0e-5));
    assert!(state.relative_pitch.abs() < 1.0e-4 && state.relative_yaw.abs() < 1.0e-4);
    let before = state.clone();
    for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(
            advance_tank(
                &tank,
                &mut state,
                TankTick { dt, ..tick },
                &Geometry::default()
            )
            .is_none()
        );
        assert_eq!(state, before);
    }
    let malformed = TankDef {
        fire_rate: f32::NAN,
        ..tank
    };
    assert!(advance_tank(&malformed, &mut state, tick, &Geometry::default()).is_none());
    assert_eq!(state, before);
}

#[test]
fn deterministic_spread_continues_from_live_rng_and_cadence_values() {
    let fixture = Fixture::new();
    let tank = def("func_tank", &[("firespread", "3")]);
    let mut state = TankState::spawn(&tank, 123);
    for _ in 0..7 {
        advance_tank(&tank, &mut state, fixture.tick(0.03), &Geometry::default());
    }
    let mut continuation = state.clone();
    for _ in 0..20 {
        assert_eq!(
            advance_tank(&tank, &mut state, fixture.tick(0.03), &Geometry::default()),
            advance_tank(
                &tank,
                &mut continuation,
                fixture.tick(0.03),
                &Geometry::default()
            )
        );
    }
    assert_ne!(state.rng, 123);
}

fn direction_at(pitch: f32, yaw: f32) -> Vec3 {
    let (pitch_sin, pitch_cos) = pitch.to_radians().sin_cos();
    let (yaw_sin, yaw_cos) = yaw.to_radians().sin_cos();
    Vec3::new(pitch_cos * yaw_cos, pitch_cos * yaw_sin, -pitch_sin)
}

#[test]
fn locked_pitch_chooses_reachable_half_turn_for_automatic_and_controlled_aim() {
    let fixture = Fixture::new();
    for pitch_range in ["0", "90"] {
        for controlled in [false, true] {
            let tank = def(
                "func_tank",
                &[
                    ("angles", "60 0 0"),
                    ("pitchrate", "0"),
                    ("pitchrange", pitch_range),
                    ("pitchtolerance", "0.01"),
                    ("yawrange", "180"),
                    ("yawrate", "90"),
                    ("yawtolerance", "0.01"),
                ],
            );
            let mut state = TankState::spawn(&tank, 1);
            let direction = direction_at(60.0, 180.0);
            let mut tick = fixture.tick(0.1);
            tick.player.as_mut().unwrap().eye = direction * 128.0;
            if controlled {
                tick.controlled = Some(ControlledAim {
                    tank: fixture.tank,
                    player: fixture.player,
                    direction,
                    attack: false,
                });
            }
            for _ in 0..19 {
                assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
                assert_eq!(state.relative_pitch, 0.0);
            }
            assert!((state.relative_yaw.abs() - 171.0).abs() < 0.001);
            if let Some(aim) = &mut tick.controlled {
                aim.attack = true;
            }
            let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
            assert!(shot.direction.abs_diff_eq(direction, 1.0e-5));
            assert_eq!(state.relative_pitch, 0.0);
            assert!((state.relative_yaw.abs() - 180.0).abs() < 0.001);
        }
    }
}

#[test]
fn alternate_euler_solution_tracks_current_pose_without_returning_toward_authored_zero() {
    let fixture = Fixture::new();
    let tank = def(
        "func_tank",
        &[
            ("pitchrange", "180"),
            ("pitchrate", "20"),
            ("yawrange", "180"),
            ("yawrate", "20"),
        ],
    );
    let mut state = TankState::spawn(&tank, 1);
    state.relative_pitch = 140.0;
    state.relative_yaw = 170.0;
    let direction = direction_at(145.0, 170.0);
    let tick = TankTick {
        controlled: Some(ControlledAim {
            tank: fixture.tank,
            player: fixture.player,
            direction,
            attack: true,
        }),
        ..fixture.tick(0.1)
    };
    for expected_pitch in [142.0, 144.0, 145.0, 145.0] {
        let shot = advance_tank(&tank, &mut state, tick, &Geometry::default()).unwrap();
        assert!((state.relative_pitch - expected_pitch).abs() < 0.001);
        assert!((state.relative_yaw - 170.0).abs() < 0.001);
        assert!(
            shot.direction
                .abs_diff_eq(direction_at(expected_pitch, 170.0), 1.0e-5)
        );
    }
}

#[test]
fn full_range_pitch_and_yaw_cross_wrap_by_short_steps_without_overshoot() {
    let fixture = Fixture::new();
    let tank = def(
        "func_tank",
        &[
            ("pitchrange", "180"),
            ("pitchrate", "10"),
            ("yawrange", "180"),
            ("yawrate", "10"),
        ],
    );
    let mut state = TankState::spawn(&tank, 1);
    state.relative_pitch = 179.0;
    state.relative_yaw = 179.0;
    let direction = direction_at(-179.0, 175.0);
    let tick = TankTick {
        controlled: Some(ControlledAim {
            tank: fixture.tank,
            player: fixture.player,
            direction,
            attack: false,
        }),
        ..fixture.tick(0.1)
    };
    for _ in 0..5 {
        let previous = (state.relative_pitch, state.relative_yaw);
        assert!(advance_tank(&tank, &mut state, tick, &Geometry::default()).is_none());
        assert!(wrap_degrees(state.relative_pitch - previous.0).abs() <= 1.001);
        assert!(wrap_degrees(state.relative_yaw - previous.1).abs() <= 1.001);
    }
    assert!((state.relative_pitch + 179.0).abs() < 0.001);
    assert!((state.relative_yaw - 175.0).abs() < 0.001);
    // A narrower mechanical range cannot take the seam across its stops.
    assert_eq!(approach_angle(170.0, -170.0, 10.0, 175.0), 160.0);
}

#[test]
fn crossing_vertical_aim_keeps_yaw_continuous() {
    let fixture = Fixture::new();
    let tank = def(
        "func_tank",
        &[
            ("pitchrange", "180"),
            ("pitchrate", "60"),
            ("yawrange", "180"),
            ("yawrate", "720"),
        ],
    );
    let mut state = TankState::spawn(&tank, 1);
    state.relative_pitch = 88.0;
    state.relative_yaw = 37.0;
    for pitch in [89.0, 90.0, 91.0, 92.0] {
        let direction = direction_at(pitch, if pitch == 90.0 { -120.0 } else { 37.0 });
        let tick = TankTick {
            controlled: Some(ControlledAim {
                tank: fixture.tank,
                player: fixture.player,
                direction,
                attack: false,
            }),
            ..fixture.tick(0.05)
        };
        advance_tank(&tank, &mut state, tick, &Geometry::default());
        assert!((state.relative_pitch - pitch).abs() < 0.001);
        assert!((state.relative_yaw - 37.0).abs() < 0.001);
    }
}

#[test]
fn reachable_solution_ranking_accounts_for_axis_rates() {
    let direction = direction_at(60.0, 180.0);
    for (pitch_rate, yaw_rate, expected_pitch, expected_yaw) in
        [("100", "1", 120.0, 0.0), ("1", "100", 60.0, 180.0)]
    {
        let tank = def(
            "func_tank",
            &[
                ("pitchrange", "180"),
                ("pitchrate", pitch_rate),
                ("yawrange", "180"),
                ("yawrate", yaw_rate),
            ],
        );
        let (pitch, yaw) = requested_angles(&tank, &TankState::spawn(&tank, 1), direction);
        assert!((pitch - expected_pitch).abs() < 0.001);
        assert!((yaw.abs() - expected_yaw).abs() < 0.001);
    }
}
