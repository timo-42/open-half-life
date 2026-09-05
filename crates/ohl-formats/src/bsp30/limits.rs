//! Bounds this crate enforces while decoding a BSP30 file, so a malformed or
//! adversarial file cannot force unbounded allocation or iteration.

/// Configurable ceilings for BSP30 decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest a single lump may be, in bytes.
    pub max_lump_bytes: u32,
    /// The largest number of entity blocks the entities lump may parse into.
    pub max_entities: usize,
    /// The largest number of texture-directory slots the textures lump may
    /// declare.
    pub max_textures: usize,
    /// The largest byte length of the whole entities lump text this crate
    /// will parse.
    pub max_entities_bytes: usize,
    /// The largest byte length of a single key or value string.
    pub max_entity_string_bytes: usize,
    /// The deepest the node tree may be walked by [`crate::bsp30::walk`]
    /// before it is treated as a cycle.
    pub max_walk_depth: u32,
}

impl Limits {
    /// Conservative defaults, generous enough for real GoldSrc maps but far
    /// below what would let a malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_lump_bytes: 64 * 1024 * 1024,
            max_entities: 16_384,
            max_textures: 8_192,
            max_entities_bytes: 4 * 1024 * 1024,
            max_entity_string_bytes: 4_096,
            max_walk_depth: 256,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}
