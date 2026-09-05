//! Manual, `#[ignore]`d smoke test over a lawfully owned medium.
//!
//! Point `OHL_TEST_ISO` at a disc image, mount it through `ohl-vfs`, find
//! every file that carries the `MSCF` signature (at any offset, so cabinets
//! embedded in installer executables are found too), parse each one and
//! extract every file to a counting sink.
//!
//! It prints **aggregates only**: counts, byte totals, and checksum results.
//! No name, path, offset, or byte from the medium is printed or committed.

use std::sync::Arc;

use ohl_mscab::{Cabinet, FolderSegment, FolderStream, Limits, NeverCancelled, SliceSource};
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, EntryType, Mount};

/// Largest file this test will buffer while looking for cabinets.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Largest number of signature hits considered per file.
const MAX_CANDIDATES: usize = 64;

#[derive(Default)]
struct Totals {
    files_scanned: u64,
    file_bytes_scanned: u64,
    signature_hits: u64,
    cabinets_parsed: u64,
    cabinet_folders: u64,
    cabinet_files: u64,
    folders_extracted: u64,
    blocks_read: u64,
    checksums_verified: u64,
    checksums_absent: u64,
    uncompressed_bytes: u64,
    parse_failures: u64,
    extract_failures: u64,
}

#[test]
#[ignore = "requires OHL_TEST_ISO to name a lawfully owned medium"]
fn manual_cabinet_aggregates_over_a_real_iso() {
    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        println!("OHL_TEST_ISO is unset; nothing to check");
        return;
    };
    let source = Arc::new(MediaSource::open(std::path::Path::new(&path)).expect("open medium"));
    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount medium");

    let mut totals = Totals::default();
    let mut queue = vec![String::from("/")];
    while let Some(directory) = queue.pop() {
        let Ok(entries) = mount.list(&directory) else {
            continue;
        };
        for entry in entries {
            let child = if directory == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{directory}/{}", entry.name)
            };
            match entry.entry_type {
                EntryType::Directory => queue.push(child),
                EntryType::File => scan_file(&mount, &child, &mut totals),
                EntryType::Unknown => {}
            }
        }
    }

    println!("files_scanned={}", totals.files_scanned);
    println!("file_bytes_scanned={}", totals.file_bytes_scanned);
    println!("mscf_signature_hits={}", totals.signature_hits);
    println!("cabinets_parsed={}", totals.cabinets_parsed);
    println!("parse_failures={}", totals.parse_failures);
    println!("cabinet_folders={}", totals.cabinet_folders);
    println!("cabinet_files={}", totals.cabinet_files);
    println!("folders_extracted={}", totals.folders_extracted);
    println!("extract_failures={}", totals.extract_failures);
    println!("data_blocks_read={}", totals.blocks_read);
    println!("checksums_verified={}", totals.checksums_verified);
    println!("checksums_absent={}", totals.checksums_absent);
    println!("uncompressed_bytes={}", totals.uncompressed_bytes);
}

fn scan_file(mount: &Mount, path: &str, totals: &mut Totals) {
    let Ok(mut file) = mount.open_file(path) else {
        return;
    };
    let size = file.size();
    totals.files_scanned += 1;
    if !(36..=MAX_FILE_BYTES).contains(&size) {
        return;
    }
    let mut bytes = vec![0u8; usize::try_from(size).expect("64-bit host")];
    let mut filled = 0usize;
    while filled < bytes.len() {
        match file.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return,
        }
    }
    bytes.truncate(filled);
    totals.file_bytes_scanned += filled as u64;

    let mut candidates = Vec::new();
    for offset in 0..bytes.len().saturating_sub(4) {
        if bytes[offset..offset + 4] == *b"MSCF" {
            candidates.push(offset);
            if candidates.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    totals.signature_hits += candidates.len() as u64;

    for offset in candidates {
        let view = &bytes[offset..];
        let source = SliceSource::new(view);
        let Ok(cabinet) = Cabinet::parse(&source, 0, &Limits::default()) else {
            totals.parse_failures += 1;
            continue;
        };
        totals.cabinets_parsed += 1;
        totals.cabinet_folders += u64::from(cabinet.header().folder_count);
        totals.cabinet_files += u64::from(cabinet.header().file_count);

        for folder_index in 0..cabinet.header().folder_count {
            let Ok(segment) = FolderSegment::new(&cabinet, 0, folder_index) else {
                totals.extract_failures += 1;
                continue;
            };
            let compression = cabinet.folders()[folder_index as usize].compression;
            let Ok(mut stream) =
                FolderStream::new(&source, compression, Limits::default(), vec![segment])
            else {
                totals.extract_failures += 1;
                continue;
            };
            let mut buffer = vec![0u8; 32_768];
            let mut failed = false;
            loop {
                match stream.read(&mut buffer, &NeverCancelled) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            let stats = stream.stats();
            totals.blocks_read += stats.blocks_read;
            totals.checksums_verified += stats.checksums_verified;
            totals.checksums_absent += stats.checksums_absent;
            totals.uncompressed_bytes += stats.uncompressed_bytes;
            if failed {
                totals.extract_failures += 1;
            } else {
                totals.folders_extracted += 1;
            }
        }
    }
}
