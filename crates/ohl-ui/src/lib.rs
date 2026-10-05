//! The Open Half-Life UI shell: an egui overlay providing a Quake-style
//! developer console, a data-driven HUD, the player-facing menus (main and
//! pause menu, single-player, multiplayer and options panes), rebindable
//! controls and a deathmatch scoreboard.
//!
//! This crate owns no window and no game state; [`layer::UiLayer`] is a
//! thin adapter from winit + wgpu to egui that the composition root
//! (`ohl-app`, not touched by this crate) will drive once per frame, and
//! [`console`], [`hud`], [`menu`] and [`scoreboard`] are pure state plus egui
//! draw calls the host feeds every frame, and [`bindings`] is the controls
//! table the menu edits and the host reads. Nothing here links a C library or contains
//! `unsafe` code; the workspace's `forbid(unsafe_code)` lint applies
//! unchanged.

mod layer;
mod root_ui;

pub mod bindings;
pub mod console;
pub mod debug;
pub mod hud;
pub mod menu;
pub mod scoreboard;

pub use layer::UiLayer;
pub use root_ui::root_ui;

/// Re-exported so callers can name egui/wgpu screen-size types without
/// pinning their own, possibly different, version of these crates.
pub use egui;
pub use egui_wgpu;
pub use winit;
