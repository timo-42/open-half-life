//! The table of contents: directory records followed by entry records.
//!
//! Record layouts (little-endian, no padding); see `docs/FORMAT_SOURCES.md`.
//!
//! Directory record:
//!
//! | offset | size | field                                   |
//! |-------:|-----:|-----------------------------------------|
//! |   0x00 |    2 | number of entries in this directory     |
//! |   0x02 |    2 | record size, including trailing padding |
//! |   0x04 |    2 | name length in bytes                    |
//! |   0x06 |    n | name bytes                              |
//!
//! Entry record:
//!
//! | offset | size | field                                   |
//! |-------:|-----:|-----------------------------------------|
//! |   0x00 |    1 | last volume holding this entry's bytes  |
//! |   0x01 |    2 | entry index                             |
//! |   0x03 |    4 | expanded size                           |
//! |   0x07 |    4 | stored size                             |
//! |   0x0b |    4 | offset of the entry's bytes             |
//! |   0x0f |    4 | MS-DOS packed date/time                 |
//! |   0x13 |    4 | reserved                                |
//! |   0x17 |    2 | record size, including trailing padding |
//! |   0x19 |    1 | attribute flags                         |
//! |   0x1a |    1 | non-zero when the entry is split        |
//! |   0x1b |    1 | reserved                                |
//! |   0x1c |    1 | first volume holding this entry's bytes |
//! |   0x1d |    1 | name length in bytes                    |
//! |   0x1e |    n | name bytes                              |

use alloc::vec::Vec;
use core::fmt;

use crate::bytes::{le16, le32};
use crate::error::{Error, Limit, Result};
use crate::header::{ArchiveHeader, DATA_START};
use crate::limits::Limits;
use crate::source::{Cancellation, check_cancelled};

/// Fixed part of a directory record, up to and including the name length.
pub const DIRECTORY_RECORD_FIXED: usize = 6;
/// Fixed part of an entry record, up to and including the name length.
pub const ENTRY_RECORD_FIXED: usize = 30;

/// Attribute bit marking an entry as stored rather than imploded.
pub const ATTRIBUTE_UNCOMPRESSED: u8 = 0x10;

/// A bounded, opaque name taken from the archive.
///
/// The bytes are the writer's original 8.3-era OEM bytes. This crate never
/// interprets, transcodes, formats or logs them: `Debug` prints only the
/// length, so a name can never leak into a diagnostic by accident. Callers
/// that must display a name are responsible for sanitizing it themselves.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Name {
    bytes: Vec<u8>,
}

impl Name {
    /// Wraps `bytes` as a name, rejecting anything longer than
    /// `limits.max_name_bytes`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LimitExceeded`] when the name is too long.
    pub fn new(bytes: &[u8], limits: &Limits) -> Result<Self> {
        if bytes.len() > limits.max_name_bytes as usize {
            return Err(Error::LimitExceeded(Limit::NameBytes));
        }
        Ok(Self {
            bytes: Vec::from(bytes),
        })
    }

    /// The raw name bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The name's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the name is empty, which the root directory record uses.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The lowercased bytes after the last `.`, or an empty slice when the
    /// name carries no extension. Used for classification only.
    #[must_use]
    pub fn extension_bytes(&self) -> Vec<u8> {
        let dot = self.bytes.iter().rposition(|byte| *byte == b'.');
        match dot {
            Some(at) => self.bytes[at + 1..].to_ascii_lowercase(),
            None => Vec::new(),
        }
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately redacted: never print archive-derived name bytes.
        write!(f, "Name({} bytes)", self.bytes.len())
    }
}

/// One directory record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    /// The directory's name, `\`-separated on disk and possibly empty.
    pub name: Name,
    /// Number of entry records that belong to this directory.
    pub entry_count: u16,
}

/// One entry record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The entry's own name, without any directory part.
    pub name: Name,
    /// Index of the [`Directory`] this entry belongs to.
    pub directory_index: u32,
    /// The writer's own index for this entry.
    pub index: u16,
    /// Size after expansion.
    pub expanded_size: u32,
    /// Size as stored inside the archive.
    pub stored_size: u32,
    /// Offset of the entry's bytes, relative to the archive base.
    pub offset: u32,
    /// MS-DOS packed date/time, kept as an opaque number.
    pub datetime: u32,
    /// Attribute flags; see [`ATTRIBUTE_UNCOMPRESSED`].
    pub attributes: u8,
    /// Whether the writer marked this entry as split across volumes.
    pub split: bool,
    /// First volume holding this entry's bytes.
    pub volume_start: u8,
    /// Last volume holding this entry's bytes.
    pub volume_end: u8,
}

impl Entry {
    /// Whether the entry's bytes are stored verbatim rather than imploded.
    #[must_use]
    pub const fn is_stored(&self) -> bool {
        self.attributes & ATTRIBUTE_UNCOMPRESSED != 0
    }

    /// Whether the entry claims bytes in more than one volume.
    #[must_use]
    pub const fn spans_volumes(&self) -> bool {
        self.split || self.volume_start != self.volume_end
    }
}

/// The parsed table of contents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableOfContents {
    directories: Vec<Directory>,
    entries: Vec<Entry>,
}

impl TableOfContents {
    /// Every directory record, in archive order.
    #[must_use]
    pub fn directories(&self) -> &[Directory] {
        &self.directories
    }

    /// Every entry record, in archive order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The directory at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when `index` is out of range.
    pub fn directory(&self, index: u32) -> Result<&Directory> {
        self.directories.get(index as usize).ok_or(Error::NotFound)
    }

    /// The entry at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] when `index` is out of range.
    pub fn entry(&self, index: u32) -> Result<&Entry> {
        self.entries.get(index as usize).ok_or(Error::NotFound)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// Returns the `len` bytes of the record starting at the cursor, without
    /// advancing, rejecting a record that runs off the end of the buffer.
    fn peek(&self, len: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(len).ok_or(Error::OutOfRange)?;
        self.bytes.get(self.at..end).ok_or(Error::Truncated)
    }

    /// Advances by a record size that must cover `minimum` and stay inside
    /// the buffer.
    fn advance(&mut self, record_size: u16, minimum: usize) -> Result<()> {
        let record_size = record_size as usize;
        if record_size < minimum {
            return Err(Error::InvalidInput);
        }
        let next = self.at.checked_add(record_size).ok_or(Error::OutOfRange)?;
        if next > self.bytes.len() {
            return Err(Error::Truncated);
        }
        self.at = next;
        Ok(())
    }
}

/// Parses the table of contents out of `toc`, which must be exactly the
/// archive bytes from `header.toc_offset` to the end of the archive.
///
/// # Errors
///
/// Returns a sanitized [`Error`] for any inconsistent count, record size,
/// name length, offset or extent, and [`Error::Cancelled`] when `cancel` is
/// signalled between records.
pub fn parse<C: Cancellation + ?Sized>(
    toc: &[u8],
    header: &ArchiveHeader,
    limits: &Limits,
    cancel: &C,
) -> Result<TableOfContents> {
    limits.validate()?;
    if toc.len() as u64 != header.toc_bytes() {
        return Err(Error::InvalidInput);
    }

    let mut cursor = Cursor::new(toc);
    let mut directories = Vec::new();
    let mut declared_entries = 0u32;

    for _ in 0..header.directory_count {
        check_cancelled(cancel)?;
        let fixed = cursor.peek(DIRECTORY_RECORD_FIXED)?;
        let entry_count = le16(fixed, 0x00);
        let record_size = le16(fixed, 0x02);
        let name_len = le16(fixed, 0x04) as usize;
        if name_len > limits.max_name_bytes as usize {
            return Err(Error::LimitExceeded(Limit::NameBytes));
        }
        let minimum = DIRECTORY_RECORD_FIXED
            .checked_add(name_len)
            .ok_or(Error::OutOfRange)?;
        let record = cursor.peek(minimum)?;
        let name = Name::new(&record[DIRECTORY_RECORD_FIXED..minimum], limits)?;
        cursor.advance(record_size, minimum)?;

        declared_entries = declared_entries
            .checked_add(u32::from(entry_count))
            .ok_or(Error::OutOfRange)?;
        if declared_entries > limits.max_entries {
            return Err(Error::LimitExceeded(Limit::Entries));
        }
        directories.push(Directory { name, entry_count });
    }

    if declared_entries != u32::from(header.entry_count) {
        return Err(Error::InvalidInput);
    }

    let mut entries = Vec::new();
    for (directory_index, directory) in directories.iter().enumerate() {
        for _ in 0..directory.entry_count {
            check_cancelled(cancel)?;
            let fixed = cursor.peek(ENTRY_RECORD_FIXED)?;
            let name_len = fixed[0x1d] as usize;
            if name_len > limits.max_name_bytes as usize {
                return Err(Error::LimitExceeded(Limit::NameBytes));
            }
            let minimum = ENTRY_RECORD_FIXED
                .checked_add(name_len)
                .ok_or(Error::OutOfRange)?;
            let record = cursor.peek(minimum)?;
            let record_size = le16(record, 0x17);
            let entry = Entry {
                name: Name::new(&record[ENTRY_RECORD_FIXED..minimum], limits)?,
                directory_index: u32::try_from(directory_index).map_err(|_| Error::OutOfRange)?,
                index: le16(record, 0x01),
                expanded_size: le32(record, 0x03),
                stored_size: le32(record, 0x07),
                offset: le32(record, 0x0b),
                datetime: le32(record, 0x0f),
                attributes: record[0x19],
                split: record[0x1a] != 0,
                volume_start: record[0x1c],
                volume_end: record[0x00],
            };
            validate_entry(&entry, header, limits)?;
            cursor.advance(record_size, minimum)?;
            entries.push(entry);
        }
    }

    Ok(TableOfContents {
        directories,
        entries,
    })
}

fn validate_entry(entry: &Entry, header: &ArchiveHeader, limits: &Limits) -> Result<()> {
    if u64::from(entry.stored_size) > limits.max_stored_bytes_per_entry {
        return Err(Error::LimitExceeded(Limit::StoredBytesPerEntry));
    }
    if u64::from(entry.expanded_size) > limits.max_expanded_bytes_per_entry {
        return Err(Error::LimitExceeded(Limit::ExpandedBytesPerEntry));
    }
    if u64::from(entry.offset) < DATA_START {
        return Err(Error::OutOfRange);
    }
    let end = u64::from(entry.offset)
        .checked_add(u64::from(entry.stored_size))
        .ok_or(Error::OutOfRange)?;
    if end > u64::from(header.archive_size) {
        return Err(Error::OutOfRange);
    }
    if entry.is_stored() && entry.stored_size != entry.expanded_size {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ATTRIBUTE_UNCOMPRESSED, Name};
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use alloc::vec;

    #[test]
    fn a_name_is_redacted_in_debug_output() {
        let name = Name::new(b"secret.txt", &Limits::default()).unwrap();
        let rendered = alloc::format!("{name:?}");
        assert_eq!(rendered, "Name(10 bytes)");
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn a_name_longer_than_the_limit_is_rejected() {
        let limits = Limits {
            max_name_bytes: 4,
            ..Limits::default()
        };
        assert_eq!(
            Name::new(b"toolong", &limits).err(),
            Some(Error::LimitExceeded(Limit::NameBytes))
        );
    }

    #[test]
    fn extensions_are_lowercased_and_optional() {
        let limits = Limits::default();
        assert_eq!(
            Name::new(b"MAP.BSP", &limits).unwrap().extension_bytes(),
            b"bsp".to_vec()
        );
        assert!(
            Name::new(b"noextension", &limits)
                .unwrap()
                .extension_bytes()
                .is_empty()
        );
        assert!(
            Name::new(b"trailing.", &limits)
                .unwrap()
                .extension_bytes()
                .is_empty()
        );
    }

    #[test]
    fn an_empty_name_is_allowed_for_the_root_directory() {
        let name = Name::new(b"", &Limits::default()).unwrap();
        assert!(name.is_empty());
        assert_eq!(name.len(), 0);
        assert_eq!(name.as_bytes(), b"");
    }

    #[test]
    fn the_uncompressed_attribute_bit_is_the_documented_one() {
        assert_eq!(ATTRIBUTE_UNCOMPRESSED, 0x10);
        assert_eq!(vec![ATTRIBUTE_UNCOMPRESSED], vec![16u8]);
    }
}
