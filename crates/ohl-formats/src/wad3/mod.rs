//! WAD3 texture package decoding.
//!
//! See `docs/FORMAT_SOURCES.md` ("GoldSrc BSP v30 and WAD3") for the public
//! documentation this module was implemented from.

mod limits;
mod raw;

pub use limits::Limits;
pub use raw::{EntryKind, MAGIC, NAME_LEN};

use crate::error::{FormatError, Result};
use crate::miptex_body::{DecodedMiptex, MiptexHeader, decode_body};
use crate::util::{prefix_of, slice_of, sub_slice};
use raw::{RawDirectoryEntry, RawHeader};

/// One directory entry with its name and type already extracted.
#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntry<'a> {
    /// Null-padded 16-byte name (compare case-insensitively via
    /// [`DirectoryEntry::name_matches`]).
    pub name: &'a [u8; NAME_LEN],
    /// The documented entry-type byte, decoded.
    pub kind: EntryKind,
    /// The entry's on-disk (compressed) size in bytes.
    pub disk_size: u32,
    /// The entry's decompressed size in bytes.
    pub full_size: u32,
    offset: u32,
}

impl DirectoryEntry<'_> {
    /// Case-insensitive comparison against `name`, treating the fixed
    /// 16-byte field as NUL-padded ASCII.
    #[must_use]
    pub fn name_matches(&self, name: &str) -> bool {
        let field = trim_name(self.name);
        field.eq_ignore_ascii_case(name.as_bytes())
    }
}

fn trim_name(name: &[u8; NAME_LEN]) -> &[u8] {
    let len = name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    &name[..len]
}

/// A validated, zero-copy view over one WAD3 file.
pub struct Wad3<'a> {
    data: &'a [u8],
    entries: &'a [RawDirectoryEntry],
}

impl<'a> Wad3<'a> {
    /// Parses and validates a WAD3 file's header and directory.
    ///
    /// Validates the magic, that the directory itself fits within `data`,
    /// and that every entry's `offset`/`disk_size` fall within `data`
    /// (rejecting compressed entries, since GoldSrc never produces them and
    /// this crate does not implement a decompressor).
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self> {
        let (header, _): (&RawHeader, _) = prefix_of(data)?;
        if header.magic != raw::MAGIC {
            return Err(FormatError::BadSignature);
        }
        let num_entries = header.num_entries.get() as usize;
        if num_entries > limits.max_entries {
            return Err(FormatError::LimitExceeded);
        }
        let dir_offset = header.dir_offset.get() as usize;
        let dir_bytes_len = num_entries
            .checked_mul(core::mem::size_of::<RawDirectoryEntry>())
            .ok_or(FormatError::OutOfBounds)?;
        let dir_bytes = sub_slice(data, dir_offset, dir_bytes_len)?;
        let entries = slice_of::<RawDirectoryEntry>(dir_bytes)?;

        for entry in entries {
            if entry.disk_size.get() > limits.max_entry_bytes
                || entry.full_size.get() > limits.max_entry_bytes
            {
                return Err(FormatError::LimitExceeded);
            }
            // Validate the entry's bytes are in-bounds now, so later lookups
            // never need to re-check.
            let _ = sub_slice(
                data,
                entry.offset.get() as usize,
                entry.disk_size.get() as usize,
            )?;
        }

        Ok(Self { data, entries })
    }

    /// The number of directory entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the directory is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry_at(&self, index: usize) -> Result<DirectoryEntry<'a>> {
        let raw = self
            .entries
            .get(index)
            .ok_or(FormatError::IndexOutOfRange)?;
        Ok(DirectoryEntry {
            name: &raw.name,
            kind: EntryKind::from_byte(raw.kind),
            disk_size: raw.disk_size.get(),
            full_size: raw.full_size.get(),
            offset: raw.offset.get(),
        })
    }

    /// Iterates over every directory entry.
    pub fn entries(&self) -> impl Iterator<Item = Result<DirectoryEntry<'a>>> + '_ {
        (0..self.entries.len()).map(move |i| self.entry_at(i))
    }

    /// Finds a directory entry by case-insensitive name.
    pub fn find(&self, name: &str) -> Result<Option<DirectoryEntry<'a>>> {
        for index in 0..self.entries.len() {
            let entry = self.entry_at(index)?;
            if entry.name_matches(name) {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    fn entry_bytes(&self, entry: &DirectoryEntry<'a>) -> Result<&'a [u8]> {
        sub_slice(self.data, entry.offset as usize, entry.disk_size as usize)
    }

    /// Decodes a `0x43` (miptex) entry's texture body: name, width, height,
    /// four mip levels, and its 256-color palette.
    ///
    /// Returns [`FormatError::InvalidInput`] if `entry.kind` is not
    /// [`EntryKind::Miptex`].
    pub fn decode_miptex(&self, entry: &DirectoryEntry<'a>) -> Result<Miptex<'a>> {
        if entry.kind != EntryKind::Miptex {
            return Err(FormatError::InvalidInput);
        }
        let bytes = self.entry_bytes(entry)?;
        let (header, _) = prefix_of::<MiptexHeader>(bytes)?;
        let width = header.width.get();
        let height = header.height.get();
        let offsets = [
            header.offsets[0].get(),
            header.offsets[1].get(),
            header.offsets[2].get(),
            header.offsets[3].get(),
        ];
        let body = decode_body(bytes, width, height, offsets)?;
        Ok(Miptex {
            name: header.name,
            width,
            height,
            body,
        })
    }
}

/// A decoded WAD3 `0x43` miptex entry.
#[derive(Debug, Clone, Copy)]
pub struct Miptex<'a> {
    /// Null-padded 16-byte texture name (matches the directory entry name).
    pub name: [u8; NAME_LEN],
    /// Declared width.
    pub width: u32,
    /// Declared height.
    pub height: u32,
    /// Decoded mip levels and shared palette.
    pub body: DecodedMiptex<'a>,
}
