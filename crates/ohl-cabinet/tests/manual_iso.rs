//! Manual, opt-in check against a locally owned ISO.
//!
//! This test is `#[ignore]`d and never runs in CI. It runs only when
//! `OHL_TEST_ISO` names a local image the operator already owns. Per
//! `docs/CLEAN_ROOM.md` and `docs/MEDIA_IMPORT.md` it prints **only** bounded
//! aggregates — counts, sizes and a success flag — and never a name, path,
//! label or byte of the media. It never executes anything found on the image.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;

use hadris_iso::IsoImage;
use hadris_iso::directory::DirectoryRef;
use ohl_cabinet_format::{CAB_SIGNATURE, CabinetHeader, Limits as FormatLimits};

/// Ceilings on the directory walk, so a hostile image cannot hang the test.
const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 32;

/// Extensions searched for, case-insensitively.
const HEADER_EXTENSION: &str = ".hdr";
const CABINET_EXTENSION: &str = ".cab";
/// The conventional first header name, published in InstallShield's own
/// product documentation.
const CONVENTIONAL_HEADER_NAME: &str = "data1.hdr";

fn normalise(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.split(';').next().unwrap_or("").trim_end_matches('.');
    text.to_ascii_lowercase()
}

#[test]
#[ignore = "requires a locally owned ISO named by OHL_TEST_ISO"]
fn manual_iso_header_survey() {
    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        println!("OHL_TEST_ISO is not set; skipping");
        return;
    };
    let path = PathBuf::from(path);
    let file = File::open(&path).expect("open the image named by OHL_TEST_ISO");
    let image = IsoImage::open(file).expect("parse the image as ISO 9660");

    let mut stack: Vec<(DirectoryRef, usize)> = vec![(image.root_dir().dir_ref(), 0)];
    let mut visited = 0usize;
    // (entry, rank) where a lower rank is preferred: the conventional header
    // name first, then any header, then a volume (which carries the header
    // when no separate header file is present).
    let mut header_candidates: Vec<(hadris_iso::read::DirEntry, u8)> = Vec::new();
    let mut cabinet_count = 0usize;
    let mut cabinet_bytes = 0u64;
    let mut header_file_count = 0usize;
    let mut signature_hits = 0usize;
    // A second handle used only to sample each file's first four bytes, so
    // the survey does not depend on extensions alone.
    let mut sampler = File::open(&path).expect("open the image named by OHL_TEST_ISO");

    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_DEPTH || visited > MAX_ENTRIES {
            break;
        }
        for entry in image.open_dir(directory).entries() {
            let Ok(entry) = entry else { break };
            visited += 1;
            if visited > MAX_ENTRIES {
                break;
            }
            if entry.record.is_special() {
                continue;
            }
            if entry.record.is_directory() {
                if let Ok(child) = entry.as_dir_ref(&image) {
                    stack.push((child, depth + 1));
                }
                continue;
            }
            let carries_signature = {
                let offset = (u64::from(entry.record.header().extent.read())
                    + u64::from(entry.record.header().extended_attr_record))
                    * 2048;
                let mut magic = [0u8; 4];
                sampler.seek(SeekFrom::Start(offset)).is_ok()
                    && sampler.read_exact(&mut magic).is_ok()
                    && u32::from_le_bytes(magic) == CAB_SIGNATURE
            };
            if carries_signature {
                signature_hits += 1;
                header_candidates.push((entry.clone(), 0));
            }

            let name = normalise(entry.name());
            if name.ends_with(HEADER_EXTENSION) {
                header_file_count += 1;
                let rank = if name == CONVENTIONAL_HEADER_NAME {
                    1
                } else {
                    2
                };
                header_candidates.push((entry, rank));
            } else if name.ends_with(CABINET_EXTENSION) {
                cabinet_count += 1;
                cabinet_bytes += entry.total_size();
                header_candidates.push((entry, 3));
            }
        }
    }

    println!("iso: directory entries visited = {visited}");
    println!("iso: cabinet volumes found = {cabinet_count}");
    println!("iso: cabinet volume bytes = {cabinet_bytes}");
    println!("iso: header files found = {header_file_count}");
    println!("iso: files carrying the InstallShield signature = {signature_hits}");
    println!("iso: header candidates found = {}", header_candidates.len());

    assert!(
        !header_candidates.is_empty(),
        "no cabinet or header file was found on the image"
    );

    // Prefer the conventional first header, then any header file, then a
    // volume, which carries the header when no header file exists.
    header_candidates.sort_by_key(|(_, rank)| *rank);

    let limits = FormatLimits::default();
    let mut surveyed = false;
    let mut rejected = 0usize;

    for (entry, _) in &header_candidates {
        let Ok(bytes) = image.read_file(entry) else {
            rejected += 1;
            continue;
        };
        println!("candidate: bytes = {}", bytes.len());
        let header = match CabinetHeader::parse(&bytes, &limits) {
            Ok(header) => header,
            Err(error) => {
                // Sanitized, project-defined error text only.
                println!("candidate: not an InstallShield cabinet ({error})");
                rejected += 1;
                continue;
            }
        };
        survey(&header);
        surveyed = true;
        break;
    }

    println!("iso: candidates rejected = {rejected}");
    println!("iso: installshield cabinet surveyed = {surveyed}");
}

fn survey(header: &CabinetHeader<'_>) {
    let mut compressed = 0usize;
    let mut obfuscated = 0usize;
    let mut split = 0usize;
    let mut invalid = 0usize;
    let mut parsed = 0usize;
    let mut expanded_total = 0u64;
    let mut compressed_total = 0u64;
    let mut max_volume = 0u16;
    let mut named = 0usize;

    for descriptor in header.file_descriptors() {
        let Ok(descriptor) = descriptor else { continue };
        parsed += 1;
        expanded_total = expanded_total.saturating_add(descriptor.expanded_size);
        compressed_total = compressed_total.saturating_add(descriptor.compressed_size);
        max_volume = max_volume.max(descriptor.volume);
        if descriptor.flags.is_compressed() {
            compressed += 1;
        }
        if descriptor.flags.is_obfuscated() {
            obfuscated += 1;
        }
        if descriptor.flags.is_split() {
            split += 1;
        }
        if descriptor.flags.is_invalid() {
            invalid += 1;
        }
    }
    for index in 0..header.file_count() {
        if header.file_name(index).is_ok() {
            named += 1;
        }
    }
    let mut directories_named = 0usize;
    for name in header.directories() {
        if name.is_ok() {
            directories_named += 1;
        }
    }

    println!("cabinet: major version = {}", header.version().major());
    println!("cabinet: unicode strings = {}", header.is_unicode());
    println!("cabinet: directory count = {}", header.directory_count());
    println!("cabinet: directories decoded = {directories_named}");
    println!("cabinet: file count = {}", header.file_count());
    println!("cabinet: file descriptors parsed = {parsed}");
    println!("cabinet: file names decoded = {named}");
    println!("cabinet: file groups = {}", header.file_groups().len());
    println!("cabinet: components = {}", header.components().len());
    println!("cabinet: compressed files = {compressed}");
    println!("cabinet: obfuscated files = {obfuscated}");
    println!("cabinet: split files = {split}");
    println!("cabinet: invalid files = {invalid}");
    println!("cabinet: expanded bytes total = {expanded_total}");
    println!("cabinet: stored bytes total = {compressed_total}");
    println!("cabinet: highest volume number = {max_volume}");
}
