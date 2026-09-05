//! The fixed-size InstallShield 3 archive header.
//!
//! Layout (all little-endian, no padding), as published by the Archive Team
//! file-format wiki and the `ISArchiveV3.h` description it links; see
//! `docs/FORMAT_SOURCES.md`:
//!
//! | offset | size | field                                          |
//! |-------:|-----:|------------------------------------------------|
//! |   0x00 |    4 | signature word 1, `0x8C65_5D13`                |
//! |   0x04 |    4 | signature word 2, `0x0002_013A`                |
//! |   0x08 |    2 | reserved                                       |
//! |   0x0a |    2 | non-zero when the archive is one of a set      |
//! |   0x0c |    2 | entry count                                    |
//! |   0x0e |    4 | MS-DOS packed date/time                        |
//! |   0x12 |    4 | archive size in bytes                          |
//! |   0x16 |    4 | total expanded size in bytes                   |
//! |   0x1a |    4 | reserved                                       |
//! |   0x1e |    1 | volume count (first volume only)               |
//! |   0x1f |    1 | volume number, 1-based                         |
//! |   0x20 |    1 | reserved                                       |
//! |   0x21 |    4 | split begin address                            |
//! |   0x25 |    4 | split end address                              |
//! |   0x29 |    4 | table-of-contents address                      |
//! |   0x2d |    4 | reserved                                       |
//! |   0x31 |    2 | directory count                                |
//! |   0x33 |    4 | reserved                                       |
//! |   0x37 |    4 | reserved                                       |

use crate::bytes::{le16, le32};
use crate::error::{Error, Limit, Result};
use crate::limits::Limits;

/// First signature word, little-endian `0x8C65_5D13`.
pub const SIGNATURE_WORD_1: u32 = 0x8c65_5d13;
/// Second signature word, little-endian `0x0002_013A`.
pub const SIGNATURE_WORD_2: u32 = 0x0002_013a;

/// The eight signature bytes an archive begins with.
pub const SIGNATURE: &[u8] = &[0x13, 0x5d, 0x65, 0x8c, 0x3a, 0x01, 0x02, 0x00];

/// Encoded size of the archive header.
pub const HEADER_SIZE: usize = 59;

/// Offset, relative to the archive base, at which entry data begins.
///
/// InstallShield 3 places the first entry's bytes at a fixed offset; each
/// entry also records its own offset, which this crate uses and validates.
pub const DATA_START: u64 = 255;

/// The parsed and validated archive header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchiveHeader {
    /// Number of entries recorded in the table of contents.
    pub entry_count: u16,
    /// Number of directories recorded in the table of contents.
    pub directory_count: u16,
    /// The archive's own length in bytes.
    pub archive_size: u32,
    /// The sum of every entry's expanded size, as recorded by the writer.
    pub expanded_size: u32,
    /// MS-DOS packed date/time, kept as an opaque number.
    pub datetime: u32,
    /// Non-zero when the writer marked this archive as one of a set.
    pub multi_volume: u16,
    /// Total number of volumes, recorded in the first volume only.
    pub volume_count: u8,
    /// This volume's 1-based number.
    pub volume_number: u8,
    /// First byte of the split region, when the archive is split.
    pub split_begin: u32,
    /// Last byte of the split region, when the archive is split.
    pub split_end: u32,
    /// Offset of the table of contents, relative to the archive base.
    pub toc_offset: u32,
}

impl ArchiveHeader {
    /// Parses and validates the header at the start of `bytes`.
    ///
    /// # Errors
    ///
    /// - [`Error::Truncated`] when `bytes` is shorter than [`HEADER_SIZE`].
    /// - [`Error::BadSignature`] when either signature word is wrong.
    /// - [`Error::LimitExceeded`] when a count or the archive size exceeds
    ///   `limits`.
    /// - [`Error::OutOfRange`] when the table of contents does not lie inside
    ///   the archive.
    /// - [`Error::InvalidInput`] when counts and the table of contents are
    ///   mutually inconsistent.
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        limits.validate()?;
        if bytes.len() < HEADER_SIZE {
            return Err(Error::Truncated);
        }
        if le32(bytes, 0x00) != SIGNATURE_WORD_1 || le32(bytes, 0x04) != SIGNATURE_WORD_2 {
            return Err(Error::BadSignature);
        }

        let header = Self {
            multi_volume: le16(bytes, 0x0a),
            entry_count: le16(bytes, 0x0c),
            datetime: le32(bytes, 0x0e),
            archive_size: le32(bytes, 0x12),
            expanded_size: le32(bytes, 0x16),
            volume_count: bytes[0x1e],
            volume_number: bytes[0x1f],
            split_begin: le32(bytes, 0x21),
            split_end: le32(bytes, 0x25),
            toc_offset: le32(bytes, 0x29),
            directory_count: le16(bytes, 0x31),
        };
        header.validate(limits)?;
        Ok(header)
    }

    fn validate(&self, limits: &Limits) -> Result<()> {
        if u64::from(self.archive_size) > limits.max_archive_bytes {
            return Err(Error::LimitExceeded(Limit::ArchiveBytes));
        }
        if u32::from(self.directory_count) > limits.max_directories {
            return Err(Error::LimitExceeded(Limit::Directories));
        }
        if u32::from(self.entry_count) > limits.max_entries {
            return Err(Error::LimitExceeded(Limit::Entries));
        }
        // The table of contents must start inside the archive, at or after
        // the fixed data start, and leave room for at least one record.
        if u64::from(self.archive_size) < DATA_START {
            return Err(Error::OutOfRange);
        }
        if u64::from(self.toc_offset) < DATA_START
            || u64::from(self.toc_offset) >= u64::from(self.archive_size)
        {
            return Err(Error::OutOfRange);
        }
        if self.toc_bytes() > limits.max_directory_bytes {
            return Err(Error::LimitExceeded(Limit::DirectoryBytes));
        }
        // An archive with entries needs at least one directory to hold them,
        // and a directory-free archive cannot hold entries.
        if (self.directory_count == 0) != (self.entry_count == 0) {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }

    /// Number of table-of-contents bytes, from the table's offset to the end
    /// of the archive.
    #[must_use]
    pub const fn toc_bytes(&self) -> u64 {
        // `validate` guarantees `toc_offset < archive_size`.
        (self.archive_size as u64).saturating_sub(self.toc_offset as u64)
    }

    /// Whether the writer marked this archive as one of a multi-volume set.
    #[must_use]
    pub const fn is_multi_volume(&self) -> bool {
        self.multi_volume != 0 || self.volume_count > 1 || self.volume_number > 1
    }

    /// Encodes the header, for synthetic test archives.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut bytes = [0u8; HEADER_SIZE];
        bytes[0x00..0x04].copy_from_slice(&SIGNATURE_WORD_1.to_le_bytes());
        bytes[0x04..0x08].copy_from_slice(&SIGNATURE_WORD_2.to_le_bytes());
        bytes[0x0a..0x0c].copy_from_slice(&self.multi_volume.to_le_bytes());
        bytes[0x0c..0x0e].copy_from_slice(&self.entry_count.to_le_bytes());
        bytes[0x0e..0x12].copy_from_slice(&self.datetime.to_le_bytes());
        bytes[0x12..0x16].copy_from_slice(&self.archive_size.to_le_bytes());
        bytes[0x16..0x1a].copy_from_slice(&self.expanded_size.to_le_bytes());
        bytes[0x1e] = self.volume_count;
        bytes[0x1f] = self.volume_number;
        bytes[0x21..0x25].copy_from_slice(&self.split_begin.to_le_bytes());
        bytes[0x25..0x29].copy_from_slice(&self.split_end.to_le_bytes());
        bytes[0x29..0x2d].copy_from_slice(&self.toc_offset.to_le_bytes());
        bytes[0x31..0x33].copy_from_slice(&self.directory_count.to_le_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::{ArchiveHeader, DATA_START, HEADER_SIZE, SIGNATURE};
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use alloc::vec;

    fn sound() -> ArchiveHeader {
        ArchiveHeader {
            entry_count: 2,
            directory_count: 1,
            archive_size: 1024,
            expanded_size: 32,
            datetime: 0x2a2a_2a2a,
            multi_volume: 0,
            volume_count: 1,
            volume_number: 1,
            split_begin: 0,
            split_end: 0,
            toc_offset: 512,
        }
    }

    #[test]
    fn the_signature_constant_matches_the_two_words() {
        let encoded = sound().encode();
        assert_eq!(&encoded[..8], SIGNATURE);
    }

    #[test]
    fn round_trips_through_encode() {
        let header = sound();
        let encoded = header.encode();
        assert_eq!(encoded.len(), HEADER_SIZE);
        assert_eq!(
            ArchiveHeader::parse(&encoded, &Limits::default()).unwrap(),
            header
        );
    }

    #[test]
    fn rejects_a_short_header() {
        let encoded = sound().encode();
        assert_eq!(
            ArchiveHeader::parse(&encoded[..HEADER_SIZE - 1], &Limits::default()),
            Err(Error::Truncated)
        );
    }

    #[test]
    fn rejects_a_wrong_first_signature_word() {
        let mut encoded = sound().encode();
        encoded[0] ^= 0xff;
        assert_eq!(
            ArchiveHeader::parse(&encoded, &Limits::default()),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn rejects_a_wrong_second_signature_word() {
        let mut encoded = sound().encode();
        encoded[4] ^= 0xff;
        assert_eq!(
            ArchiveHeader::parse(&encoded, &Limits::default()),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn rejects_a_toc_offset_at_or_past_the_archive_end() {
        let mut header = sound();
        header.toc_offset = header.archive_size;
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &Limits::default()),
            Err(Error::OutOfRange)
        );
    }

    #[test]
    fn rejects_a_toc_offset_before_the_data_start() {
        let mut header = sound();
        header.toc_offset = u32::try_from(DATA_START).expect("the data start fits a u32") - 1;
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &Limits::default()),
            Err(Error::OutOfRange)
        );
    }

    #[test]
    fn rejects_an_archive_shorter_than_the_data_start() {
        let mut header = sound();
        header.archive_size = 8;
        header.toc_offset = 4;
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &Limits::default()),
            Err(Error::OutOfRange)
        );
    }

    #[test]
    fn rejects_entries_without_directories() {
        let mut header = sound();
        header.directory_count = 0;
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &Limits::default()),
            Err(Error::InvalidInput)
        );
    }

    #[test]
    fn rejects_directories_without_entries() {
        let mut header = sound();
        header.entry_count = 0;
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &Limits::default()),
            Err(Error::InvalidInput)
        );
    }

    #[test]
    fn rejects_an_archive_larger_than_the_limit() {
        let header = sound();
        let limits = Limits {
            max_archive_bytes: 512,
            ..Limits::default()
        };
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &limits),
            Err(Error::LimitExceeded(Limit::ArchiveBytes))
        );
    }

    #[test]
    fn rejects_more_directories_than_the_limit() {
        let mut header = sound();
        header.directory_count = 40;
        let limits = Limits {
            max_directories: 8,
            ..Limits::default()
        };
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &limits),
            Err(Error::LimitExceeded(Limit::Directories))
        );
    }

    #[test]
    fn rejects_more_entries_than_the_limit() {
        let mut header = sound();
        header.entry_count = 40;
        let limits = Limits {
            max_entries: 8,
            ..Limits::default()
        };
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &limits),
            Err(Error::LimitExceeded(Limit::Entries))
        );
    }

    #[test]
    fn rejects_a_table_of_contents_larger_than_the_limit() {
        let header = sound();
        let limits = Limits {
            max_directory_bytes: 16,
            ..Limits::default()
        };
        assert_eq!(
            ArchiveHeader::parse(&header.encode(), &limits),
            Err(Error::LimitExceeded(Limit::DirectoryBytes))
        );
    }

    #[test]
    fn detects_a_multi_volume_marker() {
        let mut header = sound();
        assert!(!header.is_multi_volume());
        header.multi_volume = 1;
        assert!(header.is_multi_volume());
    }

    #[test]
    fn an_all_zero_buffer_is_not_an_archive() {
        let zeroes = vec![0u8; HEADER_SIZE];
        assert_eq!(
            ArchiveHeader::parse(&zeroes, &Limits::default()),
            Err(Error::BadSignature)
        );
    }
}
