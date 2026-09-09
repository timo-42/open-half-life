//! Project-authored synthetic fixtures, published so `ohl-app`'s CLI test
//! can build the same payload tree this crate's own tests use.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::test_support::{
    BRUSH_FLOOR_HALF_EXTENT, BRUSH_FLOOR_TOP_Z, Bsp30Builder, CollisionBrush,
    collision_room_brushes,
};

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

/// The map name the two-leaf visibility fixture is published under.
pub const PVS_MAP: &str = "ohlpvssynth";

/// The map name that fixture's own `trigger_changelevel` names.
pub const PVS_NEXT_MAP: &str = "ohlpvssynth2";

/// Builds a wide, flat, closed room split into **two leaves** by the
/// `x = 0` plane, with a hand-written visibility lump: leaf 1 (`x >= 0`)
/// sees only itself and leaf 2 (`x < 0`) sees only itself.
///
/// The room is 2,048 units across, deliberately wider than
/// `crate::transition::DEFAULT_CARRY_RADIUS`, so a test can place an entity
/// *beyond* the radius and still inside the landmark's own leaf — the case
/// the documented "inside the transition volume, or otherwise in the
/// landmark's PVS" eligibility rule exists for, and the one a radius alone
/// can never express. `visible_everywhere` writes the "every leaf sees every
/// leaf" lump instead, for the counterpart case.
///
/// Project-authored geometry and visibility bits; no bytes here come from
/// any game installation.
#[must_use]
pub fn pvs_room_bsp(entities: &str, visible_everywhere: bool) -> Vec<u8> {
    const HALF: f32 = 1024.0;
    const HEIGHT: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);
    let split_plane = 1u32;
    b.push_edge(0, 0);

    b.add_embedded_texture("ohlfloor", 64, 64, 210);

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

    // One decompressed row per leaf, one byte each (three leaves fit in a
    // byte). Bit `n` of a row names leaf `n + 1`, the lump's own
    // leaf-1-based bit numbering: `0b01` is "sees leaf 1", `0b10` is "sees
    // leaf 2". Leaf 1's row is at offset 0 and leaf 2's at offset 1.
    if visible_everywhere {
        b.visibility.push(0b0000_0011);
        b.visibility.push(0b0000_0011);
    } else {
        b.visibility.push(0b0000_0001);
        b.visibility.push(0b0000_0010);
    }

    let extent: i16 = 1024;
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
        1,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        1,
        [0, 0, 0, 0],
    );
    // Front child (the `x >= 0` side) is leaf 1, back child leaf 2.
    b.push_node(
        split_plane,
        -2,
        -3,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        2,
    );

    let brushes = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ];
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

/// The map name the touch-trigger fixture is published under.
pub const TOUCH_DOOR_MAP: &str = "ohltouchdoorsynth";

/// The `targetname` both the gating `trigger_multiple` and the door it
/// targets use to find each other.
pub const TOUCH_TRIGGER_NAME: &str = "ohl_touch_trigger";

/// The `targetname` of the door the touch trigger targets. Nothing in this
/// fixture lets the player `use` the door directly: it only opens by
/// walking through the trigger volume, reproducing the reported training-map
/// bug (a `func_door` gated by an adjoining touch trigger that never opened
/// for a walking player).
pub const TOUCH_DOOR_NAME: &str = "ohl_touch_door";

/// [`ai_room_bsp`]'s flat, collidable floor plus two extra brush
/// submodels that carry no faces of their own (only a bounding box, which
/// is all `ohl_game::registry::Registry` reads for a mover or a trigger
/// volume): submodel 1 is the gated door, submodel 2 is the touch trigger
/// standing between the player start and the door. Project-authored
/// geometry; no bytes here come from any game installation.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn door_behind_touch_trigger_bsp(entities: &str) -> Vec<u8> {
    const HALF: f32 = 256.0;
    const HEIGHT: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);
    let split_plane = 1u32;
    b.push_edge(0, 0);

    b.add_embedded_texture("ohlfloor", 64, 64, 210);

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

    let brushes = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ];
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
    // Submodel 1: the door, well ahead of the player start (x = 200), with
    // no faces or collision of its own — only its bounding box matters
    // here, since this fixture tests the map-logic state machine, not
    // rendering or being physically blocked by the leaf.
    b.push_model(
        [168.0, -32.0, 0.0],
        [232.0, 32.0, 96.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );
    // Submodel 2: the touch trigger, standing between the player start
    // (x = -96) and the door (x = 200).
    b.push_model(
        [64.0, -48.0, 0.0],
        [128.0, 48.0, 96.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );

    b.build()
}

/// A `worldspawn` plus an `info_player_start` at `(-96, 0, 36)`, a
/// `func_door` (submodel `*1`, targetname [`TOUCH_DOOR_NAME`]) that stays
/// open once triggered, and a `trigger_multiple` (submodel `*2`, targetname
/// [`TOUCH_TRIGGER_NAME`]) targeting it — the training-map shape this
/// fixture reproduces: a closed door with no direct `use` path, gated by an
/// adjoining touch trigger.
#[must_use]
pub fn door_behind_touch_trigger_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{TOUCH_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"-1\"\n\"angle\" \"90\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_multiple\"\n\"targetname\" \"{TOUCH_TRIGGER_NAME}\"\n\
         \"target\" \"{TOUCH_DOOR_NAME}\"\n\"model\" \"*2\"\n\"origin\" \"0 0 0\"\n}}\n"
    )
}

/// The map name the touch-`trigger_changelevel` fixture is published under.
pub const TOUCH_CHANGELEVEL_MAP: &str = "ohltouchchangelevelsynth";

/// A `worldspawn` plus an `info_player_start` at `(-96, 0, 36)` and a
/// `trigger_changelevel` volume (submodel `*2`, the same touch-trigger box
/// [`door_behind_touch_trigger_bsp`] already provides) between the start
/// and where the gated door would be, naming `next_map`/[`LANDMARK`] —
/// reproducing a level-exit volume a player reaches by walking, not by
/// `use`. Submodel `*1` (the door bounding box) is left unclaimed by any
/// entity here.
#[must_use]
pub fn touch_changelevel_entities(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    )
}

/// As [`touch_changelevel_entities`], but with the "USE Only" spawnflag
/// (`2`; see `ohl_game::registry::SPAWNFLAG_CHANGELEVEL_USE_ONLY`) set, so
/// a test can assert that walking into the same volume does *not* fire it.
#[must_use]
pub fn touch_changelevel_use_only_entities(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 0 36\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n\
         \"spawnflags\" \"2\"\n}}\n"
    )
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

/// Strips `entity`'s [`ohl_ai::MonsterAi`], if it has one, so a test can
/// build the inert-actor state a `monster_*` classname missing from this
/// project's spec table would otherwise produce (see
/// `ohl_engine::ai::EngineSpawnRules::spawn_for`'s doc comment): an
/// `Actor`, visible and possessable, that never thinks and never moves on
/// its own. Used to prove a `scripted_sequence` bound to such a monster
/// cannot stall forever (`ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS`).
pub fn strip_monster_ai(game: &mut crate::Game, entity: ohl_game::hecs::Entity) {
    game.registry_mut()
        .world
        .remove_one::<ohl_ai::MonsterAi>(entity)
        .ok();
}

/// An [`crate::Input`] with the `use` edge pressed.
#[must_use]
pub fn use_input() -> crate::Input {
    crate::Input {
        use_pressed: true,
        ..crate::Input::default()
    }
}

/// [`ohl_formats::test_support::build_brush_entity_floor_bsp`]'s geometry —
/// a void worldspawn model and, on submodel 1, a floor slab of the kind a
/// mapper builds as a brush entity — with a caller-authored entity block
/// instead of that helper's fixed one, so a test can add e.g. a monster and
/// a `scripted_sequence` whose `killtarget` names the floor. Reproduces the
/// same "brush entity is the only floor" shape `brush_entity_collision.rs`
/// tests, plus whatever map logic `entities` adds.
#[must_use]
pub fn killable_brush_floor_bsp(entities: &str) -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);
    // Submodel 0: no solids at all, exactly like
    // `build_brush_entity_floor_bsp`'s void world.
    let world_heads = b.push_collision_hulls(&[]);
    // Submodel 1: the floor slab, 16 units thick under `BRUSH_FLOOR_TOP_Z`.
    let slab_mins = [
        -BRUSH_FLOOR_HALF_EXTENT,
        -BRUSH_FLOOR_HALF_EXTENT,
        BRUSH_FLOOR_TOP_Z - 16.0,
    ];
    let slab_maxs = [
        BRUSH_FLOOR_HALF_EXTENT,
        BRUSH_FLOOR_HALF_EXTENT,
        BRUSH_FLOOR_TOP_Z,
    ];
    let slab_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(slab_mins, slab_maxs)]);
    b.push_model([-4096.0; 3], [4096.0; 3], [0.0; 3], world_heads, 2, 0, 0);
    b.push_model(slab_mins, slab_maxs, [0.0; 3], slab_heads, 2, 0, 0);
    b.build()
}

/// The map name the mover fixture is published under.
pub const MOVER_MAP: &str = "ohlmoversynth";

/// Half-extent of the mover's brush on X and Y, and its height above/below
/// its own origin — an arbitrary, project-authored box big enough for a
/// standing player to rest on.
const MOVER_HALF: f32 = 32.0;
const MOVER_TOP_Z: f32 = 8.0;
const MOVER_BOTTOM_Z: f32 = -8.0;

/// How far apart the two `path_corner` nodes are, along `+X`.
pub const MOVER_SEGMENT_LENGTH: f32 = 150.0;

/// The `func_train`'s `speed`/`startspeed`, units/second — non-zero
/// `startspeed` starts it moving immediately at map load, without needing
/// a trigger.
pub const MOVER_SPEED: f32 = 50.0;

/// The `info_player_start`'s height above the mover's own origin, resting
/// on top of its brush.
pub const MOVER_PLAYER_START_Z: f32 = MOVER_TOP_Z + 36.0;

/// Half-extent of the worldspawn model's own visible floor face, on `X`
/// and `Y` — a project-authored quad, far enough below the train's ride
/// (see [`MOVER_FLOOR_Z`]) that it never overlaps the train's brush or any
/// capture viewpoint riding it. Matches [`synthetic_map_bsp_with_entities`]'s
/// own floor quad's extent, which is already sized to fit comfortably
/// within the 900 lighting samples both fixtures push per face at the
/// documented 16-unit luxel spacing.
const MOVER_FLOOR_HALF: f32 = 192.0;

/// The `Z` the worldspawn model's own visible floor face sits at, well
/// below [`MOVER_BOTTOM_Z`] so it can never be mistaken for something the
/// player or the train's own brush could touch.
const MOVER_FLOOR_Z: f32 = -512.0;

/// A void world (submodel `*0`, no collision at all — its only visible
/// geometry is a single flat floor face far below the ride, carrying no
/// collision of its own, added purely so a renderer has *something* of
/// the worldspawn model to upload; see this function's own body) with a
/// single solid box (submodel `*1`) resting at the world origin,
/// referenced by a `func_train` riding a two-node `path_corner` chain
/// along `+X`. An `info_player_start` sits on top of the train's brush at
/// its resting position. The same shape `ohl-engine`'s own
/// `tests/mover_riders.rs` tests directly; published here so a
/// headless-capture CLI test can ride it too. Every keyvalue and
/// coordinate here is authored for this project; nothing is derived from
/// any payload.
#[must_use]
pub fn mover_train_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let player_z = MOVER_PLAYER_START_Z;
    let speed = MOVER_SPEED;
    let segment = MOVER_SEGMENT_LENGTH;
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"0 0 {player_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_train\"\n\"model\" \"*1\"\n\
         \"target\" \"ohl_node1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_corner\"\n\"targetname\" \"ohl_node1\"\n\
         \"target\" \"ohl_node2\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_corner\"\n\"targetname\" \"ohl_node2\"\n\
         \"origin\" \"{segment} 0 0\"\n}}\n"
    ));

    // The renderer legitimately refuses to upload a worldspawn model with
    // zero faces (`ohl_render::RenderError::WorldTooLarge`, also used for
    // any other degenerate/empty model) — every real published map has
    // visible geometry, so an empty world is treated as malformed input,
    // not a valid level. This fixture was authored for pure physics tests
    // (`ohl-engine`'s own `tests/mover_riders.rs`, `zero_speed_path_node.rs`)
    // and originally had none; a single flat floor face, textured and lit
    // like every other synthetic fixture's floor, satisfies the renderer
    // without adding any collision (the train's own brush stays the only
    // thing the player can stand on).
    b.push_plane([0.0, 0.0, 1.0], MOVER_FLOOR_Z, 2);
    let floor_texture = b.add_embedded_texture("ohlmoverfloor", 64, 64, 200);
    let floor_corners = [
        [-MOVER_FLOOR_HALF, -MOVER_FLOOR_HALF, MOVER_FLOOR_Z],
        [MOVER_FLOOR_HALF, -MOVER_FLOOR_HALF, MOVER_FLOOR_Z],
        [MOVER_FLOOR_HALF, MOVER_FLOOR_HALF, MOVER_FLOOR_Z],
        [-MOVER_FLOOR_HALF, MOVER_FLOOR_HALF, MOVER_FLOOR_Z],
    ];
    for corner in floor_corners {
        b.push_vertex(corner);
    }
    for corner in 0..4u16 {
        let next = (corner + 1) % 4;
        b.push_edge(corner, next);
    }
    // Surfedge values are direct, 0-based edge indices (magnitude only;
    // the sign picks which of an edge's two vertices comes first) — no
    // dummy edge 0 is reserved here, unlike `synthetic_map_bsp_with_entities`,
    // since this is the only face this fixture ever builds.
    for step in 0..4 {
        b.push_surfedge(step);
    }
    b.push_texinfo(
        [1.0, 0.0, 0.0],
        0.0,
        [0.0, 1.0, 0.0],
        0.0,
        u32::try_from(floor_texture).expect("one texture slot fits"),
        0,
    );
    let lighting_offset = i32::try_from(b.lighting.len()).expect("fits");
    for _ in 0..900 {
        b.push_lighting_rgb(200, 200, 200);
    }
    b.push_face(0, 0, 0, 4, 0, [0, 0xFF, 0xFF, 0xFF], lighting_offset);

    // Submodel 0: the worldspawn model — one visible floor face, far below
    // the ride, and no solids of its own: the train's own brush is the
    // only thing the player can stand on.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        1,
    );
    // Submodel 1: the train's own brush, resting at its first node.
    let train_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [-MOVER_HALF, -MOVER_HALF, MOVER_BOTTOM_Z],
        [MOVER_HALF, MOVER_HALF, MOVER_TOP_Z],
    )]);
    b.push_model(
        [-MOVER_HALF, -MOVER_HALF, MOVER_BOTTOM_Z],
        [MOVER_HALF, MOVER_HALF, MOVER_TOP_Z],
        [0.0, 0.0, 0.0],
        train_heads,
        2,
        0,
        0,
    );
    b.build()
}

// ---------------------------------------------------------------------
// A track train whose path carries a zero "New Train Speed" node
// ---------------------------------------------------------------------

/// The map name the zero-`speed`-node train fixture is published under.
pub const ZERO_SPEED_NODE_MAP: &str = "ohlzerospeednodesynth";

/// Where the fixture's middle `path_track` — the one carrying the
/// documented default "New Train Speed" of `0` — sits along `+X`.
pub const ZERO_SPEED_NODE_X: f32 = 150.0;

/// Where the fixture's `trigger_changelevel` volume starts along `+X`:
/// past [`ZERO_SPEED_NODE_X`], so only a train that keeps going after the
/// zero-`speed` node ever carries its passenger into it.
pub const ZERO_SPEED_TRIGGER_MIN_X: f32 = 200.0;

/// Where the fixture's last `path_track` sits, well past the trigger.
pub const ZERO_SPEED_END_X: f32 = 400.0;

/// A void world (submodel `*0`, no collision) with a solid track-train
/// brush (submodel `*1`) the player starts standing on, a three-node
/// `path_track` chain along `+X` whose *middle* node carries `speed "0"`
/// — the keyvalue's own documented default, meaning "no speed change" —
/// and a `trigger_changelevel` volume (submodel `*2`) beyond that middle
/// node.
///
/// The player never presses anything: the whole journey is the ride. A
/// train that reads the zero override literally parks itself on the middle
/// node and the volume is never reached; one that reads it as "leave the
/// speed alone" carries its passenger through. Same shape as
/// [`mover_train_bsp`], which this is modelled on.
///
/// Every keyvalue and coordinate here is authored for this project;
/// nothing is derived from any payload.
#[must_use]
pub fn zero_speed_node_train_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let player_z = MOVER_PLAYER_START_Z;
    let speed = MOVER_SPEED;
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"0 0 {player_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"target\" \"ohl_zn1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_zn1\"\n\
         \"target\" \"ohl_zn2\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_zn2\"\n\
         \"target\" \"ohl_zn3\"\n\"speed\" \"0\"\n\
         \"origin\" \"{ZERO_SPEED_NODE_X} 0 0\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_zn3\"\n\
         \"origin\" \"{ZERO_SPEED_END_X} 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{NEXT_MAP}\"\n\"landmark\" \"{LANDMARK}\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    ));

    // Submodel 0: a void world, so the train's own brush is the only thing
    // holding the player up.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the train's own brush, resting at its first node.
    let train_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [-MOVER_HALF, -MOVER_HALF, MOVER_BOTTOM_Z],
        [MOVER_HALF, MOVER_HALF, MOVER_TOP_Z],
    )]);
    b.push_model(
        [-MOVER_HALF, -MOVER_HALF, MOVER_BOTTOM_Z],
        [MOVER_HALF, MOVER_HALF, MOVER_TOP_Z],
        [0.0, 0.0, 0.0],
        train_heads,
        2,
        0,
        0,
    );
    // Submodel 2: the level-exit volume, a bounding box only (like every
    // other trigger volume in this module).
    b.push_model(
        [ZERO_SPEED_TRIGGER_MIN_X, -64.0, -32.0],
        [ZERO_SPEED_TRIGGER_MIN_X + 64.0, 64.0, 96.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );
    b.build()
}

// ---------------------------------------------------------------------
// A `func_tracktrain` whose track turns a square corner
// ---------------------------------------------------------------------

/// The map name the bending track-train fixture is published under.
pub const BEND_TRAIN_MAP: &str = "ohlbendtrainsynth";

/// The `targetname` of the fixture's `func_tracktrain`.
pub const BEND_TRAIN_NAME: &str = "ohl_bend_train";

/// Where the fixture's origin brush sits — the entity's own `origin`
/// keyvalue, deliberately non-zero so the fixture exercises the ordinary
/// origin-brush shape (compiled geometry stored relative to this point,
/// `ohl_game::pose::track_train_transform`) rather than the world-baked
/// one. It is also the chain's first node, so the car spawns exactly where
/// it was compiled.
pub const BEND_TRAIN_ORIGIN: [f32; 3] = [100.0, 0.0, 0.0];

/// The chain's middle node: the corner itself. The first segment runs
/// `+X` from [`BEND_TRAIN_ORIGIN`] to here, the second runs `+Y` from here
/// to [`BEND_TRAIN_END`], so the car's drawn yaw steps from 0 to 90
/// degrees in the single simulation step it changes segment on — the
/// whole point of the fixture.
pub const BEND_TRAIN_CORNER: [f32; 3] = [400.0, 0.0, 0.0];

/// The chain's last node.
pub const BEND_TRAIN_END: [f32; 3] = [400.0, 300.0, 0.0];

/// The car's compiled half-extents about its own origin brush: long along
/// its own `+X` (the direction it faces), narrow across. Long enough that
/// a passenger seated [`BEND_SEAT_OFFSET_X`] units from the pivot is well
/// inside it before the corner and would be well outside the *turned* car
/// if the turn did not carry them.
pub const BEND_CAR_HALF_LENGTH: f32 = 96.0;
/// See [`BEND_CAR_HALF_LENGTH`].
pub const BEND_CAR_HALF_WIDTH: f32 = 40.0;
/// The car floor's top, in the compiled frame.
pub const BEND_CAR_TOP_Z: f32 = 8.0;
/// The car floor's bottom, in the compiled frame.
pub const BEND_CAR_BOTTOM_Z: f32 = -8.0;

/// How far along the car, from its origin brush, the fixture's
/// `info_player_start` seats the passenger.
pub const BEND_SEAT_OFFSET_X: f32 = 64.0;

/// The train's `speed`/`startspeed`, units per second. A non-zero
/// `startspeed` starts it moving at map load without a trigger.
pub const BEND_TRAIN_SPEED: f32 = 100.0;

/// A void world (submodel `*0`, no collision of its own, so the car is the
/// only thing holding anyone up) carrying a `func_tracktrain` (submodel
/// `*1`) on a three-node `path_track` chain that turns a square corner,
/// with an `info_player_start` seated on the car [`BEND_SEAT_OFFSET_X`]
/// units *away from* its origin brush.
///
/// This is the shape the "riding movers" gap was about: the car turns to
/// face its segment, so at the corner its whole body swings about the
/// origin brush, and the seat the passenger is standing on swings with it.
/// A passenger who is not turned with the car keeps their world offset
/// from it, ends up beyond the turned car's own footprint, and falls into
/// the void this fixture deliberately leaves under them.
///
/// Every keyvalue and coordinate here is authored for this project;
/// nothing is derived from any payload (`docs/CLEAN_ROOM.md`).
#[must_use]
pub fn bending_track_train_bsp() -> Vec<u8> {
    bending_track_train_bsp_impl(None)
}

/// Where a rider seated [`BEND_SEAT_OFFSET_X`] units off the pivot lands if
/// [`Level::rotational_carry`]'s quarter turn at the corner is *not*
/// refused: the car's own origin sits at [`BEND_TRAIN_CORNER`] the instant
/// it turns (see [`bend_train_pose`]'s doc comment for why that is the
/// pivot), and the turn swings the seat from `+X` of it to `+Y` of it, so
/// the destination is the corner offset by [`BEND_SEAT_OFFSET_X`] along
/// `+Y`. [`bending_track_train_bsp_with_pillar`] plants a solid pillar
/// centred here — standing across the seat the un-refused carry would land
/// the rider in, but nowhere near the seat's position on either straight
/// segment either side of the turn — so the refusal guard is the only
/// thing standing between the rider and being teleported into it.
///
/// [`Level::rotational_carry`]: crate::level::Level::rotational_carry
pub const BEND_PILLAR_CENTER: [f32; 3] = [
    BEND_TRAIN_CORNER[0],
    BEND_SEAT_OFFSET_X,
    BEND_TRAIN_ORIGIN[2] + BEND_CAR_TOP_Z + 36.0,
];

/// Half-extents of the pillar [`BEND_PILLAR_CENTER`] is centred at: wide
/// and tall enough to certainly contain a standing player hull placed
/// anywhere near that seat, in every axis, while staying well clear of the
/// `y = 0` segment the car (and its seated rider) travel before the turn.
pub const BEND_PILLAR_HALF_EXTENTS: [f32; 3] = [40.0, 40.0, 60.0];

/// [`bending_track_train_bsp`], with one addition: a static pillar
/// (`worldspawn` geometry, submodel `*0`) standing across the world-space
/// spot a rider seated off the car's pivot would be teleported into if the
/// corner's quarter-turn carry were applied unconditionally — see
/// [`BEND_PILLAR_CENTER`]. Used to check that the carry's own
/// `.filter(|carried| !start_solid)` guard (`Systems::player_move`) really
/// does refuse that destination and leave the rider where they were
/// standing, rather than resolving the resulting overlap some other way.
///
/// Every keyvalue and coordinate here is authored for this project;
/// nothing is derived from any payload (`docs/CLEAN_ROOM.md`).
#[must_use]
pub fn bending_track_train_bsp_with_pillar() -> Vec<u8> {
    bending_track_train_bsp_impl(Some((BEND_PILLAR_CENTER, BEND_PILLAR_HALF_EXTENTS)))
}

fn bending_track_train_bsp_impl(pillar: Option<([f32; 3], [f32; 3])>) -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let [ox, oy, oz] = BEND_TRAIN_ORIGIN;
    let [cx, cy, cz] = BEND_TRAIN_CORNER;
    let [ex, ey, ez] = BEND_TRAIN_END;
    let seat_x = ox + BEND_SEAT_OFFSET_X;
    let seat_z = oz + BEND_CAR_TOP_Z + 36.0;
    let speed = BEND_TRAIN_SPEED;
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{seat_x} {oy} {seat_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"targetname\" \"{BEND_TRAIN_NAME}\"\n\
         \"target\" \"ohl_bend1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"{ox} {oy} {oz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_bend1\"\n\
         \"target\" \"ohl_bend2\"\n\"origin\" \"{ox} {oy} {oz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_bend2\"\n\
         \"target\" \"ohl_bend3\"\n\"origin\" \"{cx} {cy} {cz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_bend3\"\n\
         \"origin\" \"{ex} {ey} {ez}\"\n}}\n"
    ));

    // Submodel 0: a void world (so the car's own brush is the only thing
    // holding the passenger up), plus, when `pillar` is given, one static
    // solid brush standing across the corner's post-turn seat.
    let world_brushes = pillar.map_or_else(Vec::new, |(center, half)| {
        let [px, py, pz] = center;
        let [hx, hy, hz] = half;
        vec![CollisionBrush::box_brush(
            [px - hx, py - hy, pz - hz],
            [px + hx, py + hy, pz + hz],
        )]
    });
    let world_heads = b.push_collision_hulls(&world_brushes);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the car, compiled relative to its origin brush.
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [
            -BEND_CAR_HALF_LENGTH,
            -BEND_CAR_HALF_WIDTH,
            BEND_CAR_BOTTOM_Z,
        ],
        [BEND_CAR_HALF_LENGTH, BEND_CAR_HALF_WIDTH, BEND_CAR_TOP_Z],
    )]);
    b.push_model(
        [
            -BEND_CAR_HALF_LENGTH,
            -BEND_CAR_HALF_WIDTH,
            BEND_CAR_BOTTOM_Z,
        ],
        [BEND_CAR_HALF_LENGTH, BEND_CAR_HALF_WIDTH, BEND_CAR_TOP_Z],
        [0.0, 0.0, 0.0],
        car_heads,
        2,
        0,
        0,
    );
    b.build()
}

/// Where [`bending_track_train_bsp`]'s car currently is, for a test that
/// has to express an expectation in the car's own frame.
#[derive(Debug, Clone, Copy)]
pub struct BendTrainPose {
    /// The world point the car's compiled `(0, 0, 0)` — its origin brush —
    /// currently sits at: the same `origin` `sync_brush_collision` hands
    /// `ohl_physics::CollisionModel::set_brush_pose` and the renderer
    /// composes its placement matrix with.
    pub origin: [f32; 3],
    /// The yaw, in degrees, the car is both drawn and collided at.
    pub yaw_degrees: f32,
}

/// Reads [`BendTrainPose`] off a running [`crate::Game`] loaded with
/// [`bending_track_train_bsp`].
///
/// # Panics
/// If the game is not running that fixture, or its train has stopped
/// somewhere with no defined heading.
#[must_use]
pub fn bend_train_pose(game: &crate::Game) -> BendTrainPose {
    let registry = game.registry();
    let entity = registry.find(BEND_TRAIN_NAME)[0];
    let authored = registry
        .world
        .get::<&ohl_game::registry::Transform>(entity)
        .expect("the fixture train has a transform")
        .origin;
    let origin = authored + ohl_game::pose::brush_offset(registry, entity);
    let (axis, yaw_degrees, _) = ohl_game::pose::brush_pose_rotation(registry, entity);
    assert_eq!(
        axis,
        ohl_physics::Vec3::Z,
        "the fixture train is on a horizontal segment and must report a yaw"
    );
    BendTrainPose {
        origin: origin.to_array(),
        yaw_degrees,
    }
}

// ---------------------------------------------------------------------
// A `func_tracktrain` the map spawns the player *inside*
// ---------------------------------------------------------------------

/// The map name the embedded-spawn track-train fixture is published under.
pub const EMBEDDED_SPAWN_MAP: &str = "ohlembedspawnsynth";

/// The `targetname` of the fixture's `func_tracktrain`.
pub const EMBEDDED_SPAWN_TRAIN_NAME: &str = "ohl_embed_train";

/// The fixture's origin brush and first chain node, non-zero for the same
/// reason [`BEND_TRAIN_ORIGIN`] is: the ordinary origin-brush shape, not
/// the world-baked one.
pub const EMBEDDED_SPAWN_TRAIN_ORIGIN: [f32; 3] = [100.0, 0.0, 0.0];

/// The chain's only other node: a long straight run along `+X`, so the
/// whole fixture is about *departure*, with no corner to confuse it.
pub const EMBEDDED_SPAWN_TRAIN_END: [f32; 3] = [1600.0, 0.0, 0.0];

/// The car's compiled half-extents about its own origin brush, as
/// [`BEND_CAR_HALF_LENGTH`]/[`BEND_CAR_HALF_WIDTH`].
pub const EMBEDDED_SPAWN_CAR_HALF_LENGTH: f32 = 96.0;
/// See [`EMBEDDED_SPAWN_CAR_HALF_LENGTH`].
pub const EMBEDDED_SPAWN_CAR_HALF_WIDTH: f32 = 40.0;
/// The car floor's top, in the compiled frame.
pub const EMBEDDED_SPAWN_CAR_TOP_Z: f32 = 8.0;
/// The car floor's bottom, in the compiled frame.
pub const EMBEDDED_SPAWN_CAR_BOTTOM_Z: f32 = -8.0;

/// How far along the car, from its origin brush, the `info_player_start`
/// is placed.
pub const EMBEDDED_SPAWN_SEAT_OFFSET_X: f32 = 64.0;

/// How far *below* a clear standing height on the car's floor the
/// fixture's `info_player_start` is placed, so the standing hull starts
/// overlapping the car's own solid.
///
/// This is the shape a real map hands the port: a spawn point authored
/// against the original engine's own compiled clip tree, landing a few
/// units inside this project's when the two do not agree to the unit (see
/// `docs/FORMAT_SOURCES.md`, "Collision hulls and player movement"). Big
/// enough here that no epsilon can absorb it, and well within the bound
/// `ohl_physics::settle_at_spawn`'s nudge may spend.
pub const EMBEDDED_SPAWN_DEPTH: f32 = 12.0;

/// The train's `speed`/`startspeed`, units per second. Non-zero
/// `startspeed`, so the car is moving on the very first simulation step —
/// there is no grace period in which a passenger could fall onto it.
pub const EMBEDDED_SPAWN_TRAIN_SPEED: f32 = 100.0;

/// A void world (submodel `*0`) carrying a `func_tracktrain` (submodel
/// `*1`) on a two-node straight `path_track` chain, with an
/// `info_player_start` placed [`EMBEDDED_SPAWN_DEPTH`] units *inside* the
/// car's own solid rather than cleanly on top of it, and a `startspeed`
/// that has the car moving from the first step.
///
/// Nothing here comes from any game installation; every keyvalue and
/// coordinate is authored for this project (`docs/CLEAN_ROOM.md`).
#[must_use]
pub fn embedded_spawn_track_train_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let [ox, oy, oz] = EMBEDDED_SPAWN_TRAIN_ORIGIN;
    let [ex, ey, ez] = EMBEDDED_SPAWN_TRAIN_END;
    let seat_x = ox + EMBEDDED_SPAWN_SEAT_OFFSET_X;
    let seat_z = oz + EMBEDDED_SPAWN_CAR_TOP_Z + 36.0 - EMBEDDED_SPAWN_DEPTH;
    let speed = EMBEDDED_SPAWN_TRAIN_SPEED;
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{seat_x} {oy} {seat_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"targetname\" \"{EMBEDDED_SPAWN_TRAIN_NAME}\"\n\
         \"target\" \"ohl_embed1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"{ox} {oy} {oz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_embed1\"\n\
         \"target\" \"ohl_embed2\"\n\"origin\" \"{ox} {oy} {oz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_embed2\"\n\
         \"origin\" \"{ex} {ey} {ez}\"\n}}\n"
    ));

    // Submodel 0: a void world, so the car's own brush is the only thing
    // that can hold the passenger up — a rider who is dropped here has
    // nothing else to land on, which is what makes the ride measurable.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the car, compiled relative to its origin brush.
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        [
            -EMBEDDED_SPAWN_CAR_HALF_LENGTH,
            -EMBEDDED_SPAWN_CAR_HALF_WIDTH,
            EMBEDDED_SPAWN_CAR_BOTTOM_Z,
        ],
        [
            EMBEDDED_SPAWN_CAR_HALF_LENGTH,
            EMBEDDED_SPAWN_CAR_HALF_WIDTH,
            EMBEDDED_SPAWN_CAR_TOP_Z,
        ],
    )]);
    b.push_model(
        [
            -EMBEDDED_SPAWN_CAR_HALF_LENGTH,
            -EMBEDDED_SPAWN_CAR_HALF_WIDTH,
            EMBEDDED_SPAWN_CAR_BOTTOM_Z,
        ],
        [
            EMBEDDED_SPAWN_CAR_HALF_LENGTH,
            EMBEDDED_SPAWN_CAR_HALF_WIDTH,
            EMBEDDED_SPAWN_CAR_TOP_Z,
        ],
        [0.0, 0.0, 0.0],
        car_heads,
        2,
        0,
        0,
    );
    b.build()
}

/// Where [`embedded_spawn_track_train_bsp`]'s car's compiled `(0, 0, 0)` —
/// its origin brush — currently sits, read the same way
/// [`bend_train_pose`] reads its own fixture's.
///
/// # Panics
/// If the game is not running that fixture.
#[must_use]
pub fn embedded_spawn_train_origin(game: &crate::Game) -> [f32; 3] {
    let registry = game.registry();
    let entity = registry.find(EMBEDDED_SPAWN_TRAIN_NAME)[0];
    let authored = registry
        .world
        .get::<&ohl_game::registry::Transform>(entity)
        .expect("the fixture train has a transform")
        .origin;
    (authored + ohl_game::pose::brush_offset(registry, entity)).to_array()
}

// ---------------------------------------------------------------------
// A `func_door_rotating` blocking a corridor
// ---------------------------------------------------------------------

/// The map name the rotating-door fixture is published under.
pub const ROTATING_DOOR_MAP: &str = "ohlrotdoorsynth";

/// The `targetname` of the fixture's `func_door_rotating`.
pub const ROTATING_DOOR_NAME: &str = "ohl_rot_door";

/// The rotating door leaf's world-space closed-pose box: 16 units thick
/// along `X`, spanning `y` from `-88` to `88` (176 units — leaving only an
/// 8-unit gap to each of the corridor's own walls at `y = +/-96`, too
/// narrow for the 32-unit-wide standing hull to slip through), full
/// height. Sized generously relative to the standing hull's own 32x32
/// footprint: a hull's clip-tree planes are pre-expanded outward by half
/// its own box (see `ohl_formats::test_support::Bsp30Builder::
/// push_collision_hulls`'s `expand_plane`) *before* this package's rotation
/// is applied to them, so a door leaf sized only a little larger than the
/// hull swings out a pre-expanded footprint comparable in size to the
/// corridor itself and never really clears it — this fixture's numbers
/// were chosen empirically (a standalone probe against
/// `ohl_physics::CollisionModel::set_brush_pose`) to leave the open door's
/// full pre-expanded sweep entirely outside the corridor's own walkable
/// width, the same margin a real map's mapper leaves by building a door
/// frame and a wall recess substantially bigger than the player. Used only
/// for this module's doc comments and the tests that assert against it;
/// [`rotating_door_bsp`] itself compiles the submodel relative to
/// [`ROTATING_DOOR_PIVOT`] (see its own doc comment for why).
pub const ROTATING_DOOR_MINS: [f32; 3] = [184.0, -88.0, 0.0];
/// See [`ROTATING_DOOR_MINS`].
pub const ROTATING_DOOR_MAXS: [f32; 3] = [200.0, 88.0, 96.0];

/// The world-space point the door pivots about — a real map's own
/// `func_door_rotating`'s required "origin brush" position (TWHL wiki
/// `func_door_rotating`, `docs/FORMAT_SOURCES.md`) — and, per
/// [`rotating_door_bsp`]'s own doc comment, also where the compiler leaves
/// the compiled submodel's local `(0, 0, 0)`. Chosen as
/// [`ROTATING_DOOR_MINS`]'s own `x`-midpoint, `y = -88` edge.
pub const ROTATING_DOOR_PIVOT: [f32; 3] = [192.0, -88.0, 0.0];

/// A `worldspawn`-only, faceless world (collision only; nothing here draws
/// anything, exactly like [`killable_brush_floor_bsp`]) shaped as a
/// corridor 192 units wide (`y` in `-96..96`, unbounded along `x`) plus a
/// *real* solid submodel 1 for the door leaf — unlike
/// `door_behind_touch_trigger_bsp`'s door, whose submodel carries no
/// collision hulls of its own (bare `-1` heads) because that fixture only
/// exercises the map-logic state machine. This one needs a door that
/// actually blocks and unblocks the corridor, so its hulls are pushed the
/// same way [`killable_brush_floor_bsp`]'s floor slab's are.
///
/// The submodel's own box is compiled *relative to
/// [`ROTATING_DOOR_PIVOT`]* — `[ROTATING_DOOR_MINS] - [ROTATING_DOOR_PIVOT]`
/// .. `[ROTATING_DOOR_MAXS] - [ROTATING_DOOR_PIVOT]` — rather than at its
/// absolute world position, matching how a real map's own `func_train`
/// compiles (see `crate::render::rotated_placement`'s and
/// `ohl_game::pose::track_train_transform`'s doc comments: "the compiler
/// writes that origin brush's position into the entity's `origin`
/// keyvalue and stores the submodel's geometry relative to it"), which
/// [`rotating_door_entities`]'s `origin` keyvalue must then equal exactly
/// so the two cancel back to [`ROTATING_DOOR_MINS`]/[`ROTATING_DOOR_MAXS`]
/// at rest (`distance`/`angle_deg = 0`).
#[must_use]
pub fn rotating_door_bsp(entities: &str) -> Vec<u8> {
    let local_mins = [
        ROTATING_DOOR_MINS[0] - ROTATING_DOOR_PIVOT[0],
        ROTATING_DOOR_MINS[1] - ROTATING_DOOR_PIVOT[1],
        ROTATING_DOOR_MINS[2] - ROTATING_DOOR_PIVOT[2],
    ];
    let local_maxs = [
        ROTATING_DOOR_MAXS[0] - ROTATING_DOOR_PIVOT[0],
        ROTATING_DOOR_MAXS[1] - ROTATING_DOOR_PIVOT[1],
        ROTATING_DOOR_MAXS[2] - ROTATING_DOOR_PIVOT[2],
    ];

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);
    // The world: a floor at z=0 plus two side walls forming the corridor.
    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -96.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -96.0),
    ]);
    let door_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(local_mins, local_maxs)]);
    b.push_model([-4096.0; 3], [4096.0; 3], [0.0; 3], world_heads, 2, 0, 0);
    b.push_model(local_mins, local_maxs, [0.0; 3], door_heads, 2, 0, 0);
    // Submodel 2: a *non-solid* volume around the player start, carrying
    // only a bounding box (bare hull heads, like
    // `door_behind_touch_trigger_bsp`'s own trigger submodel), for a
    // fixture that wants the door opened by walking rather than by forcing
    // its state. Entity texts that never reference `"*2"` — including
    // [`rotating_door_entities`] — simply leave it unused.
    let trigger_heads = b.push_collision_hulls(&[]);
    b.push_model(
        ROTATING_DOOR_TRIGGER_MINS,
        ROTATING_DOOR_TRIGGER_MAXS,
        [0.0; 3],
        trigger_heads,
        2,
        0,
        0,
    );
    b.build()
}

/// The touch volume [`rotating_door_trigger_entities`]'s
/// `trigger_multiple` occupies: a slab of the corridor around the
/// `info_player_start`, so a player who does nothing at all is already
/// standing in it on the first tick.
pub const ROTATING_DOOR_TRIGGER_MINS: [f32; 3] = [120.0, -96.0, 0.0];
/// See [`ROTATING_DOOR_TRIGGER_MINS`].
pub const ROTATING_DOOR_TRIGGER_MAXS: [f32; 3] = [175.0, 96.0, 96.0];

/// The `targetname` of [`rotating_door_trigger_entities`]'s trigger.
pub const ROTATING_DOOR_TRIGGER_NAME: &str = "ohl_rot_door_trigger";

/// [`rotating_door_entities`], plus a `trigger_multiple` (submodel `*2`)
/// covering the player start and targeting the door — so the door is
/// opened by the *player*, through the engine's own touch-trigger phase,
/// with the player as the activator whose side of the hinge plane decides
/// which way it swings (`ohl_game::registry::RotatingDoorSwing`,
/// `docs/FORMAT_SOURCES.md` item 26). No bytes here come from any game
/// installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rotating_door_trigger_entities() -> String {
    format!(
        "{}{{\n\"classname\" \"trigger_multiple\"\n\
         \"targetname\" \"{ROTATING_DOOR_TRIGGER_NAME}\"\n\
         \"target\" \"{ROTATING_DOOR_NAME}\"\n\"model\" \"*2\"\n\
         \"wait\" \"10\"\n\"origin\" \"0 0 0\"\n}}\n",
        rotating_door_entities(),
    )
}

/// A `worldspawn` plus an `info_player_start` short of the door (`x =
/// 150`, facing `+x`, the corridor's own length — close enough for a
/// `use` press to reach the door's brush-centre, `ohl_engine::USE_RADIUS`
/// being 64 units, but still short of its closed leaf at `x = 184`), and a
/// `func_door_rotating` (submodel `*1`, targetname [`ROTATING_DOOR_NAME`])
/// pivoting about [`ROTATING_DOOR_PIVOT`] through 90 degrees at 360
/// degrees/second (a quarter turn in a quarter second) about the default
/// `Z` axis, and `wait = -1` so it never auto-closes once opened (a test
/// can walk through it without racing a timer). No bytes here come from
/// any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rotating_door_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door_rotating\"\n\"targetname\" \"{ROTATING_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"origin\" \"{} {} {}\"\n}}\n",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

/// [`rotating_door_entities`], with the "Use Only" spawnflag
/// (`ohl_game::registry::SPAWNFLAG_DOOR_USE_ONLY`, 256) set on the door: a
/// closed door built this way never opens from the player's own touch
/// (`ohl_game::logic::Simulation::touch_doors`), only from a `use` press or
/// another entity's fire chain — see `docs/FORMAT_SOURCES.md` item 30. No
/// bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rotating_door_use_only_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door_rotating\"\n\"targetname\" \"{ROTATING_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"spawnflags\" \"{}\"\n\
         \"origin\" \"{} {} {}\"\n}}\n",
        ohl_game::registry::SPAWNFLAG_DOOR_USE_ONLY,
        ROTATING_DOOR_PIVOT[0],
        ROTATING_DOOR_PIVOT[1],
        ROTATING_DOOR_PIVOT[2],
    )
}

/// [`rotating_door_entities`], with the door's own `targetname` omitted:
/// the Sven Co-op wiki's `Func_door` page (`docs/FORMAT_SOURCES.md` item
/// 30) documents a door as opening on touch "unless they have a name, in
/// which's case they require to be triggered manually", so
/// `ohl_game::logic::Simulation::touch_doors` only ever opens an unnamed
/// door — this fixture is the corridor's own unnamed variant, used to
/// prove that path in isolation from the "Use Only"/"Passable" flag
/// exclusions [`rotating_door_use_only_entities`] and a `func_door`'s own
/// spawnflags cover. Since the door has no `targetname`, a test using this
/// fixture cannot look its entity up by name (`Registry::find`) the way
/// every other fixture's own test helper does; it must query the registry
/// for its one `Door` component directly instead. No bytes here come from
/// any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rotating_door_unnamed_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door_rotating\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"origin\" \"{} {} {}\"\n}}\n",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

/// The map name the classname-solidity corridor fixture is published
/// under.
pub const CLIP_CORRIDOR_MAP: &str = "ohlclipcorridorsynth";

/// [`rotating_door_bsp`]'s own corridor geometry (a floor plus two side
/// walls at `y = +/-96`, unbounded along `x`, plus a real solid submodel
/// `*1` spanning [`ROTATING_DOOR_MINS`]..[`ROTATING_DOOR_MAXS`]), but with
/// a plain, non-moving brush entity of the caller's own `classname` as
/// that submodel instead of a `func_door_rotating` — no
/// `speed`/`distance`/`wait`, since this fixture exists to prove
/// *solidity to the player*, not movement. Reuses [`ROTATING_DOOR_PIVOT`]
/// purely because [`rotating_door_bsp`] compiles submodel `*1` relative to
/// that point (see its own doc comment); the brush itself never moves
/// here. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn corridor_brush_entities(classname: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"{classname}\"\n\"model\" \"*1\"\n\
         \"origin\" \"{} {} {}\"\n}}\n",
        ROTATING_DOOR_PIVOT[0], ROTATING_DOOR_PIVOT[1], ROTATING_DOOR_PIVOT[2],
    )
}

// ---------------------------------------------------------------------
// A `func_rotating` turntable the player stands on
// ---------------------------------------------------------------------

/// The map name the rotating-platform fixture is published under.
pub const ROTATING_PLATFORM_MAP: &str = "ohlrotplatsynth";

/// The `targetname` of the fixture's `func_rotating`.
pub const ROTATING_PLATFORM_NAME: &str = "ohl_rot_platform";

/// The turntable's half extent on `X` and `Y`; its top surface is at
/// `z = 0` and it is 16 units thick. Large enough that a rider standing at
/// [`ROTATING_PLATFORM_SPAWN_RADIUS`] stays well inside the disc inscribed
/// in it for a full revolution.
pub const ROTATING_PLATFORM_HALF_EXTENT: f32 = 192.0;

/// How far from the turntable's axis the fixture's `info_player_start`
/// stands: far enough out that the tangential ride is unmistakable (tens
/// of units per second at the fixture's own `speed`), well inside the
/// disc inscribed in [`ROTATING_PLATFORM_HALF_EXTENT`].
pub const ROTATING_PLATFORM_SPAWN_RADIUS: f32 = 96.0;

/// Degrees per second the fixture's `func_rotating` spins at (its `speed`
/// keyvalue; TWHL wiki `func_rotating`, `docs/FORMAT_SOURCES.md`).
pub const ROTATING_PLATFORM_SPEED: f32 = 45.0;

/// A world with *no floor at all* (an open void) whose only solid is a
/// turntable compiled as submodel `*1`, relative to its own origin brush
/// at the world origin — the same convention [`rotating_door_bsp`] uses
/// and that `docs/FORMAT_SOURCES.md` item 24 records for a real rotating
/// brush entity.
///
/// The void world is the point: a player who is not carried by, or who
/// falls through, the turntable has nothing else to stand on, so either
/// failure shows up immediately as a fall instead of hiding behind the
/// worldspawn floor a less pointed fixture would have.
#[must_use]
pub fn rotating_platform_bsp(entities: &str) -> Vec<u8> {
    let mins = [
        -ROTATING_PLATFORM_HALF_EXTENT,
        -ROTATING_PLATFORM_HALF_EXTENT,
        -16.0,
    ];
    let maxs = [
        ROTATING_PLATFORM_HALF_EXTENT,
        ROTATING_PLATFORM_HALF_EXTENT,
        0.0,
    ];
    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model([-4096.0; 3], [4096.0; 3], [0.0; 3], world_heads, 2, 0, 0);
    let slab_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(mins, maxs)]);
    b.push_model(mins, maxs, [0.0; 3], slab_heads, 2, 0, 0);
    b.build()
}

/// A `worldspawn`, an `info_player_start` standing on the turntable at
/// [`ROTATING_PLATFORM_SPAWN_RADIUS`], and a `func_rotating` (submodel
/// `*1`) spinning about the documented default `Z` axis at
/// [`ROTATING_PLATFORM_SPEED`] degrees per second, already on at map load
/// (`spawnflags` bit 1, "Start On";
/// `ohl_game::registry::SPAWNFLAG_ROTATING_START_ON`). No bytes here come
/// from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rotating_platform_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{ROTATING_PLATFORM_SPAWN_RADIUS} 0 40\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_rotating\"\n\"targetname\" \"{ROTATING_PLATFORM_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"{ROTATING_PLATFORM_SPEED}\"\n\"spawnflags\" \"1\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A `func_rot_button` pressed through the real `use_pressed` input path
// ---------------------------------------------------------------------

/// The map name the `func_rot_button` fixture is published under.
pub const ROT_BUTTON_MAP: &str = "ohlrotbuttonsynth";

/// The `targetname` of the fixture's `func_rot_button`.
pub const ROT_BUTTON_NAME: &str = "ohl_rot_button";

/// The `targetname` of the `func_door` the button targets.
pub const ROT_BUTTON_DOOR_NAME: &str = "ohl_rot_button_door";

/// A `worldspawn`-only flat floor (collision only, matching
/// [`door_behind_touch_trigger_bsp`]'s own floor) plus one real brush
/// submodel — submodel 1, the button — placed with its own compiled bounds
/// centred on [`ROT_BUTTON_CENTER`].
///
/// `ohl_game::registry::BrushCenter` is the compiled bounds' midpoint
/// *plus* the entity's `origin` keyvalue, unconditionally (`docs/
/// FORMAT_SOURCES.md`'s `TODO(black-box)` item 25 is resolved: a
/// proximity-based `use` press already agrees with a brush entity's real
/// placed position, whether its `origin` is zero or not — see
/// `ohl_game::pose::brush_center`, which this fixture's own integration
/// test drives through the real `use_pressed` path
/// (`ohl_game::logic::find_usable_within`), the same mechanism
/// `rotating_door_bsp`'s own (nonzero-`origin`) door now uses too). This
/// fixture's `origin` keyvalue of `0 0 0` is simply a simplification — a
/// zero `origin` adds nothing on top of the compiled bounds, so the
/// button's box can be placed directly at [`ROT_BUTTON_CENTER`] without
/// also working out a compile-relative-to-pivot offset — not a workaround
/// for a gap that still needs one.
#[must_use]
pub fn rot_button_bsp(entities: &str) -> Vec<u8> {
    const HALF: f32 = 256.0;
    const HEIGHT: f32 = 256.0;
    const BUTTON_HALF: f32 = 16.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ]);
    b.push_model(
        [-HALF, -HALF, 0.0],
        [HALF, HALF, HEIGHT],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the button itself — no faces or collision of its own
    // (only its bounding box, matching `door_behind_touch_trigger_bsp`'s
    // own door/trigger submodels), centred on [`ROT_BUTTON_CENTER`] (chosen
    // to sit close to a standing player's own eye height, not world `(0, 0,
    // 0)`, so a real `use_pressed` proximity search — measured from the
    // *eye*, `origin.z` plus `MoveConfig::view_height_standing` (28) —
    // reaches it; see [`rot_button_entities`]'s own doc comment for the
    // spawn point this was measured against).
    b.push_model(
        [
            ROT_BUTTON_CENTER[0] - BUTTON_HALF,
            ROT_BUTTON_CENTER[1] - BUTTON_HALF,
            ROT_BUTTON_CENTER[2] - BUTTON_HALF,
        ],
        [
            ROT_BUTTON_CENTER[0] + BUTTON_HALF,
            ROT_BUTTON_CENTER[1] + BUTTON_HALF,
            ROT_BUTTON_CENTER[2] + BUTTON_HALF,
        ],
        [0.0; 3],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );
    b.build()
}

/// The world-space centre [`rot_button_bsp`] compiles the button's bounding
/// box around; see that function's own doc comment.
pub const ROT_BUTTON_CENTER: [f32; 3] = [0.0, 0.0, 64.0];

/// A `worldspawn` plus an `info_player_start` (`0 -24 40`; a standing
/// player's eye — `origin.z` plus the published `view_height_standing`, 28
/// — sits at `(0, -24, 68)`, about 24 units from [`ROT_BUTTON_CENTER`] and
/// well inside `ohl_engine::USE_RADIUS`), a `func_rot_button` (submodel
/// `*1`, targetname [`ROT_BUTTON_NAME`], `origin 0 0 0`) rotating 90
/// degrees at 360 degrees/second (a quarter turn in a quarter second) about
/// the default `Z` axis with `wait = -1` (stays pressed once opened),
/// targeting a `func_door` ([`ROT_BUTTON_DOOR_NAME`], also `wait = -1`) so a
/// real `use_pressed` press can be observed opening it end to end. No bytes
/// here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rot_button_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -24 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_rot_button\"\n\"targetname\" \"{ROT_BUTTON_NAME}\"\n\
         \"target\" \"{ROT_BUTTON_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{ROT_BUTTON_DOOR_NAME}\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A `momentary_rot_button` driving a `momentary_door` through the real
// `use_held` input path (M9.8, `docs/FORMAT_SOURCES.md` item 29)
// ---------------------------------------------------------------------

/// The map name the `momentary_rot_button`/`momentary_door` fixture is
/// published under.
pub const MOMENTARY_DOOR_MAP: &str = "ohlmomentarydoorsynth";

/// The `targetname` of the fixture's `momentary_rot_button`.
pub const MOMENTARY_ROT_BUTTON_NAME: &str = "ohl_momentary_button";

/// The `targetname` of the `momentary_door` the button targets.
pub const MOMENTARY_DOOR_NAME: &str = "ohl_momentary_door";

/// The world-space centre [`momentary_door_bsp`] compiles the button's
/// bounding box around; see [`ROT_BUTTON_CENTER`]'s own doc comment for why
/// this fixture reuses the same placement convention (close to a standing
/// player's own eye height).
pub const MOMENTARY_BUTTON_CENTER: [f32; 3] = [0.0, 0.0, 64.0];

/// The world-space centre [`momentary_door_bsp`] compiles the door's own
/// bounding box around at `fraction = 0.0` — placed well clear of the
/// button so the two submodels' boxes never overlap.
pub const MOMENTARY_DOOR_CENTER: [f32; 3] = [256.0, 0.0, 64.0];

/// A `worldspawn`-only flat floor (matching [`rot_button_bsp`]'s own),
/// submodel 1 (the button, centred on [`MOMENTARY_BUTTON_CENTER`]) and
/// submodel 2 (the door, centred on [`MOMENTARY_DOOR_CENTER`], `64` units
/// wide along `+X` so `momentary_door_entities`'s `angle 0` gives it a
/// `travel_distance` of `64`). Neither submodel carries faces or collision
/// of its own, the same simplification [`rot_button_bsp`] already makes.
#[must_use]
pub fn momentary_door_bsp(entities: &str) -> Vec<u8> {
    const HALF: f32 = 512.0;
    const HEIGHT: f32 = 256.0;
    const BUTTON_HALF: f32 = 16.0;
    const DOOR_HALF_X: f32 = 32.0;
    const DOOR_HALF_YZ: f32 = 16.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ]);
    b.push_model(
        [-HALF, -HALF, 0.0],
        [HALF, HALF, HEIGHT],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the button.
    b.push_model(
        [
            MOMENTARY_BUTTON_CENTER[0] - BUTTON_HALF,
            MOMENTARY_BUTTON_CENTER[1] - BUTTON_HALF,
            MOMENTARY_BUTTON_CENTER[2] - BUTTON_HALF,
        ],
        [
            MOMENTARY_BUTTON_CENTER[0] + BUTTON_HALF,
            MOMENTARY_BUTTON_CENTER[1] + BUTTON_HALF,
            MOMENTARY_BUTTON_CENTER[2] + BUTTON_HALF,
        ],
        [0.0; 3],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );
    // Submodel 2: the door, `2 * DOOR_HALF_X` (64) wide along `X` — the
    // axis `momentary_door_entities`'s `angle 0` selects as `movedir`, so
    // this is the bounding-box-derived `travel_distance`
    // `ohl_game::registry::brush_travel_distance` computes for it.
    b.push_model(
        [
            MOMENTARY_DOOR_CENTER[0] - DOOR_HALF_X,
            MOMENTARY_DOOR_CENTER[1] - DOOR_HALF_YZ,
            MOMENTARY_DOOR_CENTER[2] - DOOR_HALF_YZ,
        ],
        [
            MOMENTARY_DOOR_CENTER[0] + DOOR_HALF_X,
            MOMENTARY_DOOR_CENTER[1] + DOOR_HALF_YZ,
            MOMENTARY_DOOR_CENTER[2] + DOOR_HALF_YZ,
        ],
        [0.0; 3],
        [-1, -1, -1, -1],
        0,
        0,
        0,
    );
    b.build()
}

/// A `worldspawn` plus an `info_player_start` (`0 -24 40`; the same
/// distance-to-button placement [`rot_button_entities`] uses, well inside
/// `ohl_engine::USE_RADIUS` of [`MOMENTARY_BUTTON_CENTER`]), a
/// `momentary_rot_button` (submodel `*1`, targetname
/// [`MOMENTARY_ROT_BUTTON_NAME`], `origin 0 0 0`) turning 90 degrees at 180
/// degrees per second (a full `0.0..=1.0` sweep in half a second) about the
/// documented default `Z` axis, with the "Auto return" spawnflag (`16`) set
/// so releasing `use` returns it — targeting a `momentary_door` (submodel
/// `*2`, targetname [`MOMENTARY_DOOR_NAME`]) that travels along `+X`
/// (`angle 0`) at a matching `128` units/second (`travel_distance = 64`, so
/// the same `speed / travel_distance` ratio as the button's own
/// `speed / distance`, keeping the two in step tick for tick) so a real
/// `use_held` press can be observed opening it, and releasing `use` can be
/// observed closing it again, end to end. No bytes here come from any game
/// installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn momentary_door_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -24 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"momentary_rot_button\"\n\
         \"targetname\" \"{MOMENTARY_ROT_BUTTON_NAME}\"\n\
         \"target\" \"{MOMENTARY_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"180\"\n\"distance\" \"90\"\n\
         \"returnspeed\" \"180\"\n\"spawnflags\" \"16\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"momentary_door\"\n\"targetname\" \"{MOMENTARY_DOOR_NAME}\"\n\
         \"model\" \"*2\"\n\"speed\" \"128\"\n\"angle\" \"0\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    )
}

/// The map name the health-gated `func_rot_button` fixture below is
/// published under (distinct from [`ROT_BUTTON_MAP`] purely so both
/// fixtures can be told apart at a glance; nothing in either loads them
/// together).
pub const ROT_BUTTON_HEALTH_MAP: &str = "ohlrotbuttonhealthsynth";

/// The `targetname` of the health-gated `func_rot_button` fixture below.
pub const ROT_BUTTON_HEALTH_NAME: &str = "ohl_rot_button_health";

/// The `targetname` of the `func_door` the health-gated fixture's button
/// targets.
pub const ROT_BUTTON_HEALTH_DOOR_NAME: &str = "ohl_rot_button_health_door";

/// The fixture's `func_rot_button` `health` keyvalue: comfortably below the
/// published 40-damage `.357 Magnum` single-shot ([`ohl_combat::spec`]'s own
/// citation for `WeaponId::Python`), so one landed shot exhausts it in a
/// single hit rather than needing two.
pub const ROT_BUTTON_HEALTH: f32 = 30.0;

/// [`rot_button_entities`]'s own fixture, with two additions: the
/// `func_rot_button` carries a `health` keyvalue
/// ([`ROT_BUTTON_HEALTH`]), and a `weapon_357` sits exactly at the
/// `info_player_start`'s own origin so the very first step's pickup touch
/// (`crate::pickups::PICKUP_TOUCH_RADIUS`, well inside a zero-distance
/// touch) already grants it — a real player armed only by walking over a
/// weapon, not one spawned holding it. The spawn's `angle` is `90` (yaw
/// only, pitch `0`; GoldSrc's own `angle` convention — see
/// `ohl_world::spawn::PlayerSpawn`'s doc comment for "counter-clockwise
/// around +X") rather than [`rot_button_entities`]'s own `0`: that fixture
/// is only ever pressed by *proximity* (`find_usable_within`, which does
/// not care which way the player faces), but this one is pressed by a real
/// hitscan trace along the camera's view direction, which does. At `angle
/// 90` the camera faces `+y` directly at [`ROT_BUTTON_CENTER`]; since the
/// spawn's standing eye height (`origin.z` plus the published
/// `view_height_standing`, 28, i.e. `z = 68`) already sits inside the
/// button's own compiled box (`z` in `48..=80`, [`rot_button_bsp`]'s own
/// `BUTTON_HALF`), a level `pitch = 0` shot lands without needing to aim
/// up or down. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn rot_button_health_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -24 40\"\n\
         \"angle\" \"90\"\n}}\n\
         {{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 -24 40\"\n}}\n\
         {{\n\"classname\" \"func_rot_button\"\n\"targetname\" \"{ROT_BUTTON_HEALTH_NAME}\"\n\
         \"target\" \"{ROT_BUTTON_HEALTH_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"360\"\n\"distance\" \"90\"\n\"wait\" \"-1\"\n\
         \"health\" \"{ROT_BUTTON_HEALTH}\"\n\"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{ROT_BUTTON_HEALTH_DOOR_NAME}\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A `momentary_rot_button` reached by walking, then driven by held `use`
// ---------------------------------------------------------------------

/// The map name the `momentary_rot_button` walk-and-hold fixture below is
/// published under.
pub const MOMENTARY_ROT_BUTTON_MAP: &str = "ohlmomentaryrotbuttonsynth";

/// The `targetname` of the fixture's `momentary_rot_button`.
pub const MOMENTARY_ROT_BUTTON_WALK_NAME: &str = "ohl_momentary_rot_button";

/// [`rot_button_bsp`]'s own geometry (a flat floor plus one bounding-box-
/// only submodel centred on [`ROT_BUTTON_CENTER`]), reused unchanged: a
/// `momentary_rot_button` needs exactly the same "one real brush submodel"
/// shape a `func_rot_button` does for `ohl_game::pose::brush_center` (and so
/// `ohl_game::logic::find_momentary_rot_button_within`'s own proximity
/// search) to find it at all.
///
/// A `worldspawn` plus an `info_player_start` well outside
/// `ohl_engine::USE_RADIUS` of [`ROT_BUTTON_CENTER`] (`0 -200 40`; about 200
/// units of horizontal distance, more than three times the 64-unit radius),
/// facing it (`angle 90`, the same "counter-clockwise from +x" convention
/// [`rot_button_health_entities`]'s own doc comment explains), and a
/// `momentary_rot_button` (submodel `*1`, targetname
/// [`MOMENTARY_ROT_BUTTON_WALK_NAME`], `origin 0 0 0`, `speed 45`/`distance 90`)
/// with no `target` — this project does not implement `momentary_door`
/// (`docs/FORMAT_SOURCES.md` item 27's own documented gap), so nothing
/// would read one back out regardless. Combined `forward`+`use_held` input
/// must first close the distance (walking) before the proximity search
/// (`ohl_engine::Systems::triggers_and_movers`, phase 12) can find and
/// start driving the valve — the same "walk then hold" shape a real
/// combat-smoke scenario would use, see
/// `crates/ohl-engine/tests/momentary_rot_button.rs`'s own module doc for
/// why this synthetic fixture stands in for one. No bytes here come from
/// any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn momentary_rot_button_entities() -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -200 40\"\n\
         \"angle\" \"90\"\n}}\n\
         {{\n\"classname\" \"momentary_rot_button\"\n\
         \"targetname\" \"{MOMENTARY_ROT_BUTTON_WALK_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"45\"\n\"distance\" \"90\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A closed door gating a `trigger_changelevel` beyond it
// ---------------------------------------------------------------------

/// The map name the reachability-report fixture is published under.
pub const REACH_DOOR_MAP: &str = "ohlreachdoorsynth";

/// The `targetname` of the fixture's blocking `func_door`.
pub const REACH_DOOR_NAME: &str = "ohl_reach_door";

/// The closed door leaf's world-space box: a real solid submodel filling
/// the whole 192-unit-wide corridor (`y` in `-96..96`, an 8-unit margin to
/// each wall, matching [`ROTATING_DOOR_MINS`]'s own reasoning), 16 units
/// thick along `x`, full corridor height.
pub const REACH_DOOR_MINS: [f32; 3] = [184.0, -88.0, 0.0];
/// See [`REACH_DOOR_MINS`].
pub const REACH_DOOR_MAXS: [f32; 3] = [200.0, 88.0, 96.0];

/// A `worldspawn`-only corridor (`y` in `-96..96`, unbounded along `x`,
/// floor at `z = 0`, no ceiling) plus a *real* solid submodel 1 for a
/// plain translating door leaf at [`REACH_DOOR_MINS`]/[`REACH_DOOR_MAXS`]
/// — compiled at its absolute world position, unlike
/// [`rotating_door_bsp`]'s door, because a plain `func_door` (not a
/// `func_door_rotating`) needs no origin-brush-relative compile (see
/// [`rotating_door_bsp`]'s own doc comment for why a rotating one does) —
/// and a *non-solid* submodel 2 (bare hull heads, like
/// [`door_behind_touch_trigger_bsp`]'s touch trigger) beyond the door, for
/// a `trigger_changelevel` volume. No bytes here come from any game
/// installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_door_bsp(entities: &str) -> Vec<u8> {
    const TRIGGER_MINS: [f32; 3] = [300.0, -96.0, 0.0];
    const TRIGGER_MAXS: [f32; 3] = [360.0, 96.0, 96.0];
    // A closed box, not an unbounded corridor like `rotating_door_bsp`'s:
    // this module's reachability walk floods every reachable cell rather
    // than driving a scripted number of ticks, so the fixture's own
    // reachable area has to stay small and finite for the walk (and its
    // test) to run quickly and deterministically. `X_MIN`/`X_MAX` cap the
    // corridor well past the changelevel trigger; `CEILING` caps its
    // height well past where the door leaf ends up once slid open.
    const X_MIN: f32 = -256.0;
    const X_MAX: f32 = 480.0;
    const CEILING: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -CEILING),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -X_MAX),
        CollisionBrush::half_space([1.0, 0.0, 0.0], X_MIN),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -96.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -96.0),
    ]);
    let door_heads =
        b.push_collision_hulls(&[CollisionBrush::box_brush(REACH_DOOR_MINS, REACH_DOOR_MAXS)]);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(
        [X_MIN, -96.0, 0.0],
        [X_MAX, 96.0, CEILING],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    b.push_model(
        REACH_DOOR_MINS,
        REACH_DOOR_MAXS,
        [0.0; 3],
        door_heads,
        2,
        0,
        0,
    );
    b.push_model(TRIGGER_MINS, TRIGGER_MAXS, [0.0; 3], trigger_heads, 2, 0, 0);

    b.build()
}

/// A `worldspawn` plus an `info_player_start` short of the door (`x =
/// 150`, facing `+x`, matching [`rotating_door_entities`]'s own
/// placement), and a `func_door` (submodel `*1`, targetname
/// [`REACH_DOOR_NAME`]) that slides straight up (`angle -1`, the
/// documented "up" sentinel; see `ohl_game::registry::movedir_from_angles`)
/// clear of the corridor's own unbounded ceiling when opened, with `wait =
/// -1` so it never auto-closes. No bytes here come from any game
/// installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_door_only_entities() -> String {
    reachability_door_only_entities_at_height(REACH_SPAWN_Z)
}

/// The `z` [`reachability_door_only_entities`] stands its
/// `info_player_start` at: just clear of the fixture's own floor, the way
/// an ordinary map places one.
pub const REACH_SPAWN_Z: f32 = 40.0;

/// As [`reachability_door_only_entities`], with the `info_player_start`
/// placed at an arbitrary height instead of [`REACH_SPAWN_Z`] — for the
/// walk's own "settle the spawn onto the floor first" regression, which
/// needs a start hanging further above the floor than
/// [`crate::reachability::DROP`]. No bytes here come from any game
/// installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_door_only_entities_at_height(spawn_z: f32) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"150 0 {spawn_z}\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{REACH_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"-1\"\n\"angle\" \"-1\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    )
}

/// As [`reachability_door_only_entities`], plus a `trigger_changelevel`
/// volume (submodel `*2`) beyond the door, naming `next_map`/[`LANDMARK`]
/// — the shape [`crate::reachability`]'s own regression test walks: a
/// closed door hides the level-change trigger until it opens. No bytes
/// here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_changelevel_entities(next_map: &str) -> String {
    reachability_changelevel_entities_at_height(next_map, REACH_SPAWN_Z)
}

/// As [`reachability_changelevel_entities`], with the `info_player_start`
/// at an arbitrary height; see
/// [`reachability_door_only_entities_at_height`].
#[must_use]
pub fn reachability_changelevel_entities_at_height(next_map: &str, spawn_z: f32) -> String {
    format!(
        "{}{{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n",
        reachability_door_only_entities_at_height(spawn_z),
    )
}

// ---------------------------------------------------------------------
// A corridor blocked by a `func_breakable`, and one blocked by a
// `func_pushable` (M9.10, `docs/FORMAT_SOURCES.md` item 32)
// ---------------------------------------------------------------------

/// The map name the `func_breakable` corridor fixture is published under.
pub const BREAKABLE_MAP: &str = "ohlbreakablesynth";

/// The map name the `func_pushable` corridor fixture is published under.
pub const PUSHABLE_MAP: &str = "ohlpushablesynth";

/// The `targetname` of the corridor fixtures' obstacle (the
/// `func_breakable`, or the `func_pushable`).
pub const OBSTACLE_NAME: &str = "ohl_obstacle";

/// The `targetname` of the `func_door` the breakable fixture's obstacle
/// targets, so "breaking fires `target`" is observable end to end.
pub const BREAKABLE_DOOR_NAME: &str = "ohl_breakable_door";

/// The near face of the corridor obstacle, on `+X`: a player walking
/// forward from the spawn point stops here while it is still in the way.
pub const OBSTACLE_NEAR_X: f32 = 64.0;

/// The far face of the `func_breakable` corridor obstacle, on `+X`.
pub const OBSTACLE_FAR_X: f32 = 96.0;

/// Half the width of the walled corridor [`obstacle_corridor_bsp`] can
/// build: wide enough that the 64-unit crate below leaves no gap a
/// 32-unit-wide player could squeeze through, and wide enough that the
/// crate's own 64-unit collision hull is not flush against either wall.
pub const CORRIDOR_HALF_Y: f32 = 48.0;

/// The upper corner of the `func_breakable` fixture's obstacle: a slab
/// spanning the full corridor, tall enough to cover a standing player.
pub const BREAKABLE_OBSTACLE_MAXS: [f32; 3] = [OBSTACLE_FAR_X, 64.0, 128.0];

/// The upper corner of the `func_pushable` fixture's crate: 64 units on
/// every axis, so `ohl_physics::Hull::for_size` picks the documented
/// 64x64x64 large hull for it and the traced push is the crate's own size
/// rather than a rough stand-in.
pub const PUSHABLE_CRATE_MAXS: [f32; 3] = [OBSTACLE_NEAR_X + 64.0, 32.0, 64.0];

/// The `func_breakable`'s `health` keyvalue: below the published 40-damage
/// `.357 Magnum` single shot (`ohl_combat::spec`'s own citation for
/// `WeaponId::Python`), so one landed shot breaks it outright.
pub const BREAKABLE_HEALTH: f32 = 30.0;

/// The `X` coordinate of the near face of the optional back wall the
/// pushable fixture can place behind the crate, to prove a pushed brush
/// stops on contact with world geometry instead of sliding through it.
pub const PUSHABLE_BACK_WALL_X: f32 = 192.0;

/// A flat floor (worldspawn) plus one obstacle submodel spanning the
/// straight walk from the spawn point, with real collision hulls of its own
/// so it genuinely blocks a walking player — unlike the bounding-box-only
/// submodels [`rot_button_bsp`] uses, which are only ever needed for a
/// proximity search.
///
/// Submodel 1 is the obstacle, a box from `x = ` [`OBSTACLE_NEAR_X`] to
/// [`OBSTACLE_FAR_X`], `y` in `-half_y..half_y` and `z` in `0..top_z`, so a
/// player walking `+X` from `(0, 0, 40)` runs into it. `corridor` walls the
/// straight walk in on both sides at `y = ±`[`CORRIDOR_HALF_Y`], so a
/// narrower obstacle still cannot be walked around; `back_wall` emits
/// submodel 2, a static slab at [`PUSHABLE_BACK_WALL_X`] a fixture can
/// declare as a `func_wall` to stand in the way of a pushed crate.
///
/// No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn obstacle_corridor_bsp(
    entities: &str,
    obstacle_maxs: [f32; 3],
    corridor: bool,
    back_wall: bool,
) -> Vec<u8> {
    const HALF: f32 = 512.0;
    const HEIGHT: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);
    let mut world = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -HEIGHT),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -HALF),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -HALF),
    ];
    if corridor {
        world.push(CollisionBrush::box_brush(
            [-HALF, CORRIDOR_HALF_Y, 0.0],
            [HALF, HALF, HEIGHT],
        ));
        world.push(CollisionBrush::box_brush(
            [-HALF, -HALF, 0.0],
            [HALF, -CORRIDOR_HALF_Y, HEIGHT],
        ));
    }
    let world_heads = b.push_collision_hulls(&world);
    let obstacle_mins = [OBSTACLE_NEAR_X, -obstacle_maxs[1], 0.0];
    let obstacle_heads =
        b.push_collision_hulls(&[CollisionBrush::box_brush(obstacle_mins, obstacle_maxs)]);
    let wall_mins = [PUSHABLE_BACK_WALL_X, -HALF, 0.0];
    let wall_maxs = [PUSHABLE_BACK_WALL_X + 16.0, HALF, HEIGHT];
    let wall_heads = back_wall
        .then(|| b.push_collision_hulls(&[CollisionBrush::box_brush(wall_mins, wall_maxs)]));
    b.push_model(
        [-HALF, -HALF, 0.0],
        [HALF, HALF, HEIGHT],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    b.push_model(
        obstacle_mins,
        obstacle_maxs,
        [0.0; 3],
        obstacle_heads,
        2,
        0,
        0,
    );
    if let Some(wall_heads) = wall_heads {
        b.push_model(wall_mins, wall_maxs, [0.0; 3], wall_heads, 2, 0, 0);
    }
    b.build()
}

/// A `worldspawn`, an `info_player_start` at `(0, 0, 40)` facing `+X`
/// (`angle 0`, the "counter-clockwise around +X" convention
/// `ohl_world::spawn::PlayerSpawn` documents) with a `weapon_357` sitting on
/// it so the very first pickup touch arms the player, a `func_breakable`
/// (submodel `*1`, [`BREAKABLE_HEALTH`] hit points, `targetname`
/// [`OBSTACLE_NAME`]) squarely in the way, and a `func_door`
/// ([`BREAKABLE_DOOR_NAME`], `wait -1`) as its "Target on Break" so the
/// documented `target` fire is observable. `spawnflags` is caller-chosen so
/// the same fixture can exercise the documented "Only Trigger (1)" and
/// "Touch (2)" flags. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn breakable_corridor_entities(health: f32, spawnflags: u32) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"weapon_357\"\n\"origin\" \"0 0 40\"\n}}\n\
         {{\n\"classname\" \"func_breakable\"\n\"targetname\" \"{OBSTACLE_NAME}\"\n\
         \"target\" \"{BREAKABLE_DOOR_NAME}\"\n\"model\" \"*1\"\n\
         \"health\" \"{health}\"\n\"spawnflags\" \"{spawnflags}\"\n\"material\" \"1\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{BREAKABLE_DOOR_NAME}\"\n\
         \"speed\" \"200\"\n\"wait\" \"-1\"\n}}\n"
    )
}

/// The same corridor, blocked by a `func_pushable` crate (submodel `*1`,
/// `targetname` [`OBSTACLE_NAME`]) instead: a player walking into it must
/// shove it out of the way to get past. `friction` is caller-chosen (the
/// documented `0..400` resistance range), and `back_wall` declares the
/// fixture's optional static `func_wall` (submodel `*2`) so a test can prove
/// a pushed crate stops against world geometry. No bytes here come from any
/// game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn pushable_corridor_entities(friction: f32, back_wall: bool) -> String {
    let wall = if back_wall {
        "{\n\"classname\" \"func_wall\"\n\"model\" \"*2\"\n}\n"
    } else {
        ""
    };
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_pushable\"\n\"targetname\" \"{OBSTACLE_NAME}\"\n\
         \"model\" \"*1\"\n\"friction\" \"{friction}\"\n\"material\" \"1\"\n}}\n\
         {wall}"
    )
}

// ---------------------------------------------------------------------
// A one-way ledge: the only route to the `trigger_changelevel` is a
// straight fall taller than the reachability walk's old, conservative
// 72-unit drop bound.
// ---------------------------------------------------------------------

/// The map name the reachability-report ledge/drop fixture is published
/// under.
pub const REACH_LEDGE_MAP: &str = "ohlreachledgesynth";

/// Where the high floor ends and the low floor begins, along `x`.
pub const REACH_LEDGE_EDGE_X: f32 = 64.0;

/// How far below the high floor's top the low floor's top sits — chosen
/// well past [`crate::reachability::DROP`] (72 units) so only a one-way
/// fall, not a stepped descent, reaches it.
pub const REACH_LEDGE_DROP_HEIGHT: f32 = 150.0;

/// A `worldspawn`-only corridor (`y` in `-96..96`, floor at `z = 0` for
/// `x <= `[`REACH_LEDGE_EDGE_X`], no ceiling below it) whose floor drops
/// [`REACH_LEDGE_DROP_HEIGHT`] units at that edge and continues at the
/// lower height out to `x = 400`, plus a *non-solid* submodel 1 for a
/// `trigger_changelevel` volume sitting on the low floor. Both floors are
/// real solid brushes (not one continuous half-space), so a walk stepping
/// past the edge finds open air, not a lower step within stepping
/// distance. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_ledge_bsp(entities: &str) -> Vec<u8> {
    const X_MIN: f32 = -256.0;
    const X_MAX: f32 = 400.0;
    const CEILING: f32 = 300.0;
    const TRIGGER_MINS: [f32; 3] = [150.0, -96.0, -REACH_LEDGE_DROP_HEIGHT];
    const TRIGGER_MAXS: [f32; 3] = [220.0, 96.0, -REACH_LEDGE_DROP_HEIGHT + 96.0];

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, -1.0], -CEILING),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -X_MAX),
        CollisionBrush::half_space([1.0, 0.0, 0.0], X_MIN),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -96.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -96.0),
        // The high floor, `x <= REACH_LEDGE_EDGE_X`, top at `z = 0`.
        CollisionBrush::box_brush([X_MIN, -96.0, -32.0], [REACH_LEDGE_EDGE_X, 96.0, 0.0]),
        // The low floor, `x > REACH_LEDGE_EDGE_X`, top at
        // `z = -REACH_LEDGE_DROP_HEIGHT`: a real cliff, not a stepped ramp.
        CollisionBrush::box_brush(
            [REACH_LEDGE_EDGE_X, -96.0, -32.0 - REACH_LEDGE_DROP_HEIGHT],
            [X_MAX, 96.0, -REACH_LEDGE_DROP_HEIGHT],
        ),
    ]);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(
        [X_MIN, -96.0, -32.0 - REACH_LEDGE_DROP_HEIGHT],
        [X_MAX, 96.0, CEILING],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    b.push_model(TRIGGER_MINS, TRIGGER_MAXS, [0.0; 3], trigger_heads, 2, 0, 0);

    b.build()
}

/// A `worldspawn` plus an `info_player_start` on the high floor (`x = 0`,
/// facing `+x`) and a `trigger_changelevel` (submodel `*1`, naming
/// `next_map`/[`LANDMARK`]) on the low floor beyond the ledge — the shape
/// [`crate::reachability`]'s own long-drop regression test walks. No bytes
/// here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_ledge_entities(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*1\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A one-way gap: the only route to the `trigger_changelevel` is a
// horizontal jump, bounded by the walking player's own run speed and
// jump airtime (`ohl_physics::MoveConfig` defaults).
// ---------------------------------------------------------------------

/// The map name the reachability-report gap/jump fixture is published
/// under.
pub const REACH_GAP_MAP: &str = "ohlreachgapsynth";

/// Where the near floor ends and the open gap begins, along `x`.
pub const REACH_GAP_EDGE_X: f32 = 64.0;

/// How long the far floor runs past the gap, along `x`.
pub const REACH_GAP_FAR_FLOOR_LENGTH: f32 = 150.0;

/// A `worldspawn`-only corridor (`y` in `-96..96`, floor at `z = 0` on
/// both sides) with a real, floorless gap `gap_width` units wide starting
/// at [`REACH_GAP_EDGE_X`] — no brush at all covers that span, so a walk's
/// down-trace there finds no floor within any bound, not just a deep one
/// — plus a *non-solid* submodel 1 for a `trigger_changelevel` volume on
/// the far floor. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_gap_bsp(gap_width: f32, entities: &str) -> Vec<u8> {
    const X_MIN: f32 = -256.0;
    const CEILING: f32 = 300.0;
    let far_floor_start = REACH_GAP_EDGE_X + gap_width;
    let x_max = far_floor_start + REACH_GAP_FAR_FLOOR_LENGTH;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, -1.0], -CEILING),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -x_max),
        CollisionBrush::half_space([1.0, 0.0, 0.0], X_MIN),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -96.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -96.0),
        // The near floor, `x <= REACH_GAP_EDGE_X`, top at `z = 0`.
        CollisionBrush::box_brush([X_MIN, -96.0, -32.0], [REACH_GAP_EDGE_X, 96.0, 0.0]),
        // The far floor, `x >= far_floor_start`, top at `z = 0` — the same
        // height as the near floor, so only the horizontal distance (not
        // any ascent) is under test.
        CollisionBrush::box_brush([far_floor_start, -96.0, -32.0], [x_max, 96.0, 0.0]),
    ]);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(
        [X_MIN, -96.0, -32.0],
        [x_max, 96.0, CEILING],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    let trigger_mins = [far_floor_start + 10.0, -96.0, 0.0];
    let trigger_maxs = [x_max - 10.0, 96.0, 96.0];
    b.push_model(trigger_mins, trigger_maxs, [0.0; 3], trigger_heads, 2, 0, 0);

    b.build()
}

/// A `worldspawn` plus an `info_player_start` on the near floor (`x = 0`,
/// facing `+x`) and a `trigger_changelevel` (submodel `*1`, naming
/// `next_map`/[`LANDMARK`]) on the far floor beyond the gap — the shape
/// [`crate::reachability`]'s own jump regression tests walk, both the
/// narrow (reachable) and wide (unreachable) gap. No bytes here come from
/// any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_gap_entities(next_map: &str) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*1\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    )
}

// ---------------------------------------------------------------------
// A corridor blocked by a `func_breakable`, or a `func_pushable`, hiding a
// `trigger_changelevel` beyond it (M9.11, `crate::reachability`'s own
// breakable/pushable extension)
// ---------------------------------------------------------------------

/// The map name the reachability-report `func_breakable` fixture is
/// published under.
pub const REACH_BREAKABLE_MAP: &str = "ohlreachbreakablesynth";

/// The map name the reachability-report `func_pushable` fixture is
/// published under.
pub const REACH_PUSHABLE_MAP: &str = "ohlreachpushablesynth";

/// The map name the reachability-report `func_pendulum` fixture is
/// published under.
pub const REACH_PENDULUM_MAP: &str = "ohlreachpendulumsynth";

/// The obstacle's box, spanning the whole corridor width — reuses
/// [`OBSTACLE_NAME`]'s own near face ([`OBSTACLE_NEAR_X`]) so the shape
/// matches [`obstacle_corridor_bsp`]'s fixture family.
const REACH_OBSTACLE_MAXS: [f32; 3] = [OBSTACLE_FAR_X, 64.0, 128.0];

/// The `trigger_changelevel` volume beyond the obstacle: comfortably past
/// [`REACH_OBSTACLE_MAXS`]'s far `X` face, well short of the fixture's own
/// outer walls.
const REACH_OBSTACLE_TRIGGER_MINS: [f32; 3] = [200.0, -96.0, 0.0];
/// See [`REACH_OBSTACLE_TRIGGER_MINS`].
const REACH_OBSTACLE_TRIGGER_MAXS: [f32; 3] = [260.0, 96.0, 96.0];

/// A walled corridor (matching [`obstacle_corridor_bsp`]'s own shape) with
/// a real solid submodel 1 obstacle ([`REACH_OBSTACLE_MAXS`]) and a
/// non-solid submodel 2 beyond it for a `trigger_changelevel` volume
/// ([`REACH_OBSTACLE_TRIGGER_MINS`]/[`REACH_OBSTACLE_TRIGGER_MAXS`]). No
/// bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
fn reachability_obstacle_bsp(entities: &str) -> Vec<u8> {
    const X_MIN: f32 = -256.0;
    const X_MAX: f32 = 320.0;
    const CEILING: f32 = 256.0;

    let mut b = Bsp30Builder::new();
    b.set_entities_text(entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -CEILING),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -X_MAX),
        CollisionBrush::half_space([1.0, 0.0, 0.0], X_MIN),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -CORRIDOR_HALF_Y),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -CORRIDOR_HALF_Y),
    ]);
    let obstacle_mins = [OBSTACLE_NEAR_X, -REACH_OBSTACLE_MAXS[1], 0.0];
    let obstacle_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        obstacle_mins,
        REACH_OBSTACLE_MAXS,
    )]);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(
        [X_MIN, -CORRIDOR_HALF_Y, 0.0],
        [X_MAX, CORRIDOR_HALF_Y, CEILING],
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    b.push_model(
        obstacle_mins,
        REACH_OBSTACLE_MAXS,
        [0.0; 3],
        obstacle_heads,
        2,
        0,
        0,
    );
    b.push_model(
        REACH_OBSTACLE_TRIGGER_MINS,
        REACH_OBSTACLE_TRIGGER_MAXS,
        [0.0; 3],
        trigger_heads,
        2,
        0,
        0,
    );

    b.build()
}

/// [`reachability_obstacle_bsp`], with a `func_breakable` (submodel `*1`,
/// [`OBSTACLE_NAME`], `health` [`BREAKABLE_HEALTH`]) as the obstacle and a
/// `trigger_changelevel` (submodel `*2`) beyond it. No bytes here come
/// from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_breakable_bsp(next_map: &str) -> Vec<u8> {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_breakable\"\n\"targetname\" \"{OBSTACLE_NAME}\"\n\
         \"model\" \"*1\"\n\"health\" \"{BREAKABLE_HEALTH}\"\n\"material\" \"1\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );
    reachability_obstacle_bsp(&entities)
}

/// [`reachability_obstacle_bsp`], with a `func_pushable` (submodel `*1`,
/// [`OBSTACLE_NAME`], `friction 0`) as the obstacle and a
/// `trigger_changelevel` (submodel `*2`) beyond it. No bytes here come
/// from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_pushable_bsp(next_map: &str) -> Vec<u8> {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_pushable\"\n\"targetname\" \"{OBSTACLE_NAME}\"\n\
         \"model\" \"*1\"\n\"friction\" \"0\"\n\"material\" \"1\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );
    reachability_obstacle_bsp(&entities)
}

/// [`reachability_obstacle_bsp`], with a `func_pendulum` (submodel `*1`,
/// [`OBSTACLE_NAME`]) as the obstacle and a `trigger_changelevel`
/// (submodel `*2`) beyond it — the fixture
/// [`crate::reachability`]'s own `assume_pendulum_wait` regression test
/// walks. No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn reachability_pendulum_bsp(next_map: &str) -> Vec<u8> {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_pendulum\"\n\"targetname\" \"{OBSTACLE_NAME}\"\n\
         \"model\" \"*1\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );
    reachability_obstacle_bsp(&entities)
}

// ---------------------------------------------------------------------

/// The map name the track-change fixture is published under.
pub const TRACK_CHANGE_MAP: &str = "ohltrackchangesynth";

/// The `targetname` of the fixture's `func_tracktrain`.
pub const TRACK_CHANGE_TRAIN_NAME: &str = "ohl_tc_train";

/// The fixture's top chain: the car spawns on the first node and dead-ends
/// on the second, which is the one carrying the documented `netname`
/// ("fire on dead end") that names the platform.
pub const TRACK_CHANGE_TOP_START: [f32; 3] = [0.0, 0.0, 0.0];
/// See [`TRACK_CHANGE_TOP_START`].
pub const TRACK_CHANGE_TOP_END: [f32; 3] = [200.0, 0.0, 0.0];

/// The fixture's bottom chain, [`TRACK_CHANGE_HEIGHT`] units below the top
/// one and running along `+Y` rather than `+X`, so a car that arrives on
/// it has both descended and turned.
pub const TRACK_CHANGE_BOTTOM_START: [f32; 3] = [200.0, 0.0, -300.0];
/// See [`TRACK_CHANGE_BOTTOM_START`].
pub const TRACK_CHANGE_BOTTOM_END: [f32; 3] = [200.0, 400.0, -300.0];

/// The platform's documented `height` ("travel distance, from top to
/// bottom"), matching the drop between the fixture's two chains.
pub const TRACK_CHANGE_HEIGHT: f32 = 300.0;
/// The platform's documented `rotation` ("the spin done by this platform
/// on entire way up/down"), a quarter turn onto the `+Y` bottom chain.
pub const TRACK_CHANGE_ROTATION: f32 = 90.0;
/// The platform's documented `speed`, units per second over the whole
/// trip, so the trip takes [`TRACK_CHANGE_HEIGHT`] / this seconds.
pub const TRACK_CHANGE_SPEED: f32 = 100.0;
/// The train's own `speed`/`startspeed`.
pub const TRACK_CHANGE_TRAIN_SPEED: f32 = 100.0;

/// The car's compiled half-extents about its origin brush, and the seat
/// offset the fixture's `info_player_start` sits at along the car.
pub const TRACK_CHANGE_CAR_HALF_LENGTH: f32 = 96.0;
/// See [`TRACK_CHANGE_CAR_HALF_LENGTH`].
pub const TRACK_CHANGE_CAR_HALF_WIDTH: f32 = 48.0;
/// See [`TRACK_CHANGE_CAR_HALF_LENGTH`].
pub const TRACK_CHANGE_CAR_TOP_Z: f32 = 8.0;
/// See [`TRACK_CHANGE_CAR_HALF_LENGTH`].
pub const TRACK_CHANGE_CAR_BOTTOM_Z: f32 = -8.0;

/// A void world (submodel `*0`) carrying a `func_tracktrain` (submodel
/// `*1`) that rides a two-node top chain into a dead end, a
/// `func_trackautochange` platform (submodel `*2`) named by that dead
/// end's documented `netname`, and a two-node bottom chain the platform is
/// documented to assign the train to when it arrives.
///
/// This is the shape the campaign blocker was: a ride whose track runs out
/// with a moving piece of track waiting to carry it onward. Without the
/// `netname` fire-on-dead-end the platform never activates; without the
/// platform the train never reaches the second chain, and every node along
/// that chain — including the ones whose `message` opens the doors ahead
/// of the ride — is never passed.
///
/// The world is void so the car is the only thing holding the passenger
/// up: a passenger the platform fails to carry falls out of the map rather
/// than quietly standing on a floor.
///
/// Every keyvalue and coordinate here is authored for this project;
/// nothing is derived from any payload (`docs/CLEAN_ROOM.md`).
#[must_use]
pub fn track_change_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    let [tsx, tsy, tsz] = TRACK_CHANGE_TOP_START;
    let [tex, tey, tez] = TRACK_CHANGE_TOP_END;
    let [bsx, bsy, bsz] = TRACK_CHANGE_BOTTOM_START;
    let [bex, bey, bez] = TRACK_CHANGE_BOTTOM_END;
    let seat_z = tsz + TRACK_CHANGE_CAR_TOP_Z + 36.0;
    let speed = TRACK_CHANGE_TRAIN_SPEED;
    let height = TRACK_CHANGE_HEIGHT;
    let rotation = TRACK_CHANGE_ROTATION;
    let platform_speed = TRACK_CHANGE_SPEED;
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{tsx} {tsy} {seat_z}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"targetname\" \"{TRACK_CHANGE_TRAIN_NAME}\"\n\
         \"target\" \"ohl_tc_top1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"{tsx} {tsy} {tsz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_tc_top1\"\n\
         \"target\" \"ohl_tc_top2\"\n\"origin\" \"{tsx} {tsy} {tsz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_tc_top2\"\n\
         \"netname\" \"ohl_tc_lift\"\n\"origin\" \"{tex} {tey} {tez}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_tc_bottom1\"\n\
         \"target\" \"ohl_tc_bottom2\"\n\"origin\" \"{bsx} {bsy} {bsz}\"\n}}\n\
         {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_tc_bottom2\"\n\
         \"origin\" \"{bex} {bey} {bez}\"\n}}\n\
         {{\n\"classname\" \"func_trackautochange\"\n\"model\" \"*2\"\n\
         \"targetname\" \"ohl_tc_lift\"\n\
         \"train\" \"{TRACK_CHANGE_TRAIN_NAME}\"\n\
         \"toptrack\" \"ohl_tc_top2\"\n\"bottomtrack\" \"ohl_tc_bottom1\"\n\
         \"height\" \"{height}\"\n\"rotation\" \"{rotation}\"\n\
         \"speed\" \"{platform_speed}\"\n\
         \"origin\" \"{tex} {tey} {tez}\"\n}}\n"
    ));

    // Submodel 0: a void world.
    let world_heads = b.push_collision_hulls(&[]);
    b.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the car, compiled relative to its origin brush.
    let car_mins = [
        -TRACK_CHANGE_CAR_HALF_LENGTH,
        -TRACK_CHANGE_CAR_HALF_WIDTH,
        TRACK_CHANGE_CAR_BOTTOM_Z,
    ];
    let car_maxs = [
        TRACK_CHANGE_CAR_HALF_LENGTH,
        TRACK_CHANGE_CAR_HALF_WIDTH,
        TRACK_CHANGE_CAR_TOP_Z,
    ];
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(car_mins, car_maxs)]);
    b.push_model(car_mins, car_maxs, [0.0, 0.0, 0.0], car_heads, 2, 0, 0);
    // Submodel 2: the platform itself, a slab under the car, compiled
    // relative to its own origin brush so it turns about it.
    let lift_mins = [
        -TRACK_CHANGE_CAR_HALF_LENGTH,
        -TRACK_CHANGE_CAR_HALF_WIDTH,
        -32.0,
    ];
    let lift_maxs = [
        TRACK_CHANGE_CAR_HALF_LENGTH,
        TRACK_CHANGE_CAR_HALF_WIDTH,
        -16.0,
    ];
    let lift_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(lift_mins, lift_maxs)]);
    b.push_model(lift_mins, lift_maxs, [0.0, 0.0, 0.0], lift_heads, 2, 0, 0);
    b.build()
}

// ---------------------------------------------------------------------
// A level boundary the player crosses *aboard* a `func_tracktrain`
// ---------------------------------------------------------------------

/// The map name the rider-boundary fixture's *source* map is published
/// under.
pub const RIDER_SOURCE_MAP: &str = "ohlridersynth";
/// The map name the rider-boundary fixture's *destination* map is
/// published under.
pub const RIDER_DESTINATION_MAP: &str = "ohlridersynth2";
/// A destination map whose arrival point is inside a parked brush entity's
/// own solid, for the embedded-arrival settle.
pub const RIDER_EMBEDDED_MAP: &str = "ohlridersynth3";
/// A destination map that *ends* the ride: its copy of the car sits on a
/// one-node chain, so the chain defines no heading anywhere.
pub const RIDER_TERMINUS_MAP: &str = "ohlridersynth4";
/// A destination map declaring the same named `func_wall` as
/// [`RiderMap::SourceOnBlock`], somewhere else entirely.
pub const RIDER_BLOCK_MAP: &str = "ohlridersynth5";

/// The `globalname` every copy of the fixture's car shares — the documented
/// cross-level correlation key.
pub const RIDER_CAR_GLOBAL: &str = "ohl_global_rider_car";

/// The car's compiled half-extents about its own origin brush.
pub const RIDER_CAR_HALF_LENGTH: f32 = 96.0;
/// See [`RIDER_CAR_HALF_LENGTH`].
pub const RIDER_CAR_HALF_WIDTH: f32 = 40.0;
/// The car floor's top, in the compiled frame.
pub const RIDER_CAR_TOP_Z: f32 = 8.0;
/// The car floor's bottom, in the compiled frame.
pub const RIDER_CAR_BOTTOM_Z: f32 = -8.0;

/// The car's `speed`/`startspeed`, units per second.
pub const RIDER_CAR_SPEED: f32 = 100.0;

/// Where the source map's chain starts, and where its car's origin brush
/// is. The world floor's top is `0`, so the car rides clear above it.
pub const RIDER_SOURCE_CHAIN_HEAD: [f32; 3] = [0.0, 0.0, 64.0];
/// Where the source map's chain ends: a straight run along `+X`, so the
/// source car's heading is zero degrees.
pub const RIDER_SOURCE_CHAIN_TAIL: [f32; 3] = [1600.0, 0.0, 64.0];

/// Where the *destination* map's copy of the same chain starts. Deliberately
/// nowhere near the source's, because that is the whole point: two maps
/// that share a ride place their own copy of it by their own `path_track`
/// chain, not by the landmark.
pub const RIDER_DESTINATION_CHAIN_HEAD: [f32; 3] = [200.0, 300.0, 64.0];
/// Where the destination map's chain ends: a straight run along `+Y`, so
/// the destination car's heading is ninety degrees from the source's.
pub const RIDER_DESTINATION_CHAIN_TAIL: [f32; 3] = [200.0, 1900.0, 64.0];

/// How far along the car, from its origin brush, the source map's
/// `info_player_start` sits: well off centre, so a seat that is *not*
/// carried relative to the car is visibly in the wrong place.
pub const RIDER_SEAT_OFFSET_X: f32 = 64.0;

/// Where the fixture's `info_landmark` sits — identical in every map, so a
/// landmark-relative arrival is the *identity* placement and any difference
/// in the player's arrival origin is the rider rule and nothing else.
pub const RIDER_LANDMARK_ORIGIN: [f32; 3] = [0.0, 0.0, 0.0];

/// Where a chain running along `+Y` from [`RIDER_SOURCE_CHAIN_HEAD`] ends,
/// so the car it carries is posed ninety degrees from the `+X` one.
pub const RIDER_TURNED_CHAIN_TAIL: [f32; 3] = [0.0, 1600.0, 64.0];

/// Where a *non-riding* player stands in the source map: on the world
/// floor, clear of the car, so their arrival is placed by the landmark
/// offset rather than by any seat.
pub const RIDER_FOOT_SPAWN: [f32; 3] = [0.0, -400.0, 36.0];

/// How far along `+X` [`RiderMap::BlockElsewhere`] moves its copy of the
/// named block: far enough that a placement measured against the block
/// instead of the landmark could not be mistaken for rounding.
pub const RIDER_BLOCK_DISPLACEMENT_X: f32 = 600.0;

/// The parked block the embedded-arrival map puts at [`RIDER_FOOT_SPAWN`]:
/// its top is high enough that the arriving standing hull overlaps it, and
/// low enough that the bounded nudge can free it.
pub const RIDER_BLOCK_TOP_Z: f32 = 24.0;

/// Which of the fixture's maps to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiderMap {
    /// The source map: a world floor, a car on a chain running along `+X`,
    /// and an `info_player_start` standing on the car's floor.
    Source,
    /// The source map with the player start on the *world floor* instead,
    /// clear of the car.
    SourceOnFoot,
    /// The destination map: the same car, on the same-named chain, placed
    /// somewhere else and pointing somewhere else.
    Destination,
    /// A destination whose arrival point is inside a parked brush entity.
    Embedded,
    /// The source map with its chain running along `+Y` instead, so the
    /// car it carries a passenger out of is posed at ninety degrees.
    SourceTurned,
    /// A destination that *ends* the ride: its copy of the car sits on a
    /// chain of exactly **one** `path_track`, so the chain defines no
    /// heading anywhere and the car has none of its own.
    Terminus,
    /// The source map with the player standing on top of a named, parked
    /// `func_wall` — a brush entity that is *not* a ride.
    SourceOnBlock,
    /// A destination that declares the same named `func_wall` somewhere
    /// else entirely, so a placement measured against it would be visibly
    /// different from the landmark offset.
    BlockElsewhere,
}

/// Two (or three) maps of one synthetic campaign, sharing a
/// `func_tracktrain` by `globalname` and a `path_track` chain by node name,
/// but placing that chain differently — the shape a real shared ride takes,
/// where each map's copy of the track is authored in its own coordinates.
///
/// Nothing here comes from any game installation; every keyvalue and
/// coordinate is authored for this project (`docs/CLEAN_ROOM.md`).
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture builder for a five-map synthetic campaign; splitting \
              it would scatter coordinates that only make sense together"
)]
pub fn rider_boundary_bsp(map: RiderMap) -> Vec<u8> {
    let (head, tail) = match map {
        RiderMap::Destination => (RIDER_DESTINATION_CHAIN_HEAD, RIDER_DESTINATION_CHAIN_TAIL),
        RiderMap::SourceTurned => (RIDER_SOURCE_CHAIN_HEAD, RIDER_TURNED_CHAIN_TAIL),
        _ => (RIDER_SOURCE_CHAIN_HEAD, RIDER_SOURCE_CHAIN_TAIL),
    };
    let [hx, hy, hz] = head;
    let [tx, ty, tz] = tail;
    let [lx, ly, lz] = RIDER_LANDMARK_ORIGIN;
    let [fx, fy, fz] = RIDER_FOOT_SPAWN;
    let seat = match map {
        // The car is posed along `+Y` here, so the seat offset is too:
        // this is the same place *in the car*, not in the world.
        RiderMap::SourceTurned => [
            hx,
            hy + RIDER_SEAT_OFFSET_X,
            hz + RIDER_CAR_TOP_Z + PLAYER_STANDING_HALF_HEIGHT,
        ],
        // Standing on the block's top rather than on the world floor.
        RiderMap::SourceOnBlock => [fx, fy, RIDER_BLOCK_TOP_Z + PLAYER_STANDING_HALF_HEIGHT],
        RiderMap::Source => [
            hx + RIDER_SEAT_OFFSET_X,
            hy,
            hz + RIDER_CAR_TOP_Z + PLAYER_STANDING_HALF_HEIGHT,
        ],
        _ => RIDER_FOOT_SPAWN,
    };
    let speed = RIDER_CAR_SPEED;
    // The embedded map's parked block: the same submodel the car uses,
    // placed so its top is `RIDER_BLOCK_TOP_Z` and the arriving standing
    // hull overlaps it from below.
    let block = match map {
        // The same block in three places: under the arrival point (so the
        // arriving hull is embedded in it), under the source map's player
        // start (so they are standing on a named brush that is not a ride),
        // and moved far along `+X` in the destination (so a placement
        // measured against it would be nowhere near the landmark offset).
        RiderMap::Embedded | RiderMap::SourceOnBlock | RiderMap::BlockElsewhere => {
            let bz = RIDER_BLOCK_TOP_Z - RIDER_BLOCK_HALF_HEIGHT;
            let bx = if map == RiderMap::BlockElsewhere {
                fx + RIDER_BLOCK_DISPLACEMENT_X
            } else {
                fx
            };
            format!(
                "{{\n\"classname\" \"func_wall\"\n\"model\" \"*2\"\n\
                 \"targetname\" \"ohl_rider_block\"\n\"origin\" \"{bx} {fy} {bz}\"\n}}\n"
            )
        }
        _ => String::new(),
    };
    // A terminus map parks its copy of the car on a chain of exactly one
    // node: the ride ends there, so there is no segment anywhere in the
    // chain and the car has no heading of its own.
    let chain = if map == RiderMap::Terminus {
        format!(
            "{{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_rider1\"\n\
             \"origin\" \"{hx} {hy} {hz}\"\n}}\n"
        )
    } else {
        format!(
            "{{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_rider1\"\n\
             \"target\" \"ohl_rider2\"\n\"origin\" \"{hx} {hy} {hz}\"\n}}\n\
             {{\n\"classname\" \"path_track\"\n\"targetname\" \"ohl_rider2\"\n\
             \"origin\" \"{tx} {ty} {tz}\"\n}}\n"
        )
    };
    let mut b = Bsp30Builder::new();
    b.set_entities_text(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\
         \"origin\" \"{} {} {}\"\n\"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"{lx} {ly} {lz}\"\n}}\n\
         {{\n\"classname\" \"func_tracktrain\"\n\"model\" \"*1\"\n\
         \"targetname\" \"ohl_rider_car\"\n\"globalname\" \"{RIDER_CAR_GLOBAL}\"\n\
         \"target\" \"ohl_rider1\"\n\"speed\" \"{speed}\"\n\
         \"startspeed\" \"{speed}\"\n\"height\" \"0\"\n\
         \"origin\" \"{hx} {hy} {hz}\"\n}}\n\
         {chain}{block}",
        seat[0], seat[1], seat[2],
    ));
    let _ = fz;

    // Submodel 0: a floor slab the whole fixture stands on, so a player
    // who is *not* riding has ordinary world geometry under them.
    let floor_mins = [-2048.0, -2048.0, -64.0];
    let floor_maxs = [2048.0, 2048.0, 0.0];
    let world_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(floor_mins, floor_maxs)]);
    b.push_model(
        floor_mins,
        floor_maxs,
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    // Submodel 1: the car, compiled relative to its origin brush.
    let car_mins = [
        -RIDER_CAR_HALF_LENGTH,
        -RIDER_CAR_HALF_WIDTH,
        RIDER_CAR_BOTTOM_Z,
    ];
    let car_maxs = [RIDER_CAR_HALF_LENGTH, RIDER_CAR_HALF_WIDTH, RIDER_CAR_TOP_Z];
    let car_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(car_mins, car_maxs)]);
    b.push_model(car_mins, car_maxs, [0.0, 0.0, 0.0], car_heads, 2, 0, 0);
    // Submodel 2: the parked block, compiled about its own origin brush.
    let block_mins = [-64.0, -64.0, -RIDER_BLOCK_HALF_HEIGHT];
    let block_maxs = [64.0, 64.0, RIDER_BLOCK_HALF_HEIGHT];
    let block_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(block_mins, block_maxs)]);
    b.push_model(
        block_mins,
        block_maxs,
        [0.0, 0.0, 0.0],
        block_heads,
        2,
        0,
        0,
    );
    b.build()
}

/// Half the standing hull's height, i.e. how far a standing player's origin
/// sits above the surface they are resting on.
const PLAYER_STANDING_HALF_HEIGHT: f32 = 36.0;

/// Half the parked block's compiled height.
const RIDER_BLOCK_HALF_HEIGHT: f32 = 62.0;

// ---------------------------------------------------------------------
// An L-shaped corridor with a turn and a closed door in it: the route
// planner's own end-to-end fixture (`crate::route_plan`)
// ---------------------------------------------------------------------

/// The map name the route planner's corridor fixture is published under.
pub const PLAN_TURN_MAP: &str = "ohlplanturnsynth";

/// The `targetname` of the fixture's blocking `func_door`.
pub const PLAN_TURN_DOOR_NAME: &str = "ohl_plan_door";

/// The fixture's bounding box: an L carved out of it by one solid filler
/// block, so a walk from the player start has to run along the first leg,
/// turn ninety degrees into the second, and carry on.
const PLAN_TURN_MIN: [f32; 3] = [-64.0, -64.0, 0.0];
/// See [`PLAN_TURN_MIN`].
const PLAN_TURN_MAX: [f32; 3] = [256.0, 448.0, 448.0];

/// The solid block that turns the fixture's bounding box into an L: it
/// fills everything but the first leg (`x` up to the corner) and the
/// second (`y` past it).
const PLAN_TURN_FILLER_MIN: [f32; 3] = [-64.0, 64.0, 0.0];
/// See [`PLAN_TURN_FILLER_MIN`].
const PLAN_TURN_FILLER_MAX: [f32; 3] = [128.0, 448.0, 448.0];

/// The closed door leaf, across the second leg with an eight-unit margin
/// to each wall (the same margin [`REACH_DOOR_MINS`] leaves), full
/// corridor height so nothing steps or jumps over it.
const PLAN_TURN_DOOR_MIN: [f32; 3] = [136.0, 240.0, 0.0];
/// See [`PLAN_TURN_DOOR_MIN`].
const PLAN_TURN_DOOR_MAX: [f32; 3] = [248.0, 256.0, 192.0];

/// The `trigger_changelevel` volume at the far end of the second leg.
const PLAN_TURN_TRIGGER_MIN: [f32; 3] = [128.0, 384.0, 0.0];
/// See [`PLAN_TURN_TRIGGER_MIN`].
const PLAN_TURN_TRIGGER_MAX: [f32; 3] = [256.0, 448.0, 192.0];

/// An L-shaped corridor (bounding box [`PLAN_TURN_MIN`]/[`PLAN_TURN_MAX`]
/// with the solid filler block [`PLAN_TURN_FILLER_MIN`]/
/// [`PLAN_TURN_FILLER_MAX`] carving the L), a real solid submodel 1 for a
/// plain translating door leaf across the second leg, and a non-solid
/// submodel 2 beyond the door for a `trigger_changelevel` volume — the
/// shape `crate::route_plan`'s own regression tests plan a route through:
/// a straight run, a ninety-degree turn, a door press, and a final run
/// into the trigger.
///
/// The door slides straight up (`angle -1`, the documented "up" sentinel)
/// clear of the corridor's own ceiling and never auto-closes (`wait -1`).
/// No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn plan_turn_bsp(next_map: &str) -> Vec<u8> {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"func_door\"\n\"targetname\" \"{PLAN_TURN_DOOR_NAME}\"\n\
         \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"-1\"\n\"angle\" \"-1\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*2\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );

    let mut b = Bsp30Builder::new();
    b.set_entities_text(&entities);

    let world_heads = b.push_collision_hulls(&[
        CollisionBrush::half_space([0.0, 0.0, 1.0], PLAN_TURN_MIN[2]),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -PLAN_TURN_MAX[2]),
        CollisionBrush::half_space([1.0, 0.0, 0.0], PLAN_TURN_MIN[0]),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -PLAN_TURN_MAX[0]),
        CollisionBrush::half_space([0.0, 1.0, 0.0], PLAN_TURN_MIN[1]),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -PLAN_TURN_MAX[1]),
        CollisionBrush::box_brush(PLAN_TURN_FILLER_MIN, PLAN_TURN_FILLER_MAX),
    ]);
    let door_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        PLAN_TURN_DOOR_MIN,
        PLAN_TURN_DOOR_MAX,
    )]);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(PLAN_TURN_MIN, PLAN_TURN_MAX, [0.0; 3], world_heads, 2, 0, 0);
    b.push_model(
        PLAN_TURN_DOOR_MIN,
        PLAN_TURN_DOOR_MAX,
        [0.0; 3],
        door_heads,
        2,
        0,
        0,
    );
    b.push_model(
        PLAN_TURN_TRIGGER_MIN,
        PLAN_TURN_TRIGGER_MAX,
        [0.0; 3],
        trigger_heads,
        2,
        0,
        0,
    );

    b.build()
}

/// The map name [`plan_cost_bsp`] is registered under.
pub const PLAN_COST_MAP: &str = "ohlplancostsynth";

/// The two-level fixture's bounding box.
const PLAN_COST_MIN: [f32; 3] = [-64.0, -64.0, 0.0];
/// See [`PLAN_COST_MIN`].
const PLAN_COST_MAX: [f32; 3] = [448.0, 256.0, 384.0];

/// The upper level: a slab filling the west end of the box, whose top is
/// [`PLAN_COST_LEDGE_Z`] above the lower floor.
const PLAN_COST_SHELF_MIN: [f32; 3] = [-64.0, -64.0, 0.0];
/// See [`PLAN_COST_SHELF_MIN`].
const PLAN_COST_SHELF_MAX: [f32; 3] = [192.0, 256.0, 128.0];

/// How far the upper level stands above the lower one, in world units:
/// further than [`crate::reachability::DROP`], so stepping off it is a
/// one-way fall rather than a step down, and well inside the height a
/// player lands from unhurt, so the walk is free to plan it.
pub const PLAN_COST_LEDGE_Z: f32 = PLAN_COST_SHELF_MAX[2];

/// Where the upper level ends and the fall begins.
pub const PLAN_COST_LEDGE_X: f32 = PLAN_COST_SHELF_MAX[0];

/// The strip of the map the staircase occupies, when there is one.
const PLAN_COST_STAIR_Y_MIN: f32 = 192.0;
/// How many steps the staircase has, and how tall/deep each one is.
const PLAN_COST_STAIR_COUNT: i32 = 7;
/// See [`PLAN_COST_STAIR_COUNT`].
const PLAN_COST_STAIR_SIZE: f32 = 16.0;

/// The `trigger_changelevel` volume at the far end of the lower floor.
const PLAN_COST_TRIGGER_MIN: [f32; 3] = [384.0, -64.0, 0.0];
/// See [`PLAN_COST_TRIGGER_MIN`].
const PLAN_COST_TRIGGER_MAX: [f32; 3] = [448.0, 256.0, 128.0];

/// Two levels joined two ways: a [`PLAN_COST_LEDGE_Z`]-unit ledge the
/// player may simply step off, and — when `with_stairs` — a staircase down
/// one side, taking many more steps to walk but costing no fall at all.
/// The goal sits on the lower floor, reachable either way.
///
/// This is the shape `crate::route_plan`'s cost-ordered walk is measured
/// against: counted in grid steps the fall is much the shorter route, so a
/// breadth-first walk takes it every time; counted in what it costs a body,
/// the staircase wins, and the fall is what is left when the staircase is
/// not there.
///
/// No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn plan_cost_bsp(next_map: &str, with_stairs: bool) -> Vec<u8> {
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 170\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*1\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );

    let mut b = Bsp30Builder::new();
    b.set_entities_text(&entities);

    let mut solid = vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], PLAN_COST_MIN[2]),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -PLAN_COST_MAX[2]),
        CollisionBrush::half_space([1.0, 0.0, 0.0], PLAN_COST_MIN[0]),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -PLAN_COST_MAX[0]),
        CollisionBrush::half_space([0.0, 1.0, 0.0], PLAN_COST_MIN[1]),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -PLAN_COST_MAX[1]),
        CollisionBrush::box_brush(PLAN_COST_SHELF_MIN, PLAN_COST_SHELF_MAX),
    ];
    if with_stairs {
        for step in 0..PLAN_COST_STAIR_COUNT {
            #[allow(clippy::cast_precision_loss, reason = "a small step index")]
            let index = step as f32;
            let x = PLAN_COST_LEDGE_X + index * PLAN_COST_STAIR_SIZE;
            let top = PLAN_COST_LEDGE_Z - (index + 1.0) * PLAN_COST_STAIR_SIZE;
            solid.push(CollisionBrush::box_brush(
                [x, PLAN_COST_STAIR_Y_MIN, PLAN_COST_MIN[2]],
                [x + PLAN_COST_STAIR_SIZE, PLAN_COST_MAX[1], top],
            ));
        }
    }
    let world_heads = b.push_collision_hulls(&solid);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(PLAN_COST_MIN, PLAN_COST_MAX, [0.0; 3], world_heads, 2, 0, 0);
    b.push_model(
        PLAN_COST_TRIGGER_MIN,
        PLAN_COST_TRIGGER_MAX,
        [0.0; 3],
        trigger_heads,
        2,
        0,
        0,
    );

    b.build()
}

/// The map name [`plan_ladder_bsp`] is registered under.
pub const PLAN_LADDER_MAP: &str = "ohlplanladdersynth";

/// The shaft fixture's bounding box.
const PLAN_LADDER_MIN: [f32; 3] = [-64.0, -64.0, 0.0];
/// See [`PLAN_LADDER_MIN`].
const PLAN_LADDER_MAX: [f32; 3] = [640.0, 192.0, 640.0];

/// The shelf the player starts on: a solid block filling the west half of
/// the box up to its top surface, so everything east of it is a shaft
/// down to the floor.
const PLAN_LADDER_SHELF_MIN: [f32; 3] = [-64.0, -64.0, 0.0];
/// See [`PLAN_LADDER_SHELF_MIN`].
const PLAN_LADDER_SHELF_MAX: [f32; 3] = [64.0, 192.0, 448.0];

/// The climbable volume down the shelf's east face, from the shaft floor
/// to the shelf's own top.
const PLAN_LADDER_VOLUME_MIN: [f32; 3] = [64.0, -64.0, 0.0];
/// See [`PLAN_LADDER_VOLUME_MIN`].
const PLAN_LADDER_VOLUME_MAX: [f32; 3] = [96.0, 192.0, 448.0];

/// The `trigger_changelevel` volume at the far end of the shaft floor.
const PLAN_LADDER_TRIGGER_MIN: [f32; 3] = [512.0, -64.0, 0.0];
/// See [`PLAN_LADDER_TRIGGER_MIN`].
const PLAN_LADDER_TRIGGER_MAX: [f32; 3] = [640.0, 192.0, 128.0];

/// The drop from the shelf to the shaft floor, in world units — taller
/// than the height a player lands from unhurt, and taller than the one
/// `crate::route_plan` plans a fall from at full health, so the only
/// route down this fixture that costs no health is the ladder.
pub const PLAN_LADDER_DROP: f32 = PLAN_LADDER_SHELF_MAX[2];

/// A shelf over a shaft with a `trigger_changelevel` on the floor below
/// it, and — when `with_ladder` — a climbable (`CONTENTS_LADDER`) volume
/// down the shelf's east face: the shape `crate::route_plan`'s own ladder
/// regression tests plan a route through.
///
/// Without the ladder the only way down is a [`PLAN_LADDER_DROP`]-unit
/// fall, which is exactly what that module's drop bound refuses.
///
/// No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn plan_ladder_bsp(next_map: &str, with_ladder: bool) -> Vec<u8> {
    plan_shaft_bsp(next_map, with_ladder, false)
}

/// The same shelf and shaft as [`plan_ladder_bsp`], with no ladder and a
/// lethal `trigger_hurt` on the shaft floor: a pit that kills whoever
/// walks off the shelf into it.
///
/// What it is for: a route planner that walks the player somewhere fatal
/// has to *say* so rather than keep planning from a body that cannot
/// move, and the only way to test that is to have somewhere fatal to
/// walk to.
///
/// No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn plan_pit_bsp(next_map: &str) -> Vec<u8> {
    plan_shaft_bsp(next_map, false, true)
}

/// The builder behind [`plan_ladder_bsp`] and [`plan_pit_bsp`].
fn plan_shaft_bsp(next_map: &str, with_ladder: bool, lethal: bool) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut hurt = String::new();
    if lethal {
        // Three point volumes, a radius apart along the shaft floor, so
        // wherever a player who stepped off the shelf comes down they
        // land in one of them: the pit is meant to be fatal, not
        // fatal-if-aimed.
        for x in [160.0f32, 288.0, 416.0] {
            let _ = write!(
                hurt,
                "{{\n\"classname\" \"trigger_hurt\"\n\
                 \"origin\" \"{x} 64 36\"\n\"dmg\" \"500\"\n}}\n"
            );
        }
    }
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-16 64 490\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*1\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n\
         {hurt}"
    );

    let mut b = Bsp30Builder::new();
    b.set_entities_text(&entities);

    let solid = [
        CollisionBrush::half_space([0.0, 0.0, 1.0], PLAN_LADDER_MIN[2]),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -PLAN_LADDER_MAX[2]),
        CollisionBrush::half_space([1.0, 0.0, 0.0], PLAN_LADDER_MIN[0]),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -PLAN_LADDER_MAX[0]),
        CollisionBrush::half_space([0.0, 1.0, 0.0], PLAN_LADDER_MIN[1]),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -PLAN_LADDER_MAX[1]),
        CollisionBrush::box_brush(PLAN_LADDER_SHELF_MIN, PLAN_LADDER_SHELF_MAX),
    ];
    let ladder = [CollisionBrush::box_brush(
        PLAN_LADDER_VOLUME_MIN,
        PLAN_LADDER_VOLUME_MAX,
    )];
    let volumes: &[(i32, &[CollisionBrush])] = if with_ladder {
        &[(ohl_physics::contents::LADDER, &ladder)]
    } else {
        &[]
    };
    let world_heads = b.push_collision_hulls_with_contents(&solid, volumes);
    let trigger_heads = b.push_collision_hulls(&[]);

    b.push_model(
        PLAN_LADDER_MIN,
        PLAN_LADDER_MAX,
        [0.0; 3],
        world_heads,
        2,
        0,
        0,
    );
    b.push_model(
        PLAN_LADDER_TRIGGER_MIN,
        PLAN_LADDER_TRIGGER_MAX,
        [0.0; 3],
        trigger_heads,
        2,
        0,
        0,
    );

    b.build()
}

/// The map name [`plan_lift_bsp`] is registered under.
pub const PLAN_LIFT_MAP: &str = "ohlplanliftsynth";

/// The `targetname` of the fixture's lift, and of the trigger volume that
/// starts it. Project-authored, like every other literal in this module.
pub const PLAN_LIFT_NAME: &str = "ohl_plan_lift";

/// The lift fixture's bounding box: a shaft with a landing floor at one
/// end and a tall ledge at the other.
const PLAN_LIFT_MIN: [f32; 3] = [-192.0, -64.0, -256.0];
/// See [`PLAN_LIFT_MIN`].
const PLAN_LIFT_MAX: [f32; 3] = [512.0, 192.0, 512.0];

/// The floor the player starts on: a slab whose top is the walked level.
const PLAN_LIFT_FLOOR_MIN: [f32; 3] = [-192.0, -64.0, -256.0];
/// See [`PLAN_LIFT_FLOOR_MIN`].
const PLAN_LIFT_FLOOR_MAX: [f32; 3] = [0.0, 192.0, 0.0];

/// The ledge the lift serves, flush against the platform's far face:
/// its top is [`PLAN_LIFT_TRAVEL`] above the starting floor, too tall to
/// step, jump or climb to, with no way round.
const PLAN_LIFT_LEDGE_MIN: [f32; 3] = [128.0, -64.0, -256.0];
/// See [`PLAN_LIFT_LEDGE_MIN`].
const PLAN_LIFT_LEDGE_MAX: [f32; 3] = [512.0, 192.0, 256.0];

/// The lift itself at rest: a platform bridging the shaft whose top sits
/// one ordinary step above the starting floor, so the walk simply walks
/// onto it.
const PLAN_LIFT_PLATFORM_MIN: [f32; 3] = [0.0, -64.0, -240.0];
/// See [`PLAN_LIFT_PLATFORM_MIN`].
const PLAN_LIFT_PLATFORM_MAX: [f32; 3] = [128.0, 192.0, 16.0];

/// The `func_button` [`LiftFixture::ButtonPlat`] wires to its platform: a
/// small panel on the ledge's own face, at the height the eye of a player
/// standing on the platform is, and within a `use` press of the platform's
/// far end.
const PLAN_LIFT_BUTTON_MIN: [f32; 3] = [120.0, 48.0, 64.0];
/// See [`PLAN_LIFT_BUTTON_MIN`].
const PLAN_LIFT_BUTTON_MAX: [f32; 3] = [128.0, 80.0, 96.0];

/// How far the lift travels, straight up.
pub const PLAN_LIFT_TRAVEL: f32 = 256.0;

/// The touch volume that starts the lift, sitting on the lift's own top
/// surface: walking onto the platform is what fires it.
const PLAN_LIFT_TRIGGER_MIN: [f32; 3] = [0.0, -64.0, 16.0];
/// See [`PLAN_LIFT_TRIGGER_MIN`].
const PLAN_LIFT_TRIGGER_MAX: [f32; 3] = [128.0, 192.0, 80.0];

/// Where [`LiftFixture::OutOfReach`] puts that same volume instead: high
/// above the shaft, where no walked cell ever stands in it and no `use`
/// press reaches.
const PLAN_LIFT_FAR_TRIGGER_MIN: [f32; 3] = [0.0, -64.0, 400.0];
/// See [`PLAN_LIFT_FAR_TRIGGER_MIN`].
const PLAN_LIFT_FAR_TRIGGER_MAX: [f32; 3] = [128.0, 192.0, 464.0];

/// The `trigger_changelevel` volume at the far end of the ledge.
const PLAN_LIFT_GOAL_MIN: [f32; 3] = [384.0, -64.0, 256.0];
/// See [`PLAN_LIFT_GOAL_MIN`].
const PLAN_LIFT_GOAL_MAX: [f32; 3] = [512.0, 192.0, 384.0];

/// Which lift [`plan_lift_bsp`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftFixture {
    /// A `func_door` used as a lift (`wait -1`, so it stays where it
    /// goes), started by a `trigger_multiple` volume lying on its own top
    /// surface: walking onto the platform is what sets it going.
    TouchDoor,
    /// A `func_plat` with no touch volume at all, started by a `use`
    /// press on the `func_button` beside it, and returning on its own
    /// once its `wait` is up.
    ///
    /// A press on the platform *itself* is deliberately not what this
    /// tests: a standing player's eye is a whole [`crate::USE_RADIUS`]
    /// above their feet, so the brush centre of the thing they are
    /// standing on is never within a press of them. A lift the player
    /// rides is switched from beside it, and that is what this builds.
    ButtonPlat,
    /// The same lift as [`Self::TouchDoor`], with its trigger volume
    /// moved high above the shaft: nothing the player can stand on
    /// touches it, its own brush centre is far below any `use` press, and
    /// so there is no way to set it going at all.
    OutOfReach,
}

/// A shaft with a lift in it: a starting floor, a ledge
/// [`PLAN_LIFT_TRAVEL`] units above it with a `trigger_changelevel` on
/// top, and a platform bridging the two that travels straight up when it
/// is set going.
///
/// Nothing but the lift connects the two levels: the ledge is far taller
/// than a step, a jump or a survivable fall, and there is no ladder. So a
/// route to the goal exists exactly when the walk can *ride* — which is
/// what `crate::route_plan`'s ride edge is measured against, in all three
/// of the shapes [`LiftFixture`] describes.
///
/// No bytes here come from any game installation; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn plan_lift_bsp(next_map: &str, fixture: LiftFixture) -> Vec<u8> {
    let mover = match fixture {
        LiftFixture::ButtonPlat => format!(
            "{{\n\"classname\" \"func_plat\"\n\"targetname\" \"{PLAN_LIFT_NAME}\"\n\
             \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"20\"\n\"angle\" \"-1\"\n\
             \"height\" \"{PLAN_LIFT_TRAVEL}\"\n\"origin\" \"0 0 0\"\n}}\n\
             {{\n\"classname\" \"func_button\"\n\"model\" \"*4\"\n\
             \"target\" \"{PLAN_LIFT_NAME}\"\n\"speed\" \"100\"\n\"wait\" \"5\"\n\
             \"origin\" \"0 0 0\"\n}}\n"
        ),
        LiftFixture::TouchDoor | LiftFixture::OutOfReach => format!(
            "{{\n\"classname\" \"func_door\"\n\"targetname\" \"{PLAN_LIFT_NAME}\"\n\
             \"model\" \"*1\"\n\"speed\" \"100\"\n\"wait\" \"-1\"\n\"angle\" \"-1\"\n\
             \"lip\" \"0\"\n\"origin\" \"0 0 0\"\n}}\n"
        ),
    };
    // The `func_plat` is started by a press on itself, so it gets no
    // touch volume; the other two get one, in or out of reach.
    let trigger = if fixture == LiftFixture::ButtonPlat {
        String::new()
    } else {
        format!(
            "{{\n\"classname\" \"trigger_multiple\"\n\"model\" \"*2\"\n\
             \"target\" \"{PLAN_LIFT_NAME}\"\n\"wait\" \"4\"\n\"origin\" \"0 0 0\"\n}}\n"
        )
    };
    let entities = format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"-96 64 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {mover}{trigger}\
         {{\n\"classname\" \"trigger_changelevel\"\n\"model\" \"*3\"\n\
         \"map\" \"{next_map}\"\n\"landmark\" \"{LANDMARK}\"\n\"origin\" \"0 0 0\"\n}}\n"
    );

    let (trigger_min, trigger_max) = if fixture == LiftFixture::OutOfReach {
        (PLAN_LIFT_FAR_TRIGGER_MIN, PLAN_LIFT_FAR_TRIGGER_MAX)
    } else {
        (PLAN_LIFT_TRIGGER_MIN, PLAN_LIFT_TRIGGER_MAX)
    };

    let mut b = Bsp30Builder::new();
    b.set_entities_text(&entities);

    let solid = [
        CollisionBrush::half_space([0.0, 0.0, 1.0], PLAN_LIFT_MIN[2]),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -PLAN_LIFT_MAX[2]),
        CollisionBrush::half_space([1.0, 0.0, 0.0], PLAN_LIFT_MIN[0]),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -PLAN_LIFT_MAX[0]),
        CollisionBrush::half_space([0.0, 1.0, 0.0], PLAN_LIFT_MIN[1]),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -PLAN_LIFT_MAX[1]),
        CollisionBrush::box_brush(PLAN_LIFT_FLOOR_MIN, PLAN_LIFT_FLOOR_MAX),
        CollisionBrush::box_brush(PLAN_LIFT_LEDGE_MIN, PLAN_LIFT_LEDGE_MAX),
    ];
    let world_heads = b.push_collision_hulls(&solid);
    let lift_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
        PLAN_LIFT_PLATFORM_MIN,
        PLAN_LIFT_PLATFORM_MAX,
    )]);
    let trigger_heads = b.push_collision_hulls(&[]);
    let goal_heads = b.push_collision_hulls(&[]);

    b.push_model(PLAN_LIFT_MIN, PLAN_LIFT_MAX, [0.0; 3], world_heads, 2, 0, 0);
    b.push_model(
        PLAN_LIFT_PLATFORM_MIN,
        PLAN_LIFT_PLATFORM_MAX,
        [0.0; 3],
        lift_heads,
        2,
        0,
        0,
    );
    b.push_model(trigger_min, trigger_max, [0.0; 3], trigger_heads, 2, 0, 0);
    b.push_model(
        PLAN_LIFT_GOAL_MIN,
        PLAN_LIFT_GOAL_MAX,
        [0.0; 3],
        goal_heads,
        2,
        0,
        0,
    );
    if fixture == LiftFixture::ButtonPlat {
        let button_heads = b.push_collision_hulls(&[CollisionBrush::box_brush(
            PLAN_LIFT_BUTTON_MIN,
            PLAN_LIFT_BUTTON_MAX,
        )]);
        b.push_model(
            PLAN_LIFT_BUTTON_MIN,
            PLAN_LIFT_BUTTON_MAX,
            [0.0; 3],
            button_heads,
            2,
            0,
            0,
        );
    }

    b.build()
}
