//! Bounds this crate enforces while decoding a WAD3 file.

/// Configurable ceilings for WAD3 decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest number of directory entries a WAD3 file may declare.
    pub max_entries: usize,
    /// The largest a single miptex entry's declared `full_size` may be.
    pub max_entry_bytes: u32,
}

impl Limits {
    /// Conservative defaults.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_entries: 65_536,
            max_entry_bytes: 32 * 1024 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}
