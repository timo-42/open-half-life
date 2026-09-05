//! Building a [`WorldModel`] from a validated BSP v30 file.

use ohl_formats::bsp30::{Bsp, Face, Limits, TexInfo};
use ohl_formats::wad3::{self, Wad3};

use crate::culling::{Aabb, Frustum};
use crate::error::{Result, WorldError};
use crate::geometry::{FaceGeometry, WorldVertex};
use crate::lightmap::{LUXEL_SIZE, LightmapExtents, ShelfPacker, ShelfRect, lightmap_extents};
use crate::spawn::{PlayerSpawn, find_player_start};
use crate::texture::{TextureImage, resolve};
use crate::vis::VisibilitySet;

/// `BSPTEXTUREINFO::flags` bit marking a surface the compiler did not light
/// (sky, and other "special" surfaces).
const TEX_SPECIAL: u32 = 0x1;

/// The light style value meaning "this slot is unused".
const STYLE_NONE: u8 = 0xFF;

/// The width, in pixels, of the packed lightmap atlas.
pub const LIGHTMAP_ATLAS_WIDTH: u32 = 1024;

/// The largest height the packed lightmap atlas may grow to.
pub const LIGHTMAP_ATLAS_MAX_HEIGHT: u32 = 4096;

/// The largest number of vertices this crate will emit for one map.
pub const MAX_VERTICES: usize = 4_000_000;

/// Options for [`WorldModel::build`].
#[derive(Default)]
pub struct WorldBuildOptions<'a> {
    /// Raw WAD3 packages consulted, in order, for externally stored
    /// textures. Textures not found in any of them become the checkerboard
    /// placeholder.
    pub wads: &'a [&'a [u8]],
    /// BSP decoding limits handed to `ohl-formats`.
    pub limits: Limits,
}

/// One contiguous run of indices sharing a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawBatch {
    /// Index into [`WorldModel::textures`].
    pub texture: usize,
    /// First entry in the index buffer.
    pub first_index: u32,
    /// Number of indices, always a multiple of three.
    pub index_count: u32,
}

/// A per-frame list of index ranges to draw, produced by
/// [`WorldModel::build_draw_list`].
#[derive(Debug, Default)]
pub struct DrawList {
    /// Indices into [`WorldModel::vertices`], grouped by [`Self::batches`].
    pub indices: Vec<u32>,
    /// One entry per texture that has visible geometry this frame.
    pub batches: Vec<DrawBatch>,
    /// Scratch marking which faces survived culling; retained across frames
    /// so a steady-state frame allocates nothing.
    visible: Vec<bool>,
}

impl DrawList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The total number of triangles the list draws.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// An owned, GPU-ready view of a map's worldspawn model.
pub struct WorldModel {
    /// Every vertex of the world mesh.
    pub vertices: Vec<WorldVertex>,
    /// The full index buffer, ordered so that all faces sharing a texture
    /// are contiguous.
    pub indices: Vec<u32>,
    /// The static, uncullled batches spanning [`Self::indices`].
    pub batches: Vec<DrawBatch>,
    /// Per-face index ranges, in the same texture-major order as
    /// [`Self::indices`].
    pub faces: Vec<FaceGeometry>,
    /// One decoded RGBA8 image per BSP texture slot.
    pub textures: Vec<TextureImage>,
    /// The packed RGBA8 lightmap atlas.
    pub lightmap_atlas: TextureImage,
    /// World-space bounds of the worldspawn model.
    pub bounds: Aabb,
    /// The map's `info_player_start`, when it has one.
    pub spawn: Option<PlayerSpawn>,
    nodes: Vec<WorldNode>,
    planes: Vec<WorldPlane>,
    /// `leaf_faces[leaf]` is a `(start, count)` slice of `leaf_face_list`.
    leaf_faces: Vec<(u32, u32)>,
    leaf_face_list: Vec<u32>,
    /// `face_light[ordered_face]` is that face's mean lightmap colour.
    face_light: Vec<[f32; 3]>,
    vis: VisibilitySet,
    max_walk_depth: u32,
}

struct WorldNode {
    plane: usize,
    children: [i32; 2],
}

struct WorldPlane {
    normal: [f32; 3],
    dist: f32,
}

/// Per-face data collected before the texture-major reordering.
struct PendingFace {
    texture: usize,
    vertices: Vec<WorldVertex>,
    bounds: Aabb,
    /// The mean of the face's style-0 lightmap samples, in `0..1`, used as
    /// the ambient approximation for entities standing near it.
    average_light: [f32; 3],
}

impl WorldModel {
    /// Builds the worldspawn model (submodel 0) from `bsp`.
    ///
    /// Submodels 1.. (doors, platforms, triggers) are deliberately skipped
    /// at this milestone; they need entity-driven transforms that do not
    /// exist yet.
    #[allow(clippy::too_many_lines)]
    pub fn build(bsp: &Bsp<'_>, options: &WorldBuildOptions<'_>) -> Result<Self> {
        let limits = &options.limits;
        let wad_limits = wad3::Limits::default();
        let mut wads = Vec::new();
        for bytes in options.wads {
            if let Ok(wad) = Wad3::parse(bytes, &wad_limits) {
                wads.push(wad);
            }
        }

        let directory = bsp.textures(limits)?;
        let mut textures = Vec::with_capacity(directory.len());
        for slot in 0..directory.len() {
            let miptex = directory.get(slot).unwrap_or(None);
            textures.push(resolve(miptex, &wads));
        }
        if textures.is_empty() {
            // A map with no texture slots still needs one batch target.
            textures.push(TextureImage::placeholder());
        }

        let models = bsp.models(limits)?;
        let model = models.first().ok_or(WorldError::NoWorldModel)?;
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

        let mut packer = ShelfPacker::new(LIGHTMAP_ATLAS_WIDTH, LIGHTMAP_ATLAS_MAX_HEIGHT, 1);
        let mut atlas_pixels: Vec<(ShelfRect, Vec<u8>)> = Vec::new();
        // Reserve a 1x1 opaque-white tile so fullbright faces have somewhere
        // to sample from.
        let white = packer.insert(1, 1).ok_or(WorldError::LimitExceeded)?;
        atlas_pixels.push((white, vec![255, 255, 255, 255]));

        let mut pending: Vec<PendingFace> = Vec::with_capacity(face_count);
        // `bsp_face_index -> pending slot`, so leaves can find their faces.
        let mut face_slot = vec![u32::MAX; faces.len()];
        let mut total_vertices = 0usize;

        for bsp_face in first_face..face_end {
            let face = &faces[bsp_face];
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
                // A negative surfedge traverses its edge backwards.
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
            total_vertices = total_vertices
                .checked_add(positions.len())
                .ok_or(WorldError::LimitExceeded)?;
            if total_vertices > MAX_VERTICES {
                return Err(WorldError::LimitExceeded);
            }

            let (st, bounds) = surface_coordinates(&positions, texinfo)?;
            let (min_s, max_s, min_t, max_t) = st_bounds(&st);
            let lit = face.lightmap_offset.get() >= 0
                && face.styles[0] != STYLE_NONE
                && texinfo.flags.get() & TEX_SPECIAL == 0;

            let tile = if lit {
                place_lightmap(bsp, face, min_s, max_s, min_t, max_t, limits, &mut packer)
                    .ok()
                    .flatten()
            } else {
                None
            };
            let average_light = match &tile {
                Some((rect, _, pixels)) => {
                    atlas_pixels.push((*rect, pixels.clone()));
                    average_rgb(pixels)
                }
                // An unlit or fullbright face contributes nothing useful to
                // the ambient estimate, so it reads as neutral white, the
                // same value its geometry is shaded with.
                None => [1.0, 1.0, 1.0],
            };

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
                let lightmap_uv = match &tile {
                    Some((rect, extents, _)) => atlas_uv(s, t, *rect, *extents),
                    None => white_uv(white),
                };
                face_vertices.push(WorldVertex {
                    position: *position,
                    uv,
                    lightmap_uv,
                });
            }

            face_slot[bsp_face] =
                u32::try_from(pending.len()).map_err(|_| WorldError::LimitExceeded)?;
            pending.push(PendingFace {
                texture,
                vertices: face_vertices,
                bounds,
                average_light,
            });
        }

        let atlas = compose_atlas(&packer, &atlas_pixels)?;
        #[allow(clippy::cast_precision_loss)]
        let atlas_scale = (atlas.width() as f32, atlas.height() as f32);

        let (mesh, model_bounds, ordered_faces, batches, order) = assemble(&pending, atlas_scale);
        let (mesh_vertices, mesh_indices) = mesh;
        let mut face_light = vec![[1.0f32; 3]; pending.len()];
        for (original_slot, face) in pending.iter().enumerate() {
            if let Some(slot) = face_light.get_mut(order[original_slot] as usize) {
                *slot = face.average_light;
            }
        }

        // Remap `bsp face -> ordered face`.
        for slot in &mut face_slot {
            if *slot != u32::MAX {
                *slot = order[*slot as usize];
            }
        }

        let leaves = bsp.leaves(limits)?;
        let marksurfaces = bsp.marksurfaces(limits)?;
        let mut leaf_faces = Vec::with_capacity(leaves.len());
        let mut leaf_face_list = Vec::new();
        for leaf in leaves {
            let start =
                u32::try_from(leaf_face_list.len()).map_err(|_| WorldError::LimitExceeded)?;
            let first = leaf.first_marksurface.get() as usize;
            let count = leaf.num_marksurfaces.get() as usize;
            for step in 0..count {
                let Some(mark) = marksurfaces.get(first.saturating_add(step)) else {
                    break;
                };
                let bsp_face = mark.0.get() as usize;
                if let Some(&slot) = face_slot.get(bsp_face)
                    && slot != u32::MAX
                {
                    leaf_face_list.push(slot);
                }
            }
            let end = u32::try_from(leaf_face_list.len()).map_err(|_| WorldError::LimitExceeded)?;
            leaf_faces.push((start, end - start));
        }

        let vis_offsets: Vec<i32> = leaves.iter().map(|leaf| leaf.vis_offset.get()).collect();
        let vis_lump = bsp
            .raw_lump(ohl_formats::bsp30::LumpId::Visibility, limits)
            .unwrap_or(&[]);
        let vis = VisibilitySet::build(vis_lump, &vis_offsets)?;

        let nodes = bsp
            .nodes(limits)?
            .iter()
            .map(|node| WorldNode {
                plane: node.plane.get() as usize,
                children: [
                    i32::from(node.children[0].get()),
                    i32::from(node.children[1].get()),
                ],
            })
            .collect();
        let planes = bsp
            .planes(limits)?
            .iter()
            .map(|plane| WorldPlane {
                normal: [
                    plane.normal[0].get(),
                    plane.normal[1].get(),
                    plane.normal[2].get(),
                ],
                dist: plane.dist.get(),
            })
            .collect();

        let spawn = bsp
            .entities(limits)
            .ok()
            .and_then(|entities| find_player_start(&entities));

        Ok(Self {
            vertices: mesh_vertices,
            indices: mesh_indices,
            batches,
            faces: ordered_faces,
            textures,
            lightmap_atlas: atlas,
            bounds: model_bounds,
            spawn,
            nodes,
            planes,
            leaf_faces,
            leaf_face_list,
            face_light,
            vis,
            max_walk_depth: limits.max_walk_depth,
        })
    }

    /// Finds the leaf containing `point` by walking the worldspawn node
    /// tree, or `None` when the walk exceeds its depth bound or hits an
    /// out-of-range reference.
    #[must_use]
    pub fn leaf_at(&self, point: [f32; 3]) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut current: i32 = 0;
        for _ in 0..self.max_walk_depth {
            if current < 0 {
                // `!child` is the leaf index, per the BSP child encoding.
                return usize::try_from(!current).ok();
            }
            let node = self.nodes.get(usize::try_from(current).ok()?)?;
            let plane = self.planes.get(node.plane)?;
            let distance = point[0] * plane.normal[0]
                + point[1] * plane.normal[1]
                + point[2] * plane.normal[2]
                - plane.dist;
            current = node.children[usize::from(distance < 0.0)];
        }
        None
    }

    /// The faces referenced by `leaf`.
    #[must_use]
    pub fn leaf_face_slots(&self, leaf: usize) -> &[u32] {
        let Some(&(start, count)) = self.leaf_faces.get(leaf) else {
            return &[];
        };
        let start = start as usize;
        let end = start + count as usize;
        self.leaf_face_list.get(start..end).unwrap_or(&[])
    }

    /// An approximate ambient light colour, in `0..1`, for an entity
    /// standing at `point`.
    ///
    /// GoldSrc's own entity lighting traces downward and samples the
    /// lightmap of the surface it lands on. This is a deliberately coarser
    /// approximation with the same intent: it averages the mean lightmap
    /// colour of every face the containing leaf references, which is cheap,
    /// never fails, and is documented as an approximation in
    /// `docs/MILESTONES.md`. A point outside the map, or in a leaf with no
    /// faces, reads as neutral white so a model is never invisible.
    #[must_use]
    pub fn ambient_at(&self, point: [f32; 3]) -> [f32; 3] {
        let Some(leaf) = self.leaf_at(point) else {
            return [1.0, 1.0, 1.0];
        };
        let slots = self.leaf_face_slots(leaf);
        let mut sum = [0.0f64; 3];
        let mut count = 0u32;
        for &slot in slots {
            let Some(light) = self.face_light.get(slot as usize) else {
                continue;
            };
            for axis in 0..3 {
                sum[axis] += f64::from(light[axis]);
            }
            count += 1;
        }
        if count == 0 {
            return [1.0, 1.0, 1.0];
        }
        let scale = f64::from(count);
        #[allow(clippy::cast_possible_truncation)]
        [
            (sum[0] / scale) as f32,
            (sum[1] / scale) as f32,
            (sum[2] / scale) as f32,
        ]
    }

    /// The decompressed visibility set.
    #[must_use]
    pub fn visibility(&self) -> &VisibilitySet {
        &self.vis
    }

    /// Fills `out` with the index ranges visible from `eye`.
    ///
    /// Faces are kept when the leaf that references them is in the eye
    /// leaf's PVS *and* their bounds intersect `frustum`. Passing `None` for
    /// the frustum disables the frustum half of the test; standing in a leaf
    /// with no visibility row (or outside the map entirely) disables the PVS
    /// half, so the fallback is always "draw more", never "draw less".
    pub fn build_draw_list(&self, eye: [f32; 3], frustum: Option<&Frustum>, out: &mut DrawList) {
        out.indices.clear();
        out.batches.clear();
        out.visible.clear();
        out.visible.resize(self.faces.len(), false);

        let eye_leaf = self.leaf_at(eye);
        match eye_leaf {
            Some(leaf) if leaf != 0 && self.vis.leaf_count() > 0 => {
                for other in 0..self.leaf_faces.len() {
                    if self.vis.is_visible(leaf, other) {
                        for &slot in self.leaf_face_slots(other) {
                            if let Some(flag) = out.visible.get_mut(slot as usize) {
                                *flag = true;
                            }
                        }
                    }
                }
            }
            _ => out.visible.fill(true),
        }

        let mut current: Option<DrawBatch> = None;
        for (slot, face) in self.faces.iter().enumerate() {
            if !out.visible[slot] {
                continue;
            }
            if let Some(frustum) = frustum
                && !frustum.intersects(&face.bounds)
            {
                continue;
            }
            let start = face.first_index as usize;
            let end = start + face.index_count as usize;
            let Some(range) = self.indices.get(start..end) else {
                continue;
            };
            match &mut current {
                Some(batch) if batch.texture == face.texture => {
                    batch.index_count += face.index_count;
                }
                _ => {
                    if let Some(batch) = current.take() {
                        out.batches.push(batch);
                    }
                    current = Some(DrawBatch {
                        texture: face.texture,
                        first_index: u32::try_from(out.indices.len()).unwrap_or(u32::MAX),
                        index_count: face.index_count,
                    });
                }
            }
            out.indices.extend_from_slice(range);
        }
        if let Some(batch) = current {
            out.batches.push(batch);
        }
    }
}

/// Projects each position onto the face's texture axes, returning the
/// `(s, t)` pairs and the face's world-space bounds.
fn surface_coordinates(
    positions: &[[f32; 3]],
    texinfo: &TexInfo,
) -> Result<(Vec<(f32, f32)>, Aabb)> {
    let s_vector = [
        texinfo.s_vector[0].get(),
        texinfo.s_vector[1].get(),
        texinfo.s_vector[2].get(),
    ];
    let t_vector = [
        texinfo.t_vector[0].get(),
        texinfo.t_vector[1].get(),
        texinfo.t_vector[2].get(),
    ];
    let s_shift = texinfo.s_shift.get();
    let t_shift = texinfo.t_shift.get();

    let mut st = Vec::with_capacity(positions.len());
    let mut bounds = Aabb::empty();
    for position in positions {
        if !position.iter().all(|v| v.is_finite()) {
            return Err(WorldError::NonFiniteGeometry);
        }
        bounds.extend(*position);
        let s = position[0] * s_vector[0]
            + position[1] * s_vector[1]
            + position[2] * s_vector[2]
            + s_shift;
        let t = position[0] * t_vector[0]
            + position[1] * t_vector[1]
            + position[2] * t_vector[2]
            + t_shift;
        if !s.is_finite() || !t.is_finite() {
            return Err(WorldError::NonFiniteGeometry);
        }
        st.push((s, t));
    }
    Ok((st, bounds))
}

fn st_bounds(st: &[(f32, f32)]) -> (f32, f32, f32, f32) {
    let mut min_s = f32::INFINITY;
    let mut max_s = f32::NEG_INFINITY;
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    for &(s, t) in st {
        min_s = min_s.min(s);
        max_s = max_s.max(s);
        min_t = min_t.min(t);
        max_t = max_t.max(t);
    }
    (min_s, max_s, min_t, max_t)
}

/// Computes a face's lightmap extents, reads its style-0 samples, and packs
/// them into the atlas. Returns `Ok(None)` when the face's samples are
/// unavailable or the atlas is full, in which case it renders fullbright.
#[allow(clippy::too_many_arguments)]
fn place_lightmap(
    bsp: &Bsp<'_>,
    face: &Face,
    min_s: f32,
    max_s: f32,
    min_t: f32,
    max_t: f32,
    limits: &Limits,
    packer: &mut ShelfPacker,
) -> Result<Option<(ShelfRect, LightmapExtents, Vec<u8>)>> {
    let extents = lightmap_extents(min_s, max_s, min_t, max_t)?;
    let Ok(samples) =
        bsp.lightmap_samples(face.lightmap_offset.get(), extents.sample_count(), limits)
    else {
        return Ok(None);
    };
    let Some(rect) = packer.insert(extents.width, extents.height) else {
        return Ok(None);
    };
    let mut pixels = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        pixels.extend_from_slice(&[sample.r, sample.g, sample.b, 255]);
    }
    Ok(Some((rect, extents, pixels)))
}

/// Lightmap-atlas coordinates for one vertex, sampling luxel centres.
fn atlas_uv(s: f32, t: f32, rect: ShelfRect, extents: LightmapExtents) -> [f32; 2] {
    #[allow(clippy::cast_precision_loss)]
    let (min_s, min_t) = (extents.min_s as f32, extents.min_t as f32);
    #[allow(clippy::cast_precision_loss)]
    let (x, y) = (rect.x as f32, rect.y as f32);
    let luxel_s = ((s - min_s) / LUXEL_SIZE).clamp(0.0, f32::from(u16::MAX));
    let luxel_t = ((t - min_t) / LUXEL_SIZE).clamp(0.0, f32::from(u16::MAX));
    // Stored in atlas *pixels*; `assemble` scales to 0..1 once the atlas
    // height is known.
    [x + luxel_s + 0.5, y + luxel_t + 0.5]
}

/// The mean colour of an RGBA8 tile, in `0..1`.
fn average_rgb(pixels: &[u8]) -> [f32; 3] {
    let (texels, _) = pixels.as_chunks::<4>();
    if texels.is_empty() {
        return [1.0, 1.0, 1.0];
    }
    let mut sum = [0.0f64; 3];
    for texel in texels {
        for (axis, total) in sum.iter_mut().enumerate() {
            *total += f64::from(texel[axis]);
        }
    }
    // A tile is at most a few thousand texels, so the divisor is exact in
    // `f64`; the cast back to `f32` is the only rounding step.
    let scale = f64::from(u32::try_from(texels.len()).unwrap_or(u32::MAX)) * 255.0;
    #[allow(clippy::cast_possible_truncation)]
    [
        (sum[0] / scale) as f32,
        (sum[1] / scale) as f32,
        (sum[2] / scale) as f32,
    ]
}

fn white_uv(white: ShelfRect) -> [f32; 2] {
    #[allow(clippy::cast_precision_loss)]
    [white.x as f32 + 0.5, white.y as f32 + 0.5]
}

fn compose_atlas(packer: &ShelfPacker, tiles: &[(ShelfRect, Vec<u8>)]) -> Result<TextureImage> {
    let width = packer.width();
    let height = packer.used_height().max(1);
    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    for (rect, pixels) in tiles {
        for row in 0..rect.height {
            let src = (row as usize) * (rect.width as usize) * 4;
            let dst_y = rect.y as usize + row as usize;
            if dst_y >= height as usize {
                continue;
            }
            let dst = (dst_y * width as usize + rect.x as usize) * 4;
            let len = (rect.width as usize) * 4;
            if src + len > pixels.len() || dst + len > rgba.len() {
                continue;
            }
            rgba[dst..dst + len].copy_from_slice(&pixels[src..src + len]);
        }
    }
    TextureImage::new(width, height, rgba)
}

type Mesh = (Vec<WorldVertex>, Vec<u32>);

/// Reorders faces so that all faces sharing a texture are contiguous, emits
/// the vertex and index buffers, and scales lightmap coordinates (still in
/// atlas pixels at this point) into `0..1`.
///
/// Returns the mesh, the model bounds, the reordered faces, the static
/// batches, and a `pending slot -> ordered slot` permutation.
fn assemble(
    pending: &[PendingFace],
    atlas_scale: (f32, f32),
) -> (Mesh, Aabb, Vec<FaceGeometry>, Vec<DrawBatch>, Vec<u32>) {
    let mut order: Vec<usize> = (0..pending.len()).collect();
    order.sort_by_key(|&slot| pending[slot].texture);

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut faces = Vec::with_capacity(pending.len());
    let mut batches: Vec<DrawBatch> = Vec::new();
    let mut permutation = vec![0u32; pending.len()];
    let mut bounds = Aabb::empty();

    for (ordered_slot, &original_slot) in order.iter().enumerate() {
        let face = &pending[original_slot];
        let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        let first_index = u32::try_from(indices.len()).unwrap_or(u32::MAX);
        for vertex in &face.vertices {
            let mut vertex = *vertex;
            vertex.lightmap_uv = [
                vertex.lightmap_uv[0] / atlas_scale.0.max(1.0),
                vertex.lightmap_uv[1] / atlas_scale.1.max(1.0),
            ];
            vertices.push(vertex);
        }
        // Triangle fan around the first vertex; GoldSrc faces are convex.
        for corner in 1..face.vertices.len().saturating_sub(1) {
            indices.push(base);
            indices.push(base + u32::try_from(corner).unwrap_or(0));
            indices.push(base + u32::try_from(corner + 1).unwrap_or(0));
        }
        let index_count = u32::try_from(indices.len()).unwrap_or(u32::MAX) - first_index;
        if face.bounds.is_valid() {
            bounds.extend(face.bounds.min);
            bounds.extend(face.bounds.max);
        }
        faces.push(FaceGeometry {
            texture: face.texture,
            first_index,
            index_count,
            bounds: face.bounds,
        });
        permutation[original_slot] = u32::try_from(ordered_slot).unwrap_or(u32::MAX);

        match batches.last_mut() {
            Some(batch) if batch.texture == face.texture => batch.index_count += index_count,
            _ => batches.push(DrawBatch {
                texture: face.texture,
                first_index,
                index_count,
            }),
        }
    }

    ((vertices, indices), bounds, faces, batches, permutation)
}
