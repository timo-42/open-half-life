//! Fuzzes the script-binary file-record recogniser and the record-to-stream
//! mapping over arbitrary bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_wise::{FileMap, Limits, Overlay, script};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_script_bytes: 1 << 16,
        max_file_records: 512,
        max_inflated_bytes_per_stream: 1 << 20,
        ..Limits::DEFAULT
    };
    let overlay = Overlay {
        offset: 0x1000,
        len: data.len() as u64,
        image_len: 0x1000 + data.len() as u64,
    };
    if let Ok(table) = script::parse(data, &overlay, &limits) {
        let map = FileMap::build(&table, &[], &overlay);
        for index in 0..table.len() {
            let _ = map.open_file(&table, index, limits);
        }
    }
});
