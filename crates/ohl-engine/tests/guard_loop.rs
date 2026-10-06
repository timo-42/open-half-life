//! The guard loop (`ohl_engine::guard`): a player who stands still next to
//! a hostile monster dies, and a player who runs the guard loop instead
//! survives and kills it.
//!
//! Every fixture here is project-authored (`ohl_engine::test_support`); no
//! bytes come from any game installation. See `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    PLAN_SCRIPTED_MAP, PLAN_SCRIPTED_MONSTER_MODEL, ScriptedStart, plan_scripted_goal_bsp,
    plan_scripted_monster_model_bytes,
};
use ohl_engine::{
    AssetSource, Game, Input, MemoryAssets, StartInventoryItem, TICK_SECONDS, guard_input,
};

/// How long each run below stands its ground, in ticks: long enough for a
/// ranged monster to acquire the player and empty far more than a
/// hundred hit points into them.
const GUARD_TICKS: usize = 3_600;

/// The corridor fixture with a monster hostile to the player at its far
/// end, and the loadout a real campaign run would have carried in.
fn game() -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLAN_SCRIPTED_MAP}.bsp"),
        plan_scripted_goal_bsp("ohlplannext", ScriptedStart::ByHostileMonster),
    );
    assets.insert(
        PLAN_SCRIPTED_MONSTER_MODEL,
        plan_scripted_monster_model_bytes(),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, PLAN_SCRIPTED_MAP).expect("the fixture loads");
    game.give_start_inventory(&[
        StartInventoryItem::Weapon(ohl_combat::WeaponId::Mp5),
        StartInventoryItem::Weapon(ohl_combat::WeaponId::Crowbar),
    ]);
    game
}

/// Runs `ticks` of the guard loop, and reports the lowest health the
/// player was ever seen at.
fn guard_for(game: &mut Game, ticks: usize) -> f32 {
    let mut lowest = game.player_health();
    for _ in 0..ticks {
        let input = guard_input(game);
        game.tick(TICK_SECONDS, &input);
        lowest = lowest.min(game.player_health());
    }
    lowest
}

/// Runs `ticks` holding nothing at all — the plain `wait` a route used to
/// end with.
fn wait_for(game: &mut Game, ticks: usize) {
    let input = Input::default();
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &input);
    }
}

/// The mutation this whole feature turns on: standing still through the
/// same stretch of time, with the same loadout, on the same map, kills
/// the player. If this ever stops being true the guard test below is
/// passing for the wrong reason.
#[test]
fn standing_still_next_to_a_hostile_monster_kills_the_player() {
    let mut game = game();
    assert_eq!(game.monster_count(), 1, "the fixture spawns one monster");
    wait_for(&mut game, GUARD_TICKS);
    assert!(
        game.player_health() <= 0.0,
        "a waiting player must die here, or the guard run proves nothing"
    );
    assert_eq!(
        game.monster_death_count(),
        0,
        "a player who never shoots kills nothing"
    );
}

/// The guard loop defends the spot: the player is still alive at the end
/// and the monster is dead.
#[test]
fn the_guard_loop_keeps_the_player_alive_and_kills_the_monster() {
    let mut game = game();
    let lowest = guard_for(&mut game, GUARD_TICKS);
    assert!(
        game.player_health() > 0.0,
        "the guard loop must leave the player alive (lowest health seen: {lowest})"
    );
    assert!(
        game.monster_death_count() > 0,
        "the guard loop must actually kill what is shooting at it"
    );
    assert!(
        game.weapon_fired_count() > 0,
        "the guard loop fires the weapon it selected"
    );
    assert!(
        game.shot_hit_count() > 0,
        "the guard loop aims before it fires"
    );
}

/// The loop is a pure function of the game state, so two runs of it from
/// the same fixture agree tick for tick — the property a replayed route
/// depends on.
#[test]
fn two_guard_runs_from_the_same_state_are_identical() {
    let mut first = game();
    let mut second = game();
    for _ in 0..GUARD_TICKS {
        let first_input = guard_input(&first);
        let second_input = guard_input(&second);
        assert_eq!(first_input, second_input, "the guard loop diverged");
        first.tick(TICK_SECONDS, &first_input);
        second.tick(TICK_SECONDS, &second_input);
        assert_eq!(
            first.ai_state_hash(),
            second.ai_state_hash(),
            "two guarded runs diverged"
        );
    }
    assert!((first.player_health() - second.player_health()).abs() < f32::EPSILON);
}

/// With nothing to fight with, the loop does not stand there and take it:
/// it backs away from the threat, along a heading its own hull traces say
/// is clear, and never ends up inside geometry.
#[test]
fn an_empty_loadout_backs_away_from_the_threat() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLAN_SCRIPTED_MAP}.bsp"),
        plan_scripted_goal_bsp("ohlplannext", ScriptedStart::ByHostileMonster),
    );
    assets.insert(
        PLAN_SCRIPTED_MONSTER_MODEL,
        plan_scripted_monster_model_bytes(),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, PLAN_SCRIPTED_MAP).expect("the fixture loads");
    let monster = game
        .hostile_monster_eyes()
        .first()
        .map(|(_, eye)| *eye)
        .expect("the fixture spawns something hostile");
    let start_distance = monster.distance(glam::Vec3::from_array(game.player_origin()));
    for _ in 0..600 {
        let input = guard_input(&game);
        game.tick(TICK_SECONDS, &input);
        assert!(!game.eye_is_in_solid(), "a retreat never walks into solid");
    }
    let end_distance = monster.distance(glam::Vec3::from_array(game.player_origin()));
    // A real threshold, not just "further away than it started": a
    // monster's own melee knockback shoves a player who does nothing at
    // all about a third of a unit, so `end > start` passes for a guard
    // loop that was never run at all. Half of one probe is two orders of
    // magnitude past that and can only have been covered by walking (the
    // loop as written covers about forty-seven units here).
    assert!(
        end_distance - start_distance > ohl_engine::guard::GUARD_RETREAT_PROBE / 2.0,
        "an unarmed guard retreats half a probe's worth of ground at the very \
         least ({start_distance} -> {end_distance})"
    );
}

/// With nothing hostile on the map at all, the loop stands its ground: no
/// movement key is ever held, so a route that arrived where it meant to
/// does not wander off mid-wait.
#[test]
fn with_nothing_hostile_the_guard_stands_still() {
    let mut assets = MemoryAssets::new();
    assets.insert(
        &format!("maps/{PLAN_SCRIPTED_MAP}.bsp"),
        plan_scripted_goal_bsp("ohlplannext", ScriptedStart::ByTrigger),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, PLAN_SCRIPTED_MAP).expect("the fixture loads");
    let start = game.player_origin();
    for _ in 0..600 {
        let input = guard_input(&game);
        assert_eq!((input.forward, input.right, input.up), (0, 0, 0));
        assert!(!input.jump && !input.attack);
        game.tick(TICK_SECONDS, &input);
    }
    let end = game.player_origin();
    let moved = (0..2)
        .map(|axis| (end[axis] - start[axis]).abs())
        .fold(0.0_f32, f32::max);
    assert!(moved < 1.0, "the guard never walks away from its spot");
}
