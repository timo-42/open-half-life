//! Synthetic-media tests for the ECMA-119 preflight and archive.
//!
//! Every fixture is produced by the project-authored builder in
//! [`crate::test_support`]. No byte, name, count or listing comes from any
//! real medium.

use crate::archive::Iso9660Archive;
use crate::preflight::preflight;
use crate::test_support::{self as fixture, Options};
use ohl_core::SanitizedError;
use ohl_media_archive::block::SliceBlockReader;
use ohl_media_archive::{
    BLOCK_SIZE, BLOCK_SIZE_U64, DirectoryLimits, EntryType, FilesystemDescription, MediaArchive,
    MediaClass, MediaFileHandle,
};

fn mount(image: &[u8]) -> Result<Iso9660Archive<SliceBlockReader<'_>>, SanitizedError> {
    Iso9660Archive::open(SliceBlockReader::new(image), DirectoryLimits::default())
}

fn mount_with(
    image: &[u8],
    limits: DirectoryLimits,
) -> Result<Iso9660Archive<SliceBlockReader<'_>>, SanitizedError> {
    Iso9660Archive::open(SliceBlockReader::new(image), limits)
}

// --- preflight -------------------------------------------------------------

#[test]
fn a_joliet_volume_is_classified_and_labelled() {
    let image = fixture::make_image(Options::default());
    let result = preflight(&mut SliceBlockReader::new(&image)).expect("valid fixture");
    assert_eq!(result.media.media_class, MediaClass::Iso9660);
    assert_eq!(
        result.media.filesystem,
        FilesystemDescription::Iso9660Joliet
    );
    assert_eq!(result.media.volume_label.as_str(), fixture::VOLUME_LABEL);
    assert!(result.uses_joliet());
}

#[test]
fn a_primary_only_volume_is_classified_without_joliet() {
    let image = fixture::make_image(Options {
        joliet: false,
        ..Options::default()
    });
    let result = preflight(&mut SliceBlockReader::new(&image)).expect("valid fixture");
    assert_eq!(result.media.filesystem, FilesystemDescription::Iso9660);
    assert_eq!(result.media.volume_label.as_str(), fixture::VOLUME_LABEL);
    assert!(!result.uses_joliet());
}

#[test]
fn a_logical_block_size_other_than_2048_is_rejected() {
    let image = fixture::make_image(Options {
        logical_block_size: 512,
        ..Options::default()
    });
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_descriptor_set_without_a_terminator_is_rejected() {
    let image = fixture::make_image(Options {
        terminator: false,
        ..Options::default()
    });
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_truncated_descriptor_set_is_rejected() {
    let image = fixture::make_image(Options {
        terminator: false,
        ..Options::default()
    });
    // The source stops before the descriptor scan can complete.
    let truncated = &image[..18 * BLOCK_SIZE];
    assert_eq!(
        preflight(&mut SliceBlockReader::new(truncated)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_volume_space_size_beyond_the_source_is_rejected() {
    let image = fixture::make_image(Options {
        volume_space_too_large: true,
        ..Options::default()
    });
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_root_extent_outside_the_volume_is_rejected() {
    let image = fixture::make_image(Options {
        root_extent_outside_volume: true,
        ..Options::default()
    });
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_root_length_that_is_not_a_whole_block_is_rejected() {
    let image = fixture::make_image(Options {
        root_size_override: 100,
        ..Options::default()
    });
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::InvalidInput)
    );
}

#[test]
fn a_malformed_non_joliet_supplementary_descriptor_is_skipped() {
    let image = fixture::make_image(Options {
        joliet: false,
        malformed_non_joliet_supplementary: true,
        ..Options::default()
    });
    let result = preflight(&mut SliceBlockReader::new(&image)).expect("primary stays valid");
    assert_eq!(result.media.filesystem, FilesystemDescription::Iso9660);
    assert!(!result.uses_joliet());
}

#[test]
fn media_without_a_recognised_volume_structure_is_unsupported() {
    let mut image = fixture::make_image(Options {
        joliet: false,
        ..Options::default()
    });
    // Damage the standard identifier: the image is now neither an ECMA-167
    // nor an ECMA-119 volume.
    image[16 * BLOCK_SIZE + 3] = b'X';
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&image)),
        Err(SanitizedError::Unsupported)
    );

    let zeroes = alloc::vec![0u8; 300 * BLOCK_SIZE];
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&zeroes)),
        Err(SanitizedError::Unsupported)
    );
    assert_eq!(
        preflight(&mut SliceBlockReader::new(&[])),
        Err(SanitizedError::Unsupported)
    );
}

// --- archive ---------------------------------------------------------------

#[test]
fn the_joliet_tree_lists_reads_and_seeks() {
    let image = fixture::make_image(Options::default());
    let mut archive = mount(&image).expect("mounted");
    assert!(archive.uses_joliet());
    assert_eq!(archive.volume_label().as_str(), fixture::VOLUME_LABEL);
    assert_eq!(archive.media_class(), MediaClass::Iso9660);

    let root = archive.list("/").expect("listed the root");
    assert_eq!(root.len(), 2);
    let directory = root
        .iter()
        .find(|entry| entry.name == fixture::JOLIET_DIRECTORY_NAME)
        .expect("Joliet subdirectory entry");
    assert_eq!(directory.entry_type, EntryType::Directory);
    let sentinel = root
        .iter()
        .find(|entry| entry.name == fixture::JOLIET_SENTINEL_NAME)
        .expect("Joliet file entry");
    assert_eq!(sentinel.entry_type, EntryType::File);
    assert_eq!(sentinel.size_bytes, fixture::SENTINEL_CONTENTS.len() as u64);

    let nested_directory = alloc::format!("/{}", fixture::JOLIET_DIRECTORY_NAME);
    let nested = archive.list(&nested_directory).expect("listed the subtree");
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].name, fixture::JOLIET_NESTED_NAME);

    let nested_path = alloc::format!("{nested_directory}/{}", fixture::JOLIET_NESTED_NAME);
    let mut file = archive.open_file(&nested_path).expect("opened the file");
    assert_eq!(file.size(), fixture::NESTED_CONTENTS.len() as u64);
    let mut buffer = alloc::vec![0u8; fixture::NESTED_CONTENTS.len()];
    assert_eq!(
        archive.read_file(&mut file, &mut buffer).expect("read"),
        fixture::NESTED_CONTENTS.len()
    );
    assert_eq!(buffer, fixture::NESTED_CONTENTS.as_bytes());
    file.seek(5).expect("seek inside the file");
    assert_eq!(file.position(), 5);
    assert_eq!(
        file.seek(file.size() + 1),
        Err(SanitizedError::InvalidInput)
    );

    assert!(
        archive
            .open_file_at(&nested_directory, fixture::JOLIET_NESTED_NAME)
            .is_ok()
    );
    assert_eq!(
        archive.open_file_at(&nested_directory, "../escape"),
        Err(SanitizedError::InvalidInput)
    );
    assert_eq!(
        archive.open_file(&nested_directory),
        Err(SanitizedError::InvalidInput)
    );
    assert_eq!(
        archive.list("/does-not-exist").unwrap_err(),
        SanitizedError::NotFound
    );
    assert_eq!(
        archive.list("../escape").unwrap_err(),
        SanitizedError::InvalidInput
    );
}

#[test]
fn the_primary_tree_strips_version_suffixes_and_folds_case() {
    let image = fixture::make_image(Options {
        joliet: false,
        ..Options::default()
    });
    let mut archive = mount(&image).expect("mounted");
    assert!(!archive.uses_joliet());

    let root = archive.list("/").expect("listed the root");
    assert!(
        root.iter()
            .any(|entry| entry.name == fixture::PRIMARY_SENTINEL_NAME)
    );
    assert!(
        root.iter()
            .any(|entry| entry.name == fixture::PRIMARY_DIRECTORY_NAME)
    );
    // Path lookup folds ASCII case, which primary-tree identifiers require.
    assert!(archive.list("/fixdir").is_ok());
}

#[test]
fn structural_defects_are_rejected_at_mount_time() {
    for options in [
        Options {
            logical_block_size: 512,
            ..Options::default()
        },
        Options {
            terminator: false,
            ..Options::default()
        },
        Options {
            volume_space_too_large: true,
            ..Options::default()
        },
        Options {
            root_extent_outside_volume: true,
            ..Options::default()
        },
        Options {
            root_size_override: 100,
            ..Options::default()
        },
    ] {
        let image = fixture::make_image(options);
        assert_eq!(
            mount(&image).err(),
            Some(SanitizedError::InvalidInput),
            "a structurally invalid descriptor set must not mount"
        );
    }
}

#[test]
fn record_level_defects_are_rejected_while_listing() {
    for options in [
        Options {
            file_extent_outside_volume: true,
            ..Options::default()
        },
        Options {
            overlong_identifier: true,
            ..Options::default()
        },
        Options {
            multi_extent_file: true,
            ..Options::default()
        },
        Options {
            file_record_volume_sequence: 2,
            ..Options::default()
        },
        Options {
            child_directory_size_override: 100,
            ..Options::default()
        },
    ] {
        let image = fixture::make_image(options);
        let mut archive = mount(&image).expect("the descriptor set itself is valid");
        assert_eq!(
            archive.list("/").err(),
            Some(SanitizedError::InvalidInput),
            "a record-level defect must not be listed"
        );
    }
}

#[test]
fn a_malformed_non_joliet_supplementary_descriptor_still_mounts() {
    let image = fixture::make_image(Options {
        joliet: false,
        malformed_non_joliet_supplementary: true,
        ..Options::default()
    });
    let mut archive = mount(&image).expect("mounted");
    assert!(!archive.uses_joliet());
    assert_eq!(archive.list("/").expect("listed").len(), 2);
}

#[test]
fn joliet_siblings_differing_only_in_case_resolve_independently() {
    let image = fixture::make_image(Options {
        joliet_case_siblings: true,
        ..Options::default()
    });
    let mut archive = mount(&image).expect("mounted");
    let upper = archive.open_file("/CaseName.txt").expect("upper sibling");
    let lower = archive.open_file("/casename.txt").expect("lower sibling");
    assert_eq!(upper.size(), 8);
    assert_eq!(lower.size(), 9);
    assert_eq!(
        archive.open_file("/CASENAME.TXT").err(),
        Some(SanitizedError::NotFound),
        "Joliet lookups are not case folded"
    );
}

#[test]
fn a_directory_extent_cycle_is_rejected() {
    let image = fixture::make_image(Options {
        directory_cycle: true,
        ..Options::default()
    });
    let mut archive = mount(&image).expect("mounted");
    let looped = alloc::format!(
        "/{}/{}",
        fixture::JOLIET_DIRECTORY_NAME,
        fixture::JOLIET_LOOP_NAME
    );
    assert_eq!(
        archive.list(&looped).err(),
        Some(SanitizedError::InvalidInput)
    );
}

#[test]
fn paging_is_bounded_ordered_and_cursor_bound_to_its_mount() {
    let image = fixture::make_image(Options {
        extra_root_files: 6,
        ..Options::default()
    });
    let limits = DirectoryLimits {
        max_page_entries: 2,
        ..DirectoryLimits::default()
    };
    let mut archive = mount_with(&image, limits).expect("mounted");

    let mut page = archive.list_page("/").expect("first page");
    assert_eq!(page.entries.len(), 2);
    assert!(!page.is_complete());
    let mut total = page.entries.len();
    while let Some(cursor) = page.cursor.take() {
        page = archive.continue_list(cursor).expect("continuation page");
        total += page.entries.len();
    }
    assert_eq!(total, 8);
    assert_eq!(archive.list("/").expect("complete list").len(), 8);

    // A cursor from one mount is never accepted by an unrelated mount.
    let foreign = archive
        .list_page("/")
        .expect("page")
        .cursor
        .expect("cursor");
    let mut other = mount_with(&image, limits).expect("second mount");
    assert_eq!(
        other.continue_list(foreign).err(),
        Some(SanitizedError::InvalidInput)
    );
}

#[test]
fn invalid_limits_are_rejected_before_any_read() {
    let image = fixture::make_image(Options::default());
    let limits = DirectoryLimits {
        max_page_entries: 0,
        ..DirectoryLimits::default()
    };
    assert_eq!(
        mount_with(&image, limits).err(),
        Some(SanitizedError::InvalidInput)
    );
}

// --- fuzz-style robustness -------------------------------------------------

proptest::proptest! {
    #[test]
    fn decoding_arbitrary_bytes_never_panics(
        bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..40_000)
    ) {
        let _ = preflight(&mut SliceBlockReader::new(&bytes));
    }

    #[test]
    fn mutating_a_valid_image_never_panics(
        offset in 0usize..300 * BLOCK_SIZE,
        value in proptest::num::u8::ANY,
    ) {
        let mut image = fixture::make_image(Options::default());
        image[offset] = value;
        if let Ok(mut archive) = mount(&image) {
            let _ = archive.list("/");
            let _ = archive.open_file("/anything");
        }
    }
}

// --- manual, owned-media check --------------------------------------------

/// Reads the medium named by `OHL_TEST_ISO` and prints only aggregates.
///
/// This test is `#[ignore]`d: it needs a lawfully owned medium the repository
/// never contains. It deliberately prints no name, path, or content — only
/// the media class, the sanitized label's length, and entry counts — so its
/// output can be pasted into a review without disclosing anything about the
/// medium.
#[test]
#[ignore = "requires OHL_TEST_ISO to name a lawfully owned medium"]
fn manual_owned_media_aggregates() {
    use std::io::{Read as _, Seek as _};

    struct FileBlocks {
        file: std::fs::File,
        blocks: u64,
    }

    impl ohl_media_archive::BlockReader for FileBlocks {
        type Error = SanitizedError;

        fn read_block(
            &mut self,
            lba: u64,
            out: &mut ohl_media_archive::Block,
        ) -> Result<(), Self::Error> {
            if lba >= self.blocks {
                return Err(SanitizedError::InvalidInput);
            }
            self.file
                .seek(std::io::SeekFrom::Start(lba * BLOCK_SIZE_U64))
                .map_err(|_| SanitizedError::Internal)?;
            self.file
                .read_exact(out)
                .map_err(|_| SanitizedError::Internal)
        }

        fn block_count(&self) -> u64 {
            self.blocks
        }
    }

    let Ok(path) = std::env::var("OHL_TEST_ISO") else {
        println!("OHL_TEST_ISO is unset; nothing to check");
        return;
    };
    let file = std::fs::File::open(path).expect("the named medium is readable");
    let blocks = file.metadata().expect("metadata").len() / BLOCK_SIZE_U64;
    let mut archive = Iso9660Archive::open(FileBlocks { file, blocks }, DirectoryLimits::default())
        .expect("the medium mounts");

    let root = archive.list("/").expect("the root lists");
    let mut directories = 0usize;
    let mut files = 0usize;
    let mut nested_entries = 0usize;
    for entry in &root {
        if entry.entry_type == EntryType::Directory {
            directories += 1;
            let path = alloc::format!("/{}", entry.name);
            nested_entries += archive.list(&path).map_or(0, |entries| entries.len());
        } else {
            files += 1;
        }
    }

    println!("media class: {}", archive.media_class());
    println!("filesystem: {}", archive.filesystem());
    println!("volume label length: {}", archive.volume_label().len());
    println!("source blocks: {blocks}");
    println!("root entries: {}", root.len());
    println!("root directories: {directories}");
    println!("root files: {files}");
    println!("entries one level below the root: {nested_entries}");
}
