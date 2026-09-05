//! Fixed, payload-free failure codes for the media package.
//!
//! Every code in this module follows the `ohl-core` sanitization rule: the
//! variants carry no payload, so no `Debug` or `Display` implementation can
//! interpolate a path, a media-derived byte, a volume label, or an OS error
//! string into a diagnostic. Callers that need more context must record it
//! out of band.

use ohl_core::SanitizedError;
use ohl_platform::{MediaSourceError, SourceStabilityError};

/// A sanitized failure of fingerprinting or of the validated-media proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaError {
    /// The pinned capability and the values the caller supplied do not
    /// describe the same object, so nothing can be proved about it.
    InvalidCapability,
    /// The pinned object changed at or during the operation.
    SourceChanged,
    /// A native read failed for a reason that is not a detected change.
    SourceReadFailed,
    /// A caller-supplied label is not bounded, printable ASCII.
    InvalidLabel,
}

impl MediaError {
    /// The fixed, payload-free message for this code.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidCapability => "media capability does not match the supplied description",
            Self::SourceChanged => "pinned media source changed during the operation",
            Self::SourceReadFailed => "pinned media source could not be read",
            Self::InvalidLabel => "label is not bounded printable ASCII",
        }
    }
}

impl core::fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for MediaError {}

impl From<MediaError> for SanitizedError {
    fn from(error: MediaError) -> Self {
        match error {
            MediaError::InvalidCapability
            | MediaError::SourceChanged
            | MediaError::InvalidLabel => Self::InvalidInput,
            MediaError::SourceReadFailed => Self::Internal,
        }
    }
}

impl From<MediaSourceError> for MediaError {
    fn from(error: MediaSourceError) -> Self {
        match error {
            MediaSourceError::Changed
            | MediaSourceError::UnexpectedEof
            | MediaSourceError::OutOfRange => Self::SourceChanged,
            _ => Self::SourceReadFailed,
        }
    }
}

/// A sanitized provenance-cache failure code.
///
/// The codes are the Rust port of the C++ `media::ImportCacheError` set, plus
/// the three the Rust publication path adds: an unavailable user cache
/// directory, a contended publication lock, and an oversized manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportCacheError {
    /// The request itself is unusable: the capability does not agree with the
    /// proof it was built from.
    InvalidRequest,
    /// The pinned source could not be reauthenticated before publication.
    SourceReadFailed,
    /// The pinned source changed after validation, so nothing is published.
    SourceChanged,
    /// The cache path is relative, contains `.`/`..`, or a component is a
    /// symbolic link or not a directory.
    UnsafeCachePath,
    /// No per-user cache directory could be discovered on this platform.
    CacheUnavailable,
    /// A cache directory could not be created.
    CacheCreateFailed,
    /// Another writer holds the exclusive publication lock for this entry.
    CacheBusy,
    /// An existing manifest is larger than the bounded manifest limit and is
    /// therefore not read at all.
    ManifestTooLarge,
    /// An existing manifest declares a schema version this build does not
    /// implement.
    ManifestSchemaUnsupported,
    /// An existing manifest is unreadable, tampered with, or describes other
    /// media. Nothing is overwritten.
    ManifestConflict,
    /// The manifest could not be staged or published.
    ManifestWriteFailed,
}

impl ImportCacheError {
    /// The fixed, payload-free message for this code.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid cache request",
            Self::SourceReadFailed => "source media could not be validated",
            Self::SourceChanged => "source media changed after preflight",
            Self::UnsafeCachePath => "cache path is relative, linked, or not a directory",
            Self::CacheUnavailable => "no user cache directory is available",
            Self::CacheCreateFailed => "cache directory could not be created",
            Self::CacheBusy => "another writer holds the cache publication lock",
            Self::ManifestTooLarge => "existing provenance manifest exceeds the bounded size",
            Self::ManifestSchemaUnsupported => "provenance manifest schema is not supported",
            Self::ManifestConflict => "existing provenance manifest does not match the source",
            Self::ManifestWriteFailed => "provenance manifest could not be published",
        }
    }
}

impl core::fmt::Display for ImportCacheError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for ImportCacheError {}

impl From<ImportCacheError> for SanitizedError {
    fn from(error: ImportCacheError) -> Self {
        match error {
            ImportCacheError::InvalidRequest
            | ImportCacheError::SourceChanged
            | ImportCacheError::UnsafeCachePath
            | ImportCacheError::ManifestTooLarge
            | ImportCacheError::ManifestConflict => Self::InvalidInput,
            ImportCacheError::CacheUnavailable | ImportCacheError::ManifestSchemaUnsupported => {
                Self::Unsupported
            }
            ImportCacheError::SourceReadFailed
            | ImportCacheError::CacheCreateFailed
            | ImportCacheError::CacheBusy
            | ImportCacheError::ManifestWriteFailed => Self::Internal,
        }
    }
}

impl From<MediaError> for ImportCacheError {
    fn from(error: MediaError) -> Self {
        match error {
            MediaError::InvalidCapability | MediaError::InvalidLabel => Self::InvalidRequest,
            MediaError::SourceChanged => Self::SourceChanged,
            MediaError::SourceReadFailed => Self::SourceReadFailed,
        }
    }
}

impl From<SourceStabilityError> for ImportCacheError {
    fn from(error: SourceStabilityError) -> Self {
        match error {
            SourceStabilityError::InvalidCapability => Self::InvalidRequest,
            SourceStabilityError::SourceChanged | SourceStabilityError::DigestMismatch => {
                Self::SourceChanged
            }
            SourceStabilityError::ReadFailure | SourceStabilityError::Cancelled => {
                Self::SourceReadFailed
            }
            // The enum is `#[non_exhaustive]`; a code added later must not
            // silently become a success.
            _ => Self::SourceReadFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImportCacheError, MediaError};
    use ohl_core::SanitizedError;
    use ohl_platform::MediaSourceError;

    const MEDIA_ERRORS: &[MediaError] = &[
        MediaError::InvalidCapability,
        MediaError::SourceChanged,
        MediaError::SourceReadFailed,
        MediaError::InvalidLabel,
    ];

    const CACHE_ERRORS: &[ImportCacheError] = &[
        ImportCacheError::InvalidRequest,
        ImportCacheError::SourceReadFailed,
        ImportCacheError::SourceChanged,
        ImportCacheError::UnsafeCachePath,
        ImportCacheError::CacheUnavailable,
        ImportCacheError::CacheCreateFailed,
        ImportCacheError::CacheBusy,
        ImportCacheError::ManifestTooLarge,
        ImportCacheError::ManifestSchemaUnsupported,
        ImportCacheError::ManifestConflict,
        ImportCacheError::ManifestWriteFailed,
    ];

    #[test]
    fn every_message_is_a_fixed_literal_without_path_punctuation() {
        for message in MEDIA_ERRORS
            .iter()
            .map(ToString::to_string)
            .chain(CACHE_ERRORS.iter().map(ToString::to_string))
        {
            assert!(!message.is_empty());
            assert!(!message.contains('/'));
            assert!(!message.contains('\\'));
            assert!(message.is_ascii());
        }
    }

    #[test]
    fn every_code_maps_to_a_sanitized_error() {
        for error in MEDIA_ERRORS {
            let _: SanitizedError = (*error).into();
        }
        for error in CACHE_ERRORS {
            let _: SanitizedError = (*error).into();
        }
    }

    #[test]
    fn truncation_and_range_failures_are_reported_as_a_change() {
        assert_eq!(
            MediaError::from(MediaSourceError::UnexpectedEof),
            MediaError::SourceChanged
        );
        assert_eq!(
            MediaError::from(MediaSourceError::OutOfRange),
            MediaError::SourceChanged
        );
        assert_eq!(
            MediaError::from(MediaSourceError::ReadFailed),
            MediaError::SourceReadFailed
        );
    }
}
