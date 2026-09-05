//! Manual, opt-in survey of a locally owned image.
//!
//! This test is `#[ignore]`d and never runs in CI. It runs only when
//! `OHL_TEST_ISO` names a local image the operator already owns. Per
//! `docs/CLEAN_ROOM.md` and `docs/MEDIA_IMPORT.md` it prints **only** bounded
//! aggregates — counts, byte totals and fixed classification words — and
//! never a name, path, label or byte of the media. It never executes
//! anything found on the image.

use std::sync::Arc;

use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, EntryType, MediaFile, Mount};
use ohl_wise::{ChecksumStatus, Discard, Error, ImageSource, Limits, NeverCancelled, read_package};

/// Ceilings on the directory walk, so a hostile image cannot hang the test.
const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 32;

/// The fixed classification vocabulary this survey may print. Every word is
/// a documented, public GoldSrc or platform container extension; nothing is
/// derived from the medium.
const CLASSES: &[&str] = &[
    "bsp", "wad", "mdl", "spr", "wav", "txt", "cfg", "dll", "exe", "other",
];

fn class_index(extension: &[u8]) -> usize {
    CLASSES
        .iter()
        .position(|class| class.as_bytes() == extension)
        .unwrap_or(CLASSES.len() - 1)
}

/// An `ImageSource` over one mounted file.
struct MediaFileSource {
    file: MediaFile,
    len: u64,
}

impl ImageSource for MediaFileSource {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Error> {
        if offset >= self.len {
            return Ok(0);
        }
        self.file.seek(offset).map_err(|_| Error::SourceFailed)?;
        let mut filled = 0;
        while filled < buf.len() {
            let read = self
                .file
                .read(&mut buf[filled..])
                .map_err(|_| Error::SourceFailed)?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        Ok(filled)
    }

    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.len)
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one linear survey with fixed output")]
#[ignore = "requires a locally owned image named by OHL_TEST_ISO"]
fn manual_iso_wise_survey() {
    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        println!("OHL_TEST_ISO is not set; skipping");
        return;
    };
    let source = Arc::new(
        MediaSource::open(std::path::Path::new(&path))
            .expect("open the image named by OHL_TEST_ISO"),
    );
    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount the image");

    // Find the largest file that begins with the `MZ` signature, without
    // recording, printing or reusing its name outside this scope.
    let mut stack = vec![(String::from("/"), 0usize)];
    let mut visited = 0usize;
    let mut best: Option<(String, u64)> = None;
    while let Some((directory, depth)) = stack.pop() {
        if depth > MAX_DEPTH || visited > MAX_ENTRIES {
            break;
        }
        let Ok(entries) = mount.list(&directory) else {
            continue;
        };
        for entry in entries {
            visited += 1;
            let child = if directory == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{directory}/{}", entry.name)
            };
            match entry.entry_type {
                EntryType::Directory => stack.push((child, depth + 1)),
                EntryType::File => {
                    if best
                        .as_ref()
                        .is_some_and(|(_, size)| *size >= entry.size_bytes)
                    {
                        continue;
                    }
                    let Ok(mut file) = mount.open_file(&child) else {
                        continue;
                    };
                    let mut magic = [0u8; 2];
                    if file.read(&mut magic).is_ok() && magic == *b"MZ" {
                        best = Some((child, entry.size_bytes));
                    }
                }
                EntryType::Unknown => {}
            }
        }
    }

    println!("survey: directory entries visited = {visited}");
    let Some((path, size)) = best else {
        println!("survey: no executable image found");
        return;
    };
    println!("survey: largest executable bytes = {size}");

    let file = mount.open_file(&path).expect("open the executable");
    let len = file.size();
    let mut image = MediaFileSource { file, len };

    let package = match read_package(&mut image, None, Limits::DEFAULT, &NeverCancelled) {
        Ok(package) => package,
        Err(error) => {
            println!("survey: package not readable ({error})");
            return;
        }
    };

    let summary = package.summary();
    println!("survey: streams = {}", summary.streams);
    println!("survey: crc matches = {}", summary.crc_matches);
    println!("survey: crc mismatches = {}", summary.crc_mismatches);
    println!("survey: crc absent = {}", summary.crc_absent);
    println!("survey: resyncs = {}", summary.resyncs);
    println!(
        "survey: total inflated bytes = {}",
        summary.total_inflated_bytes
    );
    println!("survey: overlay bytes covered = {}", summary.covered_bytes);
    println!("survey: header bytes = {}", package.header().header_len);
    println!("survey: file records = {}", package.file_table().len());
    println!(
        "survey: records resolved to a stream = {}",
        package.file_map().mapped_count()
    );
    println!(
        "survey: records resolved by content = {}",
        package.file_map().content_matched_count()
    );
    println!(
        "survey: unnamed streams = {}",
        package.file_map().unnamed_streams().len()
    );
    println!(
        "survey: records with plausible stored offsets = {}",
        package.file_table().plausible_offset_count()
    );

    let mut counts = vec![0usize; CLASSES.len()];
    let mut bytes = vec![0u64; CLASSES.len()];
    for (index, record) in package.file_table().records().iter().enumerate() {
        let class = record
            .path
            .extension()
            .map_or(CLASSES.len() - 1, |extension| class_index(&extension));
        counts[class] += 1;
        // Only measured sizes are totalled; a declared size is not evidence.
        bytes[class] += package.file_map().list()[index]
            .stream_inflated_size
            .unwrap_or(0);
    }
    for (index, class) in CLASSES.iter().enumerate() {
        println!(
            "survey: class {class} count = {} bytes = {}",
            counts[index], bytes[index]
        );
    }

    // Verify one mapped file end to end, printing only whether it verified.
    let verified = package
        .file_map()
        .list()
        .iter()
        .position(|entry| entry.stream_index.is_some())
        .is_some_and(|index| {
            package.open_file(index).is_ok_and(|mut reader| {
                reader
                    .read_all(&mut image, &mut Discard, &NeverCancelled, Limits::DEFAULT)
                    .is_ok()
            })
        });
    println!("survey: sample file verified = {verified}");
    assert!(
        summary.streams > 0,
        "the chain walk must observe at least one stream"
    );
    let _ = ChecksumStatus::Match;
}
