//! Fixtures shared by this crate's unit tests.
//!
//! Only compiled for `cfg(test)`; nothing here is part of the public API.

use ohl_platform::{MediaSource, SourceFingerprint};

/// A pinned [`MediaSource`] over a temporary file, kept alive by the fixture.
pub(crate) struct PinnedSourceFixture {
    /// Owns the temporary file for as long as the fixture lives.
    _directory: tempfile::TempDir,
    /// The pinned capability.
    source: MediaSource,
    /// The fingerprint the capability was accepted with.
    fingerprint: SourceFingerprint,
    /// The path of the pinned file, for tests that mutate it deliberately.
    path: std::path::PathBuf,
}

impl PinnedSourceFixture {
    /// The pinned capability.
    pub(crate) const fn media_source(&self) -> &MediaSource {
        &self.source
    }

    /// The accepted fingerprint.
    pub(crate) const fn fingerprint(&self) -> &SourceFingerprint {
        &self.fingerprint
    }

    /// The pinned file's path.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// Writes `content` to a temporary file and pins it.
pub(crate) fn pinned_source(content: &[u8]) -> PinnedSourceFixture {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("pinned.source");
    std::fs::write(&path, content).expect("write source");
    let source = MediaSource::open(&path).expect("pin source");
    let fingerprint = SourceFingerprint {
        size_bytes: source.size(),
        sha256: ohl_core::StreamingSha256::digest(content),
    };
    PinnedSourceFixture {
        _directory: directory,
        source,
        fingerprint,
        path,
    }
}
