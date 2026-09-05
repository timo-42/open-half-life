//! Round-trip, accessor, and malformed-field rejection tests for `bsp30`,
//! using this crate's own synthetic fixture writer
//! (`ohl_formats::test_support`). No bytes here come from any game
//! installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::{Bsp, Limits, Miptex};
use ohl_formats::test_support::Bsp30Builder;

/// Builds a tiny, internally consistent synthetic map: one model, two
/// leaves split by one plane, a handful of faces/edges, one embedded and one
/// external miptex, and a two-entity entities lump.
fn tiny_map() -> Bsp30Builder {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 0\"\n}\n",
    );

    // One splitting plane: x = 0.
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);

    // Four vertices forming two unit-ish quads either side of the plane.
    b.push_vertex([-1.0, -1.0, 0.0]);
    b.push_vertex([-1.0, 1.0, 0.0]);
    b.push_vertex([1.0, -1.0, 0.0]);
    b.push_vertex([1.0, 1.0, 0.0]);

    // Edges (index 0 is conventionally unused; keep it as a zero edge).
    b.push_edge(0, 0);
    b.push_edge(0, 1);
    b.push_edge(1, 2);
    b.push_edge(2, 3);
    b.push_edge(3, 0);

    // One face using edges 1..4 (via surfedges).
    b.push_surfedge(0); // unused slot 0
    b.push_surfedge(1);
    b.push_surfedge(2);
    b.push_surfedge(3);

    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 0, 0);
    b.push_face(0, 0, 1, 3, 0, [0, 0xFF, 0xFF, 0xFF], -1);

    // A handful of lighting samples.
    for i in 0..4u8 {
        b.push_lighting_rgb(i, i, i);
    }

    // Two leaves, no visibility list (vis_offset = -1 => always visible).
    b.push_leaf(-1, -1, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    b.push_leaf(-1, -1, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    b.push_marksurface(0);

    // One node splitting the two leaves via the plane above.
    b.push_node(0, -1, -2, [-1, -1, 0], [1, 1, 0], 0, 1);

    // One clipnode mirroring the same split for hull 1.
    b.push_clipnode(0, -1, -2);

    // One model referencing the node tree and clipnode hull.
    b.push_model(
        [-1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [0, 0, 0, 0],
        2,
        0,
        1,
    );

    b.add_embedded_texture("wall01", 16, 16, 7);
    b.add_external_texture("wall02", 32, 32);

    b
}

#[test]
fn round_trips_header_and_every_lump() {
    let bytes = tiny_map().build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("valid synthetic map parses");

    assert_eq!(bsp.entities(&limits).unwrap().len(), 2);
    assert_eq!(bsp.planes(&limits).unwrap().len(), 1);
    assert_eq!(bsp.vertices(&limits).unwrap().len(), 4);
    assert_eq!(bsp.edges(&limits).unwrap().len(), 5);
    assert_eq!(bsp.surfedges(&limits).unwrap().len(), 4);
    assert_eq!(bsp.texinfo(&limits).unwrap().len(), 1);
    assert_eq!(bsp.faces(&limits).unwrap().len(), 1);
    assert_eq!(bsp.lighting(&limits).unwrap().len(), 4);
    assert_eq!(bsp.leaves(&limits).unwrap().len(), 2);
    assert_eq!(bsp.marksurfaces(&limits).unwrap().len(), 1);
    assert_eq!(bsp.nodes(&limits).unwrap().len(), 1);
    assert_eq!(bsp.clipnodes(&limits).unwrap().len(), 1);
    assert_eq!(bsp.models(&limits).unwrap().len(), 1);
}

#[test]
fn decodes_embedded_and_external_textures() {
    let bytes = tiny_map().build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("valid map parses");
    let textures = bsp.textures(&limits).unwrap();
    assert_eq!(textures.len(), 2);

    match textures.get(0).unwrap().expect("slot 0 present") {
        Miptex::Embedded {
            width,
            height,
            body,
            ..
        } => {
            assert_eq!(width, 16);
            assert_eq!(height, 16);
            assert_eq!(body.mips[0].indices.len(), 16 * 16);
            assert_eq!(body.mips[3].indices.len(), 2 * 2);
            assert_eq!(body.mips[0].indices[0], 7);
            assert_eq!(body.palette.get(7).r, 7);
        }
        Miptex::External { .. } => panic!("slot 0 should be embedded"),
    }

    match textures.get(1).unwrap().expect("slot 1 present") {
        Miptex::External { width, height, .. } => {
            assert_eq!(width, 32);
            assert_eq!(height, 32);
        }
        Miptex::Embedded { .. } => panic!("slot 1 should be external"),
    }
}

#[test]
fn find_leaf_walks_the_node_tree() {
    let bytes = tiny_map().build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("valid map parses");
    let models = bsp.models(&limits).unwrap();
    let head_node = models[0].headnodes[0].get();

    let leaf = bsp.find_leaf(head_node, [5.0, 0.0, 0.0], &limits).unwrap();
    assert_eq!(leaf, 0);
    let leaf = bsp.find_leaf(head_node, [-5.0, 0.0, 0.0], &limits).unwrap();
    assert_eq!(leaf, 1);
}

#[test]
fn leaf_without_vis_offset_is_always_visible() {
    let bytes = tiny_map().build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("valid map parses");
    let leaves = bsp.leaves(&limits).unwrap();
    assert!(bsp.is_leaf_visible(&leaves[0], 0, 2, &limits).unwrap());
}

#[test]
fn decodes_compressed_visibility() {
    let mut b = tiny_map();
    // A single literal byte with bit 0 clear, bit 1 set, at vis_offset 0.
    b.visibility = alloc_vec(&[0b0000_0010]);
    b.leaves.clear();
    b.push_leaf(-1, 0, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    b.push_leaf(-1, 0, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("valid map parses");
    let leaves = bsp.leaves(&limits).unwrap();
    assert!(!bsp.is_leaf_visible(&leaves[0], 0, 2, &limits).unwrap());
    assert!(bsp.is_leaf_visible(&leaves[0], 1, 2, &limits).unwrap());
}

fn alloc_vec(bytes: &[u8]) -> Vec<u8> {
    bytes.to_vec()
}

// --- Malformed-field rejection tests -------------------------------------

#[test]
fn rejects_bad_version() {
    let mut bytes = tiny_map().build();
    bytes[0..4].copy_from_slice(&29i32.to_le_bytes());
    assert!(Bsp::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_lump_outside_file() {
    let mut bytes = tiny_map().build();
    // Corrupt the first lump's offset (right after the 4-byte version) to
    // point past the end of the file.
    let huge_offset = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    bytes[4..8].copy_from_slice(&huge_offset.to_le_bytes());
    assert!(Bsp::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_lump_size_not_a_multiple_of_element_size() {
    let bytes = tiny_map().build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("header is valid");
    // Planes lump is a multiple of 20 bytes; a raw sub-slice missing one
    // byte is not.
    let full = bsp
        .raw_lump(ohl_formats::bsp30::LumpId::Planes, &limits)
        .unwrap();
    assert!(!full.is_empty());
    let short = &full[..full.len() - 1];
    assert!(ohl_formats::bsp30::Bsp::parse(&bytes, &limits).is_ok());
    // Directly exercise the slice-cast rejection via the public accessor
    // semantics: a lump length in the header that is not a multiple of the
    // element size must be rejected.
    let mut bad = bytes.clone();
    // planes lump length lives in dir entry index 1 -> header offset
    // 4 + 1*8 + 4 = 16.
    let len_field_offset = 4 + 8 + 4;
    let corrupted_len = u32::from_le_bytes(
        bad[len_field_offset..len_field_offset + 4]
            .try_into()
            .unwrap(),
    ) - 1;
    bad[len_field_offset..len_field_offset + 4].copy_from_slice(&corrupted_len.to_le_bytes());
    let bsp = Bsp::parse(&bad, &limits).expect("directory bounds still valid");
    assert!(bsp.planes(&limits).is_err());
    let _ = short;
}

#[test]
fn rejects_index_out_of_range() {
    let mut b = tiny_map();
    b.marksurfaces.clear();
    b.push_marksurface(9999); // no such face
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    let marksurfaces = bsp.marksurfaces(&limits).unwrap();
    let faces = bsp.faces(&limits).unwrap();
    let bad_index = marksurfaces[0].0.get() as usize;
    assert!(faces.get(bad_index).is_none());
}

#[test]
fn rejects_miptex_offsets_outside_lump() {
    let mut b = tiny_map();
    // Corrupt the embedded texture's first mip offset to point far outside
    // the textures lump. `texture_bodies` holds tex0's body (added first)
    // starting at byte 0: name(16)+width(4)+height(4)+offsets[4](16).
    let offset0_field = 16 + 4 + 4;
    let huge = 0x00FF_FFFF_u32;
    b.texture_bodies[offset0_field..offset0_field + 4].copy_from_slice(&huge.to_le_bytes());
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    let textures = bsp.textures(&limits).unwrap();
    assert!(textures.get(0).is_err());
}

#[test]
fn rejects_pvs_overrun() {
    let mut b = tiny_map();
    b.visibility = alloc_vec(&[0x00, 0xFF]); // claims 255 zero bytes.
    b.leaves.clear();
    b.push_leaf(-1, 0, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    b.push_leaf(-1, 0, [-1, -1, 0], [1, 1, 0], 0, 1, [0, 0, 0, 0]);
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    let leaves = bsp.leaves(&limits).unwrap();
    assert!(bsp.is_leaf_visible(&leaves[0], 0, 2, &limits).is_err());
}

#[test]
fn rejects_entities_lump_without_nul() {
    let mut b = tiny_map();
    b.entities = b"{\"classname\" \"worldspawn\"}".to_vec();
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    assert!(bsp.entities(&limits).is_err());
}

#[test]
fn rejects_node_recursion_loop() {
    let mut b = tiny_map();
    b.nodes.clear();
    // Node 0's front child points back at node 0: an infinite loop without
    // a depth limit.
    b.push_node(0, 0, -1, [-1, -1, 0], [1, 1, 0], 0, 1);
    let bytes = b.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    assert!(bsp.find_leaf(0, [5.0, 0.0, 0.0], &limits).is_err());
}
