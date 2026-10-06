//! Synthetic end-to-end projectile input, AI delivery and continuation checks.
#![allow(clippy::float_cmp)]

use ohl_ai::{Actor, MonsterKind};
use ohl_combat::{AmmoType, WeaponId, hud_slot};
use ohl_engine::save_state::{FiringSnapshot, ProjectileEntityRef, WeaponSnapshot};
use ohl_engine::test_support::{
    AI_MAP, ai_room_bsp, entity_block, entity_of_classname, plan_scripted_monster_model_bytes,
};
use ohl_engine::{Game, Input, MemoryAssets, TICK_SECONDS};

fn fixture(classname: &str, prisoner: bool) -> (Game, MemoryAssets) {
    let mut text = String::from(
        "{\"classname\" \"worldspawn\"}\n{\"classname\" \"info_player_start\" \"origin\" \"0 0 36\" \"angle\" \"0\"}\n",
    );
    if !classname.is_empty() {
        text.push_str(&entity_block(
            classname,
            [128.0, 0.0, 36.0],
            180.0,
            &[("spawnflags", if prisoner { "16" } else { "0" })],
        ));
    }
    let bytes = ai_room_bsp(&text, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    if let Some(path) = MonsterKind::from_classname(classname).default_model_path() {
        assets.insert(path, plan_scripted_monster_model_bytes());
    }
    (
        Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("synthetic room"),
        assets,
    )
}

/// Human throws must leave their living owner outside the player's blast region.
/// Keep the close shared fixture for unrelated projectile tests.
fn safe_human_grenade_fixture(classname: &str) -> (Game, MemoryAssets) {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};

    let text = format!(
        "{{\"classname\" \"worldspawn\"}}{}{}",
        entity_block("info_player_start", [800.0, 0.0, 36.0], 180.0, &[]),
        entity_block(classname, [0.0; 3], 0.0, &[])
    );
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&text);
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -64.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -960.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -160.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -160.0),
    ]);
    builder.push_model(
        [-64.0, -160.0, 0.0],
        [960.0, 160.0, 512.0],
        [0.0; 3],
        heads,
        1,
        0,
        0,
    );
    let bytes = builder.build();
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    assets.insert(
        MonsterKind::from_classname(classname)
            .default_model_path()
            .expect("human model"),
        plan_scripted_monster_model_bytes(),
    );
    let game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("safe human grenade room");
    assert_safe_human_grenade_geometry(&game, classname);
    (game, assets)
}

/// Independent full-pose separation: any radius-200 blast hurting this player
/// is outside the radius-200 region of the modeled owner at launch.
fn assert_safe_human_grenade_geometry(game: &Game, classname: &str) {
    use ohl_ai::Vec3;
    use ohl_game::registry::Transform;
    use ohl_world::{StudioModel, StudioPose};

    let entity = entity_of_classname(game, classname).expect("human actor");
    let owner = *game.registry().world.get::<&Actor>(entity).expect("owner");
    let player = *game
        .registry()
        .world
        .get::<&Actor>(game.player_entity())
        .expect("player");
    let transform = *game
        .registry()
        .world
        .get::<&Transform>(entity)
        .expect("transform");
    assert!(owner.alive && player.alive && owner.health > 0.0 && player.health > 0.0);
    assert_eq!(owner.origin, transform.origin);
    assert_eq!(transform.angles, Vec3::ZERO);
    let world = game.collision().expect("actual world");
    for actor in [owner, player] {
        let clear = world.trace(actor.hull, actor.query_origin(), actor.query_origin());
        assert!(!clear.start_solid && !clear.all_solid, "actual hull fits");
        let floor = world.trace(
            actor.hull,
            actor.query_origin(),
            actor.query_origin() - Vec3::Z,
        );
        assert!(
            !floor.start_solid && floor.fraction < 1.0,
            "floor supported"
        );
    }
    let sight = world.trace(ohl_physics::Hull::Point, owner.eye(), player.eye());
    assert!(!sight.start_solid && !sight.all_solid && sight.fraction == 1.0);
    assert!((96.0..=1024.0).contains(&owner.eye().distance(player.eye())));
    let model = StudioModel::parse(
        &plan_scripted_monster_model_bytes(),
        &ohl_formats::mdl10::Limits::default(),
    )
    .expect("same authored model");
    assert_eq!(model.hitboxes.len(), 1);
    assert_eq!(model.hitboxes[0].bone, 0);
    assert_eq!(model.bones[0].parent, None);
    assert_eq!(model.bones[0].value, [0.0; 6]);
    assert_eq!(model.sequences.len(), 1);
    assert_eq!(model.sequences[0].frame_count, 2);
    assert_eq!(model.sequences[0].fps, 10.0);
    let (player_min, _) = player.body_frame.world_bounds(player.hull, player.origin);
    // The authored root translates linearly on X only between these endpoints.
    for (time, x) in [(0.0, 10.0), (0.1, 20.0)] {
        let pose = StudioPose::sample(&model, 0, time).expect("authored pose");
        let (min, max) = pose.hitbox_bounds(&model.hitboxes[0]).expect("posed owner");
        assert_eq!(min, [x - 24.0, -24.0, 0.0]);
        assert_eq!(max, [x + 24.0, 24.0, 72.0]);
        assert!(
            player_min.x - (owner.origin.x + max[0]) > 400.0,
            "positive player BLAST and live-owner exposure regions do not overlap"
        );
    }
}

/// Only the timed-grenade test needs guaranteed exposure after a real bounce.
fn grenade_room() -> (Game, MemoryAssets) {
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};

    let mut builder = Bsp30Builder::new();
    let text = format!(
        "{{\"classname\" \"worldspawn\"}}\n\
         {{\"classname\" \"info_player_start\" \"origin\" \"-40 0 36\" \"angle\" \"0\"}}\n{}",
        entity_block(
            "monster_gargantua",
            [32.0, 0.0, 36.0],
            180.0,
            &[("spawnflags", "16")]
        )
    );
    builder.set_entities_text(&text);
    let heads = builder.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -128.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -80.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -80.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -80.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -80.0),
    ]);
    builder.push_model(
        [-80.0, -80.0, 0.0],
        [80.0, 80.0, 128.0],
        [0.0; 3],
        heads,
        1,
        0,
        0,
    );
    let bytes = builder.build();
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    assets.insert(
        MonsterKind::Gargantua
            .default_model_path()
            .expect("model path"),
        plan_scripted_monster_model_bytes(),
    );
    (
        Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("convex room"),
        assets,
    )
}

/// A point inside every pose, with a strict radius margin from every room corner.
fn grenade_exposure_point(game: &Game) -> ohl_ai::Vec3 {
    use ohl_ai::Vec3;
    use ohl_game::registry::Transform;
    use ohl_physics::Hull;
    use ohl_world::{StudioModel, StudioPose};

    let entity = entity_of_classname(game, "monster_gargantua").expect("target");
    let actor = *game.registry().world.get::<&Actor>(entity).expect("actor");
    let transform = *game
        .registry()
        .world
        .get::<&Transform>(entity)
        .expect("transform");
    let anim = *game
        .registry()
        .world
        .get::<&ohl_engine::StudioAnim>(entity)
        .expect("same loaded posed model");
    assert!(actor.alive);
    assert_eq!(anim.model, 0, "only this model exists in the asset source");
    assert_eq!(anim.sequence, 0);
    assert!(anim.cycle.is_finite() && anim.cycle >= 0.0);
    assert_eq!(actor.origin, transform.origin);
    assert_eq!((actor.origin.x, actor.origin.y), (32.0, 0.0));
    assert!(
        (0.0..0.1).contains(&actor.origin.z),
        "spawn settled on floor"
    );
    assert_eq!(actor.hull, Hull::Large);
    assert_eq!(transform.angles, Vec3::new(0.0, 180.0, 0.0));
    let world = game.collision().expect("collision");
    for (hull, point) in [
        (actor.hull, actor.query_origin()),
        (Hull::Standing, Vec3::from_array(game.player_origin())),
    ] {
        let clear = world.trace(hull, point, point);
        assert!(!clear.start_solid && !clear.all_solid, "actual hull fits");
        let floor = world.trace(hull, point, point - Vec3::Z);
        assert!(
            !floor.start_solid && floor.fraction < 1.0,
            "floor supported"
        );
    }
    let model = StudioModel::parse(
        &plan_scripted_monster_model_bytes(),
        &ohl_formats::mdl10::Limits::default(),
    )
    .expect("same posed model");
    assert_eq!(model.hitboxes.len(), 1);
    assert_eq!(model.hitboxes[0].bone, 0);
    assert_eq!(model.bones[0].parent, None);
    assert_eq!(model.bones[0].value, [0.0; 6]);
    assert_eq!(model.sequences.len(), 1);
    assert_eq!(model.sequences[0].frame_count, 2);
    assert_eq!(model.sequences[0].fps, 10.0);
    // Root X translates from 10 to 20; its other channels are zero. Sampling
    // interpolates those endpoints, so their intersection covers every pose.
    let rotation = ohl_combat::Quat::from_rotation_z(180.0_f32.to_radians());
    let local_point = Vec3::new(15.0, 0.0, 36.0);
    for (time, x) in [(0.0, 10.0), (0.1, 20.0)] {
        let pose = StudioPose::sample(&model, 0, time).expect("authored pose");
        let (min, max) = pose.hitbox_bounds(&model.hitboxes[0]).expect("posed box");
        assert_eq!(min, [x - 24.0, -24.0, 0.0]);
        assert_eq!(max, [x + 24.0, 24.0, 72.0]);
        assert!(
            local_point.cmpgt(Vec3::from_array(min)).all()
                && local_point.cmplt(Vec3::from_array(max)).all()
        );
        for corner in 0..8 {
            let local = Vec3::new(
                if corner & 1 == 0 { min[0] } else { max[0] },
                if corner & 2 == 0 { min[1] } else { max[1] },
                if corner & 4 == 0 { min[2] } else { max[2] },
            );
            let point = actor.origin + rotation * local;
            assert!(
                point.cmpgt(Vec3::new(-80.0, -80.0, 0.0)).all()
                    && point.cmplt(Vec3::new(80.0, 80.0, 128.0)).all()
            );
        }
    }
    let common = actor.origin + rotation * local_point;
    for x in [-80.0, 80.0] {
        for y in [-80.0, 80.0] {
            for z in [0.0, 128.0] {
                assert!(
                    Vec3::new(x, y, z).distance(common) < 160.0,
                    "margin under radius 200"
                );
            }
        }
    }
    common
}

fn equip(
    game: &mut Game,
    assets: &MemoryAssets,
    weapon: WeaponId,
    clip: u32,
    reserve: &[(AmmoType, u32)],
) {
    let mut save = game.to_save(0);
    let inventory = save.inventory.as_mut().expect("inventory");
    inventory.weapons = vec![WeaponSnapshot::default(); WeaponId::ALL.len()];
    let index = WeaponId::ALL
        .iter()
        .position(|id| *id == weapon)
        .expect("weapon index");
    inventory.weapons[index] = WeaponSnapshot { owned: true, clip };
    inventory.ammo = vec![0; AmmoType::ALL.len()];
    for &(kind, amount) in reserve {
        inventory.ammo[AmmoType::ALL
            .iter()
            .position(|id| *id == kind)
            .expect("ammo index")] = amount;
    }
    inventory.selected = Some(u8::try_from(index).expect("small table"));
    inventory.firing = Some(FiringSnapshot {
        weapon: inventory.selected.expect("selected"),
        state_tag: 0,
        timer: 0.0,
    });
    *game = Game::from_save(assets, &save).expect("synthetic loadout restore");
    // Ordinary selection is still the trigger into the player's firing path.
    game.tick(
        TICK_SECONDS,
        &Input {
            select_slot: Some(hud_slot(weapon).slot),
            ..Input::default()
        },
    );
}

fn tick(game: &mut Game, count: usize, input: Input) {
    for _ in 0..count {
        game.tick(TICK_SECONDS, &input);
    }
}

fn low_health(game: &Game, classname: &str) {
    let entity = entity_of_classname(game, classname).expect("synthetic target");
    game.registry()
        .world
        .get::<&mut Actor>(entity)
        .expect("actor")
        .health = 1.0;
}

#[test]
fn real_rpg_launch_kills_a_low_health_gargantua_after_flight() {
    let (mut game, assets) = fixture("monster_gargantua", true);
    equip(&mut game, &assets, WeaponId::Rpg, 1, &[]);
    low_health(&game, "monster_gargantua");
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.projectile_count(), 1, "real input emitted a rocket");
    assert_eq!(game.inventory().clip(WeaponId::Rpg), 0);
    assert_eq!(
        game.monster_death_count(),
        0,
        "flight is not an instant hit"
    );
    let save = game.to_save(0);
    let physical = &save.projectiles.as_ref().expect("physics").projectiles[0];
    assert!(
        (physical.age - TICK_SECONDS).abs() < 0.0001,
        "player spawn advances once"
    );
    let profile = &save.projectile_runtime.as_ref().expect("profiles").attacks[0];
    assert_eq!(profile.damage, 100.0);
    assert_eq!(profile.owner, Some(ProjectileEntityRef::Player));
    tick(&mut game, 60, Input::default());
    assert_eq!(
        game.monster_death_count(),
        1,
        "BLAST reaches the normal boss lifecycle"
    );
}

#[test]
fn real_hand_grenade_keeps_its_fuse_then_kills_a_low_health_gargantua() {
    let (mut game, assets) = grenade_room();
    equip(&mut game, &assets, WeaponId::HandGrenade, 1, &[]);
    let exposure = grenade_exposure_point(&game);
    low_health(&game, "monster_gargantua");
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.projectile_count(), 1);
    let emitted = game.to_save(0);
    let profile = &emitted
        .projectile_runtime
        .as_ref()
        .expect("profile")
        .attacks[0];
    assert_eq!(profile.damage, 100.0);
    assert_eq!(profile.blast_radius, Some(200.0));
    assert_eq!(profile.owner, Some(ProjectileEntityRef::Player));
    let mut contact = false;
    for _ in 0..450 {
        tick(&mut game, 1, Input::default());
        let save = game.to_save(0);
        if let Some(projectile) = save
            .projectiles
            .as_ref()
            .expect("physics")
            .projectiles
            .first()
        {
            let point = ohl_ai::Vec3::from_array(projectile.position);
            assert!(
                point.cmpgt(ohl_ai::Vec3::new(-80.0, -80.0, 0.0)).all()
                    && point.cmplt(ohl_ai::Vec3::new(80.0, 80.0, 128.0)).all(),
                "projectile stays in convex interior"
            );
            let sight = game.collision().expect("collision").trace(
                ohl_physics::Hull::Point,
                point,
                exposure,
            );
            assert!(
                !sight.start_solid && sight.fraction == 1.0,
                "actual blast path stays clear"
            );
            contact |= projectile.resting || projectile.velocity[0] < 0.0;
        }
    }
    assert_eq!(
        game.monster_death_count(),
        0,
        "contact must not detonate a hand grenade"
    );
    assert_eq!(game.projectile_count(), 1);
    assert!(
        contact,
        "real contact reflected or stopped the thrown grenade"
    );
    assert_eq!(
        grenade_exposure_point(&game),
        exposure,
        "target envelope stayed fixed"
    );
    let before_fuse = game.to_save(0);
    let physical = &before_fuse
        .projectiles
        .as_ref()
        .expect("physics")
        .projectiles[0];
    assert!(physical.fuse.is_some_and(|fuse| fuse > 0.0 && fuse < 0.6));
    tick(&mut game, 70, Input::default());
    assert_eq!(game.projectile_count(), 0, "timed fuse removed the grenade");
    assert_eq!(game.monster_death_count(), 1, "fuse detonation uses BLAST");
    assert_eq!(profile.damage_bits, ohl_combat::DamageType::BLAST.bits());
}

#[test]
fn real_mp5_secondary_uses_grenade_reserves_and_detonates_on_contact() {
    for clip in [0, 50] {
        let (mut game, assets) = fixture("monster_gargantua", true);
        equip(
            &mut game,
            &assets,
            WeaponId::Mp5,
            clip,
            &[(AmmoType::Mp5Grenades, 2), (AmmoType::NineMillimeter, 17)],
        );
        low_health(&game, "monster_gargantua");
        tick(
            &mut game,
            1,
            Input {
                attack2: true,
                ..Input::default()
            },
        );
        assert_eq!(game.projectile_count(), 1);
        assert_eq!(game.inventory().clip(WeaponId::Mp5), clip);
        assert_eq!(game.inventory().ammo(AmmoType::Mp5Grenades).current(), 1);
        assert_eq!(
            game.inventory().ammo(AmmoType::NineMillimeter).current(),
            17
        );
        assert_eq!(game.monster_death_count(), 0);
        tick(&mut game, 100, Input::default());
        assert_eq!(
            game.monster_death_count(),
            1,
            "contact blast precedes a timed fuse"
        );
    }
}

#[test]
fn mp5_dry_secondary_reload_overlap_and_simultaneous_buttons_preserve_pools() {
    let (mut game, assets) = fixture("", true);
    equip(
        &mut game,
        &assets,
        WeaponId::Mp5,
        0,
        &[(AmmoType::NineMillimeter, 30)],
    );
    tick(
        &mut game,
        1,
        Input {
            attack2: true,
            ..Input::default()
        },
    );
    assert_eq!(game.projectile_count(), 0);
    assert_eq!(
        game.inventory().ammo(AmmoType::NineMillimeter).current(),
        30
    );
    equip(
        &mut game,
        &assets,
        WeaponId::Mp5,
        0,
        &[(AmmoType::NineMillimeter, 30), (AmmoType::Mp5Grenades, 2)],
    );
    tick(
        &mut game,
        1,
        Input {
            attack2: true,
            reload: true,
            ..Input::default()
        },
    );
    tick(
        &mut game,
        40,
        Input {
            attack2: true,
            ..Input::default()
        },
    );
    assert_eq!(
        game.inventory().ammo(AmmoType::Mp5Grenades).current(),
        2,
        "reload cannot draw launcher ammo"
    );
    tick(&mut game, 400, Input::default());
    assert_eq!(game.inventory().clip(WeaponId::Mp5), 30);
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            attack2: true,
            ..Input::default()
        },
    );
    assert_eq!(game.inventory().clip(WeaponId::Mp5), 29);
    assert_eq!(
        game.inventory().ammo(AmmoType::Mp5Grenades).current(),
        2,
        "primary wins both buttons"
    );
}

#[test]
fn satchel_radio_held_edge_zero_ammo_and_foreign_owner_survive_restore() {
    let (mut game, assets) = fixture("monster_zombie", true);
    equip(&mut game, &assets, WeaponId::Satchel, 1, &[]);
    low_health(&game, "monster_zombie");
    game.debug_place_satchel([-220.0, 220.0, 36.0])
        .expect("foreign owner-less charge");
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.deployable_count(), 2);
    assert_eq!(game.inventory().clip(WeaponId::Satchel), 0);
    assert_eq!(game.inventory().ammo(AmmoType::Satchels).current(), 0);
    tick(
        &mut game,
        125,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(
        game.deployable_count(),
        2,
        "holding place cannot become radio detonation"
    );
    let bytes = game.save_bytes(0).expect("save");
    let mut restored = Game::load_bytes(&assets, &bytes).expect("load");
    tick(
        &mut restored,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(restored.deployable_count(), 2, "held edge survives save");
    tick(&mut restored, 1, Input::default());
    tick(
        &mut restored,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(
        restored.deployable_count(),
        1,
        "radio selects only player's charges"
    );
    assert_eq!(
        restored.monster_death_count(),
        1,
        "radio blast reaches an ordinary target"
    );
}

#[test]
fn satchel_secondary_adds_charges_with_automatic_single_round_reload() {
    let (mut game, assets) = fixture("", true);
    equip(
        &mut game,
        &assets,
        WeaponId::Satchel,
        1,
        &[(AmmoType::Satchels, 1)],
    );
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    tick(
        &mut game,
        250,
        Input {
            attack2: true,
            ..Input::default()
        },
    );
    assert_eq!(game.deployable_count(), 2);
    assert_eq!(game.inventory().ammo(AmmoType::Satchels).current(), 0);
    assert_eq!(game.inventory().clip(WeaponId::Satchel), 0);
}

#[test]
fn real_tripmine_failed_placement_is_free_and_player_crossing_triggers_after_arming() {
    let (mut game, assets) = fixture("", true);
    equip(&mut game, &assets, WeaponId::Tripmine, 1, &[]);
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.deployable_count(), 0);
    assert_eq!(
        game.inventory().clip(WeaponId::Tripmine),
        1,
        "failed wall trace refunds the mine"
    );
    game.set_viewpoint([208.0, 0.0, 36.0], 0.0, 0.0);
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.deployable_count(), 1);
    assert_eq!(game.inventory().clip(WeaponId::Tripmine), 0);
    game.set_viewpoint([208.0, 96.0, 36.0], 0.0, 0.0);
    tick(&mut game, 100, Input::default());
    assert_eq!(game.deployable_count(), 1, "quiet before arming");
    let bytes = game.save_bytes(0).expect("save mine mid-arm");
    for mut continuation in vec![
        game,
        Game::load_bytes(&assets, &bytes).expect("restore mine"),
    ]
    .into_boxed_slice()
    {
        tick(&mut continuation, 250, Input::default());
        assert_eq!(
            continuation.deployable_count(),
            1,
            "armed mine must ignore its own stand-in"
        );
        let before = continuation.player_health();
        continuation.set_viewpoint([208.0, 0.0, 36.0], 0.0, 0.0);
        tick(&mut continuation, 1, Input::default());
        assert_eq!(
            continuation.deployable_count(),
            0,
            "model-less player crosses the live beam"
        );
        assert!(
            continuation.player_health() < before,
            "wall blast escapes the solid surface"
        );
    }
}

#[test]
fn live_grunt_schedule_queues_a_grenade_that_later_hurts_the_player() {
    let (mut game, _) = safe_human_grenade_fixture("monster_human_grunt");
    let grunt = entity_of_classname(&game, "monster_human_grunt").expect("grunt");
    let mut thrown = false;
    for _ in 0..600 {
        let before = game.player_health();
        tick(&mut game, 1, Input::default());
        let save = game.to_save(0);
        if save
            .projectiles
            .as_ref()
            .expect("physics")
            .projectiles
            .iter()
            .any(|p| p.kind_tag == 3)
        {
            assert_eq!(
                game.player_health(),
                before,
                "throw does not inflict immediate grenade damage"
            );
            let p = save
                .projectiles
                .as_ref()
                .expect("physics")
                .projectiles
                .iter()
                .find(|p| p.kind_tag == 3)
                .expect("grenade");
            assert_eq!(p.age, 0.0, "AI spawn waits for next phase 7");
            assert_safe_human_grenade_geometry(&game, "monster_human_grunt");
            thrown = true;
            break;
        }
    }
    assert!(thrown, "live senses and schedules must produce Range2");
    // Kill the shooter through normal damage/lifecycle; its emitted grenade remains live.
    ohl_engine::test_support::queue_monster_damage(&mut game, grunt, None, 10_000.0);
    tick(&mut game, 1, Input::default());
    assert_eq!(
        game.monster_death_count(),
        1,
        "the shooter is dead before detonation"
    );
    let before = game.player_health();
    tick(&mut game, 530, Input::default());
    assert!(
        game.player_health() < before,
        "queued AI grenade travels, fuses, and damages player"
    );
}

#[test]
fn live_monster_projectiles_capture_each_published_attack_profile() {
    use ohl_campaign::Difficulty;
    use ohl_engine::GameConfig;
    let cases: &[(&str, u8, [f32; 3])] = &[
        ("monster_human_grunt", 3, [100.0, 100.0, 100.0]),
        ("monster_human_assassin", 3, [100.0, 100.0, 100.0]),
        ("monster_bullchicken", 6, [10.0, 10.0, 15.0]),
        ("monster_alien_controller", 7, [3.0, 4.0, 5.0]),
        ("monster_alien_controller", 8, [15.0, 25.0, 35.0]),
        ("monster_apache", 1, [150.0, 150.0, 150.0]),
        ("monster_bigmomma", 9, [100.0, 120.0, 160.0]),
        ("monster_alien_grunt", 4, [4.0, 5.0, 8.0]),
    ];
    for &(classname, tag, expected) in cases {
        for (skill, difficulty) in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard]
            .into_iter()
            .enumerate()
        {
            let (_, assets) = if tag == 3 {
                safe_human_grenade_fixture(classname)
            } else {
                fixture(classname, false)
            };
            let mut game = Game::load_with(
                &assets,
                AI_MAP,
                &GameConfig {
                    difficulty,
                    ..Default::default()
                },
            )
            .expect("difficulty fixture");
            if tag == 3 {
                assert_safe_human_grenade_geometry(&game, classname);
            }
            if tag == 9 {
                game.set_viewpoint([-128.0, 0.0, 36.0], 0.0, 0.0);
            }
            let mut found = false;
            for _ in 0..1500 {
                tick(&mut game, 1, Input::default());
                let save = game.to_save(0);
                if let Some(projectile) = save
                    .projectiles
                    .as_ref()
                    .expect("physics")
                    .projectiles
                    .iter()
                    .find(|p| p.kind_tag == tag)
                {
                    let attack = save
                        .projectile_runtime
                        .as_ref()
                        .expect("profile")
                        .attacks
                        .iter()
                        .find(|p| p.id == projectile.id)
                        .expect("resolved profile");
                    assert_eq!(
                        attack.damage, expected[skill],
                        "source-specific attack {classname}, kind {tag}"
                    );
                    assert!(matches!(
                        attack.owner,
                        Some(ProjectileEntityRef::Registry(_))
                    ));
                    if tag == 9 {
                        assert_eq!(attack.blast_radius, Some([250.0, 250.0, 275.0][skill]));
                    }
                    if tag == 8 {
                        let speed = glam::Vec3::from_array(projectile.velocity).length();
                        assert!((speed - [650.0, 800.0, 1000.0][skill]).abs() < 0.01);
                        assert_eq!(attack.target, Some(ProjectileEntityRef::Player));
                    }
                    found = true;
                    break;
                }
            }
            assert!(
                found,
                "live schedule never launched {classname}, kind {tag}"
            );
        }
    }
}

#[test]
fn incoming_spit_and_controller_hand_ball_hit_the_model_less_player_after_flight() {
    for (classname, tag, damage) in [
        ("monster_bullchicken", 6, 10.0),
        ("monster_alien_controller", 7, 4.0),
    ] {
        let (mut game, _) = fixture(classname, false);
        let monster = entity_of_classname(&game, classname).expect("shooter");
        let mut launched = false;
        for _ in 0..600 {
            let before = game.player_health();
            tick(&mut game, 1, Input::default());
            if game
                .to_save(0)
                .projectiles
                .expect("physics")
                .projectiles
                .iter()
                .any(|p| p.kind_tag == tag)
            {
                assert_eq!(
                    game.player_health(),
                    before,
                    "flight replaces instant hitscan"
                );
                launched = true;
                break;
            }
        }
        assert!(launched);
        ohl_engine::test_support::strip_monster_ai(&mut game, monster);
        let before = game.player_health();
        tick(&mut game, 50, Input::default());
        assert_eq!(
            game.player_health(),
            before - damage,
            "one physical impact on player hull"
        );
    }
}

#[test]
fn projectile_capacity_rejection_refunds_the_real_player_shot() {
    let (mut game, assets) = fixture("", true);
    equip(&mut game, &assets, WeaponId::Rpg, 1, &[]);
    for _ in 0..128 {
        game.debug_spawn_projectile(
            ohl_combat::ProjectileKind::Rocket,
            [0.0, 96.0, 100.0],
            [0.0; 3],
        )
        .expect("fill bound");
    }
    let fired = game.weapon_fired_count();
    tick(
        &mut game,
        1,
        Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(game.projectile_count(), 128);
    assert_eq!(game.inventory().clip(WeaponId::Rpg), 1);
    assert_eq!(game.weapon_fired_count(), fired);
}

#[test]
fn mp5_reload_without_primary_reserves_does_not_suppress_a_valid_grenade() {
    for clip in [0, 17] {
        let (mut game, assets) = fixture("", true);
        equip(
            &mut game,
            &assets,
            WeaponId::Mp5,
            clip,
            &[(AmmoType::Mp5Grenades, 2)],
        );
        tick(
            &mut game,
            1,
            Input {
                reload: true,
                attack2: true,
                ..Input::default()
            },
        );
        assert_eq!(
            game.projectile_count(),
            1,
            "ineligible primary reload must not seize grenade reserves"
        );
        assert_eq!(game.inventory().clip(WeaponId::Mp5), clip);
        assert_eq!(game.inventory().ammo(AmmoType::NineMillimeter).current(), 0);
        assert_eq!(game.inventory().ammo(AmmoType::Mp5Grenades).current(), 1);
    }
}
