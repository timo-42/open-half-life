//! Pinned-source stability checks at validation boundaries.
//!
//! A [`crate::MediaSource`] proves *which* object is being read, not that its
//! bytes have stopped changing. Any subsystem that is about to publish
//! something derived from a source — a cache manifest, a staged payload — must
//! first reauthenticate the source's complete content against the fingerprint
//! it accepted earlier. That is what
//! [`verify_complete_source_stability`] does, and it is the direct port of the
//! C++ `media::detail::verify_complete_source_stability`.
//!
//! The check brackets the full rehash with a [`crate::MediaSource::verify_unchanged`]
//! call at both ends, so a mutation that lands mid-stream is reported as a
//! source change rather than as a digest mismatch. Results carry no source
//! identity, no bytes, and no digest material.

use ohl_core::StreamingSha256;

use crate::media_source::{MediaSource, MediaSourceError};

/// The read window used while rehashing, in bytes (64 KiB, as in C++).
const REHASH_CHUNK_BYTES: u64 = 64 * 1024;

/// What a caller accepted about a source at validation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceFingerprint {
    /// The size the source had when it was validated.
    pub size_bytes: u64,
    /// The SHA-256 of the source's complete content at validation time.
    pub sha256: [u8; 32],
}

/// A sanitized stability-check failure code.
///
/// As with [`MediaSourceError`], every variant is payload-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceStabilityError {
    /// The capability and the fingerprint disagree before any read: they do
    /// not describe the same object, so nothing can be verified.
    InvalidCapability,
    /// The pinned object changed at or during the check.
    SourceChanged,
    /// A native read failed for a reason that is not a detected change.
    ReadFailure,
    /// Every byte was read, but the content no longer hashes to the accepted
    /// fingerprint.
    DigestMismatch,
    /// The caller asked to stop before the check completed.
    Cancelled,
}

impl SourceStabilityError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidCapability => "source capability does not match its fingerprint",
            Self::SourceChanged => "pinned source changed during verification",
            Self::ReadFailure => "pinned source could not be read during verification",
            Self::DigestMismatch => "pinned source content no longer matches its fingerprint",
            Self::Cancelled => "source verification was cancelled",
        }
    }
}

impl core::fmt::Display for SourceStabilityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for SourceStabilityError {}

impl From<SourceStabilityError> for ohl_core::SanitizedError {
    fn from(error: SourceStabilityError) -> Self {
        match error {
            SourceStabilityError::InvalidCapability
            | SourceStabilityError::SourceChanged
            | SourceStabilityError::DigestMismatch => Self::InvalidInput,
            SourceStabilityError::ReadFailure | SourceStabilityError::Cancelled => Self::Internal,
        }
    }
}

/// Classifies a boundary [`MediaSource::verify_unchanged`] result.
fn map_boundary_error(result: Result<(), MediaSourceError>) -> Result<(), SourceStabilityError> {
    match result {
        Ok(()) => Ok(()),
        Err(MediaSourceError::Changed) => Err(SourceStabilityError::SourceChanged),
        Err(_) => Err(SourceStabilityError::ReadFailure),
    }
}

/// Reauthenticates the complete content of an already validated pinned
/// source against `fingerprint`.
///
/// # Errors
///
/// See [`SourceStabilityError`]. A read failure is always re-checked against
/// the pinned object first, so a truncation observed as an early end of file
/// is reported as [`SourceStabilityError::SourceChanged`] and not as an
/// opaque read failure.
pub fn verify_complete_source_stability(
    source: &MediaSource,
    fingerprint: &SourceFingerprint,
) -> Result<(), SourceStabilityError> {
    verify_complete_source_stability_with_cancellation(source, fingerprint, &mut || false)
}

/// [`verify_complete_source_stability`] with a cancellation predicate.
///
/// `stop_requested` is polled once before the first read and again after each
/// chunk, mirroring the C++ `CancellationToken` polling points. It is never
/// polled after the last chunk, so a cancellation that arrives at the same
/// moment the check completes resolves in favour of the completed check.
///
/// # Errors
///
/// See [`SourceStabilityError`].
pub fn verify_complete_source_stability_with_cancellation(
    source: &MediaSource,
    fingerprint: &SourceFingerprint,
    stop_requested: &mut dyn FnMut() -> bool,
) -> Result<(), SourceStabilityError> {
    if source.size() != fingerprint.size_bytes {
        return Err(SourceStabilityError::InvalidCapability);
    }

    map_boundary_error(source.verify_unchanged())?;
    if stop_requested() {
        return Err(SourceStabilityError::Cancelled);
    }

    let capacity =
        usize::try_from(REHASH_CHUNK_BYTES).map_err(|_| SourceStabilityError::ReadFailure)?;
    let mut digest = StreamingSha256::new();
    let mut buffer = vec![0u8; capacity];
    let mut offset: u64 = 0;
    while offset < fingerprint.size_bytes {
        let remaining = fingerprint.size_bytes - offset;
        let count = remaining.min(REHASH_CHUNK_BYTES);
        let chunk =
            &mut buffer[..usize::try_from(count).map_err(|_| SourceStabilityError::ReadFailure)?];
        if let Err(read_error) = source.read_exact_at(offset, chunk) {
            // A failed read is ambiguous on its own: ask the pinned object
            // first, so a change is reported as a change.
            map_boundary_error(source.verify_unchanged())?;
            return Err(match read_error {
                MediaSourceError::UnexpectedEof | MediaSourceError::OutOfRange => {
                    SourceStabilityError::SourceChanged
                }
                _ => SourceStabilityError::ReadFailure,
            });
        }
        digest.update(chunk);
        offset += count;

        if offset < fingerprint.size_bytes && stop_requested() {
            return Err(SourceStabilityError::Cancelled);
        }
    }

    map_boundary_error(source.verify_unchanged())?;
    if digest.finalize() == fingerprint.sha256 {
        Ok(())
    } else {
        Err(SourceStabilityError::DigestMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceStabilityError, map_boundary_error};
    use crate::media_source::MediaSourceError;

    #[test]
    fn boundary_results_are_classified() {
        assert_eq!(map_boundary_error(Ok(())), Ok(()));
        assert_eq!(
            map_boundary_error(Err(MediaSourceError::Changed)),
            Err(SourceStabilityError::SourceChanged)
        );
        assert_eq!(
            map_boundary_error(Err(MediaSourceError::ReadFailed)),
            Err(SourceStabilityError::ReadFailure)
        );
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            SourceStabilityError::InvalidCapability,
            SourceStabilityError::SourceChanged,
            SourceStabilityError::ReadFailure,
            SourceStabilityError::DigestMismatch,
            SourceStabilityError::Cancelled,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
