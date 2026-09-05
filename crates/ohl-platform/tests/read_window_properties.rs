//! Property tests for the read-window bounds check.
//!
//! The invariant under test is the one the C++ contract states: a read
//! succeeds **exactly** when `offset + length <= size()`, it returns exactly
//! the bytes at that window, and every other window is rejected as
//! `OutOfRange` without touching the destination buffer or the OS.

use std::fs;
use std::io::Write as _;

use ohl_platform::{MediaSource, MediaSourceError};
use proptest::prelude::*;

/// The single fixture size every case is generated against.
const FIXTURE_SIZE: usize = 1024;

/// [`FIXTURE_SIZE`] as the offset type.
const FIXTURE_SIZE_U64: u64 = FIXTURE_SIZE as u64;

/// Generates `(offset, first, second)` triples that always fit inside the
/// fixture, so the property never has to reject a generated case.
fn a_split_window() -> impl Strategy<Value = (usize, usize, usize)> {
    (0usize..=FIXTURE_SIZE)
        .prop_flat_map(|offset| (Just(offset), 0usize..=(FIXTURE_SIZE - offset)))
        .prop_flat_map(|(offset, first)| {
            (
                Just(offset),
                Just(first),
                0usize..=(FIXTURE_SIZE - offset - first),
            )
        })
}

/// Creates one shared fixture whose byte at `index` is `(index * 37) & 0xff`.
fn fixture() -> (tempfile::TempDir, Vec<u8>, MediaSource) {
    let root = tempfile::tempdir().expect("temporary directory");
    let bytes: Vec<u8> = (0..FIXTURE_SIZE)
        .map(|index| u8::try_from((index * 37) & 0xff).expect("masked to a byte"))
        .collect();
    let path = root.path().join("properties.fixture");
    let mut file = fs::File::create(&path).expect("fixture creation");
    file.write_all(&bytes).expect("fixture bytes");
    file.sync_all().expect("fixture flush");
    let source = MediaSource::open(&path).expect("acquisition");
    (root, bytes, source)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Windows inside the pinned size always read exactly the right bytes,
    /// and windows outside it are always refused.
    #[test]
    fn a_window_is_accepted_exactly_when_it_fits(offset in 0u64..2048, length in 0usize..2048) {
        let (_root, bytes, source) = fixture();
        let mut destination = vec![0xccu8; length];
        let result = source.read_exact_at(offset, &mut destination);

        let requested = u64::try_from(length).expect("generated lengths are small");
        let fits = offset
            .checked_add(requested)
            .is_some_and(|end| end <= source.size());
        if fits {
            let start = usize::try_from(offset).expect("offset fits the fixture");
            prop_assert_eq!(result, Ok(()));
            prop_assert_eq!(&destination[..], &bytes[start..start + length]);
        } else {
            prop_assert_eq!(result, Err(MediaSourceError::OutOfRange));
            prop_assert!(
                destination.iter().all(|byte| *byte == 0xcc),
                "a refused read must not disturb the destination"
            );
        }
    }

    /// No offset, however large, can make a read succeed past the pinned end,
    /// and none of them can overflow the bounds arithmetic.
    #[test]
    fn far_offsets_are_always_refused(
        offset in (FIXTURE_SIZE_U64 + 1)..=u64::MAX,
        length in 0usize..64,
    ) {
        let (_root, _bytes, source) = fixture();
        let mut destination = vec![0u8; length];
        prop_assert_eq!(
            source.read_exact_at(offset, &mut destination),
            Err(MediaSourceError::OutOfRange)
        );
    }

    /// Reading a window in two adjacent pieces yields the same bytes as
    /// reading it in one call, which is what "no shared seek cursor" means in
    /// observable terms.
    #[test]
    fn split_reads_compose((offset, first, second) in a_split_window()) {
        let (_root, _bytes, source) = fixture();
        let offset64 = u64::try_from(offset).expect("generated offsets are small");
        let first64 = u64::try_from(first).expect("generated lengths are small");

        let mut whole = vec![0u8; first + second];
        source
            .read_exact_at(offset64, &mut whole)
            .expect("in-range read");

        let mut head = vec![0u8; first];
        let mut tail = vec![0u8; second];
        source
            .read_exact_at(offset64, &mut head)
            .expect("in-range head read");
        source
            .read_exact_at(offset64 + first64, &mut tail)
            .expect("in-range tail read");

        head.extend_from_slice(&tail);
        prop_assert_eq!(head, whole);
    }
}
