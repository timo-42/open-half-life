//! Quake `PACK` archive directory parsing.
//!
//! See `docs/FORMAT_SOURCES.md` ("Quake PAK archives") for the public
//! documentation this module was implemented from.
//!
//! A PAK file is a 12-byte header (`"PACK"` magic, a directory offset and a
//! directory size, both `u32` little-endian byte counts) followed by however
//! many 64-byte directory entries the header's directory size implies. Each
//! entry is a 56-byte NUL-terminated name followed by a `u32` offset and a
//! `u32` size (both relative to the start of the file), naming a byte range
//! that holds one member file's raw (uncompressed) contents.
//!
//! This module only validates and indexes the directory: it never
//! interprets a member's contents (that is `bsp30`, `wad3`, `mdl10`, `spr`,
//! or a future audio decoder's job once the caller has extracted the
//! member's bytes).
//!
//! Two entry points cover the two ways a caller may have the directory
//! bytes available:
//!
//! - [`Directory::parse`] borrows the whole file as one in-memory byte
//!   slice (used by this crate's own tests, the fuzz target, and any small
//!   or memory-mapped PAK).
//! - [`Directory::from_parts`] accepts only the already-read header fields
//!   and the directory bytes on their own, so a caller streaming a large PAK
//!   from disk via its own bounded `read_at`-style reads never has to load
//!   the whole archive into memory just to build the directory.

mod limits;
mod raw;

pub use limits::Limits;
pub use raw::{MAGIC, NAME_LEN};

use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::util::{prefix_of, slice_of, sub_slice};
use raw::{RawEntry, RawHeader};

/// The on-disk size of the fixed header.
pub const HEADER_LEN: usize = core::mem::size_of::<RawHeader>();
/// The on-disk size of one directory entry.
pub const ENTRY_LEN: usize = core::mem::size_of::<RawEntry>();

/// One validated, owned directory entry.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// The NUL-padded 56-byte name field, exactly as stored on disk.
    pub name: [u8; NAME_LEN],
    /// The byte offset of this entry's contents, relative to the start of
    /// the archive.
    pub offset: u32,
    /// The byte length of this entry's contents.
    pub size: u32,
}

impl Entry {
    /// The name trimmed at its NUL terminator (validated present by
    /// [`Directory::parse`] / [`Directory::from_parts`]).
    #[must_use]
    pub fn trimmed_name(&self) -> &[u8] {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        &self.name[..len]
    }

    /// Case-insensitive comparison of the trimmed name against `name`.
    #[must_use]
    pub fn name_matches(&self, name: &str) -> bool {
        self.trimmed_name().eq_ignore_ascii_case(name.as_bytes())
    }
}

/// A validated PAK directory: every entry's name is NUL-terminated within
/// its 56-byte field and every entry's `offset`/`size` range is checked
/// against the archive's total length.
///
/// When the source directory lists the same name more than once, the first
/// occurrence wins: [`Directory::find`] always returns the first match, and
/// [`Directory::entries`] preserves on-disk order so callers building their
/// own first-wins index see duplicates in the same deterministic order.
#[derive(Debug, Clone)]
pub struct Directory {
    entries: Vec<Entry>,
}

fn validate_entries(
    raw_entries: &[RawEntry],
    total_len: u64,
    limits: &Limits,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        // Reject a name with no NUL terminator anywhere in its 56-byte
        // field: a real PAK always terminates the name, so an unterminated
        // field indicates a malformed or hostile directory.
        if !raw.name.contains(&0) {
            return Err(FormatError::InvalidText);
        }
        let size = raw.size.get();
        if size > limits.max_entry_bytes {
            return Err(FormatError::LimitExceeded);
        }
        let offset = u64::from(raw.offset.get());
        let end = offset
            .checked_add(u64::from(size))
            .ok_or(FormatError::OutOfBounds)?;
        if end > total_len {
            return Err(FormatError::OutOfBounds);
        }
        entries.push(Entry {
            name: raw.name,
            offset: raw.offset.get(),
            size,
        });
    }
    Ok(entries)
}

fn entry_count(dir_size: u32, limits: &Limits) -> Result<usize> {
    let dir_size = dir_size as usize;
    if !dir_size.is_multiple_of(ENTRY_LEN) {
        return Err(FormatError::SizeNotMultiple);
    }
    let num_entries = dir_size / ENTRY_LEN;
    if num_entries > limits.max_entries {
        return Err(FormatError::LimitExceeded);
    }
    Ok(num_entries)
}

impl Directory {
    /// Parses a whole in-memory PAK file: validates the header, the
    /// directory's bounds within `data`, the entry count (directory size
    /// divided by 64 bytes, rejecting a remainder), and every entry's name
    /// and byte range.
    pub fn parse(data: &[u8], limits: &Limits) -> Result<Self> {
        let (header, _): (&RawHeader, _) = prefix_of(data)?;
        if header.magic != MAGIC {
            return Err(FormatError::BadSignature);
        }
        let num_entries = entry_count(header.dir_size.get(), limits)?;
        let dir_offset = header.dir_offset.get() as usize;
        let dir_bytes_len = num_entries
            .checked_mul(ENTRY_LEN)
            .ok_or(FormatError::OutOfBounds)?;
        let dir_bytes = sub_slice(data, dir_offset, dir_bytes_len)?;
        let raw_entries = slice_of::<RawEntry>(dir_bytes)?;
        let entries = validate_entries(raw_entries, data.len() as u64, limits)?;
        Ok(Self { entries })
    }

    /// Parses a directory from only its already-read header fields and
    /// directory bytes, without requiring the whole archive in memory.
    ///
    /// `total_len` is the archive's total byte length (used to bound every
    /// entry's `offset + size`); `dir_size` must equal `dir_bytes.len()`
    /// (the caller is expected to have read exactly that many bytes from
    /// `dir_offset`, which this function does not otherwise need since it
    /// never re-derives an absolute position).
    pub fn from_parts(
        total_len: u64,
        dir_size: u32,
        dir_bytes: &[u8],
        limits: &Limits,
    ) -> Result<Self> {
        let num_entries = entry_count(dir_size, limits)?;
        let expected_len = num_entries
            .checked_mul(ENTRY_LEN)
            .ok_or(FormatError::OutOfBounds)?;
        if dir_bytes.len() != expected_len {
            return Err(FormatError::Truncated);
        }
        let raw_entries = slice_of::<RawEntry>(dir_bytes)?;
        let entries = validate_entries(raw_entries, total_len, limits)?;
        Ok(Self { entries })
    }

    /// The number of directory entries (including any duplicate names).
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the directory is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over every directory entry in on-disk order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    /// Finds a directory entry by case-insensitive name. When the
    /// directory lists the name more than once, the first occurrence wins.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name_matches(name))
    }
}

/// Reads a `RawHeader` from the fixed 12 header bytes, validating the
/// magic. Exposed so a caller streaming a PAK from disk (via its own
/// bounded `read_at`) can validate the header and learn `dir_offset`/
/// `dir_size` before deciding how many directory bytes to read.
pub fn parse_header(header_bytes: &[u8; HEADER_LEN]) -> Result<(u32, u32)> {
    let (header, _): (&RawHeader, _) = prefix_of(header_bytes)?;
    if header.magic != MAGIC {
        return Err(FormatError::BadSignature);
    }
    Ok((header.dir_offset.get(), header.dir_size.get()))
}
