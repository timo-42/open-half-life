//! Synthetic BSP30/WAD3 fixture writers for this crate's own tests, its
//! `proptest` suites, and the standalone `fuzz/` corpora.
//!
//! Every byte produced here is authored by this project for testing only;
//! nothing is derived from any game installation (see `docs/CLEAN_ROOM.md`).
//! Exposed as a public module (gated by the `test-support` feature, and
//! always available under `#[cfg(test)]`) specifically so the `fuzz/`
//! workspace — which cannot see this crate's `#[cfg(test)]` items — can
//! build its corpus from the same synthetic writers used by the in-tree
//! tests, rather than duplicating them.

use alloc::vec::Vec;

use crate::palette::{PALETTE_LEN, Rgb8};

/// A deterministic, project-authored 256-color palette (a grayscale ramp).
#[must_use]
pub fn synthetic_palette() -> [Rgb8; PALETTE_LEN] {
    let mut palette = [Rgb8::new(0, 0, 0); PALETTE_LEN];
    for (i, entry) in palette.iter_mut().enumerate() {
        // `i` ranges over `0..PALETTE_LEN` (256), so this never truncates.
        #[allow(clippy::cast_possible_truncation)]
        let level = i as u8;
        *entry = Rgb8::new(level, level, level);
    }
    palette
}

/// Encodes a name into a fixed 16-byte, NUL-padded field.
#[must_use]
pub fn fixed_name(name: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let bytes = name.as_bytes();
    let len = bytes.len().min(16);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

/// Builds a complete miptex body (the 40-byte name/width/height/offsets
/// header, four mip levels, and the trailing `u16` palette length + 768-byte
/// palette), matching the layout documented in `docs/FORMAT_SOURCES.md` for
/// both WAD3 `0x43` entries and embedded BSP30 textures.
#[must_use]
pub fn build_miptex_body(
    name: &str,
    width: u32,
    height: u32,
    fill: u8,
    palette: &[Rgb8; PALETTE_LEN],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&fixed_name(name));
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());

    let sizes = [
        (width, height),
        (width / 2, height / 2),
        (width / 4, height / 4),
        (width / 8, height / 8),
    ];
    let mut offset = 40u32;
    let mut offsets = [0u32; 4];
    for (i, (w, h)) in sizes.iter().enumerate() {
        offsets[i] = offset;
        offset += w * h;
    }
    for value in offsets {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for (w, h) in sizes {
        let count = (w * h) as usize;
        out.extend(core::iter::repeat_n(fill, count));
    }
    let palette_len_u16 = u16::try_from(PALETTE_LEN).expect("PALETTE_LEN fits u16");
    out.extend_from_slice(&palette_len_u16.to_le_bytes());
    for entry in palette {
        out.push(entry.r);
        out.push(entry.g);
        out.push(entry.b);
    }
    out
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// The 15 BSP30 lump indices, matching `bsp30::LumpId`'s order.
const LUMP_COUNT: usize = 15;

/// A synthetic BSP30 file builder.
///
/// Each field is the raw byte contents of one lump; call the typed `push_*`
/// helpers to append well-formed records, or mutate a lump's `Vec<u8>`
/// directly (it is `pub(crate)`... actually `pub`, since callers writing
/// malformed-field tests need direct byte access) to construct deliberately
/// malformed fixtures.
#[derive(Default)]
pub struct Bsp30Builder {
    pub entities: Vec<u8>,
    pub planes: Vec<u8>,
    pub vertexes: Vec<u8>,
    pub visibility: Vec<u8>,
    pub nodes: Vec<u8>,
    pub texinfo: Vec<u8>,
    pub faces: Vec<u8>,
    pub lighting: Vec<u8>,
    pub clipnodes: Vec<u8>,
    pub leaves: Vec<u8>,
    pub marksurfaces: Vec<u8>,
    pub edges: Vec<u8>,
    pub surfedges: Vec<u8>,
    pub models: Vec<u8>,
    texture_offsets: Vec<u32>,
    /// The concatenated raw bytes of every texture body appended so far, in
    /// slot order. Exposed so malformed-fixture tests can corrupt an
    /// individual texture's fields (offsets, dimensions, ...) in place.
    pub texture_bodies: Vec<u8>,
}

impl Bsp30Builder {
    /// A builder with every lump empty.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the entities lump to `text` followed by a single trailing NUL.
    pub fn set_entities_text(&mut self, text: &str) {
        self.entities.clear();
        self.entities.extend_from_slice(text.as_bytes());
        self.entities.push(0);
    }

    pub fn push_plane(&mut self, normal: [f32; 3], dist: f32, kind: i32) {
        for v in normal {
            push_f32(&mut self.planes, v);
        }
        push_f32(&mut self.planes, dist);
        push_i32(&mut self.planes, kind);
    }

    pub fn push_vertex(&mut self, point: [f32; 3]) {
        for v in point {
            push_f32(&mut self.vertexes, v);
        }
    }

    pub fn push_edge(&mut self, v0: u16, v1: u16) {
        push_u16(&mut self.edges, v0);
        push_u16(&mut self.edges, v1);
    }

    pub fn push_surfedge(&mut self, v: i32) {
        push_i32(&mut self.surfedges, v);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_node(
        &mut self,
        plane: u32,
        front: i16,
        back: i16,
        mins: [i16; 3],
        maxs: [i16; 3],
        first_face: u16,
        num_faces: u16,
    ) {
        push_u32(&mut self.nodes, plane);
        push_i16(&mut self.nodes, front);
        push_i16(&mut self.nodes, back);
        for v in mins {
            push_i16(&mut self.nodes, v);
        }
        for v in maxs {
            push_i16(&mut self.nodes, v);
        }
        push_u16(&mut self.nodes, first_face);
        push_u16(&mut self.nodes, num_faces);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_leaf(
        &mut self,
        contents: i32,
        vis_offset: i32,
        mins: [i16; 3],
        maxs: [i16; 3],
        first_marksurface: u16,
        num_marksurfaces: u16,
        ambient_levels: [u8; 4],
    ) {
        push_i32(&mut self.leaves, contents);
        push_i32(&mut self.leaves, vis_offset);
        for v in mins {
            push_i16(&mut self.leaves, v);
        }
        for v in maxs {
            push_i16(&mut self.leaves, v);
        }
        push_u16(&mut self.leaves, first_marksurface);
        push_u16(&mut self.leaves, num_marksurfaces);
        self.leaves.extend_from_slice(&ambient_levels);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_face(
        &mut self,
        plane: u16,
        plane_side: u16,
        first_edge: u32,
        num_edges: u16,
        texinfo: u16,
        styles: [u8; 4],
        lightmap_offset: i32,
    ) {
        push_u16(&mut self.faces, plane);
        push_u16(&mut self.faces, plane_side);
        push_u32(&mut self.faces, first_edge);
        push_u16(&mut self.faces, num_edges);
        push_u16(&mut self.faces, texinfo);
        self.faces.extend_from_slice(&styles);
        push_i32(&mut self.faces, lightmap_offset);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_texinfo(
        &mut self,
        s_vector: [f32; 3],
        s_shift: f32,
        t_vector: [f32; 3],
        t_shift: f32,
        miptex_index: u32,
        flags: u32,
    ) {
        for v in s_vector {
            push_f32(&mut self.texinfo, v);
        }
        push_f32(&mut self.texinfo, s_shift);
        for v in t_vector {
            push_f32(&mut self.texinfo, v);
        }
        push_f32(&mut self.texinfo, t_shift);
        push_u32(&mut self.texinfo, miptex_index);
        push_u32(&mut self.texinfo, flags);
    }

    pub fn push_clipnode(&mut self, plane: i32, front: i16, back: i16) {
        push_i32(&mut self.clipnodes, plane);
        push_i16(&mut self.clipnodes, front);
        push_i16(&mut self.clipnodes, back);
    }

    pub fn push_marksurface(&mut self, face: u16) {
        push_u16(&mut self.marksurfaces, face);
    }

    pub fn push_lighting_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.lighting.extend_from_slice(&[r, g, b]);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_model(
        &mut self,
        mins: [f32; 3],
        maxs: [f32; 3],
        origin: [f32; 3],
        headnodes: [i32; 4],
        vis_leafs: i32,
        first_face: i32,
        num_faces: i32,
    ) {
        for v in mins {
            push_f32(&mut self.models, v);
        }
        for v in maxs {
            push_f32(&mut self.models, v);
        }
        for v in origin {
            push_f32(&mut self.models, v);
        }
        for v in headnodes {
            push_i32(&mut self.models, v);
        }
        push_i32(&mut self.models, vis_leafs);
        push_i32(&mut self.models, first_face);
        push_i32(&mut self.models, num_faces);
    }

    /// Appends an embedded texture (pixel data + palette present) to the
    /// texture directory and returns its slot index.
    pub fn add_embedded_texture(&mut self, name: &str, width: u32, height: u32, fill: u8) -> usize {
        let palette = synthetic_palette();
        let body = build_miptex_body(name, width, height, fill, &palette);
        let index = self.texture_offsets.len();
        // Offsets are relative to the start of the textures lump; the body
        // will be appended after the directory header, computed in `build`.
        self.texture_offsets
            .push(u32::try_from(self.texture_bodies.len()).unwrap());
        self.texture_bodies.extend_from_slice(&body);
        index
    }

    /// Appends an external texture (name/dimensions only, all mip offsets
    /// `0`) to the texture directory and returns its slot index.
    pub fn add_external_texture(&mut self, name: &str, width: u32, height: u32) -> usize {
        let mut body = Vec::new();
        body.extend_from_slice(&fixed_name(name));
        body.extend_from_slice(&width.to_le_bytes());
        body.extend_from_slice(&height.to_le_bytes());
        body.extend_from_slice(&[0u8; 16]); // four zero offsets
        let index = self.texture_offsets.len();
        self.texture_offsets
            .push(u32::try_from(self.texture_bodies.len()).unwrap());
        self.texture_bodies.extend_from_slice(&body);
        index
    }

    /// Appends a "missing" texture slot (the documented `0xFFFF_FFFF`
    /// sentinel offset, no body).
    pub fn add_missing_texture_slot(&mut self) -> usize {
        let index = self.texture_offsets.len();
        self.texture_offsets.push(u32::MAX);
        index
    }

    fn build_textures_lump(&self) -> Vec<u8> {
        let mut lump = Vec::new();
        push_u32(
            &mut lump,
            u32::try_from(self.texture_offsets.len()).unwrap(),
        );
        let directory_len = 4 + self.texture_offsets.len() * 4;
        for offset in &self.texture_offsets {
            if *offset == u32::MAX {
                push_u32(&mut lump, u32::MAX);
            } else {
                push_u32(&mut lump, u32::try_from(directory_len).unwrap() + offset);
            }
        }
        lump.extend_from_slice(&self.texture_bodies);
        lump
    }

    /// Serializes the whole file: a version-30 header followed by the 15
    /// lumps in their documented order.
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        let textures = self.build_textures_lump();
        let lumps: [&[u8]; LUMP_COUNT] = [
            &self.entities,
            &self.planes,
            &textures,
            &self.vertexes,
            &self.visibility,
            &self.nodes,
            &self.texinfo,
            &self.faces,
            &self.lighting,
            &self.clipnodes,
            &self.leaves,
            &self.marksurfaces,
            &self.edges,
            &self.surfedges,
            &self.models,
        ];

        let header_len = 4 + LUMP_COUNT * 8;
        let mut out = Vec::new();
        push_i32(&mut out, 30);
        let mut offset = header_len;
        let mut directory = Vec::new();
        for lump in &lumps {
            push_u32(&mut directory, u32::try_from(offset).unwrap());
            push_u32(&mut directory, u32::try_from(lump.len()).unwrap());
            offset += lump.len();
        }
        out.extend_from_slice(&directory);
        for lump in &lumps {
            out.extend_from_slice(lump);
        }
        out
    }
}

#[derive(Clone, Copy)]
struct WadEntryMeta {
    name: [u8; 16],
    kind: u8,
}

/// A synthetic WAD3 file builder.
#[derive(Default)]
pub struct Wad3Builder {
    entries: Vec<(WadEntryMeta, Vec<u8>)>,
}

impl Wad3Builder {
    /// A builder with no entries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a `0x43` miptex entry with a synthetic checkerboard-free pixel
    /// fill and the project's synthetic palette.
    pub fn add_miptex(&mut self, name: &str, width: u32, height: u32, fill: u8) {
        let palette = synthetic_palette();
        let body = build_miptex_body(name, width, height, fill, &palette);
        self.entries.push((
            WadEntryMeta {
                name: fixed_name(name),
                kind: 0x43,
            },
            body,
        ));
    }

    /// Adds a raw entry with an arbitrary type byte and body, for
    /// malformed-fixture tests.
    pub fn add_raw_entry(&mut self, name: &str, kind: u8, body: Vec<u8>) {
        self.entries.push((
            WadEntryMeta {
                name: fixed_name(name),
                kind,
            },
            body,
        ));
    }

    /// Serializes the whole file: header, entry bodies, then the directory.
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"WAD3");
        push_u32(&mut out, u32::try_from(self.entries.len()).unwrap());
        // Placeholder for dir_offset, patched below.
        let dir_offset_pos = out.len();
        push_u32(&mut out, 0);

        let mut directory = Vec::new();
        for (entry, body) in &self.entries {
            let offset = u32::try_from(out.len()).unwrap();
            out.extend_from_slice(body);
            push_u32(&mut directory, offset);
            push_u32(&mut directory, u32::try_from(body.len()).unwrap());
            push_u32(&mut directory, u32::try_from(body.len()).unwrap());
            directory.push(entry.kind);
            directory.push(0); // compression
            directory.extend_from_slice(&[0, 0]); // padding
            directory.extend_from_slice(&entry.name);
        }

        let dir_offset = u32::try_from(out.len()).unwrap();
        out[dir_offset_pos..dir_offset_pos + 4].copy_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&directory);
        out
    }
}

// ---------------------------------------------------------------------
// MDL v10 / SPR fixtures
// ---------------------------------------------------------------------

/// Encodes a name into a fixed, NUL-padded field of any size (a generalized
/// [`fixed_name`] for the 32/64-byte name fields used by MDL v10).
#[must_use]
pub fn fixed_name_sized<const N: usize>(name: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let bytes = name.as_bytes();
    let len = bytes.len().min(N);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

/// Byte offsets and sizes of every section in [`build_minimal_mdl10`]'s
/// output, so tests can locate and corrupt one field at a time without
/// re-deriving this layout by hand.
#[derive(Debug, Clone, Copy)]
pub struct MinimalMdl10Layout {
    pub header_size: usize,
    pub bones_offset: usize,
    pub bones_size: usize,
    pub textures_offset: usize,
    pub texture_data_offset: usize,
    pub texture_data_size: usize,
    pub skin_refs_offset: usize,
    pub sequences_offset: usize,
    pub body_parts_offset: usize,
    pub models_offset: usize,
    pub meshes_offset: usize,
    pub verts_offset: usize,
    pub vert_bones_offset: usize,
    pub norms_offset: usize,
    pub norm_bones_offset: usize,
    pub tricommands_offset: usize,
    pub anim_data_offset: usize,
    pub total_len: usize,
}

/// Builds a minimal, well-formed, synthetic MDL v10 file: 2 bones (a root
/// and one child), 1 texture (16x16), 1 body part containing 1 model
/// containing 1 mesh (a 4-trivert / 2-triangle strip over a synthetic
/// quad), 1 sequence with 2 frames (bone 0's X position channel carries a
/// real 2-value compressed animation run; every other channel is the
/// bind-pose default), and no bone controllers, hitboxes, sequence groups,
/// attachments, events, or transitions.
///
/// Every byte here is authored by this project for testing only; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build_minimal_mdl10() -> (Vec<u8>, MinimalMdl10Layout) {
    const HEADER_SIZE: usize = 244;
    const BONE_SIZE: usize = 112;
    const TEXTURE_SIZE: usize = 80;
    const SEQUENCE_SIZE: usize = 176;
    const BODYPART_SIZE: usize = 76;
    const MODEL_SIZE: usize = 112;
    const MESH_SIZE: usize = 20;
    const VEC3_SIZE: usize = 12;
    const TRIVERT_SIZE: usize = 8;

    let tex_width = 16u32;
    let tex_height = 16u32;
    let palette = synthetic_palette();
    let pixel_count = (tex_width * tex_height) as usize;
    let texture_data_size = pixel_count + PALETTE_LEN * 3;

    let bones_offset = HEADER_SIZE;
    let bones_size = BONE_SIZE * 2;
    let textures_offset = bones_offset + bones_size;
    let texture_data_offset = textures_offset + TEXTURE_SIZE;
    let skin_refs_offset = texture_data_offset + texture_data_size;
    let skin_refs_size = 2; // 1 family * 1 ref * 2 bytes
    let sequences_offset = skin_refs_offset + skin_refs_size;
    let body_parts_offset = sequences_offset + SEQUENCE_SIZE;
    let models_offset = body_parts_offset + BODYPART_SIZE;
    let meshes_offset = models_offset + MODEL_SIZE;
    let verts_offset = meshes_offset + MESH_SIZE;
    let verts_size = VEC3_SIZE * 4;
    let vert_bones_offset = verts_offset + verts_size;
    let vert_bones_size = 4;
    let norms_offset = vert_bones_offset + vert_bones_size;
    let norms_size = VEC3_SIZE;
    let norm_bones_offset = norms_offset + norms_size;
    let norm_bones_size = 1;
    let tricommands_offset = norm_bones_offset + norm_bones_size;
    // A 2-byte `i16` run-length header, the run's 4 triverts, and a 2-byte
    // `i16` zero terminator (see `Mdl::decode_mesh_commands`'s doc comment
    // for why the header and terminator are 16-bit, not 32-bit).
    let tricommands_size = 2 + TRIVERT_SIZE * 4 + 2;
    let anim_data_offset = tricommands_offset + tricommands_size;
    // 2 bones * 12-byte offset records, then one 6-byte compressed run
    // (2-byte valid/total header + 2 `i16` values) for bone 0's slot 0.
    let anim_data_size = 12 * 2 + 6;
    let total_len = anim_data_offset + anim_data_size;

    let mut out = Vec::with_capacity(total_len);

    // --- Header (patched with real offsets below; write zeros first). ---
    out.extend_from_slice(b"IDST");
    push_i32(&mut out, 10); // version
    out.extend_from_slice(&fixed_name_sized::<64>("minimal"));
    push_u32(&mut out, u32::try_from(total_len).unwrap()); // length
    for _ in 0..3 {
        push_f32(&mut out, 0.0); // eyeposition
    }
    for _ in 0..12 {
        push_f32(&mut out, 0.0); // min, max, bbmin, bbmax
    }
    push_i32(&mut out, 0); // flags
    push_u32(&mut out, 2); // num_bones
    push_u32(&mut out, u32::try_from(bones_offset).unwrap());
    push_u32(&mut out, 0); // num_bone_controllers
    push_u32(&mut out, u32::try_from(textures_offset).unwrap()); // bone_controller_index (count 0, any in-bounds value)
    push_u32(&mut out, 0); // num_hitboxes
    push_u32(&mut out, u32::try_from(textures_offset).unwrap()); // hitbox_index
    push_u32(&mut out, 1); // num_seq
    push_u32(&mut out, u32::try_from(sequences_offset).unwrap());
    push_u32(&mut out, 0); // num_seq_groups
    push_u32(&mut out, u32::try_from(sequences_offset).unwrap()); // seq_group_index
    push_u32(&mut out, 1); // num_textures
    push_u32(&mut out, u32::try_from(textures_offset).unwrap());
    push_u32(&mut out, u32::try_from(texture_data_offset).unwrap()); // texture_data_index
    push_u32(&mut out, 1); // num_skin_ref
    push_u32(&mut out, 1); // num_skin_families
    push_u32(&mut out, u32::try_from(skin_refs_offset).unwrap());
    push_u32(&mut out, 1); // num_body_parts
    push_u32(&mut out, u32::try_from(body_parts_offset).unwrap());
    push_u32(&mut out, 0); // num_attachments
    push_u32(&mut out, u32::try_from(anim_data_offset).unwrap()); // attachment_index
    push_i32(&mut out, 0); // sound_table
    push_i32(&mut out, 0); // sound_index
    push_i32(&mut out, 0); // sound_groups
    push_i32(&mut out, 0); // sound_group_index
    push_u32(&mut out, 0); // num_transitions
    push_u32(&mut out, u32::try_from(anim_data_offset).unwrap()); // transition_index
    assert_eq!(out.len(), HEADER_SIZE);

    // --- Bones ---
    // Bone 0: root.
    out.extend_from_slice(&fixed_name_sized::<32>("root"));
    push_i32(&mut out, -1); // parent
    push_i32(&mut out, 0); // flags
    for _ in 0..6 {
        push_i32(&mut out, -1); // bonecontroller
    }
    for _ in 0..6 {
        push_f32(&mut out, 0.0); // value
    }
    for _ in 0..6 {
        push_f32(&mut out, 1.0); // scale
    }
    // Bone 1: child of bone 0.
    out.extend_from_slice(&fixed_name_sized::<32>("child"));
    push_i32(&mut out, 0); // parent
    push_i32(&mut out, 0); // flags
    for _ in 0..6 {
        push_i32(&mut out, -1);
    }
    for _ in 0..6 {
        push_f32(&mut out, 0.0);
    }
    for _ in 0..6 {
        push_f32(&mut out, 1.0);
    }
    assert_eq!(out.len(), textures_offset);

    // --- Texture ---
    out.extend_from_slice(&fixed_name_sized::<64>("wall"));
    push_u32(&mut out, 0); // flags
    push_u32(&mut out, tex_width);
    push_u32(&mut out, tex_height);
    push_u32(&mut out, u32::try_from(texture_data_offset).unwrap());
    assert_eq!(out.len(), texture_data_offset);
    out.extend(core::iter::repeat_n(7u8, pixel_count));
    for entry in &palette {
        out.push(entry.r);
        out.push(entry.g);
        out.push(entry.b);
    }
    assert_eq!(out.len(), skin_refs_offset);

    // --- Skin refs (1 family x 1 ref) ---
    push_i16(&mut out, 0);
    assert_eq!(out.len(), sequences_offset);

    // --- Sequence ---
    out.extend_from_slice(&fixed_name_sized::<32>("idle"));
    push_f32(&mut out, 10.0); // fps
    push_i32(&mut out, 0); // flags
    push_i32(&mut out, 0); // activity
    push_i32(&mut out, 0); // actweight
    push_u32(&mut out, 0); // num_events
    push_u32(&mut out, u32::try_from(anim_data_offset).unwrap()); // event_index
    push_u32(&mut out, 2); // num_frames
    push_i32(&mut out, 0); // num_pivots
    push_i32(&mut out, 0); // pivot_index
    push_i32(&mut out, 0); // motion_type
    push_i32(&mut out, 0); // motion_bone
    for _ in 0..3 {
        push_f32(&mut out, 0.0); // linear_movement
    }
    push_i32(&mut out, 0); // automove_pos_index
    push_i32(&mut out, 0); // automove_angle_index
    for _ in 0..6 {
        push_f32(&mut out, 0.0); // bbmin, bbmax
    }
    push_u32(&mut out, 1); // num_blends
    push_u32(&mut out, u32::try_from(anim_data_offset).unwrap()); // anim_index
    push_i32(&mut out, 0);
    push_i32(&mut out, 0); // blend_type[2]
    push_f32(&mut out, 0.0);
    push_f32(&mut out, 0.0); // blend_start[2]
    push_f32(&mut out, 0.0);
    push_f32(&mut out, 0.0); // blend_end[2]
    push_i32(&mut out, 0); // blend_parent
    push_i32(&mut out, 0); // seq_group
    push_i32(&mut out, 1); // entry_node
    push_i32(&mut out, 1); // exit_node
    push_i32(&mut out, 0); // node_flags
    push_i32(&mut out, 0); // next_seq
    assert_eq!(out.len(), body_parts_offset);

    // --- Body part ---
    out.extend_from_slice(&fixed_name_sized::<64>("body"));
    push_u32(&mut out, 1); // num_models
    push_i32(&mut out, 0); // base
    push_u32(&mut out, u32::try_from(models_offset).unwrap());
    assert_eq!(out.len(), models_offset);

    // --- Model ---
    out.extend_from_slice(&fixed_name_sized::<64>("model"));
    push_i32(&mut out, 0); // type
    push_f32(&mut out, 0.0); // bounding_radius
    push_u32(&mut out, 1); // num_mesh
    push_u32(&mut out, u32::try_from(meshes_offset).unwrap());
    push_u32(&mut out, 4); // num_verts
    push_u32(&mut out, u32::try_from(vert_bones_offset).unwrap());
    push_u32(&mut out, u32::try_from(verts_offset).unwrap());
    push_u32(&mut out, 1); // num_norms
    push_u32(&mut out, u32::try_from(norm_bones_offset).unwrap());
    push_u32(&mut out, u32::try_from(norms_offset).unwrap());
    push_i32(&mut out, 0); // num_groups
    push_i32(&mut out, 0); // group_index
    assert_eq!(out.len(), meshes_offset);

    // --- Mesh (a 4-trivert triangle strip: 2 triangles) ---
    // `num_tris` is not used by `Mdl::decode_mesh_commands` (see that
    // method's doc comment); the value here is a placeholder.
    push_i32(&mut out, 2);
    push_u32(&mut out, u32::try_from(tricommands_offset).unwrap());
    push_i32(&mut out, 0); // skin_ref
    push_i32(&mut out, 0); // num_norms (unused)
    push_u32(&mut out, 0); // norm_index (unused)
    assert_eq!(out.len(), verts_offset);

    // --- Verts: a unit quad ---
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        push_f32(&mut out, x);
        push_f32(&mut out, y);
        push_f32(&mut out, 0.0);
    }
    assert_eq!(out.len(), vert_bones_offset);
    out.extend_from_slice(&[0u8; 4]);
    assert_eq!(out.len(), norms_offset);

    // --- Norms ---
    push_f32(&mut out, 0.0);
    push_f32(&mut out, 0.0);
    push_f32(&mut out, 1.0);
    assert_eq!(out.len(), norm_bones_offset);
    out.push(0);
    assert_eq!(out.len(), tricommands_offset);

    // --- Trivert command stream: one strip of 4 (positive count), then
    // --- the terminating zero header.
    push_i16(&mut out, 4);
    for i in 0..4u16 {
        push_i16(&mut out, i.cast_signed()); // vert_index
        push_i16(&mut out, 0); // norm_index
        let (s, t) = [(0, 0), (16, 0), (16, 16), (0, 16)][i as usize];
        push_i16(&mut out, s);
        push_i16(&mut out, t);
    }
    push_i16(&mut out, 0); // terminator
    assert_eq!(out.len(), anim_data_offset);

    // --- Animation data (seqgroup 0, embedded) ---
    // Bone 0: slot 0 (X position) carries a real compressed run at relative
    // offset 24 (right after both 12-byte offset records); every other
    // slot is 0 (bind pose).
    push_u16(&mut out, 24);
    for _ in 0..5 {
        push_u16(&mut out, 0);
    }
    // Bone 1: every slot uses the bind pose.
    for _ in 0..6 {
        push_u16(&mut out, 0);
    }
    out.push(2); // valid
    out.push(2); // total
    push_i16(&mut out, 10);
    push_i16(&mut out, 20);
    assert_eq!(out.len(), total_len);

    let layout = MinimalMdl10Layout {
        header_size: HEADER_SIZE,
        bones_offset,
        bones_size,
        textures_offset,
        texture_data_offset,
        texture_data_size,
        skin_refs_offset,
        sequences_offset,
        body_parts_offset,
        models_offset,
        meshes_offset,
        verts_offset,
        vert_bones_offset,
        norms_offset,
        norm_bones_offset,
        tricommands_offset,
        anim_data_offset,
        total_len,
    };
    (out, layout)
}

/// Builds a minimal, well-formed, synthetic SPR file: an 8x8, 2-frame
/// sprite sharing the project's synthetic 256-color palette.
///
/// Every byte here is authored by this project for testing only; see
/// `docs/CLEAN_ROOM.md`.
#[must_use]
pub fn build_minimal_spr() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"IDSP");
    push_i32(&mut out, 2); // version
    push_i32(&mut out, 0); // type: VP_PARALLEL_UPRIGHT
    push_i32(&mut out, 0); // texture_format: SPR_NORMAL
    push_f32(&mut out, 5.656_854); // bounding_radius: sqrt(4^2 + 4^2)
    push_u32(&mut out, 8); // max_width
    push_u32(&mut out, 8); // max_height
    push_u32(&mut out, 2); // num_frames
    push_f32(&mut out, 0.0); // beam_length
    push_i32(&mut out, 0); // sync_type: synchronized

    let palette = synthetic_palette();
    push_u16(&mut out, u16::try_from(PALETTE_LEN).unwrap());
    for entry in &palette {
        out.push(entry.r);
        out.push(entry.g);
        out.push(entry.b);
    }

    for fill in [1u8, 2u8] {
        push_u32(&mut out, 0); // group
        push_i32(&mut out, -4); // origin_x
        push_i32(&mut out, -4); // origin_y
        push_u32(&mut out, 8); // width
        push_u32(&mut out, 8); // height
        out.extend(core::iter::repeat_n(fill, 64));
    }
    out
}

// ---------------------------------------------------------------------
// Collision-hull fixtures
// ---------------------------------------------------------------------
//
// A BSP file carries one BSP node tree (hull 0, used for point traces) plus
// three pre-expanded clip-hull trees built from `BSPCLIPNODE` records, one
// per player bounding-box size. The writers below generate all four trees
// from the same project-authored list of convex brushes, so collision tests
// have a fixture whose hulls agree with each other by construction.
//
// The hull sizes and the plane-expansion rule are the publicly documented
// ones recorded in `docs/FORMAT_SOURCES.md` under "Collision hulls and player
// movement"; every byte produced here is authored by this project for testing
// only (see `docs/CLEAN_ROOM.md`).

/// One outward-facing half-space. The *solid* side is `normal · x <= dist`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HullPlane {
    pub normal: [f32; 3],
    pub dist: f32,
}

impl HullPlane {
    #[must_use]
    pub const fn new(normal: [f32; 3], dist: f32) -> Self {
        Self { normal, dist }
    }
}

/// A convex solid: the intersection of the solid sides of its planes.
#[derive(Debug, Clone, Default)]
pub struct CollisionBrush {
    pub planes: Vec<HullPlane>,
}

impl CollisionBrush {
    /// A brush from an explicit list of half-spaces.
    #[must_use]
    pub fn new(planes: &[HullPlane]) -> Self {
        Self {
            planes: planes.to_vec(),
        }
    }

    /// A single unbounded half-space (`normal · x <= dist` is solid).
    #[must_use]
    pub fn half_space(normal: [f32; 3], dist: f32) -> Self {
        Self::new(&[HullPlane::new(normal, dist)])
    }

    /// An axis-aligned solid box spanning `mins..maxs`.
    #[must_use]
    pub fn box_brush(mins: [f32; 3], maxs: [f32; 3]) -> Self {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut positive = [0.0f32; 3];
            positive[axis] = 1.0;
            let mut negative = [0.0f32; 3];
            negative[axis] = -1.0;
            planes.push(HullPlane::new(positive, maxs[axis]));
            planes.push(HullPlane::new(negative, -mins[axis]));
        }
        Self { planes }
    }
}

/// The four documented GoldSrc hull bounding boxes, in hull-index order:
/// point, standing (32x32x72), large (64x64x64), and crouched (32x32x36).
pub const FIXTURE_HULL_SIZES: [([f32; 3], [f32; 3]); 4] = [
    ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]),
    ([-16.0, -16.0, -36.0], [16.0, 16.0, 36.0]),
    ([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]),
    ([-16.0, -16.0, -18.0], [16.0, 16.0, 18.0]),
];

/// Offsets `plane` outward so that a point trace through the expanded hull
/// is equivalent to sweeping the box `mins..maxs` through the original
/// solid (the documented hull-expansion rule: shift each plane by the
/// box's support distance along the plane normal).
fn expand_plane(plane: HullPlane, mins: [f32; 3], maxs: [f32; 3]) -> HullPlane {
    let mut support = 0.0f32;
    for axis in 0..3 {
        let n = plane.normal[axis];
        support += if n >= 0.0 {
            n * mins[axis]
        } else {
            n * maxs[axis]
        };
    }
    HullPlane::new(plane.normal, plane.dist - support)
}

/// Which tree kind [`Bsp30Builder::push_collision_hulls`] is emitting: the
/// BSP node tree (hull 0, whose children reference leaves) or a clipnode
/// tree (hulls 1-3, whose children reference contents values directly).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Nodes,
    Clipnodes,
}

/// `CONTENTS_EMPTY` / `CONTENTS_SOLID` / `CONTENTS_WATER` as encoded in a
/// clipnode child link.
const CHILD_EMPTY_CLIPNODE: i16 = -1;
const CHILD_SOLID_CLIPNODE: i16 = -2;
const CHILD_WATER_CLIPNODE: i16 = -3;
/// Leaf 1 (empty), leaf 0 (solid) and leaf 2 (water), encoded as
/// `!leaf_index`.
const CHILD_EMPTY_LEAF: i16 = -2;
const CHILD_SOLID_LEAF: i16 = -1;
const CHILD_WATER_LEAF: i16 = -3;

impl Bsp30Builder {
    fn plane_count(&self) -> usize {
        self.planes.len() / 20
    }

    /// Emits the tree deciding `brushes`: every point inside one of them
    /// links to `inside`, everything else to `outside`.
    fn emit_union(
        &mut self,
        brushes: &[CollisionBrush],
        mins: [f32; 3],
        maxs: [f32; 3],
        kind: TreeKind,
        inside: i16,
        outside: i16,
    ) -> i16 {
        let Some((first, rest_brushes)) = brushes.split_first() else {
            return outside;
        };
        // Everything on the front (outside) side of any of this brush's
        // planes is outside this brush, so it is decided by the union of the
        // remaining brushes; that subtree is built once and shared.
        let rest = self.emit_union(rest_brushes, mins, maxs, kind, inside, outside);
        let mut back = inside;
        for plane in first.planes.iter().rev() {
            let expanded = expand_plane(*plane, mins, maxs);
            let plane_index = self.plane_count();
            // `kind` 3 is the documented "any/non-axial" plane type, which is
            // always a valid classification for any normal.
            self.push_plane(expanded.normal, expanded.dist, 3);
            let index = match kind {
                TreeKind::Nodes => {
                    let index = self.nodes.len() / 24;
                    self.push_node(
                        u32::try_from(plane_index).unwrap(),
                        rest,
                        back,
                        [-4096; 3],
                        [4096; 3],
                        0,
                        0,
                    );
                    index
                }
                TreeKind::Clipnodes => {
                    let index = self.clipnodes.len() / 8;
                    self.push_clipnode(i32::try_from(plane_index).unwrap(), rest, back);
                    index
                }
            };
            back = i16::try_from(index).unwrap();
        }
        back
    }

    /// Emits the BSP node tree and all three clip-hull trees for `brushes`
    /// (the solid is their union), appending to the planes, nodes, leaves
    /// and clipnodes lumps, and returns the four head-node indices in the
    /// order `BSPMODEL::headnodes` stores them.
    ///
    /// Leaf 0 is the shared solid leaf, leaf 1 the empty leaf and leaf 2 a
    /// water leaf, matching the documented convention that the tree's `-1`
    /// child is solid.
    pub fn push_collision_hulls(&mut self, brushes: &[CollisionBrush]) -> [i32; 4] {
        self.push_collision_hulls_with_liquid(brushes, &[])
    }

    /// Like [`Self::push_collision_hulls`], but points inside `liquid` (and
    /// outside every solid brush) get `CONTENTS_WATER`.
    pub fn push_collision_hulls_with_liquid(
        &mut self,
        solid: &[CollisionBrush],
        liquid: &[CollisionBrush],
    ) -> [i32; 4] {
        if self.leaves.is_empty() {
            // Leaf 0: the shared solid leaf. Leaf 1: empty space. Leaf 2:
            // water.
            self.push_leaf(-2, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
            self.push_leaf(-1, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
            self.push_leaf(-3, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
        }
        let mut heads = [0i32; 4];
        for (hull, (mins, maxs)) in FIXTURE_HULL_SIZES.iter().enumerate() {
            let (kind, empty, solid_child, water) = if hull == 0 {
                (
                    TreeKind::Nodes,
                    CHILD_EMPTY_LEAF,
                    CHILD_SOLID_LEAF,
                    CHILD_WATER_LEAF,
                )
            } else {
                (
                    TreeKind::Clipnodes,
                    CHILD_EMPTY_CLIPNODE,
                    CHILD_SOLID_CLIPNODE,
                    CHILD_WATER_CLIPNODE,
                )
            };
            let liquid_tree = self.emit_union(liquid, *mins, *maxs, kind, water, empty);
            heads[hull] =
                i32::from(self.emit_union(solid, *mins, *maxs, kind, solid_child, liquid_tree));
        }
        heads
    }
}

/// The brush list of [`build_collision_room_bsp`], so tests can assert
/// against the same geometry the fixture was built from.
///
/// The room's interior spans `[-256, 256]` on X and Y and `[0, 256]` on Z,
/// with the floor's top surface at `z = 0`. It contains an 18-unit step
/// (`x` 64..192), a 19-unit ledge (`x` -192..-64), a walkable ramp whose
/// surface normal has `z = 0.8` (`y >= 128`), and a too-steep ramp whose
/// surface normal has `z = 0.5` (`y <= -128`).
#[must_use]
pub fn collision_room_brushes() -> Vec<CollisionBrush> {
    alloc::vec![
        // Floor, ceiling and the four walls, each an unbounded half-space.
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -256.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -256.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -256.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -256.0),
        // An 18-unit step and a 19-unit ledge.
        CollisionBrush::box_brush([64.0, -64.0, -16.0], [192.0, 64.0, 18.0]),
        CollisionBrush::box_brush([-192.0, -64.0, -16.0], [-64.0, 64.0, 19.0]),
        // A walkable ramp (surface normal z = 0.8) rising toward +Y.
        CollisionBrush::new(&[
            HullPlane::new([0.0, -0.6, 0.8], -76.8),
            HullPlane::new([0.0, -1.0, 0.0], -128.0),
        ]),
        // A too-steep ramp (surface normal z = 0.5) rising toward -Y.
        CollisionBrush::new(&[
            HullPlane::new([0.0, 0.866_025_4, 0.5], -110.851_25),
            HullPlane::new([0.0, 1.0, 0.0], -128.0),
        ]),
    ]
}

/// Builds a complete, project-authored BSP30 file whose collision hulls
/// describe [`collision_room_brushes`], with an `info_player_start` standing
/// on the floor at the origin.
#[must_use]
pub fn build_collision_room_bsp() -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n\
         {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 36\"\n\"angle\" \"0\"\n}\n",
    );
    let heads = builder.push_collision_hulls(&collision_room_brushes());
    builder.push_model(
        [-256.0, -256.0, 0.0],
        [256.0, 256.0, 256.0],
        [0.0, 0.0, 0.0],
        heads,
        2,
        0,
        0,
    );
    builder.build()
}

/// Builds a BSP30 file with a flat floor at `z = 0` and a pool of water
/// filling `0 < z <= 64`, so movement tests can enter and leave a liquid.
#[must_use]
pub fn build_collision_pool_bsp() -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads = builder.push_collision_hulls_with_liquid(
        &[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)],
        &[CollisionBrush::half_space([0.0, 0.0, 1.0], 64.0)],
    );
    builder.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        heads,
        3,
        0,
        0,
    );
    builder.build()
}

/// The top surface of [`build_brush_entity_floor_bsp`]'s slab.
pub const BRUSH_FLOOR_TOP_Z: f32 = 0.0;

/// The half-extent of [`build_brush_entity_floor_bsp`]'s slab on X and Y:
/// the slab spans `[-128, 128]` on both, so a player standing at the origin
/// is well inside it and one at `x = 512` is off it entirely.
pub const BRUSH_FLOOR_HALF_EXTENT: f32 = 128.0;

/// Builds a BSP30 file whose only floor is a *brush entity*: the worldspawn
/// model (submodel 0) is completely empty — an open void with nothing solid
/// in it — and submodel 1 is a slab whose top surface is at
/// [`BRUSH_FLOOR_TOP_Z`], referenced as `"*1"` by an entity with the
/// caller-chosen `classname` (`"func_wall"` for a solid one).
///
/// Maps routinely build a floor this way, and the worldspawn hulls alone
/// therefore do not describe what the player stands on: anything tracing
/// only submodel 0 sees a bottomless void here.
#[must_use]
pub fn build_brush_entity_floor_bsp(classname: &str) -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(&alloc::format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\
         \"angle\" \"0\"\n}}\n\
         {{\n\"classname\" \"{classname}\"\n\"model\" \"*1\"\n}}\n"
    ));
    // Submodel 0: no solids at all.
    let world_heads = builder.push_collision_hulls(&[]);
    // Submodel 1: the floor slab, 16 units thick under `BRUSH_FLOOR_TOP_Z`.
    let slab_heads = builder.push_collision_hulls(&[CollisionBrush::box_brush(
        [-BRUSH_FLOOR_HALF_EXTENT, -BRUSH_FLOOR_HALF_EXTENT, -16.0],
        [
            BRUSH_FLOOR_HALF_EXTENT,
            BRUSH_FLOOR_HALF_EXTENT,
            BRUSH_FLOOR_TOP_Z,
        ],
    )]);
    builder.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        world_heads,
        2,
        0,
        0,
    );
    builder.push_model(
        [-BRUSH_FLOOR_HALF_EXTENT, -BRUSH_FLOOR_HALF_EXTENT, -16.0],
        [
            BRUSH_FLOOR_HALF_EXTENT,
            BRUSH_FLOOR_HALF_EXTENT,
            BRUSH_FLOOR_TOP_Z,
        ],
        [0.0, 0.0, 0.0],
        slab_heads,
        2,
        0,
        0,
    );
    builder.build()
}

/// Builds a BSP30 file whose only solid is a single unbounded ground plane
/// with the given unit surface normal (`0, 1` is flat ground), passing
/// through the world origin.
///
/// Used to check the walkable-slope limit from both sides.
#[must_use]
pub fn build_collision_slope_bsp(normal_y: f32, normal_z: f32) -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads =
        builder.push_collision_hulls(&[CollisionBrush::half_space([0.0, normal_y, normal_z], 0.0)]);
    builder.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        heads,
        2,
        0,
        0,
    );
    builder.build()
}

// ---------------------------------------------------------------------
// PAK archive fixtures
// ---------------------------------------------------------------------

/// Encodes a name into a fixed, NUL-padded 56-byte PAK entry name field.
#[must_use]
pub fn fixed_pak_name(name: &str) -> [u8; 56] {
    fixed_name_sized::<56>(name)
}

/// A synthetic PAK archive builder.
#[derive(Default)]
pub struct PakBuilder {
    entries: Vec<([u8; 56], Vec<u8>)>,
}

impl PakBuilder {
    /// A builder with no entries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a member file with `name` (encoded into the fixed 56-byte,
    /// NUL-padded name field) and `body` bytes.
    pub fn add_entry(&mut self, name: &str, body: Vec<u8>) {
        self.entries.push((fixed_pak_name(name), body));
    }

    /// Adds an entry whose 56-byte name field is supplied verbatim
    /// (unpadded or deliberately un-terminated), for malformed-fixture
    /// tests.
    pub fn add_raw_named_entry(&mut self, name: [u8; 56], body: Vec<u8>) {
        self.entries.push((name, body));
    }

    /// Serializes the whole file: header, entry bodies, then the
    /// directory.
    #[must_use]
    pub fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"PACK");
        // Placeholders for `diroffset`/`dirsize`, patched below.
        let dir_offset_pos = out.len();
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);

        let mut directory = Vec::new();
        for (name, body) in &self.entries {
            let offset = u32::try_from(out.len()).unwrap();
            out.extend_from_slice(body);
            directory.extend_from_slice(name);
            push_u32(&mut directory, offset);
            push_u32(&mut directory, u32::try_from(body.len()).unwrap());
        }

        let dir_offset = u32::try_from(out.len()).unwrap();
        let dir_size = u32::try_from(directory.len()).unwrap();
        out[dir_offset_pos..dir_offset_pos + 4].copy_from_slice(&dir_offset.to_le_bytes());
        out[dir_offset_pos + 4..dir_offset_pos + 8].copy_from_slice(&dir_size.to_le_bytes());
        out.extend_from_slice(&directory);
        out
    }
}
