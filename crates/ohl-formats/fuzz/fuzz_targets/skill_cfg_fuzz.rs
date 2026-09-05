//! `cargo fuzz` target for `ohl_formats::skill_cfg::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::skill_cfg::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(cfg) = skill_cfg::parse(data, &limits) else {
        return;
    };
    for entry in cfg.entries() {
        let _ = cfg.get(entry.cvar);
    }
});
