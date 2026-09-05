//! Manual, opt-in survey of a locally owned disc image.
//!
//! This test is `#[ignore]`d and never runs in CI. It runs only when
//! `OHL_TEST_ISO` names a local image the operator already owns. Per
//! `docs/CLEAN_ROOM.md` it prints **only** bounded aggregates — counts, byte
//! totals and fixed class names — and never a name, path, label or byte of
//! the media. It never executes anything found on the image.

use std::sync::Arc;

use ohl_isz::{Archive, ArchiveSource, Limits, NeverCancelled, SourceError, find_signature_from};
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, EntryType, Mount};

/// Only files at least this large can plausibly hold the payload archive.
const MIN_CANDIDATE_BYTES: u64 = 64 * 1024;
/// Ceilings on the directory walk, so a hostile image cannot hang the test.
const MAX_VISITED: usize = 200_000;
const MAX_DEPTH: usize = 32;

/// The fixed extension classes reported. No media-derived name is ever
/// printed; only these project-authored class names are.
const CLASSES: [&str; 10] = [
    "bsp", "wad", "mdl", "spr", "wav", "txt", "cfg", "dll", "exe", "other",
];

fn class_index(extension: &[u8]) -> usize {
    match extension {
        b"bsp" => 0,
        b"wad" => 1,
        b"mdl" => 2,
        b"spr" => 3,
        b"wav" => 4,
        b"txt" => 5,
        b"cfg" => 6,
        b"dll" => 7,
        b"exe" => 8,
        _ => 9,
    }
}

/// An [`ArchiveSource`] over a file opened inside a mount.
struct MediaFileSource {
    file: ohl_vfs::MediaFile,
}

impl ArchiveSource for MediaFileSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, SourceError> {
        if offset >= self.file.size() {
            return Ok(0);
        }
        self.file.seek(offset).map_err(|_| SourceError)?;
        self.file.read(buf).map_err(|_| SourceError)
    }
}

/// A sink that counts bytes and records the first four bytes it is given.
#[derive(Default)]
struct CountingSink {
    bytes: u64,
    magic: [u8; 4],
    magic_len: usize,
}

impl CountingSink {
    fn write(&mut self, chunk: &[u8]) {
        self.bytes += chunk.len() as u64;
        if self.magic_len < 4 {
            let take = (4 - self.magic_len).min(chunk.len());
            self.magic[self.magic_len..self.magic_len + take].copy_from_slice(&chunk[..take]);
            self.magic_len += take;
        }
    }

    /// Whether the recorded leading bytes match one of the GoldSrc magics the
    /// engine cares about: BSP v30, WAD3, MDL (`IDST`), SPR (`IDSP`), RIFF.
    fn matches_known_magic(&self) -> bool {
        if self.magic_len < 4 {
            return false;
        }
        let magic = self.magic;
        magic == [30, 0, 0, 0]
            || &magic == b"WAD3"
            || &magic == b"IDST"
            || &magic == b"IDSP"
            || &magic == b"RIFF"
    }
}

/// Collects every file path on the mount that is large enough to be a
/// candidate, breadth-first and bounded.
fn candidate_paths(mount: &Mount) -> Vec<String> {
    let mut queue = vec![(String::from("/"), 0usize)];
    let mut candidates = Vec::new();
    let mut visited = 0usize;

    while let Some((directory, depth)) = queue.pop() {
        if depth > MAX_DEPTH || visited > MAX_VISITED {
            break;
        }
        let Ok(entries) = mount.list(&directory) else {
            continue;
        };
        for entry in entries {
            visited += 1;
            if visited > MAX_VISITED {
                break;
            }
            let child = if directory == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{directory}/{}", entry.name)
            };
            match entry.entry_type {
                EntryType::Directory => queue.push((child, depth + 1)),
                EntryType::File => {
                    if entry.size_bytes >= MIN_CANDIDATE_BYTES {
                        candidates.push(child);
                    }
                }
                EntryType::Unknown => {}
            }
        }
    }
    // Largest first is not knowable without another stat, so keep discovery
    // order; the scan below stops at the first archive it can parse.
    candidates
}

#[test]
#[ignore = "requires a locally owned disc image named by OHL_TEST_ISO"]
fn manual_iso_installshield_z_survey() {
    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        println!("OHL_TEST_ISO is not set; skipping");
        return;
    };
    let source = Arc::new(
        MediaSource::open(std::path::Path::new(&path))
            .expect("open the image named by OHL_TEST_ISO"),
    );
    let mount = Mount::open(source, DirectoryLimits::default()).expect("the image mounts");

    let candidates = candidate_paths(&mount);
    println!("iso: candidate files >= 64 KiB = {}", candidates.len());

    let limits = Limits {
        max_scan_bytes: 8 * 1024 * 1024 * 1024,
        max_archive_bytes: 4 * 1024 * 1024 * 1024,
        max_directory_bytes: 32 * 1024 * 1024,
        max_chunk_bytes: 256 * 1024,
        ..Limits::default()
    };

    let mut scanned = 0usize;
    let mut scanned_bytes = 0u64;
    let mut signature_hits = 0usize;
    let mut first_word_hits = 0usize;
    let mut surveyed = false;

    for candidate in &candidates {
        let Ok(file) = mount.open_file(candidate) else {
            continue;
        };
        scanned += 1;
        scanned_bytes += file.size();
        let mut reader = MediaFileSource { file };
        first_word_hits += count_first_signature_word(&mut reader);
        let Ok(Some(base)) = find_signature_from(&mut reader, 0, &limits, &NeverCancelled) else {
            continue;
        };
        signature_hits += 1;
        let Ok(archive) = Archive::open(&mut reader, base, &limits, &NeverCancelled) else {
            continue;
        };
        survey(&mount, candidate, archive, &limits);
        surveyed = true;
        break;
    }

    println!("iso: files scanned = {scanned}");
    println!("iso: bytes scanned = {scanned_bytes}");
    println!("iso: occurrences of signature word 1 alone = {first_word_hits}");
    println!("iso: files carrying the full 8-byte signature = {signature_hits}");
    println!("iso: archive surveyed = {surveyed}");
    // The survey is a report, not a compatibility claim: an image that
    // carries no InstallShield 3 archive is a valid outcome and is reported
    // as such rather than failing. What must hold is that the mount, the walk
    // and the bounded scan all completed.
    assert!(scanned > 0, "no candidate file could be opened");
    if !surveyed {
        println!("archive: entry count = 0");
        println!("archive: directory count = 0");
        println!("archive: total stored bytes = 0");
        println!("archive: total extracted bytes = 0");
        println!("archive: extracted entries matching a known magic = 0");
        for class in CLASSES {
            println!("archive: class {class}: entries = 0, extracted bytes = 0");
        }
    }
}

/// Counts occurrences of the archive signature's first word on its own, in
/// bounded windows. A lone first word without the second is a coincidental
/// byte pattern, not an archive; reporting it separates "no archive" from
/// "archive the parser rejected".
fn count_first_signature_word(source: &mut MediaFileSource) -> usize {
    const WINDOW: usize = 256 * 1024;
    let needle = &ohl_isz::SIGNATURE[..4];
    let mut buffer = vec![0u8; WINDOW];
    let mut position = 0u64;
    let mut hits = 0usize;
    loop {
        let mut filled = 0usize;
        while filled < WINDOW {
            match source.read_at(position + filled as u64, &mut buffer[filled..]) {
                Ok(0) | Err(_) => break,
                Ok(read) => filled += read,
            }
        }
        if filled < needle.len() {
            return hits;
        }
        hits += buffer[..filled]
            .windows(needle.len())
            .filter(|candidate| *candidate == needle)
            .count();
        if filled < WINDOW {
            return hits;
        }
        position += (filled - (needle.len() - 1)) as u64;
    }
}

fn survey(mount: &Mount, path: &str, mut archive: Archive, limits: &Limits) {
    let entry_count = archive.entries().len();
    let directory_count = archive.directories().len();
    let mut stored_total = 0u64;
    let mut extracted_total = 0u64;
    let mut failures = 0usize;
    let mut magic_hits = 0usize;
    let mut class_counts = [0usize; CLASSES.len()];
    let mut class_bytes = [0u64; CLASSES.len()];

    let mut buffer = vec![0u8; limits.max_chunk_bytes];
    for index in 0..u32::try_from(entry_count).unwrap_or(0) {
        let class = {
            let entry = archive.entry(index).expect("index came from the listing");
            stored_total += u64::from(entry.stored_size);
            class_index(&entry.name.extension_bytes())
        };
        class_counts[class] += 1;

        let Ok(file) = mount.open_file(path) else {
            failures += 1;
            continue;
        };
        let mut source = MediaFileSource { file };
        let Ok(mut reader) = archive.open_entry(index) else {
            failures += 1;
            continue;
        };
        let mut sink = CountingSink::default();
        let mut failed = false;
        loop {
            match reader.read(&mut source, &NeverCancelled, &mut buffer) {
                Ok(0) => break,
                Ok(read) => sink.write(&buffer[..read]),
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            failures += 1;
            continue;
        }
        extracted_total += sink.bytes;
        class_bytes[class] += sink.bytes;
        if sink.matches_known_magic() {
            magic_hits += 1;
        }
    }

    println!("archive: entry count = {entry_count}");
    println!("archive: directory count = {directory_count}");
    println!("archive: total stored bytes = {stored_total}");
    println!("archive: total extracted bytes = {extracted_total}");
    println!("archive: entries that failed to extract = {failures}");
    println!("archive: extracted entries matching a known magic = {magic_hits}");
    for (index, class) in CLASSES.iter().enumerate() {
        println!(
            "archive: class {class}: entries = {}, extracted bytes = {}",
            class_counts[index], class_bytes[index]
        );
    }
}
