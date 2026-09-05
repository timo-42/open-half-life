//! Failure reasons, carrying no media-derived detail.

use core::fmt;

/// Why a level could not be loaded or drawn.
///
/// Deliberately opaque: a variant names the *step* that failed, never the
/// asset, path or size involved, so it is safe to surface anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    /// The map is not published in the asset source.
    MapNotFound,
    /// The map's bytes are not a BSP v30 map this build can read.
    MapUnreadable,
    /// The map parsed but could not be turned into a renderable world.
    WorldUnbuildable,
    /// A GPU resource could not be created.
    Renderer,
    /// A save file could not be built or written.
    SaveUnwritable,
    /// A save file could not be read, or does not hold the sections this
    /// build needs.
    SaveUnreadable,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MapNotFound => "the requested map is not present in the payload",
            Self::MapUnreadable => "the map is not a BSP v30 map this build can read",
            Self::WorldUnbuildable => "the map could not be turned into a renderable world",
            Self::Renderer => "the renderer could not be created",
            Self::SaveUnwritable => "the save file could not be written",
            Self::SaveUnreadable => "the save file could not be read",
        };
        f.write_str(message)
    }
}

impl std::error::Error for EngineError {}

/// This crate's result alias.
pub type Result<T> = core::result::Result<T, EngineError>;
