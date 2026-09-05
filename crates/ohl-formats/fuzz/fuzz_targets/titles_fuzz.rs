//! `cargo fuzz` target for `ohl_formats::titles::parse` and its accessors.
//! Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::titles::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(file) = titles::parse(data, &limits) else {
        return;
    };
    for message in file.messages() {
        let _ = message.text_lossy();
        let _ = file.find(message.name);
    }
});
