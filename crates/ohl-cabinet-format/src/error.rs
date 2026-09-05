//! Sanitized parse failures.
//!
//! Variants carry no media-derived bytes, offsets, names or sizes so that
//! `Display` output is always safe to log under `docs/MEDIA_IMPORT.md`.

use core::fmt;

/// The caller-supplied ceiling that a header exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// `Limits::max_header_bytes`.
    HeaderBytes,
    /// `Limits::max_files`.
    Files,
    /// `Limits::max_directories`.
    Directories,
    /// `Limits::max_file_groups`.
    FileGroups,
    /// `Limits::max_components`.
    Components,
    /// `Limits::max_name_bytes`.
    NameBytes,
    /// `Limits::max_volumes`.
    Volumes,
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::HeaderBytes => "header bytes",
            Self::Files => "file count",
            Self::Directories => "directory count",
            Self::FileGroups => "file group count",
            Self::Components => "component count",
            Self::NameBytes => "name bytes",
            Self::Volumes => "volume count",
        };
        f.write_str(name)
    }
}

/// Every way a cabinet header can fail validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The leading signature is not the InstallShield cabinet signature.
    #[error("input does not carry the InstallShield cabinet signature")]
    BadSignature,
    /// The leading signature is a Microsoft Cabinet (`MSCF`) instead.
    #[error("input is a Microsoft Cabinet, not an InstallShield cabinet")]
    MicrosoftCabinet,
    /// The decoded major version has no known structure layout.
    #[error("cabinet header version is not supported")]
    UnsupportedVersion,
    /// A stored offset points outside the supplied buffer.
    #[error("a stored offset lies outside the header buffer")]
    OffsetOutOfRange,
    /// The buffer ends inside a structure that must be read whole.
    #[error("the header buffer ends inside a structure")]
    Truncated,
    /// A structure is internally inconsistent.
    #[error("a header structure is malformed")]
    Malformed,
    /// A caller-supplied index is outside its table.
    #[error("an index lies outside its table")]
    IndexOutOfRange,
    /// A string is unterminated, or its encoding is truncated.
    #[error("a header string is unterminated or truncated")]
    InvalidString,
    /// An offset list refers back to a node it already visited.
    #[error("a header offset list contains a cycle")]
    LinkCycle,
    /// A caller-supplied limit was exceeded.
    #[error("limit exceeded: {0}")]
    LimitExceeded(Limit),
}
