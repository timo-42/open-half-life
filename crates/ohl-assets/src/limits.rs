//! Bounds this crate enforces while indexing, resolving, and listing
//! assets.

/// Configurable ceilings for [`crate::AssetFs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest total number of indexed files (loose plus PAK entries,
    /// across every configured search path) an [`crate::AssetFs`] will
    /// build an index for.
    pub max_indexed_files: usize,
    /// The largest directory nesting depth walked under a search path's
    /// loose files.
    pub max_depth: usize,
    /// The largest byte length of a whole game-relative path.
    pub max_path_bytes: usize,
    /// The largest byte length of a single path component.
    pub max_component_bytes: usize,
    /// The largest number of components a game-relative path may have.
    pub max_components: usize,
    /// The largest number of entries [`crate::AssetFs::list_dir`] returns.
    pub max_list_results: usize,
    /// PAK directory decoding limits, passed straight to
    /// [`ohl_formats::pak::Limits`].
    pub pak: ohl_formats::pak::Limits,
}

impl Limits {
    /// Conservative defaults.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_indexed_files: 262_144,
            max_depth: 32,
            max_path_bytes: 4096,
            max_component_bytes: 255,
            max_components: 32,
            max_list_results: 8192,
            pak: ohl_formats::pak::Limits::conservative(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}
