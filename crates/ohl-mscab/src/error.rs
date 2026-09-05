//! A fixed, project-owned error enum for the cabinet decoder.
//!
//! Every variant is a fixed code with a fixed `Display` message: none of them
//! ever carries cabinet-derived bytes, offsets, names, or sizes, so a
//! `CabError` is always safe to log, to send across the parser-worker
//! boundary, or to show to a user. This mirrors `ohl_core::SanitizedError`,
//! into which it converts.

use core::fmt;

use ohl_core::SanitizedError;

/// A bounds, structural, or decompression failure while reading a cabinet.
///
/// The decoders in this crate never panic on malformed input; every fallible
/// operation returns one of these fixed variants instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CabError {
    /// The source is too short to contain a fixed-size structure.
    Truncated,
    /// The `CFHEADER` signature was not `MSCF`.
    BadSignature,
    /// The cabinet format version is not one this decoder implements.
    UnsupportedVersion,
    /// An offset or length falls outside the cabinet or the pinned source.
    OutOfBounds,
    /// A structure field held a value the specification does not allow.
    InvalidField,
    /// A folder index (from `CFFILE.iFolder`) is out of range.
    FolderIndexOutOfRange,
    /// A count or size exceeds the caller-supplied [`crate::Limits`].
    LimitExceeded,
    /// A `CFDATA` checksum did not match the documented algorithm.
    ChecksumMismatch,
    /// A compressed block failed to decode, or produced the wrong length.
    DecompressionFailed,
    /// The folder's compression type is not implemented (for example
    /// Quantum), or the file continues into a volume the caller did not
    /// supply.
    Unsupported,
    /// The caller's cancellation token was observed as cancelled.
    Cancelled,
    /// The caller-supplied [`crate::VolumeSource`] reported a read failure.
    SourceFailed,
    /// An internal invariant was violated; this indicates a bug.
    Internal,
}

impl fmt::Display for CabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "source is too short for the expected structure",
            Self::BadSignature => "cabinet signature did not match",
            Self::UnsupportedVersion => "cabinet format version is not supported",
            Self::OutOfBounds => "offset or length falls outside the cabinet",
            Self::InvalidField => "structure field failed validation",
            Self::FolderIndexOutOfRange => "folder index is out of range",
            Self::LimitExceeded => "count or size exceeds the configured limit",
            Self::ChecksumMismatch => "data block checksum did not match",
            Self::DecompressionFailed => "compressed block failed to decode",
            Self::Unsupported => "cabinet feature is not supported",
            Self::Cancelled => "operation was cancelled",
            Self::SourceFailed => "volume source reported a read failure",
            Self::Internal => "internal invariant violated",
        };
        f.write_str(message)
    }
}

impl core::error::Error for CabError {}

impl From<CabError> for SanitizedError {
    fn from(error: CabError) -> Self {
        match error {
            CabError::Unsupported | CabError::UnsupportedVersion => Self::Unsupported,
            CabError::Internal => Self::Internal,
            _ => Self::InvalidInput,
        }
    }
}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, CabError>;

#[cfg(test)]
mod tests {
    use super::{CabError, SanitizedError};
    extern crate alloc;
    use alloc::string::ToString as _;

    #[test]
    fn display_is_fixed_and_carries_no_input() {
        assert_eq!(
            CabError::ChecksumMismatch.to_string(),
            "data block checksum did not match"
        );
    }

    #[test]
    fn sanitizes_into_core_errors() {
        assert_eq!(
            SanitizedError::from(CabError::Unsupported),
            SanitizedError::Unsupported
        );
        assert_eq!(
            SanitizedError::from(CabError::Truncated),
            SanitizedError::InvalidInput
        );
        assert_eq!(
            SanitizedError::from(CabError::Internal),
            SanitizedError::Internal
        );
    }
}
