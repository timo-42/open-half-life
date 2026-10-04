//! Real-input launch followed by inspection at the existing phase-7 seam.
//! No fabricated projectile, damage counter or alternative terminal dispatcher.

use super::*;
use crate::ids::entity_id;
use crate::tanks::live_tests::{fixture, tick};
use crate::tanks::save::{TankEntityRef, TanksSnapshot};

fn terminal_hits(game: &mut crate::Game) -> Vec<QueuedDamage> {
    let (level, systems) = game.level_and_systems_mut();
    let mut damage = Vec::new();
    systems.projectiles.tick(
        level,
        &systems.hitboxes,
        0.2,
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
