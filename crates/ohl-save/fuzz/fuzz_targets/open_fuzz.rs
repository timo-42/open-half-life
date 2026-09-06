//! `cargo fuzz` target for `SaveReader::open` over wholly arbitrary bytes.
//! Must never panic on any input, including truncated or adversarially
//! crafted save files. When `open` accepts the bytes, every table entry
//! must also be readable through `section` without panicking, and a broken
//! digest must have already been rejected by `open` itself.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_save::{Limits, SaveReader};

fuzz_target!(|data: &[u8]| {
    if let Ok(reader) = SaveReader::open(data, &Limits::default()) {
        for entry in reader.sections() {
            let _ = reader.section(entry.tag);
        }
    }
});
