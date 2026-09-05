//! Fuzz target: cabinet header parsing must never panic and never read out
//! of bounds for any input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_cabinet_format::{CabinetHeader, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_header_bytes: 1 << 22,
        max_files: 4096,
        max_directories: 4096,
        max_file_groups: 512,
        max_components: 512,
        max_name_bytes: 1024,
        max_volumes: 64,
    };
    let Ok(header) = CabinetHeader::parse(data, &limits) else {
        return;
    };
    for name in header.directories() {
        let _ = name;
    }
    for descriptor in header.file_descriptors() {
        let _ = descriptor;
    }
    for index in 0..header.file_count().min(4096) {
        let _ = header.file_name(index);
    }
    let _ = header.file_groups().len();
    let _ = header.components().len();
});
