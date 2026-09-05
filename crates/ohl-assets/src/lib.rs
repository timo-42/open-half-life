//! A GoldSrc-style asset filesystem: one uniform surface that resolves a
//! game-relative resource path (`maps/<name>.bsp`, `sprites/…`, `models/…`,
//! `sound/…`, `gfx/…`) to bytes, over an imported Half-Life payload tree
//! that mixes loose files and Quake `PACK` archives.
//!
//! [`AssetFs::mount`] (constructing the filesystem) walks a list of mod
//! directories in priority order (GoldSrc's own search-path order: the mod
//! the user is actually playing first, `valve` last as the shared base
//! content) under a payload root's `files/` directory, and for each one:
//!
//! 1. discovers `pak0.pak`, `pak1.pak`, ... (a contiguous ascending run) and
//!    any other `*.pak` file (sorted), and reads just each one's header and
//!    directory bytes — never the whole archive — to learn its entries;
//! 2. walks every loose file under the mod directory, bounded by
//!    [`Limits::max_depth`] and [`Limits::max_indexed_files`];
//! 3. merges everything into one case-insensitive index under a strict,
//!    deterministic precedence: a loose file always beats a PAK entry of
//!    the same name, an earlier PAK beats a later one, and an earlier mod
//!    directory beats a later one — matching the order the search path list
//!    was given in.
//!
//! [`AssetFs::open`] then resolves a game-relative path against that index
//! and returns a [`file::AssetFile`] — a `Read + Seek` handle over either a
//! whole loose file or a bounded byte range inside a PAK archive — and
//! [`AssetFs::resolve_wads`] resolves a worldspawn `wad` key's
//! semicolon-separated, mapper-authored absolute paths by basename only,
//! exactly as GoldSrc does.
//!
//! # Logging policy
//!
//! No path this crate resolves — the caller's requested asset path, a
//! worldspawn `wad` value, a loose filesystem path, or a PAK member name —
//! is ever formatted into a log line or an [`AssetError`]. Every error this
//! crate returns is one of [`AssetError`]'s fixed, content-free variants.

mod error;
mod file;
mod index;
mod limits;
mod path;

use std::fs::File;
use std::path::Path;

pub use error::{AssetError, Result};
pub use file::AssetFile;
pub use limits::Limits;

use index::{Index, Location};

/// The default mod search path used when a caller does not configure one:
/// the base Half-Life content directory.
pub const DEFAULT_SEARCH_PATHS: &[&str] = &["valve"];

/// A resolved, read-only asset filesystem over an imported payload tree.
///
/// Built once via [`AssetFs::mount`]; every lookup afterwards is served from
/// the in-memory index built at that time; a payload tree that changes on
/// disk after `open` is not observed until the next `open`.
pub struct AssetFs {
    index: Index,
    limits: Limits,
}

impl AssetFs {
    /// Builds an [`AssetFs`] over `files_dir` (the payload root's `files/`
    /// directory) using `search_paths` as the mod directory priority order
    /// (for example `["valve_addon", "valve"]`; typically just
    /// [`DEFAULT_SEARCH_PATHS`]).
    ///
    /// A search path that does not exist under `files_dir` is silently
    /// skipped (not every optional mod directory need be present); a
    /// search path string that fails the game-relative path policy, or a
    /// PAK archive that fails to parse, is reported as an error.
    pub fn mount(files_dir: &Path, search_paths: &[String], limits: Limits) -> Result<Self> {
        let index = index::build(files_dir, search_paths, &limits)?;
        Ok(Self { index, limits })
    }

    /// Builds an [`AssetFs`] using [`DEFAULT_SEARCH_PATHS`] and
    /// [`Limits::default`].
    pub fn mount_default(files_dir: &Path) -> Result<Self> {
        let search_paths = DEFAULT_SEARCH_PATHS
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>();
        Self::mount(files_dir, &search_paths, Limits::default())
    }

    /// Whether `asset_path` resolves to an indexed loose file or PAK entry.
    #[must_use]
    pub fn exists(&self, asset_path: &str) -> bool {
        path::normalize(asset_path, &self.limits)
            .is_ok_and(|normalized| self.index.contains_key(normalized.key()))
    }

    /// Opens `asset_path` for reading, resolving it case-insensitively
    /// against every configured search path.
    pub fn open(&self, asset_path: &str) -> Result<AssetFile> {
        let normalized = path::normalize(asset_path, &self.limits)?;
        let indexed = self
            .index
            .get(normalized.key())
            .ok_or(AssetError::NotFound)?;
        Self::open_location(&indexed.location)
    }

    /// Lists every indexed asset whose path starts with `prefix` (a
    /// directory-style game-relative path; the empty string lists
    /// everything), bounded by [`Limits::max_list_results`].
    ///
    /// Returns each match's normalized display path (original casing,
    /// forward slashes), sorted.
    pub fn list_dir(&self, prefix: &str) -> Result<Vec<String>> {
        let key_prefix = if prefix.is_empty() {
            String::new()
        } else {
            path::normalize(prefix, &self.limits)?.key().to_string()
        };
        let mut results = Vec::new();
        for (key, indexed) in &self.index {
            let matches = if key_prefix.is_empty() {
                true
            } else {
                key.strip_prefix(key_prefix.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            };
            if !matches {
                continue;
            }
            if results.len() >= self.limits.max_list_results {
                return Err(AssetError::LimitExceeded);
            }
            results.push(indexed.display.clone());
        }
        results.sort();
        Ok(results)
    }

    /// Resolves a worldspawn `wad` key's semicolon-separated, mapper-
    /// authored absolute paths (for example
    /// `\quake\hlwad\halflife.wad;\quake\hlwad\liquids.wad;`) to whichever
    /// of them this filesystem actually has, matched by basename only —
    /// the mapper's own directories are always ignored, exactly as GoldSrc
    /// resolves a map's `wad` key against the engine's own search paths.
    ///
    /// An entry that fails the path policy or is not found is silently
    /// skipped, since a map commonly names more WADs than a given
    /// installation actually ships.
    #[must_use]
    pub fn resolve_wads(&self, worldspawn_wad_value: &str) -> Vec<AssetFile> {
        worldspawn_wad_value
            .split(';')
            .filter_map(|raw| {
                let normalized = path::basename(raw, &self.limits).ok()?;
                let indexed = self.index.get(normalized.key())?;
                Self::open_location(&indexed.location).ok()
            })
            .collect()
    }

    fn open_location(location: &Location) -> Result<AssetFile> {
        match location {
            Location::Loose(path) => {
                let file = File::open(path).map_err(|_| AssetError::Io)?;
                Ok(AssetFile::loose(file))
            }
            Location::Pak {
                archive,
                offset,
                size,
            } => {
                let file = File::open(archive).map_err(|_| AssetError::Io)?;
                Ok(AssetFile::pak_entry(file, *offset, *size))
            }
        }
    }
}
