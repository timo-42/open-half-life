//! Real-input launch followed by inspection at the existing phase-7 seam.
//! Capacity preparation uses the existing spawner; turret shots use real input.
//! No fake damage counter or alternative terminal dispatcher.

use super::*;
use crate::ids::entity_id;
use crate::tanks::live_tests::{
    fixture, fixture_with_unthrottled_output, health, tick, witness_count,
};
use crate::tanks::save::{TankEntityRef, TanksSnapshot};

fn terminal_hits(game: &mut crate::Game) -> Vec<QueuedDamage> {
    projectile_hits(game, 0.2)
}

fn projectile_hits(game: &mut crate::Game, dt: f32) -> Vec<QueuedDamage> {
    let (level, systems) = game.level_and_systems_mut();
    let mut damage = Vec::new();
    systems.projectiles.tick(
        level,
        &systems.hitboxes,
        dt,
        &mut damage,
        &mut systems.transient_sprites,
    );
    damage
}

fn assert_terminal_payloads(
    hits: &[QueuedDamage],
    operator: Entity,
    victim: Entity,
    source: Entity,
) {
    assert_eq!(
        hits.len(),
        2,
        "one blast contributes exactly one hit per target"
    );
    // Independent fixture geometry: the target starts at x160 with a24-unit
    // half-width. Existing projectile clearance puts its blast one unit before
    // that face. The player's nearest hull point is(-32,-32,64).
    let origin = Vec3::new(135.0, 0.0, 64.0);
    for (target, point) in [
        (operator, Vec3::new(-32.0, -32.0, 64.0)),
        (victim, Vec3::new(136.0, 0.0, 64.0)),
    ] {
        let matching = hits
            .iter()
            .filter(|hit| hit.target == target)
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "no duplicate impact plus detonation damage"
        );
        let info = matching[0].info;
        let expected_amount = 100.0 * (1.0 - point.distance(origin) / 250.0);
        assert!(info.amount > 0.0);
        assert!(
            (info.amount - expected_amount).abs() < 0.001,
            "expected amount {expected_amount}, complete actual {info:?}, operator={}",
            target == operator
        );
        assert_eq!(info.kind, ohl_combat::DamageType::BLAST);
        assert_eq!(info.attacker, Some(entity_id(operator)));
        assert_eq!(info.inflictor, Some(entity_id(source)));
        assert!(info.origin.abs_diff_eq(origin, 0.001));
        assert!(
            info.direction
                .abs_diff_eq((point - origin).normalize(), 0.001)
        );
    }
}

#[test]
fn actual_input_rocket_terminal_keeps_operator_credit_source_inflictor_and_owner_splash() {
    let (mut live, assets) = fixture("func_tankrocket", &[("bullet_damage", "100")], "");
    tick(&mut live, true, true);
    assert_eq!(live.projectile_count(), 1);
    let mut loaded = crate::Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    let mut branch_payloads = Vec::new();
    // Both branches advance a normal frame to refresh their posed hitbox data;
    // the live control was never created through from_save.
    for game in [&mut live, &mut loaded] {
        tick(game, false, false);
        let source = game.registry().find("tank")[0];
        let victim = game.registry().find("victim")[0];
        let operator = game.player_entity();
        let before = game.to_save(0);
        let mapping = &before.tanks.as_ref().unwrap().projectiles;
        assert_eq!(mapping.len(), 1);
        let source_index = u32::try_from(
            game.registry()
                .entities
                .iter()
                .position(|entity| *entity == source)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(mapping[0].source, source_index);
        assert_eq!(mapping[0].attacker, TankEntityRef::Player);
        let physical = &before.projectiles.as_ref().unwrap().projectiles;
        assert_eq!(physical.len(), 1);
        assert_eq!(physical[0].id, mapping[0].projectile);
        assert_eq!(physical[0].owner, Some(source_index));
        let profiles = &before.projectile_runtime.as_ref().unwrap().attacks;
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, physical[0].id);
        assert_eq!(
            profiles[0].owner,
            Some(crate::save_state::ProjectileEntityRef::Registry(
                source_index
            ))
        );
        let hits = terminal_hits(game);
        assert_terminal_payloads(&hits, operator, victim, source);
        branch_payloads.push(hits);
        assert_eq!(game.projectile_count(), 0);
        let after = game.to_save(0);
        assert!(after.tanks.unwrap().projectiles.is_empty());
        assert!(after.projectiles.unwrap().projectiles.is_empty());
        assert!(
            after
                .projectile_runtime
                .is_none_or(|runtime| runtime.attacks.is_empty())
        );
    }
    assert_eq!(
        branch_payloads[0], branch_payloads[1],
        "every terminal field survives restore exactly"
    );
}

#[test]
fn attribution_restore_remaps_explicit_player_identity_without_guessing_registry_zero() {
    let (mut game, _) = fixture("func_tankrocket", &[], "");
    tick(&mut game, true, true);
    let snapshot = game.to_save(0).tanks.unwrap();
    let (level, systems) = game.level_and_systems_mut();
    let previous_player = level.player;
    // Synthetic handle perturbation isolates remapping from deterministic fresh
    // allocation order. Physical source and its restored projectile stay real.
    level.player = level.registry.world.spawn(());
    assert_ne!(level.player, previous_player);
    let input = crate::tanks::ControlInput {
        player: level.player,
        position: Vec3::new(-48.0, -48.0, 36.0),
        view_direction: Vec3::X,
        alive: true,
        use_pressed: false,
        attack: false,
    };
    assert!(
        systems
            .tanks
            .restore(level, &mut systems.projectiles, Some(&snapshot), input)
    );
    assert_eq!(systems.tanks.mounted().unwrap().player, level.player);
    let entries = systems.projectiles.tank_attributions().collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1.attacker, level.player);
    assert_ne!(entries[0].1.attacker, previous_player);
    let mut foreign = snapshot;
    foreign.projectiles[0].attacker = TankEntityRef::Registry(0);
    let source_index = foreign.states[0].tank;
    systems
        .tanks
        .restore(level, &mut systems.projectiles, Some(&foreign), input);
    let found: TanksSnapshot = systems.snapshot_tanks(level).unwrap();
    assert_eq!(found.projectiles[0].source, source_index);
    assert_eq!(found.projectiles[0].attacker, TankEntityRef::Registry(0));
}

#[test]
fn full_projectile_pool_denies_real_shot_without_output_or_ghost_credit() {
    let (mut full, _) =
        fixture_with_unthrottled_output("func_tankrocket", &[("bullet_damage", "100")], "");
    {
        let (level, systems) = full.level_and_systems_mut();
        let request = crate::ai::ProjectileRequest {
            kind: ohl_combat::ProjectileKind::Rocket,
            owner: level.player,
            origin: Vec3::new(-256.0, 256.0, 128.0),
            velocity: Vec3::ZERO,
            damage: 0.0,
            damage_type: ohl_combat::DamageType::BLAST,
            blast_radius: Some(250.0),
            target: None,
        };
        for _ in 0..128 {
            assert!(systems.projectiles.spawn_request(level, &request).is_some());
        }
        assert!(systems.projectiles.spawn_request(level, &request).is_none());
        assert_eq!(systems.projectiles.count(), 128);
        assert_eq!(systems.projectiles.tank_attributions().count(), 0);
    }
    let ammo = full.inventory_totals();
    let player_health = full.player_health();
    tick(&mut full, true, true);
    assert!(full.systems_mut().tanks.mounted().is_some());
    assert_eq!(full.projectile_count(), 128);
    assert_eq!(witness_count(&full), 0);
    assert!(full.to_save(0).tanks.unwrap().projectiles.is_empty());
    assert_eq!(full.inventory_totals(), ammo);
    assert!((health(&full) - 1000.0).abs() < 0.001);
    assert!((full.player_health() - player_health).abs() < 0.001);

    let (mut available, _) =
        fixture_with_unthrottled_output("func_tankrocket", &[("bullet_damage", "100")], "");
    tick(&mut available, true, true);
    assert_eq!(available.projectile_count(), 1);
    assert_eq!(witness_count(&available), 1);
    let snapshot = available.to_save(0);
    let attribution = &snapshot.tanks.unwrap().projectiles;
    assert_eq!(attribution.len(), 1);
    assert_eq!(attribution[0].attacker, TankEntityRef::Player);
    let physical = snapshot.projectiles.unwrap().projectiles[0];
    assert_eq!(physical.id, attribution[0].projectile);
    assert_eq!(physical.owner, Some(attribution[0].source));
    for _ in 0..20 {
        tick(&mut available, false, false);
    }
    assert!((health(&available) - 900.4).abs() < 0.001);
    assert_eq!(available.projectile_count(), 0);
    assert_eq!(witness_count(&available), 1);
}

fn remap_loaded_source_brush(game: &mut crate::Game, assets: &crate::MemoryAssets) {
    use crate::assets::AssetSource;
    let bytes = assets.read("maps/ohl_tanks.bsp").unwrap();
    let limits = ohl_formats::bsp30::Limits::default();
    let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &limits).unwrap();
    let (level, _) = game.level_and_systems_mut();
    let source = level.registry.find("tank")[0];
    let old_source = level
        .brush_collision
        .iter()
        .find(|(entity, _)| *entity == source)
        .unwrap()
        .1;
    let old = std::mem::take(&mut level.brush_collision);
    let collision = level.collision.as_mut().unwrap();
    for (_, brush) in old {
        collision.detach_brush(brush);
    }
    // Attachment IDs are deliberately different from the fresh level order.
    // The unrelated first brush stays well away from both the shot and player.
    collision
        .attach_brush(&bsp, &limits, 1, Vec3::new(-256.0, -256.0, 64.0))
        .unwrap();
    if let Some(&door) = level.registry.find("blocker").first() {
        let brush = collision
            .attach_brush(&bsp, &limits, 3, Vec3::new(80.0, 0.0, 64.0))
            .unwrap();
        level.brush_collision.push((door, brush));
    }
    let current = collision
        .attach_brush(&bsp, &limits, 1, Vec3::new(0.0, 0.0, 64.0))
        .unwrap();
    assert_ne!(current, old_source);
    level.brush_collision.push((source, current));
    let own_body = collision.trace(
        ohl_physics::Hull::Point,
        Vec3::new(-32.0, 0.0, 64.0),
        Vec3::new(32.0, 0.0, 64.0),
    );
    assert!(
        own_body.fraction < 0.3,
        "remapped source is still physically solid"
    );
}

fn assert_remapped_flight(game: &mut crate::Game, blocked: bool) -> Vec<QueuedDamage> {
    let saved_mapping = game.to_save(0).tanks.unwrap().projectiles;
    assert_eq!(saved_mapping.len(), 1);
    assert_eq!(saved_mapping[0].attacker, TankEntityRef::Player);
    assert!(projectile_hits(game, 0.04).is_empty());
    let beyond_source = game.to_save(0);
    let physical = &beyond_source.projectiles.as_ref().unwrap().projectiles;
    assert_eq!(
        physical.len(),
        1,
        "rocket must leave the remapped own brush"
    );
    assert!((physical[0].position[0] - 60.0).abs() < 0.001);
    assert_eq!(beyond_source.tanks.unwrap().projectiles, saved_mapping);
    let at_door = projectile_hits(game, 0.04);
    let source = game.registry().find("tank")[0];
    let operator = game.player_entity();
    if blocked {
        assert_eq!(
            game.projectile_count(),
            0,
            "independent closed door still blocks"
        );
        assert_eq!(
            at_door.len(),
            1,
            "door occludes the victim but not owner splash"
        );
        let hit = at_door[0];
        // Authored door face x76, BSP clearance 1/32, then projectile's
        // independent one-unit outward detonation offset.
        let origin = Vec3::new(74.968_75, 0.0, 64.0);
        let point = Vec3::new(-32.0, -32.0, 64.0);
        let amount = 100.0 * (1.0 - point.distance(origin) / 250.0);
        assert_eq!(hit.target, operator);
        assert_eq!(hit.info.attacker, Some(entity_id(operator)));
        assert_eq!(hit.info.inflictor, Some(entity_id(source)));
        assert_eq!(hit.info.kind, ohl_combat::DamageType::BLAST);
        assert!(hit.info.origin.abs_diff_eq(origin, 0.001));
        assert!((hit.info.amount - amount).abs() < 0.001);
        assert!(
            hit.info
                .direction
                .abs_diff_eq((point - origin).normalize(), 0.001)
        );
        at_door
    } else {
        assert!(at_door.is_empty());
        assert_eq!(
            game.projectile_count(),
            1,
            "clear flight continues past the separate door plane"
        );
        let hits = projectile_hits(game, 0.1);
        assert_terminal_payloads(&hits, operator, game.registry().find("victim")[0], source);
        assert_eq!(game.projectile_count(), 0);
        hits
    }
}

#[test]
fn restored_rocket_uses_remapped_source_brush_and_keeps_independent_door_obstruction() {
    let door = crate::test_support::entity_block(
        "func_door",
        [80.0, 0.0, 64.0],
        90.0,
        &[
            ("model", "*3"),
            ("targetname", "blocker"),
            ("spawnflags", "256"),
        ],
    );
    for blocked in [false, true] {
        let (mut live, assets) = fixture(
            "func_tankrocket",
            &[("bullet_damage", "100")],
            if blocked { &door } else { "" },
        );
        tick(&mut live, true, true);
        let mut loaded = crate::Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
        for game in [&mut live, &mut loaded] {
            tick(game, false, false);
        }
        let before = loaded.to_save(0).tanks.unwrap();
        remap_loaded_source_brush(&mut loaded, &assets);
        assert_eq!(
            loaded.to_save(0).tanks.unwrap(),
            before,
            "stable save refs do not encode brush handles"
        );
        let actual_live = assert_remapped_flight(&mut live, blocked);
        let actual_loaded = assert_remapped_flight(&mut loaded, blocked);
        assert_eq!(
            actual_loaded, actual_live,
            "full terminal payload survives a real handle remap"
        );
    }
}
