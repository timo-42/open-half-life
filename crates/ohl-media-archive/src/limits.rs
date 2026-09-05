//! Anti-abuse ceilings applied to every walk over untrusted directory data.
//!
//! The values mirror the ceilings the C++ facade already enforces so that the
//! Rust readers cannot be made more permissive by accident. Callers may lower
//! a ceiling before mounting; raising one above its hard maximum, or setting
//! one to zero, is rejected rather than silently removing a bound.

use ohl_core::SanitizedError;

/// Bounded directory-enumeration ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryLimits {
    /// Maximum number of path components in one normalized path.
    pub max_path_components: u32,
    /// Maximum entries returned by one page.
    pub max_page_entries: u32,
    /// Maximum decoded name bytes accumulated in one page.
    pub max_page_name_bytes: u64,
    /// Maximum pages one cursor may produce.
    pub max_page_count: u32,
    /// Maximum entries one enumeration may return across all its pages.
    pub max_total_entries: u64,
    /// Maximum decoded name bytes accepted from one directory record.
    pub max_entry_name_bytes: u32,
    /// Maximum bytes one directory extent may span.
    pub max_directory_extent_bytes: u64,
    /// Maximum distinct directories visited while resolving one path, which
    /// also bounds the cycle-detection set.
    pub max_directories_visited: u32,
    /// Maximum bytes a single opened file may buffer when the underlying
    /// reader cannot stream extents.
    pub max_buffered_file_bytes: u64,
}

impl DirectoryLimits {
    /// Hard ceiling on [`Self::max_path_components`].
    pub const HARD_MAX_PATH_COMPONENTS: u32 = 64;
    /// Hard ceiling on [`Self::max_page_entries`].
    pub const HARD_MAX_PAGE_ENTRIES: u32 = 256;
    /// Hard ceiling on [`Self::max_page_name_bytes`].
    pub const HARD_MAX_PAGE_NAME_BYTES: u64 = 64 * 1_024;
    /// Hard ceiling on [`Self::max_page_count`].
    pub const HARD_MAX_PAGE_COUNT: u32 = 64;
    /// Hard ceiling on [`Self::max_total_entries`].
    pub const HARD_MAX_TOTAL_ENTRIES: u64 = 262_144;
    /// Hard ceiling on [`Self::max_entry_name_bytes`].
    pub const HARD_MAX_ENTRY_NAME_BYTES: u32 = 1_024;
    /// Hard ceiling on [`Self::max_directory_extent_bytes`], which is the
    /// C++ reader's 4,096 logical sectors.
    pub const HARD_MAX_DIRECTORY_EXTENT_BYTES: u64 = 4_096 * 2_048;
    /// Hard ceiling on [`Self::max_directories_visited`].
    pub const HARD_MAX_DIRECTORIES_VISITED: u32 = 4_096;
    /// Hard ceiling on [`Self::max_buffered_file_bytes`].
    pub const HARD_MAX_BUFFERED_FILE_BYTES: u64 = 256 * 1_024 * 1_024;

    /// Rejects limits that are zero or above their hard ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] for any out-of-range field.
    pub const fn validate(&self) -> Result<(), SanitizedError> {
        let ok = self.max_path_components != 0
            && self.max_path_components <= Self::HARD_MAX_PATH_COMPONENTS
            && self.max_page_entries != 0
            && self.max_page_entries <= Self::HARD_MAX_PAGE_ENTRIES
            && self.max_page_name_bytes != 0
            && self.max_page_name_bytes <= Self::HARD_MAX_PAGE_NAME_BYTES
            && self.max_page_count != 0
            && self.max_page_count <= Self::HARD_MAX_PAGE_COUNT
            && self.max_total_entries != 0
            && self.max_total_entries <= Self::HARD_MAX_TOTAL_ENTRIES
            && self.max_entry_name_bytes != 0
            && self.max_entry_name_bytes <= Self::HARD_MAX_ENTRY_NAME_BYTES
            && self.max_directory_extent_bytes != 0
            && self.max_directory_extent_bytes <= Self::HARD_MAX_DIRECTORY_EXTENT_BYTES
            && self.max_directories_visited != 0
            && self.max_directories_visited <= Self::HARD_MAX_DIRECTORIES_VISITED
            && self.max_buffered_file_bytes != 0
            && self.max_buffered_file_bytes <= Self::HARD_MAX_BUFFERED_FILE_BYTES;
        if ok {
            Ok(())
        } else {
            Err(SanitizedError::InvalidInput)
        }
    }
}

impl Default for DirectoryLimits {
    fn default() -> Self {
        Self {
            max_path_components: Self::HARD_MAX_PATH_COMPONENTS,
            max_page_entries: Self::HARD_MAX_PAGE_ENTRIES,
            max_page_name_bytes: Self::HARD_MAX_PAGE_NAME_BYTES,
            max_page_count: Self::HARD_MAX_PAGE_COUNT,
            max_total_entries: Self::HARD_MAX_TOTAL_ENTRIES,
            max_entry_name_bytes: Self::HARD_MAX_ENTRY_NAME_BYTES,
            max_directory_extent_bytes: Self::HARD_MAX_DIRECTORY_EXTENT_BYTES,
            max_directories_visited: Self::HARD_MAX_DIRECTORIES_VISITED,
            max_buffered_file_bytes: Self::HARD_MAX_BUFFERED_FILE_BYTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DirectoryLimits;
    use ohl_core::SanitizedError;

    #[test]
    fn default_limits_validate() {
        assert_eq!(DirectoryLimits::default().validate(), Ok(()));
    }

    #[test]
    fn zero_and_raised_limits_are_rejected() {
        let zero = DirectoryLimits {
            max_page_entries: 0,
            ..DirectoryLimits::default()
        };
        assert_eq!(zero.validate(), Err(SanitizedError::InvalidInput));

        let raised = DirectoryLimits {
            max_total_entries: DirectoryLimits::HARD_MAX_TOTAL_ENTRIES + 1,
            ..DirectoryLimits::default()
        };
        assert_eq!(raised.validate(), Err(SanitizedError::InvalidInput));
    }

    #[test]
    fn lowering_a_limit_is_accepted() {
        let lowered = DirectoryLimits {
            max_page_entries: 2,
            ..DirectoryLimits::default()
        };
        assert_eq!(lowered.validate(), Ok(()));
    }
}
