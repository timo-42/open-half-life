//! Property tests: no arbitrary byte string can make this crate panic.

use ohl_isz::testing::{ArchiveBuilder, sample_archive};
use ohl_isz::{
    Archive, Explode, Limits, NeverCancelled, SliceSource, explode_to_vec, find_signature,
};
use proptest::prelude::*;

/// Tight ceilings so a hostile input cannot make one case slow.
fn limits() -> Limits {
    Limits {
        max_scan_bytes: 1 << 20,
        max_archive_bytes: 1 << 22,
        max_directories: 64,
        max_entries: 512,
        max_directory_bytes: 1 << 18,
        max_name_bytes: 260,
        max_stored_bytes_per_entry: 1 << 20,
        max_expanded_bytes_per_entry: 1 << 20,
        max_total_expanded_bytes: 1 << 22,
        max_chunk_bytes: 4096,
    }
}

/// Opens `bytes` as an archive and drains every entry, ignoring failures.
fn exercise_archive(bytes: &[u8], base: u64) {
    let mut source = SliceSource::new(bytes);
    let Ok(mut archive) = Archive::open(&mut source, base, &limits(), &NeverCancelled) else {
        return;
    };
    let count = u32::try_from(archive.entries().len()).unwrap_or(0);
    for index in 0..count.min(64) {
        let mut source = SliceSource::new(bytes);
        let Ok(mut reader) = archive.open_entry(index) else {
            continue;
        };
        let _ = reader.read_to_vec(&mut source, &NeverCancelled);
    }
}

/// Runs the explode decoder in one shot and in 1-byte increments.
fn exercise_explode(bytes: &[u8]) {
    let _ = explode_to_vec(bytes, 1 << 16);

    let mut decoder = Explode::new();
    let mut out = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        pending.push(*byte);
        let last = index + 1 == bytes.len();
        let Ok(progress) = decoder.decode(&pending, last, &mut out, 1 << 16) else {
            return;
        };
        pending.drain(..progress.consumed);
        if progress.finished || out.len() > 1 << 16 {
            return;
        }
        out.clear();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bytes_never_panic_the_archive_parser(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        exercise_archive(&bytes, 0);
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_signature_scan(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let mut source = SliceSource::new(&bytes);
        let _ = find_signature(&mut source, &limits(), &NeverCancelled);
    }

    #[test]
    fn arbitrary_bytes_never_panic_the_explode_decoder(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        exercise_explode(&bytes);
    }

    /// A single corrupted byte anywhere in a well-formed archive.
    #[test]
    fn one_flipped_byte_never_panics(index in 0usize..4096, value in any::<u8>()) {
        let mut fixture = sample_archive();
        if index < fixture.bytes.len() {
            fixture.bytes[index] = value;
        }
        exercise_archive(&fixture.bytes, 0);
    }

    /// A well-formed archive truncated at an arbitrary point.
    #[test]
    fn a_truncated_archive_never_panics(keep in 0usize..4096) {
        let fixture = sample_archive();
        let keep = keep.min(fixture.bytes.len());
        exercise_archive(&fixture.bytes[..keep], 0);
    }

    /// Arbitrary payload bytes survive an implode/explode round trip.
    #[test]
    fn implode_round_trips_arbitrary_payloads(
        payload in prop::collection::vec(any::<u8>(), 0..3000),
        dictionary in 4u8..=6,
        coded in any::<bool>(),
    ) {
        let stream = ohl_isz::testing::implode(&payload, dictionary, coded);
        prop_assert_eq!(explode_to_vec(&stream, 1 << 20).unwrap(), payload);
    }

    /// Arbitrary payloads survive a full archive round trip.
    #[test]
    fn archives_round_trip_arbitrary_payloads(
        payload in prop::collection::vec(any::<u8>(), 0..2000),
        stored in any::<bool>(),
    ) {
        let mut builder = ArchiveBuilder::new();
        let root = builder.directory(b"D");
        builder.entry(root, b"E.DAT", &payload, stored);
        let fixture = builder.build();
        let mut source = SliceSource::new(&fixture.bytes);
        let mut archive = Archive::open(&mut source, 0, &limits(), &NeverCancelled).unwrap();
        let mut source = SliceSource::new(&fixture.bytes);
        let out = archive
            .open_entry(0)
            .unwrap()
            .read_to_vec(&mut source, &NeverCancelled)
            .unwrap();
        prop_assert_eq!(out, payload);
    }
}
