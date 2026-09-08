//! A `func_door_rotating` blocks a corridor while closed and, once opened,
//! swings clear of it — proving the rotation this package adds to
//! `ohl_physics::CollisionModel::set_brush_pose` actually blocks and
//! unblocks the player, not just draws differently.
//!
//! `ohl_engine::test_support::rotating_door_bsp`/`rotating_door_entities`
//! build a 192-unit-wide corridor (`y` in `-96..96`) with a real solid door
//! leaf (176 units wide, `y` in `-88..88`) pivoting about its own `y = -88`
//! edge; TWHL wiki `func_door_rotating` (`docs/FORMAT_SOURCES.md`, "Entity
//! keyvalues and map logic") documents the entity's `origin` keyvalue —
//! placed there by a required "origin brush" — as the axis to rotate on.
//! See [`ohl_engine::test_support::ROTATING_DOOR_MINS`]'s own doc comment
//! for why the fixture is sized this generously relative to the standing
//! hull.
//!
//! The open is driven the way a player drives it: the scripted `Input`
//! below presses `use` from where the player spawns, and the engine's own
//! `ohl_game::find_usable_within`/`ohl_engine::USE_RADIUS` proximity check
//! has to find the door for anything to happen. That is only possible
//! because a brush entity's proximity point is now its *placed* centre
//! (`ohl_game::pose::brush_center`: compiled bounds midpoint, plus the
//! `origin` keyvalue, plus its current mover displacement). This fixture's
//! door — like a real map's own `func_door_rotating` — compiles relative
//! to its origin brush (see `rotating_door_bsp`'s own doc comment), so
//! before that fix its proximity point landed near the map's `(0, 0, 0)`
//! and no press from anywhere a player could stand ever reached it; that
//! was `docs/FORMAT_SOURCES.md`'s `TODO(black-box)` item 25, and this test
//! is its regression guard.
//!
//! `a_closed_door_opens_when_the_player_walks_into_it` covers the other way
//! in: a plain, *unnamed* door (no "Use Only" spawnflag, no `targetname`)
//! opening from the player's own hull touching its closed brush while
//! walking, with no `use` press at all
//! (`ohl_game::logic::Simulation::touch_doors`, `docs/FORMAT_SOURCES.md`
//! item 30). It runs against a dedicated
//! `ohl_engine::test_support::rotating_door_unnamed_entities` fixture
//! rather than this file's usual named one, since the cited touch rule
//! ("triggered on touch, unless they have a name") only opens an unnamed
//! door this way. `a_use_only_rotating_door_blocks_the_corridor_forever`
//! is a negative case for the same feature: a *named* door with the "Use
//! Only" flag set never opens from this walk either — proving that flag's
//! own exclusion on top of the (already-excluding) `targetname` it also
//! carries.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{
    ROTATING_DOOR_MAP, ROTATING_DOOR_MAXS, ROTATING_DOOR_MINS, ROTATING_DOOR_NAME,
    rotating_door_bsp, rotating_door_entities, rotating_door_trigger_entities,
    rotating_door_unnamed_entities, rotating_door_use_only_entities,
};
use ohl_engine::{AssetSource, Game, Input, MemoryAssets};
use ohl_game::registry::{Door, MoverState};

const STEP: f32 = 1.0 / 60.0;

fn game() -> Game {
    let bytes = rotating_door_bsp(&rotating_door_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads")
}

fn tick_n(game: &mut Game, n: u32, input: &Input) {
    for _ in 0..n {
        game.tick(STEP, input);
    }
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(ROTATING_DOOR_NAME)
        .first()
        .expect("the fixture declares one named rotating door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// [`door_state`], but for a fixture whose door has no `targetname` (e.g.
/// [`rotating_door_unnamed_entities`]) — `Registry::find` has nothing to
/// look up, so this queries the registry for its one `Door` component
/// directly instead.
fn unnamed_door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    registry
        .world
        .query::<&Door>()
        .iter()
        .next()
        .expect("the fixture declares exactly one door")
        .state
}

/// A door with the "Use Only" spawnflag set fully blocks the corridor
/// forever: walking forward for far longer than it would take to reach it
/// leaves the player short of its closed leaf, not through it, since
/// nothing in this scenario presses `use` and the flag excludes it from
/// [`ohl_game::logic::Simulation::touch_doors`]'s own touch path
/// (`docs/FORMAT_SOURCES.md` item 30).
#[test]
fn a_use_only_rotating_door_blocks_the_corridor_forever() {
    let bytes = rotating_door_bsp(&rotating_door_use_only_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");
    assert_eq!(door_state(&game), MoverState::Closed);

    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    tick_n(&mut game, 600, &forward);

    let x = game.eye_position()[0];
    assert!(
        x < ROTATING_DOOR_MINS[0],
        "the closed Use Only door did not stop the player: eye x = {x}, door starts at x = {}",
        ROTATING_DOOR_MINS[0]
    );
    assert_eq!(
        door_state(&game),
        MoverState::Closed,
        "a Use Only door must not open from the player walking into it"
    );
}

/// A plain door (no "Use Only" spawnflag, no `use` press, no separate
/// `trigger_*` volume) opens the moment the player's own hull, walking
/// forward down the corridor, touches its closed brush — the gap this
/// milestone closes — and the same forward walk that would otherwise be
/// blocked then carries the player through the doorway and past it, the
/// real-`use_pressed`-free counterpart of
/// [`an_open_door_lets_the_player_walk_through`].
#[test]
fn a_closed_door_opens_when_the_player_walks_into_it() {
    // Unnamed: the Sven Co-op wiki's `Func_door` page's own touch rule
    // ("triggered on touch, unless they have a name") only opens an
    // unnamed door this way (`docs/FORMAT_SOURCES.md` item 30); the plain
    // `rotating_door_entities` fixture every other test in this file uses
    // carries a `targetname` and is deliberately excluded, so it cannot
    // stand in for this test.
    let bytes = rotating_door_bsp(&rotating_door_unnamed_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");
    assert_eq!(unnamed_door_state(&game), MoverState::Closed);

    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    tick_n(&mut game, 600, &forward);

    assert_ne!(
        unnamed_door_state(&game),
        MoverState::Closed,
        "walking into the door's own brush never opened it"
    );
    let x = game.eye_position()[0];
    assert!(
        x > ROTATING_DOOR_MAXS[0],
        "the touch-opened door still blocked the player: eye x = {x}, door ends at x = {}",
        ROTATING_DOOR_MAXS[0]
    );
    assert_eq!(
        game.doors_opened_count(),
        1,
        "the touch open should be counted the same way a use-press open is"
    );
}

/// A door opened by a `use` press from the player's own spawn — through
/// the real proximity path, nothing forced — swings clear of the corridor,
/// and the same forward walk that was blocked above now carries the player
/// through the doorway and past it.
#[test]
fn an_open_door_lets_the_player_walk_through() {
    let mut game = game();
    assert_eq!(door_state(&game), MoverState::Closed);

    let use_press = Input {
        use_pressed: true,
        ..Input::default()
    };
    game.tick(STEP, &use_press);
    assert_ne!(
        door_state(&game),
        MoverState::Closed,
        "a `use` press from the spawn point did not reach the door"
    );

    // `wait = -1`, so once it is open it stays open; let the quarter turn
    // finish before walking into the doorway.
    let idle = Input::default();
    tick_n(&mut game, 60, &idle);
    assert_eq!(door_state(&game), MoverState::Open);

    let forward = Input {
        forward: 1,
        ..Input::default()
    };
    tick_n(&mut game, 600, &forward);

    let x = game.eye_position()[0];
    assert!(
        x > ROTATING_DOOR_MAXS[0],
        "the open door still blocked the player: eye x = {x}, door ends at x = {}",
        ROTATING_DOOR_MAXS[0]
    );
}

/// The counter behind the scripted-input milestone line ("The player
/// opened a door.", `crates/ohl-app/src/script_log.rs`) counts that same
/// press, and counts nothing when the press happens out of reach.
#[test]
fn a_use_press_that_opens_a_door_is_counted() {
    let mut game = game();
    assert_eq!(game.doors_opened_count(), 0);

    let use_press = Input {
        use_pressed: true,
        ..Input::default()
    };
    game.tick(STEP, &use_press);
    assert_eq!(game.doors_opened_count(), 1);

    // A second press on an already-open door is not a second opening.
    game.tick(STEP, &use_press);
    assert_eq!(game.doors_opened_count(), 1);
}

/// A rotating door opened by the player swings *away* from them.
///
/// TWHL wiki `func_door_rotating` (`docs/FORMAT_SOURCES.md` items 24 and
/// 26): "the door will always open away from the player" unless the "One
/// Way" spawnflag is set. The fixture's door pivots about its `y = -88`
/// edge with the default `+Z` axis and a positive `distance`, which alone
/// would sweep its leaf across the corridor *toward* the player standing
/// at `x = 150`; opened through the engine's own touch-trigger phase — the
/// only path that also feeds the player's position in as the activator
/// origin — it must instead flip to `-Z` and swing the other way, leaving
/// the player where they stood and never inside solid geometry.
#[test]
fn a_rotating_door_opened_by_the_player_swings_away_from_them() {
    let bytes = rotating_door_bsp(&rotating_door_trigger_entities());
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{ROTATING_DOOR_MAP}.bsp"), bytes);
    let mut game =
        Game::load(&assets as &dyn AssetSource, ROTATING_DOOR_MAP).expect("the fixture loads");

    let start = game.eye_position();
    // The door needs a quarter second to swing (90 degrees at 360
    // degrees/second); ride out a full second of standing still, checking
    // on every step that the swing never leaves the player in solid.
    for step in 0..100 {
        game.tick(STEP, &Input::default());
        assert!(
            !game.eye_is_in_solid(),
            "step {step}: the opening door left the player inside solid geometry at {:?}",
            game.eye_position()
        );
    }

    let entity = *game
        .registry()
        .find(ROTATING_DOOR_NAME)
        .first()
        .expect("the fixture declares one named rotating door");
    let door = *game
        .registry()
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door");
    assert_eq!(
        door.state,
        MoverState::Open,
        "the touch trigger did not open the door"
    );
    assert_eq!(
        door.rotation_axis,
        Some(-glam::Vec3::Z),
        "the door did not flip its swing away from the activator"
    );

    // Untouched by the swing: a door opening away from the player neither
    // pushes them back down the corridor nor drags them along with it.
    let end = game.eye_position();
    let moved = ((end[0] - start[0]).powi(2) + (end[1] - start[1]).powi(2)).sqrt();
    assert!(
        moved < 8.0,
        "the opening door displaced the player: {start:?} -> {end:?}"
    );
}
