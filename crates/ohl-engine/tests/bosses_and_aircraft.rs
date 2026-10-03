//! Wave 1 batch B through the whole engine: the damage-type rules, the
//! Gonarch's trail, the Nihilanth's activation and shield, the aircraft's
//! `Start Inactive` route, and all of their state across a save.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`'s AI
//! room, entity blocks written below); every `targetname` is a synthetic
//! `ohl_*` name, and no bytes come from any game installation. See
//! `docs/CLEAN_ROOM.md`.

use ohl_ai::MonsterKind;
use ohl_ai::monsters::bigmomma::TrailPhase;
use ohl_ai::monsters::nihilanth::HEAD_OPEN_SECONDS;
use ohl_ai::monsters::table::BIGMOMMA_HEALTH_FACTOR;
use ohl_ai::{DamageKinds, FlightPlan, GonarchTrail, NihilanthShield};
use ohl_combat::{AmmoType, WeaponId};
use ohl_engine::StartInventoryItem;
use ohl_engine::test_support::{
    AI_MAP, ai_room_bsp, monster_entities, plan_scripted_monster_model_bytes, queue_monster_damage,
    queue_typed_monster_damage,
};
use ohl_engine::{Game, GameEvent, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::hecs::Entity;
use ohl_game::registry::MonsterActivation;
use std::fmt::Write as _;

/// The room's entity block: a worldspawn, a player start facing `+X`, and
/// whatever the test adds.
fn entities(extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {extra}"
    )
}

/// One entity block: `classname` at `origin` with `keys`.
fn block(classname: &str, origin: [f32; 3], keys: &[(&str, &str)]) -> String {
    let mut text = format!(
        "{{\n\"classname\" \"{classname}\"\n\"origin\" \"{} {} {}\"\n",
        origin[0], origin[1], origin[2]
    );
    for (key, value) in keys {
        let _ = writeln!(text, "\"{key}\" \"{value}\"");
    }
    text.push_str("}\n");
    text
}

fn trigger_auto(target: &str) -> String {
    block("trigger_auto", [0.0, 0.0, 0.0], &[("target", target)])
}

/// A `trigger_changelevel` a fired name can reach; firing it is visible to
/// the host as a `GameEvent::LevelChange`.
fn exit_trigger() -> String {
    block(
        "trigger_changelevel",
        [0.0, 0.0, 0.0],
        &[
            ("targetname", "ohl_exit"),
            ("map", "ohlelsewhere"),
            ("landmark", "ohl_landmark"),
        ],
    )
}

fn assets(entities: &str) -> MemoryAssets {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), ai_room_bsp(entities, false));
    assets
}

fn game_from(entities: &str) -> Game {
    let bytes = ai_room_bsp(entities, false);
    Game::from_map_bytes(&assets(entities), AI_MAP, &bytes).expect("the AI room loads")
}

/// Saves `game` and loads it back over the same map.
fn reload(game: &Game, entities: &str) -> Game {
    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    Game::load_bytes(&assets(entities), &bytes).expect("the save loads")
}

/// Steps `game` and reports whether any step announced a level change.
fn tick_until_level_change(game: &mut Game, ticks: usize) -> bool {
    let input = Input::default();
    let mut fired = false;
    for _ in 0..ticks {
        for event in game.tick(TICK_SECONDS, &input) {
            if matches!(event, GameEvent::LevelChange { .. }) {
                fired = true;
            }
        }
    }
    fired
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

fn the_monster(game: &Game) -> Entity {
    let monsters = monster_entities(game);
    assert_eq!(monsters.len(), 1, "the room holds one monster");
    monsters[0]
}

fn health_of(game: &Game, entity: Entity) -> f32 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .expect("actor")
        .health
}

fn origin_of(game: &Game, entity: Entity) -> ohl_ai::Vec3 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .expect("actor")
        .origin
}

// --- Damage types -----------------------------------------------------------

/// The gargantua's published immunity through the engine's own monster
/// damage intake. A hit typed as a bullet (or left untyped, as an unknown
/// source's hit is) costs no health and counts as no damage applied; a
/// blast does both; enough blast kills, once. (The hits are queued past
/// the engine's weapon-to-monster drain; the two real-input tests below
/// cover that drain.)
#[test]
fn a_gargantua_shrugs_off_bullets_and_dies_to_blast() {
    let mut game = game_from(&entities(&block(
        "monster_gargantua",
        [128.0, 0.0, 36.0],
        &[("angle", "180")],
    )));
    let garg = the_monster(&game);
    let full = health_of(&game, garg);
    assert!(full >= 800.0, "the gargantua's own cited health");

    queue_typed_monster_damage(&mut game, garg, None, 500.0, DamageKinds::BULLET);
    queue_monster_damage(&mut game, garg, None, 500.0);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, garg) - full).abs() < 1e-3,
        "bullets and untyped hits cost nothing"
    );
    assert_eq!(game.monster_death_count(), 0);
    assert_eq!(game.monster_damage_event_count(), 0, "no damage applied");

    // A blast and a bullet in the same step: the blast costs health and
    // counts as damage applied; the bullet beside it does neither.
    queue_typed_monster_damage(&mut game, garg, None, 100.0, DamageKinds::BLAST);
    queue_typed_monster_damage(&mut game, garg, None, 100.0, DamageKinds::BULLET);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, garg) - (full - 100.0)).abs() < 1e-3,
        "a blast costs health"
    );
    assert_eq!(game.monster_damage_event_count(), 1, "one hit applied");

    queue_typed_monster_damage(&mut game, garg, None, 5_000.0, DamageKinds::BLAST);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 1, "enough blast kills, once");
    assert_eq!(game.monster_count(), 0);
}

/// A gargantua in front of the player, held `Prisoner` (published bit 16)
/// so it never fights back, its species' model replaced by a synthetic one
/// with a hitbox a shot can land on, and `loadout` in the player's hands.
fn gargantua_range(loadout: &[StartInventoryItem]) -> (Game, Entity) {
    let entities = entities(&block(
        "monster_gargantua",
        [96.0, 0.0, 36.0],
        &[("angle", "180"), ("spawnflags", "16")],
    ));
    let bytes = ai_room_bsp(&entities, false);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    // Under the species' own default path, so the engine loads it as the
    // gargantua's model; the bytes are this project's synthetic fixture.
    assets.insert(
        MonsterKind::Gargantua
            .default_model_path()
            .expect("the gargantua has a default model"),
        plan_scripted_monster_model_bytes(),
    );
    let mut game = Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads");
    game.give_start_inventory(loadout);
    let garg = the_monster(&game);
    (game, garg)
}

/// Draws the weapon in HUD `slot` and, when it has a clip, loads it.
fn draw(game: &mut Game, slot: u8, reload: bool) {
    game.tick(
        TICK_SECONDS,
        &Input {
            select_slot: Some(slot),
            ..Input::default()
        },
    );
    if reload {
        game.tick(
            TICK_SECONDS,
            &Input {
                reload: true,
                ..Input::default()
            },
        );
    }
    tick(game, 300);
}

fn fire_for(game: &mut Game, ticks: usize) {
    let input = Input {
        attack: true,
        ..Input::default()
    };
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

/// The player's own egon, fired with real input: its beam is `ENERGYBEAM`,
/// one of the three types the gargantua is published as vulnerable to,
/// and the engine carries each hit's type from the weapon through its own
/// damage queue to the monster. Ten seconds of beam kill it.
#[test]
fn the_players_egon_kills_a_gargantua() {
    let (mut game, garg) = gargantua_range(&[
        StartInventoryItem::Weapon(WeaponId::Egon),
        StartInventoryItem::Ammo(AmmoType::Uranium),
        StartInventoryItem::Ammo(AmmoType::Uranium),
        StartInventoryItem::Ammo(AmmoType::Uranium),
        StartInventoryItem::Ammo(AmmoType::Uranium),
    ]);
    let full = health_of(&game, garg);
    draw(&mut game, 4, false);
    let mut fired = 0;
    while game.shot_hit_count() == 0 && fired < 100 {
        fire_for(&mut game, 1);
        fired += 1;
    }
    assert!(game.shot_hit_count() > 0, "the beam reached the gargantua");
    tick(&mut game, 1);
    assert!(health_of(&game, garg) < full, "and cost it health");
    assert!(game.monster_damage_event_count() > 0);
    // The original synthetic loadout fills the 100-cell reserve, enough
    // for the species' health at 14 damage per cell and the timed cadence.
    fire_for(&mut game, 1_000);
    assert_eq!(game.monster_death_count(), 1, "the gargantua died");
}

/// Real input reaches the monster ten times per second, spending ten
/// cells for 140 damage, at the table's TODO(black-box) 0.1-second interval.
#[test]
fn the_players_egon_hits_ten_times_per_second_and_cannot_tap_faster() {
    for tapping in [false, true] {
        let (mut game, garg) = gargantua_range(&[
            StartInventoryItem::Weapon(WeaponId::Egon),
            StartInventoryItem::Ammo(AmmoType::Uranium),
            StartInventoryItem::Ammo(AmmoType::Uranium),
        ]);
        draw(&mut game, 4, false);
        let full = health_of(&game, garg);
        let ammo = game.inventory_totals().1;
        for step in 0..100 {
            game.tick(
                TICK_SECONDS,
                &Input {
                    attack: !tapping || step % 2 == 0,
                    ..Input::default()
                },
            );
        }
        // Damage intake runs after the weapon phase on the following step.
        tick(&mut game, 1);
        assert_eq!(game.shot_hit_count(), 10, "one second, tapping={tapping}");
        assert_eq!(game.monster_damage_event_count(), 10);
        assert!((health_of(&game, garg) - (full - 140.0)).abs() < 1e-3);
        assert_eq!(game.inventory_totals().1, ammo - 10);
    }
}

/// A charged gauss release must resolve one scaled shot, not also a base
/// shot. The target and model are synthetic; held input drives the engine.
#[test]
fn the_players_gauss_charge_releases_one_scaled_hit() {
    let entities = entities(&block(
        "monster_scientist",
        [96.0, 0.0, 36.0],
        &[("angle", "180"), ("spawnflags", "16")],
    ));
    let mut assets = assets(&entities);
    assets.insert(
        MonsterKind::Scientist.default_model_path().expect("model"),
        plan_scripted_monster_model_bytes(),
    );
    let mut game =
        Game::from_map_bytes(&assets, AI_MAP, &ai_room_bsp(&entities, false)).expect("room");
    game.give_start_inventory(&[
        StartInventoryItem::Weapon(WeaponId::Gauss),
        StartInventoryItem::Ammo(AmmoType::Uranium),
    ]);
    let target = the_monster(&game);
    draw(&mut game, 4, false);
    let full = health_of(&game, target);
    let ammo = game.inventory_totals().1;
    let secondary = Input {
        attack2: true,
        ..Input::default()
    };
    game.tick(TICK_SECONDS, &secondary);
    for _ in 0..50 {
        game.tick(TICK_SECONDS, &secondary);
    }
    assert_eq!(
        game.shot_hit_count(),
        0,
        "holding does not emit damage per step"
    );
    tick(&mut game, 2);
    // Fifty held steps plus the release step: 0.51 seconds of charge.
    let charged_damage = 25.0 + 0.51 / 10.0 * (200.0 - 25.0);
    assert_eq!(game.shot_hit_count(), 1, "release resolves one hit");
    assert_eq!(game.monster_damage_event_count(), 1);
    assert!((health_of(&game, target) - (full - charged_damage)).abs() < 0.01);
    assert_eq!(game.inventory_totals().1, ammo - 1);
}

/// The same gargantua under the player's .357, fired with real input: its
/// rounds are bullets, which the gargantua's published immunity ignores.
/// They land, cost nothing, and count as no damage applied.
#[test]
fn the_players_magnum_lands_on_a_gargantua_and_costs_it_nothing() {
    let (mut game, garg) = gargantua_range(&[
        StartInventoryItem::Weapon(WeaponId::Python),
        StartInventoryItem::Ammo(AmmoType::ThreeFiveSeven),
    ]);
    let full = health_of(&game, garg);
    draw(&mut game, 2, true);
    fire_for(&mut game, 600);
    assert!(game.shot_hit_count() >= 3, "the rounds landed");
    assert!(
        (health_of(&game, garg) - full).abs() < 1e-3,
        "and cost nothing"
    );
    assert_eq!(game.monster_damage_event_count(), 0, "no damage applied");
    assert_eq!(game.monster_death_count(), 0);
}

/// The Apache's published "blast damage doubles damage": a blast costs
/// twice its amount, a bullet its own.
#[test]
fn an_apache_takes_double_from_a_blast() {
    let mut game = game_from(&entities(&block(
        "monster_apache",
        [128.0, 0.0, 160.0],
        &[],
    )));
    let apache = the_monster(&game);
    let full = health_of(&game, apache);
    queue_typed_monster_damage(&mut game, apache, None, 10.0, DamageKinds::BULLET);
    tick(&mut game, 1);
    assert!((health_of(&game, apache) - (full - 10.0)).abs() < 1e-3);
    queue_typed_monster_damage(&mut game, apache, None, 40.0, DamageKinds::BLAST);
    tick(&mut game, 1);
    assert!(
        (health_of(&game, apache) - (full - 90.0)).abs() < 1e-3,
        "the blast cost 80"
    );
}

// --- The Gonarch --------------------------------------------------------------

/// A Gonarch whose first node is `ohl_node`: it runs there, and the node
/// names `ohl_exit` to fire, `ohl_crate` to remove, `ohl_seq` to play and
/// a health to set. `extra` is added to the room.
fn gonarch_room(extra: &str) -> String {
    entities(&format!(
        "{}{}{}{}{extra}",
        block(
            "monster_bigmomma",
            [160.0, 0.0, 36.0],
            &[
                ("angle", "180"),
                ("targetname", "ohl_mother"),
                ("netname", "ohl_node"),
            ],
        ),
        block(
            "info_bigmomma",
            [32.0, 0.0, 36.0],
            &[
                ("targetname", "ohl_node"),
                ("reachtarget", "ohl_exit"),
                ("killtarget", "ohl_crate"),
                ("reachsequence", "ohl_seq"),
                ("health", "100"),
                ("spawnflags", "1"),
            ],
        ),
        block(
            "info_target",
            [0.0, 160.0, 36.0],
            &[("targetname", "ohl_crate")]
        ),
        exit_trigger(),
    ))
}

/// Reaching a node fires its `reachtarget` by name, removes its
/// `killtarget`, starts its `reachsequence`, and sets the node's health
/// (times the difficulty's published factor); on the way it is shielded.
#[test]
fn a_gonarch_reaching_a_node_fires_removes_and_plays_what_the_node_names() {
    // The script the node names, bound to the Gonarch by name.
    // `Override AI` (64), so a Gonarch already fighting is still taken.
    let script = block(
        "scripted_sequence",
        [32.0, 0.0, 36.0],
        &[
            ("targetname", "ohl_seq"),
            ("m_iszEntity", "ohl_mother"),
            ("m_iszPlay", "ohl_action"),
            ("spawnflags", "64"),
        ],
    );
    let mut game = game_from(&gonarch_room(&script));
    let gonarch = the_monster(&game);
    // This synthetic node/script contract measures assigned health. A neutral
    // actor still follows its trail, without a later mortar changing that health.
    game.registry()
        .world
        .get::<&mut ohl_ai::Actor>(gonarch)
        .expect("actor")
        .classification = ohl_ai::Classification::None;
    assert_eq!(game.registry().find("ohl_crate").len(), 1);
    assert_eq!(game.script_start_count(), 0);

    // On the trail: shielded.
    queue_monster_damage(&mut game, gonarch, None, 10_000.0);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 0, "shielded on the way");

    assert!(
        tick_until_level_change(&mut game, 300),
        "the node's reachtarget fired"
    );
    let crate_gone = game
        .registry()
        .find("ohl_crate")
        .iter()
        .all(|entity| !game.registry().world.contains(*entity));
    assert!(crate_gone, "the node's killtarget was removed");
    tick(&mut game, 5);
    assert_eq!(
        game.script_start_count(),
        1,
        "the node's reachsequence started"
    );
    let health = health_of(&game, gonarch);
    assert!(
        BIGMOMMA_HEALTH_FACTOR
            .iter()
            .any(|factor| (health - 100.0 * factor).abs() < 1e-3),
        "the node's health, scaled: {health}"
    );
    assert_eq!(
        game.registry()
            .world
            .get::<&GonarchTrail>(gonarch)
            .expect("trail")
            .phase(),
        TrailPhase::Holding { at: 0 }
    );
}

/// A Gonarch's place on its trail survives a save. Without tag 41 the load
/// rebuilds the trail from the map and sets it walking from the first node
/// again — which a save from before the tag existed still does.
#[test]
fn a_gonarchs_place_on_its_trail_round_trips_through_a_save() {
    // The same room without the exit, so the arrival fires nothing.
    let room = gonarch_room("").replace("\"targetname\" \"ohl_exit\"", "\"targetname\" \"ohl_x\"");
    let mut game = game_from(&room);
    let gonarch = the_monster(&game);
    tick(&mut game, 300);
    let phase = |game: &Game| {
        game.registry()
            .world
            .get::<&GonarchTrail>(gonarch)
            .expect("trail")
            .phase()
    };
    assert_eq!(phase(&game), TrailPhase::Holding { at: 0 });
    let health = health_of(&game, gonarch);

    let loaded = reload(&game, &room);
    assert_eq!(phase(&loaded), TrailPhase::Holding { at: 0 });
    assert!((health_of(&loaded, gonarch) - health).abs() < 1e-3);

    let mut save = game.to_save(1_700_000_000);
    assert!(save.bosses.is_some(), "a level with a boss writes tag 41");
    save.bosses = None;
    let bytes = save
        .to_bytes()
        .expect("a save missing tag 41 still encodes");
    let old = Game::load_bytes(&assets(&room), &bytes).expect("a pre-tag-41 save still loads");
    assert_eq!(
        phase(&old),
        TrailPhase::Traveling { to: 0 },
        "back at the trail's start"
    );
}

// --- The Nihilanth ------------------------------------------------------------

fn nihilanth_room(activated: bool, extra_keys: &[(&str, &str)]) -> String {
    let mut keys = vec![("angle", "180"), ("targetname", "ohl_boss")];
    keys.extend_from_slice(extra_keys);
    entities(&format!(
        "{}{}{}",
        block("monster_nihilanth", [160.0, 0.0, 128.0], &keys),
        if activated {
            trigger_auto("ohl_boss")
        } else {
            String::new()
        },
        exit_trigger(),
    ))
}

/// "When spawned, the Nihilanth does not attack immediately": in view of
/// the player it attacks nothing until a `use` of its name activates it,
/// and then it does.
#[test]
fn a_nihilanth_attacks_nothing_until_a_trigger_activates_it() {
    let mut dormant = game_from(&nihilanth_room(false, &[]));
    tick(&mut dormant, 500);
    assert!(
        (dormant.player_health() - 100.0).abs() < f32::EPSILON,
        "a dormant boss attacked"
    );

    let mut active = game_from(&nihilanth_room(true, &[]));
    tick(&mut active, 500);
    assert!(
        active.player_health() < 100.0,
        "the activated boss zapped the player"
    );
}

/// With no crystal in the room, hits drain the reserve, the head opens
/// after its delay, and only then can the boss die — once. Its page says
/// `TriggerCondition` does not work on it, so a declared death trigger
/// fires nothing.
#[test]
fn a_nihilanth_dies_once_its_head_opens_and_ignores_its_trigger_condition() {
    let keys = [("TriggerCondition", "4"), ("TriggerTarget", "ohl_exit")];
    let mut game = game_from(&nihilanth_room(false, &keys));
    let boss = the_monster(&game);

    queue_monster_damage(&mut game, boss, None, 10_000.0);
    assert!(!tick_until_level_change(&mut game, 4));
    assert_eq!(game.monster_death_count(), 0, "the reserve took it");

    // Past the head-opening delay, watching for a level change all along.
    let mut elapsed = 0.0;
    while elapsed < HEAD_OPEN_SECONDS + 0.5 {
        assert!(!tick_until_level_change(&mut game, 1));
        elapsed += TICK_SECONDS;
    }
    queue_monster_damage(&mut game, boss, None, 10_000.0);
    let fired = tick_until_level_change(&mut game, 8);
    assert_eq!(game.monster_death_count(), 1, "the boss died exactly once");
    assert!(!fired, "its TriggerCondition is not honoured");

    // The same declaration on a monster whose page does not say so fires:
    // the exit is reachable, so the silence above is the Nihilanth's own.
    let mut headcrab = game_from(&entities(&format!(
        "{}{}",
        block(
            "monster_headcrab",
            [160.0, 0.0, 36.0],
            &[("TriggerCondition", "4"), ("TriggerTarget", "ohl_exit")],
        ),
        exit_trigger(),
    )));
    let crab = the_monster(&headcrab);
    queue_monster_damage(&mut headcrab, crab, None, 10_000.0);
    assert!(tick_until_level_change(&mut headcrab, 8));
}

/// Activation and a drained reserve survive a save. The `trigger_auto`
/// that activated the boss is spent (tag 28), so a load that forgot the
/// activation would leave it dormant for good.
#[test]
fn a_nihilanths_activation_and_reserve_round_trip_through_a_save() {
    let room = nihilanth_room(true, &[]);
    let mut game = game_from(&room);
    let boss = the_monster(&game);
    tick(&mut game, 5);
    queue_monster_damage(&mut game, boss, None, 300.0);
    tick(&mut game, 1);
    let shield = |game: &Game| {
        let shield = game
            .registry()
            .world
            .get::<&NihilanthShield>(boss)
            .expect("shield");
        (shield.is_active(), shield.reserve())
    };
    let (active, reserve) = shield(&game);
    assert!(active);
    assert!(reserve < shield_capacity(&game, boss) - 1.0);

    let loaded = reload(&game, &room);
    assert_eq!(shield(&loaded), (active, reserve));
}

fn shield_capacity(game: &Game, boss: Entity) -> f32 {
    game.registry()
        .world
        .get::<&NihilanthShield>(boss)
        .expect("shield")
        .reserve_capacity()
}

/// A save taken in the one tick between the map logic's `use` landing
/// (the last phase of a tick) and the boss driver draining it (the next
/// tick's AI phase) keeps the `use`: tag 41 carries the pending count.
#[test]
fn a_use_still_pending_at_a_save_survives_it() {
    let room = nihilanth_room(true, &[]);
    let mut game = game_from(&room);
    let boss = the_monster(&game);
    tick(&mut game, 1);
    let pending = |game: &Game| {
        game.registry()
            .world
            .get::<&MonsterActivation>(boss)
            .expect("counter")
            .pending
    };
    let active = |game: &Game| {
        game.registry()
            .world
            .get::<&NihilanthShield>(boss)
            .expect("shield")
            .is_active()
    };
    assert_eq!(pending(&game), 1, "the trigger_auto's use, not yet drained");
    assert!(!active(&game));

    let mut loaded = reload(&game, &room);
    assert_eq!(pending(&loaded), 1);
    tick(&mut loaded, 1);
    assert!(active(&loaded), "the carried use activated it");
}

// --- The aircraft -------------------------------------------------------------

/// An Apache spawned `Start Inactive` on a two-corner loop, started by a
/// `trigger_auto` when `started`.
fn apache_room(started: bool) -> String {
    entities(&format!(
        "{}{}{}{}",
        block(
            "monster_apache",
            [-160.0, 0.0, 160.0],
            &[
                ("targetname", "ohl_heli"),
                ("target", "ohl_p1"),
                ("spawnflags", "64"),
            ],
        ),
        block(
            "path_corner",
            [160.0, 160.0, 192.0],
            &[("targetname", "ohl_p1"), ("target", "ohl_p2")],
        ),
        block(
            "path_corner",
            [160.0, -160.0, 160.0],
            &[("targetname", "ohl_p2"), ("target", "ohl_p1")],
        ),
        if started {
            trigger_auto("ohl_heli")
        } else {
            String::new()
        },
    ))
}

/// `Start Inactive` (64): the aircraft hangs where it was put until a
/// `use` of its name starts it, and then flies at its route.
#[test]
fn a_start_inactive_apache_waits_for_a_use_and_then_flies_its_route() {
    let mut parked = game_from(&apache_room(false));
    let apache = the_monster(&parked);
    let spawn = origin_of(&parked, apache);
    tick(&mut parked, 100);
    assert!(
        (origin_of(&parked, apache) - spawn).length() < 1e-3,
        "it moved without being started"
    );

    let mut started = game_from(&apache_room(true));
    let apache = the_monster(&started);
    tick(&mut started, 100);
    let moved = origin_of(&started, apache);
    assert!((moved - spawn).length() > 100.0, "{moved:?}");
    assert!(moved.z > spawn.z, "climbing toward the higher corner");
}

/// An aircraft's activation and place on its route survive a save.
#[test]
fn an_aircrafts_route_progress_round_trips_through_a_save() {
    let room = apache_room(true);
    let mut game = game_from(&room);
    let apache = the_monster(&game);
    tick(&mut game, 150);
    let progress = |game: &Game| {
        game.registry()
            .world
            .get::<&FlightPlan>(apache)
            .expect("plan")
            .progress()
    };
    let saved = progress(&game);
    assert!(saved.active);
    assert_eq!(saved.current, 1, "past the first corner");

    let loaded = reload(&game, &room);
    assert_eq!(progress(&loaded), saved);
}

#[test]
fn an_apache_advances_its_flight_route_while_firing_a_live_rocket() {
    let room = entities(&format!(
        "{}{}{}",
        block(
            "monster_apache",
            [128.0, 0.0, 64.0],
            &[("angle", "180"), ("target", "ohl_flight_a")]
        ),
        block(
            "path_corner",
            [128.0, 160.0, 64.0],
            &[("targetname", "ohl_flight_a"), ("target", "ohl_flight_b")]
        ),
        block(
            "path_corner",
            [128.0, -160.0, 64.0],
            &[("targetname", "ohl_flight_b"), ("target", "ohl_flight_a")]
        ),
    ));
    let mut game = game_from(&room);
    let apache = the_monster(&game);
    for _ in 0..200 {
        let before = origin_of(&game, apache);
        tick(&mut game, 1);
        let rocket_schedule = game
            .registry()
            .world
            .get::<&ohl_ai::MonsterAi>(apache)
            .expect("ai")
            .schedule_name()
            == "ohl/monsters/apache_rocket";
        if rocket_schedule {
            assert!(
                (origin_of(&game, apache) - before).length() > 0.1,
                "entering or running the rocket schedule must preserve flight movement"
            );
        }
        let launched = game
            .to_save(0)
            .projectiles
            .expect("physics")
            .projectiles
            .iter()
            .any(|p| p.kind_tag == 1 && p.age.abs() < f32::EPSILON);
        if launched {
            assert!(
                (origin_of(&game, apache) - before).length() > 0.1,
                "rocket firing must preserve flight movement in that tick"
            );
            assert!(
                game.registry()
                    .world
                    .get::<&FlightPlan>(apache)
                    .expect("flight plan")
                    .is_active()
            );
            return;
        }
    }
    panic!("live perception and secondary schedule must launch a rocket during flight");
}
