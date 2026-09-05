//! Sanitized diagnostics for `ohl-assets`.
//!
//! Every variant is a fixed code with a fixed `Display` message. No variant
//! ever carries an asset path, a filesystem path, an OS error message, or
//! any other value derived from the user's media: those are exactly the
//! strings this crate's logging policy (see the crate root docs) forbids
//! from ever reaching a log line or an error message.

use core::fmt;

/// A failure while building or querying an [`crate::AssetFs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssetError {
    /// The requested asset does not exist in any configured search path.
    NotFound,
    /// A supplied path failed the game-relative path policy (absolute,
    /// traversing, empty, over length, or otherwise invalid).
    InvalidPath,
    /// A configured [`crate::Limits`] bound was exceeded while indexing or
    /// listing (too many files, too deep, a name too long, ...).
    LimitExceeded,
    /// The payload root directory could not be read.
    RootUnreadable,
    /// A PAK archive under a search path failed to parse.
    MalformedArchive,
    /// An I/O error occurred while opening or reading an asset.
    Io,
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NotFound => "requested asset was not found in any search path",
            Self::InvalidPath => "path failed the game-relative path policy",
            Self::LimitExceeded => "count exceeds the configured limit",
            Self::RootUnreadable => "payload root directory could not be read",
            Self::MalformedArchive => "a PAK archive failed to parse",
            Self::Io => "an I/O error occurred",
        };
        f.write_str(message)
    }
}

impl core::error::Error for AssetError {}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, AssetError>;
