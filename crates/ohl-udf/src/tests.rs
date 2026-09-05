//! Synthetic-media tests for the ECMA-167 preflight and archive.
//!
//! Every fixture is produced by the project-authored builder in
//! [`crate::test_support`]. No byte, name, count or listing comes from any
//! real medium.

use crate::archive::UdfArchive;
use crate::preflight::preflight;
use crate::test_support::{self as fixture, Options};
use ohl_core::SanitizedError;
use ohl_media_archive::block::SliceBlockReader;
use ohl_media_archive::{BLOCK_SIZE, DirectoryLimits, FilesystemDescription, MediaClass};

fn run(image: &[u8]) -> Result<ohl_media_archive::MediaPreflight, SanitizedError> {
    preflight(&mut SliceBlockReader::new(image))
}

// --- preflight -------------------------------------------------------------

#[test]
fn a_valid_nsr02_volume_is_classified_and_labelled() {
    let image = fixture::valid_image();
    let result = run(&image).expect("valid fixture");
    assert_eq!(result.media_class, MediaClass::Udf);
    assert_eq!(result.filesystem, FilesystemDescription::Ecma167Nsr02);
    assert_eq!(result.volume_label.as_str(), fixture::VOLUME_LABEL);
}

#[test]
fn media_without_a_recognition_sequence_is_unsupported() {
    for options in [
        Options {
            no_recognition_sequence: true,
            ..Options::default()
        },
        Options {
            wrong_nsr_identifier: true,
            ..Options::default()
        },
    ] {
        let image = fixture::make_image(fixture::SECTOR_COUNT, options);
        assert_eq!(run(&image), Err(SanitizedError::Unsupported));
    }

    let zeroes = alloc::vec![0u8; fixture::SECTOR_COUNT * BLOCK_SIZE];
    assert_eq!(run(&zeroes), Err(SanitizedError::Unsupported));
}

#[test]
fn media_too_small_to_hold_an_anchor_is_unsupported() {
    let small = alloc::vec![0u8; 64 * BLOCK_SIZE];
    assert_eq!(run(&small), Err(SanitizedError::Unsupported));
    assert_eq!(run(&[]), Err(SanitizedError::Unsupported));
}

#[test]
fn malformed_anchors_and_descriptors_are_rejected() {
    for options in [
        Options {
            corrupt_anchor_checksum: true,
            ..Options::default()
        },
        Options {
            zero_anchor_crc_length: true,
            ..Options::default()
        },
        Options {
            anchor_crc_mismatch: true,
            ..Options::default()
        },
        Options {
            descriptor_extent_out_of_bounds: true,
            ..Options::default()
        },
        Options {
            missing_partition_descriptor: true,
            ..Options::default()
        },
        Options {
            missing_terminator: true,
            ..Options::default()
        },
        Options {
            logical_block_size: 512,
            ..Options::default()
        },
    ] {
        let image = fixture::make_image(fixture::SECTOR_COUNT, options);
        assert_eq!(
            run(&image),
            Err(SanitizedError::InvalidInput),
            "a malformed ECMA-167 structure must be rejected"
        );
    }
}

#[test]
fn a_descriptor_sequence_extent_near_u32_max_terminates_quickly() {
    // The recorded length is bounded before it is used: the scan is capped at
    // `DESCRIPTOR_SCAN_LIMIT` blocks and every read stays inside the source.
    let image = fixture::make_image(
        fixture::SECTOR_COUNT,
        Options {
            descriptor_extent_near_u32_max: true,
            ..Options::default()
        },
    );
    // The extent no longer fits inside the pinned source, so it is rejected
    // before a single descriptor block is read.
    assert_eq!(run(&image), Err(SanitizedError::InvalidInput));
}

// --- archive ---------------------------------------------------------------

#[test]
fn a_preflight_only_fixture_is_rejected_by_the_reader_without_panicking() {
    // The synthetic fixture deliberately stops at the volume descriptor
    // sequence, so it has no file set descriptor. Mounting must fail with a
    // sanitized code rather than panic or hang.
    let image = fixture::valid_image();
    let mounted = UdfArchive::open(SliceBlockReader::new(&image), DirectoryLimits::default());
    assert_eq!(mounted.err(), Some(SanitizedError::InvalidInput));
}

#[test]
fn media_of_another_class_never_mounts_as_udf() {
    let zeroes = alloc::vec![0u8; fixture::SECTOR_COUNT * BLOCK_SIZE];
    let mounted = UdfArchive::open(SliceBlockReader::new(&zeroes), DirectoryLimits::default());
    assert_eq!(mounted.err(), Some(SanitizedError::Unsupported));
}

#[test]
fn invalid_limits_are_rejected_before_any_read() {
    let image = fixture::valid_image();
    let limits = DirectoryLimits {
        max_page_count: 0,
        ..DirectoryLimits::default()
    };
    let mounted = UdfArchive::open(SliceBlockReader::new(&image), limits);
    assert_eq!(mounted.err(), Some(SanitizedError::InvalidInput));
}

/// Follow-up asked for by the dependency audit: confirm that `hadris-udf`
/// cannot be made to spin on an anchor whose main volume descriptor sequence
/// extent length is near `u32::MAX`.
///
/// The recorded length implies about 2.1 million logical blocks. The fixture
/// also omits the terminating descriptor, so nothing but the end of the source
/// can stop the scan; the run is therefore bounded by the source's own length,
/// not by the media-controlled extent. The test asserts the whole call returns
/// well inside a wall-clock budget for both a small and a much larger source.
#[test]
fn hadris_udf_does_not_hang_on_an_anchor_extent_near_u32_max() {
    for blocks in [fixture::SECTOR_COUNT, 8_192] {
        let mut image = fixture::make_image(
            blocks,
            Options {
                missing_terminator: true,
                ..Options::default()
            },
        );
        let anchor = fixture::ANCHOR_SECTOR as usize * BLOCK_SIZE;
        // Rewrite the main volume descriptor sequence extent length in place
        // and re-stamp the tag so the descriptor stays internally consistent.
        image[anchor + 16..anchor + 20].copy_from_slice(&(u32::MAX - 2_047).to_le_bytes());
        fixture::finish_tag(&mut image, fixture::ANCHOR_SECTOR, 2, 496);

        let started = std::time::Instant::now();
        let opened = hadris_udf::fs::UdfVolume::open(crate::adaptor::BlockCursor::new(
            SliceBlockReader::new(&image),
        ));
        let elapsed = started.elapsed();
        assert!(opened.is_err(), "the fixture has no file set descriptor");
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "hadris-udf took {elapsed:?} on a near-u32::MAX anchor extent"
        );
        println!(
            "hadris-udf near-u32::MAX anchor extent over {blocks} blocks completed in {elapsed:?}"
        );
    }
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
        offset in 0usize..fixture::SECTOR_COUNT * BLOCK_SIZE,
        value in proptest::num::u8::ANY,
    ) {
        let mut image = fixture::valid_image();
        image[offset] = value;
        let _ = preflight(&mut SliceBlockReader::new(&image));
        let _ = UdfArchive::open(SliceBlockReader::new(&image), DirectoryLimits::default());
    }
}
