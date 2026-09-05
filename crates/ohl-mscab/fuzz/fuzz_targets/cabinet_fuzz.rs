//! `cargo fuzz` target for `ohl_mscab::Cabinet::parse` and folder extraction
//! over wholly arbitrary bytes. Must never panic on any input.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_mscab::{Cabinet, FolderStream, Limits, NeverCancelled, SliceSource};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        // Keep a fuzz iteration cheap while still exercising every bound.
        max_folder_uncompressed_bytes: 1 << 22,
        max_blocks_per_folder: 1 << 12,
        ..Limits::default()
    };
    let source = SliceSource::new(data);
    let Ok(cabinet) = Cabinet::parse(&source, 0, &limits) else {
        return;
    };
    for file in cabinet.files().iter().take(64) {
        let _ = file.name_utf8();
        let _ = file.name_bytes();
        let _ = file.date_time();
        let _ = cabinet.folder_of(file);
    }
    for folder_index in 0..cabinet.header().folder_count.min(8) {
        let Ok(mut stream) =
            FolderStream::from_cabinet(&cabinet, &source, 0, folder_index, limits)
        else {
            continue;
        };
        let mut buffer = [0u8; 4_096];
        let mut blocks = 0u32;
        while let Ok(read) = stream.read(&mut buffer, &NeverCancelled) {
            blocks += 1;
            if read == 0 || blocks > 4_096 {
                break;
            }
        }
    }
});
