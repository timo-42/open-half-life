//! One uniform mount over either supported media class.

use std::sync::{Arc, Mutex};

use ohl_core::SanitizedError;
use ohl_iso9660::{Iso9660Archive, Iso9660File};
use ohl_media_archive::{
    DirectoryCursor, DirectoryLimits, DirectoryPage, FilesystemDescription, MediaArchive as _,
    MediaClass, MediaFileHandle as _, VolumeLabel,
};
use ohl_platform::MediaSource;
use ohl_udf::{UdfArchive, UdfFile};

use crate::block_reader::{DEFAULT_VERIFY_INTERVAL_BLOCKS, MediaSourceBlockReader};
use crate::file::MediaFile;

/// Widens a [`ohl_platform::MediaSourceError`] into the shared sanitized
/// vocabulary so preflight and archive call sites can share one error type.
fn source_error(error: ohl_platform::MediaSourceError) -> SanitizedError {
    error.into()
}

/// The mounted archive, dispatched by media class.
///
/// A trait object is deliberately not used here: [`ohl_media_archive::MediaArchive`]
/// carries an associated `File` type, so an enum keeps both readers' concrete
/// file handles distinguishable without boxing every call.
pub(crate) enum ArchiveState {
    Iso9660(Iso9660Archive<MediaSourceBlockReader>),
    Udf(UdfArchive<MediaSourceBlockReader>),
}

/// A file handle from either mounted archive class.
#[derive(Debug, Clone)]
pub(crate) enum FileHandle {
    Iso9660(Iso9660File),
    Udf(UdfFile),
}

impl FileHandle {
    pub(crate) fn size(&self) -> u64 {
        match self {
            Self::Iso9660(file) => file.size(),
            Self::Udf(file) => file.size(),
        }
    }

    pub(crate) fn position(&self) -> u64 {
        match self {
            Self::Iso9660(file) => file.position(),
            Self::Udf(file) => file.position(),
        }
    }

    pub(crate) fn seek(&mut self, offset: u64) -> Result<(), SanitizedError> {
        match self {
            Self::Iso9660(file) => file.seek(offset),
            Self::Udf(file) => file.seek(offset),
        }
    }
}

impl ArchiveState {
    fn filesystem(&self) -> FilesystemDescription {
        match self {
            Self::Iso9660(archive) => archive.filesystem(),
            Self::Udf(archive) => archive.filesystem(),
        }
    }

    fn volume_label(&self) -> &VolumeLabel {
        match self {
            Self::Iso9660(archive) => archive.volume_label(),
            Self::Udf(archive) => archive.volume_label(),
        }
    }

    fn list_page(&mut self, path: &str) -> Result<DirectoryPage, SanitizedError> {
        match self {
            Self::Iso9660(archive) => archive.list_page(path),
            Self::Udf(archive) => archive.list_page(path),
        }
    }

    fn continue_list(&mut self, cursor: DirectoryCursor) -> Result<DirectoryPage, SanitizedError> {
        match self {
            Self::Iso9660(archive) => archive.continue_list(cursor),
            Self::Udf(archive) => archive.continue_list(cursor),
        }
    }

    fn open_file(&mut self, path: &str) -> Result<FileHandle, SanitizedError> {
        match self {
            Self::Iso9660(archive) => archive.open_file(path).map(FileHandle::Iso9660),
            Self::Udf(archive) => archive.open_file(path).map(FileHandle::Udf),
        }
    }

    pub(crate) fn read_file(
        &mut self,
        handle: &mut FileHandle,
        out: &mut [u8],
    ) -> Result<usize, SanitizedError> {
        match (self, handle) {
            (Self::Iso9660(archive), FileHandle::Iso9660(file)) => archive.read_file(file, out),
            (Self::Udf(archive), FileHandle::Udf(file)) => archive.read_file(file, out),
            // A handle from one media class can never reach the other
            // archive's state through this crate's public API (`open_file`
            // always pairs a handle with the archive that produced it), so
            // this arm is defense in depth, not a reachable path.
            _ => Err(SanitizedError::InvalidInput),
        }
    }
}

/// A read-only mount over a pinned [`MediaSource`], uniform across the ISO
/// 9660/Joliet and UDF media classes.
///
/// The class is decided by a bounded structural preflight, never guessed from
/// a pathname, and every operation is bounded by the [`DirectoryLimits`]
/// supplied at mount. Cloning a mount with [`Self::share`] is a cheap,
/// read-only handle that keeps the same mounted archive state alive: cursors
/// and open files produced through one handle remain valid through any of its
/// shares, matching the C++ facade's `share()`.
pub struct Mount {
    inner: Arc<Mutex<ArchiveState>>,
    class: MediaClass,
    filesystem: FilesystemDescription,
    volume_label: VolumeLabel,
}

impl Mount {
    /// Detects the media class by running the ISO 9660 preflight and then
    /// the UDF preflight over `source`, then mounts the matching archive.
    ///
    /// # Errors
    ///
    /// [`SanitizedError::Unsupported`] when neither preflight recognizes the
    /// source, [`SanitizedError::InvalidInput`] for invalid `limits` or a
    /// structurally invalid volume, and the source's own sanitized error when
    /// a block could not be read or the pinned source changed.
    pub fn open(source: Arc<MediaSource>, limits: DirectoryLimits) -> Result<Self, SanitizedError> {
        Self::open_with_verify_interval(source, limits, DEFAULT_VERIFY_INTERVAL_BLOCKS)
    }

    /// Same as [`Self::open`], with an explicit periodic re-verification
    /// cadence for the [`MediaSourceBlockReader`] every mounted read goes
    /// through (see [`MediaSourceBlockReader::with_verify_interval`]).
    ///
    /// # Errors
    ///
    /// Same as [`Self::open`].
    pub fn open_with_verify_interval(
        source: Arc<MediaSource>,
        limits: DirectoryLimits,
        verify_interval_blocks: u64,
    ) -> Result<Self, SanitizedError> {
        let mut probe = MediaSourceBlockReader::with_verify_interval(
            Arc::clone(&source),
            verify_interval_blocks,
        )
        .map_err(source_error)?;

        match ohl_iso9660::preflight(&mut probe) {
            Ok(_) => {
                return Self::open_as_with_verify_interval(
                    MediaClass::Iso9660,
                    source,
                    limits,
                    verify_interval_blocks,
                );
            }
            Err(SanitizedError::Unsupported) => {}
            Err(error) => return Err(error),
        }

        match ohl_udf::preflight(&mut probe) {
            Ok(_) => Self::open_as_with_verify_interval(
                MediaClass::Udf,
                source,
                limits,
                verify_interval_blocks,
            ),
            Err(SanitizedError::Unsupported) => Err(SanitizedError::Unsupported),
            Err(error) => Err(error),
        }
    }

    /// Mounts `source` as `class` directly, skipping both preflights.
    ///
    /// This is for a caller that already classified the same pinned source
    /// through a prior preflight (for example while inspecting a payload
    /// before deciding to mount it) and wants to avoid reading the volume
    /// descriptors a second time. The mount's own reader (`ohl-iso9660` or
    /// `ohl-udf`) still re-runs its project-owned preflight and re-validates
    /// every structure it parses; skipping only avoids the *extra* probe this
    /// crate would otherwise perform.
    ///
    /// # Errors
    ///
    /// Same as [`Self::open`], except that a source of the other class
    /// reports [`SanitizedError::Unsupported`] or [`SanitizedError::InvalidInput`]
    /// from the class-specific reader instead of from a second probe.
    pub fn open_as(
        class: MediaClass,
        source: Arc<MediaSource>,
        limits: DirectoryLimits,
    ) -> Result<Self, SanitizedError> {
        Self::open_as_with_verify_interval(class, source, limits, DEFAULT_VERIFY_INTERVAL_BLOCKS)
    }

    /// Same as [`Self::open_as`], with an explicit periodic re-verification
    /// cadence; see [`Self::open_with_verify_interval`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::open_as`].
    pub fn open_as_with_verify_interval(
        class: MediaClass,
        source: Arc<MediaSource>,
        limits: DirectoryLimits,
        verify_interval_blocks: u64,
    ) -> Result<Self, SanitizedError> {
        limits.validate()?;
        let reader = MediaSourceBlockReader::with_verify_interval(source, verify_interval_blocks)
            .map_err(source_error)?;
        let state = match class {
            MediaClass::Iso9660 => ArchiveState::Iso9660(Iso9660Archive::open(reader, limits)?),
            MediaClass::Udf => ArchiveState::Udf(UdfArchive::open(reader, limits)?),
        };
        let filesystem = state.filesystem();
        let volume_label = state.volume_label().clone();
        Ok(Self {
            inner: Arc::new(Mutex::new(state)),
            class,
            filesystem,
            volume_label,
        })
    }

    /// Returns another read-only handle over the same mounted state.
    ///
    /// The clone is cheap: it shares the underlying archive rather than
    /// reopening or re-validating the media, so cursors and open files
    /// produced by either handle remain valid through the other.
    #[must_use]
    pub fn share(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            class: self.class,
            filesystem: self.filesystem,
            volume_label: self.volume_label.clone(),
        }
    }

    /// The media class this mount was classified as.
    pub const fn class(&self) -> MediaClass {
        self.class
    }

    /// The fixed description of the mounted structure.
    pub const fn filesystem(&self) -> FilesystemDescription {
        self.filesystem
    }

    /// The sanitized volume label, which may be empty.
    pub fn volume_label(&self) -> &VolumeLabel {
        &self.volume_label
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ArchiveState>, SanitizedError> {
        // A poisoned mutex means a prior call panicked while holding the
        // lock; the mounted state's invariants can no longer be trusted, so
        // this is reported as an internal error rather than propagating the
        // poison (which would require exposing panic payloads).
        self.inner.lock().map_err(|_| SanitizedError::Internal)
    }

    /// Returns one bounded page of `path` in deterministic on-media order.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the path is invalid (including an
    /// absolute, `.`, `..`, or empty component the normalizer rejects),
    /// absent, exceeds a configured limit, or the media is structurally
    /// invalid.
    pub fn list_page(&self, path: &str) -> Result<DirectoryPage, SanitizedError> {
        self.lock()?.list_page(path)
    }

    /// Continues an enumeration from a cursor this mount (or one of its
    /// shares) produced.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::InvalidInput`] for a cursor produced by a
    /// different mount, and the same codes as [`Self::list_page`] otherwise.
    pub fn continue_list(&self, cursor: DirectoryCursor) -> Result<DirectoryPage, SanitizedError> {
        self.lock()?.continue_list(cursor)
    }

    /// Compatibility listing: succeeds only with the complete bounded result.
    ///
    /// # Errors
    ///
    /// Returns the same codes as [`Self::list_page`]; any enumeration limit
    /// yields an error rather than a truncated listing.
    pub fn list(
        &self,
        path: &str,
    ) -> Result<Vec<ohl_media_archive::DirectoryEntry>, SanitizedError> {
        let mut page = self.list_page(path)?;
        let mut entries = std::mem::take(&mut page.entries);
        while let Some(cursor) = page.cursor.take() {
            page = self.continue_list(cursor)?;
            entries.append(&mut page.entries);
        }
        Ok(entries)
    }

    /// Opens a file by absolute, normalized path.
    ///
    /// # Errors
    ///
    /// Returns a sanitized code when the path is invalid, absent, refers to a
    /// directory, or the recorded extent is out of bounds.
    pub fn open_file(&self, path: &str) -> Result<MediaFile, SanitizedError> {
        let handle = self.lock()?.open_file(path)?;
        Ok(MediaFile::new(Arc::clone(&self.inner), handle))
    }
}
