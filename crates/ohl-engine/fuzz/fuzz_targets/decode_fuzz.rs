//! `cargo fuzz` target for `GameSave::from_bytes` over wholly arbitrary
//! bytes. Must never panic, including on truncated, malformed, or
//! adversarially crafted save files.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_engine::GameSave;

fuzz_target!(|data: &[u8]| {
    let _ = GameSave::from_bytes(data);
});
