//! Fuzz target: InstallShield 3 archive header and table-of-contents parsing
//! must never panic and never read out of bounds for any input.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_isz::{Archive, Limits, NeverCancelled, SliceSource, find_signature};

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_scan_bytes: 1 << 20,
        max_archive_bytes: 1 << 22,
        max_directories: 256,
        max_entries: 4096,
        max_directory_bytes: 1 << 18,
        max_name_bytes: 260,
        max_stored_bytes_per_entry: 1 << 20,
        max_expanded_bytes_per_entry: 1 << 20,
        max_total_expanded_bytes: 1 << 22,
        max_chunk_bytes: 4096,
    };

    let mut scanner = SliceSource::new(data);
    let base = match find_signature(&mut scanner, &limits, &NeverCancelled) {
        Ok(Some(base)) => base,
        _ => 0,
    };

    let mut source = SliceSource::new(data);
    let Ok(mut archive) = Archive::open(&mut source, base, &limits, &NeverCancelled) else {
        return;
    };
    let count = u32::try_from(archive.entries().len()).unwrap_or(0);
    for entry in archive.entries() {
        let _ = entry.name.extension_bytes();
    }
    for directory in archive.directories() {
        let _ = directory.name.len();
    }
    for index in 0..count.min(256) {
        let mut source = SliceSource::new(data);
        let Ok(mut reader) = archive.open_entry(index) else {
            continue;
        };
        let _ = reader.read_to_vec(&mut source, &NeverCancelled);
    }
});
