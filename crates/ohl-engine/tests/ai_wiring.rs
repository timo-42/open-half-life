//! M7.9 P2: monsters, navigation and the lifecycle, over a synthetic room.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. See `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{AI_MAP, ai_room_bsp, monster_entities, queue_monster_damage};
use ohl_engine::{Corpse, Game, Input, MemoryAssets};
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

/// A `monster_*` entity block at `origin`, facing `yaw`.
fn monster(classname: &str, origin: [f32; 3], yaw: f32, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"{yaw}\"\n{extra}}}\n",
        origin[0], origin[1], origin[2]
    )
}

fn game_from(entities: &str, interior_wall: bool) -> Game {
    let bytes = ai_room_bsp(entities, interior_wall);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{AI_MAP}.bsp"), bytes.clone());
    Game::from_map_bytes(&assets, AI_MAP, &bytes).expect("the AI room loads")
}

fn tick(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(ohl_engine::TICK_SECONDS, &input);
    }
}

/// Whether the single monster in `game` has acquired the player.
fn monster_sees_player(game: &Game) -> bool {
    let player = game.player_entity();
    monster_entities(game).iter().any(|entity| {
        game.registry()
            .world
            .get::<&ohl_ai::MonsterAi>(*entity)
            .ok()
            .and_then(|ai| ai.enemy())
            == Some(player)
    })
}

/// A monster looking down the room at the player acquires it; the same
/// monster with a wall between them does not.
#[test]
fn a_monster_acquires_the_player_and_loses_it_behind_a_wall() {
    let block = entities(&monster("monster_headcrab", [96.0, 0.0, 36.0], 180.0, ""));

    let mut open = game_from(&block, false);
    assert_eq!(open.monster_count(), 1, "the map declares one monster");
    assert!(
        !monster_sees_player(&open),
        "nothing is acquired before a tick"
    );
    tick(&mut open, 32);
    assert!(
        monster_sees_player(&open),
        "an unobstructed player inside the view cone is acquired"
    );

    let mut walled = game_from(&block, true);
    assert_eq!(walled.monster_count(), 1);
    tick(&mut walled, 32);
    assert!(
        !monster_sees_player(&walled),
        "a wall between them blocks the line of sight"
    );
}

/// A `monstermaker` stops at `monstercount` and never exceeds
/// `m_imaxlivechildren`.
#[test]
fn a_monstermaker_respects_its_counts() {
    let block = entities(
        "{\n\"classname\" \"monstermaker\"\n\
         \"origin\" \"0 96 36\"\n\"monstertype\" \"monster_headcrab\"\n\
         \"monstercount\" \"3\"\n\"delay\" \"0.1\"\n\
         \"m_imaxlivechildren\" \"2\"\n\"spawnflags\" \"1\"\n}\n",
    );
    let mut game = game_from(&block, false);
    assert_eq!(game.monster_count(), 0, "nothing has spawned yet");

    for _ in 0..40 {
        tick(&mut game, 10);
        assert!(
            game.monster_count() <= 2,
            "m_imaxlivechildren caps the live children"
        );
    }

    // Kill both children; the maker's remaining quota is one, so exactly one
    // more child ever appears.
    for entity in monster_entities(&game) {
        queue_monster_damage(&mut game, entity, None, 1_000.0);
    }
    tick(&mut game, 200);
    assert_eq!(
        game.monster_count(),
        1,
        "monstercount stops the maker after its third child"
    );
}

/// A killed monster dies once and leaves a corpse; a species whose corpse
/// fades loses it again after the fade delay.
#[test]
fn a_killed_monster_leaves_a_corpse_that_fades() {
    let block = entities(&monster("monster_headcrab", [96.0, 0.0, 36.0], 180.0, ""));
    let mut game = game_from(&block, false);
    let monster = monster_entities(&game)[0];

    // Exactly lethal, not an overkill: the remains are a corpse, and this
    // species' corpse fades.
    queue_monster_damage(&mut game, monster, None, 10.0);
    tick(&mut game, 1);
    assert_eq!(
        game.monster_death_count(),
        1,
        "the monster died exactly once"
    );
    assert_eq!(game.monster_count(), 0, "a dead monster stops thinking");

    let seconds_left = game
        .registry()
        .world
        .get::<&Corpse>(monster)
        .expect("a corpse was left behind")
        .seconds_left;
    assert!(
        seconds_left.is_finite() && seconds_left > 0.0,
        "this species' corpse fades, so its timer is finite"
    );

    // Queueing more damage at a dead monster cannot produce a second death.
    queue_monster_damage(&mut game, monster, None, 10.0);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 1, "death is reported once");

    tick(&mut game, 1_200);
    assert!(
        !game.registry().world.contains(monster),
        "a fading corpse is removed once its timer runs out"
    );
}

/// An overkill gibs instead of leaving a corpse.
#[test]
fn an_overkill_gibs_the_monster() {
    let block = entities(&monster("monster_headcrab", [96.0, 0.0, 36.0], 180.0, ""));
    let mut game = game_from(&block, false);
    let monster = monster_entities(&game)[0];

    queue_monster_damage(&mut game, monster, None, 500.0);
    tick(&mut game, 1);
    assert_eq!(game.monster_death_count(), 1);
    assert!(
        !game.registry().world.contains(monster),
        "a gibbed monster leaves nothing behind"
    );
}

/// Two games from the same bytes, the same seed and the same inputs agree
/// exactly after 600 steps.
#[test]
fn the_same_seed_and_inputs_reproduce_the_same_ai_state() {
    let block = entities(&format!(
        "{}{}",
        monster("monster_headcrab", [96.0, 0.0, 36.0], 180.0, ""),
        monster("monster_zombie", [0.0, 128.0, 36.0], 270.0, ""),
    ));

    let mut first = game_from(&block, false);
    let mut second = game_from(&block, false);
    assert_eq!(
        first.ai_state_hash(),
        second.ai_state_hash(),
        "two fresh loads start identical"
    );

    let inputs = [
        Input {
            forward: 1,
            ..Input::default()
        },
        Input {
            right: 1,
            jump: true,
            ..Input::default()
        },
        Input::default(),
    ];
    for step in 0..600 {
        let input = inputs[step % inputs.len()];
        first.tick(ohl_engine::TICK_SECONDS, &input);
        second.tick(ohl_engine::TICK_SECONDS, &input);
    }
    assert_eq!(
        first.ai_state_hash(),
        second.ai_state_hash(),
        "600 identical steps land on the same AI state"
    );
}

/// A map with no `info_node` leaves the navigator detached; the straight
/// line fallback still moves a monster toward the player.
#[test]
fn a_map_without_nodes_still_moves_monsters() {
    let block = entities(&monster("monster_zombie", [200.0, 0.0, 36.0], 180.0, ""));
    let mut game = game_from(&block, false);
    let monster = monster_entities(&game)[0];
    let start = game
        .registry()
        .world
        .get::<&ohl_ai::Actor>(monster)
        .expect("the monster has an actor")
        .origin;

    tick(&mut game, 400);

    let moved = game
        .registry()
        .world
        .get::<&ohl_ai::Actor>(monster)
        .map_or(0.0, |actor| (actor.origin - start).length());
    assert!(
        moved > 1.0,
        "with no node graph the straight-line fallback still moves the monster"
    );
}

/// A node lattice does not change whether the game runs: the navigator is
/// attached, and the same map still ticks.
#[test]
fn a_node_lattice_attaches_a_navigator_without_changing_the_contract() {
    let mut nodes = String::new();
    for x in -2i32..=2 {
        for y in -2i32..=2 {
            let _ = write!(
                nodes,
                "{{\n\"classname\" \"info_node\"\n\"origin\" \"{} {} 36\"\n}}\n",
                x * 64,
                y * 64
            );
        }
    }
    let block = entities(&format!(
        "{nodes}{}",
        monster("monster_zombie", [200.0, 0.0, 36.0], 180.0, "")
    ));
    let mut game = game_from(&block, false);
    assert_eq!(game.monster_count(), 1);
    tick(&mut game, 200);
    assert_eq!(
        game.monster_count(),
        1,
        "the monster is still alive and thinking"
    );
}

/// A `monster_*` classname this project has no table row for spawns nothing
/// that thinks, and does not upset the step list.
#[test]
fn an_unknown_monster_classname_never_thinks() {
    let block = entities(&monster(
        "monster_not_in_our_table",
        [96.0, 0.0, 36.0],
        180.0,
        "",
    ));
    let mut game = game_from(&block, false);
    assert_eq!(game.monster_count(), 0);
    tick(&mut game, 64);
    assert_eq!(game.monster_count(), 0);
}

/// A hostile pair resolves an attack all the way through: the AI reports
/// it, the engine traces it, and the target loses health.
#[test]
fn a_monster_attack_reaches_its_enemy() {
    let block = entities(&format!(
        "{}{}",
        monster("monster_human_grunt", [64.0, 0.0, 36.0], 0.0, ""),
        monster("monster_barney", [160.0, 0.0, 36.0], 180.0, ""),
    ));
    let mut game = game_from(&block, false);
    assert_eq!(game.monster_count(), 2);

    let before: f32 = monster_entities(&game)
        .iter()
        .filter_map(|entity| {
            game.registry()
                .world
                .get::<&ohl_ai::Actor>(*entity)
                .ok()
                .map(|actor| actor.health)
        })
        .sum();

    tick(&mut game, 1_500);

    let after: f32 = monster_entities(&game)
        .iter()
        .filter_map(|entity| {
            game.registry()
                .world
                .get::<&ohl_ai::Actor>(*entity)
                .ok()
                .map(|actor| actor.health)
        })
        .sum();
    assert!(
        after < before || game.monster_death_count() > 0,
        "two mutually hostile monsters in the same room hurt each other"
    );
}

/// Arbitrary node lattices and arbitrary inputs never panic, and never make
/// the AI state hash unreadable.
mod properties {
    use super::{entities, game_from, monster};
    use ohl_engine::Input;
    use proptest::prelude::*;
    use std::fmt::Write as _;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn an_arbitrary_node_lattice_and_input_never_panics(
            columns in 0i8..5,
            rows in 0i8..5,
            spacing in 1.0f32..192.0,
            steps in 0usize..120,
            forward in -1i8..=1,
            right in -1i8..=1,
            jump in any::<bool>(),
            duck in any::<bool>(),
        ) {
            let mut nodes = String::new();
            for column in 0..columns {
                for row in 0..rows {
                    let _ = write!(
                        nodes,
                        "{{\n\"classname\" \"info_node\"\n\"origin\" \"{} {} 36\"\n}}\n",
                        f32::from(column) * spacing,
                        f32::from(row) * spacing,
                    );
                }
            }
            let block = entities(&format!(
                "{nodes}{}",
                monster("monster_zombie", [128.0, 0.0, 36.0], 180.0, "")
            ));
            let mut game = game_from(&block, false);
            let input = Input {
                forward,
                right,
                jump,
                duck,
                ..Input::default()
            };
            for _ in 0..steps {
                game.tick(ohl_engine::TICK_SECONDS, &input);
            }
            let hash = game.ai_state_hash();
            prop_assert_eq!(hash.len(), 32);
        }
    }
}
