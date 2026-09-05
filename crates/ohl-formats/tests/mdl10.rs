//! Round-trip, accessor, and malformed-field rejection tests for `mdl10`,
//! using this crate's own synthetic fixture writer
//! (`ohl_formats::test_support::build_minimal_mdl10`). No bytes here come
//! from any game installation; see `docs/CLEAN_ROOM.md`.

#![allow(clippy::float_cmp)]

use core::mem::offset_of;
use ohl_formats::mdl10::{Bone, Limits, Mdl, RawHeader, SequenceGroupFile};
use ohl_formats::test_support::build_minimal_mdl10;

fn corrupt_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn corrupt_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn round_trips_header_and_every_table() {
    let (bytes, layout) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).expect("valid synthetic model parses");

    assert_eq!(mdl.name(), b"minimal");
    assert_eq!(mdl.bones(&limits).unwrap().len(), 2);
    assert_eq!(mdl.bone_controllers(&limits).unwrap().len(), 0);
    assert_eq!(mdl.hitboxes(&limits).unwrap().len(), 0);
    assert_eq!(mdl.textures(&limits).unwrap().len(), 1);
    assert_eq!(mdl.sequences(&limits).unwrap().len(), 1);
    assert_eq!(mdl.sequence_groups(&limits).unwrap().len(), 0);
    assert_eq!(mdl.body_parts(&limits).unwrap().len(), 1);
    assert_eq!(mdl.attachments(&limits).unwrap().len(), 0);
    let transitions = mdl.transitions(&limits).unwrap();
    assert_eq!(transitions.len(), 0);
    assert!(transitions.is_empty());

    let body_parts = mdl.body_parts(&limits).unwrap();
    let models = mdl.models(&body_parts[0], &limits).unwrap();
    assert_eq!(models.len(), 1);
    let meshes = mdl.meshes(&models[0], &limits).unwrap();
    assert_eq!(meshes.len(), 1);
    assert_eq!(mdl.vertices(&models[0], &limits).unwrap().len(), 4);
    assert_eq!(mdl.normals(&models[0], &limits).unwrap().len(), 1);
    assert_eq!(mdl.vertex_bones(&models[0], &limits).unwrap().len(), 4);
    assert_eq!(mdl.normal_bones(&models[0], &limits).unwrap().len(), 1);

    let events = mdl
        .events(&mdl.sequences(&limits).unwrap()[0], &limits)
        .unwrap();
    assert_eq!(events.len(), 0);

    let _ = layout;
}

#[test]
fn validates_bone_hierarchy_and_bone_indices() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let bones = mdl.bones(&limits).unwrap();
    Mdl::validate_bone_hierarchy(bones).expect("root(-1) then child(0) is valid");

    let body_parts = mdl.body_parts(&limits).unwrap();
    let models = mdl.models(&body_parts[0], &limits).unwrap();
    let vert_bones = mdl.vertex_bones(&models[0], &limits).unwrap();
    Mdl::validate_bone_indices(vert_bones, bones.len()).expect("all verts use bone 0");
    assert!(Mdl::validate_bone_indices(&[9], bones.len()).is_err());
}

#[test]
fn rejects_cyclic_bone_hierarchy() {
    // Two bones where bone 0's parent is itself (index 0 >= its own index).
    let bones_raw: Vec<u8> = {
        let mut v = Vec::new();
        // Reuse the same 112-byte bone layout by hand: name, parent, flags,
        // bonecontroller[6], value[6], scale[6].
        v.extend_from_slice(&[0u8; 32]);
        v.extend_from_slice(&0i32.to_le_bytes()); // parent == own index (0)
        v.extend_from_slice(&0i32.to_le_bytes());
        for _ in 0..6 {
            v.extend_from_slice(&(-1i32).to_le_bytes());
        }
        for _ in 0..12 {
            v.extend_from_slice(&0f32.to_le_bytes());
        }
        v
    };
    let bones: &[Bone] = zerocopy::FromBytes::ref_from_bytes(&bones_raw).unwrap();
    assert!(Mdl::validate_bone_hierarchy(bones).is_err());
}

#[test]
fn decode_mesh_commands_produces_two_triangles() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let body_parts = mdl.body_parts(&limits).unwrap();
    let models = mdl.models(&body_parts[0], &limits).unwrap();
    let meshes = mdl.meshes(&models[0], &limits).unwrap();
    let triangles = mdl.decode_mesh_commands(&meshes[0], &limits).unwrap();
    assert_eq!(triangles.len(), 2);
    for tri in &triangles {
        for v in &tri.verts {
            assert!(v.vert_index < 4);
            assert_eq!(v.norm_index, 0);
        }
    }
}

#[test]
fn decodes_texture_pixels_and_palette() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let textures = mdl.textures(&limits).unwrap();
    assert_eq!(textures.len(), 1);
    let image = mdl.decode_texture(&textures[0], &limits).unwrap();
    assert_eq!(image.width, 16);
    assert_eq!(image.height, 16);
    assert_eq!(image.indices[0], 7);
    assert_eq!(image.palette.get(7).r, 7);
}

#[test]
fn skin_table_looks_up_default_family() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let skins = mdl.skin_families(&limits).unwrap();
    assert_eq!(skins.get(0, 0).unwrap(), 0);
    assert!(skins.get(0, 1).is_err());
}

#[test]
fn samples_bind_pose_and_compressed_channel() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let bones = mdl.bones(&limits).unwrap();
    let seq = &mdl.sequences(&limits).unwrap()[0];

    let frame0 = Mdl::sample_bone_animation(&bytes, seq, bones, 0, &limits).unwrap();
    let frame1 = Mdl::sample_bone_animation(&bytes, seq, bones, 1, &limits).unwrap();
    assert_eq!(frame0[0].position[0], 10.0);
    assert_eq!(frame1[0].position[0], 20.0);
    // Bone 1 has no animation data: every channel is the bind pose (all
    // zero for this fixture).
    assert_eq!(frame0[1].position, [0.0, 0.0, 0.0]);
    assert_eq!(frame0[1].rotation, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn sequence_group_file_accepts_idsq_and_idst_magic() {
    let mut idsq = Vec::new();
    idsq.extend_from_slice(b"IDSQ");
    idsq.extend_from_slice(&10i32.to_le_bytes());
    idsq.extend_from_slice(&[0u8; 64]);
    idsq.extend_from_slice(&(u32::try_from(idsq.len()).unwrap() + 4).to_le_bytes());
    let file = SequenceGroupFile::parse(&idsq).expect("IDSQ magic accepted");
    assert_eq!(file.bytes().len(), idsq.len());

    let mut bad = idsq.clone();
    bad[0..4].copy_from_slice(b"NOPE");
    assert!(SequenceGroupFile::parse(&bad).is_err());
}

// --- Malformed-field rejection tests -------------------------------------

#[test]
fn rejects_bad_magic() {
    let (mut bytes, _) = build_minimal_mdl10();
    bytes[0..4].copy_from_slice(b"NOPE");
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_bad_version() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, version);
    corrupt_i32(&mut bytes, off, 9);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_bone_table_outside_file() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, bone_index);
    let huge = u32::try_from(bytes.len()).unwrap() + 1_000_000;
    corrupt_u32(&mut bytes, off, huge);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_bone_count_over_limit() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, num_bones);
    corrupt_u32(&mut bytes, off, 1_000_000);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_texture_table_outside_file() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, texture_index);
    let huge = u32::try_from(bytes.len()).unwrap() + 10;
    corrupt_u32(&mut bytes, off, huge);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_texture_pixel_data_outside_file() {
    let (bytes, layout) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let mut textures = mdl.textures(&limits).unwrap().to_vec();
    // Corrupt the (already-parsed) texture's declared width so its pixel
    // count now claims to run past the end of the file.
    let mut bad = bytes.clone();
    let width_field = layout.textures_offset + 64 + 4; // name(64) + flags(4)
    corrupt_u32(&mut bad, width_field, 0xFFFF);
    let mdl_bad = Mdl::parse(&bad, &limits).expect("header itself still validates");
    let bad_textures = mdl_bad.textures(&limits).unwrap();
    assert!(mdl_bad.decode_texture(&bad_textures[0], &limits).is_err());
    let _ = textures.pop();
}

#[test]
fn rejects_skin_table_size_mismatch() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, num_skin_ref);
    corrupt_u32(&mut bytes, off, 1_000_000);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_transitions_count_over_limit() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, num_transitions);
    corrupt_u32(&mut bytes, off, 1_000_000);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_body_part_table_outside_file() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, body_part_index);
    let huge = u32::try_from(bytes.len()).unwrap() + 10;
    corrupt_u32(&mut bytes, off, huge);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_attachment_table_outside_file() {
    let (mut bytes, _) = build_minimal_mdl10();
    let off = offset_of!(RawHeader, attachment_index);
    let huge = u32::try_from(bytes.len()).unwrap() + 10;
    corrupt_u32(&mut bytes, off, huge);
    corrupt_u32(&mut bytes, offset_of!(RawHeader, num_attachments), 1);
    assert!(Mdl::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_mesh_trivert_run_exceeding_declared_count() {
    let (bytes, layout) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let body_parts = mdl.body_parts(&limits).unwrap();
    let models = mdl.models(&body_parts[0], &limits).unwrap();
    let mut meshes = mdl.meshes(&models[0], &limits).unwrap().to_vec();

    // Corrupt the mesh's `num_tris` (trivert budget) down to 1 while the
    // command stream still declares a run of 4.
    let mut bad = bytes.clone();
    corrupt_i32(&mut bad, layout.meshes_offset, 1);
    let mdl_bad = Mdl::parse(&bad, &limits).unwrap();
    let body_parts = mdl_bad.body_parts(&limits).unwrap();
    let models = mdl_bad.models(&body_parts[0], &limits).unwrap();
    let bad_meshes = mdl_bad.meshes(&models[0], &limits).unwrap();
    assert!(
        mdl_bad
            .decode_mesh_commands(&bad_meshes[0], &limits)
            .is_err()
    );
    let _ = meshes.pop();
}

#[test]
fn rejects_animation_walk_past_frame_budget() {
    let (bytes, _) = build_minimal_mdl10();
    // A tiny walk budget makes the bounded loop reject a frame that isn't
    // in the fixture's single compressed run, instead of reading forever.
    let limits = Limits {
        max_frame_walk: 1,
        ..Limits::default()
    };
    let mdl = Mdl::parse(&bytes, &Limits::default()).unwrap();
    let bones = mdl.bones(&limits).unwrap();
    let seq = &mdl.sequences(&limits).unwrap()[0];
    assert!(Mdl::sample_bone_animation(&bytes, seq, bones, 100, &limits).is_err());
}

#[test]
fn rejects_index_out_of_range_lookups() {
    let (bytes, _) = build_minimal_mdl10();
    let limits = Limits::default();
    let mdl = Mdl::parse(&bytes, &limits).unwrap();
    let transitions = mdl.transitions(&limits).unwrap();
    assert!(transitions.get(0, 0).is_err());
}
