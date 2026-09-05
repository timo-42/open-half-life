//! The cabinet descriptor: table offsets, counts and the two fixed
//! offset-list arrays.

use crate::bytes::Cursor;
use crate::error::FormatError;

/// Number of entries in each of the two fixed offset arrays.
pub const OFFSET_COUNT: usize = 0x47;

/// Largest file-group count a component may name, per the fixed array size.
pub const MAX_FILE_GROUP_COUNT: u16 = 71;

/// Byte offset of `file_table_offset` within the descriptor.
const FILE_TABLE_OFFSET_AT: usize = 0x0c;

/// Byte offset of the file-group offset array within the descriptor.
const FILE_GROUP_OFFSETS_AT: usize = 0x3e;

/// Smallest descriptor that can hold both fixed offset arrays.
pub const MIN_CAB_DESCRIPTOR_SIZE: usize =
    FILE_GROUP_OFFSETS_AT + 2 * OFFSET_COUNT * core::mem::size_of::<u32>();

/// The parsed cabinet descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabDescriptor {
    /// File table offset, relative to the descriptor.
    pub file_table_offset: u32,
    /// File table size as recorded first.
    pub file_table_size: u32,
    /// File table size as recorded second; real media repeats the value.
    pub file_table_size2: u32,
    /// Number of directory entries at the head of the file table.
    pub directory_count: u32,
    /// Number of file entries.
    pub file_count: u32,
    /// Secondary file table offset, relative to the file table.
    pub file_table_offset2: u32,
    /// Head offsets of the file-group offset lists, relative to the
    /// descriptor. Zero means "empty slot".
    pub file_group_offsets: [u32; OFFSET_COUNT],
    /// Head offsets of the component offset lists, relative to the
    /// descriptor. Zero means "empty slot".
    pub component_offsets: [u32; OFFSET_COUNT],
}

impl CabDescriptor {
    /// Parses a descriptor from `descriptor`, which must be exactly the
    /// descriptor region already bounds-checked against the header buffer.
    pub fn parse(descriptor: &[u8]) -> Result<Self, FormatError> {
        if descriptor.len() < MIN_CAB_DESCRIPTOR_SIZE {
            return Err(FormatError::Truncated);
        }

        let mut cursor = Cursor::at(descriptor, FILE_TABLE_OFFSET_AT)?;
        let file_table_offset = cursor.u32()?;
        cursor.skip(4)?;
        let file_table_size = cursor.u32()?;
        let file_table_size2 = cursor.u32()?;
        let directory_count = cursor.u32()?;
        cursor.skip(8)?;
        let file_count = cursor.u32()?;
        let file_table_offset2 = cursor.u32()?;
        debug_assert_eq!(cursor.position(), 0x30);
        cursor.skip(0x0e)?;
        debug_assert_eq!(cursor.position(), FILE_GROUP_OFFSETS_AT);

        let mut file_group_offsets = [0u32; OFFSET_COUNT];
        for slot in &mut file_group_offsets {
            *slot = cursor.u32()?;
        }
        let mut component_offsets = [0u32; OFFSET_COUNT];
        for slot in &mut component_offsets {
            *slot = cursor.u32()?;
        }

        Ok(Self {
            file_table_offset,
            file_table_size,
            file_table_size2,
            directory_count,
            file_count,
            file_table_offset2,
            file_group_offsets,
            component_offsets,
        })
    }

    /// Whether the two recorded file table sizes agree. Real media may
    /// disagree; callers decide whether that is fatal.
    #[must_use]
    pub const fn file_table_sizes_agree(&self) -> bool {
        self.file_table_size == self.file_table_size2
    }
}

#[cfg(test)]
mod tests {
    use super::{CabDescriptor, MIN_CAB_DESCRIPTOR_SIZE, OFFSET_COUNT};
    use crate::error::FormatError;
    use alloc::vec;

    #[test]
    fn minimum_size_covers_both_offset_arrays() {
        assert_eq!(MIN_CAB_DESCRIPTOR_SIZE, 0x276);
        assert_eq!(OFFSET_COUNT, 71);
    }

    #[test]
    fn rejects_a_descriptor_shorter_than_the_fixed_arrays() {
        let short = vec![0u8; MIN_CAB_DESCRIPTOR_SIZE - 1];
        assert_eq!(CabDescriptor::parse(&short), Err(FormatError::Truncated));
    }

    #[test]
    fn parses_an_all_zero_descriptor() {
        let zeroes = vec![0u8; MIN_CAB_DESCRIPTOR_SIZE];
        let parsed = CabDescriptor::parse(&zeroes).unwrap();
        assert_eq!(parsed.file_count, 0);
        assert!(parsed.file_table_sizes_agree());
    }
}
