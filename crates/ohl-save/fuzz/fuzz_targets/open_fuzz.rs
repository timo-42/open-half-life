//! `cargo fuzz` target for `SaveReader::open` over wholly arbitrary bytes.
//! Must never panic on any input, including truncated or adversarially
//! crafted save files.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_save::{Limits, SaveReader};

fuzz_target!(|data: &[u8]| {
    let _ = SaveReader::open(data, &Limits::default());
});
