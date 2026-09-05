//! Sanitized failures reported by this crate.
//!
//! Every variant is a fixed, project-defined code whose `Display` output is a
//! fixed string literal, so no archive-derived byte, name, offset or length
//! can ever reach a log through this type. That mirrors
//! [`ohl_core::SanitizedError`], which this crate converts into at its
//! boundary.

use core::fmt;

use ohl_core::SanitizedError;

/// The ceiling that a parse or extraction exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Limit {
    /// `Limits::max_scan_bytes`.
    ScanBytes,
    /// `Limits::max_archive_bytes`.
    ArchiveBytes,
    /// `Limits::max_directories`.
    Directories,
    /// `Limits::max_entries`.
    Entries,
    /// `Limits::max_directory_bytes`.
    DirectoryBytes,
    /// `Limits::max_name_bytes`.
    NameBytes,
    /// `Limits::max_stored_bytes_per_entry`.
    StoredBytesPerEntry,
    /// `Limits::max_expanded_bytes_per_entry`.
    ExpandedBytesPerEntry,
    /// `Limits::max_total_expanded_bytes`.
    TotalExpandedBytes,
    /// `Limits::max_chunk_bytes`.
    ChunkBytes,
}

impl Limit {
    /// A fixed identifier for this ceiling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScanBytes => "signature scan bytes",
            Self::ArchiveBytes => "archive bytes",
            Self::Directories => "directory count",
            Self::Entries => "entry count",
            Self::DirectoryBytes => "table-of-contents bytes",
            Self::NameBytes => "name bytes",
            Self::StoredBytesPerEntry => "stored bytes per entry",
            Self::ExpandedBytesPerEntry => "expanded bytes per entry",
            Self::TotalExpandedBytes => "total expanded bytes",
            Self::ChunkBytes => "chunk bytes",
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An opaque, sanitized failure reported by an [`crate::ArchiveSource`].
///
/// The trait deliberately cannot surface an I/O error's text, path or errno
/// into this crate, so nothing host- or media-derived can reach a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SourceError;

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the archive source failed to read")
    }
}

impl core::error::Error for SourceError {}

/// Every way parsing or extraction can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    /// The 8-byte archive signature is absent at the given base offset.
    BadSignature,
    /// The source ended before a fixed-size structure was complete.
    Truncated,
    /// A stored count, offset or length falls outside the archive.
    OutOfRange,
    /// A stored field combination is internally inconsistent.
    InvalidInput,
    /// The archive declares itself as one volume of a multi-volume set, or an
    /// entry is split across volumes; neither is supported.
    SplitArchiveUnsupported,
    /// The requested entry or directory index does not exist.
    NotFound,
    /// The imploded stream failed to decode.
    DecompressionFailed,
    /// The expanded byte count did not match the entry's recorded size.
    SizeMismatch,
    /// A caller-supplied ceiling was exceeded.
    LimitExceeded(Limit),
    /// The archive source failed.
    Source(SourceError),
    /// The caller's cancellation token was signalled.
    Cancelled,
}

impl Error {
    /// A fixed description carrying nothing archive-derived.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadSignature => "the InstallShield 3 archive signature did not match",
            Self::Truncated => "the source ended before the structure was complete",
            Self::OutOfRange => "a stored offset or length falls outside the archive",
            Self::InvalidInput => "a stored field combination failed validation",
            Self::SplitArchiveUnsupported => "split and multi-volume archives are not supported",
            Self::NotFound => "the requested index does not exist",
            Self::DecompressionFailed => "the imploded stream failed to decode",
            Self::SizeMismatch => "expanded size did not match the entry record",
            Self::LimitExceeded(_) => "a caller-supplied limit was exceeded",
            Self::Source(_) => "the archive source failed to read",
            Self::Cancelled => "the operation was cancelled",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded(limit) => write!(f, "limit exceeded: {limit}"),
            other => f.write_str(other.as_str()),
        }
    }
}

impl core::error::Error for Error {}

impl From<SourceError> for Error {
    fn from(value: SourceError) -> Self {
        Self::Source(value)
    }
}

impl From<Error> for SanitizedError {
    fn from(value: Error) -> Self {
        match value {
            Error::NotFound => Self::NotFound,
            Error::SplitArchiveUnsupported => Self::Unsupported,
            Error::Source(_) | Error::Cancelled => Self::Internal,
            _ => Self::InvalidInput,
        }
    }
}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, Error>;
