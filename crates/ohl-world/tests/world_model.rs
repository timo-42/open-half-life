//! World-model construction tests over the project-authored synthetic room.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_world::test_support::{
    EXTERNAL_TEXTURE_NAME, ROOM_FACE_COUNT, ROOM_HALF_WIDTH, synthetic_room_bsp, synthetic_room_wad,
};
use ohl_world::{
    BspLimits, DrawList, Frustum, LightRamp, TextureImage, WorldBuildOptions, WorldModel,
    lightmap_extents,
};

use ohl_formats::bsp30::Bsp;

fn build(wads: &[&[u8]]) -> (Vec<u8>, WorldModel) {
    let bytes = synthetic_room_bsp();
    let model = {
        let limits = BspLimits::default();
        let bsp = Bsp::parse(&bytes, &limits).expect("synthetic room parses");
        WorldModel::build(
            &bsp,
            &WorldBuildOptions {
                wads,
                limits,
                ..WorldBuildOptions::default()
            },
        )
        .expect("synthetic room builds")
    };
    (bytes, model)
}

#[test]
fn triangulates_every_face_as_a_fan() {
    let (_bytes, model) = build(&[]);
    assert_eq!(model.faces.len(), ROOM_FACE_COUNT);
    // Each face is a quad: four vertices, two triangles, six indices.
    assert_eq!(model.vertices.len(), ROOM_FACE_COUNT * 4);
    assert_eq!(model.indices.len(), ROOM_FACE_COUNT * 6);
    for face in &model.faces {
        assert_eq!(face.index_count, 6);
        assert!(face.bounds.is_valid());
    }
    let total: u32 = model.batches.iter().map(|batch| batch.index_count).sum();
    assert_eq!(total as usize, model.indices.len());
}

#[test]
fn batches_are_contiguous_and_texture_major() {
    let (_bytes, model) = build(&[]);
    // Two textures: one embedded (floor/ceiling), one external (walls).
    assert_eq!(model.textures.len(), 2);
    assert_eq!(model.batches.len(), 2);
    let mut cursor = 0u32;
    let mut previous_texture = None;
    for batch in &model.batches {
        assert_eq!(batch.first_index, cursor);
        cursor += batch.index_count;
        assert_ne!(Some(batch.texture), previous_texture);
        previous_texture = Some(batch.texture);
    }
    assert_eq!(cursor as usize, model.indices.len());
}

#[test]
fn lightmap_atlas_bounds_contain_every_coordinate() {
    let (_bytes, model) = build(&[]);
    let atlas = &model.lightmap_atlas;
    assert_eq!(atlas.width(), ohl_world::LIGHTMAP_ATLAS_WIDTH);
    assert!(atlas.height() >= 17, "17-luxel faces must fit");
    assert!(atlas.height() <= ohl_world::LIGHTMAP_ATLAS_MAX_HEIGHT);
    assert_eq!(
        atlas.rgba().len(),
        (atlas.width() * atlas.height() * 4) as usize
    );
    for vertex in &model.vertices {
        for coordinate in vertex.lightmap_uv {
            assert!(
                (0.0..=1.0).contains(&coordinate),
                "lightmap coordinates stay inside the atlas"
            );
        }
    }
}

#[test]
fn floor_lightmap_extents_span_the_room() {
    // The floor spans -128..128 on both axes: sixteen 16-unit cells, and the
    // grid covers both endpoints, so seventeen luxels per axis.
    let extents = lightmap_extents(
        -ROOM_HALF_WIDTH,
        ROOM_HALF_WIDTH,
        -ROOM_HALF_WIDTH,
        ROOM_HALF_WIDTH,
    )
    .expect("finite");
    assert_eq!((extents.width, extents.height), (17, 17));
    assert_eq!(extents.sample_count(), 17 * 17);
    assert_eq!((extents.min_s, extents.min_t), (-128, -128));
}

#[test]
fn external_textures_fall_back_to_the_placeholder_without_a_wad() {
    let (_bytes, without) = build(&[]);
    let placeholder = TextureImage::placeholder();
    let external = &without.textures[1];
    assert_eq!(external.width(), placeholder.width());
    assert_eq!(external.rgba(), placeholder.rgba());

    let wad = synthetic_room_wad();
    let (_bytes, with) = build(&[&wad]);
    let resolved = &with.textures[1];
    assert_eq!(resolved.width(), 64);
    assert_ne!(resolved.rgba(), placeholder.rgba());
    assert!(
        !EXTERNAL_TEXTURE_NAME.is_empty(),
        "the fixture names its external texture"
    );
}

#[test]
fn player_start_is_parsed() {
    let (_bytes, model) = build(&[]);
    let spawn = model.spawn.expect("the room has an info_player_start");
    assert!((spawn.origin[2] - 32.0).abs() < 1e-3);
    assert!((spawn.yaw - 90.0).abs() < 1e-3);
    assert_eq!(
        model.player_start_count, 1,
        "the synthetic room places exactly one info_player_start"
    );
}

#[test]
fn pvs_limits_the_draw_list_to_visible_leaves() {
    let (_bytes, model) = build(&[]);

    // Leaf 1 (x >= 0) sees only itself, and references four of six faces.
    let leaf = model.leaf_at([64.0, 0.0, 64.0]).expect("inside the room");
    assert_eq!(leaf, 1);
    let mut list = DrawList::new();
    model.build_draw_list([64.0, 0.0, 64.0], None, &mut list);
    assert_eq!(list.triangle_count(), 4 * 2);

    // Leaf 2 (x < 0) sees both leaves, so it draws the whole room.
    let leaf = model.leaf_at([-64.0, 0.0, 64.0]).expect("inside the room");
    assert_eq!(leaf, 2);
    model.build_draw_list([-64.0, 0.0, 64.0], None, &mut list);
    assert_eq!(list.triangle_count(), ROOM_FACE_COUNT * 2);
    assert!(!list.batches.is_empty());
}

#[test]
fn a_frustum_facing_away_culls_everything() {
    let (_bytes, model) = build(&[]);
    // A degenerate view-projection whose planes all sit far behind the room.
    let mut matrix = [0.0f32; 16];
    matrix[0] = 1.0;
    matrix[5] = 1.0;
    matrix[10] = 1.0;
    matrix[12] = 100_000.0;
    matrix[15] = 1.0;
    let frustum = Frustum::from_view_projection(&matrix);
    let mut list = DrawList::new();
    model.build_draw_list([-64.0, 0.0, 64.0], Some(&frustum), &mut list);
    assert_eq!(list.triangle_count(), 0);
    assert!(list.batches.is_empty());
}

/// Writes the synthetic room to the system temporary directory so a
/// developer can point `ohl-app --features dev-tools -- --dev-bsp ...` at
/// it. Ignored by default because it is a manual-check aid, not an
/// assertion; the file it writes is generated here and contains no game
/// media.
#[test]
#[ignore = "manual-check aid: writes a synthetic map to the temp directory"]
fn writes_the_synthetic_room_for_manual_checks() {
    let directory = std::env::temp_dir();
    std::fs::write(
        directory.join("ohl-synthetic-room.bsp"),
        synthetic_room_bsp(),
    )
    .expect("temporary directory is writable");
    std::fs::write(
        directory.join("ohl-synthetic-room.wad"),
        synthetic_room_wad(),
    )
    .expect("temporary directory is writable");
}

fn build_with_ramp(ramp: LightRamp) -> WorldModel {
    let bytes = synthetic_room_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic room parses");
    WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ramp,
        },
    )
    .expect("synthetic room builds")
}

/// Every packed luxel must be the raw compiled sample mapped through the
/// documented ramp, and the composed product of a known texel and a known
/// luxel must land on a pinned code value.
///
/// The raw product a plain multiply would give is what made every capture
/// 2.5-3x too dark; this pins the tone curve so that regression cannot
/// return unnoticed.
#[test]
fn lightmap_samples_are_ramped_before_they_reach_the_atlas() {
    let raw = build_with_ramp(LightRamp::identity());
    let ramped = build_with_ramp(LightRamp::default());
    let table = LightRamp::default().table();

    let raw_pixels = raw.lightmap_atlas.rgba();
    let ramped_pixels = ramped.lightmap_atlas.rgba();
    assert_eq!(raw_pixels.len(), ramped_pixels.len());
    let (raw_chunks, _) = raw_pixels.as_chunks::<4>();
    let (ramped_chunks, _) = ramped_pixels.as_chunks::<4>();
    for (raw_byte, ramped_byte) in raw_chunks.iter().zip(ramped_chunks.iter()) {
        for channel in 0..3 {
            assert_eq!(table.apply(raw_byte[channel]), ramped_byte[channel]);
        }
        assert_eq!(
            raw_byte[3], ramped_byte[3],
            "the ramp touches colour only, never alpha"
        );
    }

    // The composed value a plain-multiply shader produces for a mid-grey
    // texel (0x80) over the fixture's darkest luxel (0x40).
    let texel = 0x80u32;
    let luxel = 0x40u8;
    let raw_product = texel * u32::from(luxel) / 255;
    let ramped_product = texel * u32::from(table.apply(luxel)) / 255;
    assert_eq!(
        raw_product, 0x20,
        "the un-ramped product is the old baseline"
    );
    assert_eq!(
        ramped_product, 68,
        "the ramped product is the pinned tone-curve constant"
    );
}

/// An unlit/fullbright face samples the reserved white tile, which the ramp
/// must leave at white (the ramp fixes both endpoints).
#[test]
fn the_fullbright_white_tile_survives_the_ramp() {
    let model = build_with_ramp(LightRamp::default());
    let atlas = model.lightmap_atlas.rgba();
    assert_eq!(
        &atlas[..4],
        &[255, 255, 255, 255],
        "the reserved 1x1 white tile stays fullbright"
    );
}

/// `build_submodels` reports a submodel that will not build instead of
/// dropping it, so an entity that should be visible cannot vanish silently.
#[test]
fn build_submodels_reports_failures_instead_of_dropping_them() {
    let bytes = synthetic_room_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic room parses");
    let options = WorldBuildOptions {
        wads: &[],
        limits,
        ..WorldBuildOptions::default()
    };
    let count = WorldModel::submodel_count(&bsp, &limits).expect("model lump decodes");
    assert!(count >= 1, "worldspawn is always model 0");
    let set = WorldModel::build_submodels(&bsp, &options, &[0, count + 7]);
    assert_eq!(set.models.len(), 1);
    assert_eq!(set.models[0].0, 0);
    assert_eq!(set.failure_count(), 1);
    assert_eq!(set.failures[0].0, count + 7);
    assert_eq!(set.failures[0].1, ohl_world::WorldError::SubmodelOutOfRange);
}

/// A synthetic map with four worldspawn faces: one ordinary occluder, one
/// `sky`-textured face, one face whose `texinfo` produces a non-finite
/// texture-space coordinate (a malformed `s_shift`, over otherwise
/// perfectly finite vertex positions), and one degenerate face (its edge
/// walk yields only 2 vertices, not the 3 a polygon needs).
///
/// This reproduces the fidelity finding behind
/// [`ohl_world::WorldModel::dropped_faces`] (see that field's doc comment):
/// earlier, a single face like the third one here took the *whole* model's
/// build down via `?`, which for a real map means one bad face in a large
/// brush entity (an indoor hall's wall, say) could silently remove all of
/// that entity's geometry — including whatever it was occluding a real sky
/// face behind. Building must now succeed, keep the good faces (including
/// the sky one), and count the two bad faces instead.
fn map_with_a_bad_face_alongside_a_sky_face() -> Vec<u8> {
    use ohl_formats::test_support::Bsp30Builder;

    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n\0");

    // One dummy leaf referencing the sky face's marksurface, so `has_sky`
    // (derived from each leaf's marksurface list, not the raw per-face
    // scan) picks it up; nothing else about this leaf is geometrically
    // meaningful, since this fixture does not exercise PVS or the BSP walk.
    b.push_marksurface(1);
    b.push_leaf(-2, -1, [0, 0, 0], [0, 0, 0], 0, 1, [0, 0, 0, 0]);

    // Edge 0 is conventionally unused (matches `synthetic_room_bsp`).
    b.push_edge(0, 0);

    // Four quads, 4 vertices/edges/surfedges each, laid out contiguously so
    // quad `i`'s surfedges start at slot `i * 4` (edge `i * 4 + 1`).
    let quads: [[[f32; 3]; 4]; 4] = [
        // Quad 0: the good occluder.
        [
            [-8.0, -8.0, 0.0],
            [8.0, -8.0, 0.0],
            [8.0, 8.0, 0.0],
            [-8.0, 8.0, 0.0],
        ],
        // Quad 1: the sky face.
        [
            [-8.0, -8.0, 64.0],
            [8.0, -8.0, 64.0],
            [8.0, 8.0, 64.0],
            [-8.0, 8.0, 64.0],
        ],
        // Quad 2: finite vertices, but paired with a non-finite `texinfo`.
        [
            [-8.0, -8.0, 128.0],
            [8.0, -8.0, 128.0],
            [8.0, 8.0, 128.0],
            [-8.0, 8.0, 128.0],
        ],
        // Quad 3: only its first 2 vertices are referenced (see `push_face`
        // below), producing a degenerate 2-vertex "polygon".
        [
            [-8.0, -8.0, 192.0],
            [8.0, -8.0, 192.0],
            [8.0, 8.0, 192.0],
            [-8.0, 8.0, 192.0],
        ],
    ];
    for (index, quad) in quads.iter().enumerate() {
        let base = u16::try_from(index * 4).expect("four quads fit u16");
        for corner in quad {
            b.push_vertex(*corner);
        }
        for corner in 0..4u16 {
            let next = (corner + 1) % 4;
            b.push_edge(base + corner, base + next);
        }
        let first_edge = i32::try_from(index * 4 + 1).expect("four quads fit i32");
        for step in 0..4 {
            b.push_surfedge(first_edge + step);
        }
    }

    // texinfo 0: the good occluder's, finite.
    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 0, 0);
    // texinfo 1: the sky face's, finite, pointing at the sky texture slot.
    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 1, 0);
    // texinfo 2: a malformed `s_shift` (non-finite) over quad 2's otherwise
    // perfectly ordinary, finite vertex positions.
    b.push_texinfo([1.0, 0.0, 0.0], f32::INFINITY, [0.0, 1.0, 0.0], 0.0, 0, 0);
    // texinfo 3: finite; quad 3 is dropped for its vertex count, not this.
    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 0, 0);

    // Every face is unlit (`lightmap_offset < 0`), so this fixture needs no
    // lighting lump at all.
    let unlit_styles = [0xFF, 0xFF, 0xFF, 0xFF];
    b.push_face(0, 0, 1, 4, 0, unlit_styles, -1); // face 0: good occluder
    b.push_face(0, 0, 5, 4, 1, unlit_styles, -1); // face 1: sky
    b.push_face(0, 0, 9, 4, 2, unlit_styles, -1); // face 2: non-finite texinfo
    b.push_face(0, 0, 13, 2, 3, unlit_styles, -1); // face 3: degenerate (2 edges)

    b.push_model(
        [-8.0, -8.0, 0.0],
        [8.0, 8.0, 192.0],
        [0.0, 0.0, 0.0],
        [0, 0, 0, 0],
        0,
        0,
        4,
    );

    b.add_embedded_texture("ohlfloor", 4, 4, 128);
    b.add_embedded_texture("sky", 4, 4, 128);

    b.build()
}

#[test]
fn a_non_finite_or_degenerate_face_is_dropped_and_counted_instead_of_failing_the_whole_model() {
    let bytes = map_with_a_bad_face_alongside_a_sky_face();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("the fixture parses");
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .expect("one bad face must not fail the whole model's build");

    // The sky face is real and correctly detected; the good occluder is the
    // only face left in the drawable mesh (the sky face is excluded from
    // `faces` by design, and the two bad faces are dropped, not drawn).
    assert!(model.has_sky, "the sky face must still be recognised");
    assert_eq!(
        model.faces.len(),
        1,
        "only the one good, non-sky face should end up in the drawable mesh"
    );
    assert_eq!(
        model.dropped_faces, 2,
        "the non-finite-texinfo face and the degenerate 2-vertex face are \
         both counted, not silently discarded with no diagnostic"
    );
}
