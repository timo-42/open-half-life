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
