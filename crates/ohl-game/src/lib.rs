//! Entity registry and deterministic map logic for Open Half-Life.
//!
//! This crate turns the raw keyvalue maps the BSP entities lump yields
//! (`ohl_formats::bsp30::entities::parse`) into typed [`keyvalues::EntityDef`]
//! values, populates a [`hecs`] world with one entity per map entity plus a
//! bounded `targetname -> entity` index ([`registry::Registry`]), exposes
//! per-entity brush-model instancing data for the renderer
//! ([`brush::ModelInstance`]), and runs a small, deterministic,
//! fixed-timestep map logic simulation ([`logic::Simulation`]) covering
//! doors, buttons, platforms, `multi_manager` fan-out, triggers and level
//! changes.
//!
//! No rendering, audio, physics, AI or combat lives here; see
//! `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic") for the
//! public documentation this crate's entity semantics were implemented
//! from, and `docs/CLEAN_ROOM.md` for the project's clean-room policy.
#![forbid(unsafe_code)]

pub mod brush;
pub mod camera;
pub mod keyvalues;
pub mod logic;
pub mod registry;
pub mod scripts;
pub mod track_train;

/// Re-exported so a caller can name `Entity`, `World` and the query types
/// this crate's `Registry` exposes without pinning its own, possibly
/// different, version of the ECS.
pub use hecs;

pub use brush::ModelInstance;
pub use camera::TriggerCameraState;
pub use keyvalues::{EntityDef, Limits as KeyvalueLimits, ModelRef};
pub use logic::{
    Event, LevelChange, PendingFire, Simulation, SimulationState, TriggerSnapshot,
    find_usable_within,
};
pub use registry::{Ladder, Registry, TRIGGER_HURT_INTERVAL_SECONDS, TriggerCamera, TriggerHurt};
pub use track_train::{PathChain, PathNode, TrackTrain, TrackTrainState};
