//! Fuzzes the whole Wise package reader over arbitrary bytes: PE overlay
//! location, the bounded header scan, the stream chain and the script.
//!
//! Every input byte comes from the fuzzer; nothing is derived from media.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_wise::{Limits, NeverCancelled, SliceSource, read_package};

fn limits() -> Limits {
    Limits {
        max_streams: 64,
        max_compressed_bytes_per_stream: 1 << 20,
        max_inflated_bytes_per_stream: 1 << 20,
        max_total_inflated_bytes: 1 << 22,
        max_script_bytes: 1 << 16,
        max_file_records: 512,
        ..Limits::DEFAULT
    }
}

fuzz_target!(|data: &[u8]| {
    let mut source = SliceSource::new(data);
    let _ = read_package(&mut source, None, limits(), &NeverCancelled);
    let mut source = SliceSource::new(data);
    let _ = read_package(&mut source, Some(0), limits(), &NeverCancelled);
});
