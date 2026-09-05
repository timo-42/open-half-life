//! GoldSrc studiomodel ("MDL v10") decoding.
//!
//! See `docs/FORMAT_SOURCES.md` ("GoldSrc MDL v10 and SPR") for the public
//! documentation this module was implemented from. As with [`crate::bsp30`]
//! and [`crate::wad3`], [`Mdl`] is a borrowing, zero-copy view: every count
//! and offset from the header (and every nested chunk index/offset) is
//! validated against the actual buffer before use, and no accessor panics on
//! malformed input.
//!
//! This module intentionally does not implement multi-file model loading
//! (external texture files, external "sequence group" `IDSQ` files): each
//! file is parsed independently by [`Mdl::parse`] or [`SequenceGroupFile::parse`],
//! and animation sampling ([`Mdl::sample_bone_animation`]) takes the
//! relevant animation-data buffer explicitly so callers control which file's
//! bytes are used.

use alloc::vec::Vec;
use core::mem::size_of;

use zerocopy::byteorder::little_endian::{F32, I16, I32, U16, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::error::{FormatError, Result};
use crate::palette::{Indexed8, PALETTE_LEN, Palette, Rgb8};
use crate::util::{checked_pixel_count, exact_of, prefix_of, slice_of, sub_slice};

/// The fixed 4-byte main-file signature ("IDST").
pub const MAGIC_IDST: [u8; 4] = *b"IDST";
/// The fixed 4-byte sequence-group-file signature ("IDSQ").
pub const MAGIC_IDSQ: [u8; 4] = *b"IDSQ";
/// MDL v10's fixed version field value.
pub const VERSION: i32 = 10;
/// The fixed 32-byte short-name field length (bones, sequence labels,
/// attachment names).
pub const NAME32_LEN: usize = 32;
/// The fixed 64-byte long-name field length (model/texture/bodypart names).
pub const NAME64_LEN: usize = 64;

// ---------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------

/// Bounds this crate enforces while decoding an MDL v10 (or `IDSQ`) file, so
/// a malformed or adversarial file cannot force unbounded allocation or
/// iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest number of bones a model may declare (GoldSrc tooling
    /// documents a 128-bone practical limit; kept generous here).
    pub max_bones: usize,
    /// The largest number of bone controllers.
    pub max_bone_controllers: usize,
    /// The largest number of hitboxes.
    pub max_hitboxes: usize,
    /// The largest number of sequences.
    pub max_sequences: usize,
    /// The largest number of sequence groups.
    pub max_sequence_groups: usize,
    /// The largest number of textures.
    pub max_textures: usize,
    /// The largest single texture's `width * height` pixel count.
    pub max_texture_pixels: u32,
    /// The largest number of skin families.
    pub max_skin_families: usize,
    /// The largest number of replaceable textures (skin refs) per family.
    pub max_skin_refs: usize,
    /// The largest number of body parts.
    pub max_body_parts: usize,
    /// The largest number of models per body part.
    pub max_models: usize,
    /// The largest number of meshes per model.
    pub max_meshes: usize,
    /// The largest number of vertices/normals per model.
    pub max_verts: usize,
    /// The largest number of triverts a single mesh's command stream may
    /// decode into (bounds the whole bounded-decode loop, not just one
    /// command run).
    pub max_triverts: usize,
    /// The largest number of attachments.
    pub max_attachments: usize,
    /// The largest number of animation events per sequence.
    pub max_events: usize,
    /// The largest `numtransitions` (the transition graph is
    /// `numtransitions * numtransitions` bytes).
    pub max_transitions: usize,
    /// The largest number of frames [`Mdl::sample_bone_animation`] will
    /// walk into a single compressed animation channel before giving up
    /// (guards against a maliciously long run chain).
    pub max_frame_walk: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for real GoldSrc models but
    /// far below what would let a malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bones: 512,
            max_bone_controllers: 64,
            max_hitboxes: 256,
            max_sequences: 4096,
            max_sequence_groups: 64,
            max_textures: 4096,
            max_texture_pixels: 16 * 1024 * 1024,
            max_skin_families: 256,
            max_skin_refs: 256,
            max_body_parts: 64,
            max_models: 64,
            max_meshes: 256,
            max_verts: 65_536,
            max_triverts: 262_144,
            max_attachments: 64,
            max_events: 4096,
            max_transitions: 256,
            max_frame_walk: 65_536,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

// ---------------------------------------------------------------------
// Raw, zero-copy, little-endian struct layouts
// ---------------------------------------------------------------------

/// The main/sequence-group file header (244 bytes). A sequence-group file
/// may be loaded with only its first 76 bytes meaningful (see
/// [`SequenceGroupFile`]); the documented layout notes that "a regular
/// header may also be used" for sequence-group files.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawHeader {
    pub id: [u8; 4],
    pub version: I32,
    pub name: [u8; NAME64_LEN],
    pub length: U32,
    pub eyeposition: [F32; 3],
    pub min: [F32; 3],
    pub max: [F32; 3],
    pub bbmin: [F32; 3],
    pub bbmax: [F32; 3],
    pub flags: I32,
    pub num_bones: U32,
    pub bone_index: U32,
    pub num_bone_controllers: U32,
    pub bone_controller_index: U32,
    pub num_hitboxes: U32,
    pub hitbox_index: U32,
    pub num_seq: U32,
    pub seq_index: U32,
    pub num_seq_groups: U32,
    pub seq_group_index: U32,
    pub num_textures: U32,
    pub texture_index: U32,
    pub texture_data_index: U32,
    pub num_skin_ref: U32,
    pub num_skin_families: U32,
    pub skin_index: U32,
    pub num_body_parts: U32,
    pub body_part_index: U32,
    pub num_attachments: U32,
    pub attachment_index: U32,
    pub sound_table: I32,
    pub sound_index: I32,
    pub sound_groups: I32,
    pub sound_group_index: I32,
    pub num_transitions: U32,
    pub transition_index: U32,
}

/// The reduced 76-byte header used to identify a sequence-group ("IDSQ")
/// file.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawSequenceHeader {
    pub id: [u8; 4],
    pub version: I32,
    pub name: [u8; NAME64_LEN],
    pub length: U32,
}

/// A skeleton bone (112 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Bone {
    pub name: [u8; NAME32_LEN],
    pub parent: I32,
    pub flags: I32,
    pub bone_controller: [I32; 6],
    pub value: [F32; 6],
    pub scale: [F32; 6],
}

/// A bone controller (24 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct BoneController {
    pub bone: I32,
    pub kind: I32,
    pub start: F32,
    pub end: F32,
    pub rest: I32,
    pub index: I32,
}

/// A hitbox (32 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Hitbox {
    pub bone: I32,
    pub group: I32,
    pub bbmin: [F32; 3],
    pub bbmax: [F32; 3],
}

/// A texture chunk (80 bytes; excludes the pixel data it points to).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Texture {
    pub name: [u8; NAME64_LEN],
    pub flags: U32,
    pub width: U32,
    pub height: U32,
    pub index: U32,
}

/// A sequence group descriptor (104 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct SequenceGroup {
    pub label: [u8; NAME32_LEN],
    pub name: [u8; NAME64_LEN],
    pub unused1: I32,
    pub unused2: I32,
}

/// A sequence description (176 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Sequence {
    pub label: [u8; NAME32_LEN],
    pub fps: F32,
    pub flags: I32,
    pub activity: I32,
    pub actweight: I32,
    pub num_events: U32,
    pub event_index: U32,
    pub num_frames: U32,
    pub num_pivots: I32,
    pub pivot_index: I32,
    pub motion_type: I32,
    pub motion_bone: I32,
    pub linear_movement: [F32; 3],
    pub automove_pos_index: I32,
    pub automove_angle_index: I32,
    pub bbmin: [F32; 3],
    pub bbmax: [F32; 3],
    pub num_blends: U32,
    pub anim_index: U32,
    pub blend_type: [I32; 2],
    pub blend_start: [F32; 2],
    pub blend_end: [F32; 2],
    pub blend_parent: I32,
    pub seq_group: I32,
    pub entry_node: I32,
    pub exit_node: I32,
    pub node_flags: I32,
    pub next_seq: I32,
}

/// An animation event (76 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct AnimEvent {
    pub frame: I32,
    pub event: I32,
    pub kind: I32,
    pub options: [u8; NAME64_LEN],
}

/// An attachment point (88 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Attachment {
    pub name: [u8; NAME32_LEN],
    pub kind: I32,
    pub bone: I32,
    pub org: [F32; 3],
    pub vectors: [[F32; 3]; 3],
}

/// A body part (76 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Bodypart {
    pub name: [u8; NAME64_LEN],
    pub num_models: U32,
    pub base: I32,
    pub model_index: U32,
}

/// A model within a body part (112 bytes).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Model {
    pub name: [u8; NAME64_LEN],
    pub kind: I32,
    pub bounding_radius: F32,
    pub num_mesh: U32,
    pub mesh_index: U32,
    pub num_verts: U32,
    pub vert_info_index: U32,
    pub vert_index: U32,
    pub num_norms: U32,
    pub norm_info_index: U32,
    pub norm_index: U32,
    pub num_groups: I32,
    pub group_index: I32,
}

/// A mesh (20 bytes): one triangle command stream plus the texture it uses.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Mesh {
    pub num_tris: I32,
    pub tri_index: U32,
    pub skin_ref: I32,
    pub num_norms: I32,
    pub norm_index: U32,
}

/// One vertex position or normal (`vec3_t`).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Vec3 {
    pub v: [F32; 3],
}

/// One trivert command-stream entry (8 bytes): indices plus absolute (not
/// normalized) texture coordinates.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawTrivert {
    pub vert_index: I16,
    pub norm_index: I16,
    pub s: I16,
    pub t: I16,
}

/// One decoded triangle vertex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trivert {
    /// Index into the model's vertex-position array.
    pub vert_index: u16,
    /// Index into the model's vertex-normal array.
    pub norm_index: u16,
    /// Absolute (non-normalized) horizontal texture coordinate.
    pub s: i16,
    /// Absolute (non-normalized) vertical texture coordinate.
    pub t: i16,
}

/// One decoded triangle (three [`Trivert`]s), expanded from a mesh's
/// strip/fan command stream by [`decode_mesh_commands`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Triangle {
    pub verts: [Trivert; 3],
}

/// One bone's six animation-channel offsets (12 bytes): one `u16` offset
/// per motion-type slot (`STUDIO_X/Y/Z/XR/YR/ZR`), each relative to the
/// start of this 12-byte record. A zero offset means "no animation data;
/// use the bone's bind-pose `value`".
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct AnimOffsets {
    pub offset: [U16; 6],
}

/// One compressed animation-value run header (2 bytes): `valid` raw values
/// follow immediately, then the run's last value is held for the remaining
/// `total - valid` frames it covers. See "Animation frame" in
/// `docs/FORMAT_SOURCES.md`: the reviewed documentation states this
/// run-length scheme's *purpose* (removing consecutive identical values)
/// but explicitly defers the exact decode algorithm to Valve's own source,
/// which this project does not consult; the run/hold algorithm implemented
/// by [`decode_anim_channel`] is this project's own design against the
/// documented `valid`/`total` field semantics.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct AnimValueHeader {
    pub valid: u8,
    pub total: u8,
}

/// A decoded per-bone pose sample from [`Mdl::sample_bone_animation`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BonePose {
    /// Local-space position (bind pose `value[0..3]` plus any decoded
    /// animation delta scaled by `scale[0..3]`).
    pub position: [f32; 3],
    /// Local-space rotation, as a quaternion `[x, y, z, w]` built from the
    /// bind-pose-plus-animation Euler angles via [`euler_to_quaternion`].
    pub rotation: [f32; 4],
}

// ---------------------------------------------------------------------
// Name helper
// ---------------------------------------------------------------------

/// Trims a fixed, NUL-padded name field at its first NUL byte.
#[must_use]
pub fn trim_name(name: &[u8]) -> &[u8] {
    let len = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    &name[..len]
}

// ---------------------------------------------------------------------
// Mdl: the main parsed view
// ---------------------------------------------------------------------

/// A validated, zero-copy view over one MDL v10 file (main model file or an
/// external texture file sharing the same header layout).
pub struct Mdl<'a> {
    data: &'a [u8],
    header: RawHeader,
}

fn count_and_index(count: U32, index: U32) -> (usize, usize) {
    (count.get() as usize, index.get() as usize)
}

fn table_bytes(
    data: &[u8],
    index: usize,
    count: usize,
    elem_size: usize,
    limit: usize,
) -> Result<&[u8]> {
    if count > limit {
        return Err(FormatError::LimitExceeded);
    }
    let len = count
        .checked_mul(elem_size)
        .ok_or(FormatError::OutOfBounds)?;
    sub_slice(data, index, len)
}

impl<'a> Mdl<'a> {
    /// Parses and validates an MDL v10 file's header, and validates that
    /// every table the header describes falls within `data`.
    ///
    /// This only validates counts/offsets; it does not decode any table's
    /// contents (that happens lazily, per accessor).
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self> {
        let (header, _): (&RawHeader, _) = prefix_of(data)?;
        if header.id != MAGIC_IDST {
            return Err(FormatError::BadSignature);
        }
        if header.version.get() != VERSION {
            return Err(FormatError::BadSignature);
        }
        let header = *header;

        // Validate every table now so later accessors can assume the range
        // is in-bounds.
        let (n, i) = count_and_index(header.num_bones, header.bone_index);
        table_bytes(data, i, n, size_of::<Bone>(), limits.max_bones)?;
        let (n, i) = count_and_index(header.num_bone_controllers, header.bone_controller_index);
        table_bytes(
            data,
            i,
            n,
            size_of::<BoneController>(),
            limits.max_bone_controllers,
        )?;
        let (n, i) = count_and_index(header.num_hitboxes, header.hitbox_index);
        table_bytes(data, i, n, size_of::<Hitbox>(), limits.max_hitboxes)?;
        let (n, i) = count_and_index(header.num_seq, header.seq_index);
        table_bytes(data, i, n, size_of::<Sequence>(), limits.max_sequences)?;
        let (n, i) = count_and_index(header.num_seq_groups, header.seq_group_index);
        table_bytes(
            data,
            i,
            n,
            size_of::<SequenceGroup>(),
            limits.max_sequence_groups,
        )?;
        let (n, i) = count_and_index(header.num_textures, header.texture_index);
        table_bytes(data, i, n, size_of::<Texture>(), limits.max_textures)?;
        let num_skin_ref = header.num_skin_ref.get() as usize;
        let num_skin_families = header.num_skin_families.get() as usize;
        if num_skin_ref > limits.max_skin_refs || num_skin_families > limits.max_skin_families {
            return Err(FormatError::LimitExceeded);
        }
        let skin_count = num_skin_ref
            .checked_mul(num_skin_families)
            .ok_or(FormatError::OutOfBounds)?;
        sub_slice(
            data,
            header.skin_index.get() as usize,
            skin_count.checked_mul(2).ok_or(FormatError::OutOfBounds)?,
        )?;
        let (n, i) = count_and_index(header.num_body_parts, header.body_part_index);
        table_bytes(data, i, n, size_of::<Bodypart>(), limits.max_body_parts)?;
        let (n, i) = count_and_index(header.num_attachments, header.attachment_index);
        table_bytes(data, i, n, size_of::<Attachment>(), limits.max_attachments)?;
        let num_transitions = header.num_transitions.get() as usize;
        if num_transitions > limits.max_transitions {
            return Err(FormatError::LimitExceeded);
        }
        let transition_bytes = num_transitions
            .checked_mul(num_transitions)
            .ok_or(FormatError::OutOfBounds)?;
        sub_slice(
            data,
            header.transition_index.get() as usize,
            transition_bytes,
        )?;

        Ok(Self { data, header })
    }

    /// The raw validated header.
    #[must_use]
    pub fn header(&self) -> &RawHeader {
        &self.header
    }

    /// The model's trimmed name.
    #[must_use]
    pub fn name(&self) -> &'a [u8] {
        // Re-slice from the original buffer (rather than `self.header`'s
        // owned copy) so the returned name borrows `'a`. The `name` field
        // sits right after the 4-byte `id` and 4-byte `version` fields;
        // `Self::parse` already validated `data` is at least
        // `size_of::<RawHeader>()` bytes long.
        let start = 8;
        let end = start + NAME64_LEN;
        trim_name(&self.data[start..end])
    }

    /// The skeleton bones, in parent-before-child order (root first).
    pub fn bones(&self, limits: &Limits) -> Result<&'a [Bone]> {
        let (n, i) = count_and_index(self.header.num_bones, self.header.bone_index);
        let bytes = table_bytes(self.data, i, n, size_of::<Bone>(), limits.max_bones)?;
        slice_of(bytes)
    }

    /// Validates that every bone's `parent` index is either `-1` (root) or
    /// a valid, earlier bone index (rejecting self-parenting and forward
    /// references, which would otherwise make hierarchy walks cyclic).
    pub fn validate_bone_hierarchy(bones: &[Bone]) -> Result<()> {
        for (index, bone) in bones.iter().enumerate() {
            let parent = bone.parent.get();
            if parent == -1 {
                continue;
            }
            let parent = usize::try_from(parent).map_err(|_| FormatError::IndexOutOfRange)?;
            if parent >= index {
                return Err(FormatError::IndexOutOfRange);
            }
        }
        Ok(())
    }

    /// The bone controllers.
    pub fn bone_controllers(&self, limits: &Limits) -> Result<&'a [BoneController]> {
        let (n, i) = count_and_index(
            self.header.num_bone_controllers,
            self.header.bone_controller_index,
        );
        let bytes = table_bytes(
            self.data,
            i,
            n,
            size_of::<BoneController>(),
            limits.max_bone_controllers,
        )?;
        slice_of(bytes)
    }

    /// The hitboxes.
    pub fn hitboxes(&self, limits: &Limits) -> Result<&'a [Hitbox]> {
        let (n, i) = count_and_index(self.header.num_hitboxes, self.header.hitbox_index);
        let bytes = table_bytes(self.data, i, n, size_of::<Hitbox>(), limits.max_hitboxes)?;
        slice_of(bytes)
    }

    /// The sequences.
    pub fn sequences(&self, limits: &Limits) -> Result<&'a [Sequence]> {
        let (n, i) = count_and_index(self.header.num_seq, self.header.seq_index);
        let bytes = table_bytes(self.data, i, n, size_of::<Sequence>(), limits.max_sequences)?;
        slice_of(bytes)
    }

    /// The sequence groups.
    pub fn sequence_groups(&self, limits: &Limits) -> Result<&'a [SequenceGroup]> {
        let (n, i) = count_and_index(self.header.num_seq_groups, self.header.seq_group_index);
        let bytes = table_bytes(
            self.data,
            i,
            n,
            size_of::<SequenceGroup>(),
            limits.max_sequence_groups,
        )?;
        slice_of(bytes)
    }

    /// The animation events belonging to `seq`.
    pub fn events(&self, seq: &Sequence, limits: &Limits) -> Result<&'a [AnimEvent]> {
        let n = seq.num_events.get() as usize;
        let i = seq.event_index.get() as usize;
        let bytes = table_bytes(self.data, i, n, size_of::<AnimEvent>(), limits.max_events)?;
        slice_of(bytes)
    }

    /// The texture chunk headers.
    pub fn textures(&self, limits: &Limits) -> Result<&'a [Texture]> {
        let (n, i) = count_and_index(self.header.num_textures, self.header.texture_index);
        let bytes = table_bytes(self.data, i, n, size_of::<Texture>(), limits.max_textures)?;
        slice_of(bytes)
    }

    /// Decodes one texture's 8-bit indexed pixel data and its palette.
    ///
    /// The reviewed documentation's Texture section (`docs/FORMAT_SOURCES.md`)
    /// covers only the 80-byte header; it does not state where the palette
    /// lives. Since GoldSrc textures are the same generic 8-bit-indexed
    /// convention documented for WAD3/BSP30 miptexes elsewhere in this crate
    /// (a trailing 256-entry RGB palette immediately after the pixel data,
    /// no mip levels), this project applies that same convention here.
    pub fn decode_texture(&self, texture: &Texture, limits: &Limits) -> Result<Indexed8<'a>> {
        let width = texture.width.get();
        let height = texture.height.get();
        let pixel_count = checked_pixel_count(width, height, limits.max_texture_pixels)?;
        let start = texture.index.get() as usize;
        let indices = sub_slice(self.data, start, pixel_count)?;
        let palette_start = start
            .checked_add(pixel_count)
            .ok_or(FormatError::OutOfBounds)?;
        let palette_bytes = sub_slice(self.data, palette_start, PALETTE_LEN * 3)?;
        let palette_array = exact_of::<[Rgb8; PALETTE_LEN]>(palette_bytes)?;
        Ok(Indexed8 {
            indices,
            palette: Palette::new(palette_array),
            width,
            height,
        })
    }

    /// The skin-family matrix: `numskinfamilies` rows of `numskinref`
    /// `i16` texture indices each.
    pub fn skin_families(&self, limits: &Limits) -> Result<SkinTable<'a>> {
        let num_skin_ref = self.header.num_skin_ref.get() as usize;
        let num_skin_families = self.header.num_skin_families.get() as usize;
        if num_skin_ref > limits.max_skin_refs || num_skin_families > limits.max_skin_families {
            return Err(FormatError::LimitExceeded);
        }
        let count = num_skin_ref
            .checked_mul(num_skin_families)
            .ok_or(FormatError::OutOfBounds)?;
        let bytes = sub_slice(
            self.data,
            self.header.skin_index.get() as usize,
            count.checked_mul(2).ok_or(FormatError::OutOfBounds)?,
        )?;
        let refs = slice_of::<I16>(bytes)?;
        Ok(SkinTable { refs, num_skin_ref })
    }

    /// The body parts.
    pub fn body_parts(&self, limits: &Limits) -> Result<&'a [Bodypart]> {
        let (n, i) = count_and_index(self.header.num_body_parts, self.header.body_part_index);
        let bytes = table_bytes(
            self.data,
            i,
            n,
            size_of::<Bodypart>(),
            limits.max_body_parts,
        )?;
        slice_of(bytes)
    }

    /// The models belonging to one body part.
    pub fn models(&self, body_part: &Bodypart, limits: &Limits) -> Result<&'a [Model]> {
        let n = body_part.num_models.get() as usize;
        let i = body_part.model_index.get() as usize;
        let bytes = table_bytes(self.data, i, n, size_of::<Model>(), limits.max_models)?;
        slice_of(bytes)
    }

    /// The meshes belonging to one model.
    pub fn meshes(&self, model: &Model, limits: &Limits) -> Result<&'a [Mesh]> {
        let n = model.num_mesh.get() as usize;
        let i = model.mesh_index.get() as usize;
        let bytes = table_bytes(self.data, i, n, size_of::<Mesh>(), limits.max_meshes)?;
        slice_of(bytes)
    }

    /// A model's vertex positions.
    pub fn vertices(&self, model: &Model, limits: &Limits) -> Result<&'a [Vec3]> {
        let n = model.num_verts.get() as usize;
        let i = model.vert_index.get() as usize;
        let bytes = table_bytes(self.data, i, n, size_of::<Vec3>(), limits.max_verts)?;
        slice_of(bytes)
    }

    /// A model's per-vertex bone indices (one byte per vertex; see the note
    /// on [`Model`]'s `vert_info_index` field for why this project treats
    /// the array as byte-sized rather than the descriptive-only "array of
    /// int" wording in the reviewed documentation).
    pub fn vertex_bones(&self, model: &Model, limits: &Limits) -> Result<&'a [u8]> {
        let n = model.num_verts.get() as usize;
        let i = model.vert_info_index.get() as usize;
        table_bytes(self.data, i, n, 1, limits.max_verts)
    }

    /// A model's vertex normals.
    pub fn normals(&self, model: &Model, limits: &Limits) -> Result<&'a [Vec3]> {
        let n = model.num_norms.get() as usize;
        let i = model.norm_index.get() as usize;
        let bytes = table_bytes(self.data, i, n, size_of::<Vec3>(), limits.max_verts)?;
        slice_of(bytes)
    }

    /// A model's per-normal bone indices (one byte per normal).
    pub fn normal_bones(&self, model: &Model, limits: &Limits) -> Result<&'a [u8]> {
        let n = model.num_norms.get() as usize;
        let i = model.norm_info_index.get() as usize;
        table_bytes(self.data, i, n, 1, limits.max_verts)
    }

    /// Validates that every entry of a bone-index array (from
    /// [`Mdl::vertex_bones`] or [`Mdl::normal_bones`]) refers to a valid
    /// bone.
    pub fn validate_bone_indices(indices: &[u8], bone_count: usize) -> Result<()> {
        for &index in indices {
            if index as usize >= bone_count {
                return Err(FormatError::IndexOutOfRange);
            }
        }
        Ok(())
    }

    /// The attachments.
    pub fn attachments(&self, limits: &Limits) -> Result<&'a [Attachment]> {
        let (n, i) = count_and_index(self.header.num_attachments, self.header.attachment_index);
        let bytes = table_bytes(
            self.data,
            i,
            n,
            size_of::<Attachment>(),
            limits.max_attachments,
        )?;
        slice_of(bytes)
    }

    /// The sequence transition graph: a `numtransitions x numtransitions`
    /// byte matrix (row-major). Entry/exit node numbers in [`Sequence`] are
    /// documented as 1-based; subtract 1 before indexing.
    pub fn transitions(&self, limits: &Limits) -> Result<TransitionGraph<'a>> {
        let n = self.header.num_transitions.get() as usize;
        if n > limits.max_transitions {
            return Err(FormatError::LimitExceeded);
        }
        let len = n.checked_mul(n).ok_or(FormatError::OutOfBounds)?;
        let bytes = sub_slice(self.data, self.header.transition_index.get() as usize, len)?;
        Ok(TransitionGraph { bytes, n })
    }

    /// Decodes `mesh`'s triangle-strip/fan command stream into a bounded
    /// list of triangles.
    ///
    /// Every command's declared trivert count is validated against
    /// `limits.max_triverts` (checked cumulatively across the whole mesh,
    /// not just per-command) before any allocation grows, and decoding
    /// stops with an error rather than reading past `mesh.num_tris`
    /// triverts' worth of command-stream bytes.
    pub fn decode_mesh_commands(&self, mesh: &Mesh, limits: &Limits) -> Result<Vec<Triangle>> {
        let num_tris =
            usize::try_from(mesh.num_tris.get()).map_err(|_| FormatError::InvalidInput)?;
        if num_tris > limits.max_triverts {
            return Err(FormatError::LimitExceeded);
        }
        // `num_tris` is the documented trivert budget for this mesh's whole
        // command stream (every strip/fan run's triverts combined), so
        // reserve exactly that many `RawTrivert` records' worth of bytes.
        let trivert_bytes_len = num_tris
            .checked_mul(size_of::<RawTrivert>())
            .ok_or(FormatError::OutOfBounds)?;
        // The command stream is `count: i32` headers interleaved with
        // trivert records; the exact byte length isn't known up front
        // (headers are interspersed), so read from `tri_index` onward and
        // let `sub_slice` bound each access as we walk it instead.
        let mut cursor = mesh.tri_index.get() as usize;
        let mut consumed_triverts = 0usize;
        let mut triangles = Vec::new();

        loop {
            if consumed_triverts >= num_tris {
                break;
            }
            let header_bytes = sub_slice(self.data, cursor, 4)?;
            let count = i32::from_le_bytes([
                header_bytes[0],
                header_bytes[1],
                header_bytes[2],
                header_bytes[3],
            ]);
            cursor = cursor.checked_add(4).ok_or(FormatError::OutOfBounds)?;
            if count == 0 {
                break;
            }
            let is_fan = count < 0;
            let run_len = count.unsigned_abs() as usize;
            if consumed_triverts
                .checked_add(run_len)
                .is_none_or(|total| total > num_tris)
            {
                return Err(FormatError::LimitExceeded);
            }
            let run_bytes = sub_slice(
                self.data,
                cursor,
                run_len
                    .checked_mul(size_of::<RawTrivert>())
                    .ok_or(FormatError::OutOfBounds)?,
            )?;
            cursor = cursor
                .checked_add(run_bytes.len())
                .ok_or(FormatError::OutOfBounds)?;
            let raw_verts: &[RawTrivert] = slice_of(run_bytes)?;
            let verts: Vec<Trivert> = raw_verts
                .iter()
                .map(|v| Trivert {
                    vert_index: v.vert_index.get().cast_unsigned(),
                    norm_index: v.norm_index.get().cast_unsigned(),
                    s: v.s.get(),
                    t: v.t.get(),
                })
                .collect();
            if run_len < 3 {
                consumed_triverts += run_len;
                continue;
            }
            if is_fan {
                for i in 1..run_len - 1 {
                    triangles.push(Triangle {
                        verts: [verts[0], verts[i], verts[i + 1]],
                    });
                }
            } else {
                for i in 0..run_len - 2 {
                    // Preserve winding order by swapping every other
                    // triangle, matching the documented triangle-strip
                    // convention.
                    if i % 2 == 0 {
                        triangles.push(Triangle {
                            verts: [verts[i], verts[i + 1], verts[i + 2]],
                        });
                    } else {
                        triangles.push(Triangle {
                            verts: [verts[i + 1], verts[i], verts[i + 2]],
                        });
                    }
                }
            }
            consumed_triverts += run_len;
            if triangles.len() > limits.max_triverts {
                return Err(FormatError::LimitExceeded);
            }
        }
        let _ = trivert_bytes_len;
        Ok(triangles)
    }

    /// Samples one bone's local-space position/rotation at `frame` within
    /// `seq`.
    ///
    /// `anim_data` is the buffer the sequence's animation offsets are
    /// relative to: pass this `Mdl`'s own bytes when `seq.seq_group == 0`
    /// (animation embedded in the main file); for a non-zero `seq_group`
    /// this crate does not resolve the external file itself, so the caller
    /// must parse it via [`SequenceGroupFile::parse`] and pass its bytes.
    /// Only the first blend animation (index 0) is sampled; blending
    /// multiple animations together is left to the caller (see "Animation
    /// blending" in `docs/FORMAT_SOURCES.md`).
    pub fn sample_bone_animation(
        anim_data: &[u8],
        seq: &Sequence,
        bones: &[Bone],
        frame: u32,
        limits: &Limits,
    ) -> Result<Vec<BonePose>> {
        let bone_count = bones.len();
        let anim_index = seq.anim_index.get() as usize;
        let mut poses = Vec::with_capacity(bone_count);
        for (bone_index, bone) in bones.iter().enumerate() {
            let record_offset = anim_index
                .checked_add(
                    bone_index
                        .checked_mul(size_of::<AnimOffsets>())
                        .ok_or(FormatError::OutOfBounds)?,
                )
                .ok_or(FormatError::OutOfBounds)?;
            let record_bytes = sub_slice(anim_data, record_offset, size_of::<AnimOffsets>())?;
            let offsets: &AnimOffsets = exact_of(record_bytes)?;

            let mut channel = [0.0f32; 6];
            for (slot, value) in channel.iter_mut().enumerate() {
                let raw_offset = offsets.offset[slot].get();
                *value = if raw_offset == 0 {
                    bone.value[slot].get()
                } else {
                    let base = record_offset
                        .checked_add(raw_offset as usize)
                        .ok_or(FormatError::OutOfBounds)?;
                    let delta = decode_anim_channel(anim_data, base, frame, limits)?;
                    bone.value[slot].get() + delta * bone.scale[slot].get()
                };
            }
            let position = [channel[0], channel[1], channel[2]];
            let rotation = euler_to_quaternion([channel[3], channel[4], channel[5]]);
            poses.push(BonePose { position, rotation });
        }
        Ok(poses)
    }
}

/// The `numskinfamilies x numskinref` skin-remapping matrix.
pub struct SkinTable<'a> {
    refs: &'a [I16],
    num_skin_ref: usize,
}

impl SkinTable<'_> {
    /// Looks up the texture index used by `family` for replaceable-texture
    /// slot `slot`.
    pub fn get(&self, family: usize, slot: usize) -> Result<i16> {
        if slot >= self.num_skin_ref {
            return Err(FormatError::IndexOutOfRange);
        }
        let index = family
            .checked_mul(self.num_skin_ref)
            .and_then(|base| base.checked_add(slot))
            .ok_or(FormatError::IndexOutOfRange)?;
        self.refs
            .get(index)
            .map(|v: &zerocopy::byteorder::little_endian::I16| v.get())
            .ok_or(FormatError::IndexOutOfRange)
    }
}

/// A validated `numtransitions x numtransitions` byte matrix.
pub struct TransitionGraph<'a> {
    bytes: &'a [u8],
    n: usize,
}

impl TransitionGraph<'_> {
    /// The graph's side length (`numtransitions`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Whether the graph has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Looks up `[from][to]` (already 0-based; callers subtract the
    /// documented 1-based `entrynode`/`exitnode` themselves).
    pub fn get(&self, from: usize, to: usize) -> Result<u8> {
        if from >= self.n || to >= self.n {
            return Err(FormatError::IndexOutOfRange);
        }
        let index = from
            .checked_mul(self.n)
            .and_then(|base| base.checked_add(to))
            .ok_or(FormatError::IndexOutOfRange)?;
        self.bytes
            .get(index)
            .copied()
            .ok_or(FormatError::OutOfBounds)
    }
}

/// A parsed, validated `IDSQ` sequence-group file (or a main file's header
/// read only for its identity, per the documented "a regular header may
/// also be used" note).
pub struct SequenceGroupFile<'a> {
    data: &'a [u8],
}

impl<'a> SequenceGroupFile<'a> {
    /// Parses and validates a sequence-group file's 76-byte header,
    /// accepting either the `IDSQ` or `IDST` magic (both are documented as
    /// valid for this file kind).
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let (header, _): (&RawSequenceHeader, _) = prefix_of(data)?;
        if header.id != MAGIC_IDSQ && header.id != MAGIC_IDST {
            return Err(FormatError::BadSignature);
        }
        if header.version.get() != VERSION {
            return Err(FormatError::BadSignature);
        }
        Ok(Self { data })
    }

    /// The full file bytes, to pass into [`Mdl::sample_bone_animation`] as
    /// `anim_data`.
    #[must_use]
    pub fn bytes(&self) -> &'a [u8] {
        self.data
    }
}

// ---------------------------------------------------------------------
// Compressed animation-value decoding
// ---------------------------------------------------------------------

/// Decodes one motion-type channel's value at `frame`, starting at byte
/// offset `start` in `data`, per this project's own run/hold interpretation
/// of the documented `valid`/`total` semantics (see [`AnimValueHeader`]).
///
/// Each run is a 2-byte `(valid, total)` header followed by `valid` raw
/// `i16` values; the run covers `total` frames total, so frames
/// `valid..total` (if any) repeat the run's last stored value. Walking
/// stops (bounded by `limits.max_frame_walk`) once the run containing
/// `frame` is found.
pub fn decode_anim_channel(data: &[u8], start: usize, frame: u32, limits: &Limits) -> Result<f32> {
    let mut cursor = start;
    let mut frames_seen: u32 = 0;
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > limits.max_frame_walk {
            return Err(FormatError::RecursionLimitExceeded);
        }
        let header_bytes = sub_slice(data, cursor, size_of::<AnimValueHeader>())?;
        let header: &AnimValueHeader = exact_of(header_bytes)?;
        let valid = u32::from(header.valid);
        let total = u32::from(header.total).max(1);
        let values_bytes = sub_slice(
            data,
            cursor.checked_add(2).ok_or(FormatError::OutOfBounds)?,
            (valid as usize)
                .checked_mul(2)
                .ok_or(FormatError::OutOfBounds)?,
        )?;
        let values: &[I16] = slice_of(values_bytes)?;
        if frame < frames_seen + total {
            let within = frame - frames_seen;
            let value = if (within as usize) < values.len() {
                values[within as usize].get()
            } else {
                values
                    .last()
                    .map_or(0, |v: &zerocopy::byteorder::little_endian::I16| v.get())
            };
            return Ok(f32::from(value));
        }
        frames_seen += total;
        cursor = cursor
            .checked_add(2)
            .and_then(|c| c.checked_add(values_bytes.len()))
            .ok_or(FormatError::OutOfBounds)?;
    }
}

// ---------------------------------------------------------------------
// Euler -> quaternion
// ---------------------------------------------------------------------

/// Converts a `[roll (X), pitch (Y), yaw (Z)]` Euler-angle triple (radians)
/// into a unit quaternion `[x, y, z, w]`, applying yaw, then pitch, then
/// roll (matching the axis convention documented for MDL v10 bone
/// rotations in `docs/FORMAT_SOURCES.md`: "pitch is the rotation around the
/// Y axis, yaw is the rotation around the Z axis and roll is the rotation
/// around the X axis"). This is a minimal, dependency-free implementation
/// (no external quaternion/matrix crate).
#[must_use]
pub fn euler_to_quaternion(angles: [f32; 3]) -> [f32; 4] {
    let (roll, pitch, yaw) = (angles[0], angles[1], angles[2]);
    let (sr, cr) = libm_sincos(roll * 0.5);
    let (sp, cp) = libm_sincos(pitch * 0.5);
    let (sy, cy) = libm_sincos(yaw * 0.5);

    // Quaternion for R = Rz(yaw) * Ry(pitch) * Rx(roll).
    let w = cr * cp * cy + sr * sp * sy;
    let x = sr * cp * cy - cr * sp * sy;
    let y = cr * sp * cy + sr * cp * sy;
    let z = cr * cp * sy - sr * sp * cy;
    [x, y, z, w]
}

/// A tiny, allocation-free `(sin, cos)` helper so this module does not need
/// `std` or an external math crate for the handful of trig calls in
/// [`euler_to_quaternion`].
fn libm_sincos(x: f32) -> (f32, f32) {
    (libm_sin(x), libm_sin(x + core::f32::consts::FRAC_PI_2))
}

/// A bounded-error `sin` approximation good enough for animation preview:
/// range-reduces to `[-pi, pi]` then uses a degree-7 minimax-style
/// (Bhaskara I-derived) polynomial. This project intentionally avoids
/// `std`'s `f32::sin` (unavailable in `no_std`) and an external math crate.
fn libm_sin(x: f32) -> f32 {
    const TAU: f32 = core::f32::consts::PI * 2.0;
    let mut y = x % TAU;
    if y > core::f32::consts::PI {
        y -= TAU;
    } else if y < -core::f32::consts::PI {
        y += TAU;
    }
    // Bhaskara I's sine approximation, accurate to within ~0.002 over the
    // full range; sufficient for previewing bone orientation.
    let pi = core::f32::consts::PI;
    if y >= 0.0 {
        (16.0 * y * (pi - y)) / (5.0 * pi * pi - 4.0 * y * (pi - y))
    } else {
        let ay = -y;
        -((16.0 * ay * (pi - ay)) / (5.0 * pi * pi - 4.0 * ay * (pi - ay)))
    }
}
