//! Floor pickups through the walking player, inventory and renderer.
//! All maps and model bytes are project-authored synthetic fixtures.

use ohl_combat::PickupKind;
use ohl_engine::skirmish::{
    AMMO_RESPAWN_SECONDS, ITEM_RESPAWN_SECONDS, NOT_IN_DEATHMATCH_SPAWNFLAG, WEAPON_RESPAWN_SECONDS,
};
use ohl_engine::test_support::{
    AI_MAP, deathmatch_room_bsp, entity_block, entity_of_classname, queue_engine_damage,
};
use ohl_engine::{Game, Input, MemoryAssets, Pickup, RenderTarget, SkirmishConfig, TICK_SECONDS};
use ohl_formats::test_support::build_minimal_mdl10;
use ohl_game::hecs::Entity;
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};

const OPT_IN: &str = "OHL_RENDER_GPU_TEST";
const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

struct Case {
    classname: &'static str,
    model: &'static str,
    respawn_seconds: f32,
}

const CASES: [Case; 4] = [
    Case {
        classname: "weapon_shotgun",
        model: "models/w_shotgun.mdl",
        respawn_seconds: WEAPON_RESPAWN_SECONDS,
    },
    Case {
        classname: "ammo_buckshot",
        model: "models/w_shotbox.mdl",
        respawn_seconds: AMMO_RESPAWN_SECONDS,
    },
    Case {
        classname: "item_healthkit",
        model: "models/w_medkit.mdl",
        respawn_seconds: ITEM_RESPAWN_SECONDS,
    },
    Case {
        classname: "item_battery",
        model: "models/w_battery.mdl",
        respawn_seconds: ITEM_RESPAWN_SECONDS,
    },
];

fn build_game(case: Option<&Case>, excluded: bool) -> Game {
    let flags = if excluded {
        NOT_IN_DEATHMATCH_SPAWNFLAG
    } else {
        0
    }
    .to_string();
    let extra = case.map_or_else(String::new, |case| {
        entity_block(
            case.classname,
            [96.0, 0.0, 4.0],
            0.0,
            &[("spawnflags", &flags)],
        )
    });
    let map = deathmatch_room_bsp(&[(0.0, 0.0)], false, &extra);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), map.clone());
    if let Some(case) = case {
        let (mut model, layout) = build_minimal_mdl10();
        // This synthetic model is a static quad, so frames compare pickup
        // visibility without also comparing its animation at different times.
        model[layout.sequences_offset + 32..layout.sequences_offset + 36]
            .copy_from_slice(&0.0_f32.to_le_bytes());
        assets.insert(case.model, model);
    }
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &map).expect("the arena loads");
    game.start_skirmish(
        &assets,
        &SkirmishConfig {
            bots: 0,
            frag_limit: 0,
            time_limit_seconds: 0.0,
            ..SkirmishConfig::default()
        },
    )
    .expect("the arena has a spawn point");
    if case.is_some_and(|case| case.classname == "item_healthkit") {
        let player = game.player_entity();
        queue_engine_damage(&mut game, player, 10.0);
        tick_for(&mut game, 0.5);
    }
    game
}

fn tick_for(game: &mut Game, seconds: f32) {
    let mut elapsed = 0.0;
    while elapsed < seconds {
        game.tick(TICK_SECONDS, &Input::default());
        elapsed += TICK_SECONDS;
    }
}

fn taken(game: &Game, entity: Entity) -> bool {
    game.registry()
        .world
        .get::<&Pickup>(entity)
        .expect("a pickup")
        .taken
}

fn quantity(game: &Game, kind: PickupKind) -> f64 {
    match kind {
        PickupKind::Weapon(weapon) => f64::from(u8::from(game.inventory().has_weapon(weapon))),
        PickupKind::Ammo(ammo) => f64::from(game.inventory().ammo(ammo).current()),
        PickupKind::HealthKit => f64::from(game.player_health()),
        PickupKind::Battery => f64::from(game.player_armor()),
        _ => unreachable!("this fixture uses weapons, ammo, health and armour"),
    }
}

fn collect_and_retreat(game: &mut Game, case: &Case, entity: Entity) {
    let kind = ohl_combat::classify_classname(case.classname).unwrap();
    let before = quantity(game, kind);
    assert!(!taken(game, entity));
    for _ in 0..180 {
        game.tick(
            TICK_SECONDS,
            &Input {
                forward: 1,
                ..Input::default()
            },
        );
        if taken(game, entity) {
            break;
        }
    }
    assert!(
        taken(game, entity),
        "walking over the floor pickup collects it"
    );
    assert!(
        quantity(game, kind) > before,
        "collecting grants the pickup's effect"
    );
    assert_eq!(game.pickup_count(), 1);
    for _ in 0..180 {
        game.tick(
            TICK_SECONDS,
            &Input {
                forward: -1,
                ..Input::default()
            },
        );
        if game.player_origin()[0] < 32.0 {
            break;
        }
    }
    assert!(
        game.player_origin()[0] < 32.0,
        "the player walked out of pickup reach"
    );
}

#[test]
fn walking_collects_visible_floor_pickups_and_they_respawn_without_duplicate_grants() {
    for case in &CASES {
        let mut game = build_game(Some(case), false);
        assert_eq!(
            game.prop_count(),
            1,
            "a pickup with no model field is placed"
        );
        let entity = entity_of_classname(&game, case.classname).unwrap();
        assert!(
            game.registry()
                .world
                .get::<&ohl_engine::StudioAnim>(entity)
                .is_ok()
        );
        collect_and_retreat(&mut game, case, entity);
        tick_for(&mut game, case.respawn_seconds - 1.0);
        assert!(taken(&game, entity), "still gone before the respawn delay");
        tick_for(&mut game, 1.5);
        assert!(!taken(&game, entity), "back after the respawn delay");
        assert_eq!(game.pickup_count(), 1, "respawning alone grants nothing");
    }
}

fn render_frame(game: &mut Game, context: &GpuContext) -> Vec<u8> {
    let target = OffscreenTarget::new(context, WIDTH, HEIGHT).expect("offscreen target");
    game.render_from(
        context,
        RenderTarget {
            view: target.view(),
            width: WIDTH,
            height: HEIGHT,
            format: OFFSCREEN_FORMAT,
        },
        [106.5, 0.5, 16.0],
        89.0,
        0.0,
    )
    .expect("the frame renders");
    context.wait();
    target.read_rgba(context).expect("frame reads back")
}

fn run_render_test() {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping pickup render test: {error}");
            return;
        }
    };
    let empty = render_frame(&mut build_game(None, false), &context);
    for case in &CASES {
        let mut game = build_game(Some(case), false);
        let entity = entity_of_classname(&game, case.classname).unwrap();
        let visible = render_frame(&mut game, &context);
        assert_ne!(visible, empty, "the available pickup is drawn");
        collect_and_retreat(&mut game, case, entity);
        assert_eq!(
            render_frame(&mut game, &context),
            empty,
            "the collected pickup is hidden"
        );
        tick_for(&mut game, case.respawn_seconds - 1.0);
        assert_eq!(
            render_frame(&mut game, &context),
            empty,
            "hidden until its respawn delay"
        );
        tick_for(&mut game, 1.5);
        assert_eq!(
            render_frame(&mut game, &context),
            visible,
            "the respawned pickup is drawn again"
        );
    }
    let mut excluded = build_game(Some(&CASES[0]), true);
    let entity = entity_of_classname(&excluded, CASES[0].classname).unwrap();
    assert!(taken(&excluded, entity));
    tick_for(&mut excluded, WEAPON_RESPAWN_SECONDS + 1.0);
    assert_eq!(
        render_frame(&mut excluded, &context),
        empty,
        "excluded pickups never appear"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn collecting_and_respawning_pickups_changes_the_rendered_frame() {
    if std::env::var_os(OPT_IN).is_none() {
        run_render_test();
    }
}

#[test]
fn collecting_and_respawning_pickups_changes_the_rendered_frame_when_opted_in() {
    if std::env::var_os(OPT_IN).is_some() {
        run_render_test();
    }
}
