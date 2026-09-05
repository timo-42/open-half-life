//! Integration tests: mounting the synthetic ISO 9660/Joliet fixture through
//! a real `ohl_platform::MediaSource` over a temp file.
//!
//! Every image byte comes from `ohl_iso9660::test_support`'s project-authored
//! builder (exposed here through that crate's `test-support` feature), which
//! is itself a direct port of the C++ synthetic-media builder. No byte, name,
//! count or listing comes from any real medium.

use std::io::Write as _;
use std::sync::Arc;

use ohl_iso9660::test_support::{self as fixture, Options};
use ohl_media_archive::{DirectoryLimits, EntryType, FilesystemDescription, MediaClass};
use ohl_platform::MediaSource;
use ohl_vfs::Mount;

/// Writes `image` to a fresh temp file and opens it through a real, pinned
/// `MediaSource`, matching how the rest of the engine acquires media.
fn media_source(image: &[u8]) -> (tempfile::NamedTempFile, Arc<MediaSource>) {
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    file.write_all(image).expect("write synthetic image");
    file.flush().expect("flush synthetic image");
    let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));
    (file, source)
}

#[test]
fn mounts_the_joliet_fixture_and_classifies_it() {
    let image = fixture::make_image(Options::default());
    let (_file, source) = media_source(&image);

    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount succeeds");
    assert_eq!(mount.class(), MediaClass::Iso9660);
    assert_eq!(mount.filesystem(), FilesystemDescription::Iso9660Joliet);
    assert_eq!(mount.volume_label().as_str(), fixture::VOLUME_LABEL);
}

#[test]
fn mounts_the_primary_only_fixture_without_joliet() {
    let image = fixture::make_image(Options {
        joliet: false,
        ..Options::default()
    });
    let (_file, source) = media_source(&image);

    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount succeeds");
    assert_eq!(mount.filesystem(), FilesystemDescription::Iso9660);
}

#[test]
fn lists_reads_and_seeks_through_the_uniform_api() {
    let image = fixture::make_image(Options::default());
    let (_file, source) = media_source(&image);
    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount succeeds");

    let root = mount.list("/").expect("listed the root");
    assert_eq!(root.len(), 2);
    let directory = root
        .iter()
        .find(|entry| entry.name == fixture::JOLIET_DIRECTORY_NAME)
        .expect("Joliet subdirectory entry");
    assert_eq!(directory.entry_type, EntryType::Directory);

    let nested_directory = format!("/{}", fixture::JOLIET_DIRECTORY_NAME);
    let nested_path = format!("{nested_directory}/{}", fixture::JOLIET_NESTED_NAME);
    let mut opened = mount.open_file(&nested_path).expect("opened the file");
    assert_eq!(opened.size(), fixture::NESTED_CONTENTS.len() as u64);

    let mut buffer = vec![0u8; fixture::NESTED_CONTENTS.len()];
    assert_eq!(
        opened.read(&mut buffer).expect("read"),
        fixture::NESTED_CONTENTS.len()
    );
    assert_eq!(buffer, fixture::NESTED_CONTENTS.as_bytes());

    opened.seek(5).expect("seek inside the file");
    assert_eq!(opened.position(), 5);
    assert_eq!(
        opened.seek(opened.size() + 1),
        Err(ohl_core::SanitizedError::InvalidInput)
    );
}

#[test]
fn rejects_paths_the_shared_normalizer_rejects() {
    let image = fixture::make_image(Options::default());
    let (_file, source) = media_source(&image);
    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount succeeds");

    assert_eq!(
        mount.list("../escape"),
        Err(ohl_core::SanitizedError::InvalidInput)
    );
    assert_eq!(
        mount.open_file("fixture-directory/./fixture-file").err(),
        Some(ohl_core::SanitizedError::InvalidInput)
    );
    assert_eq!(
        mount.list("/does-not-exist"),
        Err(ohl_core::SanitizedError::NotFound)
    );
}

#[test]
fn pagination_continues_and_rejects_a_foreign_cursor() {
    let image = fixture::make_image(Options {
        extra_root_files: 6,
        ..Options::default()
    });
    let (_file_a, source_a) = media_source(&image);
    let (_file_b, source_b) = media_source(&image);

    let limits = DirectoryLimits {
        max_page_entries: 2,
        ..DirectoryLimits::default()
    };
    let mount = Mount::open_as(MediaClass::Iso9660, source_a, limits).expect("first mount");
    let other = Mount::open_as(MediaClass::Iso9660, source_b, limits).expect("second mount");

    let mut page = mount.list_page("/").expect("first page");
    assert_eq!(page.entries.len(), 2);
    assert!(!page.is_complete());
    let mut total = page.entries.len();
    while let Some(cursor) = page.cursor.take() {
        page = mount.continue_list(cursor).expect("continuation page");
        total += page.entries.len();
    }
    assert_eq!(total, 8);
    assert_eq!(mount.list("/").expect("complete listing").len(), 8);

    let foreign = mount
        .list_page("/")
        .expect("page")
        .cursor
        .expect("cursor available");
    assert_eq!(
        other.continue_list(foreign),
        Err(ohl_core::SanitizedError::InvalidInput)
    );
}

#[test]
fn a_shared_mount_keeps_cursors_and_open_files_valid() {
    let image = fixture::make_image(Options {
        extra_root_files: 6,
        ..Options::default()
    });
    let (_file, source) = media_source(&image);
    let limits = DirectoryLimits {
        max_page_entries: 2,
        ..DirectoryLimits::default()
    };
    let mount = Mount::open_as(MediaClass::Iso9660, source, limits).expect("mounted");
    let share = mount.share();
    assert_eq!(share.class(), mount.class());
    assert_eq!(share.volume_label().as_str(), mount.volume_label().as_str());

    // A cursor produced by the original handle continues through its share.
    let page = mount.list_page("/").expect("first page");
    let cursor = page.cursor.expect("cursor available");
    assert!(share.continue_list(cursor).is_ok());

    // A file opened through the share is read through the same shared
    // archive state; dropping the `Mount` that opened it does not matter.
    let nested_directory = format!("/{}", fixture::JOLIET_DIRECTORY_NAME);
    let nested_path = format!("{nested_directory}/{}", fixture::JOLIET_NESTED_NAME);
    let mut opened = share
        .open_file(&nested_path)
        .expect("opened through the share");
    drop(share);
    let mut buffer = vec![0u8; fixture::NESTED_CONTENTS.len()];
    assert_eq!(
        opened
            .read(&mut buffer)
            .expect("read after dropping the share"),
        fixture::NESTED_CONTENTS.len()
    );
}

#[test]
fn truncation_mid_mount_is_detected_via_verify_unchanged() {
    let image = fixture::make_image(Options::default());
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    file.write_all(&image).expect("write synthetic image");
    file.flush().expect("flush synthetic image");
    let source = Arc::new(MediaSource::open(file.path()).expect("open pinned source"));

    // A verify interval of one block means every fresh block read
    // re-verifies. Truncate to a length that still comfortably contains every
    // sector the root listing touches (the root and child directory extents
    // sit well under sector 32), so the physical read below would otherwise
    // succeed; only the size mismatch `verify_unchanged` compares against the
    // acquisition snapshot can catch this truncation.
    let mount = Mount::open_with_verify_interval(source, DirectoryLimits::default(), 1)
        .expect("mount succeeds");

    let truncated_sectors = (fixture::SECTOR_COUNT / 2) as u64;
    assert!(truncated_sectors * 2_048 > u64::from(fixture::PRIMARY_CHILD_SECTOR) * 2_048);
    file.as_file()
        .set_len(truncated_sectors * 2_048)
        .expect("truncate the pinned object mid-mount");

    assert_eq!(
        mount.list("/").err(),
        Some(ohl_core::SanitizedError::InvalidInput)
    );
}

#[test]
fn udf_preflight_rejects_media_with_no_recognisable_structure() {
    // No mountable synthetic UDF fixture exists yet, so this covers the
    // rejection path: a bounded, all-zero image carries neither an ECMA-119
    // primary volume descriptor nor an ECMA-167 recognition sequence.
    let image = vec![0u8; 300 * 2_048];
    let (_file, source) = media_source(&image);

    let result = Mount::open(source, DirectoryLimits::default());
    assert_eq!(result.err(), Some(ohl_core::SanitizedError::Unsupported));
}

#[test]
#[ignore = "manual: point OHL_TEST_ISO at a real disc image to smoke-test Mount over it"]
fn manual_mount_aggregates_over_a_real_iso() {
    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        eprintln!("OHL_TEST_ISO not set; skipping manual mount smoke test");
        return;
    };

    let source =
        Arc::new(MediaSource::open(std::path::Path::new(&path)).expect("open pinned source"));
    let mount = Mount::open(source, DirectoryLimits::default()).expect("mount succeeds");
    let root = mount.list("/").expect("listed the root");

    // Aggregates only: no media-derived name, path, or byte is printed.
    println!("class={:?}", mount.class());
    println!("filesystem={:?}", mount.filesystem());
    println!("volume_label_len={}", mount.volume_label().len());
    println!("root_entry_count={}", root.len());
}
