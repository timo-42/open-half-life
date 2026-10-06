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
fn bone_names_are_normalized_lookup_keys() {
    let (mut bytes, layout) = build_minimal_mdl10();
    bytes[layout.bones_offset..layout.bones_offset + 4].copy_from_slice(b"ROOT");
    let model = StudioModel::parse(&bytes, &Limits::default()).unwrap();
    assert_eq!(model.bone_names, ["root", "child"]);
    assert_eq!(model.bone_names.len(), model.bones.len());
}

#[test]
#[allow(clippy::float_cmp)] // Matching bone matrices must be copied exactly.
fn a_held_skeleton_follows_named_bones_and_preserves_unmatched_child_offsets() {
    let mut parent = model();
    parent.bones[1].value[5] = std::f32::consts::FRAC_PI_2;
    let parent_pose = StudioPose::bind(&parent);
    let mut held = model();
    // The held model's root corresponds to bone 1 of the player, rather
    // than its root. The extra bone is independently authored for this test.
    held.bone_names = vec!["child".to_owned(), "ohl_weapon_tip".to_owned()];
    held.bones[1].value[0] = 2.0;
    let merged = StudioPose::merge(&held, &parent, &parent_pose).unwrap();
    assert_eq!(merged.matrices[0], parent_pose.matrices[1]);
    assert!((merged.matrices[1][12] - parent_pose.matrices[1][12]).abs() < 1e-5);
    assert!((merged.matrices[1][13] - parent_pose.matrices[1][13] - 2.0).abs() < 1e-5);

    let first = StudioPose::sample(&parent, 0, 0.0).unwrap();
    let later = StudioPose::sample(&parent, 0, 0.1).unwrap();
    let first_held = StudioPose::merge(&held, &parent, &first).unwrap();
    let later_held = StudioPose::merge(&held, &parent, &later).unwrap();
    assert_ne!(first_held.matrices[0], later_held.matrices[0]);
    assert_eq!(later_held.matrices[0], later.matrices[1]);
}

#[test]
fn unrelated_or_missing_parent_bones_do_not_attach_a_weapon() {
    let parent = model();
    let mut held = model();
    held.bone_names = vec!["ohl_unrelated_a".to_owned(), "ohl_unrelated_b".to_owned()];
    assert!(StudioPose::merge(&held, &parent, &StudioPose::bind(&parent)).is_none());
    assert!(StudioPose::merge(&parent, &parent, &StudioPose { matrices: vec![] }).is_none());
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
