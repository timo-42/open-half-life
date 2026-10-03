//! Additive tag 42 overlays immutable tag 26 physics; all fixtures are synthetic.
#![allow(clippy::float_cmp)]
use ohl_combat::{DamageType, ProjectileKind};
use ohl_engine::save_state::{
    DeployableOwnerSnapshot, ProjectileAttackSnapshot, ProjectileEntityRef,
    ProjectileRuntimeSnapshot,
};
use ohl_engine::test_support::{AI_MAP, ai_room_bsp};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};

fn fixture() -> (Game, MemoryAssets) {
    let text = "{\"classname\" \"worldspawn\"}\n{\"classname\" \"info_player_start\" \"origin\" \"-128 0 36\"}\n{\"classname\" \"monster_human_grunt\" \"origin\" \"128 128 36\" \"spawnflags\" \"16\"}\n";
    let bytes = ai_room_bsp(text, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    (
        Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("synthetic room"),
        assets,
    )
}

#[test]
fn all_physical_kinds_and_distinct_profiles_round_trip_without_changing_tag_26() {
    let (mut game, assets) = fixture();
    for kind in [
        ProjectileKind::CrossbowBolt,
        ProjectileKind::Rocket,
        ProjectileKind::Mp5Grenade,
        ProjectileKind::HandGrenade,
        ProjectileKind::Hornet,
        ProjectileKind::Snark,
        ProjectileKind::BullsquidSpit,
        ProjectileKind::ControllerBall,
        ProjectileKind::ControllerHomingBall,
        ProjectileKind::GonarchMortar,
        ProjectileKind::Rocket,
        ProjectileKind::Hornet,
    ] {
        game.debug_spawn_projectile(kind, [0.0, 0.0, 100.0], [10.0, 0.0, 0.0])
            .expect("capacity");
    }
    game.debug_place_satchel([0.0, 32.0, 36.0])
        .expect("satchel");
    game.debug_place_tripmine([220.0, 0.0, 64.0], [1.0, 0.0, 0.0])
        .expect("mine");
    let mut save = game.to_save(0);
    let runtime = save.projectile_runtime.as_mut().expect("metadata");
    for (index, attack) in runtime.attacks.iter_mut().enumerate() {
        attack.damage = [
            50.0, 100.0, 100.0, 100.0, 7.0, 10.0, 15.0, 5.0, 35.0, 160.0, 150.0, 8.0,
        ][index];
        attack.owner = Some(if index == 8 {
            ProjectileEntityRef::Registry(2)
        } else {
            ProjectileEntityRef::Player
        });
        attack.target = (index == 8).then_some(ProjectileEntityRef::Player);
    }
    for owner in &mut runtime.deployable_owners {
        owner.owner = Some(ProjectileEntityRef::Player);
    }
    runtime.secondary_cooldowns = vec![(2, 4.25)];
    runtime.player_controls.primary_held = true;
    save.projectiles.as_mut().expect("physics").projectiles[8].owner = Some(2);
    let physics =
        postcard::to_allocvec(save.projectiles.as_ref().expect("physics")).expect("encode physics");
    let expected = runtime.clone();
    let bytes = save.to_bytes().expect("encode save");
    let restored = Game::load_bytes(&assets, &bytes).expect("decode save");
    let after = restored.to_save(0);
    assert_eq!(after.projectile_runtime, Some(expected));
    assert_eq!(
        postcard::to_allocvec(after.projectiles.as_ref().expect("physics")).expect("encode"),
        physics
    );
    assert_eq!(restored.deployable_count(), 2);
    assert_eq!(restored.deployable_stand_in_count(), 2);
    let again = Game::load_bytes(&assets, &restored.save_bytes(0).expect("save again"))
        .expect("restore again");
    assert_eq!(
        again.deployable_stand_in_count(),
        2,
        "restore does not accumulate stand-ins"
    );
}

#[test]
fn an_old_save_without_tag_42_never_guesses_player_ownership() {
    let (mut game, assets) = fixture();
    game.debug_spawn_projectile(ProjectileKind::Rocket, [0.0, 0.0, 100.0], [10.0, 0.0, 0.0])
        .expect("rocket");
    let mut save = game.to_save(0);
    save.projectile_runtime = None;
    let restored =
        Game::load_bytes(&assets, &save.to_bytes().expect("old encoding")).expect("old save");
    let after = restored.to_save(0);
    let attack = &after
        .projectile_runtime
        .expect("legacy profile captured")
        .attacks[0];
    assert_eq!(attack.owner, None);
    assert_eq!(
        attack.damage, 120.0,
        "documented legacy fallback, not current player's profile"
    );
}

#[test]
fn malformed_profiles_duplicate_ids_and_orphans_cannot_create_or_strengthen_projectiles() {
    let (mut game, assets) = fixture();
    game.debug_spawn_projectile(ProjectileKind::Rocket, [0.0, 0.0, 100.0], [10.0, 0.0, 0.0])
        .expect("rocket");
    let mut save = game.to_save(0);
    let runtime = save.projectile_runtime.as_mut().expect("runtime");
    runtime.attacks[0].damage = f32::NAN;
    runtime.attacks[0].owner = Some(ProjectileEntityRef::Registry(u32::MAX));
    runtime.attacks.push(ProjectileAttackSnapshot {
        id: 0,
        damage: 9999.0,
        damage_bits: DamageType::BLAST.bits(),
        blast_radius: Some(250.0),
        owner: Some(ProjectileEntityRef::Player),
        target: None,
    });
    runtime.attacks.push(ProjectileAttackSnapshot {
        id: 999,
        damage: 9999.0,
        damage_bits: 0,
        blast_radius: None,
        owner: None,
        target: None,
    });
    runtime.deployable_owners.push(DeployableOwnerSnapshot {
        id: 777,
        owner: Some(ProjectileEntityRef::Player),
    });
    let restored = Game::from_save(&assets, &save).expect("sanitized restore");
    assert_eq!(restored.projectile_count(), 1);
    let after = restored.to_save(0).projectile_runtime.expect("metadata");
    assert_eq!(after.attacks.len(), 1);
    assert_eq!(after.attacks[0].damage, 0.0);
    assert_eq!(after.attacks[0].owner, None);
    assert!(after.deployable_owners.is_empty());
}

#[test]
fn tag_42_rejects_oversized_sequence_before_restoring() {
    let profile = ProjectileAttackSnapshot {
        id: 0,
        damage: 1.0,
        damage_bits: 0,
        blast_radius: None,
        owner: None,
        target: None,
    };
    let too_many = ProjectileRuntimeSnapshot {
        attacks: vec![profile; 129],
        ..Default::default()
    };
    let bytes = postcard::to_allocvec(&too_many).expect("malicious fixture");
    assert!(postcard::from_bytes::<ProjectileRuntimeSnapshot>(&bytes).is_err());
}

#[test]
fn fuse_guidance_and_secondary_cooldown_continuation_matches_across_save_load() {
    let (mut setup, assets) = fixture();
    for (kind, origin, velocity) in [
        (
            ProjectileKind::HandGrenade,
            [0.0, 64.0, 100.0],
            [50.0, 0.0, 100.0],
        ),
        (
            ProjectileKind::Rocket,
            [0.0, -64.0, 100.0],
            [10.0, 0.0, 0.0],
        ),
        (
            ProjectileKind::ControllerHomingBall,
            [0.0, 0.0, 100.0],
            [10.0, 0.0, 0.0],
        ),
    ] {
        setup
            .debug_spawn_projectile(kind, origin, velocity)
            .expect("fixture projectile");
    }
    // Inject initial synthetic control values, then let a live game age them before capture.
    let mut initial = setup.to_save(0);
    let runtime = initial.projectile_runtime.as_mut().expect("metadata");
    runtime.attacks[1].owner = Some(ProjectileEntityRef::Player);
    runtime.attacks[2].target = Some(ProjectileEntityRef::Player);
    runtime.secondary_cooldowns = vec![(2, 4.0)];
    let mut uninterrupted = Game::from_save(&assets, &initial).expect("initial fixture");
    for _ in 0..125 {
        uninterrupted.tick(TICK_SECONDS, &Input::default());
    }
    let checkpoint = uninterrupted.to_save(0);
    let physics = checkpoint.projectiles.as_ref().expect("physics");
    assert!(
        physics.projectiles[0]
            .fuse
            .is_some_and(|fuse| fuse > 3.0 && fuse < 4.0)
    );
    assert!(
        physics.projectiles[1].guide_point.is_some(),
        "live aim drove rocket guidance"
    );
    let metadata = checkpoint.projectile_runtime.as_ref().expect("metadata");
    assert_eq!(
        metadata.attacks[2].target,
        Some(ProjectileEntityRef::Player)
    );
    assert!(metadata.secondary_cooldowns[0].1 > 2.0 && metadata.secondary_cooldowns[0].1 < 3.0);
    let mut restored = Game::load_bytes(
        &assets,
        &uninterrupted.save_bytes(0).expect("capture live game"),
    )
    .expect("restored branch");
    assert_eq!(
        uninterrupted.to_save(0).projectiles,
        restored.to_save(0).projectiles
    );
    assert_eq!(
        uninterrupted.to_save(0).projectile_runtime,
        restored.to_save(0).projectile_runtime
    );
    for _ in 0..420 {
        uninterrupted.tick(TICK_SECONDS, &Input::default());
        restored.tick(TICK_SECONDS, &Input::default());
        assert_eq!(
            uninterrupted.to_save(0).projectiles,
            restored.to_save(0).projectiles
        );
        assert_eq!(
            uninterrupted.to_save(0).projectile_runtime,
            restored.to_save(0).projectile_runtime
        );
        assert_eq!(uninterrupted.player_health(), restored.player_health());
    }
}

#[test]
fn duplicate_physical_ids_are_filtered_and_restored_counters_skip_live_handles() {
    let (mut game, assets) = fixture();
    game.debug_spawn_projectile(ProjectileKind::Rocket, [0.0, 0.0, 100.0], [10.0, 0.0, 0.0])
        .expect("rocket");
    game.debug_place_satchel([0.0, 32.0, 36.0])
        .expect("satchel");
    game.debug_place_tripmine([220.0, 0.0, 64.0], [1.0, 0.0, 0.0])
        .expect("mine");
    let mut save = game.to_save(0);
    let physics = save.projectiles.as_mut().expect("physics");
    physics.projectiles.push(physics.projectiles[0]);
    physics.satchels.push(physics.satchels[0]);
    physics.tripmines[0].id = physics.satchels[0].id;
    physics.projectile_next_id = physics.projectiles[0].id;
    physics.deployable_next_id = physics.satchels[0].id;
    let mut restored = Game::from_save(&assets, &save).expect("sanitize duplicate physics");
    assert_eq!(
        restored
            .to_save(0)
            .projectiles
            .expect("physics")
            .projectiles
            .len(),
        1
    );
    assert_eq!(restored.deployable_count(), 1);
    assert_eq!(restored.deployable_stand_in_count(), 1);
    restored
        .debug_spawn_projectile(ProjectileKind::Rocket, [0.0, 0.0, 100.0], [10.0, 0.0, 0.0])
        .expect("next projectile");
    restored
        .debug_place_satchel([0.0, 64.0, 36.0])
        .expect("next satchel");
    let next = restored.to_save(0).projectiles.expect("physics");
    assert_ne!(next.projectiles[0].id, next.projectiles[1].id);
    assert_ne!(next.satchels[0].id, next.satchels[1].id);
}
