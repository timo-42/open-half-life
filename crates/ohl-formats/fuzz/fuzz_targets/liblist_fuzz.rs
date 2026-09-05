//! `cargo fuzz` target for `ohl_formats::liblist::parse` and its accessors.
//! Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::liblist::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(list) = liblist::parse(data, &limits) else {
        return;
    };
    let _ = list.startmap();
    let _ = list.trainmap();
    let _ = list.game();
    let _ = list.game_type();
    let _ = list.mpentity();
    for (key, _) in list.entries() {
        let _ = list.get(key);
    }
});
