//! `cargo fuzz` target for `ohl_formats::spr::Spr::parse` and its
//! accessors. Must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::spr::{Limits, Spr};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
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
});
