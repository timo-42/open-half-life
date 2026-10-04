//! Real fixed-step input through Systems; geometry and actors are synthetic.

use ohl_ai::{Actor, MonsterKind};
use ohl_combat::{AmmoType, WeaponId, hud_slot};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_game::hecs::Entity;
use ohl_game::tanks::TankState;
use ohl_physics::Hull;

use super::Vec3;
use crate::test_support::{entity_block, plan_scripted_monster_model_bytes};
use crate::{Game, Input, MemoryAssets, StartInventoryItem, TICK_SECONDS};

/// A solid origin-centered turret, an independently solid authored controls
/// brush (the classname must exclude it), and a stationary damageable actor.
/// No output counters are synthesized: the witness is a real delayed relay.
pub(crate) fn fixture(
    variant: &str,
    overrides: &[(&str, &str)],
    extra: &str,
) -> (Game, MemoryAssets) {
    let mut values = std::collections::BTreeMap::from([
        ("model", "*1"),
        ("targetname", "tank"),
        ("target", "shot_output"),
        ("spawnflags", "32"),
        ("bullet", "1"),
        ("bullet_damage", "23"),
        ("firerate", "1"),
        ("yawrate", "720"),
        ("pitchrate", "720"),
        ("pitchrange", "90"),
        ("laserentity", "beam"),
        ("iMagnitude", "100"),
    ]);
    values.extend(overrides.iter().copied());
    let entries = values.into_iter().collect::<Vec<_>>();
    let entities = format!(
        "{}{}{}{}{}{}{}{}",
        entity_block("worldspawn", [0.0; 3], 0.0, &[]),
        entity_block("info_player_start", [-48.0, -48.0, 36.0], 0.0, &[]),
        entity_block(variant, [0.0, 0.0, 64.0], 0.0, &entries),
        entity_block(
            "func_tankcontrols",
            [-48.0, -48.0, 36.0],
            0.0,
            &[
                ("model", "*2"),
                ("target", "tank"),
                ("targetname", "controls")
            ]
        ),
        entity_block(
            "monster_human_grunt",
            [160.0, 0.0, 36.0],
            180.0,
            &[("targetname", "victim"), ("spawnflags", "16")]
        ),
        entity_block(
            "trigger_relay",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "shot_output"),
                ("target", "witness"),
                ("delay", "5")
            ]
        ),
        entity_block(
            "env_laser",
            [0.0; 3],
            0.0,
            &[
                ("targetname", "beam"),
                ("width", "3"),
                ("rendercolor", "64 128 192"),
                ("renderamt", "96")
            ]
        ),
        extra,
    );
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&entities);
    let world = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::box_brush([480.0, -512.0, 0.0], [512.0, 512.0, 256.0]),
    ]);
    let tank_min = [-16.0, -4.0, -4.0];
    let tank_max = [24.0, 4.0, 4.0];
    let turret = builder.push_collision_hulls(&[CollisionBrush::box_brush(tank_min, tank_max)]);
    let controls_min = [-12.0, -12.0, -36.0];
    let controls_max = [12.0, 12.0, 36.0];
    let controls =
        builder.push_collision_hulls(&[CollisionBrush::box_brush(controls_min, controls_max)]);
    let block_min = [-4.0, -40.0, -64.0];
    let block_max = [4.0, 40.0, 64.0];
    let blocker = builder.push_collision_hulls(&[CollisionBrush::box_brush(block_min, block_max)]);
    builder.push_model([-512.0; 3], [512.0; 3], [0.0; 3], world, 2, 0, 0);
    builder.push_model(tank_min, tank_max, [0.0; 3], turret, 2, 0, 0);
    builder.push_model(controls_min, controls_max, [0.0; 3], controls, 2, 0, 0);
    builder.push_model(block_min, block_max, [0.0; 3], blocker, 2, 0, 0);
    let bytes = builder.build();
    let mut assets = MemoryAssets::new();
    assets.insert("maps/ohl_tanks.bsp", bytes.clone());
    if let Some(path) = MonsterKind::from_classname("monster_human_grunt").default_model_path() {
        assets.insert(path, plan_scripted_monster_model_bytes());
    }
    let game = Game::from_map_bytes(&assets, "ohl_tanks", &bytes).unwrap();
    let victim = named(&game, "victim");
    // Retain MonsterAi: the existing live hitbox and monster-damage adapters
    // require it. The published prisoner flag prevents enemy acquisition while
    // preserving the ordinary physical target and damage path under test.
    game.registry()
        .world
        .get::<&mut Actor>(victim)
        .unwrap()
        .health = 1000.0;
    (game, assets)
}

fn named(game: &Game, name: &str) -> Entity {
    game.registry().find(name)[0]
}

fn health(game: &Game) -> f32 {
    game.registry()
        .world
        .get::<&Actor>(named(game, "victim"))
        .unwrap()
        .health
}

pub(crate) fn tick(game: &mut Game, attack: bool, use_pressed: bool) {
    game.tick(
        TICK_SECONDS,
        &Input {
            attack,
            use_pressed,
            use_held: use_pressed,
            ..Input::default()
        },
    );
}

fn advance(game: &mut Game, count: usize, attack: bool) {
    for _ in 0..count {
        tick(game, attack, false);
    }
}

fn witness_count(game: &Game) -> usize {
    game.to_save(0)
        .simulation
        .pending
        .iter()
        .filter(|fire| fire.target == "witness")
        .count()
}

#[test]
fn actual_use_attack_routes_each_variant_through_combat_and_one_target_edge() {
    for variant in [
        "func_tank",
        "func_tankrocket",
        "func_tanklaser",
        "func_tankmortar",
    ] {
        let (mut game, _) = fixture(variant, &[], "");
        game.give_start_inventory(&[StartInventoryItem::Weapon(WeaponId::Glock)]);
        game.tick(
            TICK_SECONDS,
            &Input {
                select_slot: Some(hud_slot(WeaponId::Glock).slot),
                ..Input::default()
            },
        );
        let ammo = game.inventory_totals();
        let fired = game.weapon_fired_count();
        tick(&mut game, true, true);
        assert!(game.systems_mut().tanks.mounted().is_some(), "{variant}");
        advance(&mut game, 24, true);
        assert!(health(&game) < 1000.0, "{variant} must deliver real damage");
        assert_eq!(game.inventory_totals(), ammo);
        assert_eq!(game.weapon_fired_count(), fired);
        assert_eq!(
            witness_count(&game),
            1,
            "one actual scheduled output for {variant}"
        );
    }
}

#[test]
fn zero_damage_and_none_bullet_preserve_output_without_hitting() {
    for (variant, overrides) in [
        ("func_tank", vec![("bullet_damage", "0")]),
        ("func_tank", vec![("bullet", "0")]),
        ("func_tankrocket", vec![("bullet_damage", "0")]),
        ("func_tanklaser", vec![("bullet_damage", "0")]),
        ("func_tankmortar", vec![("iMagnitude", "0")]),
    ] {
        let (mut game, _) = fixture(variant, &overrides, "");
        tick(&mut game, true, true);
        advance(&mut game, 24, true);
        assert!((health(&game) - 1000.0).abs() < 0.001);
        assert_eq!(witness_count(&game), 1);
    }
}

#[test]
fn ordinary_mount_does_not_fire_the_per_shot_target() {
    let (mut game, _) = fixture("func_tank", &[], "");
    tick(&mut game, false, true);
    assert!(game.systems_mut().tanks.mounted().is_some());
    assert_eq!(witness_count(&game), 0);
    tick(&mut game, true, false);
    assert_eq!(witness_count(&game), 1);
    assert!((health(&game) - 977.0).abs() < 0.001);
}

#[test]
fn automatic_tank_aims_at_and_damages_the_real_player() {
    let (mut game, _) = fixture("func_tank", &[("spawnflags", "1")], "");
    let before = game.player_health();
    advance(&mut game, 40, false);
    assert!(game.player_health() < before);
    assert!(game.systems_mut().tanks.mounted().is_none());
    assert!(
        (health(&game) - 1000.0).abs() < 0.001,
        "NPC is not an autonomous target"
    );
}

#[test]
fn exact_source_brush_is_ignored_but_a_separate_closed_door_still_blocks() {
    let blocker = entity_block(
        "func_door",
        [80.0, 0.0, 64.0],
        90.0,
        &[
            ("model", "*3"),
            ("targetname", "blocker"),
            ("spawnflags", "256"),
        ],
    );
    for variant in ["func_tank", "func_tanklaser", "func_tankrocket"] {
        let (mut clear, _) = fixture(variant, &[], "");
        let (mut blocked, _) = fixture(variant, &[], &blocker);
        for game in [&mut clear, &mut blocked] {
            tick(game, true, true);
            advance(game, 7, false);
        }
        if variant == "func_tankrocket" {
            // At 80 units of scheduled flight the separate door has already
            // terminated one rocket, while the unobstructed rocket is still
            // flying toward the actor. Splash may legitimately cross the door.
            assert_eq!(blocked.projectile_count(), 0);
            assert_eq!(clear.projectile_count(), 1);
            assert!((health(&clear) - 1000.0).abs() < 0.001);
            advance(&mut clear, 17, false);
        } else {
            assert!((health(&blocked) - 1000.0).abs() < 0.001);
        }
        assert!(health(&clear) < 1000.0);
    }
}

#[test]
fn controls_are_invisible_to_both_collision_models_and_live_aim_updates_both() {
    let (mut game, _) = fixture("func_tank", &[], "");
    let origin = game.player_origin();
    game.set_viewpoint(origin, 0.0, 90.0);
    tick(&mut game, false, true);
    advance(&mut game, 16, false);
    let tank = named(&game, "tank");
    assert!(
        (game
            .registry()
            .world
            .get::<&TankState>(tank)
            .unwrap()
            .relative_yaw
            - 90.0)
            .abs()
            < 0.01
    );
    let (level, _) = game.level_and_systems_mut();
    for collision in [
        level.collision.as_ref().unwrap(),
        level.monster_collision.as_ref().unwrap(),
    ] {
        let controls = collision.trace(
            Hull::Point,
            Vec3::new(-72.0, -48.0, 36.0),
            Vec3::new(-24.0, -48.0, 36.0),
        );
        assert!(controls.fraction > 0.999, "controls must remain non-solid");
        let new_pose = collision.trace(
            Hull::Point,
            Vec3::new(-32.0, 16.0, 64.0),
            Vec3::new(32.0, 16.0, 64.0),
        );
        assert!(
            new_pose.fraction < 0.6,
            "both models must see yaw90 immediately"
        );
    }
}

pub(super) fn equip(game: &mut Game, weapon: WeaponId) {
    game.give_start_inventory(&[
        StartInventoryItem::Weapon(weapon),
        StartInventoryItem::Ammo(AmmoType::Uranium),
    ]);
    game.tick(
        TICK_SECONDS,
        &Input {
            select_slot: Some(hud_slot(weapon).slot),
            ..Input::default()
        },
    );
    // A pickup grants single-use weapons in reserve; their ordinary reload
    // must finish before a new primary edge can place a real satchel.
    advance(game, 60, false);
    if weapon == WeaponId::Satchel {
        assert_eq!(game.inventory().clip(weapon), 1);
    }
}

#[test]
fn charged_gauss_mount_cancels_release_without_spending_or_refunding_resources() {
    let (mut game, _) = fixture("func_tank", &[], "");
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
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        3
    );
    let before_ammo = game.inventory_totals();
    let before_fired = game.weapon_fired_count();
    tick(&mut game, true, true);
    assert!(game.systems_mut().tanks.mounted().is_some());
    assert_eq!(game.weapon_fired_count(), before_fired);
    assert_eq!(game.inventory_totals(), before_ammo);
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        0
    );
    assert!(
        (health(&game) - 977.0).abs() < 0.001,
        "only the turret delivers damage"
    );
}

#[test]
fn continuous_egon_mount_and_owned_release_edge_suppress_handheld_pulses() {
    let (mut game, _) = fixture("func_tank", &[], "");
    equip(&mut game, WeaponId::Egon);
    advance(&mut game, 15, true);
    let ammo = game.inventory_totals();
    let fired = game.weapon_fired_count();
    tick(&mut game, true, true);
    advance(&mut game, 30, true);
    tick(&mut game, true, true); // owned release edge
    assert_eq!(game.weapon_fired_count(), fired);
    assert_eq!(game.inventory_totals(), ammo);
    tick(&mut game, true, false); // next ordinary input owns the weapon again
    assert!(game.weapon_fired_count() > fired);
    assert!(game.inventory_totals().1 < ammo.1);
}

#[test]
fn held_satchel_input_cannot_become_a_radio_edge_on_mount_or_release() {
    let (mut game, _) = fixture("func_tank", &[], "");
    equip(&mut game, WeaponId::Satchel);
    tick(&mut game, true, false); // actual placement
    assert_eq!(game.deployable_count(), 1);
    tick(&mut game, false, true); // mount, without Attack
    advance(&mut game, 60, true);
    assert_eq!(game.deployable_count(), 1);
    tick(&mut game, true, true); // release controls with held Attack
    tick(&mut game, true, false); // still held: no new radio edge
    assert_eq!(game.deployable_count(), 1);
    tick(&mut game, false, false);
    tick(&mut game, true, false); // fresh ordinary press really detonates
    assert_eq!(game.deployable_count(), 0);
}

#[test]
fn laser_pulse_captures_live_appearance_once_and_ages_without_extra_damage() {
    let (mut game, _) = fixture("func_tanklaser", &[], "");
    let beam = named(&game, "beam");
    {
        let mut render = game
            .registry()
            .world
            .get::<&mut ohl_game::keyvalues::RenderProps>(beam)
            .unwrap();
        render.color = [25, 50, 75];
        render.amt = 128;
    }
    tick(&mut game, true, true);
    let pulses = game.systems_mut().tanks.laser_pulses().to_vec();
    assert_eq!(pulses.len(), 1);
    let pulse = pulses[0];
    assert!(pulse.start.abs_diff_eq(Vec3::new(0.0, 0.0, 64.0), 0.001));
    assert!(pulse.end.x > 100.0 && pulse.end.x < 170.0);
    assert!((pulse.width - 3.0).abs() < 0.001);
    for (found, authored) in pulse.color.into_iter().zip([25.0, 50.0, 75.0, 128.0]) {
        assert!((found - authored / 255.0).abs() < 0.001);
    }
    assert!((health(&game) - 977.0).abs() < 0.001);
    advance(&mut game, 12, false);
    assert!(game.systems_mut().tanks.laser_pulses().is_empty());
    assert!((health(&game) - 977.0).abs() < 0.001);
}
