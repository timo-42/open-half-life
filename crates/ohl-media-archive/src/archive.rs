//! The uniform read-only archive surface both media classes implement.
//!
//! `ohl-vfs` depends on this trait rather than on either reader, so adding a
//! media class does not change any caller outside the reader crates.

use crate::class::{FilesystemDescription, MediaClass, VolumeLabel};
use crate::listing::{DirectoryCursor, DirectoryEntry, DirectoryPage};
use alloc::vec::Vec;
use ohl_core::SanitizedError;

/// A read-only file opened inside a mounted archive.
pub trait MediaFileHandle {
    /// The file's recorded size in bytes.
    fn size(&self) -> u64;
    /// The current read position.
    fn position(&self) -> u64;
    /// Moves the read position. Seeking past the end is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] when `offset` exceeds
    /// [`Self::size`].
    fn seek(&mut self, offset: u64) -> Result<(), SanitizedError>;
}

/// A mounted, read-only media archive.
///
/// Every method is bounded by the mount's `DirectoryLimits`, and every failure
/// is a sanitized code that carries no name, path, or media byte.
pub trait MediaArchive {
    /// The archive's file handle type.
    type File: MediaFileHandle;

    /// The media class this archive was mounted as.
    fn media_class(&self) -> MediaClass;

    /// The fixed description of the mounted structure.
    fn filesystem(&self) -> FilesystemDescription;

    /// The sanitized volume label, which may be empty.
    fn volume_label(&self) -> &VolumeLabel;

    /// Returns one bounded page of `path` in deterministic on-media order.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the path is invalid, absent, exceeds a
    /// limit, or the media is structurally invalid.
    fn list_page(&mut self, path: &str) -> Result<DirectoryPage, SanitizedError>;

    /// Continues an enumeration from a cursor this archive produced.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] for a cursor from another
    /// mount, and the same codes as [`Self::list_page`] otherwise.
    fn continue_list(&mut self, cursor: DirectoryCursor) -> Result<DirectoryPage, SanitizedError>;

    /// Compatibility listing: succeeds only with the complete bounded result.
    ///
    /// # Errors
    ///
    /// Returns the same codes as [`Self::list_page`]; any enumeration limit
    /// yields an error rather than a truncated listing.
    fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>, SanitizedError> {
        let mut page = self.list_page(path)?;
        let mut entries = core::mem::take(&mut page.entries);
        while let Some(cursor) = page.cursor.take() {
            page = self.continue_list(cursor)?;
            entries.append(&mut page.entries);
        }
        Ok(entries)
    }

    /// Opens a file by absolute path.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the path is invalid, absent, refers to a
    /// directory, or the recorded extent is out of bounds.
    fn open_file(&mut self, path: &str) -> Result<Self::File, SanitizedError>;

    /// Opens `entry_name` inside the already-resolved directory `directory`.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] when `entry_name` is not a
    /// single path component, and the same codes as [`Self::open_file`]
    /// otherwise.
    fn open_file_at(
        &mut self,
        directory: &str,
        entry_name: &str,
    ) -> Result<Self::File, SanitizedError>;

    /// Reads up to `out.len()` bytes at the file's current position.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the handle belongs to another mount or
    /// the underlying block read fails.
    fn read_file(&mut self, file: &mut Self::File, out: &mut [u8])
    -> Result<usize, SanitizedError>;
}
