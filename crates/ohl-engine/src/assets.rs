//! Where a level's bytes come from.

use std::io::Read as _;

use ohl_assets::AssetFs;

/// The read-only, game-relative asset lookup a [`crate::Game`] needs.
///
/// Paths are GoldSrc-style, forward-slashed and mod-relative (for example
/// `maps/foo.bsp`), exactly as [`ohl_assets::AssetFs`] resolves them. Every
/// method returns `None`/empty rather than an error: a missing asset is an
/// ordinary condition (an incomplete payload, an optional model) that the
/// caller degrades over.
pub trait AssetSource {
    /// Reads one asset in full, or `None` when it is not published.
    fn read(&self, asset_path: &str) -> Option<Vec<u8>>;

    /// Resolves a worldspawn `wad` value to the texture packages it names,
    /// matching by basename the way GoldSrc does.
    ///
    /// The default implementation resolves nothing, which leaves externally
    /// stored textures on their placeholder.
    fn resolve_wads(&self, worldspawn_wad_value: &str) -> Vec<Vec<u8>> {
        let _ = worldspawn_wad_value;
        Vec::new()
    }
}

/// An [`AssetSource`] over an imported payload's `files/` directory.
pub struct AssetFsSource {
    fs: AssetFs,
}

impl AssetFsSource {
    /// Wraps an already-mounted asset filesystem.
    #[must_use]
    pub fn new(fs: AssetFs) -> Self {
        Self { fs }
    }

    /// The wrapped filesystem, for callers that need it directly.
    #[must_use]
    pub fn asset_fs(&self) -> &AssetFs {
        &self.fs
    }
}

impl AssetSource for AssetFsSource {
    fn read(&self, asset_path: &str) -> Option<Vec<u8>> {
        let mut file = self.fs.open(asset_path).ok()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }

    fn resolve_wads(&self, worldspawn_wad_value: &str) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for mut wad in self.fs.resolve_wads(worldspawn_wad_value) {
            let mut bytes = Vec::new();
            if wad.read_to_end(&mut bytes).is_ok() {
                out.push(bytes);
            }
        }
        out
    }
}

/// An [`AssetSource`] backed by an in-memory table, for tests and hosts that
/// have already staged the bytes themselves.
#[derive(Debug, Default, Clone)]
pub struct MemoryAssets {
    entries: std::collections::BTreeMap<String, Vec<u8>>,
}

impl MemoryAssets {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts (or replaces) one asset.
    pub fn insert(&mut self, asset_path: &str, bytes: Vec<u8>) {
        self.entries.insert(asset_path.to_ascii_lowercase(), bytes);
    }
}

impl AssetSource for MemoryAssets {
    fn read(&self, asset_path: &str) -> Option<Vec<u8>> {
        self.entries.get(&asset_path.to_ascii_lowercase()).cloned()
    }
}
