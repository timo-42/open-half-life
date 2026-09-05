//! A fixed, project-owned error enum for world-model construction.
//!
//! Like [`ohl_formats::FormatError`], every variant is a fixed code with a
//! fixed `Display` message: none of them ever carries map-derived bytes,
//! names, indices, or offsets, so a `WorldError` is always safe to log.

use core::fmt;

use ohl_formats::FormatError;

/// A failure while building a [`crate::WorldModel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorldError {
    /// A lump failed to decode or validate.
    Format(FormatError),
    /// The map declares no submodels, so there is no worldspawn model to
    /// build geometry from.
    NoWorldModel,
    /// A record referenced an index outside the table it points into.
    IndexOutOfRange,
    /// A count, size, or byte total exceeded this crate's configured
    /// ceilings.
    LimitExceeded,
    /// A face's texture coordinates are not finite, so its lightmap extents
    /// cannot be computed.
    NonFiniteGeometry,
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Format(_) => "map lump failed to decode",
            Self::NoWorldModel => "map declares no worldspawn submodel",
            Self::IndexOutOfRange => "index is out of range for the referenced table",
            Self::LimitExceeded => "count exceeds the configured limit",
            Self::NonFiniteGeometry => "face geometry is not finite",
        };
        f.write_str(message)
    }
}

impl std::error::Error for WorldError {}

impl From<FormatError> for WorldError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, WorldError>;
