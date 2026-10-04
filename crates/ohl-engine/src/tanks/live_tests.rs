//! Real fixed-step input through Systems; geometry and actors are synthetic.

use ohl_ai::{Actor, MonsterKind};
use ohl_combat::{AmmoType, WeaponId, hud_slot};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, build_minimal_mdl10_with_hitbox};
use ohl_game::hecs::Entity;
use ohl_game::tanks::TankState;
use ohl_physics::Hull;

use super::Vec3;
use crate::test_support::entity_block;
use crate::{Game, Input, MemoryAssets, StartInventoryItem, TICK_SECONDS};

/// A solid origin-centered turret, an independently solid authored controls
/// brush (the classname must exclude it), and a stationary damageable actor.
/// No output counters are synthesized: the witness is a real delayed relay.
pub(crate) fn fixture(
    variant: &str,
    overrides: &[(&str, &str)],
    extra: &str,
) -> (Game, MemoryAssets) {
    fixture_with_controls(variant, overrides, extra, [-48.0, -48.0, 36.0], 36.0)
}

/// The shot-count oracle opts out of relay throttling so two same-tick
/// dispatches remain observable as two real pending delayed events.
pub(crate) fn fixture_with_unthrottled_output(
    variant: &str,
    overrides: &[(&str, &str)],
    extra: &str,
) -> (Game, MemoryAssets) {
    fixture_with_setup(variant, overrides, extra, [-48.0, -48.0, 36.0], 36.0, "0")
}

fn fixture_with_controls(
    variant: &str,
    overrides: &[(&str, &str)],
    extra: &str,
    controls_origin: [f32; 3],
    controls_half_height: f32,
) -> (Game, MemoryAssets) {
    fixture_with_setup(
        variant,
        overrides,
        extra,
        controls_origin,
        controls_half_height,
        "0.2",
    )
}

fn fixture_with_setup(
    variant: &str,
    overrides: &[(&str, &str)],
    extra: &str,
    controls_origin: [f32; 3],
    controls_half_height: f32,
    witness_wait: &str,
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
            controls_origin,
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
                ("delay", "5"),
                ("wait", witness_wait)
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
    let controls_min = [-12.0, -12.0, -controls_half_height];
    let controls_max = [12.0, 12.0, controls_half_height];
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
        assets.insert(path, stationary_target_model());
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

fn stationary_target_model() -> Vec<u8> {
    let (mut bytes, layout) =
        build_minimal_mdl10_with_hitbox([-24.0, -24.0, 0.0], [24.0, 24.0, 72.0]);
    // The format fixture normally animates its root X through10..20. Offset0
    // selects the builder's zero bind-pose channel instead, keeping this
    // turret target stationary at its independently specified world bounds.
    // General studio animation-cursor persistence is outside tag44's scope.
    bytes[layout.anim_data_offset..layout.anim_data_offset + 2]
        .copy_from_slice(&0u16.to_le_bytes());
    let model = ohl_world::StudioModel::parse(&bytes, &ohl_formats::mdl10::Limits::default())
        .expect("synthetic stationary model");
    let bind = ohl_world::StudioPose::bind(&model);
    assert_eq!(
        bind.matrices[0].map(f32::to_bits),
        glam::Mat4::IDENTITY.to_cols_array().map(f32::to_bits)
    );
    for time in [0.0, 0.02, 0.05, 0.2, 1.0] {
        assert_eq!(
            ohl_world::StudioPose::sample(&model, 0, time).unwrap(),
            bind
        );
    }
    bytes
}

fn named(game: &Game, name: &str) -> Entity {
    game.registry().find(name)[0]
}

pub(crate) fn health(game: &Game) -> f32 {
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

pub(crate) fn witness_count(game: &Game) -> usize {
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
        let (mut game, _) = fixture_with_unthrottled_output(variant, &[], "");
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
    let door_definition = entity_block(
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
        let (mut blocked, _) = fixture(variant, &[], &door_definition);
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
fn denied_direct_controls_use_cannot_become_remote_and_keeps_handheld_attack() {
    for missing_bounds in [false, true] {
        let (mut game, _) = if missing_bounds {
            fixture("func_tank", &[], "")
        } else {
            fixture_with_controls("func_tank", &[], "", [-48.0, -48.0, 124.0], 1.0)
        };
        equip(&mut game, WeaponId::Glock);
        game.tick(
            TICK_SECONDS,
            &Input {
                reload: true,
                ..Input::default()
            },
        );
        advance(&mut game, 160, false);
        assert_eq!(game.inventory().clip(WeaponId::Glock), 17);
        let controls = named(&game, "controls");
        if missing_bounds {
            game.level_and_systems_mut()
                .0
                .registry
                .world
                .remove_one::<ohl_game::registry::BrushBounds>(controls)
                .unwrap();
        } else {
            let bounds = game
                .registry()
                .world
                .get::<&ohl_game::registry::BrushBounds>(controls)
                .unwrap();
            let center = (bounds.mins + bounds.maxs) * 0.5;
            let eye = Vec3::from_array(game.camera().position);
            assert!((center.distance(eye) - 60.0).abs() < 0.1);
            let player = Vec3::from_array(game.player_origin());
            assert!(
                player.distance(player.clamp(bounds.mins, bounds.maxs)) > super::CONTROL_MARGIN
            );
        }
        assert_eq!(
            ohl_game::find_usable_within(
                game.registry(),
                Vec3::from_array(game.camera().position),
                crate::USE_RADIUS
            ),
            Some(controls)
        );
        let before = game.weapon_fired_count();
        let ammo = game.inventory_totals().1;
        tick(&mut game, true, true);
        assert!(game.systems_mut().tanks.mounted().is_none());
        assert!(game.to_save(0).tanks.unwrap().pending_use.is_none());
        assert_eq!(game.weapon_fired_count(), before + 1);
        assert_eq!(game.inventory_totals().1, ammo - 1);
        tick(&mut game, false, false);
        let state = game.to_save(0).tanks.unwrap();
        assert!(
            state.mounted.is_none()
                && state.pending_use.is_none()
                && state.pending_remote.is_none()
        );
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
    let equipped_ammo = game.inventory_totals();
    let equipped_fired = game.weapon_fired_count();
    advance(&mut game, 15, true);
    let ammo = game.inventory_totals();
    let fired = game.weapon_fired_count();
    assert!(fired > equipped_fired);
    assert!(ammo.1 < equipped_ammo.1);
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        4
    );
    tick(&mut game, true, true);
    assert!(game.systems_mut().tanks.mounted().is_some());
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        0
    );
    assert_eq!(game.inventory_totals(), ammo);
    assert_eq!(game.weapon_fired_count(), fired);
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

#[test]
fn real_mounted_reload_finishes_passively_and_conserves_ammo_until_owned_release() {
    let (mut game, _) = fixture("func_tank", &[], "");
    equip(&mut game, WeaponId::Glock);
    game.tick(
        TICK_SECONDS,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    advance(&mut game, 160, false);
    assert_eq!(game.inventory().clip(WeaponId::Glock), 17);
    let before_shot = game.weapon_fired_count();
    tick(&mut game, true, false);
    assert_eq!(game.weapon_fired_count(), before_shot + 1);
    assert_eq!(game.inventory().clip(WeaponId::Glock), 16);
    // Cross the 0.3-second firing cooldown before requesting reload: the
    // existing firing-to-idle transition consumes its input tick.
    advance(&mut game, 40, false);
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        0
    );
    game.give_start_inventory(&[StartInventoryItem::Ammo(AmmoType::NineMillimeter)]);
    let reserve = game.inventory().ammo(AmmoType::NineMillimeter).current();
    assert!(reserve > 0);
    let total = game.inventory_totals();
    let fired = game.weapon_fired_count();
    game.tick(
        TICK_SECONDS,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        2
    );
    tick(&mut game, false, true);
    assert!(game.systems_mut().tanks.mounted().is_some());
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        2
    );
    assert_eq!(game.inventory().clip(WeaponId::Glock), 16);
    advance(&mut game, 160, false);
    assert_eq!(
        game.to_save(0).inventory.unwrap().firing.unwrap().state_tag,
        0
    );
    assert_eq!(game.inventory().clip(WeaponId::Glock), 17);
    assert_eq!(
        game.inventory().ammo(AmmoType::NineMillimeter).current(),
        reserve - 1
    );
    assert_eq!(game.inventory_totals(), total);
    assert_eq!(game.weapon_fired_count(), fired);
    tick(&mut game, true, true);
    assert!(game.systems_mut().tanks.mounted().is_none());
    assert_eq!(game.inventory_totals(), total);
    assert_eq!(game.weapon_fired_count(), fired);
    tick(&mut game, true, false);
    assert_eq!(game.weapon_fired_count(), fired + 1);
    assert_eq!(game.inventory_totals().1, total.1 - 1);
    assert_eq!(game.inventory().clip(WeaponId::Glock), 16);
}

#[test]
fn actual_mortar_miss_has_no_blast_and_horizontal_hit_damages_once() {
    for upward in [true, false] {
        let (mut game, _) = fixture_with_unthrottled_output("func_tankmortar", &[], "");
        let origin = game.player_origin();
        game.set_viewpoint(origin, if upward { -60.0 } else { 0.0 }, 0.0);
        tick(&mut game, false, true);
        advance(&mut game, 12, false);
        assert!(game.systems_mut().tanks.mounted().is_some());
        let tank = named(&game, "tank");
        let pose = ohl_game::tanks::tank_pose(game.registry(), tank).unwrap();
        let direction = if upward {
            Vec3::new(0.5, 0.0, 0.866_025_4)
        } else {
            Vec3::X
        };
        assert!(pose.forward().abs_diff_eq(direction, 0.001));
        assert_eq!(witness_count(&game), 0);
        let player_health = game.player_health();
        tick(&mut game, true, false);
        assert_eq!(
            game.projectile_count(),
            0,
            "mortar is an instant trace, not a rocket"
        );
        assert_eq!(witness_count(&game), 1);
        let blasts = game
            .systems_mut()
            .map_effects
            .presentation()
            .blasts
            .to_vec();
        if upward {
            assert!(blasts.is_empty(), "a miss cannot fabricate an origin blast");
            assert!((health(&game) - 1000.0).abs() < 0.001);
            assert!((game.player_health() - player_health).abs() < 0.001);
        } else {
            assert_eq!(blasts.len(), 1);
            assert!(
                blasts[0]
                    .origin
                    .abs_diff_eq(Vec3::new(136.0, 0.0, 64.0), 0.001)
            );
            assert!((blasts[0].radius - 200.0).abs() < 0.001);
            assert!(
                (health(&game) - 900.0).abs() < 0.001,
                "one 100-damage contact blast"
            );
        }
        advance(&mut game, 10, false);
        assert!((health(&game) - if upward { 1000.0 } else { 900.0 }).abs() < 0.001);
        assert_eq!(witness_count(&game), 1);
        assert_eq!(game.projectile_count(), 0);
    }
}

#[test]
fn still_turning_input_updates_both_collision_models_before_next_frame_catchup() {
    let (mut game, _) = fixture("func_tank", &[], "");
    let origin = game.player_origin();
    game.set_viewpoint(origin, 0.0, 90.0);
    tick(&mut game, false, true);
    advance(&mut game, 8, false);
    let tank = named(&game, "tank");
    let yaw = game
        .registry()
        .world
        .get::<&TankState>(tank)
        .unwrap()
        .relative_yaw;
    assert!(
        (yaw - 64.8).abs() < 0.001,
        "9 steps at 720 degrees/s, still turning"
    );
    let (level, _) = game.level_and_systems_mut();
    for collision in [
        level.collision.as_ref().unwrap(),
        level.monster_collision.as_ref().unwrap(),
    ] {
        // Independent rotated-box contact: local y=4 at world y16 gives
        // x=(16*cos64.8-4)/sin64.8, approximately3.11. The small interval
        // accommodates the existing collision-plane clearance. The previous
        // 57.6-degree pose enters near5.42 and cannot pass this assertion.
        let contact = collision.trace(
            Hull::Point,
            Vec3::new(-32.0, 16.0, 64.0),
            Vec3::new(32.0, 16.0, 64.0),
        );
        assert!(
            contact.fraction > 0.545 && contact.fraction < 0.552,
            "current 64.8-degree contact required"
        );
        let clear = collision.trace(
            Hull::Point,
            Vec3::new(-32.0, 28.0, 64.0),
            Vec3::new(32.0, 28.0, 64.0),
        );
        assert!(
            clear.fraction > 0.999,
            "ray beyond rotated extent stays clear"
        );
    }
}
