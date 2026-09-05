//! A bounded byte window over exactly one file inside the mounted media.
//!
//! The worker's `read_request` messages carry an offset and a length. Without
//! a window those are offsets into the whole pinned image, so an untrusted
//! worker that guessed a plausible offset could read any part of the medium.
//! [`SourceWindow`] narrows that authority to one container: offset zero is
//! the first byte of the window, and a read that would cross its end is
//! refused before any byte is touched.
//!
//! The window is a [`SourceOps`] seam, so it composes with the existing
//! [`SourceReadBroker`](crate::SourceReadBroker) quotas rather than replacing
//! them. The broker still bounds the per-request size, the request count, and
//! the cumulative reply bytes against the whole-source policy fixed at the
//! handshake; the window bounds *where* those reads land.
//!
//! Reads go through the [`MediaFile`] the [`Mount`](ohl_vfs::Mount) opened,
//! so ISO 9660 or UDF extent mapping, the mount's bounded buffering, and its
//! periodic source-stability verification all still apply. The pinned
//! `MediaSource` the broker hands each call is used only for
//! [`SourceOps::verify_unchanged`], which is exactly the check the broker
//! runs before and after every serviceable read.

use std::sync::{Mutex, PoisonError};

use ohl_core::SanitizedError;
use ohl_platform::{MediaSource, MediaSourceError};
use ohl_vfs::MediaFile;

use crate::io::sealed;
use crate::source_read_broker::SourceOps;

/// A read-only window `[base_offset, base_offset + length)` inside one file.
pub struct SourceWindow {
    file: Mutex<MediaFile>,
    base_offset: u64,
    length: u64,
}

impl core::fmt::Debug for SourceWindow {
    /// Prints the bounds only: the file handle is media-derived state.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SourceWindow")
            .field("base_offset", &self.base_offset)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl SourceWindow {
    /// Binds `file` to the window starting at `base_offset` for `length`
    /// bytes.
    ///
    /// # Errors
    /// [`SanitizedError::InvalidInput`] when the window is empty or does not
    /// lie inside the file.
    pub fn new(file: MediaFile, base_offset: u64, length: u64) -> Result<Self, SanitizedError> {
        let end = base_offset.checked_add(length);
        if length == 0 || end.is_none_or(|end| end > file.size()) {
            return Err(SanitizedError::InvalidInput);
        }
        Ok(Self {
            file: Mutex::new(file),
            base_offset,
            length,
        })
    }

    /// The first byte of the window, as an offset inside the container file.
    #[must_use]
    pub const fn base_offset(&self) -> u64 {
        self.base_offset
    }

    /// The window's length in bytes; also the largest offset+size a worker
    /// read may reach.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Reads `destination` in full at the window-relative `offset`.
    fn read_window(&self, offset: u64, destination: &mut [u8]) -> Result<(), MediaSourceError> {
        let requested = u64::try_from(destination.len()).unwrap_or(u64::MAX);
        // Clamped to the window: a read that would cross its end is refused
        // whole, because a short read is not something the reply codec can
        // express and a silently truncated one would misreport the container.
        let within = offset
            .checked_add(requested)
            .is_some_and(|end| end <= self.length);
        if !within {
            return Err(MediaSourceError::OutOfRange);
        }
        let Some(absolute) = self.base_offset.checked_add(offset) else {
            return Err(MediaSourceError::OutOfRange);
        };

        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        file.seek(absolute)
            .map_err(|_| MediaSourceError::OutOfRange)?;
        let mut filled = 0usize;
        while filled < destination.len() {
            match file.read(&mut destination[filled..]) {
                // The window lies inside the file, so a zero-length read here
                // is a truncated container, not a legal end of stream.
                Ok(0) => return Err(MediaSourceError::UnexpectedEof),
                Ok(count) => filled += count,
                Err(_) => return Err(MediaSourceError::ReadFailed),
            }
        }
        Ok(())
    }
}

impl sealed::Sealed for SourceWindow {}

impl SourceOps for SourceWindow {
    fn verify_unchanged(&self, source: &MediaSource) -> Result<(), MediaSourceError> {
        source.verify_unchanged()
    }

    fn read_exact_at(
        &self,
        _source: &MediaSource,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<(), MediaSourceError> {
        self.read_window(offset, destination)
    }
}
