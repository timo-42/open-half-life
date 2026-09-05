//! GPU-ready face geometry.

use crate::culling::Aabb;

/// One vertex of the world mesh.
///
/// The layout is fixed and mirrored by `ohl-render`'s vertex buffer layout:
/// three floats of GoldSrc-space position, two of diffuse texture
/// coordinates, two of lightmap-atlas coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct WorldVertex {
    /// Position in GoldSrc world units (X forward, Y left, Z up).
    pub position: [f32; 3],
    /// Diffuse texture coordinates, already divided by the texture size.
    pub uv: [f32; 2],
    /// Lightmap-atlas texture coordinates, in `0..1` atlas space.
    pub lightmap_uv: [f32; 2],
}

/// The number of bytes one [`WorldVertex`] occupies in a vertex buffer.
pub const VERTEX_BYTES: usize = 7 * 4;

impl WorldVertex {
    /// Appends this vertex to `out` in the little-endian layout the renderer
    /// uploads.
    ///
    /// Serialising by hand keeps the crate free of a `bytemuck`-style
    /// `unsafe impl Pod`, which the workspace's `forbid(unsafe_code)` would
    /// reject anyway.
    pub fn write_le(&self, out: &mut Vec<u8>) {
        for value in self
            .position
            .iter()
            .chain(self.uv.iter())
            .chain(self.lightmap_uv.iter())
        {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// Where one map face's triangles live in the world index buffer.
#[derive(Debug, Clone, Copy)]
pub struct FaceGeometry {
    /// Index into [`crate::WorldModel::textures`].
    pub texture: usize,
    /// First entry in [`crate::WorldModel::indices`].
    pub first_index: u32,
    /// Number of indices, always a multiple of three.
    pub index_count: u32,
    /// World-space bounds, used by the frustum test.
    pub bounds: Aabb,
}

/// Serialises `vertices` into the renderer's little-endian vertex format.
#[must_use]
pub fn vertex_bytes(vertices: &[WorldVertex]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vertices.len() * VERTEX_BYTES);
    for vertex in vertices {
        vertex.write_le(&mut out);
    }
    out
}

/// Serialises `indices` into little-endian `u32`s.
#[must_use]
pub fn index_bytes(indices: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(indices.len() * 4);
    for index in indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{VERTEX_BYTES, WorldVertex, index_bytes, vertex_bytes};

    #[test]
    fn vertex_serialises_to_seven_floats() {
        let vertex = WorldVertex {
            position: [1.0, 2.0, 3.0],
            uv: [0.5, 0.25],
            lightmap_uv: [0.125, 0.0625],
        };
        let bytes = vertex_bytes(&[vertex]);
        assert_eq!(bytes.len(), VERTEX_BYTES);
        assert_eq!(&bytes[..4], &1.0f32.to_le_bytes());
        assert_eq!(&bytes[24..], &0.0625f32.to_le_bytes());
    }

    #[test]
    fn indices_serialise_little_endian() {
        assert_eq!(index_bytes(&[1, 258]), vec![1, 0, 0, 0, 2, 1, 0, 0]);
    }
}
