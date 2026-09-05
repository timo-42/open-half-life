//! `cargo fuzz` target for `ohl_formats::sentences::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::sentences::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(file) = sentences::parse(data, &limits) else {
        return;
    };
    for sentence in file.sentences() {
        let _ = file.find(sentence.name);
        for word in &sentence.words {
            let _ = word.token;
            let _ = word.modifiers;
        }
    }
});
