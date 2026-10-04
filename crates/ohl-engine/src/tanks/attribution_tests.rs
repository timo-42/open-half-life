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

#[test]
fn actual_input_rocket_terminal_keeps_operator_credit_source_inflictor_and_owner_splash() {
    let (mut live, assets) = fixture("func_tankrocket", &[("bullet_damage", "100")], "");
    tick(&mut live, true, true);
    assert_eq!(live.projectile_count(), 1);
    let mut loaded = crate::Game::load_bytes(&assets, &live.save_bytes(0).unwrap()).unwrap();
    // Both branches advance a normal frame to refresh their posed hitbox data;
    // the live control was never created through from_save.
    for game in [&mut live, &mut loaded] {
        tick(game, false, false);
        let source = game.registry().find("tank")[0];
        let victim = game.registry().find("victim")[0];
        let operator = game.player_entity();
        let hits = terminal_hits(game);
        for target in [operator, victim] {
            let hit = hits
                .iter()
                .find(|hit| hit.target == target)
                .expect("positive target damage");
            assert!(hit.info.amount > 0.0);
            assert_eq!(hit.info.attacker, Some(entity_id(operator)));
            assert_eq!(hit.info.inflictor, Some(entity_id(source)));
        }
        assert_eq!(game.projectile_count(), 0);
        assert!(game.to_save(0).tanks.unwrap().projectiles.is_empty());
    }
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
