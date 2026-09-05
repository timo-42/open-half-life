//! The fallback backend for targets that are neither Unix nor Windows.
//!
//! There is no portable way to pin a native file identity on such a target,
//! so acquisition always fails with [`MediaSourceError::Unsupported`] rather
//! than silently degrading to a weaker guarantee. This mirrors the C++
//! `media_source_unsupported.cpp` translation unit.

use std::path::Path;

use super::shared::NativeSnapshot;
use crate::media_source::MediaSourceError;

/// A never-constructed stand-in for a pinned native file object.
#[derive(Debug)]
pub(crate) enum PinnedFile {}

impl PinnedFile {
    /// Always fails: this target has no pinned-acquisition backend.
    pub(crate) fn open(_path: &Path) -> Result<(Self, NativeSnapshot), MediaSourceError> {
        Err(MediaSourceError::Unsupported)
    }

    /// Unreachable: no value of this type can exist.
    pub(crate) fn snapshot(&self) -> Result<NativeSnapshot, MediaSourceError> {
        match *self {}
    }

    /// Unreachable: no value of this type can exist.
    pub(crate) fn read_at(
        &self,
        _offset: u64,
        _destination: &mut [u8],
    ) -> Result<usize, MediaSourceError> {
        match *self {}
    }
}
