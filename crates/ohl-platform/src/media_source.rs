//! A pinned, read-only capability for exactly one native file object.
//!
//! # The contract
//!
//! [`MediaSource::open`] performs **one** native acquisition of the selected
//! path and then forgets it. Everything afterwards — every read, every
//! verification — goes through the retained native handle, so the pathname is
//! never resolved a second time. Concretely:
//!
//! - the final path component is opened without following a symbolic link or
//!   reparse point (`O_NOFOLLOW` on Unix, `FILE_FLAG_OPEN_REPARSE_POINT` plus
//!   an attribute rejection on Windows). Intermediate components use ordinary
//!   platform resolution; this contract makes no promise about them;
//! - anything that is not an ordinary regular file — a directory, FIFO,
//!   socket, device, terminal, or reparse point — is rejected;
//! - the object's native identity (`st_dev`/`st_ino` on Unix and macOS, the
//!   volume serial number plus the 64-bit file index on Windows), its size,
//!   and its native content-change indicator are captured at acquisition and
//!   pinned for the lifetime of the capability;
//! - reads are bounded positional reads that never consult a shared seek
//!   cursor, so `&MediaSource` may be shared across threads freely;
//! - the handle is move-only. Rust ownership replaces the C++ deleted copy
//!   and move operations; share it behind an [`std::sync::Arc`] when several
//!   subsystems need the same pinned identity.
//!
//! # What pinning does and does not buy
//!
//! Pinning defeats *retargeting*: once the object is acquired, replacing,
//! renaming, or deleting the pathname cannot make this capability read a
//! different object. It does **not** make the bytes immutable — an external
//! writer holding its own descriptor can still mutate the pinned object.
//! Callers must therefore call [`MediaSource::verify_unchanged`] at defined
//! phase boundaries and perform the end-to-end content verification in
//! [`crate::stability`] before publishing anything derived from the source.
//!
//! # Sanitized failures
//!
//! [`MediaSourceError`] variants are payload-free codes. Neither their
//! `Debug` nor their `Display` output can contain a path, an OS error string,
//! or a media-derived byte, and each converts into an
//! [`ohl_core::SanitizedError`] for call sites that speak only that
//! vocabulary.

use std::path::Path;

use ohl_core::SanitizedError;

use crate::sys::{NativeSnapshot, PinnedFile};

/// A sanitized media-source failure code.
///
/// Every variant is payload-free, so no formatting implementation can ever
/// interpolate caller- or media-supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaSourceError {
    /// The selected path does not name an existing object.
    NotFound,
    /// The acquired object is not an ordinary regular file, or the final
    /// component was a symbolic link or reparse point.
    NotRegularFile,
    /// The native acquisition failed for another reason.
    OpenFailed,
    /// A native positional read failed.
    ReadFailed,
    /// The object reported end of file inside the pinned size.
    UnexpectedEof,
    /// The requested read window does not lie inside the pinned size.
    OutOfRange,
    /// The pinned object no longer matches its acquisition snapshot.
    Changed,
    /// A native resource limit was reached.
    ResourceExhausted,
    /// This target has no pinned-acquisition backend.
    Unsupported,
}

impl MediaSourceError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::NotFound => "media source was not found",
            Self::NotRegularFile => "media source is not a regular file",
            Self::OpenFailed => "media source could not be opened",
            Self::ReadFailed => "media source read failed",
            Self::UnexpectedEof => "media source ended before the requested range",
            Self::OutOfRange => "requested range is outside the pinned media source",
            Self::Changed => "pinned media source changed after acquisition",
            Self::ResourceExhausted => "a native resource limit was reached",
            Self::Unsupported => "media source acquisition is not supported on this target",
        }
    }
}

impl core::fmt::Display for MediaSourceError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for MediaSourceError {}

impl From<MediaSourceError> for SanitizedError {
    /// Widens a media-source code into the shared sanitized vocabulary.
    ///
    /// The mapping is deliberately lossy — `SanitizedError` is the vocabulary
    /// used at subsystem boundaries, where the caller must not be able to
    /// distinguish, say, "not a regular file" from "changed under us" if that
    /// would tell it something about the user's filesystem. Call sites that
    /// need the finer distinction keep the [`MediaSourceError`].
    fn from(error: MediaSourceError) -> Self {
        match error {
            MediaSourceError::NotFound => Self::NotFound,
            MediaSourceError::Unsupported => Self::Unsupported,
            MediaSourceError::NotRegularFile
            | MediaSourceError::OutOfRange
            | MediaSourceError::UnexpectedEof
            | MediaSourceError::Changed => Self::InvalidInput,
            MediaSourceError::OpenFailed
            | MediaSourceError::ReadFailed
            | MediaSourceError::ResourceExhausted => Self::Internal,
        }
    }
}

/// A read-only capability for one pinned native file object.
///
/// See the [module documentation](self) for the full contract. The type is
/// move-only and carries no interior mutability, so `&MediaSource` is `Sync`
/// and positional reads through a shared reference are safe to issue
/// concurrently.
#[derive(Debug)]
pub struct MediaSource {
    /// The retained native handle. Dropping it is the only close.
    handle: PinnedFile,
    /// The acquisition-time observation every later check compares against.
    acquired: NativeSnapshot,
}

impl MediaSource {
    /// Acquires `path` once and returns the pinned capability.
    ///
    /// The path is used only inside this call: it is neither stored nor
    /// resolved again. On success the returned value owns the sole handle to
    /// the acquired object.
    ///
    /// # Errors
    ///
    /// [`MediaSourceError::NotFound`] when nothing is there,
    /// [`MediaSourceError::NotRegularFile`] when the final component is a
    /// link or the object is not an ordinary file,
    /// [`MediaSourceError::ResourceExhausted`] on a native limit, and
    /// [`MediaSourceError::OpenFailed`] otherwise.
    pub fn open(path: &Path) -> Result<Self, MediaSourceError> {
        let (handle, acquired) = PinnedFile::open(path)?;
        Ok(Self { handle, acquired })
    }

    /// The size captured at acquisition, in bytes.
    ///
    /// This is the pinned size: it never changes for the lifetime of the
    /// capability, even if the underlying object is truncated or extended.
    /// Every read window is validated against this value, and
    /// [`Self::verify_unchanged`] reports a disagreement as
    /// [`MediaSourceError::Changed`].
    pub const fn size(&self) -> u64 {
        self.acquired.size_bytes
    }

    /// Reads exactly `destination.len()` bytes starting at `offset`.
    ///
    /// The read never consults or moves a shared seek cursor. An empty read
    /// is valid at every offset from `0` through [`Self::size`] inclusive,
    /// matching the C++ contract, and performs no native call at all.
    ///
    /// # Errors
    ///
    /// [`MediaSourceError::OutOfRange`] when the window is not entirely
    /// inside the pinned size (including when `offset` alone exceeds it, and
    /// including windows whose end would overflow `u64`);
    /// [`MediaSourceError::UnexpectedEof`] when the object reports end of
    /// file inside the pinned size, which means it was truncated after
    /// acquisition; [`MediaSourceError::ReadFailed`] or
    /// [`MediaSourceError::ResourceExhausted`] on a native failure.
    pub fn read_exact_at(
        &self,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<(), MediaSourceError> {
        let requested =
            u64::try_from(destination.len()).map_err(|_| MediaSourceError::OutOfRange)?;
        let available = self
            .size()
            .checked_sub(offset)
            .ok_or(MediaSourceError::OutOfRange)?;
        if requested > available {
            return Err(MediaSourceError::OutOfRange);
        }

        let mut completed: usize = 0;
        while completed < destination.len() {
            let progressed = u64::try_from(completed).map_err(|_| MediaSourceError::OutOfRange)?;
            let position = offset
                .checked_add(progressed)
                .ok_or(MediaSourceError::OutOfRange)?;
            let count = self
                .handle
                .read_at(position, &mut destination[completed..])?;
            if count == 0 {
                return Err(MediaSourceError::UnexpectedEof);
            }
            completed = completed
                .checked_add(count)
                .ok_or(MediaSourceError::ReadFailed)?;
            if completed > destination.len() {
                // A backend that reported more bytes than the buffer can hold
                // is broken; refuse rather than trust the count.
                return Err(MediaSourceError::ReadFailed);
            }
        }
        Ok(())
    }

    /// Re-queries the pinned object and compares it with the acquisition
    /// snapshot.
    ///
    /// The comparison covers the regular-file type, the stable native
    /// identity, the size, and the native content-change indicator. It never
    /// resolves the original pathname, so a pathname replacement is invisible
    /// here (and correctly so: the capability still refers to the object it
    /// pinned), while truncation, extension, or an in-place rewrite of the
    /// pinned object is reported.
    ///
    /// # Errors
    ///
    /// [`MediaSourceError::Changed`] when the pinned object no longer matches
    /// its snapshot, or [`MediaSourceError::ReadFailed`] when the object
    /// cannot be re-queried at all.
    pub fn verify_unchanged(&self) -> Result<(), MediaSourceError> {
        let current = self.handle.snapshot()?;
        if current.matches_pin(&self.acquired) {
            Ok(())
        } else {
            Err(MediaSourceError::Changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaSource, MediaSourceError};
    use ohl_core::SanitizedError;

    #[test]
    fn every_message_is_a_fixed_literal_without_punctuation_from_input() {
        for error in [
            MediaSourceError::NotFound,
            MediaSourceError::NotRegularFile,
            MediaSourceError::OpenFailed,
            MediaSourceError::ReadFailed,
            MediaSourceError::UnexpectedEof,
            MediaSourceError::OutOfRange,
            MediaSourceError::Changed,
            MediaSourceError::ResourceExhausted,
            MediaSourceError::Unsupported,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.is_empty());
            assert!(
                !rendered.contains('/') && !rendered.contains('\\'),
                "a sanitized message must never look like a path: {rendered}"
            );
        }
    }

    #[test]
    fn codes_widen_into_the_shared_sanitized_vocabulary() {
        assert_eq!(
            SanitizedError::from(MediaSourceError::NotFound),
            SanitizedError::NotFound
        );
        assert_eq!(
            SanitizedError::from(MediaSourceError::Unsupported),
            SanitizedError::Unsupported
        );
        assert_eq!(
            SanitizedError::from(MediaSourceError::Changed),
            SanitizedError::InvalidInput
        );
        assert_eq!(
            SanitizedError::from(MediaSourceError::ReadFailed),
            SanitizedError::Internal
        );
    }

    #[test]
    fn the_capability_is_move_only_but_shareable_across_threads() {
        // `MediaSource` deliberately implements neither `Clone` nor `Copy`:
        // ownership is what the C++ class expressed with deleted copy and
        // move operations, and it is enforced here by the type system at
        // every call site. What must be asserted is that sharing it behind a
        // reference across threads is still allowed.
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<MediaSource>();
    }
}
