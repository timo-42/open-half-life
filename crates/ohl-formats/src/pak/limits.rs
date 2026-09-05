//! Bounds this crate enforces while decoding a PAK directory.

/// Configurable ceilings for PAK directory decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest number of directory entries a PAK file may declare.
    pub max_entries: usize,
    /// The largest a single entry's declared `size` may be.
    pub max_entry_bytes: u32,
}

impl Limits {
    /// Conservative defaults: generous enough for real GoldSrc PAKs (which
    /// hold thousands of small files and a handful of large audio/texture
    /// entries) while still bounding a hostile directory.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_entries: 65_536,
            max_entry_bytes: 256 * 1024 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}
