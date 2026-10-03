//! What a mover does with whoever is in its way: a door that closes on the
//! player pushes them until it cannot, then reverses and deals its `dmg`;
//! a monster walking a route through a touch-eligible door opens it the
//! way the player does — unless the door's "Monsters Can't" spawnflag says
//! otherwise.
//!
//! Both fixtures reuse `ohl_engine::test_support::rotating_door_bsp`'s
//! corridor (192 units wide, `y` in `-96..96`, unbounded along `x`) with a
//! translating `func_door` as its solid submodel `*1` instead of the
//! rotating one: the leaf spans `ROTATING_DOOR_MINS..ROTATING_DOOR_MAXS`
//! at rest (`x` in `184..200`, the corridor's full width bar an 8-unit gap
//! each side) and slides along `+y` to open, since its `angle` is `90`
//! (`ohl_game::registry::movedir_from_angles`). The keyvalues are the
//! published `func_door` ones (`speed`, `wait`, `lip`, `dmg`, `angle`,
//! `spawnflags`), and the blocked-door behaviour asserted here is the Sven
//! Co-op wiki's `Func_door` page's — see `docs/FORMAT_SOURCES.md`, "Mover
//! blocking, branching paths and monster-opened doors".
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_MAXS, ROTATING_DOOR_MINS, ROTATING_DOOR_PIVOT, actor_origin,
    entity_block, entity_of_classname, queue_monster_damage, rotating_door_bsp,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets, TICK_SECONDS};
use ohl_game::registry::{Door, MoverState, SPAWNFLAG_DOOR_MONSTERS_CANT};

/// The door's `dmg` keyvalue: what one block costs the player.
const DOOR_DMG: f32 = 5.0;

/// A `worldspawn`, an `info_player_start` at `x = 150` facing `+x`, and
/// the sliding door described in the module doc: 100 units/second, a
/// one-second `wait`, `lip` zero so it clears the corridor fully, and
/// `dmg` [`DOOR_DMG`]. Unnamed, so the player's own touch opens it.
fn sliding_door_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"90\"\n\
         \"speed\" \"100\"\n\"wait\" \"1\"\n\"lip\" \"0\"\n\"dmg\" \"{DOOR_DMG}\"\n\
         \"origin\" \"{} {} {}\"\n}}\n",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

fn game_with(entities: &str) -> Game {
    let bytes = rotating_door_bsp(entities);
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads")
}

fn door_state(game: &Game) -> MoverState {
    game.registry()
        .world
        .query::<&Door>()
        .iter()
        .next()
        .expect("the fixture declares exactly one door")
        .state
}

fn walk_forward(game: &mut Game, ticks: u32) {
    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &forward);
    }
}

fn stand(game: &mut Game, ticks: u32) {
    for _ in 0..ticks {
        game.tick(TICK_SECONDS, &Input::default());
    }
}

/// The player walks into the closed door (opening it by touch) and is
/// steered to a stop in the doorway. When the door's `wait` runs out it
/// slides back across the corridor: first it *pushes* the player along
/// ahead of it, then — with the corridor wall behind them — it cannot, and
/// is blocked. The cited rule: it deals its `dmg` and moves back the way
/// it came, so the player is neither trapped inside it nor left embedded,
/// and the door tries again once its `wait` runs out — blocking, and
/// dealing its `dmg`, once per attempt rather than once per tick.
#[test]
fn a_door_closing_on_the_player_pushes_them_then_reverses_and_deals_its_dmg() {
    let mut game = game_with(&sliding_door_entities());
    assert_eq!(door_state(&game), MoverState::Closed);

    // Walk into the door until the player's origin reaches its near face
    // — the door has to open first, which the walk itself triggers — then
    // steer back and forth to a stop in the doorway, since a walk released
    // here slides on well past it. The door is still opening, or open and
    // waiting, for the whole of this.
    let doorway_x = f32::midpoint(ROTATING_DOOR_MINS[0], ROTATING_DOOR_MAXS[0]);
    let mut walked = 0;
    while game.player_origin()[0] < ROTATING_DOOR_MINS[0] && walked < 600 {
        walk_forward(&mut game, 1);
        walked += 1;
    }
    assert!(
        matches!(door_state(&game), MoverState::Opening | MoverState::Open),
        "walking into the door opened it: {:?}",
        door_state(&game)
    );
    for _ in 0..120 {
        let x = game.player_origin()[0];
        let forward = if x < doorway_x - 2.0 {
            1
        } else if x > doorway_x + 2.0 {
            -1
        } else {
            0
        };
        game.tick(
            TICK_SECONDS,
            &Input {
                forward,
                ..Input::default()
            },
        );
    }
    assert!(
        (game.player_origin()[0] - doorway_x).abs() < 16.0,
        "the player stands in the doorway: origin {:?}",
        game.player_origin()
    );
    assert!(
        matches!(door_state(&game), MoverState::Opening | MoverState::Open),
        "the door is still open while they settle: {:?}",
        door_state(&game)
    );
    let health_before = game.player_health();
    let y_before = game.player_origin()[1];

    // Stand there while the door finishes opening, waits, closes onto the
    // player, pushes them to the wall, is blocked, reverses, and starts
    // its next attempt: about a second of each at 100 units/second, at
    // 100 ticks a second.
    let mut saw_closing = false;
    let mut saw_reopen = false;
    let mut pushed_to_y = y_before;
    for _ in 0..700 {
        stand(&mut game, 1);
        match door_state(&game) {
            MoverState::Closing => saw_closing = true,
            MoverState::Opening | MoverState::Open if saw_closing => saw_reopen = true,
            _ => {}
        }
        pushed_to_y = pushed_to_y.min(game.player_origin()[1]);
        assert!(
            !game.eye_is_in_solid(),
            "the player is never left embedded in the door: origin {:?}, door {:?}",
            game.player_origin(),
            door_state(&game)
        );
    }
    assert!(
        saw_closing,
        "the door's wait ran out and it started closing"
    );
    assert!(
        pushed_to_y < y_before - 40.0,
        "the closing leaf pushed the player along ahead of it: y {y_before} -> {pushed_to_y}"
    );
    assert!(
        saw_reopen,
        "blocked against the corridor wall, the door reversed: state {:?}",
        door_state(&game)
    );
    let blocks = game.player_damage_event_count();
    assert!(blocks >= 1, "the blocked door dealt its dmg");
    let lost = health_before - game.player_health();
    #[allow(clippy::cast_precision_loss)]
    let expected = blocks as f32 * DOOR_DMG;
    assert!(
        (lost - expected).abs() < 1e-3,
        "each block costs exactly the door's dmg: {blocks} blocks, lost {lost}"
    );
    assert!(
        blocks <= 3,
        "a reversed door deals its dmg once per blocked attempt, not once per tick: {blocks}"
    );
}

/// A `monster_barney` with a walking `scripted_sequence` on the far side
/// of the door: the route runs straight through the closed leaf, and the
/// monster's own hull touching it opens it, so it reaches its mark. With
/// the "Monsters Can't" spawnflag on the door, the same route ends at the
/// closed leaf. The player starts well behind the guard, facing away, so
/// nothing but the guard ever touches the door.
fn monster_route_entities(door_spawnflags: u32) -> String {
    let script = entity_block(
        "scripted_sequence",
        [320.0, 0.0, 36.0],
        0.0,
        &[
            ("targetname", "ohl_script"),
            ("m_iszEntity", "ohl_guard"),
            ("m_iszPlay", "ohl_action"),
            ("m_iszIdle", "ohl_wait"),
            ("m_fMoveTo", "1"),
        ],
    );
    let guard = entity_block(
        "monster_barney",
        [60.0, 0.0, 36.0],
        0.0,
        &[("targetname", "ohl_guard")],
    );
    let start = entity_block(
        "trigger_auto",
        [0.0, 0.0, 0.0],
        0.0,
        &[("target", "ohl_script")],
    );
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-200 0 40\"\n\
         \"angle\" \"180\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"90\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n\"lip\" \"0\"\n\
         \"spawnflags\" \"{door_spawnflags}\"\n\
         \"origin\" \"{} {} {}\"\n}}\n{guard}{script}{start}",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

/// How long the guard is given to walk its route: 260 units at a walking
/// pace, plus the door's own open, with plenty to spare.
const ROUTE_TICKS: u32 = 1_800;

#[test]
fn a_monster_walking_a_route_through_a_door_opens_it() {
    let mut game = game_with(&monster_route_entities(0));
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    assert_eq!(door_state(&game), MoverState::Closed);

    stand(&mut game, ROUTE_TICKS);

    assert!(
        matches!(door_state(&game), MoverState::Opening | MoverState::Open),
        "the guard's own touch opened the door: {:?}",
        door_state(&game)
    );
    let arrived = actor_origin(&game, guard);
    assert!(
        arrived.x > ROTATING_DOOR_MAXS[0],
        "the guard walked through the doorway to its mark: {arrived:?}"
    );
    assert_eq!(
        game.doors_opened_count(),
        1,
        "the monster's open is counted like the player's"
    );
    assert!(
        (game.player_health() - 100.0).abs() < f32::EPSILON,
        "nothing here touched the player"
    );
}

#[test]
fn a_monsters_cant_door_stays_closed_against_a_monsters_route() {
    let mut game = game_with(&monster_route_entities(SPAWNFLAG_DOOR_MONSTERS_CANT));
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");

    stand(&mut game, ROUTE_TICKS);

    assert_eq!(
        door_state(&game),
        MoverState::Closed,
        "\"Monsters Can't\": the guard's touch does not move the door"
    );
    let stopped = actor_origin(&game, guard);
    assert!(
        stopped.x < ROTATING_DOOR_MINS[0],
        "the guard is held short of the closed leaf: {stopped:?}"
    );
    assert_eq!(game.doors_opened_count(), 0);
}

/// Where the guard of [`monster_in_doorway_entities`] is walked to. A
/// scripted walk ends a little short of its mark (the guard stops about 24
/// units before it), so the mark sits that far past the leaf's own middle,
/// `x = 192`, to leave the guard standing across the leaf's thickness. It
/// is off the corridor's centre line toward `-y`, the side the closing
/// leaf pushes toward, so the leaf's leading face reaches the guard
/// part-way through its close, pushes it, and runs it into the corridor's
/// `y = -96` wall with travel still left.
const DOORWAY_MARK: [f32; 3] = [216.0, -40.0, 36.0];

/// The door's `wait` in [`monster_in_doorway_entities`]: long enough for
/// the guard to finish its walk into the doorway before the leaf closes.
const DOORWAY_WAIT: &str = "4";

/// A `monster_barney` walked by its `scripted_sequence` from the near side
/// of the corridor to [`DOORWAY_MARK`], through the closed sliding door —
/// its own touch opens it, as in [`monster_route_entities`] — and left
/// standing there. The door is unnamed, so nothing but that touch moves
/// it, and carries `dmg` [`DOOR_DMG`].
fn monster_in_doorway_entities() -> String {
    let script = entity_block(
        "scripted_sequence",
        DOORWAY_MARK,
        0.0,
        &[
            ("targetname", "ohl_script"),
            ("m_iszEntity", "ohl_guard"),
            ("m_iszPlay", "ohl_action"),
            ("m_iszIdle", "ohl_wait"),
            ("m_fMoveTo", "1"),
        ],
    );
    let guard = entity_block(
        "monster_barney",
        [60.0, -40.0, 36.0],
        0.0,
        &[("targetname", "ohl_guard")],
    );
    let start = entity_block(
        "trigger_auto",
        [0.0, 0.0, 0.0],
        0.0,
        &[("target", "ohl_script")],
    );
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-200 0 40\"\n\
         \"angle\" \"180\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"90\"\n\
         \"speed\" \"200\"\n\"wait\" \"{DOORWAY_WAIT}\"\n\"lip\" \"0\"\n\
         \"dmg\" \"{DOOR_DMG}\"\n\
         \"origin\" \"{} {} {}\"\n}}\n{guard}{script}{start}",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

fn actor_health(game: &Game, entity: ohl_game::hecs::Entity) -> f32 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .map(|actor| actor.health)
        .expect("the guard has an actor")
}

/// The monster half of the blocked-mover rule. The guard opens the door by
/// walking into it and stops in the doorway; when the door's `wait` runs
/// out the leaf closes onto it. Phase 12 pushes the guard along ahead of
/// the leaf — nothing else ever moves a monster out of a mover's way — and
/// once the corridor wall is behind it the door is blocked: it reverses,
/// and the guard (not the player, who is nowhere near) takes the door's
/// `dmg`, once per blocked attempt.
#[test]
fn a_door_closing_on_a_monster_pushes_it_then_reverses_and_deals_it_the_dmg() {
    let mut game = game_with(&monster_in_doorway_entities());
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    let health_before = actor_health(&game, guard);

    let mut saw_closing = false;
    let mut saw_reopen = false;
    let mut arrived_y = None;
    let mut pushed_to_y = f32::INFINITY;
    for _ in 0..ROUTE_TICKS {
        stand(&mut game, 1);
        let state = door_state(&game);
        let origin = actor_origin(&game, guard);
        match state {
            MoverState::Closing => {
                if !saw_closing {
                    arrived_y = Some(origin.y);
                }
                saw_closing = true;
            }
            MoverState::Opening | MoverState::Open if saw_closing => saw_reopen = true,
            _ => {}
        }
        if saw_closing {
            pushed_to_y = pushed_to_y.min(origin.y);
        }
    }

    let arrived_y = arrived_y.expect("the door's wait ran out and it started closing");
    let at_mark = actor_origin(&game, guard);
    assert!(
        (at_mark.x - 192.0).abs() < 8.0,
        "the guard stands across the leaf's path: {at_mark:?}"
    );
    assert!(
        pushed_to_y < arrived_y - 20.0,
        "the closing leaf pushed the guard along ahead of it: y {arrived_y} -> {pushed_to_y}"
    );
    assert!(
        saw_reopen,
        "blocked with the guard against the corridor wall, the door reversed"
    );
    let lost = health_before - actor_health(&game, guard);
    assert!(lost > 0.0, "the blocked door dealt the guard its dmg");
    let attempts = (lost / DOOR_DMG).round();
    assert!(
        (lost - attempts * DOOR_DMG).abs() < 1e-3 && attempts <= 4.0,
        "each blocked attempt costs exactly the door's dmg, once: lost {lost}"
    );
    assert_eq!(
        game.player_damage_event_count(),
        0,
        "the player was never in the door's way"
    );
}

/// A `monster_barney` spawned facing the closed sliding door with its hull
/// inside the door's touch margin but clear of the leaf itself: its touch
/// opens the door on the first tick it is alive for. The player is far
/// behind it, so nothing else touches the door.
fn monster_at_the_door_entities() -> String {
    // The hull's `+x` face sits two units short of the leaf's near face,
    // inside `ohl_game::logic`'s four-unit door touch margin.
    let guard = entity_block(
        "monster_barney",
        [ROTATING_DOOR_MINS[0] - 16.0 - 2.0, 0.0, 36.0],
        0.0,
        &[("targetname", "ohl_guard")],
    );
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-200 0 40\"\n\
         \"angle\" \"180\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"model\" \"*1\"\n\"angle\" \"90\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n\"lip\" \"0\"\n\
         \"origin\" \"{} {} {}\"\n}}\n{guard}",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

/// A corpse does not open doors: the same guard at the same spot, dead
/// before the first tick, leaves the door closed — with the living guard
/// as the control.
#[test]
fn a_dead_monster_at_a_door_does_not_open_it() {
    let mut living = game_with(&monster_at_the_door_entities());
    stand(&mut living, 10);
    assert!(
        matches!(door_state(&living), MoverState::Opening | MoverState::Open),
        "the control: a living guard's touch opens the door"
    );

    let mut game = game_with(&monster_at_the_door_entities());
    let guard = entity_of_classname(&game, "monster_barney").expect("the guard spawned");
    // Exactly its own health — enough to kill it, not enough to gib it —
    // applied by the first tick's lifecycle phase, which runs before the
    // map-logic phase that does the touching.
    let health = actor_health(&game, guard);
    queue_monster_damage(&mut game, guard, None, health);
    stand(&mut game, 10);
    assert!(
        game.registry()
            .world
            .get::<&ohl_ai::Actor>(guard)
            .is_ok_and(|actor| !actor.alive),
        "the guard died and its corpse is still there to be tested against"
    );
    assert_eq!(
        door_state(&game),
        MoverState::Closed,
        "a dead guard's hull does not touch the door open"
    );
}
