//! Posing a studio model must never panic, for any sequence index, frame,
//! or playback time.
//!
//! The animation data a real model carries is compressed and cross
//! referenced by offsets, so a hostile or truncated file can point a
//! sequence anywhere. `StudioPose::sample` is the only entry point that
//! walks those offsets, so it is the one that has to stay total: every
//! out-of-range case must come back as an error or a bind pose, never as a
//! panic.
//!
//! The fixture is the project-authored synthetic model; its bytes are
//! mutated field by field so that every generated file is still
//! *structurally* well formed (a real header, whole records) with arbitrary
//! sequence descriptions, which is the shape a hostile model would take.

use ohl_formats::mdl10::Limits;
use ohl_formats::test_support::{MinimalMdl10Layout, build_minimal_mdl10};
use ohl_world::{StudioModel, StudioPose};
use proptest::prelude::*;

/// Overwrites four bytes at `offset` with a little-endian `u32`.
fn patch_u32(bytes: &mut [u8], offset: usize, value: u32) {
    if let Some(slot) = bytes.get_mut(offset..offset + 4) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

/// Overwrites four bytes at `offset` with a little-endian `f32`.
fn patch_f32(bytes: &mut [u8], offset: usize, value: f32) {
    if let Some(slot) = bytes.get_mut(offset..offset + 4) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

/// Field offsets inside the fixture's single 176-byte sequence record,
/// derived from the documented layout (see `docs/FORMAT_SOURCES.md`).
const SEQUENCE_FPS: usize = 32;
const SEQUENCE_FLAGS: usize = 36;
const SEQUENCE_NUM_FRAMES: usize = 56;
const SEQUENCE_ANIM_INDEX: usize = 124;

fn fixture() -> (Vec<u8>, MinimalMdl10Layout) {
    build_minimal_mdl10()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any sequence description, sampled at any time, either builds a pose
    /// or reports an error.
    #[test]
    fn sampling_never_panics(
        fps in prop::num::f32::ANY,
        flags in prop::num::i32::ANY,
        frames in 0u32..4096,
        anim_index in prop::num::u32::ANY,
        sequence in 0usize..8,
        time in prop::num::f32::ANY,
    ) {
        let (mut bytes, layout) = fixture();
        let record = layout.sequences_offset;
        patch_f32(&mut bytes, record + SEQUENCE_FPS, fps);
        patch_u32(&mut bytes, record + SEQUENCE_FLAGS, flags.cast_unsigned());
        patch_u32(&mut bytes, record + SEQUENCE_NUM_FRAMES, frames);
        patch_u32(&mut bytes, record + SEQUENCE_ANIM_INDEX, anim_index);

        let limits = Limits::default();
        let Ok(model) = StudioModel::parse(&bytes, &limits) else {
            return Ok(());
        };
        // Every sequence index, in range or not, and every frame the
        // sequence claims to have.
        let _ = StudioPose::sample(&model, sequence, time);
        for frame in [0u32, 1, frames.saturating_sub(1), frames, u32::MAX] {
            #[allow(clippy::cast_precision_loss)]
            let at = frame as f32 / 10.0;
            let _ = StudioPose::sample(&model, sequence, at);
        }
        let _ = StudioPose::bind(&model);
    }

    /// Building a model from an arbitrarily rewritten mesh command stream,
    /// vertex table, or texture header either succeeds or errors out.
    #[test]
    fn building_never_panics(
        tri_count in prop::num::i32::ANY,
        tri_index in prop::num::u32::ANY,
        num_verts in prop::num::u32::ANY,
        skin_ref in prop::num::i32::ANY,
        texture_flags in prop::num::u32::ANY,
        texture_width in prop::num::u32::ANY,
    ) {
        let (mut bytes, layout) = fixture();
        // Mesh record: `num_tris`, `tri_index`, `skin_ref`.
        patch_u32(&mut bytes, layout.meshes_offset, tri_count.cast_unsigned());
        patch_u32(&mut bytes, layout.meshes_offset + 4, tri_index);
        patch_u32(&mut bytes, layout.meshes_offset + 8, skin_ref.cast_unsigned());
        // Model record: `num_verts` sits 64 + 4 + 4 + 4 + 4 bytes in.
        patch_u32(&mut bytes, layout.models_offset + 80, num_verts);
        // Texture record: `flags` and `width` follow the 64-byte name.
        patch_u32(&mut bytes, layout.textures_offset + 64, texture_flags);
        patch_u32(&mut bytes, layout.textures_offset + 68, texture_width);

        let limits = Limits::default();
        if let Ok(model) = StudioModel::parse(&bytes, &limits) {
            let _ = model.visible_meshes(&[0, 1, 2]);
            for family in 0..3 {
                for slot in 0..3 {
                    let _ = model.resolve_skin(family, slot);
                }
            }
            let pose = StudioPose::bind(&model);
            for hitbox in &model.hitboxes {
                let _ = pose.hitbox_bounds(hitbox);
            }
            for attachment in &model.attachments {
                let _ = pose.attachment_origin(attachment);
            }
        }
    }
}
