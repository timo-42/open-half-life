//! Map entities the registry's fallthrough used to swallow, driven end to
//! end through the real [`Game`] loop.
//!
//! `crates/ohl-game/src/logic.rs`'s own unit tests already pin each
//! entity's state machine in isolation (the conveyor's sign flip, the
//! wall's visible/solid flag, the strip and end-section events). What only
//! an engine test can show is the half that lives outside `ohl-game`: a
//! conveyor's surface velocity actually reaching the walking player through
//! `Level::brush_ride_velocity`, a switched-off `func_wall_toggle` actually
//! ceasing to be solid in both `ohl_physics::CollisionModel`s, the strip
//! actually emptying the engine's own inventory and ammo ledgers, a
//! `trigger_endsection` actually surfacing as a [`GameEvent`] the host can
//! act on, a `weaponbox` stocking what its own keyvalues name, and an
//! `item_security` firing its `target` when it is picked up. A plain
//! `func_button` with `health` (already implemented, but only ever tested
//! as a `func_rot_button`) is covered here too.
//!
//! Every fixture is a void world with one brush-entity floor
//! (`killable_brush_floor_bsp`, the same shape
//! `brush_entity_collision.rs`/`killtarget_brush_despawn.rs` already use)
//! or the button room `rot_button.rs` uses. No bytes here come from any
//! game installation; see `docs/CLEAN_ROOM.md`.

use std::fmt::Write as _;

use ohl_combat::{AmmoType, WeaponId};
use ohl_engine::test_support::{
    ROT_BUTTON_CENTER, WALL_TOGGLE_BLOCK_MAX, killable_brush_floor_bsp, rot_button_bsp,
    wall_toggle_block_bsp,
};
use ohl_engine::{AssetSource, Game, GameEvent, Input, MemoryAssets, StartInventoryItem};
use ohl_formats::test_support::BRUSH_FLOOR_TOP_Z;
use ohl_game::registry::{Door, MoverState, WallToggle};

const STEP: f32 = 1.0 / 60.0;

/// The floor entity's `targetname` in every fixture below that names one.
const FLOOR_NAME: &str = "ohl_floor";

fn game_from(map: &str, bytes: Vec<u8>) -> Game {
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{map}.bsp"), bytes);
    Game::load(&assets as &dyn AssetSource, map).expect("the fixture loads")
}

fn tick_n(game: &mut Game, steps: u32, input: &Input) -> Vec<GameEvent> {
    let mut events = Vec::new();
    for _ in 0..steps {
        events.extend(game.tick(STEP, input));
    }
    events
}

/// A `worldspawn`, an `info_player_start` above the slab, and the slab
/// itself declared as `classname` with `keys` appended.
fn floor_entities(classname: &str, keys: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"{classname}\"\n\"targetname\" \"{FLOOR_NAME}\"\n\
         \"model\" \"*1\"\n{keys}}}\n"
    )
}

/// A `trigger_auto` that fires `target` after `delay` seconds. The
/// documented "Remove On fire" flag is left off, so the same fixture can
/// declare two of them and have both fire.
fn auto_trigger(target: &str, delay: f32) -> String {
    format!(
        "{{\n\"classname\" \"trigger_auto\"\n\"target\" \"{target}\"\n\
         \"delay\" \"{delay}\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// `func_conveyor`
// ---------------------------------------------------------------------

/// The conveyor fixture's push speed, in units per second. Well under the
/// slab's own 128-unit half-extent per second of travel, so a second of
/// carry never runs the player off the belt's own floor.
const CONVEYOR_SPEED: f32 = 60.0;

fn conveyor_game(map: &str, extra: &str) -> Game {
    let entities = format!(
        "{}{extra}",
        floor_entities(
            "func_conveyor",
            &format!("\"speed\" \"{CONVEYOR_SPEED}\"\n\"angle\" \"90\"\n"),
        )
    );
    game_from(map, killable_brush_floor_bsp(&entities))
}

/// A player standing still on a `func_conveyor` is carried along it: the
/// published "speed of push" reaches them through the very same
/// `PlayerController::base_velocity` seam a moving `func_train` already
/// rides on, even though the conveyor's own brush never moves an inch.
#[test]
fn a_conveyor_carries_a_player_who_is_standing_still_on_it() {
    let mut game = conveyor_game("ohlconveyorsynth", "");
    // Let the player settle onto the slab first: an airborne player has no
    // ground brush, and a conveyor only carries what stands on it.
    tick_n(&mut game, 30, &Input::default());
    assert!(
        game.player_on_ground(),
        "the player must land on the conveyor before it can carry them"
    );
    let start = game.player_origin();

    tick_n(&mut game, 60, &Input::default());
    let end = game.player_origin();
    let carried = end[1] - start[1];
    assert!(
        carried > CONVEYOR_SPEED * 0.5,
        "one second on a {CONVEYOR_SPEED} u/s belt must carry the player well along +Y, moved {carried}"
    );
    assert!(
        (end[0] - start[0]).abs() < 4.0,
        "an `angle 90` belt pushes along +Y only"
    );
    assert!(
        (end[2] - BRUSH_FLOOR_TOP_Z).abs() < 64.0,
        "the ride must not launch the player off the slab"
    );
    assert!(
        game.ground_mover_speed() > 0.0,
        "a carried player is riding a mover, and the engine must report it as one"
    );
}

/// "Triggering a `func_conveyor` will negate the speed thus pushing in the
/// opposite direction": the same fixture, with a `trigger_auto` switching
/// the belt a second in, must carry the player back the other way.
#[test]
fn triggering_a_conveyor_reverses_the_direction_it_carries_the_player() {
    let mut game = conveyor_game("ohlconveyorrevsynth", &auto_trigger(FLOOR_NAME, 1.0));
    tick_n(&mut game, 30, &Input::default());
    let start = game.player_origin();
    // Up to the switch: carried along +Y.
    tick_n(&mut game, 45, &Input::default());
    let at_switch = game.player_origin();
    assert!(
        at_switch[1] > start[1],
        "the belt runs +Y until it is switched"
    );

    // Past the switch: carried back along -Y, far enough to undo the
    // outbound travel rather than merely stopping.
    tick_n(&mut game, 90, &Input::default());
    let after = game.player_origin();
    assert!(
        after[1] < at_switch[1] - CONVEYOR_SPEED * 0.5,
        "after the switch the belt must carry the player back along -Y"
    );
}

/// A `func_conveyor` with the published "No push (1)" spawnflag has its push
/// disabled: it is still a solid floor, and it carries nobody.
#[test]
fn a_no_push_conveyor_is_a_floor_and_nothing_more() {
    let entities = floor_entities(
        "func_conveyor",
        &format!("\"speed\" \"{CONVEYOR_SPEED}\"\n\"angle\" \"90\"\n\"spawnflags\" \"1\"\n"),
    );
    let mut game = game_from(
        "ohlconveyornopushsynth",
        killable_brush_floor_bsp(&entities),
    );
    tick_n(&mut game, 30, &Input::default());
    assert!(
        game.player_on_ground(),
        "a cosmetic conveyor is still solid"
    );
    let start = game.player_origin();
    tick_n(&mut game, 60, &Input::default());
    let end = game.player_origin();
    assert!(
        (end[1] - start[1]).abs() < 1.0,
        "a No push conveyor must not carry the player at all"
    );
}

/// A conveyor carries what stands *on* it; it is not a mover closing on a
/// player caught *inside* it. The player-move phase pushes an embedded
/// player clear at the velocity of the brush that moved into them, and a
/// conveyor's brush never moves: a player spawned inside the belt is
/// neither shoved along it nor reported as blocking it, which is what the
/// push records when a mover that *is* moving cannot push someone clear.
#[test]
fn a_conveyor_does_not_shove_a_player_caught_inside_it() {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 {}\"\n}}\n\
         {{\n\"classname\" \"func_conveyor\"\n\"model\" \"*1\"\n\
         \"speed\" \"{CONVEYOR_SPEED}\"\n\"angle\" \"90\"\n}}\n",
        BRUSH_FLOOR_TOP_Z - 8.0
    );
    let mut game = game_from(
        "ohlconveyorembeddedsynth",
        killable_brush_floor_bsp(&entities),
    );
    assert!(
        game.position_is_in_solid(game.player_origin()),
        "the fixture embeds the player in the belt"
    );
    let start = game.player_origin();
    for _ in 0..60 {
        game.tick(STEP, &Input::default());
        assert!(
            game.movers_blocked().is_empty(),
            "a conveyor is not a mover closing on the player"
        );
    }
    let end = game.player_origin();
    assert!(
        (end[1] - start[1]).abs() < CONVEYOR_SPEED * 0.25,
        "an embedded player was carried along the belt: moved {}",
        end[1] - start[1]
    );
}

// ---------------------------------------------------------------------
// `func_wall_toggle`
// ---------------------------------------------------------------------

/// A point comfortably inside the fixture slab's own compiled box (the
/// slab is 16 units thick, ending at [`BRUSH_FLOOR_TOP_Z`]), so
/// [`Game::position_is_in_solid`] reads the wall's solidity directly rather
/// than inferring it from where the player ended up.
fn inside_the_slab() -> [f32; 3] {
    [0.0, 0.0, BRUSH_FLOOR_TOP_Z - 8.0]
}

/// Switched off, a `func_wall_toggle` is "non-solid" (VDC GoldSrc, through a
/// search-engine result summary); switched on again it is an ordinary wall,
/// in the player's collision model and the monsters'. Both halves, on one
/// fixture, read straight off the collision models rather than off the
/// player's position — the switch is what is under test, not the fall.
#[test]
fn a_func_wall_toggle_stops_and_starts_being_solid_as_it_is_switched() {
    let entities = format!(
        "{}{}{}",
        floor_entities("func_wall_toggle", ""),
        auto_trigger(FLOOR_NAME, 0.5),
        auto_trigger(FLOOR_NAME, 1.5),
    );
    let mut game = game_from("ohlwalltogglesynth", killable_brush_floor_bsp(&entities));
    let probe = inside_the_slab();

    tick_n(&mut game, 6, &Input::default());
    assert!(
        game.position_is_in_solid(probe),
        "the wall starts solid without the Starts invisible flag"
    );
    assert!(monster_model_is_solid_at(&game, probe));

    // Past the first switch.
    tick_n(&mut game, 45, &Input::default());
    assert!(
        !game.position_is_in_solid(probe),
        "a switched-off func_wall_toggle must stop being solid"
    );
    assert!(
        !monster_model_is_solid_at(&game, probe),
        "and stop being solid to a monster too"
    );

    // Past the second: the brush was suspended, not detached, so it comes
    // back.
    tick_n(&mut game, 90, &Input::default());
    assert!(
        game.position_is_in_solid(probe),
        "switching it back on must restore the very same collision brush"
    );
    assert!(monster_model_is_solid_at(&game, probe));
}

/// Whether `point` is solid in the collision model monsters navigate
/// against, which keeps its own copy of every brush entity.
fn monster_model_is_solid_at(game: &Game, point: [f32; 3]) -> bool {
    let model = game
        .monster_collision()
        .expect("the fixture has collision hulls");
    ohl_physics::contents::is_solid(ohl_physics::point_contents(
        model,
        glam::Vec3::from_array(point),
    ))
}

/// The published "Starts invisible" flag: the wall is off from the first
/// tick, so the player — standing on nothing — falls straight through the
/// only floor the map has.
#[test]
fn a_func_wall_toggle_that_starts_invisible_is_not_a_floor() {
    let entities = floor_entities("func_wall_toggle", "\"spawnflags\" \"1\"\n");
    let mut game = game_from(
        "ohlwalltogglehiddensynth",
        killable_brush_floor_bsp(&entities),
    );
    assert!(
        !game.position_is_in_solid(inside_the_slab()),
        "a wall that starts invisible is not solid on the first tick either"
    );
    assert_eq!(
        game.submodel_count(),
        1,
        "its geometry is built all the same, so switching it on has something to draw"
    );
    tick_n(&mut game, 60, &Input::default());
    assert!(!game.player_on_ground());
    assert!(
        game.player_origin()[2] < BRUSH_FLOOR_TOP_Z - 32.0,
        "with the only floor switched off the player falls through it"
    );
}

// ---------------------------------------------------------------------
// A shootable `func_button`
// ---------------------------------------------------------------------

/// The button fixture's `health`: below the published 40-damage `.357`
/// single shot, so one landed shot exhausts it outright.
const BUTTON_HEALTH: f32 = 30.0;
const BUTTON_NAME: &str = "ohl_shot_button";
const BUTTON_DOOR_NAME: &str = "ohl_shot_button_door";

/// `rot_button_health_entities`'s fixture with a plain `func_button` in
/// place of the `func_rot_button`: the same room, the same spawn facing,
/// the same `weapon_357` sitting on the spawn point.
fn shootable_button_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -24 40\"\n\
         \"angle\" \"90\"\n}}\n\
         {{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 -24 40\"\n}}\n\
         {{\n\"classname\" \"func_button\"\n\"targetname\" \"{BUTTON_NAME}\"\n\
         \"target\" \"{BUTTON_DOOR_NAME}\"\n\"model\" \"*1\"\n\
         \"speed\" \"100\"\n\"wait\" \"-1\"\n\"health\" \"{BUTTON_HEALTH}\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{BUTTON_DOOR_NAME}\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
    )
}

fn door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(BUTTON_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// The documented "(or by being shot, if Health is > 0)" press path for a
/// plain `func_button` — the sibling of the `func_rot_button` case
/// `rot_button.rs` already covers, driven the same way: pick the weapon up
/// by walking over it, select it, reload, fire one shot at the button.
#[test]
fn a_shot_presses_a_health_gated_func_button_and_opens_its_target_door() {
    let mut game = game_from(
        "ohlshotbuttonsynth",
        rot_button_bsp(&shootable_button_entities()),
    );
    assert_eq!(door_state(&game), MoverState::Closed);

    game.tick(STEP, &Input::default());
    assert!(game.inventory().has_weapon(WeaponId::Python));
    game.tick(
        STEP,
        &Input {
            select_slot: Some(2),
            ..Input::default()
        },
    );
    game.tick(
        STEP,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    tick_n(&mut game, 180, &Input::default());
    assert!(game.inventory().clip(WeaponId::Python) > 0);

    // The button's compiled box is centred on `ROT_BUTTON_CENTER`, and the
    // spawn faces it; a level shot lands inside it.
    assert!(ROT_BUTTON_CENTER[1].abs() < f32::EPSILON);
    game.tick(
        STEP,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    tick_n(&mut game, 60, &Input::default());
    assert_eq!(
        door_state(&game),
        MoverState::Open,
        "one .357 shot must exhaust the button's 30 health, press it and open its door"
    );
}

/// The same fixture, never shot at: a `health`-gated button is not pressed
/// by standing next to it.
#[test]
fn idling_next_to_a_health_gated_func_button_does_not_press_it() {
    let mut game = game_from(
        "ohlshotbuttonidlesynth",
        rot_button_bsp(&shootable_button_entities()),
    );
    tick_n(&mut game, 120, &Input::default());
    assert_eq!(door_state(&game), MoverState::Closed);
}

/// A point well inside the toggle block, away from where the fixtures
/// below stand anyone, so a point-contents probe reads the block itself.
const BLOCK_PROBE: [f32; 3] = [24.0, 24.0, 90.0];

/// A `func_wall_toggle` block that starts invisible, switched on by a
/// `trigger_auto` half a second in, with the player standing at
/// `player_start` and `extra` entities appended.
fn block_game(map: &str, player_start: [f32; 3], extra: &str) -> Game {
    let [x, y, z] = player_start;
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"{x} {y} {z}\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_wall_toggle\"\n\"targetname\" \"ohl_block\"\n\
         \"model\" \"*1\"\n\"spawnflags\" \"1\"\n}}\n\
         {}{extra}",
        auto_trigger("ohl_block", 0.5)
    );
    game_from(map, wall_toggle_block_bsp(&entities))
}

fn block_is_on(game: &Game) -> bool {
    let registry = game.registry();
    let entity = *registry
        .find("ohl_block")
        .first()
        .expect("the fixture names its block");
    registry
        .world
        .get::<&WallToggle>(entity)
        .expect("the block is a func_wall_toggle")
        .visible
}

/// Switched on around the player, a `func_wall_toggle` waits: it is on as
/// far as the map is concerned, but not solid while the player stands in
/// it, so they are not embedded for good. Once they have walked out, it is
/// solid. (Project-authored; see `Level::hold_toggled_walls_for_occupants`.)
#[test]
fn a_wall_switched_on_around_the_player_waits_until_they_step_out() {
    let mut game = block_game("ohlblockplayersynth", [0.0, 0.0, 37.0], "");
    tick_n(&mut game, 60, &Input::default());
    assert!(block_is_on(&game), "the trigger_auto switched the block on");
    assert!(
        !game.position_is_in_solid(BLOCK_PROBE),
        "the block must not turn solid around the player"
    );
    assert!(!monster_model_is_solid_at(&game, BLOCK_PROBE));

    let start = game.player_origin();
    tick_n(
        &mut game,
        60,
        &Input {
            forward: 1,
            ..Input::default()
        },
    );
    let out = game.player_origin();
    assert!(
        out[0] > WALL_TOGGLE_BLOCK_MAX[0] + 16.0,
        "the player walks out of the block: from {start:?} to {out:?}"
    );
    tick_n(&mut game, 2, &Input::default());
    assert!(
        game.position_is_in_solid(BLOCK_PROBE),
        "with nobody inside, the block turns solid"
    );
    assert!(monster_model_is_solid_at(&game, BLOCK_PROBE));
}

/// The same block switched on around a monster waits too, in both
/// collision models, while the player stands well clear of it.
#[test]
fn a_wall_switched_on_around_a_monster_waits_for_it() {
    let monster = "{\n\"classname\" \"monster_scientist\"\n\"targetname\" \"ohl_inside\"\n\
                   \"origin\" \"0 0 37\"\n\"angle\" \"0\"\n}\n";
    let mut game = block_game("ohlblockmonstersynth", [-160.0, 0.0, 37.0], monster);
    tick_n(&mut game, 60, &Input::default());
    assert!(block_is_on(&game), "the trigger_auto switched the block on");
    let inside = *game
        .registry()
        .find("ohl_inside")
        .first()
        .expect("the fixture names its monster");
    let origin = game
        .registry()
        .world
        .get::<&ohl_game::registry::Transform>(inside)
        .expect("the monster has a transform")
        .origin;
    assert!(
        origin.x.abs() < WALL_TOGGLE_BLOCK_MAX[0] && origin.y.abs() < WALL_TOGGLE_BLOCK_MAX[1],
        "the monster is still standing inside the block: {origin:?}"
    );
    assert!(
        !game.position_is_in_solid(BLOCK_PROBE),
        "the block must not turn solid around a monster"
    );
    assert!(!monster_model_is_solid_at(&game, BLOCK_PROBE));
}

/// A `monster_generic` spawned with its published "Not solid" flag is not
/// solid to be embedded, so it does not hold the wall up; the same prop
/// spawned solid does.
#[test]
fn a_not_solid_prop_inside_does_not_hold_the_wall_up() {
    let held = |flags: &str| {
        let prop = format!(
            "{{\n\"classname\" \"monster_generic\"\n\"origin\" \"0 0 37\"\n\
             \"spawnflags\" \"{flags}\"\n}}\n"
        );
        let mut game = block_game("ohlblockpropsynth", [-160.0, 0.0, 37.0], &prop);
        assert_eq!(game.monster_count(), 1, "the prop spawned");
        tick_n(&mut game, 60, &Input::default());
        assert!(block_is_on(&game));
        !game.position_is_in_solid(BLOCK_PROBE)
    };
    assert!(held("0"), "a solid prop inside holds the wall up");
    assert!(!held("4"), "a Not solid prop does not");
}

/// The control for both: switched on with nobody inside, the block is
/// solid on the next step.
#[test]
fn a_wall_switched_on_with_nobody_inside_is_solid_at_once() {
    let mut game = block_game("ohlblockemptysynth", [-160.0, 0.0, 37.0], "");
    tick_n(&mut game, 60, &Input::default());
    assert!(block_is_on(&game));
    assert!(game.position_is_in_solid(BLOCK_PROBE));
    assert!(monster_model_is_solid_at(&game, BLOCK_PROBE));
}

// ---------------------------------------------------------------------
// `player_weaponstrip`
// ---------------------------------------------------------------------

/// A `player_weaponstrip` fired by name empties the engine's own inventory
/// *and* its reserve-ammo ledger — the two are separate in
/// `crate::combat`, and a strip that cleared only one of them would leave
/// the player with ammo for guns they no longer have.
#[test]
fn a_player_weaponstrip_empties_the_players_weapons_and_ammo() {
    let entities = format!(
        "{}{}{}{}",
        floor_entities("func_wall", ""),
        "{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 0 40\"\n}\n\
         {\n\"classname\" \"ammo_357\"\n\"origin\" \"0 0 40\"\n}\n",
        "{\n\"classname\" \"player_weaponstrip\"\n\"targetname\" \"ohl_strip\"\n\
         \"origin\" \"0 0 40\"\n}\n",
        auto_trigger("ohl_strip", 1.0),
    );
    let mut game = game_from("ohlweaponstripsynth", killable_brush_floor_bsp(&entities));

    tick_n(&mut game, 30, &Input::default());
    assert!(
        game.inventory().has_weapon(WeaponId::Python),
        "the weapon at the spawn point must be picked up first"
    );
    assert!(
        !game.inventory().ammo(AmmoType::ThreeFiveSeven).is_empty(),
        "and the ammo box with it"
    );
    assert_eq!(game.weapon_strip_count(), 0);

    tick_n(&mut game, 90, &Input::default());
    assert_eq!(
        game.weapon_strip_count(),
        1,
        "the trigger_auto must have reached the strip"
    );
    assert_eq!(game.inventory().owned_weapons().count(), 0);
    assert!(game.inventory().selected().is_none());
    for kind in AmmoType::ALL {
        assert!(
            game.inventory().ammo(kind).is_empty(),
            "{kind:?} must be empty after a strip"
        );
    }
}

/// The HUD forgets the stripped gun as well: its clip and reserve numbers
/// are only refreshed while a weapon is selected, so a strip that left them
/// would keep the gun's last numbers on screen with nothing in hand.
#[test]
fn a_strip_clears_the_huds_ammo_numbers() {
    let entities = format!(
        "{}{}{}",
        floor_entities("func_wall", ""),
        "{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 0 40\"\n}\n\
         {\n\"classname\" \"player_weaponstrip\"\n\"targetname\" \"ohl_strip\"\n}\n",
        auto_trigger("ohl_strip", 2.0),
    );
    let mut game = game_from("ohlstriphudsynth", killable_brush_floor_bsp(&entities));
    tick_n(&mut game, 1, &Input::default());
    tick_n(
        &mut game,
        1,
        &Input {
            select_slot: Some(2),
            ..Input::default()
        },
    );
    tick_n(&mut game, 30, &Input::default());
    assert!(
        game.hud().reserve_ammo.is_some(),
        "the gun's numbers are on the HUD before the strip"
    );

    tick_n(&mut game, 120, &Input::default());
    assert_eq!(game.weapon_strip_count(), 1);
    assert_eq!(game.hud().clip_ammo, None);
    assert_eq!(game.hud().reserve_ammo, None);
}

/// A strip also stops whatever the player was in the middle of firing. The
/// firing state machine keeps its own copy of the loaded clip, so a strip
/// that emptied only the inventory would hand the same gun back, given
/// again later, with the rounds it had before the strip still chambered.
#[test]
fn a_weapon_given_back_after_a_strip_comes_back_unloaded() {
    let entities = format!(
        "{}{}{}",
        floor_entities("func_wall", ""),
        "{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 0 40\"\n}\n\
         {\n\"classname\" \"player_weaponstrip\"\n\"targetname\" \"ohl_strip\"\n}\n",
        auto_trigger("ohl_strip", 4.0),
    );
    let mut game = game_from("ohlstripreloadsynth", killable_brush_floor_bsp(&entities));
    let select = Input {
        select_slot: Some(2),
        ..Input::default()
    };

    tick_n(&mut game, 1, &Input::default());
    tick_n(&mut game, 1, &select);
    tick_n(
        &mut game,
        1,
        &Input {
            reload: true,
            ..Input::default()
        },
    );
    tick_n(&mut game, 120, &Input::default());
    assert!(
        game.inventory().clip(WeaponId::Python) > 0,
        "the fixture loads the gun before the strip"
    );

    tick_n(&mut game, 180, &Input::default());
    assert_eq!(game.weapon_strip_count(), 1);

    game.give_start_inventory(&[StartInventoryItem::Weapon(WeaponId::Python)]);
    tick_n(&mut game, 1, &select);
    let fired_before = game.weapon_fired_count();
    tick_n(
        &mut game,
        1,
        &Input {
            attack: true,
            ..Input::default()
        },
    );
    assert_eq!(
        game.weapon_fired_count(),
        fired_before,
        "a gun given back after a strip must not fire rounds loaded before it"
    );
}

// ---------------------------------------------------------------------
// `trigger_endsection`
// ---------------------------------------------------------------------

/// A `trigger_endsection` fired by a map's own chain surfaces as
/// [`GameEvent::EndSection`], which is what `ohl-app` ends the run on. The
/// event carries nothing map-derived — that is the point of it being a
/// unit variant.
#[test]
fn a_trigger_endsection_surfaces_as_a_game_event() {
    let entities = format!(
        "{}{}{}",
        floor_entities("func_wall", ""),
        "{\n\"classname\" \"trigger_endsection\"\n\"targetname\" \"ohl_end\"\n\
         \"section\" \"ohl_test_section\"\n\"origin\" \"0 0 40\"\n}\n",
        auto_trigger("ohl_end", 0.5),
    );
    let mut game = game_from("ohlendsectionsynth", killable_brush_floor_bsp(&entities));

    let before = tick_n(&mut game, 6, &Input::default());
    assert!(
        !before
            .iter()
            .any(|event| matches!(event, GameEvent::EndSection)),
        "nothing ends the section before the chain reaches it"
    );

    let after = tick_n(&mut game, 60, &Input::default());
    assert_eq!(
        after
            .iter()
            .filter(|event| matches!(event, GameEvent::EndSection))
            .count(),
        1,
        "the section ends exactly once"
    );
}

// ---------------------------------------------------------------------
// `weaponbox`
// ---------------------------------------------------------------------

/// A `weaponbox` at the spawn point carrying `keys` (already-quoted
/// keyvalue lines), on an ordinary floor.
fn weaponbox_game(map: &str, boxes: &[&str]) -> Game {
    let mut entities = floor_entities("func_wall", "");
    for keys in boxes {
        let _ = write!(
            entities,
            "{{\n\"classname\" \"weaponbox\"\n\"origin\" \"0 0 40\"\n{keys}}}\n"
        );
    }
    game_from(map, killable_brush_floor_bsp(&entities))
}

/// A `weaponbox` stocks exactly what its own published, case-sensitive
/// keys name: the `357` and `buckshot` keys fill those pools by the
/// amounts given, and a differently cased spelling of a real key stocks
/// nothing. It never hands over a weapon.
#[test]
fn a_weaponbox_stocks_the_ammo_its_own_keys_name() {
    let mut game = weaponbox_game(
        "ohlweaponboxsynth",
        &["\"357\" \"6\"\n\"buckshot\" \"12\"\n\"argrenades\" \"2\"\n"],
    );
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(game.pickup_count(), 1, "the box is picked up");
    let inventory = game.inventory();
    assert_eq!(inventory.ammo(AmmoType::ThreeFiveSeven).current(), 6);
    assert_eq!(inventory.ammo(AmmoType::Buckshot).current(), 12);
    assert!(
        inventory.ammo(AmmoType::Mp5Grenades).is_empty(),
        "a key in the wrong case stocks nothing"
    );
    assert_eq!(
        inventory.owned_weapons().count(),
        0,
        "a weaponbox holds only ammunition"
    );
}

/// "Even if the ammunition load of a carried weapon is full, this entity
/// will be picked up permanently": a box whose pool the first one already
/// filled is taken all the same, and so is one stocked with nothing this
/// build recognises. An `ammo_*` box in the same position would stay.
#[test]
fn a_weaponbox_is_taken_even_when_it_has_nothing_left_to_give() {
    let mut game = weaponbox_game(
        "ohlweaponboxfullsynth",
        &[
            "\"357\" \"1000\"\n",
            "\"357\" \"6\"\n",
            "\"argrenades\" \"2\"\n",
        ],
    );
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(
        game.pickup_count(),
        3,
        "every weaponbox is taken, full pools or not"
    );
    let cap = AmmoType::ThreeFiveSeven
        .published_max_carry()
        .expect("the .357 carry cap is published");
    assert_eq!(
        game.inventory().ammo(AmmoType::ThreeFiveSeven).current(),
        cap
    );
}

// ---------------------------------------------------------------------
// `item_security`
// ---------------------------------------------------------------------

const CARD_DOOR_NAME: &str = "ohl_card_door";

fn card_door_state(game: &Game) -> MoverState {
    let registry = game.registry();
    let entity = *registry
        .find(CARD_DOOR_NAME)
        .first()
        .expect("the fixture declares one named door");
    registry
        .world
        .get::<&Door>(entity)
        .expect("the named entity is a door")
        .state
}

/// An `item_security` is picked up without the suit, grants nothing, and
/// fires its own `target` as it goes — the "pickup ability ... used as a
/// way to unlock other kinds of entities" a map wires it for.
#[test]
fn picking_up_a_security_card_fires_its_target() {
    let entities = format!(
        "{}{}",
        floor_entities("func_wall", ""),
        format_args!(
            "{{\n\"classname\" \"item_security\"\n\"origin\" \"0 0 40\"\n\
             \"target\" \"{CARD_DOOR_NAME}\"\n}}\n\
             {{\n\"classname\" \"func_door\"\n\"targetname\" \"{CARD_DOOR_NAME}\"\n\
             \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
        ),
    );
    let mut game = game_from("ohlsecuritycardsynth", killable_brush_floor_bsp(&entities));
    assert_eq!(card_door_state(&game), MoverState::Closed);

    tick_n(&mut game, 60, &Input::default());
    assert_eq!(game.pickup_count(), 1, "the card is picked up, suit or not");
    assert_eq!(game.inventory().owned_weapons().count(), 0);
    assert_ne!(
        card_door_state(&game),
        MoverState::Closed,
        "taking the card must fire the door it targets"
    );
}

// ---------------------------------------------------------------------
// Saving what a map switched (save tag 39)
// ---------------------------------------------------------------------

/// The block fixture with its wall starting *on*, switched off half a
/// second in, and a model-less `func_conveyor` reversed at the same time —
/// both by `trigger_auto`s, which a save records as spent.
fn switched_game(map: &str) -> (MemoryAssets, Game) {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-160 0 37\"\n}}\n\
         {{\n\"classname\" \"func_wall_toggle\"\n\"targetname\" \"ohl_block\"\n\
         \"model\" \"*1\"\n}}\n\
         {{\n\"classname\" \"func_conveyor\"\n\"targetname\" \"ohl_belt\"\n\
         \"speed\" \"{CONVEYOR_SPEED}\"\n}}\n\
         {}{}",
        auto_trigger("ohl_block", 0.5),
        auto_trigger("ohl_belt", 0.5),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{map}.bsp"), wall_toggle_block_bsp(&entities));
    let game = Game::load(&assets as &dyn AssetSource, map).expect("the fixture loads");
    (assets, game)
}

fn belt_speed(game: &Game) -> f32 {
    let registry = game.registry();
    let entity = *registry
        .find("ohl_belt")
        .first()
        .expect("the fixture names its belt");
    registry
        .world
        .get::<&ohl_game::registry::Conveyor>(entity)
        .expect("the belt is a func_conveyor")
        .speed
}

/// A wall a spent trigger switched off, and a belt it reversed, are still
/// off and reversed after a save and a load: the trigger cannot switch
/// them again, so a load that put them back would leave the barrier solid
/// for good — or, here, solid around wherever the player had walked
/// through it.
#[test]
fn a_switched_wall_and_a_reversed_belt_survive_a_save_and_load() {
    let (assets, mut game) = switched_game("ohlswitchsavesynth");
    tick_n(&mut game, 60, &Input::default());
    assert!(!block_is_on(&game));
    assert!(!game.position_is_in_solid(BLOCK_PROBE));
    assert!(belt_speed(&game) < 0.0);

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    assert!(!block_is_on(&reloaded), "the wall is still off");
    assert!(
        !reloaded.position_is_in_solid(BLOCK_PROBE),
        "and still not solid"
    );
    assert!(belt_speed(&reloaded) < 0.0, "the belt still runs backwards");

    tick_n(&mut reloaded, 60, &Input::default());
    assert!(!block_is_on(&reloaded), "nothing switches it back");
    assert!(!reloaded.position_is_in_solid(BLOCK_PROBE));
}

/// A save written before tag 39 existed still loads, with both entities
/// back at their spawn state — exactly what every earlier build did.
#[test]
fn a_save_without_the_switch_section_still_loads() {
    let (assets, mut game) = switched_game("ohlswitcholdsavesynth");
    tick_n(&mut game, 60, &Input::default());
    let mut save = game.to_save(1_700_000_000);
    save.switches = None;
    let bytes = save.to_bytes().expect("an old-shaped save still encodes");
    let reloaded = Game::load_bytes(&assets, &bytes).expect("an old-shaped save still loads");
    assert!(
        block_is_on(&reloaded),
        "no tag 39: the wall is back at spawn"
    );
    assert!(reloaded.position_is_in_solid(BLOCK_PROBE));
    assert!((belt_speed(&reloaded) - CONVEYOR_SPEED).abs() < 1e-3);
}

/// A pickup taken before a save stays taken after it. For an
/// `item_security` that matters beyond the item itself: taking it fires its
/// `target`, and a card that came back to be taken again would fire it a
/// second time.
#[test]
fn a_security_card_taken_before_a_save_is_not_taken_again_after_it() {
    let entities = format!(
        "{}{}",
        floor_entities("func_wall", ""),
        format_args!(
            "{{\n\"classname\" \"item_security\"\n\"origin\" \"0 0 40\"\n\
             \"target\" \"{CARD_DOOR_NAME}\"\n}}\n\
             {{\n\"classname\" \"func_door\"\n\"targetname\" \"{CARD_DOOR_NAME}\"\n\
             \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
        ),
    );
    let mut assets = MemoryAssets::new();
    assets.insert(
        "maps/ohlcardsavesynth.bsp",
        killable_brush_floor_bsp(&entities),
    );
    let mut game =
        Game::load(&assets as &dyn AssetSource, "ohlcardsavesynth").expect("the fixture loads");
    tick_n(&mut game, 30, &Input::default());
    assert_eq!(game.pickup_count(), 1, "the card is taken before the save");

    let bytes = game.save_bytes(1_700_000_000).expect("the save is written");
    let mut reloaded = Game::load_bytes(&assets, &bytes).expect("the save loads");
    tick_n(&mut reloaded, 30, &Input::default());
    assert_eq!(
        reloaded.pickup_count(),
        0,
        "the card stays taken, and its target is not fired again"
    );
}
