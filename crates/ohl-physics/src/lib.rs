//! Clean-room GoldSrc-style collision hulls and player movement.
//!
//! This crate implements two things, both from public documentation only
//! (see `docs/FORMAT_SOURCES.md`, "Collision hulls and player movement", and
//! `docs/CLEAN_ROOM.md`):
//!
//! - [`hull`]: tracing a line segment through one of a BSP map's four
//!   pre-expanded collision hulls, the standard way a GoldSrc map answers
//!   "where does this player-sized box first hit something";
//! - [`movement`]: the player movement step built on top of those traces —
//!   gravity, ground detection, friction, ground and air acceleration,
//!   jumping, stepping up, sliding along planes, ducking, a basic water
//!   mode, ladders, long jumps, riding movers, and noclip.
//!
//! [`controller::PlayerController`] wraps the two into the fixed-timestep
//! object a host application drives from its frame loop.
//!
//! Everything is `no_std` + `alloc`, links no C libraries, and never panics
//! on malformed map data: [`hull::CollisionModel::from_bsp`] validates every
//! plane, node and leaf index once, at construction, so tracing afterwards is
//! total. Depth-limited traversal means a cyclic or malformed hull tree costs
//! a bounded amount of work instead of overflowing the stack.
//!
//! Coordinates are GoldSrc world units (X forward, Y left, Z up, one unit
//! ≈ one inch), the same space `ohl-world` uses.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod controller;
pub mod hull;
pub mod movement;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use controller::{ControllerInput, PlayerController};
pub use hull::{
    CollisionModel, DIST_EPSILON, HULL_SIZES, Hull, MAX_TRACE_DEPTH, Trace, contents,
    point_contents, trace_hull,
};
pub use movement::{
    LiquidKind, MoveConfig, MoveEvents, MoveInput, PlayerState, WaterLevel, categorize_liquid,
    in_ladder_volume, ladder_normal, player_move, player_move_events,
};

/// Re-exported so callers can use this crate's vector type without also
/// pinning `glam` themselves.
pub use glam::Vec3;
