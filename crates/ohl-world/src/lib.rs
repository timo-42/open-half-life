//! A renderable world model built from clean-room GoldSrc BSP v30 data.
//!
//! This crate turns the borrowing, validated views published by
//! [`ohl_formats::bsp30`] into owned, GPU-ready data: triangulated face
//! geometry with diffuse and lightmap texture coordinates, decoded RGBA
//! textures, a packed lightmap atlas, and the visibility information a
//! renderer needs to cull what it draws.
//!
//! It performs no I/O, links no C libraries, and never panics on malformed
//! input: every fallible step returns [`WorldError`]. Nothing here is
//! derived from any game installation; see `docs/CLEAN_ROOM.md`.
//!
//! Coordinates stay in GoldSrc's own space (X forward, Y left, Z up, one
//! unit ≈ one inch). Converting to a renderer's clip space is the
//! renderer's job (`ohl-render`), so the culling planes in [`Frustum`] are
//! expressed in the same space as the vertices.

mod culling;
mod error;
mod geometry;
mod lightmap;
mod model;
mod spawn;
mod texture;
mod vis;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use culling::{Aabb, Frustum};
pub use error::{Result, WorldError};
pub use geometry::{FaceGeometry, VERTEX_BYTES, WorldVertex, index_bytes, vertex_bytes};
pub use lightmap::{LightmapExtents, ShelfPacker, ShelfRect, lightmap_extents};
pub use model::{
    DrawBatch, DrawList, LIGHTMAP_ATLAS_MAX_HEIGHT, LIGHTMAP_ATLAS_WIDTH, WorldBuildOptions,
    WorldModel,
};
pub use spawn::{PlayerSpawn, find_player_start};
pub use texture::TextureImage;

/// Re-exported so callers can pass decoding limits without also depending
/// on `ohl-formats` directly.
pub use ohl_formats::bsp30::Limits as BspLimits;
pub use vis::VisibilitySet;
