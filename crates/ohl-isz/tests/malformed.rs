//! Every malformed-field rejection, plus the cancellation and limit paths.
//!
//! Each case starts from a project-authored synthetic archive and corrupts
//! exactly one field, so a failure names the check that regressed.

use ohl_isz::testing::{ArchiveBuilder, SyntheticArchive, sample_archive};
use ohl_isz::{
    Archive, Cancellation, Error, Limit, Limits, NeverCancelled, SliceSource, find_signature,
};

/// A token that is signalled from the very first poll.
struct AlwaysCancelled;

impl Cancellation for AlwaysCancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

/// A token that is signalled after `after` polls.
struct CancelAfter {
    remaining: core::cell::Cell<u32>,
}

impl CancelAfter {
    fn new(after: u32) -> Self {
        Self {
            remaining: core::cell::Cell::new(after),
        }
    }
}

impl Cancellation for CancelAfter {
    fn is_cancelled(&self) -> bool {
        let left = self.remaining.get();
        if left == 0 {
            return true;
        }
        self.remaining.set(left - 1);
        false
    }
}

fn open_with(bytes: &[u8], limits: &Limits) -> Result<Archive, Error> {
    let mut source = SliceSource::new(bytes);
    Archive::open(&mut source, 0, limits, &NeverCancelled)
}

fn open(bytes: &[u8]) -> Result<Archive, Error> {
    open_with(bytes, &Limits::default())
}

fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn fixture() -> SyntheticArchive {
    sample_archive()
}

// ---------------------------------------------------------------- header

#[test]
fn rejects_a_corrupt_first_signature_word() {
    let mut archive = fixture();
    archive.bytes[0] ^= 0xff;
    assert_eq!(open(&archive.bytes), Err(Error::BadSignature));
}

#[test]
fn rejects_a_corrupt_second_signature_word() {
    let mut archive = fixture();
    archive.bytes[5] ^= 0xff;
    assert_eq!(open(&archive.bytes), Err(Error::BadSignature));
}

#[test]
fn rejects_a_container_shorter_than_the_header() {
    let archive = fixture();
    assert_eq!(open(&archive.bytes[..40]), Err(Error::Truncated));
}

#[test]
fn rejects_a_table_of_contents_offset_past_the_archive_end() {
    let mut archive = fixture();
    put32(&mut archive.bytes, 0x29, u32::MAX);
    assert_eq!(open(&archive.bytes), Err(Error::OutOfRange));
}

#[test]
fn rejects_a_table_of_contents_offset_before_the_data_start() {
    let mut archive = fixture();
    put32(&mut archive.bytes, 0x29, 12);
    assert_eq!(open(&archive.bytes), Err(Error::OutOfRange));
}

#[test]
fn rejects_an_archive_size_that_disagrees_with_the_container() {
    let mut archive = fixture();
    let inflated = u32::try_from(archive.bytes.len()).unwrap() + 4_096;
    put32(&mut archive.bytes, 0x12, inflated);
    // The table of contents now claims bytes the container does not have.
    assert_eq!(open(&archive.bytes), Err(Error::Truncated));
}

#[test]
fn rejects_a_directory_count_that_disagrees_with_the_entry_count() {
    let mut archive = fixture();
    put16(&mut archive.bytes, 0x31, 0);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_an_entry_count_that_disagrees_with_the_directory_records() {
    let mut archive = fixture();
    put16(&mut archive.bytes, 0x0c, 2);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_a_multi_volume_marker() {
    let mut builder = ArchiveBuilder::new().multi_volume(1);
    let root = builder.directory(b"");
    builder.entry(root, b"A.TXT", b"invented", true);
    let archive = builder.build();
    assert_eq!(open(&archive.bytes), Err(Error::SplitArchiveUnsupported));
}

#[test]
fn rejects_a_volume_number_above_one() {
    let mut archive = fixture();
    archive.bytes[0x1f] = 2;
    assert_eq!(open(&archive.bytes), Err(Error::SplitArchiveUnsupported));
}

// ----------------------------------------------------- directory records

#[test]
fn rejects_a_directory_record_size_below_its_fixed_part() {
    let mut archive = fixture();
    let at = archive.directory_records[0];
    put16(&mut archive.bytes, at + 2, 3);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_a_zero_directory_record_size() {
    let mut archive = fixture();
    let at = archive.directory_records[0];
    put16(&mut archive.bytes, at + 2, 0);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_a_directory_record_size_that_runs_past_the_table() {
    let mut archive = fixture();
    let at = archive.directory_records[0];
    put16(&mut archive.bytes, at + 2, u16::MAX);
    assert_eq!(open(&archive.bytes), Err(Error::Truncated));
}

#[test]
fn rejects_a_directory_name_longer_than_its_record() {
    let mut archive = fixture();
    let at = archive.directory_records[1];
    put16(&mut archive.bytes, at + 4, 900);
    assert_eq!(
        open(&archive.bytes),
        Err(Error::LimitExceeded(Limit::NameBytes))
    );
}

#[test]
fn rejects_a_directory_name_longer_than_the_name_limit() {
    let archive = fixture();
    let limits = Limits {
        max_name_bytes: 2,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::NameBytes))
    );
}

#[test]
fn rejects_a_directory_entry_count_above_the_entry_limit() {
    let mut archive = fixture();
    let at = archive.directory_records[0];
    put16(&mut archive.bytes, at, u16::MAX);
    let limits = Limits {
        max_entries: 8,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::Entries))
    );
}

// --------------------------------------------------------- entry records

#[test]
fn rejects_an_entry_record_size_below_its_fixed_part() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    put16(&mut archive.bytes, at + 0x17, 8);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_an_entry_record_size_that_runs_past_the_table() {
    let mut archive = fixture();
    let at = archive.entry_records[2];
    put16(&mut archive.bytes, at + 0x17, u16::MAX);
    assert_eq!(open(&archive.bytes), Err(Error::Truncated));
}

#[test]
fn rejects_an_entry_name_length_that_runs_past_the_table() {
    let mut archive = fixture();
    let at = archive.entry_records[2];
    archive.bytes[at + 0x1d] = 255;
    assert_eq!(open(&archive.bytes), Err(Error::Truncated));
}

#[test]
fn rejects_an_entry_name_longer_than_the_name_limit() {
    let archive = fixture();
    let limits = Limits {
        max_name_bytes: 5,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::NameBytes))
    );
}

#[test]
fn rejects_an_entry_offset_before_the_data_start() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    put32(&mut archive.bytes, at + 0x0b, 4);
    assert_eq!(open(&archive.bytes), Err(Error::OutOfRange));
}

#[test]
fn rejects_an_entry_extent_past_the_archive_end() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    // Large enough to run off the end of the archive, small enough to stay
    // under the per-entry stored-size ceiling.
    put32(&mut archive.bytes, at + 0x07, 1_000_000);
    assert_eq!(open(&archive.bytes), Err(Error::OutOfRange));
}

#[test]
fn rejects_a_stored_entry_whose_sizes_disagree() {
    let mut archive = fixture();
    // Entry 1 is the stored one; make its expanded size differ.
    let at = archive.entry_records[1];
    put32(&mut archive.bytes, at + 0x03, 17);
    assert_eq!(open(&archive.bytes), Err(Error::InvalidInput));
}

#[test]
fn rejects_a_stored_size_above_the_per_entry_limit() {
    let archive = fixture();
    let limits = Limits {
        max_stored_bytes_per_entry: 16,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::StoredBytesPerEntry))
    );
}

#[test]
fn rejects_an_expanded_size_above_the_per_entry_limit() {
    let archive = fixture();
    let limits = Limits {
        max_expanded_bytes_per_entry: 16,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::ExpandedBytesPerEntry))
    );
}

#[test]
fn rejects_a_split_entry_at_open_time() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    archive.bytes[at + 0x1a] = 1;
    let mut opened = open(&archive.bytes).expect("the archive itself is well formed");
    assert_eq!(
        opened.open_entry(0).err(),
        Some(Error::SplitArchiveUnsupported)
    );
}

#[test]
fn rejects_an_entry_that_starts_and_ends_in_different_volumes() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    archive.bytes[at] = 2; // last volume
    let mut opened = open(&archive.bytes).expect("the archive itself is well formed");
    assert_eq!(
        opened.open_entry(0).err(),
        Some(Error::SplitArchiveUnsupported)
    );
}

#[test]
fn rejects_an_unknown_entry_index() {
    let archive = fixture();
    let mut opened = open(&archive.bytes).unwrap();
    assert_eq!(opened.entry(99).err(), Some(Error::NotFound));
    assert_eq!(opened.directory(99).err(), Some(Error::NotFound));
    assert_eq!(opened.open_entry(99).err(), Some(Error::NotFound));
}

// ------------------------------------------------------------ extraction

#[test]
fn rejects_a_corrupt_imploded_stream() {
    let mut archive = fixture();
    // Entry 0 is imploded and starts at the fixed data start.
    let offset = usize::try_from(ohl_isz::DATA_START).unwrap();
    for byte in &mut archive.bytes[offset..offset + 8] {
        *byte = 0xff;
    }
    let mut opened = open(&archive.bytes).expect("the table of contents is intact");
    let mut source = SliceSource::new(&archive.bytes);
    let outcome = opened
        .open_entry(0)
        .expect("entry opens")
        .read_to_vec(&mut source, &NeverCancelled);
    assert!(matches!(
        outcome,
        Err(Error::DecompressionFailed | Error::SizeMismatch)
    ));
}

#[test]
fn rejects_an_expanded_size_that_disagrees_with_the_stream() {
    let mut archive = fixture();
    let at = archive.entry_records[0];
    let recorded = u32::from_le_bytes(archive.bytes[at + 3..at + 7].try_into().unwrap());
    put32(&mut archive.bytes, at + 0x03, recorded + 1);
    let mut opened = open(&archive.bytes).expect("the table of contents is intact");
    let mut source = SliceSource::new(&archive.bytes);
    assert_eq!(
        opened
            .open_entry(0)
            .expect("entry opens")
            .read_to_vec(&mut source, &NeverCancelled),
        Err(Error::SizeMismatch)
    );
}

#[test]
fn rejects_extraction_beyond_the_total_expanded_budget() {
    let archive = fixture();
    let limits = Limits {
        max_total_expanded_bytes: 40,
        ..Limits::default()
    };
    let mut opened = open_with(&archive.bytes, &limits).expect("the archive opens");
    assert!(opened.open_entry(0).is_ok());
    assert_eq!(
        opened.open_entry(1).err(),
        Some(Error::LimitExceeded(Limit::TotalExpandedBytes))
    );
}

// --------------------------------------------------------------- limits

#[test]
fn rejects_invalid_limits_everywhere_they_are_accepted() {
    let archive = fixture();
    let limits = Limits {
        max_chunk_bytes: 0,
        ..Limits::default()
    };
    assert_eq!(open_with(&archive.bytes, &limits), Err(Error::InvalidInput));
    let mut source = SliceSource::new(&archive.bytes);
    assert_eq!(
        find_signature(&mut source, &limits, &NeverCancelled),
        Err(Error::InvalidInput)
    );
}

#[test]
fn rejects_a_table_of_contents_larger_than_its_limit() {
    let archive = fixture();
    let limits = Limits {
        max_directory_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        open_with(&archive.bytes, &limits),
        Err(Error::LimitExceeded(Limit::DirectoryBytes))
    );
}

// ---------------------------------------------------------- cancellation

#[test]
fn cancellation_stops_the_signature_scan() {
    let archive = fixture();
    let mut source = SliceSource::new(&archive.bytes);
    assert_eq!(
        find_signature(&mut source, &Limits::default(), &AlwaysCancelled),
        Err(Error::Cancelled)
    );
}

#[test]
fn cancellation_stops_opening_an_archive() {
    let archive = fixture();
    let mut source = SliceSource::new(&archive.bytes);
    assert_eq!(
        Archive::open(&mut source, 0, &Limits::default(), &AlwaysCancelled),
        Err(Error::Cancelled)
    );
}

#[test]
fn cancellation_stops_the_table_of_contents_walk() {
    let archive = fixture();
    let mut source = SliceSource::new(&archive.bytes);
    // Two polls happen before the walk starts, so the third stops it.
    assert_eq!(
        Archive::open(&mut source, 0, &Limits::default(), &CancelAfter::new(2)),
        Err(Error::Cancelled)
    );
}

#[test]
fn cancellation_stops_extraction_between_blocks() {
    let mut builder = ArchiveBuilder::new();
    let root = builder.directory(b"");
    let payload: Vec<u8> = (0..300_000u32)
        .map(|index| u8::try_from(index % 241).unwrap_or(0))
        .collect();
    builder.entry(root, b"BIG.DAT", &payload, false);
    let archive = builder.build();

    let mut opened = open(&archive.bytes).expect("the archive opens");
    let mut source = SliceSource::new(&archive.bytes);
    let mut reader = opened.open_entry(0).expect("entry opens");
    let mut buffer = [0u8; 512];
    let cancel = CancelAfter::new(3);
    let mut outcome = Ok(0);
    for _ in 0..10_000 {
        outcome = reader.read(&mut source, &cancel, &mut buffer);
        if outcome.is_err() {
            break;
        }
    }
    assert_eq!(outcome, Err(Error::Cancelled));
}
