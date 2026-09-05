//! Fuzzes the DEFLATE stream chain walk, including its bounded
//! resynchronisation policy, over arbitrary bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_wise::{Chain, Limits, NeverCancelled, SliceSource};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_streams: 64,
        max_compressed_bytes_per_stream: 1 << 20,
        max_inflated_bytes_per_stream: 1 << 20,
        max_total_inflated_bytes: 1 << 22,
        ..Limits::DEFAULT
    };
    let mut source = SliceSource::new(data);
    let mut chain = Chain::new(0, data.len() as u64, limits);
    while let Some(event) = chain.next_event(&mut source, &NeverCancelled) {
        if event.is_err() {
            break;
        }
    }
});
