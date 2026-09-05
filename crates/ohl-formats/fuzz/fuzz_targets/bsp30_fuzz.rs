//! `cargo fuzz` target for `ohl_formats::bsp30::Bsp::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::bsp30::{Bsp, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
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

    if let Ok(models) = bsp.models(&limits) {
        for model in models.iter().take(4) {
            let _ = bsp.find_leaf(model.headnodes[0].get(), [0.0, 0.0, 0.0], &limits);
        }
    }
    if let Ok(leaves) = bsp.leaves(&limits) {
        for leaf in leaves.iter().take(4) {
            let _ = bsp.is_leaf_visible(leaf, 0, leaves.len().max(1), &limits);
        }
    }
    if let Ok(textures) = bsp.textures(&limits) {
        for i in 0..textures.len().min(8) {
            let _ = textures.get(i);
        }
    }
});
