//! The Open Half-Life game state: one struct that owns a loaded level and
//! everything that acts on it, so a host binary stays a thin composition
//! root.
//!
//! [`Game`] owns the renderable [`ohl_world::WorldModel`], the entity
//! [`ohl_game::Registry`], the map-logic [`ohl_game::Simulation`], the
//! collision hulls and [`ohl_physics::PlayerController`], and (once a GPU
//! context is attached) the renderer handles. It exposes exactly two verbs:
//! [`Game::tick`] advances a frame from an [`Input`] snapshot and returns
//! the [`GameEvent`]s the host must act on, and [`Game::render`] draws the
//! current frame into a caller-supplied target.
//!
//! Every asset this crate needs arrives through the [`AssetSource`] trait,
//! so a host can back it with an imported payload
//! ([`AssetFsSource`], over [`ohl_assets::AssetFs`]) and a test can back it
//! with bytes it built itself. This crate performs no I/O of its own.
//!
//! # Logging
//!
//! Nothing here logs. Names, paths, counts and sizes read out of a map are
//! media-derived, so they are returned to the caller as data (for example
//! [`Game::missing_model_count`]) rather than written to a diagnostic.
#![forbid(unsafe_code)]

mod assets;
mod error;
mod game;
mod input;
mod level;
mod render;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use assets::{AssetFsSource, AssetSource, MemoryAssets};
pub use error::{EngineError, Result};
pub use game::{Game, GameEvent};
pub use input::Input;
pub use render::RenderTarget;

/// How far the mouse turns the player, in degrees per pixel.
pub const MOUSE_SENSITIVITY: f32 = 0.15;

/// How close (in GoldSrc units) the player must be to a door or button for
/// "use" to reach it.
pub const USE_RADIUS: f32 = 64.0;

/// The largest simulation step a single [`Game::tick`] applies, so a stalled
/// frame cannot tunnel the player through the world.
pub const MAX_TICK_SECONDS: f32 = 0.1;
