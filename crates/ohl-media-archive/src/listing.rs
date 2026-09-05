//! The bounded directory-listing model shared by both readers.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

/// What a directory entry refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryType {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// Something the reader deliberately does not interpret.
    Unknown,
}

/// One listed entry. `name` is a decoded, NUL-free single path component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    /// The decoded entry name.
    pub name: String,
    /// The entry kind.
    pub entry_type: EntryType,
    /// The recorded size in bytes; zero for directories.
    pub size_bytes: u64,
}

/// Identifies one mount. A cursor is only ever accepted by the mount that
/// produced it, so a cursor cannot be replayed against unrelated media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MountId(usize);

impl MountId {
    /// Allocates the next process-unique mount identity.
    pub fn allocate() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// An opaque continuation token for a paged directory enumeration.
///
/// The token records where the enumeration stopped, never a path or a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryCursor {
    /// The mount that produced this cursor.
    pub mount: MountId,
    /// The extent the enumeration is walking, used for identity and for
    /// cycle detection.
    pub directory_extent: u64,
    /// The recorded byte length of that extent.
    pub directory_length: u64,
    /// Index of the next entry to return.
    pub next_index: u64,
    /// Number of entries already returned across previous pages.
    pub returned_entries: u64,
    /// Number of pages already produced.
    pub pages_emitted: u32,
}

/// One bounded page of a directory enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage {
    /// The entries in this page, in deterministic on-media order.
    pub entries: Vec<DirectoryEntry>,
    /// Present only when another page is available.
    pub cursor: Option<DirectoryCursor>,
}

impl DirectoryPage {
    /// Whether the enumeration finished with this page.
    pub fn is_complete(&self) -> bool {
        self.cursor.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::{DirectoryPage, MountId};

    #[test]
    fn mount_identities_are_distinct() {
        assert_ne!(MountId::allocate(), MountId::allocate());
    }

    #[test]
    fn a_page_without_a_cursor_is_complete() {
        let page = DirectoryPage {
            entries: alloc::vec::Vec::new(),
            cursor: None,
        };
        assert!(page.is_complete());
    }
}
