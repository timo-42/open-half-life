//! `proptest`-driven fuzzing over arbitrary bytes: `bsp30::Bsp::parse`,
//! `wad3::Wad3::parse`, `mdl10::Mdl::parse`/`SequenceGroupFile::parse`, and
//! `spr::Spr::parse`, plus every accessor this crate exposes, must never
//! panic, no matter how malformed the input is.

use ohl_formats::bsp30::{Bsp, LumpId};
use ohl_formats::mdl10::{Mdl, SequenceGroupFile};
use ohl_formats::spr::Spr;
use ohl_formats::wad3::Wad3;
use proptest::prelude::*;

fn exercise_bsp(data: &[u8]) {
    let limits = ohl_formats::bsp30::Limits::default();
    let Ok(bsp) = Bsp::parse(data, &limits) else {
        return;
    };
    let _ = bsp.entities(&limits);
    let _ = bsp.planes(&limits);
    let _ = bsp.vertices(&limits);
    let _ = bsp.nodes(&limits);
    let _ = bsp.texinfo(&limits);
    let _ = bsp.faces(&limits);
    let _ = bsp.lighting(&limits);
    let _ = bsp.clipnodes(&limits);
    let _ = bsp.marksurfaces(&limits);
    let _ = bsp.edges(&limits);
    let _ = bsp.surfedges(&limits);
    let _ = bsp.models(&limits);
    let _ = bsp.raw_lump(LumpId::Visibility, &limits);

    if let Ok(leaves) = bsp.leaves(&limits) {
        for leaf in leaves.iter().take(4) {
            let _ = bsp.is_leaf_visible(leaf, 0, leaves.len().max(1), &limits);
        }
    }
    if let Ok(models) = bsp.models(&limits) {
        for model in models.iter().take(4) {
            let _ = bsp.find_leaf(model.headnodes[0].get(), [0.0, 0.0, 0.0], &limits);
        }
    }
    if let Ok(textures) = bsp.textures(&limits) {
        for i in 0..textures.len().min(8) {
            let _ = textures.get(i);
        }
    }
}

fn exercise_wad3(data: &[u8]) {
    let limits = ohl_formats::wad3::Limits::default();
    let Ok(wad) = Wad3::parse(data, &limits) else {
        return;
    };
    for entry in wad.entries().take(64) {
        let Ok(entry) = entry else { continue };
        let _ = wad.decode_miptex(&entry);
    }
    let _ = wad.find("anything");
}

fn exercise_mdl10(data: &[u8]) {
    let limits = ohl_formats::mdl10::Limits::default();
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
}

fn exercise_spr(data: &[u8]) {
    let limits = ohl_formats::spr::Limits::default();
    let Ok(spr) = Spr::parse(data, &limits) else {
        return;
    };
    let _ = spr.kind();
    let _ = spr.texture_format();
    let _ = spr.sync_type();
    let _ = spr.palette();
    for i in 0..spr.frame_count().min(8) {
        if let Ok(frame) = spr.frame(i, &limits) {
            let _ = frame.image.pixel(0, 0);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn bsp30_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_bsp(&data);
    }

    #[test]
    fn wad3_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_wad3(&data);
    }

    #[test]
    fn mdl10_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_mdl10(&data);
    }

    #[test]
    fn spr_parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        exercise_spr(&data);
    }
}
