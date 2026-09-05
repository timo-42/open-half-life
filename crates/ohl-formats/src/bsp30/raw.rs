//! Zero-copy, little-endian struct layouts for BSP v30, as documented in
//! `docs/FORMAT_SOURCES.md` under "GoldSrc BSP v30 and WAD3" (BSP29 lump
//! order and directory shape from the Unofficial Quake Specs section 4;
//! GoldSrc-specific struct fields from the Valve Developer Community
//! "BSP (GoldSrc)" article).

use zerocopy::byteorder::little_endian::{F32, I16, I32, U16, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// BSP v30's fixed version field value.
pub const VERSION: i32 = 30;

/// The number of lumps in the directory (fixed by the format).
pub const LUMP_COUNT: usize = 15;

/// Lump indices, in on-disk directory order (Unofficial Quake Specs section
/// 4, `dheader_t`; the same order is retained by GoldSrc BSP v30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum LumpId {
    Entities = 0,
    Planes = 1,
    Textures = 2,
    Vertexes = 3,
    Visibility = 4,
    Nodes = 5,
    TexInfo = 6,
    Faces = 7,
    Lighting = 8,
    Clipnodes = 9,
    Leaves = 10,
    Marksurfaces = 11,
    Edges = 12,
    Surfedges = 13,
    Models = 14,
}

/// One `{offset, length}` directory entry.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawLumpDir {
    pub offset: U32,
    pub length: U32,
}

/// The fixed 4-byte version field followed by 15 directory entries.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct RawHeader {
    pub version: I32,
    pub lumps: [RawLumpDir; LUMP_COUNT],
}

/// A map plane (`plane_t` in the Quake spec).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Plane {
    pub normal: [F32; 3],
    pub dist: F32,
    pub kind: I32,
}

/// A world-space vertex.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Vertex {
    pub point: [F32; 3],
}

/// A BSP tree node (`BSPNODE`, GoldSrc fields).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Node {
    pub plane: U32,
    pub children: [I16; 2],
    pub mins: [I16; 3],
    pub maxs: [I16; 3],
    pub first_face: U16,
    pub num_faces: U16,
}

/// A collision-hull node (`BSPCLIPNODE`).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Clipnode {
    pub plane: I32,
    pub children: [I16; 2],
}

/// A BSP leaf (`BSPLEAF`, GoldSrc fields).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Leaf {
    pub contents: I32,
    pub vis_offset: I32,
    pub mins: [I16; 3],
    pub maxs: [I16; 3],
    pub first_marksurface: U16,
    pub num_marksurfaces: U16,
    pub ambient_levels: [u8; 4],
}

/// Texture-mapping axes for one face (`BSPTEXTUREINFO`).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct TexInfo {
    pub s_vector: [F32; 3],
    pub s_shift: F32,
    pub t_vector: [F32; 3],
    pub t_shift: F32,
    pub miptex_index: U32,
    pub flags: U32,
}

/// A polygon face (`BSPFACE`, GoldSrc fields).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Face {
    pub plane: U16,
    pub plane_side: U16,
    pub first_edge: U32,
    pub num_edges: U16,
    pub texinfo: U16,
    pub styles: [u8; 4],
    pub lightmap_offset: I32,
}

/// One directed edge between two vertices.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Edge {
    pub vertices: [U16; 2],
}

/// A signed reference into the edge table; negative means traverse the edge
/// in reverse (Unofficial Quake Specs section 4, `ledges`).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Surfedge(pub I32);

/// A face index referenced from a leaf's mark-surface run.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Marksurface(pub U16);

/// One submodel (`BSPMODEL`).
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
pub struct Model {
    pub mins: [F32; 3],
    pub maxs: [F32; 3],
    pub origin: [F32; 3],
    pub headnodes: [I32; 4],
    pub vis_leafs: I32,
    pub first_face: I32,
    pub num_faces: I32,
}
