//! Project-authored synthetic fixtures, published so `ohl-app`'s CLI test
//! can build the same payload tree this crate's own tests use.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, collision_room_brushes};

/// One quad in the fixture: its four corners in winding order, and the
/// texture slot it draws with.
type Quad = ([[f32; 3]; 4], u32);

/// The map name the synthetic fixture is published under.
pub const SYNTHETIC_MAP: &str = "ohlsynth";

/// The `targetname` both the door and the button in the fixture use.
pub const DOOR_NAME: &str = "ohl_door";

/// The landmark the fixture's `trigger_changelevel` names.
pub const LANDMARK: &str = "ohl_landmark";

/// The destination map the fixture's `trigger_changelevel` names.
pub const NEXT_MAP: &str = "ohlsynth2";

/// The entity block: a lit room with a player start, one brush door on
/// submodel 1, a landmark, and a `trigger_changelevel` the door's `use`
/// chain never reaches (tests fire it directly).
///
/// `extra` (one or more complete `{ ... }` entity blocks) is appended after
/// the fixture's own entities, so a test can add a prop or sprite entity
/// without duplicating the rest of the fixture.
fn entities_text_with_extra(next_map: &str, extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"4\"\n\"angle\" \"90\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 0\"\n}}\n\
         {{\n\"classname\" \"func_button\"\n\"target\" \"ohl_exit\"\n\
         \"origin\" \"0 100 32\"\n\"speed\" \"50\"\n\"wait\" \"1\"\n\"delay\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_exit\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n}}\n\
         {extra}"
    )
}

/// Builds the synthetic map: a closed, lit box with a second brush submodel
/// for the door, collision hulls, and the entity block above.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn synthetic_map_bsp() -> Vec<u8> {
    synthetic_map_bsp_named(NEXT_MAP)
}

/// The same fixture with a caller-chosen `trigger_changelevel` destination,
/// so a test can build the *destination* map too.
#[must_use]
pub fn synthetic_map_bsp_named(next_map: &str) -> Vec<u8> {
    synthetic_map_bsp_with_extra_entity(next_map, "")
}

/// As [`synthetic_map_bsp_named`], with one or more extra entity blocks
/// (already-formatted `{ ... }` text) appended to the entities lump, so a
/// test can add e.g. a `monster_generic` prop or an `env_sprite` without
/// rebuilding the whole fixture's geometry.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn synthetic_map_bsp_with_extra_entity(next_map: &str, extra_entity: &str) -> Vec<u8> {
    synthetic_map_bsp_with_entities(&entities_text_with_extra(next_map, extra_entity))
}

/// The same room geometry with a caller-authored entity block, so a test can
/// build a second map of a two-map campaign (its own landmark, its own
/// carried entities) without inventing new geometry.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn synthetic_map_bsp_with_entities(entities: &str) -> Vec<u8> {
    const HALF: f32 = 192.0;
    const HEIGHT: f32 = 192.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);
    let split_plane = 1u32;
    b.push_edge(0, 0);

    b.add_embedded_texture("ohlfloor", 64, 64, 210);
    b.add_embedded_texture("ohldoor", 64, 64, 130);

    // Two quads: the room's floor (submodel 0) and the door leaf
    // (submodel 1), each with its own texture and fully-lit samples.
    let quads: [Quad; 2] = [
        (
            [
                [-HALF, -HALF, 0.0],
                [HALF, -HALF, 0.0],
                [HALF, HALF, 0.0],
                [-HALF, HALF, 0.0],
            ],
            0,
        ),
        (
            [
                [-64.0, -8.0, 0.0],
                [64.0, -8.0, 0.0],
                [64.0, -8.0, 96.0],
                [-64.0, -8.0, 96.0],
            ],
            1,
        ),
    ];
    for (index, (corners, texture)) in quads.into_iter().enumerate() {
        let base = u16::try_from(index * 4).expect("two quads fit u16");
        for corner in corners {
            b.push_vertex(corner);
        }
        for corner in 0..4u16 {
            let next = (corner + 1) % 4;
            b.push_edge(base + corner, base + next);
        }
        let first_edge = i32::try_from(index * 4 + 1).expect("fits");
        for step in 0..4 {
            b.push_surfedge(first_edge + step);
        }
        b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, texture, 0);
        let offset = i32::try_from(b.lighting.len()).expect("fits");
        for sample in 0..900 {
            let level = 96 + u8::try_from((sample * 7) % 128).unwrap_or(0);
            b.push_lighting_rgb(level, level, level);
        }
        b.push_face(
            0,
            0,
            u32::try_from(index * 4).expect("fits"),
            4,
            u16::try_from(index).expect("fits"),
            [0, 0xFF, 0xFF, 0xFF],
            offset,
        );
        b.push_marksurface(u16::try_from(index).expect("fits"));
    }

    b.visibility.push(0b0000_0011);
    b.visibility.push(0b0000_0011);

    let extent: i16 = 192;
    let height: i16 = 192;
    b.push_leaf(-2, -1, [0, 0, 0], [0, 0, 0], 0, 0, [0, 0, 0, 0]);
    b.push_leaf(
        -1,
        0,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        1,
        [0, 0, 0, 0],
    );
    b.push_leaf(
        -1,
        0,
        [-extent, -extent, 0],
        [extent, extent, height],
        1,
        1,
        [0, 0, 0, 0],
    );
    b.push_node(
        split_plane,
        -2,
        -3,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        2,
    );

    // Real clip hulls, so the walking player has a floor to stand on.
    let brushes: Vec<CollisionBrush> = collision_room_brushes();
    let head_nodes = b.push_collision_hulls(&brushes);

    // Submodel 0: worldspawn (the floor face). Submodel 1: the door leaf.
    b.push_model(
        [-HALF, -HALF, 0.0],
        [HALF, HALF, HEIGHT],
        [0.0, 0.0, 0.0],
        head_nodes,
        2,
        0,
        1,
    );
    // A door leaf with real depth along its move direction (+Y), so the
    // registry derives a non-zero travel distance and the door takes time
    // to open.
    b.push_model(
        [-64.0, -8.0, 0.0],
        [64.0, 56.0, 96.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        1,
        1,
    );

    b.build()
}

// --- M7.9 P2: an AI room -------------------------------------------------

/// The map name the AI fixture is published under.
pub const AI_MAP: &str = "ohlaisynth";

/// Builds a plain, flat, closed room with a caller-authored entity block and
/// an optional interior wall.
///
/// Unlike [`synthetic_map_bsp_with_entities`], the collision hulls here are
/// nothing but the six faces of a box (plus, optionally, one solid slab on
/// the `x = 0` plane): no steps, no ramps and no door leaf, so a test about
/// *who can see whom* is not also a test about walking up a ledge. The wall
/// spans the room's full width and height, so a monster on one side of it
/// has no line of sight to anything on the other.
///
/// Project-authored geometry; no bytes here come from any game
/// installation.
#[must_use]
pub fn ai_room_bsp(entities: &str, interior_wall: bool) -> Vec<u8> {
    const HALF: f32 = 256.0;
    const HEIGHT: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);
    let split_plane = 1u32;
    b.push_edge(0, 0);

    b.add_embedded_texture("ohlfloor", 64, 64, 210);

    // One quad: the room's floor. Nothing else needs to be drawable for an
    // AI test, and a smaller face list keeps the fixture easy to read.
    let floor: [[f32; 3]; 4] = [
        [-HALF, -HALF, 0.0],
        [HALF, -HALF, 0.0],
        [HALF, HALF, 0.0],
        [-HALF, HALF, 0.0],
    ];
    for corner in floor {
        b.push_vertex(corner);
    }
    for corner in 0..4u16 {
        b.push_edge(corner, (corner + 1) % 4);
    }
    for step in 0..4 {
        b.push_surfedge(1 + step);
    }
    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 0, 0);
    let offset = i32::try_from(b.lighting.len()).expect("fits");
    for sample in 0..900 {
        let level = 96 + u8::try_from((sample * 7) % 128).unwrap_or(0);
        b.push_lighting_rgb(level, level, level);
    }
    b.push_face(0, 0, 0, 4, 0, [0, 0xFF, 0xFF, 0xFF], offset);
    b.push_marksurface(0);

    b.visibility.push(0b0000_0011);
    b.visibility.push(0b0000_0011);

    let extent: i16 = 256;
    let height: i16 = 256;
    b.push_leaf(-2, -1, [0, 0, 0], [0, 0, 0], 0, 0, [0, 0, 0, 0]);
    b.push_leaf(
        -1,
        0,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        1,
        [0, 0, 0, 0],
    );
    b.push_leaf(
        -1,
        0,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        1,
        [0, 0, 0, 0],
    );
    b.push_node(
        split_plane,
        -2,
        -3,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        2,
    );

    let mut brushes = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ];
    if interior_wall {
        brushes.push(CollisionBrush::box_brush(
            [-16.0, -HALF, 0.0],
            [16.0, HALF, HEIGHT],
        ));
    }
    let head_nodes = b.push_collision_hulls(&brushes);

    b.push_model(
        [-HALF, -HALF, 0.0],
        [HALF, HALF, HEIGHT],
        [0.0, 0.0, 0.0],
        head_nodes,
        1,
        0,
        1,
    );

    b.build()
}

/// Queues `amount` points of damage against `target`, as if a weapon had
/// hit it, so a test can kill a monster without a weapon existing yet.
///
/// Applied by the next step's lifecycle phase, exactly like any other hit.
pub fn queue_monster_damage(
    game: &mut crate::Game,
    target: ohl_game::hecs::Entity,
    attacker: Option<ohl_game::hecs::Entity>,
    amount: f32,
) {
    let origin = ohl_ai::Vec3::ZERO;
    let event = match attacker {
        Some(attacker) => ohl_ai::DamageEvent::new(target, attacker, amount, origin),
        None => ohl_ai::DamageEvent::environmental(target, amount, origin),
    };
    game.systems_mut().ai_mut().queue_damage(event);
}

/// Runs `game` for `inputs.len()` ticks, one input per tick, at the fixed
/// step (`crate::TICK_SECONDS`).
///
/// This is what `ohl-app`'s `--script` loop does over a parsed scripted
/// input file (see that crate's `script.rs`), pulled down into this crate
/// so a determinism test can drive the same sequence without a CLI, a
/// script file, or a GPU.
pub fn run_script(game: &mut crate::Game, inputs: &[crate::Input]) {
    for input in inputs {
        game.tick(crate::TICK_SECONDS, input);
    }
}

/// Every entity in the current level that is a thinking monster, in
/// ascending entity-id order.
#[must_use]
pub fn monster_entities(game: &crate::Game) -> Vec<ohl_game::hecs::Entity> {
    let mut entities: Vec<ohl_game::hecs::Entity> = game
        .registry()
        .world
        .query::<(ohl_game::hecs::Entity, &ohl_ai::MonsterAi)>()
        .iter()
        .map(|(entity, _)| entity)
        .collect();
    entities.sort_unstable_by_key(|entity: &ohl_game::hecs::Entity| entity.id());
    entities
}

// --- M7.11: a scripted-sequence room -------------------------------------

/// The map name the scripted-sequence fixture is published under.
pub const SCRIPT_MAP: &str = "ohlscriptsynth";

/// The same flat, closed room [`ai_room_bsp`] builds, published under its
/// own name so a scripting test's fixture never collides with an AI one.
///
/// Project-authored geometry and entity text; no bytes here come from any
/// game installation.
#[must_use]
pub fn script_room_bsp(entities: &str) -> Vec<u8> {
    ai_room_bsp(entities, false)
}

/// A `worldspawn` plus an `info_player_start` at `player_origin`, followed
/// by whatever entity blocks a test adds.
#[must_use]
pub fn script_room_entities(player_origin: [f32; 3], extra: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"0\"\n}}\n\
         {extra}",
        player_origin[0], player_origin[1], player_origin[2]
    )
}

/// One entity block: `classname`, `origin`, `angle`, then `keys` verbatim.
///
/// Every key a test passes is a published `scripted_sequence`,
/// `scripted_sentence` or `monster_*` keyvalue; see
/// `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters".
#[must_use]
pub fn entity_block(classname: &str, origin: [f32; 3], yaw: f32, keys: &[(&str, &str)]) -> String {
    let mut block = format!(
        "{{\n\"classname\" \"{classname}\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"{yaw}\"\n",
        origin[0], origin[1], origin[2]
    );
    for (key, value) in keys {
        use std::fmt::Write as _;
        let _ = writeln!(block, "\"{key}\" \"{value}\"");
    }
    block.push_str("}\n");
    block
}

/// Builds a [`crate::Game`] over [`script_room_bsp`] with `entities`.
#[must_use]
pub fn script_game(entities: &str) -> crate::Game {
    let bytes = script_room_bsp(entities);
    let mut assets = crate::MemoryAssets::new();
    assets.insert(&format!("maps/{SCRIPT_MAP}.bsp"), bytes.clone());
    crate::Game::from_map_bytes(&assets, SCRIPT_MAP, &bytes).expect("the script room loads")
}

/// The world position of `entity`, as the AI sees it.
#[must_use]
pub fn actor_origin(game: &crate::Game, entity: ohl_game::hecs::Entity) -> ohl_ai::Vec3 {
    game.registry()
        .world
        .get::<&ohl_ai::Actor>(entity)
        .map(|actor| actor.origin)
        .unwrap_or_default()
}

/// The first entity whose `classname` is `classname`, in spawn order.
#[must_use]
pub fn entity_of_classname(game: &crate::Game, classname: &str) -> Option<ohl_game::hecs::Entity> {
    let mut found: Vec<ohl_game::hecs::Entity> = game
        .registry()
        .world
        .query::<(ohl_game::hecs::Entity, &ohl_game::registry::ClassName)>()
        .iter()
        .filter(|(_, name)| name.0 == classname)
        .map(|(entity, _)| entity)
        .collect();
    found.sort_unstable_by_key(|entity: &ohl_game::hecs::Entity| entity.id());
    found.first().copied()
}

/// An [`crate::Input`] with the `use` edge pressed.
#[must_use]
pub fn use_input() -> crate::Input {
    crate::Input {
        use_pressed: true,
        ..crate::Input::default()
    }
}
