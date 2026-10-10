//! Authored live-grenade guard controls; no game-media data or new save fields.

use ohl_ai::Actor;
use ohl_combat::{DamageType, WeaponId};
use ohl_engine::save_state::{
    FiringSnapshot, ProjectileAttackSnapshot, ProjectileEntityRef, ProjectileRuntimeSnapshot,
    ProjectileSnapshot,
};
use ohl_engine::test_support::{
    AI_MAP, ai_room_bsp, entity_block, entity_of_classname, queue_monster_damage,
};
use ohl_engine::{Game, Input, MemoryAssets, StartInventoryItem, TICK_SECONDS, guard_step};

fn scenario(position: [f32; 3], fuse: Option<f32>, kind: DamageType, radius: f32) -> Game {
    let text = format!(
        "{{\"classname\" \"worldspawn\"}}\n\
         {{\"classname\" \"info_player_start\" \"origin\" \"0 0 36\" \"angle\" \"0\"}}\n{}",
        entity_block(
            "monster_human_grunt",
            [128.0, 128.0, 0.0],
            180.0,
            &[("spawnflags", "16")]
        )
    );
    let bytes = ai_room_bsp(&text, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("authored room");
    game.give_start_inventory(&[StartInventoryItem::Weapon(WeaponId::Mp5)]);
    let owner = entity_of_classname(&game, "monster_human_grunt").expect("owner");
    let owner_index = u32::try_from(
        game.registry()
            .entities
            .iter()
            .position(|entity| *entity == owner)
            .expect("spawn index"),
    )
    .expect("small authored registry");
    let mut save = game.to_save(0);
    save.player.health = 1.0;
    save.player.armor = 0.0;
    let inventory = save.inventory.as_mut().expect("inventory");
    let mp5 = WeaponId::ALL
        .iter()
        .position(|id| *id == WeaponId::Mp5)
        .expect("MP5 index");
    inventory.weapons[mp5].clip = ohl_combat::spec(WeaponId::Mp5).clip_size.expect("MP5 clip");
    let selected = u8::try_from(mp5).expect("small weapon table");
    inventory.selected = Some(selected);
    inventory.firing = Some(FiringSnapshot {
        weapon: selected,
        state_tag: 0,
        timer: 0.0,
    });
    let physics = save.projectiles.as_mut().expect("physics snapshot");
    physics.projectile_next_id = 1;
    physics.projectiles = vec![ProjectileSnapshot {
        id: 0,
        kind_tag: 3,
        owner: Some(owner_index),
        position,
        velocity: [0.0; 3],
        age: 3.0,
        fuse,
        guide_point: None,
        target: None,
        attack_cooldown: 0.0,
        hop_cooldown: 0.0,
        resting: true,
    }];
    save.projectile_runtime = Some(ProjectileRuntimeSnapshot {
        attacks: vec![ProjectileAttackSnapshot {
            id: 0,
            damage: 100.0,
            damage_bits: kind.bits(),
            blast_radius: Some(radius),
            owner: Some(ProjectileEntityRef::Registry(owner_index)),
            target: None,
        }],
        ..ProjectileRuntimeSnapshot::default()
    });
    Game::from_save(&assets, &save).expect("authored armed grenade restored")
}

#[test]
fn a_dead_owners_live_grenade_requires_retreat_and_survival() {
    // Both an offset grenade and one directly underfoot have an open safe exit.
    for x in [32.0, 0.0] {
        let mut game = scenario([x, 0.0, 1.0], Some(2.0), DamageType::BLAST, 200.0);
        let owner = entity_of_classname(&game, "monster_human_grunt").expect("owner");
        let health = game
            .registry()
            .world
            .get::<&Actor>(owner)
            .expect("live owner")
            .health;
        queue_monster_damage(&mut game, owner, None, health);
        game.tick(TICK_SECONDS, &Input::default());
        assert!(
            !game
                .registry()
                .world
                .get::<&Actor>(owner)
                .expect("dead actor")
                .alive
        );
        assert!(
            game.hostile_monster_eyes().is_empty(),
            "no living target can trigger old retreat"
        );
        assert_eq!(
            game.projectile_count(),
            1,
            "owner death leaves the grenade live"
        );
        assert!(game.player_health() > 0.0, "setup is not already lethal");
        let mut retreated = false;
        for _ in 0..250 {
            let (input, decision) = guard_step(&game);
            retreated |= decision.retreating && (input.forward != 0 || input.right != 0);
            game.tick(TICK_SECONDS, &input);
        }
        assert!(
            game.player_health() > 0.0,
            "escape the dead owner's timed blast"
        );
        assert!(retreated, "a collision-checked retreat actually moved");
        assert_eq!(
            game.projectile_count(),
            0,
            "the original fuse still detonates"
        );
    }
}

#[test]
fn harmless_projectiles_preserve_the_existing_combat_input() {
    let baseline = scenario([32.0, 0.0, 1.0], None, DamageType::BULLET, 200.0);
    let expected = guard_step(&baseline);
    assert!(!expected.1.retreating);
    for (position, fuse, kind) in [
        ([32.0, 0.0, 1.0], Some(1.0), DamageType::BULLET),
        ([32.0, 0.0, 1.0], None, DamageType::BLAST),
        ([32.0, 0.0, 1.0], Some(4.0), DamageType::BLAST),
        ([-240.0, 0.0, 1.0], Some(1.0), DamageType::BLAST),
    ] {
        assert_eq!(guard_step(&scenario(position, fuse, kind, 200.0)), expected);
    }
}

#[test]
fn exposure_uses_the_body_and_includes_an_already_due_fuse() {
    for fuse in [1.0, 0.0] {
        let game = scenario([24.0, 0.0, 1.0], Some(fuse), DamageType::BLAST, 20.0);
        let eye = ohl_ai::Vec3::from_array(game.eye_position());
        assert!(eye.distance(ohl_ai::Vec3::new(24.0, 0.0, 1.0)) > 20.0);
        assert!(
            guard_step(&game).1.retreating,
            "current body is within the effective radius"
        );
    }
}
