//! Caller-supplied extraction ceilings.

/// Default staging buffer for one expanded compressed chunk.
pub const DEFAULT_CHUNK_BYTES: usize = 64 * 1024;

/// Ceilings applied to every extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest expanded size accepted for one file.
    pub max_expanded_bytes_per_file: u64,
    /// Largest total expanded size accepted across one reader's lifetime.
    pub max_total_expanded_bytes: u64,
    /// Largest accepted volume number.
    pub max_volumes: u16,
    /// Largest number of volume changes accepted while reading one file.
    pub max_volume_hops: u16,
    /// Largest number of split-link hops accepted while resolving a file.
    pub max_link_steps: u32,
    /// Staging buffer size, and the ceiling on one expanded chunk.
    pub max_chunk_bytes: usize,
}

impl Limits {
    /// The default ceilings, also returned by [`Default::default`].
    pub const DEFAULT: Self = Self {
        max_expanded_bytes_per_file: 4 * 1024 * 1024 * 1024,
        max_total_expanded_bytes: 64 * 1024 * 1024 * 1024,
        max_volumes: 1_024,
        max_volume_hops: 1_024,
        max_link_steps: 1_024,
        max_chunk_bytes: DEFAULT_CHUNK_BYTES,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
