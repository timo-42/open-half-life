//! Draw-list geometry for brush submodels 1.. (doors, buttons, platforms
//! and other brush entities), additive to [`crate::WorldModel`], which only
//! builds submodel 0 (worldspawn).
//!
//! A brush entity's own placement (origin/angles, and whichever offset the
//! map logic simulation has applied, e.g. a door mid-slide) is entity data
//! the caller supplies separately (see `ohl-game`'s `ModelInstance`); this
//! module only turns one BSP submodel's faces into GPU-ready geometry in
//! the model's own local space (which for GoldSrc brush models is the same
//! world space the map was compiled in, since brush entities do not
//! recentre their vertices around the model origin).
//!
//! Submodel faces are rendered fullbright (no lightmap atlas sampling):
//! packing a second atlas per submodel is not yet implemented, matching the
//! "not yet done" scope already recorded for submodel rendering in
//! `docs/MILESTONES.md`.

use ohl_formats::bsp30::{Bsp, Limits};

use crate::error::{Result, WorldError};
use crate::geometry::WorldVertex;
use crate::model::{DrawBatch, PendingFace, assemble, surface_coordinates};
use crate::texture::TextureImage;

/// One brush submodel's GPU-ready geometry.
#[derive(Debug, Default)]
pub struct BrushModelGeometry {
    /// The submodel's own vertex buffer.
    pub vertices: Vec<WorldVertex>,
    /// The submodel's own index buffer, texture-major ordered.
    pub indices: Vec<u32>,
    /// Contiguous per-texture index ranges spanning [`Self::indices`].
    pub batches: Vec<DrawBatch>,
}

/// Builds the draw-list geometry for `bsp`'s submodel `model_index`
/// (`BSP::models[model_index]`, the same index a `*N` `model` keyvalue
/// names).
///
/// `textures` should be the same resolved texture list a sibling
/// [`crate::WorldModel::build`] call produced for this BSP, so texture
/// indices line up; passing a shorter list is safe (out-of-range texture
/// indices are clamped) but will draw the wrong texture.
pub fn build_draw_list_for_model(
    bsp: &Bsp<'_>,
    limits: &Limits,
    textures: &[TextureImage],
    model_index: usize,
) -> Result<BrushModelGeometry> {
    if textures.is_empty() {
        return Ok(BrushModelGeometry::default());
    }

    let models = bsp.models(limits)?;
    let model = models.get(model_index).ok_or(WorldError::IndexOutOfRange)?;
    let faces = bsp.faces(limits)?;
    let texinfos = bsp.texinfo(limits)?;
    let vertices = bsp.vertices(limits)?;
    let edges = bsp.edges(limits)?;
    let surfedges = bsp.surfedges(limits)?;

    let first_face = usize::try_from(model.first_face.get()).unwrap_or(0);
    let face_count = usize::try_from(model.num_faces.get()).unwrap_or(0);
    let face_end = first_face
        .checked_add(face_count)
        .ok_or(WorldError::LimitExceeded)?;
    if face_end > faces.len() {
        return Err(WorldError::IndexOutOfRange);
    }

    let mut pending: Vec<PendingFace> = Vec::with_capacity(face_count);
    for face in &faces[first_face..face_end] {
        let texinfo = texinfos
            .get(face.texinfo.get() as usize)
            .ok_or(WorldError::IndexOutOfRange)?;
        let texture = (texinfo.miptex_index.get() as usize).min(textures.len() - 1);

        let mut positions = Vec::new();
        let num_edges = face.num_edges.get() as usize;
        let first_edge = face.first_edge.get() as usize;
        for step in 0..num_edges {
            let surfedge_index = first_edge
                .checked_add(step)
                .ok_or(WorldError::IndexOutOfRange)?;
            let surfedge = surfedges
                .get(surfedge_index)
                .ok_or(WorldError::IndexOutOfRange)?
                .0
                .get();
            let (edge_index, reversed) = if surfedge >= 0 {
                (surfedge.unsigned_abs() as usize, false)
            } else {
                (surfedge.unsigned_abs() as usize, true)
            };
            let edge = edges.get(edge_index).ok_or(WorldError::IndexOutOfRange)?;
            let vertex_index = usize::from(edge.vertices[usize::from(reversed)].get());
            let vertex = vertices
                .get(vertex_index)
                .ok_or(WorldError::IndexOutOfRange)?;
            positions.push([
                vertex.point[0].get(),
                vertex.point[1].get(),
                vertex.point[2].get(),
            ]);
        }
        if positions.len() < 3 {
            continue;
        }

        let (st, bounds) = surface_coordinates(&positions, texinfo)?;
        let texture_size = (
            f64::from(textures[texture].width()),
            f64::from(textures[texture].height()),
        );
        let mut face_vertices = Vec::with_capacity(positions.len());
        for (position, &(s, t)) in positions.iter().zip(st.iter()) {
            #[allow(clippy::cast_possible_truncation)]
            let uv = [
                (f64::from(s) / texture_size.0) as f32,
                (f64::from(t) / texture_size.1) as f32,
            ];
            face_vertices.push(WorldVertex {
                position: *position,
                uv,
                // No per-submodel lightmap atlas yet: sample the atlas
                // origin, which every `WorldModel` reserves as an opaque
                // white 1x1 tile, so submodels render fullbright rather
                // than sampling undefined data.
                lightmap_uv: [0.0, 0.0],
            });
        }
        pending.push(PendingFace {
            texture,
            vertices: face_vertices,
            bounds,
            // No per-submodel lightmap sampling here either (see the
            // `lightmap_uv` note above), so this reads as the same neutral
            // white an unlit worldspawn face gets.
            average_light: [1.0, 1.0, 1.0],
        });
    }

    let ((vertices, indices), _bounds, _faces, batches, _order) = assemble(&pending, (1.0, 1.0));
    Ok(BrushModelGeometry {
        vertices,
        indices,
        batches,
    })
}

#[cfg(test)]
mod tests {
    use super::build_draw_list_for_model;
    use crate::test_support::synthetic_room_bsp;
    use crate::texture::TextureImage;
    use ohl_formats::bsp30::Limits;

    #[test]
    fn model_zero_produces_geometry() {
        let bytes = synthetic_room_bsp();
        let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &Limits::default()).expect("valid bsp");
        let textures = vec![TextureImage::placeholder(); 8];
        let geometry = build_draw_list_for_model(&bsp, &Limits::default(), &textures, 0)
            .expect("model 0 builds");
        assert!(!geometry.vertices.is_empty());
        assert!(!geometry.indices.is_empty());
    }

    #[test]
    fn out_of_range_model_is_an_error() {
        let bytes = synthetic_room_bsp();
        let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &Limits::default()).expect("valid bsp");
        let textures = vec![TextureImage::placeholder(); 8];
        assert!(build_draw_list_for_model(&bsp, &Limits::default(), &textures, 99).is_err());
    }

    #[test]
    fn empty_textures_yields_empty_geometry() {
        let bytes = synthetic_room_bsp();
        let bsp = ohl_formats::bsp30::Bsp::parse(&bytes, &Limits::default()).expect("valid bsp");
        let geometry =
            build_draw_list_for_model(&bsp, &Limits::default(), &[], 0).expect("no textures ok");
        assert!(geometry.vertices.is_empty());
    }
}
