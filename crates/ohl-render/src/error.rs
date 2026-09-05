//! A fixed, project-owned error enum for renderer setup.
//!
//! As everywhere else in this project, no variant carries media- or
//! environment-derived text, so a `RenderError` is always safe to log.

use core::fmt;

/// A renderer setup or world-upload failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RenderError {
    /// No graphics adapter matched the requested backends. On a machine
    /// without a GPU (a CI runner, typically) this is the expected outcome.
    NoAdapter,
    /// An adapter was found but would not produce a device.
    NoDevice,
    /// A surface could not be created for the supplied window handle.
    NoSurface,
    /// The surface offered no texture format this renderer can use.
    UnsupportedSurface,
    /// Reading pixels back from an offscreen target failed.
    Readback,
    /// The world model does not fit this device's limits.
    WorldTooLarge,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NoAdapter => "no graphics adapter is available",
            Self::NoDevice => "the graphics adapter would not create a device",
            Self::NoSurface => "a render surface could not be created",
            Self::UnsupportedSurface => "the surface offers no usable texture format",
            Self::Readback => "reading pixels back from the GPU failed",
            Self::WorldTooLarge => "the world exceeds this device's limits",
        };
        f.write_str(message)
    }
}

impl std::error::Error for RenderError {}

/// A `Result` alias for this crate.
pub type Result<T> = core::result::Result<T, RenderError>;
