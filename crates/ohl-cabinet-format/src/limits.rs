//! Caller-supplied anti-abuse ceilings.

/// Ceilings applied to every count and length read out of a cabinet header.
///
/// The defaults are generous enough for real InstallShield 5/6/2003 media
/// while keeping worst-case allocation bounded for hostile input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest accepted header buffer.
    pub max_header_bytes: usize,
    /// Largest accepted file count.
    pub max_files: u32,
    /// Largest accepted directory count.
    pub max_directories: u32,
    /// Largest accepted number of file groups across all offset-list slots.
    pub max_file_groups: u32,
    /// Largest accepted number of components across all offset-list slots.
    pub max_components: u32,
    /// Largest accepted decoded name, in bytes of the encoded form.
    pub max_name_bytes: usize,
    /// Largest accepted volume number, and the ceiling on volume hops.
    pub max_volumes: u16,
}

impl Limits {
    /// The default ceilings, also returned by [`Default::default`].
    pub const DEFAULT: Self = Self {
        max_header_bytes: 64 * 1024 * 1024,
        max_files: 200_000,
        max_directories: 50_000,
        max_file_groups: 4_096,
        max_components: 4_096,
        max_name_bytes: 4_096,
        max_volumes: 1_024,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
