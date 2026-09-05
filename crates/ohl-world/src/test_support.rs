//! A synthetic, project-authored BSP v30 "room" used by this crate's tests,
//! by `ohl-render`'s headless render test, and by the development-only
//! `--dev-bsp` flag's manual check.
//!
//! Every byte is generated here from first principles on top of
//! `ohl_formats::test_support`; nothing is derived from any game
//! installation (see `docs/CLEAN_ROOM.md`). Exposed behind the
//! `test-support` feature so other crates in the workspace can share one
//! fixture instead of each inventing their own.

use ohl_formats::test_support::{Bsp30Builder, Wad3Builder};

use crate::lightmap::lightmap_extents;

/// The room's half-extent on X and Y, in GoldSrc units.
pub const ROOM_HALF_WIDTH: f32 = 128.0;

/// The room's height, in GoldSrc units.
pub const ROOM_HEIGHT: f32 = 128.0;

/// The number of faces the synthetic room has (a closed box).
pub const ROOM_FACE_COUNT: usize = 6;

/// The name of the room's externally stored (WAD3) texture.
pub const EXTERNAL_TEXTURE_NAME: &str = "ohlwall";

struct Quad {
    corners: [[f32; 3]; 4],
    s_vector: [f32; 3],
    t_vector: [f32; 3],
    plane: u16,
    plane_side: u16,
    texture: u32,
}

fn room_quads() -> [Quad; ROOM_FACE_COUNT] {
    let h = ROOM_HALF_WIDTH;
    let z = ROOM_HEIGHT;
    [
        // Floor (z = 0) and ceiling (z = height), textured with the
        // embedded miptex.
        Quad {
            corners: [[-h, -h, 0.0], [h, -h, 0.0], [h, h, 0.0], [-h, h, 0.0]],
            s_vector: [1.0, 0.0, 0.0],
            t_vector: [0.0, 1.0, 0.0],
            plane: 0,
            plane_side: 0,
            texture: 0,
        },
        Quad {
            corners: [[-h, -h, z], [-h, h, z], [h, h, z], [h, -h, z]],
            s_vector: [1.0, 0.0, 0.0],
            t_vector: [0.0, 1.0, 0.0],
            plane: 1,
            plane_side: 1,
            texture: 0,
        },
        // Four walls, textured with the external (WAD3) miptex.
        Quad {
            corners: [[-h, -h, 0.0], [-h, h, 0.0], [-h, h, z], [-h, -h, z]],
            s_vector: [0.0, 1.0, 0.0],
            t_vector: [0.0, 0.0, 1.0],
            plane: 2,
            plane_side: 1,
            texture: 1,
        },
        Quad {
            corners: [[h, -h, 0.0], [h, -h, z], [h, h, z], [h, h, 0.0]],
            s_vector: [0.0, 1.0, 0.0],
            t_vector: [0.0, 0.0, 1.0],
            plane: 2,
            plane_side: 0,
            texture: 1,
        },
        Quad {
            corners: [[-h, -h, 0.0], [-h, -h, z], [h, -h, z], [h, -h, 0.0]],
            s_vector: [1.0, 0.0, 0.0],
            t_vector: [0.0, 0.0, 1.0],
            plane: 3,
            plane_side: 1,
            texture: 1,
        },
        Quad {
            corners: [[-h, h, 0.0], [h, h, 0.0], [h, h, z], [-h, h, z]],
            s_vector: [1.0, 0.0, 0.0],
            t_vector: [0.0, 0.0, 1.0],
            plane: 3,
            plane_side: 0,
            texture: 1,
        },
    ]
}

/// Builds the synthetic room's BSP v30 bytes.
///
/// The map has one submodel, six lit faces, one embedded and one external
/// texture, two visible leaves behind an `x = 0` split with differing
/// mark-surface lists and a real compressed-visibility lump, and an
/// `info_player_start`.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
pub fn synthetic_room_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n\
         {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n\"angle\" \"90\"\n}\n",
    );

    // Planes 0..3 bound the room; plane 4 is the node split at x = 0.
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2); // floor
    b.push_plane([0.0, 0.0, 1.0], ROOM_HEIGHT, 2); // ceiling
    b.push_plane([1.0, 0.0, 0.0], ROOM_HALF_WIDTH, 0); // +/- X walls
    b.push_plane([0.0, 1.0, 0.0], ROOM_HALF_WIDTH, 1); // +/- Y walls
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0); // node split
    let split_plane = 4u32;

    // Edge 0 is conventionally unused.
    b.push_edge(0, 0);

    let quads = room_quads();
    for (index, quad) in quads.iter().enumerate() {
        let base = u16::try_from(index * 4).expect("six quads fit u16");
        for corner in quad.corners {
            b.push_vertex(corner);
        }
        for corner in 0..4u16 {
            let next = (corner + 1) % 4;
            b.push_edge(base + corner, base + next);
        }
        // Surfedge slot `index * 4` refers to edge `index * 4 + 1` (edge 0
        // is the unused conventional slot).
        let first_edge = i32::try_from(index * 4 + 1).expect("six quads fit i32");
        for step in 0..4 {
            b.push_surfedge(first_edge + step);
        }

        b.push_texinfo(quad.s_vector, 0.0, quad.t_vector, 0.0, quad.texture, 0);

        // Lay out this face's style-0 light samples contiguously.
        let (mut min_s, mut max_s) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut min_t, mut max_t) = (f32::INFINITY, f32::NEG_INFINITY);
        for corner in quad.corners {
            let s = corner[0] * quad.s_vector[0]
                + corner[1] * quad.s_vector[1]
                + corner[2] * quad.s_vector[2];
            let t = corner[0] * quad.t_vector[0]
                + corner[1] * quad.t_vector[1]
                + corner[2] * quad.t_vector[2];
            min_s = min_s.min(s);
            max_s = max_s.max(s);
            min_t = min_t.min(t);
            max_t = max_t.max(t);
        }
        let extents = lightmap_extents(min_s, max_s, min_t, max_t).expect("room faces are finite");
        let offset = i32::try_from(b.lighting.len()).expect("synthetic lighting fits i32");
        for sample in 0..extents.sample_count() {
            // A deterministic gradient so a rendered frame is obviously lit.
            let level = 64 + ((sample * 3) % 160) as u8;
            b.push_lighting_rgb(level, level, level);
        }

        b.push_face(
            quad.plane,
            quad.plane_side,
            u32::try_from(index * 4).expect("fits"),
            4,
            u16::try_from(index).expect("fits"),
            [0, 0xFF, 0xFF, 0xFF],
            offset,
        );
    }

    // Leaf 0 is the solid outside leaf. Leaf 1 (x >= 0) references the
    // floor, ceiling and -X/+X walls; leaf 2 references the two Y walls.
    for face in 0..4u16 {
        b.push_marksurface(face);
    }
    for face in 4..6u16 {
        b.push_marksurface(face);
    }

    // Visibility: leaf 1 sees only itself, leaf 2 sees both leaves.
    b.visibility.push(0b0000_0001);
    b.visibility.push(0b0000_0011);

    let extent = ROOM_HALF_WIDTH as i16;
    let height = ROOM_HEIGHT as i16;
    b.push_leaf(-2, -1, [0, 0, 0], [0, 0, 0], 0, 0, [0, 0, 0, 0]);
    b.push_leaf(
        -1,
        0,
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        4,
        [0, 0, 0, 0],
    );
    b.push_leaf(
        -1,
        1,
        [-extent, -extent, 0],
        [extent, extent, height],
        4,
        2,
        [0, 0, 0, 0],
    );

    b.push_node(
        split_plane,
        -2, // front child: leaf 1
        -3, // back child: leaf 2
        [-extent, -extent, 0],
        [extent, extent, height],
        0,
        u16::try_from(ROOM_FACE_COUNT).expect("fits"),
    );
    b.push_clipnode(split_plane.cast_signed(), -2, -3);

    b.push_model(
        [-ROOM_HALF_WIDTH, -ROOM_HALF_WIDTH, 0.0],
        [ROOM_HALF_WIDTH, ROOM_HALF_WIDTH, ROOM_HEIGHT],
        [0.0, 0.0, 0.0],
        [0, 0, 0, 0],
        2,
        0,
        i32::try_from(ROOM_FACE_COUNT).expect("fits"),
    );

    b.add_embedded_texture("ohlfloor", 64, 64, 200);
    b.add_external_texture(EXTERNAL_TEXTURE_NAME, 64, 64);

    b.build()
}

/// Builds a synthetic WAD3 package containing the room's external texture,
/// so the WAD lookup path can be exercised without any game media.
#[must_use]
pub fn synthetic_room_wad() -> Vec<u8> {
    let mut wad = Wad3Builder::new();
    wad.add_miptex(EXTERNAL_TEXTURE_NAME, 64, 64, 90);
    wad.build()
}
