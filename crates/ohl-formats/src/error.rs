//! A fixed, project-owned error enum for the format decoders in this crate.
//!
//! Every variant is a fixed code with a fixed `Display` message: none of them
//! ever carry file-derived bytes, offsets, or names, so this error type is
//! always safe to log or report (see `ohl_core::SanitizedError`, which this
//! crate mirrors in spirit but does not depend on, since this crate must stay
//! independently useful without pulling in `ohl-core`'s diagnostics policy
//! for a simple decode failure).

use core::fmt;

/// A bounds or validation failure while decoding a BSP30 or WAD3 file.
///
/// Decoders in this crate never panic on malformed input; every fallible
/// operation returns one of these fixed variants instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FormatError {
    /// The buffer is too short to contain a fixed-size header or record.
    Truncated,
    /// A magic/signature/version field did not match the expected value.
    BadSignature,
    /// A lump or entry offset/length falls outside the containing buffer.
    OutOfBounds,
    /// A lump or slice length is not an exact multiple of its element size.
    SizeNotMultiple,
    /// An index (into planes, vertices, faces, textures, ...) is out of
    /// range for the referenced table.
    IndexOutOfRange,
    /// A count exceeds a configured [`crate::bsp30::Limits`] or
    /// [`crate::wad3::Limits`] bound.
    LimitExceeded,
    /// Text data (for example the entities lump) was not validly encoded or
    /// was not terminated as required.
    InvalidText,
    /// Recursion (node walk, run-length decode) exceeded a bounded depth,
    /// which would otherwise indicate a cycle in attacker-controlled data.
    RecursionLimitExceeded,
    /// The input otherwise failed validation (malformed field combination).
    InvalidInput,
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Truncated => "buffer is too short for the expected structure",
            Self::BadSignature => "signature or version field did not match",
            Self::OutOfBounds => "offset or length falls outside the buffer",
            Self::SizeNotMultiple => "lump length is not a multiple of the element size",
            Self::IndexOutOfRange => "index is out of range for the referenced table",
            Self::LimitExceeded => "count exceeds the configured limit",
            Self::InvalidText => "text data was not validly encoded or terminated",
            Self::RecursionLimitExceeded => "recursion exceeded the bounded depth limit",
            Self::InvalidInput => "input failed validation",
        };
        f.write_str(message)
    }
}

/// A `Result` alias for this crate's decoders.
pub type Result<T> = core::result::Result<T, FormatError>;
