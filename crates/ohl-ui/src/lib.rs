//! The Open Half-Life UI shell: an egui overlay providing a Quake-style
//! developer console, a data-driven HUD and a menu skeleton (main menu,
//! pause menu, options, bindings placeholder).
//!
//! This crate owns no window and no game state; [`layer::UiLayer`] is a
//! thin adapter from winit + wgpu to egui that the composition root
//! (`ohl-app`, not touched by this crate) will drive once per frame, and
//! [`console`], [`hud`] and [`menu`] are pure state plus egui draw calls the
//! host feeds every frame. Nothing here links a C library or contains
//! `unsafe` code; the workspace's `forbid(unsafe_code)` lint applies
//! unchanged.

mod layer;
mod root_ui;

pub mod console;
pub mod hud;
pub mod menu;

pub use layer::UiLayer;
pub use root_ui::root_ui;

/// Re-exported so callers can name egui/wgpu screen-size types without
/// pinning their own, possibly different, version of these crates.
pub use egui;
pub use egui_wgpu;
pub use winit;
