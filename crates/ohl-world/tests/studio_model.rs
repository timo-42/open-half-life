//! End-to-end checks over the project-authored synthetic studio model.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::mdl10::Limits;
use ohl_formats::test_support::build_minimal_mdl10;
use ohl_world::{STUDIO_VERTEX_BYTES, StudioModel, StudioPose, studio_vertex_bytes};

fn model() -> StudioModel {
    let (bytes, _) = build_minimal_mdl10();
    StudioModel::parse(&bytes, &Limits::default()).expect("the synthetic model builds")
}

#[test]
fn every_index_addresses_a_real_vertex() {
    let model = model();
    assert!(!model.indices.is_empty());
    assert_eq!(model.indices.len() % 3, 0);
    for index in &model.indices {
        assert!((*index as usize) < model.vertices.len());
    }
    for mesh in &model.meshes {
        let end = (mesh.first_index + mesh.index_count) as usize;
        assert!(end <= model.indices.len());
        assert_eq!(mesh.index_count % 3, 0);
    }
}

#[test]
fn the_vertex_buffer_is_exactly_as_long_as_the_stride_implies() {
    let model = model();
    let bytes = studio_vertex_bytes(&model.vertices);
    assert_eq!(bytes.len(), model.vertices.len() * STUDIO_VERTEX_BYTES);
}

#[test]
fn every_bone_matrix_stays_finite_across_a_whole_sequence() {
    let model = model();
    let sequence = model.sequences.first().copied().expect("one sequence");
    assert!(sequence.duration() > 0.0);
    for step in 0..64 {
        #[allow(clippy::cast_precision_loss)]
        let time = step as f32 * sequence.duration() / 32.0;
        let pose = StudioPose::sample(&model, 0, time).expect("samples");
        assert_eq!(pose.matrices.len(), model.bones.len());
        for matrix in &pose.matrices {
            assert!(matrix.iter().all(|value| value.is_finite()));
            // The bottom row of an affine transform is unchanged.
            assert!((matrix[15] - 1.0).abs() < 1e-6);
        }
    }
}

#[test]
fn the_bind_pose_has_one_matrix_per_bone() {
    let model = model();
    let pose = StudioPose::bind(&model);
    assert_eq!(pose.matrices.len(), model.bones.len());
    for hitbox in &model.hitboxes {
        assert!(pose.hitbox_bounds(hitbox).is_some());
    }
    for attachment in &model.attachments {
        assert!(pose.attachment_origin(attachment).is_some());
    }
}

/// A GoldSrc studio model whose textures are externalized (`numtextures ==
/// 0` in the main file) must source its real texture, not the placeholder,
/// from the companion texture file's bytes when one is given.
///
/// The synthetic "main" file here is the ordinary fixture with its header
/// `numtextures` field zeroed by hand (offset 180, an `i32`, per the public
/// MDL v10 header layout `docs/FORMAT_SOURCES.md` cites) — a project-
/// authored synthetic fixture, not a byte from any game installation (see
/// `docs/CLEAN_ROOM.md`). The unmodified fixture itself stands in as the
/// "companion" file, since it already carries one real 16x16 texture and a
/// matching skin-family table under the same MDL v10 header layout an
/// external texture file shares.
#[test]
fn external_texture_file_supplies_the_real_texture_not_the_placeholder() {
    const NUM_TEXTURES_OFFSET: usize = 180;

    let (companion_bytes, _) = build_minimal_mdl10();
    let mut main_bytes = companion_bytes.clone();
    main_bytes[NUM_TEXTURES_OFFSET..NUM_TEXTURES_OFFSET + 4].copy_from_slice(&0i32.to_le_bytes());

    let model = StudioModel::parse_with_external_texture(
        &main_bytes,
        Some(&companion_bytes),
        &Limits::default(),
    )
    .expect("the model builds using the companion file's textures");

    assert_eq!(model.textures.len(), 1);
    assert_eq!(model.textures[0].image.width(), 16);
    assert_eq!(model.textures[0].image.height(), 16);
}

/// The same zeroed-out main file with no companion bytes given must still
/// build (never an error) and fall back to the placeholder texture, exactly
/// as an ordinary model with no textures at all does.
#[test]
fn missing_external_texture_file_falls_back_to_the_placeholder() {
    const NUM_TEXTURES_OFFSET: usize = 180;

    let (companion_bytes, _) = build_minimal_mdl10();
    let mut main_bytes = companion_bytes;
    main_bytes[NUM_TEXTURES_OFFSET..NUM_TEXTURES_OFFSET + 4].copy_from_slice(&0i32.to_le_bytes());

    let model = StudioModel::parse_with_external_texture(&main_bytes, None, &Limits::default())
        .expect("the model still builds without a companion file");

    // `ohl_world::texture::PLACEHOLDER_EDGE` (64), not re-exported from the
    // crate root; the fixture's own real texture is 16x16, so this also
    // confirms the fallback did not silently reuse it.
    assert_eq!(model.textures.len(), 1);
    assert_eq!(model.textures[0].image.width(), 64);
}

/// A manual-check aid, mirroring `world_model.rs`'s synthetic-room writer:
/// drops the synthetic model into the temporary directory so `--dev-mdl` can
/// be pointed at it without any game media.
#[test]
#[ignore = "manual-check aid: writes a synthetic model to the temp directory"]
fn writes_the_synthetic_model_for_manual_checks() {
    let (bytes, _) = build_minimal_mdl10();
    std::fs::write(std::env::temp_dir().join("ohl-synthetic-model.mdl"), bytes)
        .expect("temporary directory is writable");
}
