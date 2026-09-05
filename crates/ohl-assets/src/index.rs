//! Building the case-insensitive asset index: a single bounded walk over a
//! search path's loose files and PAK archives.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use ohl_formats::pak::{self, Directory, HEADER_LEN};

use crate::error::{AssetError, Result};
use crate::limits::Limits;
use crate::path::normalize;

/// Where one indexed asset's bytes actually live.
#[derive(Debug, Clone)]
pub(crate) enum Location {
    Loose(PathBuf),
    Pak {
        archive: PathBuf,
        offset: u64,
        size: u64,
    },
}

/// One indexed asset: its original-case display path and where its bytes
/// live.
pub(crate) struct Indexed {
    pub display: String,
    pub location: Location,
}

/// The whole index: a case-insensitive key to the entry that won under the
/// "loose beats PAK, earlier mod dir beats later, earlier PAK beats later"
/// precedence rules.
pub(crate) type Index = BTreeMap<String, Indexed>;

struct Budget {
    remaining_files: usize,
}

impl Budget {
    fn take(&mut self) -> Result<()> {
        self.remaining_files = self
            .remaining_files
            .checked_sub(1)
            .ok_or(AssetError::LimitExceeded)?;
        Ok(())
    }
}

/// Builds the index for one mod directory (a single [`crate::AssetFs`]
/// search path) and merges it into `index`, without overwriting any key
/// already present (first-wins precedence across search paths).
fn index_search_path(
    index: &mut Index,
    mod_root: &Path,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<()> {
    let Ok(top_level) = std::fs::read_dir(mod_root) else {
        // A configured search path that does not exist is simply skipped:
        // not every mod directory (e.g. an optional expansion) need exist.
        return Ok(());
    };

    let mut pak_names: Vec<String> = Vec::new();
    for entry in top_level {
        let Ok(entry) = entry else { continue };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.to_ascii_lowercase().ends_with(".pak") {
            pak_names.push(name);
        }
    }

    // Loose files take precedence over PAK entries of the same name, so
    // they must be indexed first (the first-wins insert below is a no-op
    // for anything already indexed).
    index_loose_files(index, mod_root, mod_root, 0, limits, budget)?;

    for archive_name in ordered_pak_names(pak_names) {
        let archive_path = mod_root.join(&archive_name);
        index_pak(index, &archive_path, limits, budget)?;
    }

    Ok(())
}

/// Orders discovered `*.pak` filenames as GoldSrc does: a contiguous
/// ascending `pak0.pak`, `pak1.pak`, ... run first (stopping at the first
/// missing number), then every other `*.pak` name, sorted
/// case-insensitively.
fn ordered_pak_names(mut names: Vec<String>) -> Vec<String> {
    let mut numbered: BTreeMap<u32, String> = BTreeMap::new();
    let mut rest: Vec<String> = Vec::new();
    for name in names.drain(..) {
        if let Some(n) = numbered_pak_index(&name) {
            numbered.insert(n, name);
        } else {
            rest.push(name);
        }
    }

    let mut ordered = Vec::new();
    let mut next = 0u32;
    while let Some(name) = numbered.remove(&next) {
        ordered.push(name);
        next += 1;
    }
    // Numbered files whose sequence had a gap are still loaded, just not as
    // part of the contiguous run; fold them into the sorted "other" group.
    rest.extend(numbered.into_values());
    rest.sort_by_key(|name| name.to_ascii_lowercase());
    ordered.extend(rest);
    ordered
}

/// Parses a `pak<digits>.pak` filename (case-insensitive) into its number.
fn numbered_pak_index(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let digits = lower.strip_prefix("pak")?.strip_suffix(".pak")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn index_loose_files(
    index: &mut Index,
    root: &Path,
    dir: &Path,
    depth: usize,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<()> {
    if depth > limits.max_depth {
        return Err(AssetError::LimitExceeded);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        // Never follows a symlink: this crate's index must only ever
        // reflect real files under the search path root.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if metadata.is_dir() {
            index_loose_files(index, root, &path, depth + 1, limits, budget)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(relative_str) = relative.to_str() else {
            continue;
        };
        if relative_str.to_ascii_lowercase().ends_with(".pak") {
            // PAK archives are indexed as containers, never as assets in
            // their own right.
            continue;
        }
        let Ok(normalized) = normalize(relative_str, limits) else {
            continue;
        };
        if index.contains_key(normalized.key()) {
            continue;
        }
        budget.take()?;
        index.insert(
            normalized.key().to_string(),
            Indexed {
                display: normalized.display().to_string(),
                location: Location::Loose(path),
            },
        );
    }
    Ok(())
}

/// Reads and validates one PAK archive's directory (header plus directory
/// bytes only — never the whole archive), and merges its entries into
/// `index` under first-wins precedence.
fn index_pak(
    index: &mut Index,
    archive_path: &Path,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<()> {
    let mut file = File::open(archive_path).map_err(|_| AssetError::MalformedArchive)?;
    let total_len = file
        .metadata()
        .map_err(|_| AssetError::MalformedArchive)?
        .len();

    let mut header_bytes = [0u8; HEADER_LEN];
    file.read_exact(&mut header_bytes)
        .map_err(|_| AssetError::MalformedArchive)?;
    let (dir_offset, dir_size) =
        pak::parse_header(&header_bytes).map_err(|_| AssetError::MalformedArchive)?;

    // Bound the directory read itself before allocating: `Directory::
    // from_parts` re-validates the entry count against `limits.pak`, but
    // this avoids allocating an attacker-chosen `dir_size` up front.
    let max_dir_bytes = limits.pak.max_entries.saturating_mul(pak::ENTRY_LEN);
    if dir_size as usize > max_dir_bytes {
        return Err(AssetError::MalformedArchive);
    }

    let mut dir_bytes = vec![0u8; dir_size as usize];
    file.seek(SeekFrom::Start(u64::from(dir_offset)))
        .map_err(|_| AssetError::MalformedArchive)?;
    file.read_exact(&mut dir_bytes)
        .map_err(|_| AssetError::MalformedArchive)?;

    let directory = Directory::from_parts(total_len, dir_size, &dir_bytes, &limits.pak)
        .map_err(|_| AssetError::MalformedArchive)?;

    for entry in directory.entries() {
        let Ok(name) = core::str::from_utf8(entry.trimmed_name()) else {
            continue;
        };
        let Ok(normalized) = normalize(name, limits) else {
            continue;
        };
        if index.contains_key(normalized.key()) {
            continue;
        }
        budget.take()?;
        index.insert(
            normalized.key().to_string(),
            Indexed {
                display: normalized.display().to_string(),
                location: Location::Pak {
                    archive: archive_path.to_path_buf(),
                    offset: u64::from(entry.offset),
                    size: u64::from(entry.size),
                },
            },
        );
    }
    Ok(())
}

/// Builds the whole index across every search path, in priority order.
pub(crate) fn build(files_dir: &Path, search_paths: &[String], limits: &Limits) -> Result<Index> {
    let mut index = Index::new();
    let mut budget = Budget {
        remaining_files: limits.max_indexed_files,
    };
    for search_path in search_paths {
        let normalized = normalize(search_path, limits).map_err(|_| AssetError::InvalidPath)?;
        let mod_root = files_dir.join(normalized.display());
        index_search_path(&mut index, &mod_root, limits, &mut budget)?;
    }
    Ok(index)
}
