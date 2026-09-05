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
