//! The Rust port of the source-stability boundary checks from
//! `src/media/src/source_stability.cpp`.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use ohl_core::StreamingSha256;
use ohl_platform::MediaSource;
use ohl_platform::stability::{
    SourceFingerprint, SourceStabilityError, verify_complete_source_stability,
    verify_complete_source_stability_with_cancellation,
};

/// Writes `bytes` and returns the fingerprint a validation pass would have
/// accepted for them.
fn fixture(path: &Path, bytes: &[u8]) -> SourceFingerprint {
    let mut file = fs::File::create(path).expect("fixture creation");
    file.write_all(bytes).expect("fixture bytes");
    file.sync_all().expect("fixture flush");
    SourceFingerprint {
        size_bytes: u64::try_from(bytes.len()).expect("fixture size"),
        sha256: StreamingSha256::digest(bytes),
    }
}

#[test]
fn an_unchanged_source_reauthenticates() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("stable.fixture");
    // Larger than one 64 KiB rehash chunk, so the chunk loop is exercised.
    let bytes: Vec<u8> = (0..200_000u32).map(|index| (index % 251) as u8).collect();
    let fingerprint = fixture(&path, &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        verify_complete_source_stability(&source, &fingerprint),
        Ok(())
    );
}

#[test]
fn an_empty_source_reauthenticates() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("empty.fixture");
    let fingerprint = fixture(&path, &[]);
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        verify_complete_source_stability(&source, &fingerprint),
        Ok(())
    );
}

#[test]
fn a_fingerprint_for_a_different_size_is_an_invalid_capability() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("mismatched-size.fixture");
    let mut fingerprint = fixture(&path, b"content");
    fingerprint.size_bytes += 1;
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        verify_complete_source_stability(&source, &fingerprint),
        Err(SourceStabilityError::InvalidCapability)
    );
}

#[test]
fn a_different_digest_for_the_same_bytes_is_a_mismatch() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("mismatched-digest.fixture");
    let mut fingerprint = fixture(&path, b"content");
    fingerprint.sha256[0] ^= 0xff;
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        verify_complete_source_stability(&source, &fingerprint),
        Err(SourceStabilityError::DigestMismatch)
    );
}

#[test]
fn a_truncation_after_pinning_is_reported_as_a_source_change() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("truncated.fixture");
    let bytes = vec![0xa5u8; 4096];
    let fingerprint = fixture(&path, &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(11)
        .expect("truncation");

    assert_eq!(
        verify_complete_source_stability(&source, &fingerprint),
        Err(SourceStabilityError::SourceChanged),
        "a change must never be reported as a digest mismatch"
    );
}

#[test]
fn cancellation_before_the_first_chunk_stops_the_check() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("cancelled.fixture");
    let bytes = vec![7u8; 300_000];
    let fingerprint = fixture(&path, &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    assert_eq!(
        verify_complete_source_stability_with_cancellation(&source, &fingerprint, &mut || true),
        Err(SourceStabilityError::Cancelled)
    );
}

#[test]
fn cancellation_between_chunks_stops_the_check() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("cancelled-mid.fixture");
    let bytes = vec![7u8; 300_000];
    let fingerprint = fixture(&path, &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    let mut polls = 0u32;
    let outcome =
        verify_complete_source_stability_with_cancellation(&source, &fingerprint, &mut || {
            polls += 1;
            polls > 2
        });
    assert_eq!(outcome, Err(SourceStabilityError::Cancelled));
}

#[test]
fn a_cancellation_that_never_fires_completes_normally() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("not-cancelled.fixture");
    let bytes = vec![3u8; 130_000];
    let fingerprint = fixture(&path, &bytes);
    let source = MediaSource::open(&path).expect("acquisition");

    let mut polls = 0u32;
    let outcome =
        verify_complete_source_stability_with_cancellation(&source, &fingerprint, &mut || {
            polls += 1;
            false
        });
    assert_eq!(outcome, Ok(()));
    assert!(polls > 1, "the predicate must be polled between chunks");
}
