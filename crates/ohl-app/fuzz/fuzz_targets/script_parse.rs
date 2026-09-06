//! `cargo fuzz` target for `Script::parse` over wholly arbitrary bytes,
//! including non-UTF-8 input.
//!
//! Must never panic, and the parsed input count must never exceed
//! `MAX_TOTAL_TICKS` (§7's documented total-ticks limit, enforced inside
//! `parse_line`): this is a regression check on that internal enforcement,
//! not a new limit invented here.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_app::script::{MAX_TOTAL_TICKS, Script};

fuzz_target!(|data: &[u8]| {
    if let Ok(script) = Script::parse(data) {
        assert!(
            (script.inputs().len() as u64) <= MAX_TOTAL_TICKS,
            "parsed input count must never exceed the documented total-ticks limit"
        );
    }
});
