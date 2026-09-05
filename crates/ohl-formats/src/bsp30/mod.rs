//! GoldSrc BSP v30 map decoding.
//!
//! See `docs/FORMAT_SOURCES.md` ("GoldSrc BSP v30 and WAD3") for the public
//! documentation this module was implemented from. [`Bsp`] is a borrowing,
//! zero-copy view over a whole BSP file: every lump offset/length is
//! validated against the file at construction time, every per-record count
//! is derived from `lump length / size_of::<Record>()` with any remainder
//! rejected, and every index used to cross-reference another lump is
//! bounds-checked at the point of use rather than trusted.

mod entities;
mod limits;
mod raw;
mod textures;
mod visibility;
pub mod walk;

use alloc::vec::Vec;

pub use entities::{Entity, parse as parse_entities};
pub use limits::Limits;
pub use raw::{
    Clipnode, Edge, Face, LUMP_COUNT, Leaf, LumpId, Marksurface, Model, Node, Plane, Surfedge,
    TexInfo, VERSION, Vertex,
};
pub use textures::{Miptex, TextureDirectory};

use crate::error::{FormatError, Result};
use crate::palette::Rgb8;
use crate::util::{prefix_of, slice_of, sub_slice};
use raw::{RawHeader, RawLumpDir};

/// A validated, zero-copy view over one BSP v30 file.
pub struct Bsp<'a> {
    data: &'a [u8],
    lumps: [RawLumpDir; LUMP_COUNT],
}

fn lump_bytes<'a>(data: &'a [u8], dir: RawLumpDir, limits: &Limits) -> Result<&'a [u8]> {
    let offset = dir.offset.get() as usize;
    let length = dir.length.get();
    if length > limits.max_lump_bytes {
        return Err(FormatError::LimitExceeded);
    }
    sub_slice(data, offset, length as usize)
}

impl<'a> Bsp<'a> {
    /// Parses and validates a BSP v30 file's header and lump directory.
    ///
    /// This only validates the header, version, and that every lump's
    /// `{offset, length}` falls within `data`; it does not decode any lump's
    /// contents (that happens lazily, per accessor, so a caller that only
    /// needs a few lumps never pays for the rest).
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self> {
        let (header, _): (&RawHeader, _) = prefix_of(data)?;
        if header.version.get() != VERSION {
            return Err(FormatError::BadSignature);
        }
        let lumps = header.lumps;
        for dir in lumps {
            // Validate now so every later accessor can assume the range is
            // in-bounds; store the (already-validated) directory rather than
            // re-deriving it.
            let _ = lump_bytes(data, dir, limits)?;
        }
        Ok(Self { data, lumps })
    }

    fn lump(&self, id: LumpId, limits: &Limits) -> Result<&'a [u8]> {
        lump_bytes(self.data, self.lumps[id as usize], limits)
    }

    /// The raw lump bytes for `id`, without further interpretation. Useful
    /// for callers that only need offsets (e.g. the visibility lump).
    pub fn raw_lump(&self, id: LumpId, limits: &Limits) -> Result<&'a [u8]> {
        self.lump(id, limits)
    }

    /// Parses the entities lump into an ordered list of key/value maps.
    pub fn entities(&self, limits: &Limits) -> Result<Vec<Entity>> {
        entities::parse(self.lump(LumpId::Entities, limits)?, limits)
    }

    /// The planes lump.
    pub fn planes(&self, limits: &Limits) -> Result<&'a [Plane]> {
        slice_of(self.lump(LumpId::Planes, limits)?)
    }

    /// The vertices lump.
    pub fn vertices(&self, limits: &Limits) -> Result<&'a [Vertex]> {
        slice_of(self.lump(LumpId::Vertexes, limits)?)
    }

    /// The BSP tree nodes lump.
    pub fn nodes(&self, limits: &Limits) -> Result<&'a [Node]> {
        slice_of(self.lump(LumpId::Nodes, limits)?)
    }

    /// The texture-info lump.
    pub fn texinfo(&self, limits: &Limits) -> Result<&'a [TexInfo]> {
        slice_of(self.lump(LumpId::TexInfo, limits)?)
    }

    /// The faces lump.
    pub fn faces(&self, limits: &Limits) -> Result<&'a [Face]> {
        slice_of(self.lump(LumpId::Faces, limits)?)
    }

    /// The lighting lump, as RGB byte triples (GoldSrc's colored lightmaps).
    pub fn lighting(&self, limits: &Limits) -> Result<&'a [Rgb8]> {
        slice_of(self.lump(LumpId::Lighting, limits)?)
    }

    /// The clip-hull nodes lump.
    pub fn clipnodes(&self, limits: &Limits) -> Result<&'a [Clipnode]> {
        slice_of(self.lump(LumpId::Clipnodes, limits)?)
    }

    /// The leaves lump.
    pub fn leaves(&self, limits: &Limits) -> Result<&'a [Leaf]> {
        slice_of(self.lump(LumpId::Leaves, limits)?)
    }

    /// The mark-surfaces lump.
    pub fn marksurfaces(&self, limits: &Limits) -> Result<&'a [Marksurface]> {
        slice_of(self.lump(LumpId::Marksurfaces, limits)?)
    }

    /// The edges lump.
    pub fn edges(&self, limits: &Limits) -> Result<&'a [Edge]> {
        slice_of(self.lump(LumpId::Edges, limits)?)
    }

    /// The signed surfedges lump.
    pub fn surfedges(&self, limits: &Limits) -> Result<&'a [Surfedge]> {
        slice_of(self.lump(LumpId::Surfedges, limits)?)
    }

    /// The submodels lump.
    pub fn models(&self, limits: &Limits) -> Result<&'a [Model]> {
        slice_of(self.lump(LumpId::Models, limits)?)
    }

    /// The texture directory (`numtex` plus its offset table); decode
    /// individual textures via [`TextureDirectory::get`].
    pub fn textures(&self, limits: &Limits) -> Result<TextureDirectory<'a>> {
        textures::parse_directory(self.lump(LumpId::Textures, limits)?, limits)
    }

    /// Looks up the lightmap RGB sample at byte offset `lightmap_offset`
    /// (as stored in [`Face::lightmap_offset`]), reading `sample_count`
    /// consecutive `Rgb8` samples.
    pub fn lightmap_samples(
        &self,
        lightmap_offset: i32,
        sample_count: usize,
        limits: &Limits,
    ) -> Result<&'a [Rgb8]> {
        if lightmap_offset < 0 {
            return Err(FormatError::IndexOutOfRange);
        }
        let lump = self.lump(LumpId::Lighting, limits)?;
        // Already checked non-negative above.
        let byte_offset = usize::try_from(lightmap_offset).map_err(|_| FormatError::OutOfBounds)?;
        let byte_len = sample_count
            .checked_mul(3)
            .ok_or(FormatError::OutOfBounds)?;
        let bytes = sub_slice(lump, byte_offset, byte_len)?;
        slice_of(bytes)
    }

    /// Decodes whether leaf-visibility bit `bit_index` is set for the PVS
    /// list beginning at leaf's `vis_offset`. A negative `vis_offset` (no
    /// compressed list, e.g. the shared outside leaf) is always visible.
    pub fn is_leaf_visible(
        &self,
        leaf: &Leaf,
        bit_index: usize,
        leaf_count: usize,
        limits: &Limits,
    ) -> Result<bool> {
        if leaf.vis_offset.get() < 0 {
            return Ok(true);
        }
        let vis = self.lump(LumpId::Visibility, limits)?;
        let decompressed_len_bytes = leaf_count.div_ceil(8);
        // Already checked non-negative above.
        let start = usize::try_from(leaf.vis_offset.get()).map_err(|_| FormatError::OutOfBounds)?;
        visibility::is_visible(vis, start, bit_index, decompressed_len_bytes)
    }

    /// Finds the leaf index containing `point`, walking from `head_node`
    /// (typically `Model::headnodes[0]`).
    pub fn find_leaf(&self, head_node: i32, point: [f32; 3], limits: &Limits) -> Result<usize> {
        let nodes = self.nodes(limits)?;
        let planes = self.planes(limits)?;
        walk::find_leaf(nodes, planes, head_node, point, limits.max_walk_depth)
    }
}
