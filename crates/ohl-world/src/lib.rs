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

pub mod brush;
mod culling;
mod error;
mod geometry;
mod lightmap;
mod model;
mod sky;
mod spawn;
mod sprite;
mod studio;
mod texture;
mod vis;
mod water;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use brush::{BrushModelGeometry, build_draw_list_for_model};
pub use culling::{Aabb, Frustum};
pub use error::{Result, WorldError};
pub use geometry::{FaceGeometry, VERTEX_BYTES, WorldVertex, index_bytes, vertex_bytes};
pub use lightmap::{
    LightRamp, LightRampTable, LightmapExtents, ShelfPacker, ShelfRect, lightmap_extents,
};
pub use model::{
    DrawBatch, DrawList, LIGHTMAP_ATLAS_MAX_HEIGHT, LIGHTMAP_ATLAS_WIDTH, SubmodelSet,
    WorldBuildOptions, WorldModel,
};
pub use sky::{SKY_FACE_SUFFIXES, SkyboxAsset, is_sky_texture};
pub use spawn::{PlayerSpawn, find_player_start};
pub use sprite::{MAX_SPRITE_FRAMERATE, SpriteAsset};
pub use studio::{
    BoneMatrix, MAX_BONES, MAX_STUDIO_VERTICES, STUDIO_LOOPING, STUDIO_NF_ADDITIVE,
    STUDIO_NF_ALPHA, STUDIO_NF_CHROME, STUDIO_NF_FLATSHADE, STUDIO_NF_FULLBRIGHT, STUDIO_NF_MASKED,
    STUDIO_NF_NOMIPS, STUDIO_VERTEX_BYTES, StudioAttachment, StudioBodyPart, StudioBone,
    StudioHitbox, StudioMesh, StudioModel, StudioPose, StudioSequence, StudioSubModel,
    StudioTexture, StudioVertex, studio_vertex_bytes,
};
pub use texture::TextureImage;
pub use water::is_liquid_texture;

/// Re-exported so callers can pass decoding limits without also depending
/// on `ohl-formats` directly.
pub use ohl_formats::bsp30::Limits as BspLimits;

/// Re-exported so callers can pass studio-model decoding limits without
/// also depending on `ohl-formats` directly.
pub use ohl_formats::mdl10::Limits as StudioLimits;

/// Re-exported so callers can pass sprite decoding limits, and read a
/// sprite's orientation/blend metadata, without also depending on
/// `ohl-formats` directly.
pub use ohl_formats::spr::{
    Limits as SprLimits, SpriteType, SyncType, TextureFormat as SprTextureFormat,
};
pub use vis::VisibilitySet;
