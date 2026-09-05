//! Sanitized failures.
//!
//! Every variant is a fixed, project-defined code. No variant carries a
//! media-derived byte, name, path, offset or size, so `Display` output is
//! always safe to log under `docs/MEDIA_IMPORT.md`.

use core::fmt;

use ohl_core::SanitizedError;

/// The caller-supplied ceiling that an input exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Limit {
    /// `Limits::max_pe_header_bytes`.
    PeHeaderBytes,
    /// `Limits::max_sections`.
    Sections,
    /// `Limits::max_header_scan_bytes`.
    HeaderScanBytes,
    /// `Limits::max_streams`.
    Streams,
    /// `Limits::max_compressed_bytes_per_stream`.
    CompressedBytesPerStream,
    /// `Limits::max_inflated_bytes_per_stream`.
    InflatedBytesPerStream,
    /// `Limits::max_total_inflated_bytes`.
    TotalInflatedBytes,
    /// `Limits::max_script_bytes`.
    ScriptBytes,
    /// `Limits::max_file_records`.
    FileRecords,
    /// `Limits::max_path_bytes`.
    PathBytes,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::PeHeaderBytes => "executable header bytes",
            Self::Sections => "section count",
            Self::HeaderScanBytes => "header scan bytes",
            Self::Streams => "stream count",
            Self::CompressedBytesPerStream => "compressed bytes per stream",
            Self::InflatedBytesPerStream => "inflated bytes per stream",
            Self::TotalInflatedBytes => "total inflated bytes",
            Self::ScriptBytes => "script bytes",
            Self::FileRecords => "file record count",
            Self::PathBytes => "path bytes",
        };
        f.write_str(name)
    }
}

/// Every way reading a Wise package can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The image does not start with the `MZ` DOS signature.
    #[error("input does not carry a DOS executable signature")]
    NotExecutable,
    /// The PE headers are malformed or internally inconsistent.
    #[error("the executable headers are malformed")]
    MalformedExecutable,
    /// The section table describes no bytes after the last section.
    #[error("the executable carries no overlay data")]
    NoOverlay,
    /// No checksum-confirmed DEFLATE stream was found within the scan window.
    #[error("no confirmed compressed stream follows the overlay header")]
    HeaderNotFound,
    /// The overlay carries ZIP local file headers, a documented variant this
    /// reader deliberately rejects rather than guessing at.
    #[error("the zip-enabled package variant is not supported")]
    ZipVariantUnsupported,
    /// The source ended inside a structure that must be read whole.
    #[error("the source ends inside a structure")]
    Truncated,
    /// A raw DEFLATE stream could not be inflated.
    #[error("a compressed stream could not be inflated")]
    DecompressionFailed,
    /// The trailing checksum does not match the inflated bytes.
    #[error("a stream checksum does not match its inflated bytes")]
    ChecksumMismatch,
    /// The second stream does not parse as a script binary.
    #[error("the script stream could not be parsed")]
    ScriptMalformed,
    /// A caller-supplied index lies outside its table.
    #[error("an index lies outside its table")]
    IndexOutOfRange,
    /// A file record's compressed range matches no stream in the chain.
    #[error("a file record refers to no stream in the chain")]
    NoStreamForRecord,
    /// The caller cancelled the operation between chunks.
    #[error("the operation was cancelled")]
    Cancelled,
    /// The caller-supplied source failed.
    #[error("the image source failed")]
    SourceFailed,
    /// A caller-supplied limit was exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(Limit),
}

impl From<Error> for SanitizedError {
    fn from(error: Error) -> Self {
        match error {
            Error::Cancelled | Error::SourceFailed => Self::Internal,
            Error::ZipVariantUnsupported => Self::Unsupported,
            Error::IndexOutOfRange | Error::NoStreamForRecord => Self::NotFound,
            _ => Self::InvalidInput,
        }
    }
}

impl From<SanitizedError> for Error {
    fn from(error: SanitizedError) -> Self {
        match error {
            SanitizedError::Unsupported => Self::ZipVariantUnsupported,
            SanitizedError::NotFound => Self::IndexOutOfRange,
            _ => Self::MalformedExecutable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Limit};
    use ohl_core::SanitizedError;

    #[test]
    fn display_is_fixed_and_carries_no_payload() {
        assert_eq!(
            Error::ChecksumMismatch.to_string(),
            "a stream checksum does not match its inflated bytes"
        );
        assert_eq!(
            Error::LimitExceeded(Limit::Streams).to_string(),
            "limit exceeded: stream count"
        );
    }

    #[test]
    fn maps_onto_the_shared_sanitized_code() {
        assert_eq!(
            SanitizedError::from(Error::ZipVariantUnsupported),
            SanitizedError::Unsupported
        );
        assert_eq!(
            SanitizedError::from(Error::Truncated),
            SanitizedError::InvalidInput
        );
    }
}
