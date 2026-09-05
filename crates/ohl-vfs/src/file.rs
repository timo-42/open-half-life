//! A read-only file opened inside a [`crate::Mount`].

use std::sync::{Arc, Mutex};

use ohl_core::SanitizedError;

use crate::mount::{ArchiveState, FileHandle};

/// A read-only file handle opened through a [`crate::Mount`].
///
/// The handle keeps the mounted archive state alive (through the same `Arc`
/// the owning [`crate::Mount`] and its shares hold), so it stays valid for as
/// long as any handle to the mount exists, even after the `Mount` value that
/// opened it is dropped.
pub struct MediaFile {
    mount: Arc<Mutex<ArchiveState>>,
    handle: FileHandle,
}

impl MediaFile {
    pub(crate) fn new(mount: Arc<Mutex<ArchiveState>>, handle: FileHandle) -> Self {
        Self { mount, handle }
    }

    /// The file's recorded size in bytes.
    pub fn size(&self) -> u64 {
        self.handle.size()
    }

    /// The current read position.
    pub fn position(&self) -> u64 {
        self.handle.position()
    }

    /// Moves the read position. Seeking past the end is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] when `offset` exceeds
    /// [`Self::size`].
    pub fn seek(&mut self, offset: u64) -> Result<(), SanitizedError> {
        self.handle.seek(offset)
    }

    /// Reads up to `out.len()` bytes at the current position, advancing it by
    /// the number of bytes returned.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the underlying block read fails; an
    /// empty result (`Ok(0)`) at or past the end of the file is not an error.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, SanitizedError> {
        let mut mount = self.mount.lock().map_err(|_| SanitizedError::Internal)?;
        mount.read_file(&mut self.handle, out)
    }
}
