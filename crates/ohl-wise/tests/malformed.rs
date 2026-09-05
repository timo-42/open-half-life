//! Malformed input is rejected, flagged or bounded — never trusted.

use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};
use ohl_wise::{Error, Limit, Limits, NeverCancelled, SliceSource, read_package};

fn files() -> Vec<SyntheticFile> {
    vec![
        SyntheticFile::new(b"one.dat", vec![3u8; 2048]),
        SyntheticFile::new(b"two.dat", vec![9u8; 2048]),
    ]
}

#[test]
fn rejects_input_that_is_not_an_executable() {
    let mut source = SliceSource::new(&[0u8; 4096]);
    assert_eq!(
        read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled).unwrap_err(),
        Error::NotExecutable
    );
}

#[test]
fn rejects_an_empty_source() {
    let mut source = SliceSource::new(&[]);
    assert_eq!(
        read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled).unwrap_err(),
        Error::NotExecutable
    );
}

#[test]
fn flags_a_corrupt_checksum_and_refuses_to_extract_that_file() {
    let built = build_package(&PackageOptions {
        corrupt_crc_of_stream: Some(2),
        ..PackageOptions::with_files(files())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("a bad checksum does not stop the walk");
    assert_eq!(package.summary().crc_mismatches, 1);
    assert_eq!(
        package.summary().crc_matches as usize,
        built.stream_count - 1
    );

    let mut reader = package.open_file(0).expect("record maps to a stream");
    let mut out = Vec::new();
    assert_eq!(
        reader
            .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
            .unwrap_err(),
        Error::ChecksumMismatch
    );
}

#[test]
fn rejects_a_truncated_final_stream() {
    let built = build_package(&PackageOptions {
        truncate_bytes: 40,
        ..PackageOptions::with_files(files())
    });
    let mut source = SliceSource::new(&built.image);
    let error = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled).unwrap_err();
    assert!(
        matches!(error, Error::Truncated | Error::DecompressionFailed),
        "unexpected error"
    );
}

#[test]
fn refuses_an_oversized_declared_inflated_size() {
    let built = build_package(&PackageOptions {
        declared_size_scale: 3,
        ..PackageOptions::with_files(files())
    });
    let mut source = SliceSource::new(&built.image);
    let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)
        .expect("the chain is unaffected by a wrong declared size");
    let mut reader = package.open_file(0).expect("record maps to a stream");
    let mut out = Vec::new();
    assert_eq!(
        reader
            .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
            .unwrap_err(),
        Error::ChecksumMismatch
    );
}

#[test]
fn enforces_the_script_ceiling() {
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    let limits = Limits {
        max_script_bytes: 8,
        ..Limits::DEFAULT
    };
    assert_eq!(
        read_package(&mut source, None, limits, &NeverCancelled).unwrap_err(),
        Error::LimitExceeded(Limit::ScriptBytes)
    );
}

#[test]
fn enforces_the_total_inflated_ceiling() {
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    let limits = Limits {
        max_total_inflated_bytes: 1024,
        ..Limits::DEFAULT
    };
    assert_eq!(
        read_package(&mut source, None, limits, &NeverCancelled).unwrap_err(),
        Error::LimitExceeded(Limit::TotalInflatedBytes)
    );
}

#[test]
fn honours_cancellation_between_chunks() {
    struct Always;
    impl ohl_wise::Cancellation for Always {
        fn is_cancelled(&self) -> bool {
            true
        }
    }
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    assert_eq!(
        read_package(&mut source, None, Limits::DEFAULT, &Always).unwrap_err(),
        Error::Cancelled
    );
}

#[test]
fn a_random_overlay_offset_fails_closed() {
    let built = build_package(&PackageOptions::with_files(files()));
    let mut source = SliceSource::new(&built.image);
    let error = read_package(
        &mut source,
        Some(built.overlay_offset + 7),
        Limits::DEFAULT,
        &NeverCancelled,
    );
    // Either the scan finds the real first stream a few bytes later, or it
    // finds nothing; it must never silently produce a bogus file table.
    match error {
        Ok(package) => assert_eq!(
            package.header().first_stream_offset,
            built.first_stream_offset
        ),
        Err(error) => assert_eq!(error, Error::HeaderNotFound),
    }
}
