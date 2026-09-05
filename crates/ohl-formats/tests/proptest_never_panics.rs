//! `proptest`-driven fuzzing over arbitrary bytes: both `bsp30::Bsp::parse`
//! and `wad3::Wad3::parse`, plus every accessor this crate exposes, must
//! never panic, no matter how malformed the input is.

use ohl_formats::bsp30::{Bsp, LumpId};
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
}
