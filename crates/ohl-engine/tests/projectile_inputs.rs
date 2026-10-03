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
    let (mut game, assets) = fixture("monster_gargantua", true);
    equip(&mut game, &assets, WeaponId::HandGrenade, 1, &[]);
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
    tick(&mut game, 450, Input::default());
    assert_eq!(
        game.monster_death_count(),
        0,
        "contact must not detonate a hand grenade"
    );
    assert_eq!(game.projectile_count(), 1);
    tick(&mut game, 70, Input::default());
    assert_eq!(game.monster_death_count(), 1, "fuse detonation uses BLAST");
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
    for mut continuation in [
        game,
        Game::load_bytes(&assets, &bytes).expect("restore mine"),
    ] {
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
    let (mut game, _) = fixture("monster_human_grunt", false);
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
            let (_, assets) = fixture(classname, false);
            let mut game = Game::load_with(
                &assets,
                AI_MAP,
                &GameConfig {
                    difficulty,
                    ..Default::default()
                },
            )
            .expect("difficulty fixture");
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
