//! GPU-ready studio models ("MDL v10") built from [`ohl_formats::mdl10`].
//!
//! This module is the model-space counterpart of [`crate::WorldModel`]: it
//! turns the borrowing, validated views published by `ohl-formats` into
//! owned, indexed triangle geometry with per-vertex bone indices, decoded
//! RGBA textures, skin families, and everything an animation sampler needs.
//! It performs no I/O and never panics on malformed input; every fallible
//! step returns [`crate::WorldError`].
//!
//! Coordinates stay in the model's own space (the same axis convention as
//! the world: X forward, Y left, Z up). Placing a model in the world is the
//! caller's job, and skinning is the renderer's: this module publishes
//! per-bone world matrices ([`StudioPose`]) and the renderer multiplies them
//! into the vertices.
//!
//! See `docs/FORMAT_SOURCES.md` ("GoldSrc MDL v10 and SPR") for the public
//! documentation the rendering semantics implemented here are derived from.
//! Nothing here comes from any game installation or engine source; see
//! `docs/CLEAN_ROOM.md`.

use ohl_formats::mdl10::{self, Bone, Limits, Mdl, Sequence};

use crate::error::{Result, WorldError};
use crate::texture::TextureImage;

/// The largest number of bones a [`StudioPose`] will produce, matching the
/// documented GoldSrc practical bone ceiling and the renderer's uniform
/// array size.
pub const MAX_BONES: usize = 128;

/// The largest number of vertices this module will emit for one model.
pub const MAX_STUDIO_VERTICES: usize = 1_000_000;

// ---------------------------------------------------------------------
// Documented texture and sequence flags
// ---------------------------------------------------------------------

/// Texture flag: shade the surface flat rather than smoothly.
pub const STUDIO_NF_FLATSHADE: u32 = 0x01;
/// Texture flag: ignore the stored texture coordinates and generate a
/// view-relative spherical environment ("chrome") mapping instead.
pub const STUDIO_NF_CHROME: u32 = 0x02;
/// Texture flag: draw at full brightness, ignoring scene lighting.
pub const STUDIO_NF_FULLBRIGHT: u32 = 0x04;
/// Texture flag: do not build mip levels for this texture.
pub const STUDIO_NF_NOMIPS: u32 = 0x08;
/// Texture flag: the texture carries an alpha channel.
pub const STUDIO_NF_ALPHA: u32 = 0x10;
/// Texture flag: composite the surface additively.
pub const STUDIO_NF_ADDITIVE: u32 = 0x20;
/// Texture flag: treat palette index 255 as fully transparent (1-bit
/// alpha test).
pub const STUDIO_NF_MASKED: u32 = 0x40;

/// Sequence flag: the sequence wraps back to its first frame instead of
/// holding its last one.
pub const STUDIO_LOOPING: i32 = 0x0001;

/// The palette index GoldSrc reserves as the transparency key for
/// [`STUDIO_NF_MASKED`] textures.
const MASK_INDEX: u8 = 255;

// ---------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------

/// One vertex of a studio mesh.
///
/// The layout is fixed and mirrored by `ohl-render`'s studio vertex buffer
/// layout: three floats of model-space position, three of model-space
/// normal, two of texture coordinates, and one `u32` bone index.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct StudioVertex {
    /// Bind-pose position in the owning bone's local space.
    pub position: [f32; 3],
    /// Bind-pose normal in the owning bone's local space.
    pub normal: [f32; 3],
    /// Texture coordinates, already divided by the texture size.
    pub uv: [f32; 2],
    /// Index into [`StudioModel::bones`]; GoldSrc binds exactly one bone
    /// per vertex.
    pub bone: u32,
}

/// The number of bytes one [`StudioVertex`] occupies in a vertex buffer.
pub const STUDIO_VERTEX_BYTES: usize = 9 * 4;

impl StudioVertex {
    /// Appends this vertex to `out` in the little-endian layout the
    /// renderer uploads.
    ///
    /// Serialising by hand keeps the crate free of a `bytemuck`-style
    /// `unsafe impl Pod`, which the workspace's `forbid(unsafe_code)` would
    /// reject anyway.
    pub fn write_le(&self, out: &mut Vec<u8>) {
        for value in self
            .position
            .iter()
            .chain(self.normal.iter())
            .chain(self.uv.iter())
        {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.bone.to_le_bytes());
    }
}

/// Serialises `vertices` into the renderer's little-endian vertex format.
#[must_use]
pub fn studio_vertex_bytes(vertices: &[StudioVertex]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vertices.len() * STUDIO_VERTEX_BYTES);
    for vertex in vertices {
        vertex.write_le(&mut out);
    }
    out
}

/// One drawable mesh: a contiguous index range sharing one skin slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudioMesh {
    /// Index into [`StudioModel::body_parts`].
    pub body_part: usize,
    /// Index into the body part's [`StudioBodyPart::models`].
    pub model: usize,
    /// The mesh's replaceable-texture slot (`skinref`), to be resolved
    /// through [`StudioModel::resolve_skin`].
    pub skin_slot: usize,
    /// First entry in [`StudioModel::indices`].
    pub first_index: u32,
    /// Number of indices, always a multiple of three.
    pub index_count: u32,
}

/// One body part: a set of mutually exclusive sub-models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioBodyPart {
    /// One entry per sub-model, each holding the mesh slots that draw it.
    pub models: Vec<StudioSubModel>,
}

/// One sub-model of a body part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StudioSubModel {
    /// Indices into [`StudioModel::meshes`].
    pub meshes: Vec<usize>,
}

// ---------------------------------------------------------------------
// Skeleton, textures, hitboxes, attachments, sequences
// ---------------------------------------------------------------------

/// One skeleton bone, reduced to what posing needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioBone {
    /// The parent bone, or `None` for a root bone.
    pub parent: Option<usize>,
    /// The bind-pose channel values (`STUDIO_X/Y/Z/XR/YR/ZR`).
    pub value: [f32; 6],
    /// The per-channel compressed-animation scales.
    pub scale: [f32; 6],
}

/// One decoded model texture.
pub struct StudioTexture {
    /// The decoded RGBA8 image.
    pub image: TextureImage,
    /// The documented `STUDIO_NF_*` flag bits.
    pub flags: u32,
}

impl StudioTexture {
    /// Whether this texture uses the view-relative spherical environment
    /// mapping.
    #[must_use]
    pub fn is_chrome(&self) -> bool {
        self.flags & STUDIO_NF_CHROME != 0
    }

    /// Whether this texture composites additively.
    #[must_use]
    pub fn is_additive(&self) -> bool {
        self.flags & STUDIO_NF_ADDITIVE != 0
    }

    /// Whether palette index 255 is the transparency key for this texture.
    #[must_use]
    pub fn is_masked(&self) -> bool {
        self.flags & STUDIO_NF_MASKED != 0
    }

    /// Whether this texture ignores scene lighting.
    #[must_use]
    pub fn is_fullbright(&self) -> bool {
        self.flags & STUDIO_NF_FULLBRIGHT != 0
    }
}

/// One hitbox, in its bone's local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioHitbox {
    /// Index into [`StudioModel::bones`].
    pub bone: usize,
    /// The documented hit group this box belongs to.
    pub group: i32,
    /// Local-space minimum corner.
    pub min: [f32; 3],
    /// Local-space maximum corner.
    pub max: [f32; 3],
}

/// One attachment point, in its bone's local space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioAttachment {
    /// Index into [`StudioModel::bones`].
    pub bone: usize,
    /// Local-space origin.
    pub origin: [f32; 3],
}

/// One animation sequence's playback description.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioSequence {
    /// Playback rate in frames per second.
    pub fps: f32,
    /// The documented `STUDIO_*` sequence flag bits.
    pub flags: i32,
    /// Number of frames in the sequence.
    pub frame_count: u32,
    /// The sequence-group file this sequence's animation data lives in;
    /// only group `0` (embedded in the main file) is sampled here.
    pub group: i32,
}

impl StudioSequence {
    /// Whether the sequence wraps back to its first frame.
    #[must_use]
    pub fn is_looping(&self) -> bool {
        self.flags & STUDIO_LOOPING != 0
    }

    /// The sequence's duration in seconds, or `0.0` when it has no frames
    /// or a non-positive rate.
    #[must_use]
    pub fn duration(&self) -> f32 {
        if !self.fps.is_finite() || self.fps <= 0.0 || self.frame_count == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let frames = self.frame_count as f32;
        frames / self.fps
    }
}

// ---------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------

/// An owned, GPU-ready studio model.
///
/// The file's own bytes are retained because sequence animation offsets are
/// relative to them: [`StudioPose::sample`] hands the retained buffer back
/// to [`ohl_formats::mdl10::Mdl::sample_bone_animation`] rather than
/// re-reading the file.
pub struct StudioModel {
    /// Every vertex of every mesh, in mesh order.
    pub vertices: Vec<StudioVertex>,
    /// The full index buffer; [`StudioMesh`] ranges span it.
    pub indices: Vec<u32>,
    /// Every mesh, grouped by body part and sub-model.
    pub meshes: Vec<StudioMesh>,
    /// The body parts and their mutually exclusive sub-models.
    pub body_parts: Vec<StudioBodyPart>,
    /// One decoded image per texture chunk.
    pub textures: Vec<StudioTexture>,
    /// The skeleton.
    pub bones: Vec<StudioBone>,
    /// The hitboxes, in bone-local space.
    pub hitboxes: Vec<StudioHitbox>,
    /// The attachment points, in bone-local space.
    pub attachments: Vec<StudioAttachment>,
    /// The sequences, in file order.
    pub sequences: Vec<StudioSequence>,
    /// Each sequence's authored label, in the same order as
    /// [`Self::sequences`], lower-cased and with its trailing NUL padding
    /// removed.
    ///
    /// The labels are model-authored data, not a project table: they exist
    /// so a caller can resolve an animation *intent* it names itself
    /// against whatever the loaded model actually publishes. Media-derived,
    /// so they are handed back as data and never logged.
    pub sequence_names: Vec<String>,
    /// The model-space bounding box (`bbmin`/`bbmax` from the header).
    pub bounds_min: [f32; 3],
    /// The model-space bounding box maximum.
    pub bounds_max: [f32; 3],
    /// `skin_families[family][slot]` is an index into [`Self::textures`].
    skin_families: Vec<Vec<usize>>,
    /// The raw sequence records, needed to re-sample animation channels.
    raw_sequences: Vec<Sequence>,
    /// The raw bone records, needed to re-sample animation channels.
    raw_bones: Vec<Bone>,
    /// The file's own bytes, retained for animation sampling.
    data: Vec<u8>,
    limits: Limits,
}

/// Decodes one fixed-length, NUL-padded MDL short-name field (a bone or
/// sequence label) into a lower-cased owned string.
///
/// Bytes that are not valid UTF-8 are replaced rather than rejected: a
/// label is a lookup key, never a diagnostic, so an odd one must degrade
/// into a key that simply matches nothing.
fn short_name(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end])
        .trim()
        .to_ascii_lowercase()
}

impl StudioModel {
    /// The index of the sequence whose authored label is `name`, compared
    /// case-insensitively, or `None` when the model publishes no such
    /// sequence.
    ///
    /// This is the whole of the engine's activity-to-sequence resolution:
    /// the *name* comes from the caller's own animation vocabulary and the
    /// *match* is against model data, so no sequence name is baked into
    /// this project.
    #[must_use]
    pub fn sequence_by_name(&self, name: &str) -> Option<usize> {
        let name = name.trim().to_ascii_lowercase();
        self.sequence_names.iter().position(|label| *label == name)
    }

    /// Builds an owned model from a parsed [`Mdl`].
    ///
    /// `data` must be the same buffer `mdl` borrows; it is retained so
    /// animation channels can be re-sampled later.
    #[allow(clippy::too_many_lines)]
    pub fn build(mdl: &Mdl<'_>, data: &[u8], limits: &Limits) -> Result<Self> {
        let raw_bones = mdl.bones(limits)?;
        if raw_bones.len() > MAX_BONES {
            return Err(WorldError::LimitExceeded);
        }
        Mdl::validate_bone_hierarchy(raw_bones)?;
        let bones: Vec<StudioBone> = raw_bones
            .iter()
            .map(|bone| StudioBone {
                parent: usize::try_from(bone.parent.get()).ok(),
                value: [
                    bone.value[0].get(),
                    bone.value[1].get(),
                    bone.value[2].get(),
                    bone.value[3].get(),
                    bone.value[4].get(),
                    bone.value[5].get(),
                ],
                scale: [
                    bone.scale[0].get(),
                    bone.scale[1].get(),
                    bone.scale[2].get(),
                    bone.scale[3].get(),
                    bone.scale[4].get(),
                    bone.scale[5].get(),
                ],
            })
            .collect();

        let raw_textures = mdl.textures(limits)?;
        let mut textures = Vec::with_capacity(raw_textures.len());
        for texture in raw_textures {
            let flags = texture.flags.get();
            let image = decode_texture(mdl, texture, limits, flags)
                .unwrap_or_else(|_| TextureImage::placeholder());
            textures.push(StudioTexture { image, flags });
        }
        if textures.is_empty() {
            textures.push(StudioTexture {
                image: TextureImage::placeholder(),
                flags: 0,
            });
        }

        let header = mdl.header();
        let skin_ref_count = header.num_skin_ref.get() as usize;
        let family_count = header.num_skin_families.get() as usize;
        let mut skin_families = Vec::with_capacity(family_count);
        if let Ok(table) = mdl.skin_families(limits) {
            for family in 0..family_count {
                let mut row = Vec::with_capacity(skin_ref_count);
                for slot in 0..skin_ref_count {
                    let resolved = table
                        .get(family, slot)
                        .ok()
                        .and_then(|index| usize::try_from(index).ok())
                        .filter(|index| *index < textures.len())
                        .unwrap_or(0);
                    row.push(resolved);
                }
                skin_families.push(row);
            }
        }

        let mut vertices: Vec<StudioVertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut meshes: Vec<StudioMesh> = Vec::new();
        let mut body_parts: Vec<StudioBodyPart> = Vec::new();

        for (body_part_index, body_part) in mdl.body_parts(limits)?.iter().enumerate() {
            let mut sub_models = Vec::new();
            for (model_index, model) in mdl.models(body_part, limits)?.iter().enumerate() {
                let source = MeshSource {
                    positions: mdl.vertices(model, limits)?,
                    normals: mdl.normals(model, limits)?,
                    vertex_bones: mdl.vertex_bones(model, limits)?,
                };
                Mdl::validate_bone_indices(source.vertex_bones, bones.len())?;
                let mut sub_model = StudioSubModel::default();

                for mesh in mdl.meshes(model, limits)? {
                    let skin_slot = usize::try_from(mesh.skin_ref.get()).unwrap_or(0);
                    let texture = resolve_slot(&skin_families, &textures, skin_slot);
                    let size = (
                        f64::from(textures[texture].image.width()),
                        f64::from(textures[texture].image.height()),
                    );
                    let triangles = mdl.decode_mesh_commands(mesh, limits)?;
                    let first_index =
                        u32::try_from(indices.len()).map_err(|_| WorldError::LimitExceeded)?;
                    emit_mesh(&triangles, &source, size, &mut vertices, &mut indices)?;
                    let index_count = u32::try_from(indices.len())
                        .map_err(|_| WorldError::LimitExceeded)?
                        - first_index;
                    if index_count == 0 {
                        continue;
                    }
                    sub_model.meshes.push(meshes.len());
                    meshes.push(StudioMesh {
                        body_part: body_part_index,
                        model: model_index,
                        skin_slot,
                        first_index,
                        index_count,
                    });
                }
                sub_models.push(sub_model);
            }
            body_parts.push(StudioBodyPart { models: sub_models });
        }

        let hitboxes = mdl
            .hitboxes(limits)
            .unwrap_or(&[])
            .iter()
            .filter_map(|hitbox| {
                let bone = usize::try_from(hitbox.bone.get()).ok()?;
                (bone < bones.len()).then_some(StudioHitbox {
                    bone,
                    group: hitbox.group.get(),
                    min: [
                        hitbox.bbmin[0].get(),
                        hitbox.bbmin[1].get(),
                        hitbox.bbmin[2].get(),
                    ],
                    max: [
                        hitbox.bbmax[0].get(),
                        hitbox.bbmax[1].get(),
                        hitbox.bbmax[2].get(),
                    ],
                })
            })
            .collect();
        let attachments = mdl
            .attachments(limits)
            .unwrap_or(&[])
            .iter()
            .filter_map(|attachment| {
                let bone = usize::try_from(attachment.bone.get()).ok()?;
                (bone < bones.len()).then_some(StudioAttachment {
                    bone,
                    origin: [
                        attachment.org[0].get(),
                        attachment.org[1].get(),
                        attachment.org[2].get(),
                    ],
                })
            })
            .collect();

        let raw_sequences = mdl.sequences(limits)?;
        let sequence_names = raw_sequences
            .iter()
            .map(|sequence| short_name(&sequence.label))
            .collect();
        let sequences = raw_sequences
            .iter()
            .map(|sequence| StudioSequence {
                fps: sequence.fps.get(),
                flags: sequence.flags.get(),
                frame_count: sequence.num_frames.get(),
                group: sequence.seq_group.get(),
            })
            .collect();

        Ok(Self {
            vertices,
            indices,
            meshes,
            body_parts,
            textures,
            bones,
            hitboxes,
            attachments,
            sequences,
            sequence_names,
            bounds_min: [
                header.bbmin[0].get(),
                header.bbmin[1].get(),
                header.bbmin[2].get(),
            ],
            bounds_max: [
                header.bbmax[0].get(),
                header.bbmax[1].get(),
                header.bbmax[2].get(),
            ],
            skin_families,
            raw_sequences: raw_sequences.to_vec(),
            raw_bones: raw_bones.to_vec(),
            data: data.to_vec(),
            limits: *limits,
        })
    }

    /// Parses `data` and builds a model from it in one step.
    pub fn parse(data: &[u8], limits: &Limits) -> Result<Self> {
        let mdl = Mdl::parse(data, limits)?;
        Self::build(&mdl, data, limits)
    }

    /// Resolves a mesh's `skin_slot` through skin `family`, falling back to
    /// family 0 and then to the slot itself.
    #[must_use]
    pub fn resolve_skin(&self, family: usize, slot: usize) -> usize {
        self.skin_families
            .get(family)
            .or_else(|| self.skin_families.first())
            .and_then(|row| row.get(slot).copied())
            .filter(|index| *index < self.textures.len())
            .unwrap_or_else(|| slot.min(self.textures.len().saturating_sub(1)))
    }

    /// The number of skin families the model declares.
    #[must_use]
    pub fn skin_family_count(&self) -> usize {
        self.skin_families.len()
    }

    /// The meshes drawn for one body configuration.
    ///
    /// `body` selects a sub-model per body part; a body part with no entry
    /// (or an out-of-range one) draws its first sub-model, which is what a
    /// caller that does not care about body groups wants.
    #[must_use]
    pub fn visible_meshes(&self, body: &[u32]) -> Vec<usize> {
        let mut out = Vec::new();
        for (index, body_part) in self.body_parts.iter().enumerate() {
            let choice = body
                .get(index)
                .and_then(|value| usize::try_from(*value).ok())
                .filter(|value| *value < body_part.models.len())
                .unwrap_or(0);
            if let Some(sub_model) = body_part.models.get(choice) {
                out.extend_from_slice(&sub_model.meshes);
            }
        }
        out
    }
}

/// One sub-model's vertex tables, borrowed while its meshes are built.
struct MeshSource<'a> {
    positions: &'a [mdl10::Vec3],
    normals: &'a [mdl10::Vec3],
    vertex_bones: &'a [u8],
}

/// Expands one mesh's decoded triangles into indexed vertices.
///
/// The strip/fan expansion repeats shared corners, so identical
/// `(vertex, normal, s, t)` tuples are collapsed back into one vertex; a
/// corner reused with different texture coordinates stays a separate
/// vertex, which is what the format's per-trivert coordinates require.
fn emit_mesh(
    triangles: &[mdl10::Triangle],
    source: &MeshSource<'_>,
    texture_size: (f64, f64),
    vertices: &mut Vec<StudioVertex>,
    indices: &mut Vec<u32>,
) -> Result<()> {
    let mut lookup: Vec<((u16, u16, i16, i16), u32)> = Vec::new();
    for triangle in triangles {
        for trivert in &triangle.verts {
            let key = (trivert.vert_index, trivert.norm_index, trivert.s, trivert.t);
            let existing = lookup
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, index)| *index);
            let index = if let Some(index) = existing {
                index
            } else {
                let position = source
                    .positions
                    .get(usize::from(trivert.vert_index))
                    .ok_or(WorldError::IndexOutOfRange)?;
                let normal = source
                    .normals
                    .get(usize::from(trivert.norm_index))
                    .ok_or(WorldError::IndexOutOfRange)?;
                let bone = source
                    .vertex_bones
                    .get(usize::from(trivert.vert_index))
                    .copied()
                    .ok_or(WorldError::IndexOutOfRange)?;
                if vertices.len() >= MAX_STUDIO_VERTICES {
                    return Err(WorldError::LimitExceeded);
                }
                // The documented trivert `s`/`t` are absolute texel
                // coordinates, so the texture's own size normalises them.
                #[allow(clippy::cast_possible_truncation)]
                let uv = [
                    (f64::from(trivert.s) / texture_size.0.max(1.0)) as f32,
                    (f64::from(trivert.t) / texture_size.1.max(1.0)) as f32,
                ];
                let index = u32::try_from(vertices.len()).map_err(|_| WorldError::LimitExceeded)?;
                vertices.push(StudioVertex {
                    position: [
                        position.v[0].get(),
                        position.v[1].get(),
                        position.v[2].get(),
                    ],
                    normal: [normal.v[0].get(), normal.v[1].get(), normal.v[2].get()],
                    uv,
                    bone: u32::from(bone),
                });
                lookup.push((key, index));
                index
            };
            indices.push(index);
        }
    }
    Ok(())
}

fn resolve_slot(skin_families: &[Vec<usize>], textures: &[StudioTexture], slot: usize) -> usize {
    skin_families
        .first()
        .and_then(|row| row.get(slot).copied())
        .filter(|index| *index < textures.len())
        .unwrap_or_else(|| slot.min(textures.len().saturating_sub(1)))
}

/// Expands one model texture's 8-bit indexed pixels into RGBA8.
///
/// [`STUDIO_NF_MASKED`] selects the documented 1-bit transparency key
/// (palette index 255); every other texture is fully opaque.
fn decode_texture(
    mdl: &Mdl<'_>,
    texture: &mdl10::Texture,
    limits: &Limits,
    flags: u32,
) -> Result<TextureImage> {
    let decoded = mdl.decode_texture(texture, limits)?;
    let width = decoded.width;
    let height = decoded.height;
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(WorldError::LimitExceeded)?;
    if decoded.indices.len() < pixels {
        return Err(WorldError::IndexOutOfRange);
    }
    let masked = flags & STUDIO_NF_MASKED != 0;
    let mut rgba = Vec::with_capacity(pixels.checked_mul(4).ok_or(WorldError::LimitExceeded)?);
    for &index in &decoded.indices[..pixels] {
        let color = decoded.palette.get(index);
        let alpha = if masked && index == MASK_INDEX {
            0
        } else {
            255
        };
        rgba.extend_from_slice(&[color.r, color.g, color.b, alpha]);
    }
    TextureImage::new(width, height, rgba)
}

// ---------------------------------------------------------------------
// Posing
// ---------------------------------------------------------------------

/// A column-major 4x4 matrix, matching what a WGSL `mat4x4<f32>` uniform
/// expects (`m[column * 4 + row]`).
pub type BoneMatrix = [f32; 16];

/// A sampled skeleton pose: one world-space matrix per bone.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioPose {
    /// `matrices[bone]` transforms that bone's bind-pose vertices into
    /// model space.
    pub matrices: Vec<BoneMatrix>,
}

impl StudioPose {
    /// The bind pose: every bone at its stored `value` channels.
    #[must_use]
    pub fn bind(model: &StudioModel) -> Self {
        let locals: Vec<([f32; 3], [f32; 4])> = model
            .bones
            .iter()
            .map(|bone| {
                let position = [bone.value[0], bone.value[1], bone.value[2]];
                let rotation =
                    mdl10::euler_to_quaternion([bone.value[3], bone.value[4], bone.value[5]]);
                (position, rotation)
            })
            .collect();
        Self {
            matrices: chain(model, &locals),
        }
    }

    /// Samples `sequence` at `time` seconds since the sequence started.
    ///
    /// The frame index advances at the sequence's own `fps`; the fractional
    /// part interpolates linearly between two adjacent frames (positions
    /// lerped, rotations normalised-lerped along the shorter arc), so
    /// playback is continuous across frame boundaries. A looping sequence
    /// wraps back to frame 0; a non-looping one holds its last frame. Bone
    /// controllers and multi-blend sequences stay at their defaults: only
    /// blend 0 is sampled, which is what
    /// [`ohl_formats::mdl10::Mdl::sample_bone_animation`] reads.
    ///
    /// Sequences stored in an external sequence-group file (`group != 0`)
    /// are not resolved here, so they fall back to the bind pose.
    pub fn sample(model: &StudioModel, sequence: usize, time: f32) -> Result<Self> {
        let Some(description) = model.sequences.get(sequence) else {
            return Ok(Self::bind(model));
        };
        let Some(raw) = model.raw_sequences.get(sequence) else {
            return Ok(Self::bind(model));
        };
        if description.group != 0 || description.frame_count == 0 || model.bones.is_empty() {
            return Ok(Self::bind(model));
        }

        let (frame, next_frame, blend) = frame_pair(description, time);
        let first = mdl10::Mdl::sample_bone_animation(
            &model.data,
            raw,
            &model.raw_bones,
            frame,
            &model.limits,
        )?;
        let locals = if blend <= f32::EPSILON || next_frame == frame {
            first
                .iter()
                .map(|pose| (pose.position, pose.rotation))
                .collect::<Vec<_>>()
        } else {
            let second = mdl10::Mdl::sample_bone_animation(
                &model.data,
                raw,
                &model.raw_bones,
                next_frame,
                &model.limits,
            )?;
            first
                .iter()
                .zip(second.iter())
                .map(|(a, b)| {
                    (
                        lerp3(a.position, b.position, blend),
                        nlerp(a.rotation, b.rotation, blend),
                    )
                })
                .collect()
        };
        Ok(Self {
            matrices: chain(model, &locals),
        })
    }

    /// The world-space centre and half-extents of `hitbox` under this pose.
    ///
    /// The returned box is the axis-aligned bound of the posed hitbox's
    /// eight corners, which is what a broad-phase query wants.
    #[must_use]
    pub fn hitbox_bounds(&self, hitbox: &StudioHitbox) -> Option<([f32; 3], [f32; 3])> {
        let matrix = self.matrices.get(hitbox.bone)?;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in 0..8u8 {
            let point = [
                if corner & 1 == 0 {
                    hitbox.min[0]
                } else {
                    hitbox.max[0]
                },
                if corner & 2 == 0 {
                    hitbox.min[1]
                } else {
                    hitbox.max[1]
                },
                if corner & 4 == 0 {
                    hitbox.min[2]
                } else {
                    hitbox.max[2]
                },
            ];
            let world = transform_point(matrix, point);
            for axis in 0..3 {
                min[axis] = min[axis].min(world[axis]);
                max[axis] = max[axis].max(world[axis]);
            }
        }
        Some((min, max))
    }

    /// The world-space origin of `attachment` under this pose.
    #[must_use]
    pub fn attachment_origin(&self, attachment: &StudioAttachment) -> Option<[f32; 3]> {
        let matrix = self.matrices.get(attachment.bone)?;
        Some(transform_point(matrix, attachment.origin))
    }
}

/// Picks the two frames `time` falls between and the blend factor.
fn frame_pair(sequence: &StudioSequence, time: f32) -> (u32, u32, f32) {
    let last = sequence.frame_count.saturating_sub(1);
    if !time.is_finite() || time <= 0.0 || !sequence.fps.is_finite() || sequence.fps <= 0.0 {
        return (0, 0, 0.0);
    }
    let position = time * sequence.fps;
    if !position.is_finite() {
        return (0, 0, 0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let span = if sequence.is_looping() {
        sequence.frame_count as f32
    } else {
        (last as f32).max(1.0)
    };
    let wrapped = if sequence.is_looping() {
        position.rem_euclid(span)
    } else {
        position.min(span)
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frame = wrapped.floor().max(0.0) as u32;
    let blend = (wrapped - wrapped.floor()).clamp(0.0, 1.0);
    let frame = frame.min(last);
    let next = if sequence.is_looping() {
        if frame >= last { 0 } else { frame + 1 }
    } else {
        frame.saturating_add(1).min(last)
    };
    (frame, next, blend)
}

/// Composes every bone's local transform with its parent chain.
///
/// The hierarchy has already been validated to be acyclic and
/// parent-before-child by
/// [`ohl_formats::mdl10::Mdl::validate_bone_hierarchy`], so one forward pass
/// is enough; a bone whose parent has not been computed yet falls back to
/// its own local transform rather than reading uninitialised state.
fn chain(model: &StudioModel, locals: &[([f32; 3], [f32; 4])]) -> Vec<BoneMatrix> {
    let mut matrices: Vec<BoneMatrix> = Vec::with_capacity(model.bones.len());
    for (index, bone) in model.bones.iter().enumerate() {
        let (position, rotation) = locals.get(index).copied().unwrap_or(([0.0; 3], [0.0; 4]));
        let local = compose(position, rotation);
        let matrix = match bone.parent {
            Some(parent) if parent < matrices.len() => multiply(&matrices[parent], &local),
            _ => local,
        };
        matrices.push(matrix);
    }
    matrices
}

/// Builds a column-major matrix from a translation and a `[x, y, z, w]`
/// quaternion.
fn compose(position: [f32; 3], rotation: [f32; 4]) -> BoneMatrix {
    let [qx, qy, qz, qw] = normalize_quaternion(rotation);
    let (xx, yy, zz) = (qx * qx, qy * qy, qz * qz);
    let (xy, xz, yz) = (qx * qy, qx * qz, qy * qz);
    let (wx, wy, wz) = (qw * qx, qw * qy, qw * qz);
    let mut matrix = [0.0f32; 16];
    matrix[0] = 1.0 - 2.0 * (yy + zz);
    matrix[1] = 2.0 * (xy + wz);
    matrix[2] = 2.0 * (xz - wy);
    matrix[4] = 2.0 * (xy - wz);
    matrix[5] = 1.0 - 2.0 * (xx + zz);
    matrix[6] = 2.0 * (yz + wx);
    matrix[8] = 2.0 * (xz + wy);
    matrix[9] = 2.0 * (yz - wx);
    matrix[10] = 1.0 - 2.0 * (xx + yy);
    matrix[12] = position[0];
    matrix[13] = position[1];
    matrix[14] = position[2];
    matrix[15] = 1.0;
    matrix
}

/// Column-major matrix product `a * b`, applying `b` first.
fn multiply(a: &BoneMatrix, b: &BoneMatrix) -> BoneMatrix {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

fn transform_point(matrix: &BoneMatrix, point: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (row, value) in out.iter_mut().enumerate() {
        *value = matrix[row] * point[0]
            + matrix[4 + row] * point[1]
            + matrix[8 + row] * point[2]
            + matrix[12 + row];
    }
    out
}

fn normalize_quaternion(q: [f32; 4]) -> [f32; 4] {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if length.is_finite() && length > 1e-6 {
        [q[0] / length, q[1] / length, q[2] / length, q[3] / length]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Normalised linear quaternion interpolation along the shorter arc.
fn nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let mut out = [0.0f32; 4];
    for (index, value) in out.iter_mut().enumerate() {
        *value = a[index] + (b[index] * sign - a[index]) * t;
    }
    normalize_quaternion(out)
}

#[cfg(test)]
mod tests {
    use super::{
        BoneMatrix, MAX_BONES, STUDIO_LOOPING, STUDIO_VERTEX_BYTES, StudioModel, StudioPose,
        StudioSequence, StudioVertex, compose, frame_pair, multiply, normalize_quaternion,
        studio_vertex_bytes,
    };
    use ohl_formats::mdl10::Limits;
    use ohl_formats::test_support::build_minimal_mdl10;

    fn model() -> StudioModel {
        let (bytes, _) = build_minimal_mdl10();
        StudioModel::parse(&bytes, &Limits::default()).expect("synthetic model builds")
    }

    #[test]
    fn triangulates_the_synthetic_strip_into_two_triangles() {
        let model = model();
        assert_eq!(model.meshes.len(), 1);
        assert_eq!(model.meshes[0].index_count, 6);
        assert_eq!(model.indices.len(), 6);
        // The strip's four triverts all differ, so nothing deduplicates.
        assert_eq!(model.vertices.len(), 4);
        assert_eq!(model.body_parts.len(), 1);
        assert_eq!(model.body_parts[0].models.len(), 1);
        assert_eq!(model.visible_meshes(&[]), vec![0]);
    }

    #[test]
    fn texture_coordinates_are_normalised_by_the_texture_size() {
        let model = model();
        for vertex in &model.vertices {
            assert!(
                (0.0..=1.0).contains(&vertex.uv[0]) && (0.0..=1.0).contains(&vertex.uv[1]),
                "the synthetic mesh's texels lie inside its 16x16 texture"
            );
        }
        // s = 16 over a 16-wide texture is exactly 1.0.
        assert!(model.vertices.iter().any(|v| (v.uv[0] - 1.0).abs() < 1e-6));
        assert!(model.vertices.iter().all(|v| v.bone == 0));
    }

    #[test]
    fn bone_matrices_follow_the_parent_chain() {
        let model = model();
        assert_eq!(model.bones.len(), 2);
        assert_eq!(model.bones[0].parent, None);
        assert_eq!(model.bones[1].parent, Some(0));

        // Frame 1 puts bone 0's X channel at 20 (value 0 + delta 20 * scale
        // 1); bone 1 inherits it through the chain.
        let pose = StudioPose::sample(&model, 0, 1.0 / 10.0).expect("frame 1 samples");
        assert!((pose.matrices[0][12] - 20.0).abs() < 1e-4);
        assert!((pose.matrices[1][12] - 20.0).abs() < 1e-4);
    }

    #[test]
    fn interpolation_is_continuous_across_a_frame_boundary() {
        let model = model();
        let step = 1.0f32 / 10.0;
        let mut previous = StudioPose::sample(&model, 0, 0.0).expect("t=0 samples");
        for tick in 1..=20 {
            #[allow(clippy::cast_precision_loss)]
            let time = tick as f32 * step / 4.0;
            let pose = StudioPose::sample(&model, 0, time).expect("samples");
            for (a, b) in previous.matrices.iter().zip(pose.matrices.iter()) {
                for (left, right) in a.iter().zip(b.iter()) {
                    assert!(
                        (left - right).abs() < 6.0,
                        "a quarter-frame step must not jump the pose"
                    );
                }
            }
            previous = pose;
        }
    }

    #[test]
    fn midpoint_interpolation_sits_between_the_two_frames() {
        let model = model();
        let half = StudioPose::sample(&model, 0, 0.05).expect("half a frame samples");
        // Frame 0 is x = 10, frame 1 is x = 20.
        assert!((half.matrices[0][12] - 15.0).abs() < 1e-3);
    }

    #[test]
    fn a_non_looping_sequence_holds_its_last_frame() {
        let model = model();
        let late = StudioPose::sample(&model, 0, 100.0).expect("late samples");
        assert!((late.matrices[0][12] - 20.0).abs() < 1e-4);
    }

    #[test]
    fn frame_pairs_wrap_only_when_looping() {
        let looping = StudioSequence {
            fps: 10.0,
            flags: STUDIO_LOOPING,
            frame_count: 4,
            group: 0,
        };
        // 0.45 s at 10 fps is frame 4.5, which wraps to 0.5 over four
        // frames.
        let (frame, next, blend) = frame_pair(&looping, 0.45);
        assert_eq!((frame, next), (0, 1));
        assert!((blend - 0.5).abs() < 1e-5);
        // The last frame blends back into the first.
        let (frame, next, _) = frame_pair(&looping, 0.35);
        assert_eq!((frame, next), (3, 0));
        let held = StudioSequence {
            flags: 0,
            ..looping
        };
        assert_eq!(frame_pair(&held, 10.0), (3, 3, 0.0));
    }

    #[test]
    fn hitboxes_and_attachments_survive_an_empty_model() {
        let model = model();
        assert!(model.hitboxes.is_empty());
        assert!(model.attachments.is_empty());
        assert!(model.bones.len() <= MAX_BONES);
    }

    #[test]
    fn identity_composition_round_trips() {
        let identity: BoneMatrix = compose([0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        let product = multiply(&identity, &identity);
        for (left, right) in product.iter().zip(identity.iter()) {
            assert!((left - right).abs() < 1e-6);
        }
        // A degenerate quaternion falls back to the identity rotation.
        let fallback = normalize_quaternion([0.0; 4]);
        for (value, expected) in fallback.iter().zip([0.0, 0.0, 0.0, 1.0f32].iter()) {
            assert!((value - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn vertices_serialise_to_nine_words() {
        let bytes = studio_vertex_bytes(&[StudioVertex {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.5, 0.25],
            bone: 7,
        }]);
        assert_eq!(bytes.len(), STUDIO_VERTEX_BYTES);
        assert_eq!(&bytes[32..], &7u32.to_le_bytes());
    }

    #[test]
    fn skin_families_resolve_to_a_real_texture() {
        let model = model();
        assert_eq!(model.skin_family_count(), 1);
        assert_eq!(model.resolve_skin(0, 0), 0);
        // Out-of-range families and slots clamp rather than panic.
        assert!(model.resolve_skin(99, 99) < model.textures.len());
    }
}
