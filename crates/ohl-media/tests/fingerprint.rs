//! Fingerprinting and validated-media proof behaviour over synthetic files.

mod support;

use std::sync::Arc;

use ohl_media::{
    FINGERPRINT_CHUNK_BYTES, MediaDigest, MediaError, ValidatedMedia, fingerprint,
    fingerprint_with_progress,
};
use ohl_platform::MediaSource;
use support::{description, expected_digest, pinned_source, synthetic_bytes};

/// FIPS 180-4 vectors, so the crate is anchored to the standard and not only
/// to `ohl-core` agreeing with itself.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn known_vectors_match_over_temporary_files() {
    let root = tempfile::tempdir().expect("temporary directory");

    let empty = pinned_source(root.path(), "empty.bin", b"");
    assert_eq!(
        fingerprint(&empty).expect("empty digest").to_hex(),
        EMPTY_SHA256
    );

    let abc = pinned_source(root.path(), "abc.bin", b"abc");
    assert_eq!(fingerprint(&abc).expect("abc digest").to_hex(), ABC_SHA256);
}

#[test]
fn a_multi_block_source_agrees_with_ohl_core() {
    let root = tempfile::tempdir().expect("temporary directory");
    // Deliberately not a multiple of the read window, so the final short
    // chunk is exercised as well.
    let length = usize::try_from(FINGERPRINT_CHUNK_BYTES).expect("window fits") * 3 + 1_237;
    let content = synthetic_bytes(length);
    let source = pinned_source(root.path(), "multi-block.bin", &content);

    assert_eq!(
        fingerprint(&source).expect("digest"),
        expected_digest(&content)
    );
}

#[test]
fn exact_window_multiples_and_boundaries_agree_with_ohl_core() {
    let root = tempfile::tempdir().expect("temporary directory");
    let window = usize::try_from(FINGERPRINT_CHUNK_BYTES).expect("window fits");
    for length in [1, window - 1, window, window + 1, window * 2] {
        let content = synthetic_bytes(length);
        let source = pinned_source(root.path(), &format!("length-{length}.bin"), &content);
        assert_eq!(
            fingerprint(&source).expect("digest"),
            expected_digest(&content),
            "digest disagreed at length {length}"
        );
    }
}

#[test]
fn progress_is_reported_for_every_completed_chunk() {
    let root = tempfile::tempdir().expect("temporary directory");
    let window = FINGERPRINT_CHUNK_BYTES;
    let content = synthetic_bytes(usize::try_from(window).expect("window fits") * 2 + 5);
    let source = pinned_source(root.path(), "progress.bin", &content);

    let mut observed = Vec::new();
    let digest =
        fingerprint_with_progress(&source, &mut |hashed| observed.push(hashed)).expect("digest");
    assert_eq!(digest, expected_digest(&content));
    assert_eq!(
        observed,
        vec![
            window,
            window * 2,
            u64::try_from(content.len()).expect("length")
        ]
    );
}

#[test]
fn truncation_between_chunks_is_reported_as_a_change() {
    let root = tempfile::tempdir().expect("temporary directory");
    let window = usize::try_from(FINGERPRINT_CHUNK_BYTES).expect("window fits");
    let path = root.path().join("truncated.bin");
    std::fs::write(&path, synthetic_bytes(window * 4)).expect("fixture");
    let source = MediaSource::open(&path).expect("pinned source");

    // Truncating the pinned object after the first chunk makes the next
    // positional read hit end of file inside the pinned size.
    let error = fingerprint_with_progress(&source, &mut |hashed| {
        if hashed == FINGERPRINT_CHUNK_BYTES {
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("writer")
                .set_len(0)
                .expect("truncate");
        }
    })
    .expect_err("a truncated source cannot be fingerprinted");
    assert_eq!(error, MediaError::SourceChanged);
}

#[test]
fn an_in_place_rewrite_between_chunks_is_reported_as_a_change() {
    use std::io::Write as _;

    let root = tempfile::tempdir().expect("temporary directory");
    let window = usize::try_from(FINGERPRINT_CHUNK_BYTES).expect("window fits");
    let path = root.path().join("rewritten.bin");
    std::fs::write(&path, synthetic_bytes(window * 4)).expect("fixture");
    let source = MediaSource::open(&path).expect("pinned source");

    let error = fingerprint_with_progress(&source, &mut |hashed| {
        if hashed == FINGERPRINT_CHUNK_BYTES {
            let mut writer = std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("writer");
            writer.write_all(b"rewritten").expect("rewrite");
        }
    })
    .expect_err("a rewritten source cannot be fingerprinted");
    assert_eq!(error, MediaError::SourceChanged);
}

#[test]
fn replacing_the_pathname_does_not_retarget_the_pinned_source() {
    let root = tempfile::tempdir().expect("temporary directory");
    let content = synthetic_bytes(4_096);
    let path = root.path().join("original.bin");
    std::fs::write(&path, &content).expect("fixture");
    let source = MediaSource::open(&path).expect("pinned source");

    std::fs::rename(&path, root.path().join("displaced.bin")).expect("displace");
    std::fs::write(&path, synthetic_bytes(8_192)).expect("impostor");

    assert_eq!(
        fingerprint(&source).expect("digest"),
        expected_digest(&content),
        "the capability must still hash the object it pinned"
    );
}

#[test]
fn the_proof_rejects_a_mismatched_size() {
    let root = tempfile::tempdir().expect("temporary directory");
    let content = synthetic_bytes(2_048);
    let source = pinned_source(root.path(), "proof.bin", &content);
    let digest = fingerprint(&source).expect("digest");

    for wrong_size in [0, content.len() as u64 - 1, content.len() as u64 + 1] {
        let error = ValidatedMedia::new(Arc::clone(&source), wrong_size, digest, description())
            .expect_err("a mismatched size cannot mint a proof");
        assert_eq!(error, MediaError::InvalidCapability);
    }

    let proof = ValidatedMedia::new(
        Arc::clone(&source),
        content.len() as u64,
        digest,
        description(),
    )
    .expect("matching size");
    assert_eq!(proof.size_bytes(), content.len() as u64);
    assert_eq!(proof.digest(), &digest);
    assert_eq!(proof.description(), &description());
    assert_eq!(proof.source_fingerprint().sha256, *digest.as_bytes());
    proof.verify_unchanged().expect("still pinned");
}

#[test]
fn the_proof_rejects_a_source_that_already_changed() {
    let root = tempfile::tempdir().expect("temporary directory");
    let path = root.path().join("changed.bin");
    std::fs::write(&path, synthetic_bytes(1_024)).expect("fixture");
    let source = Arc::new(MediaSource::open(&path).expect("pinned source"));
    let size = source.size();

    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("writer")
        .set_len(512)
        .expect("truncate");

    let error = ValidatedMedia::new(
        source,
        size,
        MediaDigest::from_bytes([0u8; 32]),
        description(),
    )
    .expect_err("a changed source cannot mint a proof");
    assert_eq!(error, MediaError::SourceChanged);
}
