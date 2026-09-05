//! Sanitized, fixed-code errors for the save container.
//!
//! Every variant carries no payload derived from file contents, so a
//! `Display` implementation on this type can never interpolate untrusted
//! bytes (mirroring `ohl_core::SanitizedError`, which this crate's fallible
//! bounded-arithmetic call sites also produce and map into here).

use core::fmt;

/// A sanitized save-container failure code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SaveError {
    /// The file does not start with the container magic.
    BadMagic,
    /// The file's major format version is not the one this build supports.
    UnsupportedMajorVersion,
    /// The file ended before a required field, table entry, or section could
    /// be read in full.
    Truncated,
    /// A count or length exceeded the caller-supplied [`crate::Limits`].
    LimitExceeded,
    /// A bounded header field failed validation (invalid UTF-8, or longer
    /// than its fixed maximum).
    HeaderInvalid,
    /// The section table is structurally invalid (for example a duplicate
    /// tag, or an entry whose offset/length falls outside the file).
    TableInvalid,
    /// A section's stored SHA-256 digest does not match its bytes.
    SectionDigestMismatch,
    /// The whole-file SHA-256 trailer does not match the file's contents.
    TrailerMismatch,
    /// [`crate::SaveWriter::add_section`] was called twice with the same
    /// tag.
    DuplicateTag,
    /// [`crate::SaveWriter::add_section`] was called with a tag reserved for
    /// this crate's own future use; see [`crate::MIN_APPLICATION_TAG`].
    ReservedTag,
    /// The requested section tag is not present in the file.
    SectionNotFound,
    /// A `postcard` encode or decode of a section payload failed.
    Codec,
    /// A save-slot name was empty, too long, or contained a character other
    /// than an ASCII letter, digit, `-`, or `_`.
    InvalidSlotName,
    /// The requested save slot does not exist.
    SlotNotFound,
    /// A filesystem operation on a save slot failed.
    Io,
}

impl SaveError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::BadMagic => "save file does not start with the expected magic",
            Self::UnsupportedMajorVersion => "save file major version is not supported",
            Self::Truncated => "save file is truncated",
            Self::LimitExceeded => "save file exceeded a configured limit",
            Self::HeaderInvalid => "save file header failed validation",
            Self::TableInvalid => "save file section table is invalid",
            Self::SectionDigestMismatch => "save file section digest mismatch",
            Self::TrailerMismatch => "save file trailer digest mismatch",
            Self::DuplicateTag => "duplicate section tag",
            Self::ReservedTag => "section tag is reserved",
            Self::SectionNotFound => "requested section was not found",
            Self::Codec => "section payload encode or decode failed",
            Self::InvalidSlotName => "save slot name failed validation",
            Self::SlotNotFound => "requested save slot was not found",
            Self::Io => "save slot filesystem operation failed",
        }
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for SaveError {}

impl From<ohl_core::SanitizedError> for SaveError {
    fn from(_error: ohl_core::SanitizedError) -> Self {
        // Every bounded-arithmetic failure in this crate arises from an
        // over-large count, offset, or length, so it is reported uniformly
        // as a limit failure rather than distinguishing overflow from
        // underflow (which would leak no information the caller could act
        // on differently).
        Self::LimitExceeded
    }
}

/// A `Result` alias for save-container operations.
pub type Result<T> = core::result::Result<T, SaveError>;

#[cfg(test)]
mod tests {
    use super::SaveError;

    #[test]
    fn every_message_is_a_fixed_non_empty_literal() {
        let codes = [
            SaveError::BadMagic,
            SaveError::UnsupportedMajorVersion,
            SaveError::Truncated,
            SaveError::LimitExceeded,
            SaveError::HeaderInvalid,
            SaveError::TableInvalid,
            SaveError::SectionDigestMismatch,
            SaveError::TrailerMismatch,
            SaveError::DuplicateTag,
            SaveError::ReservedTag,
            SaveError::SectionNotFound,
            SaveError::Codec,
            SaveError::InvalidSlotName,
            SaveError::SlotNotFound,
            SaveError::Io,
        ];
        for code in codes {
            assert!(!code.to_string().is_empty());
        }
    }

    #[test]
    fn sanitized_error_maps_to_limit_exceeded() {
        let mapped: SaveError = ohl_core::SanitizedError::ArithmeticOverflow.into();
        assert_eq!(mapped, SaveError::LimitExceeded);
    }
}
