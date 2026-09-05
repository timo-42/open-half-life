//! `cargo fuzz` target for `ohl_formats::pak::Directory::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::pak::{Directory, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(dir) = Directory::parse(data, &limits) else {
        return;
    };
    for entry in dir.entries().take(64) {
        let _ = entry.trimmed_name();
        let _ = entry.name_matches("anything");
    }
    let _ = dir.find("anything");
});
