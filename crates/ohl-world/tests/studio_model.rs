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
