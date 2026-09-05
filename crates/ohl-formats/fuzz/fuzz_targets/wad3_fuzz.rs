//! `cargo fuzz` target for `ohl_formats::wad3::Wad3::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::wad3::{Limits, Wad3};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(wad) = Wad3::parse(data, &limits) else {
        return;
    };
    for entry in wad.entries().take(64) {
        let Ok(entry) = entry else { continue };
        let _ = wad.decode_miptex(&entry);
    }
    let _ = wad.find("anything");
});
