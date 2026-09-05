//! Anti-abuse ceilings applied to every walk over untrusted archive data.
//!
//! Every count, offset and length read out of an archive is checked against
//! one of these before it is used to size an allocation, index a buffer or
//! bound a loop. A caller may lower a ceiling; raising one above its hard
//! maximum, or setting one to zero, is rejected rather than silently
//! removing a bound.

use crate::error::{Error, Result};

/// Default staging buffer for one decode step.
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;

/// Ceilings applied to signature scanning, parsing and extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest number of source bytes one [`crate::find_signature`] call may
    /// read.
    pub max_scan_bytes: u64,
    /// Largest accepted archive length, taken from the header's own
    /// archive-size field.
    pub max_archive_bytes: u64,
    /// Largest accepted directory count.
    pub max_directories: u32,
    /// Largest accepted entry count.
    pub max_entries: u32,
    /// Largest number of table-of-contents bytes buffered while listing.
    pub max_directory_bytes: u64,
    /// Largest accepted name length, in bytes, for one directory or entry.
    pub max_name_bytes: u32,
    /// Largest accepted stored (in-archive) size for one entry.
    pub max_stored_bytes_per_entry: u64,
    /// Largest accepted expanded size for one entry.
    pub max_expanded_bytes_per_entry: u64,
    /// Largest total expanded size across one reader's lifetime.
    pub max_total_expanded_bytes: u64,
    /// Staging buffer size, and the ceiling on one decode step's input.
    pub max_chunk_bytes: usize,
}

impl Limits {
    /// Hard ceiling on [`Self::max_scan_bytes`].
    pub const HARD_MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_archive_bytes`].
    pub const HARD_MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_directories`]. The on-disk field is a
    /// `u16`, so nothing above this is representable anyway.
    pub const HARD_MAX_DIRECTORIES: u32 = 65_535;
    /// Hard ceiling on [`Self::max_entries`]. The on-disk field is a `u16`.
    pub const HARD_MAX_ENTRIES: u32 = 65_535;
    /// Hard ceiling on [`Self::max_directory_bytes`].
    pub const HARD_MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_name_bytes`]. Directory names are
    /// `u16`-prefixed and entry names `u8`-prefixed on disk.
    pub const HARD_MAX_NAME_BYTES: u32 = 1_024;
    /// Hard ceiling on [`Self::max_stored_bytes_per_entry`].
    pub const HARD_MAX_STORED_BYTES_PER_ENTRY: u64 = 4 * 1024 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_expanded_bytes_per_entry`].
    pub const HARD_MAX_EXPANDED_BYTES_PER_ENTRY: u64 = 4 * 1024 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_total_expanded_bytes`].
    pub const HARD_MAX_TOTAL_EXPANDED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
    /// Hard ceiling on [`Self::max_chunk_bytes`].
    pub const HARD_MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;

    /// The default ceilings, also returned by [`Default::default`].
    pub const DEFAULT: Self = Self {
        max_scan_bytes: 2 * 1024 * 1024 * 1024,
        max_archive_bytes: 4 * 1024 * 1024 * 1024,
        max_directories: 8_192,
        max_entries: 65_535,
        max_directory_bytes: 8 * 1024 * 1024,
        max_name_bytes: 260,
        max_stored_bytes_per_entry: 1024 * 1024 * 1024,
        max_expanded_bytes_per_entry: 2 * 1024 * 1024 * 1024,
        max_total_expanded_bytes: 8 * 1024 * 1024 * 1024,
        max_chunk_bytes: DEFAULT_CHUNK_BYTES,
    };

    /// Rejects a ceiling that is zero or above its hard maximum.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for any out-of-range field.
    pub const fn validate(&self) -> Result<()> {
        let ok = self.max_scan_bytes != 0
            && self.max_scan_bytes <= Self::HARD_MAX_SCAN_BYTES
            && self.max_archive_bytes != 0
            && self.max_archive_bytes <= Self::HARD_MAX_ARCHIVE_BYTES
            && self.max_directories != 0
            && self.max_directories <= Self::HARD_MAX_DIRECTORIES
            && self.max_entries != 0
            && self.max_entries <= Self::HARD_MAX_ENTRIES
            && self.max_directory_bytes != 0
            && self.max_directory_bytes <= Self::HARD_MAX_DIRECTORY_BYTES
            && self.max_name_bytes != 0
            && self.max_name_bytes <= Self::HARD_MAX_NAME_BYTES
            && self.max_stored_bytes_per_entry != 0
            && self.max_stored_bytes_per_entry <= Self::HARD_MAX_STORED_BYTES_PER_ENTRY
            && self.max_expanded_bytes_per_entry != 0
            && self.max_expanded_bytes_per_entry <= Self::HARD_MAX_EXPANDED_BYTES_PER_ENTRY
            && self.max_total_expanded_bytes != 0
            && self.max_total_expanded_bytes <= Self::HARD_MAX_TOTAL_EXPANDED_BYTES
            && self.max_chunk_bytes != 0
            && self.max_chunk_bytes <= Self::HARD_MAX_CHUNK_BYTES;
        if ok { Ok(()) } else { Err(Error::InvalidInput) }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;
    use crate::error::Error;

    #[test]
    fn the_default_limits_validate() {
        assert_eq!(Limits::default().validate(), Ok(()));
    }

    #[test]
    fn a_zero_ceiling_is_rejected() {
        let limits = Limits {
            max_entries: 0,
            ..Limits::default()
        };
        assert_eq!(limits.validate(), Err(Error::InvalidInput));
    }

    #[test]
    fn a_ceiling_above_its_hard_maximum_is_rejected() {
        let limits = Limits {
            max_name_bytes: Limits::HARD_MAX_NAME_BYTES + 1,
            ..Limits::default()
        };
        assert_eq!(limits.validate(), Err(Error::InvalidInput));
    }
}
