//! `func_monsterclip` must actually block *monster* sensing/attack, not
//! just fail to block the player.
//!
//! A PR #129 review round found that item 33's own citation for the
//! nav-graph build and the attack hit-trace switching to
//! `Level::monster_collision` had no test behind it: pointing either of
//! those two `ai.rs` call sites back at the player's own model changed
//! nothing observable in any scenario or test in this repo, only
//! `SightContext` (sensing/movement/steering) was actually exercised.
//! This file adds two kinds of coverage:
//!
//! - A direct, low-level check that a straight trace through a
//!   `func_monsterclip` fence is blocked on `Game::monster_collision` (the
//!   same model `ai.rs::resolve_attack` traces against for its own attack
//!   hit-trace) and not blocked on `Game::collision` (the player's).
//! - A behavioural, end-to-end check with a real `monster_human_grunt`
//!   (a ranged attacker, so it does not need to path anywhere to kill the
//!   player — it only needs line of sight within its own attack range):
//!   fenced off by a `func_monsterclip`, it must never land a hit; the
//!   same fixture with the fence *removed* is the control, proving the
//!   fixture is not simply one where the monster can never attack at all
//!   (this control was necessary: an earlier version of this fixture
//!   placed the monster somewhere the grunt never noticed the player even
//!   with no fence at all, which would have made the fenced test pass for
//!   the wrong reason).
//!
//! What isolating each of `ai.rs`'s three sites individually against this
//! fixture actually found (each reverted alone, the other two left
//! correct, `cargo test --test monsterclip_blocks_monster` re-run each
//! time): reverting the static navigation-graph build (`nav::build`)
//! alone changes nothing observable — `SightContext`'s own
//! movement/steering already fully gates whether a monster ever gets
//! anywhere nav-graph waypoints would matter for, in every fixture and
//! scenario this project has today, matching the PR review's own finding
//! against the real "Power Up" map. Reverting `SightContext` alone, or
//! the attack hit-trace alone, *also* leaves this fixture's behavioural
//! test passing (the grunt still never lands a hit) — the two are
//! redundant on this exact fixture, since either the sensing gate or the
//! attack trace's own re-check independently stops the shot. Reverting
//! *both together* (leaving only `nav::build` correct) does reproduce the
//! regression (`a_monsterclip_stops_the_grunt_from_ever_hitting_the_player`
//! fails, matching the real PR #129 review finding of `"The player
//! died."` on the unfenced-for-monsters build). So this fixture's own
//! behavioural test is a guard against losing *both* sites at once, not
//! a guard against losing either one alone; the low-level trace test
//! above is what independently pins the attack-trace site's own data
//! (`Game::monster_collision` itself correctly blocks the fence), and
//! `docs/FORMAT_SOURCES.md` item 33 records this reading plainly rather
//! than leaving it implied by an untested code comment.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{ROTATING_DOOR_MAP, ROTATING_DOOR_PIVOT, rotating_door_bsp};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_physics::Hull;

/// A player start short of the fence (`x = 150`) and one
/// `monster_human_grunt` at `x = 280`, facing back toward the player —
/// well within a grunt's own ranged attack distance in a straight,
/// unobstructed corridor. `with_fence` controls whether a
/// `func_monsterclip` (submodel `*1`, spanning
/// `ohl_engine::test_support::ROTATING_DOOR_MINS`..`ROTATING_DOOR_MAXS`,
/// the same brush `corridor_brush_entities` builds for a caller-chosen
/// classname) sits between them.
fn entities(with_fence: bool) -> String {
    let fence = if with_fence {
        format!(
            "{{\n\"classname\" \"func_monsterclip\"\n\"model\" \"*1\"\n\
             \"origin\" \"{} {} {}\"\n}}\n",
            ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
        )
    } else {
        String::new()
    };
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {fence}\
         {{\n\"classname\" \"monster_human_grunt\"\n\"origin\" \"280 0 40\"\n\
         \"angle\" \"180\"\n}}\n"
    )
}

fn game(with_fence: bool) -> Game {
    let bytes = rotating_door_bsp(&entities(with_fence));
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(ohl_engine::TICK_SECONDS, input);
    }
}

/// The low-level attack-hit-trace channel: a straight trace from the
/// player's own position to the monster's, through the fence, must be
/// blocked on `Game::monster_collision` (see `docs/FORMAT_SOURCES.md`
/// item 33) and must *not* be blocked on `Game::collision`, since
/// `func_monsterclip` is documented non-solid to the player — the same
/// distinction `monsterclip_corridor.rs`'s own player-side test already
/// proves end to end for player movement, checked here directly for a
/// trace instead.
#[test]
fn a_monsterclip_blocks_a_trace_on_the_monster_model_but_not_the_player_model() {
    let game = game(true);
    let muzzle = glam::Vec3::new(150.0, 0.0, 40.0);
    let aim = glam::Vec3::new(280.0, 0.0, 40.0);

    let monster_trace = game
        .monster_collision()
        .expect("the fixture has usable collision hulls")
        .trace(Hull::Standing, muzzle, aim);
    assert!(
        monster_trace.blocked(),
        "the monster-side collision model must block a trace straight \
         through the func_monsterclip fence: fraction = {}",
        monster_trace.fraction
    );

    let player_trace = game
        .collision()
        .expect("the fixture has usable collision hulls")
        .trace(Hull::Standing, muzzle, aim);
    assert!(
        !player_trace.blocked(),
        "sanity check: the same trace on the player's own model must pass \
         straight through, exactly like the corridor walkability test"
    );
}

/// The control: with no fence at all, a `monster_human_grunt` this close
/// (130 units, well inside its own ranged attack distance) reliably kills
/// the player within a short tick budget — proving the fixture itself
/// (not the fence) is what the fenced test below depends on.
#[test]
fn without_a_fence_the_grunt_kills_the_player() {
    let mut game = game(false);
    assert_eq!(game.monster_count(), 1);

    tick_n(&mut game, 1_200, &Input::default());

    assert!(
        game.player_health() <= 0.0,
        "the unfenced control must let the grunt kill the player: health \
         is {}",
        game.player_health()
    );
}

/// The behavioural regression guard: with the fence in place, the same
/// grunt, at the same range, over the same tick budget the control above
/// needed to land a kill, must never land a single hit.
#[test]
fn a_monsterclip_stops_the_grunt_from_ever_hitting_the_player() {
    let mut game = game(true);
    assert_eq!(game.monster_count(), 1);

    tick_n(&mut game, 1_200, &Input::default());

    assert!(
        (game.player_health() - 100.0).abs() < f32::EPSILON,
        "a func_monsterclip-fenced grunt must never land an attack on the \
         player: health is {} (the unfenced control kills the player well \
         within this same tick budget)",
        game.player_health()
    );
}
