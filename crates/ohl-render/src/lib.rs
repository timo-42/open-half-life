//! The Open Half-Life wgpu renderer.
//!
//! Following `PROMPT.md`, this crate targets wgpu's Vulkan backend on Linux
//! and Windows and its Metal backend on macOS. It links no C libraries and
//! contains no `unsafe` code: wgpu and winit are safe Rust APIs, and the
//! workspace's `forbid(unsafe_code)` lint applies here unchanged.
//!
//! Nothing in this crate requires a GPU to *compile* or to run its unit
//! tests. Every entry point that needs an adapter reports
//! [`RenderError::NoAdapter`] instead of panicking, so a headless CI runner
//! can exercise the whole setup path and skip only the parts that would
//! actually rasterise.
//!
//! The crate consumes [`ohl_world::WorldModel`] and knows nothing about file
//! formats, media, or the on-disk layout of a map.

mod camera;
mod error;
mod gpu;
mod light_styles;
pub mod math;
mod offscreen;
mod render_props;
mod renderer;
mod sky;
mod studio;
mod surface;
mod water;

pub use camera::{FreeFlyCamera, MoveInput};
pub use error::{RenderError, Result};
pub use gpu::{GpuContext, preferred_backends};
pub use light_styles::{LightStyles, MAX_LIGHT_STYLES, STYLE_HZ};
pub use offscreen::{OFFSCREEN_FORMAT, OffscreenTarget};
pub use render_props::{BlendKind, RenderMode, RenderProps};
pub use renderer::{DEPTH_FORMAT, WorldRenderer};
pub use sky::SkyRenderer;
pub use studio::{ModelInstance, StudioRenderer, placement};
pub use surface::WindowSurface;

/// Re-exported so callers can name wgpu types (surface targets, formats)
/// without pinning their own, possibly different, wgpu version.
pub use wgpu;
