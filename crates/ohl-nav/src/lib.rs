//! Clean-room navigation for monster movement: a node graph, A* over it, and
//! local steering for the last leg.
//!
//! The crate is deliberately independent of the entity layer. A host collects
//! node positions (in Half-Life maps these come from the `info_node` and
//! `info_node_air` point entities, whose published keyvalues are recorded in
//! `docs/FORMAT_SOURCES.md`, "Navigation") and hands them to
//! [`graph::NodeGraph::build`] together with the map's
//! [`ohl_physics::CollisionModel`]. Nothing here reads game data, and nothing
//! here knows about `ohl-game` components; [`graph::node_seeds_from_entities`]
//! is the one convenience that reads an already-parsed BSP entities lump.
//!
//! The three layers are:
//!
//! - [`graph`]: ground-snapping the nodes, then validating every candidate
//!   link once per collision hull with hull traces, so a link that a
//!   32x32x72 humanoid can walk but a 64x64x64 monster cannot is stored as
//!   such. The result is a bounded, deterministic, optionally serialisable
//!   [`graph::NodeGraph`].
//! - [`path`]: A* with a Euclidean heuristic over the per-hull link subgraph,
//!   attaching arbitrary start and goal positions to the nearest node that a
//!   hull trace can actually reach, plus a
//!   [`path::straight_path_if_clear`] shortcut for the common open-room case.
//! - [`steer`]: turning a [`path::Path`] into a per-tick
//!   [`steer::MoveIntent`] with waypoint advancement, wall sliding and stuck
//!   detection.
//!
//! Everything is `std`, allocates only bounded amounts (every limit is an
//! explicit field of [`graph::BuildLimits`], [`path::PathLimits`] or
//! [`steer::SteerLimits`]), links no C libraries, forbids `unsafe`, and never
//! panics on adversarial input: the traces come from `ohl-physics`, which is
//! itself total over a validated collision model.
//!
//! Coordinates are GoldSrc world units (X forward, Y left, Z up), the same
//! space `ohl-world` and `ohl-physics` use.
//!
//! No Valve source, SDK code or decompiled node-graph logic was consulted;
//! see `docs/CLEAN_ROOM.md`.

pub mod graph;
pub mod path;
pub mod steer;

pub use graph::{BuildLimits, Link, Node, NodeGraph, NodeKind, NodeSeed, node_seeds_from_entities};
pub use path::{Path, PathLimits, find_path, straight_path_if_clear};
pub use steer::{MoveIntent, Steer, SteerLimits};

/// Re-exported so callers can use this crate's vector type without also
/// pinning `glam` themselves.
pub use glam::Vec3;
