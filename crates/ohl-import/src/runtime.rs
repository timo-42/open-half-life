//! Finding an already-published payload, for the engine to read.
//!
//! The import pipeline is a *writer*: it stages a tree once and records its
//! identity next to the medium's provenance entry. Everything after it — map
//! loading, textures, sounds — is a *reader*, and this module is the only
//! seam between the two. It resolves the published tree and hands out a
//! read-only view of it.
//!
//! # Authority
//!
//! [`PayloadTree`] grants exactly one power: open a file the payload policy
//! already accepted, under the published tree, for reading. It refuses any
//! spelling [`ohl_payload::PayloadPath`] refuses, so a runtime-supplied name
//! can neither escape the tree nor name a device, and it never creates,
//! writes, removes or follows a path outside it. It offers no directory
//! listing, because nothing in the engine needs one to open a known asset.
//!
//! # What it does not do
//!
//! It does not validate the payload, re-check its completion metadata, or
//! prove the tree is the one this run imported: the staging protocol already
//! did that at publication time, and the recorded identity is what binds the
//! medium to the tree.

use std::fs::File;
use std::path::{Path, PathBuf};

use ohl_core::SanitizedError;
use ohl_media::{CacheLayout, ValidatedMedia};
use ohl_payload::PayloadPath;

use crate::pipeline::recorded_payload_identity;

/// A read-only view of one published payload tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadTree {
    /// The `files` directory of the published tree.
    files: PathBuf,
}

impl PayloadTree {
    /// The directory holding the payload's files.
    ///
    /// It is the composition root's to hand to a virtual filesystem; it is
    /// not media-derived, because every component of it is either
    /// application-owned or a one-way digest.
    #[must_use]
    pub fn files_directory(&self) -> &Path {
        &self.files
    }

    /// Opens one payload-relative file for reading.
    ///
    /// # Errors
    /// [`SanitizedError::InvalidInput`] when `relative` is not a legal
    /// payload path, and [`SanitizedError::NotFound`] when no such file is
    /// published. Neither carries the spelling.
    pub fn open(&self, relative: &str) -> Result<File, SanitizedError> {
        let path = PayloadPath::parse(relative).map_err(|_| SanitizedError::InvalidInput)?;
        let mut resolved = self.files.clone();
        for component in path.components() {
            resolved.push(component);
        }
        // The staged tree contains regular files only, so anything else at
        // that name is not payload.
        let metadata =
            std::fs::symlink_metadata(&resolved).map_err(|_| SanitizedError::NotFound)?;
        if !metadata.is_file() {
            return Err(SanitizedError::NotFound);
        }
        File::open(&resolved).map_err(|_| SanitizedError::NotFound)
    }

    /// Whether a payload-relative file is published.
    #[must_use]
    pub fn contains(&self, relative: &str) -> bool {
        self.open(relative).is_ok()
    }
}

/// The published payload tree for `media`, if this host has one.
///
/// The lookup is exactly the import's own bookkeeping read back: the medium's
/// provenance entry records one payload identity, and the payload store
/// publishes that identity under one directory name.
#[must_use]
pub fn find_published_payload(
    cache_layout: &CacheLayout,
    media: &ValidatedMedia,
    payload_root: &Path,
) -> Option<PayloadTree> {
    let identity = recorded_payload_identity(cache_layout, media)?;
    let files = ohl_payload::published_files_directory(payload_root, &identity)?;
    std::fs::symlink_metadata(&files)
        .ok()
        .filter(std::fs::Metadata::is_dir)
        .map(|_| PayloadTree { files })
}

#[cfg(test)]
mod tests {
    use super::PayloadTree;
    use std::fs;

    fn tree(root: &std::path::Path) -> PayloadTree {
        let files = root.join("files");
        fs::create_dir_all(files.join("valve")).expect("create the tree");
        fs::write(files.join("valve").join("one.txt"), b"payload").expect("write a file");
        PayloadTree { files }
    }

    #[test]
    fn a_published_file_opens_and_a_missing_one_does_not() {
        let root = tempfile::tempdir().expect("temp dir");
        let tree = tree(root.path());
        assert!(tree.contains("valve/one.txt"));
        assert!(tree.open("valve/one.txt").is_ok());
        assert!(!tree.contains("valve/two.txt"));
    }

    #[test]
    fn an_illegal_spelling_is_refused_before_the_filesystem_is_touched() {
        let root = tempfile::tempdir().expect("temp dir");
        let tree = tree(root.path());
        for spelling in ["../escape", "/rooted", "valve/../../escape", "", "valve"] {
            assert!(!tree.contains(spelling), "accepted {spelling:?}");
        }
    }

    #[test]
    fn a_backslash_spelling_resolves_to_the_same_file() {
        let root = tempfile::tempdir().expect("temp dir");
        let tree = tree(root.path());
        assert!(tree.contains("valve\\one.txt"));
    }
}
