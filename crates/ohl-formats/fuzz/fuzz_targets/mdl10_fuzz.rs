//! `cargo fuzz` target for `ohl_formats::mdl10::Mdl::parse` and its
//! accessors. Must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::mdl10::{Limits, Mdl, SequenceGroupFile};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let _ = SequenceGroupFile::parse(data);
    let Ok(mdl) = Mdl::parse(data, &limits) else {
        return;
    };
    let _ = mdl.name();
    let Ok(bones) = mdl.bones(&limits) else {
        return;
    };
    let _ = Mdl::validate_bone_hierarchy(bones);
    let _ = mdl.bone_controllers(&limits);
    let _ = mdl.hitboxes(&limits);
    let _ = mdl.attachments(&limits);
    let _ = mdl.transitions(&limits);
    let _ = mdl.skin_families(&limits);

    if let Ok(textures) = mdl.textures(&limits) {
        for texture in textures.iter().take(4) {
            let _ = mdl.decode_texture(texture, &limits);
        }
    }

    if let Ok(sequences) = mdl.sequences(&limits) {
        for seq in sequences.iter().take(4) {
            let _ = mdl.events(seq, &limits);
            let _ = Mdl::sample_bone_animation(data, seq, bones, 0, &limits);
            let _ = Mdl::sample_bone_animation(data, seq, bones, u32::MAX, &limits);
        }
    }

    if let Ok(body_parts) = mdl.body_parts(&limits) {
        for body_part in body_parts.iter().take(4) {
            let Ok(models) = mdl.models(body_part, &limits) else {
                continue;
            };
            for model in models.iter().take(4) {
                let _ = mdl.vertices(model, &limits);
                let _ = mdl.normals(model, &limits);
                let _ = mdl.vertex_bones(model, &limits);
                let _ = mdl.normal_bones(model, &limits);
                if let Ok(meshes) = mdl.meshes(model, &limits) {
                    for mesh in meshes.iter().take(4) {
                        let _ = mdl.decode_mesh_commands(mesh, &limits);
                    }
                }
            }
        }
    }
});
