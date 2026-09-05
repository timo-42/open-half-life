//! File descriptors and their flag words.

use crate::bytes::Cursor;
use crate::error::FormatError;

/// The file is split across two or more volumes.
pub const FILE_SPLIT: u16 = 1;
/// The file's bytes are obfuscated with the rotate/XOR keystream.
pub const FILE_OBFUSCATED: u16 = 2;
/// The file's bytes are stored as length-prefixed raw DEFLATE chunks.
pub const FILE_COMPRESSED: u16 = 4;
/// The descriptor is a placeholder and holds no extractable data.
pub const FILE_INVALID: u16 = 8;

/// Encoded size of an InstallShield 5 file descriptor without its digest.
pub const FILE_DESCRIPTOR_SIZE_V5: usize = 0x2a;
/// Encoded size of an InstallShield 5 file descriptor including its digest.
pub const FILE_DESCRIPTOR_SIZE_V5_MD5: usize = 0x3a;
/// Encoded size of an InstallShield 6 and later file descriptor.
pub const FILE_DESCRIPTOR_SIZE_V6: usize = 0x57;

/// The descriptor's flag word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileFlags(pub u16);

impl FileFlags {
    /// Whether [`FILE_SPLIT`] is set.
    #[must_use]
    pub const fn is_split(self) -> bool {
        self.0 & FILE_SPLIT != 0
    }
    /// Whether [`FILE_OBFUSCATED`] is set.
    #[must_use]
    pub const fn is_obfuscated(self) -> bool {
        self.0 & FILE_OBFUSCATED != 0
    }
    /// Whether [`FILE_COMPRESSED`] is set.
    #[must_use]
    pub const fn is_compressed(self) -> bool {
        self.0 & FILE_COMPRESSED != 0
    }
    /// Whether [`FILE_INVALID`] is set.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.0 & FILE_INVALID != 0
    }
    /// Returns the flags with [`FILE_SPLIT`] set.
    #[must_use]
    pub const fn with_split(self) -> Self {
        Self(self.0 | FILE_SPLIT)
    }
}

/// No split link is recorded.
pub const LINK_NONE: u8 = 0;
/// The descriptor continues an earlier descriptor.
pub const LINK_PREV: u8 = 1;
/// The descriptor is continued by a later descriptor.
pub const LINK_NEXT: u8 = 2;

/// The descriptor's split-link flag byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LinkFlags(pub u8);

impl LinkFlags {
    /// Whether [`LINK_PREV`] is set.
    #[must_use]
    pub const fn has_previous(self) -> bool {
        self.0 & LINK_PREV != 0
    }
    /// Whether [`LINK_NEXT`] is set.
    #[must_use]
    pub const fn has_next(self) -> bool {
        self.0 & LINK_NEXT != 0
    }
    /// Whether no link is recorded.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == LINK_NONE
    }
}

/// One parsed file descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileDescriptor {
    /// Name offset, relative to the file table.
    pub name_offset: u32,
    /// Index into the directory table.
    pub directory_index: u32,
    /// Storage flags.
    pub flags: FileFlags,
    /// Expanded (extracted) size in bytes.
    pub expanded_size: u64,
    /// Stored size in bytes.
    pub compressed_size: u64,
    /// Offset of the file's bytes within its volume.
    pub data_offset: u64,
    /// Recorded MD5 digest of the expanded bytes; all zero when absent.
    pub md5: [u8; 16],
    /// Volume number holding the first stored byte.
    pub volume: u16,
    /// File index this descriptor continues, when
    /// [`LinkFlags::has_previous`].
    pub link_previous: u32,
    /// File index continuing this descriptor, when [`LinkFlags::has_next`].
    pub link_next: u32,
    /// Split-link flags.
    pub link_flags: LinkFlags,
}

impl FileDescriptor {
    /// Parses an InstallShield 5 descriptor starting at the cursor.
    ///
    /// `with_md5` selects the 0x3a-byte form used when the major version is
    /// exactly 5; the untagged (major 0) form is 0x2a bytes.
    pub(crate) fn parse_v5(
        cursor: &mut Cursor<'_>,
        with_md5: bool,
        volume: u16,
    ) -> Result<Self, FormatError> {
        let start = cursor.position();
        let name_offset = cursor.u32()?;
        let directory_index = cursor.u32()?;
        let flags = FileFlags(cursor.u16()?);
        let expanded_size = u64::from(cursor.u32()?);
        let compressed_size = u64::from(cursor.u32()?);
        cursor.skip(0x14)?;
        let data_offset = u64::from(cursor.u32()?);
        debug_assert_eq!(cursor.position() - start, FILE_DESCRIPTOR_SIZE_V5);
        let md5 = if with_md5 {
            let digest = cursor.md5()?;
            debug_assert_eq!(cursor.position() - start, FILE_DESCRIPTOR_SIZE_V5_MD5);
            digest
        } else {
            [0u8; 16]
        };

        Ok(Self {
            name_offset,
            directory_index,
            flags,
            expanded_size,
            compressed_size,
            data_offset,
            md5,
            volume,
            link_previous: 0,
            link_next: 0,
            link_flags: LinkFlags(LINK_NONE),
        })
    }

    /// Parses an InstallShield 6 and later descriptor starting at the cursor.
    pub(crate) fn parse_v6(cursor: &mut Cursor<'_>) -> Result<Self, FormatError> {
        let start = cursor.position();
        let flags = FileFlags(cursor.u16()?);
        let expanded_size = cursor.u64()?;
        let compressed_size = cursor.u64()?;
        let data_offset = cursor.u64()?;
        let md5 = cursor.md5()?;
        cursor.skip(0x10)?;
        let name_offset = cursor.u32()?;
        let directory_index = u32::from(cursor.u16()?);
        debug_assert_eq!(cursor.position() - start, 0x40);
        cursor.skip(0x0c)?;
        let link_previous = cursor.u32()?;
        let link_next = cursor.u32()?;
        let link_flags = LinkFlags(cursor.u8()?);
        let volume = cursor.u16()?;
        debug_assert_eq!(cursor.position() - start, FILE_DESCRIPTOR_SIZE_V6);

        Ok(Self {
            name_offset,
            directory_index,
            flags,
            expanded_size,
            compressed_size,
            data_offset,
            md5,
            volume,
            link_previous,
            link_next,
            link_flags,
        })
    }

    /// Whether the descriptor names extractable bytes.
    #[must_use]
    pub const fn is_extractable(&self) -> bool {
        !self.flags.is_invalid() && self.name_offset != 0 && self.data_offset != 0
    }

    /// The number of stored bytes to read for this descriptor.
    #[must_use]
    pub const fn stored_size(&self) -> u64 {
        if self.flags.is_compressed() {
            self.compressed_size
        } else {
            self.expanded_size
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileFlags, LinkFlags};

    #[test]
    fn flag_predicates_match_their_bits() {
        let flags = FileFlags(0b1111);
        assert!(flags.is_split() && flags.is_obfuscated());
        assert!(flags.is_compressed() && flags.is_invalid());
        assert!(!FileFlags(0).is_split());
        assert!(FileFlags(0).with_split().is_split());
    }

    #[test]
    fn link_predicates_match_their_bits() {
        assert!(LinkFlags(1).has_previous());
        assert!(LinkFlags(2).has_next());
        assert!(LinkFlags(3).has_previous() && LinkFlags(3).has_next());
        assert!(LinkFlags(0).is_none());
    }
}
