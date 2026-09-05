//! `cargo fuzz` target for `ohl_formats::hud_sprites::parse` and its
//! accessors. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_formats::hud_sprites::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let Ok(list) = hud_sprites::parse(data, &limits) else {
        return;
    };
    let _ = list.declared_count;
    for row in list.rows() {
        let _ = (
            row.name,
            row.sprite_file,
            row.resolution,
            row.x,
            row.y,
            row.w,
            row.h,
        );
    }
});
