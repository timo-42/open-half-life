//! Sanitized extraction failures and anti-abuse ceilings.

use core::fmt;

use ohl_cabinet_format::FormatError;

/// The caller-supplied ceiling that extraction exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// `Limits::max_expanded_bytes_per_file`.
    ExpandedBytesPerFile,
    /// `Limits::max_total_expanded_bytes`.
    TotalExpandedBytes,
    /// `Limits::max_volumes`.
    Volumes,
    /// `Limits::max_volume_hops`.
    VolumeHops,
    /// `Limits::max_link_steps`.
    LinkSteps,
    /// `Limits::max_chunk_bytes`.
    ChunkBytes,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::ExpandedBytesPerFile => "expanded bytes per file",
            Self::TotalExpandedBytes => "total expanded bytes",
            Self::Volumes => "volume number",
            Self::VolumeHops => "volume hops",
            Self::LinkSteps => "split link steps",
            Self::ChunkBytes => "compressed chunk bytes",
        };
        f.write_str(name)
    }
}

/// An opaque, sanitized failure reported by a [`crate::VolumeSource`].
///
/// The trait deliberately cannot surface an I/O error's text, path or errno
/// into this crate, so nothing media-derived can reach a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VolumeError;

impl fmt::Display for VolumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the volume source failed to read")
    }
}

impl core::error::Error for VolumeError {}

/// Every way extraction can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The cabinet header itself failed validation.
    #[error("malformed cabinet header: {0}")]
    Header(#[from] FormatError),
    /// A volume header is absent, short, or internally inconsistent.
    #[error("malformed volume header")]
    MalformedVolumeHeader,
    /// A stored offset lies outside the volume.
    #[error("a stored offset lies outside the volume")]
    OffsetOutOfRange,
    /// The header version has no known volume-header layout.
    #[error("cabinet version is not supported")]
    UnsupportedVersion,
    /// A volume ended before the expected number of bytes was read.
    #[error("a volume ended before the file did")]
    TruncatedVolume,
    /// A compressed chunk failed to inflate.
    #[error("decompression failed")]
    DecompressionFailed,
    /// The file is compressed but the `inflate` feature is disabled.
    #[error("compressed files require the `inflate` feature")]
    CompressionUnsupported,
    /// The descriptor is a placeholder or names no data.
    #[error("the file descriptor names no extractable data")]
    InvalidFile,
    /// The split-link graph revisits a descriptor.
    #[error("the split link graph contains a cycle")]
    LinkCycle,
    /// The expanded byte count did not match the descriptor.
    #[error("expanded size did not match the descriptor")]
    SizeMismatch,
    /// The recorded MD5 digest did not match the expanded bytes.
    #[error("digest verification failed")]
    DigestMismatch,
    /// The volume source failed.
    #[error("volume read failed")]
    Volume(#[from] VolumeError),
    /// A caller-supplied limit was exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(Limit),
}
