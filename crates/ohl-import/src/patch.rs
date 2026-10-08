//! Standalone installer import and transactional replacement-file overlays.
//!
//! The EXE is data only. Its container uses the same confined worker as an
//! ISO import. Complete replacement files override the base by portable,
//! case-insensitive path; binary deltas and installer scripts are not applied.
//! The original payload and its provenance record are never modified.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ohl_core::StreamingSha256;
use ohl_media::{
    CacheLayout, MediaClass, MediaDescription, MediaDigest, ValidatedMedia, VolumeLabel,
};
use ohl_payload::layout::{PayloadEntryMetadata, PayloadImportLimits, plan_payload_layout};
use ohl_payload::selection::SelectionRecipe;
use ohl_payload::stream::{PayloadByteSink, PayloadSource};
use ohl_platform::MediaSource;

use crate::pipeline::{
    ImportCancellation, ImportConfig, ImportError, ImportReport, ProgressSink, run_container_import,
};
use crate::{ParserWorkerProcess, SessionAllocation, SessionIdAllocator, WorkerProcess};

/// An existing published tree used as the base of a replacement-file patch.
#[derive(Clone, Copy)]
pub struct BasePayload<'a> {
    /// The identity returned by the original import.
    pub identity: &'a str,
    /// Its published `files` directory.
    pub files: &'a Path,
}

/// Pins and fingerprints an installer without executing it.
pub fn open_installer(path: &Path) -> Result<ValidatedMedia, ImportError> {
    let source = Arc::new(MediaSource::open(path).map_err(|_| ImportError::Media)?);
    ValidatedMedia::fingerprinting(
        source,
        MediaDescription::new(MediaClass::Installer, "installer", VolumeLabel::empty()),
    )
    .map_err(|_| ImportError::Media)
}

/// Imports complete replacement files over a published base, using a native
/// confined worker. The returned identity identifies the combined tree.
#[allow(clippy::too_many_arguments)]
pub fn run_patch_import(
    installer: &ValidatedMedia,
    base: BasePayload<'_>,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    let config = ImportConfig::default();
    let allocation = SessionIdAllocator::new()
        .allocate()
        .map_err(|_| ImportError::SessionsExhausted)?;
    run_patch_inner(
        installer,
        base,
        recipe,
        payload_root,
        cache_layout,
        &config,
        || {
            ParserWorkerProcess::launch(Instant::now() + config.startup_timeout)
                .map_err(ImportError::WorkerUnavailable)
        },
        allocation,
        cancellation,
        progress,
    )
}

/// The same import with the sealed test-worker seam.
#[allow(clippy::too_many_arguments)]
pub fn run_patch_import_with_worker<W: WorkerProcess>(
    installer: &ValidatedMedia,
    base: BasePayload<'_>,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    config: &ImportConfig,
    worker: W,
    allocation: SessionAllocation,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    run_patch_inner(
        installer,
        base,
        recipe,
        payload_root,
        cache_layout,
        config,
        || Ok(worker),
        allocation,
        cancellation,
        progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_patch_inner<W: WorkerProcess, F: FnOnce() -> Result<W, ImportError>>(
    installer: &ValidatedMedia,
    base: BasePayload<'_>,
    recipe: &SelectionRecipe,
    payload_root: &Path,
    cache_layout: &CacheLayout,
    config: &ImportConfig,
    launch: F,
    allocation: SessionAllocation,
    cancellation: ImportCancellation<'_>,
    progress: &mut dyn ProgressSink,
) -> Result<ImportReport, ImportError> {
    if cancellation.stop_requested() {
        return Err(ImportError::Cancelled);
    }
    let window =
        crate::locate::locate_source(installer.source(), &config.locate, cancellation.transport)
            .map_err(|_| ImportError::NoContainer)?;
    run_container_import(
        installer,
        window,
        Some(base),
        recipe,
        payload_root,
        cache_layout,
        config,
        launch,
        allocation,
        cancellation,
        progress,
    )
}

struct BaseFile {
    path: PathBuf,
    size: u64,
    digest: MediaDigest,
}

pub(crate) struct Overlay {
    entries: Vec<PayloadEntryMetadata>,
    files: BTreeMap<u64, BaseFile>,
    identity: Option<String>,
}

impl Overlay {
    #[allow(
        clippy::too_many_lines,
        reason = "one bounded walk builds the overlay and its content identity"
    )]
    pub(crate) fn prepare(
        base: Option<BasePayload<'_>>,
        patch: &[PayloadEntryMetadata],
        limits: &PayloadImportLimits,
        cancellation: ImportCancellation<'_>,
    ) -> Result<Self, ImportError> {
        // Validate patch aliases before inserting into the precedence map.
        plan_payload_layout(patch, limits).map_err(|_| ImportError::Layout)?;
        let mut result = Self {
            entries: patch.to_vec(),
            files: BTreeMap::new(),
            identity: None,
        };
        let Some(base) = base else {
            return Ok(result);
        };
        if base.identity.is_empty() || base.identity.len() > 96 {
            return Err(ImportError::Layout);
        }
        let mut entries = BTreeMap::new();
        for entry in patch {
            let mut entry = entry.clone();
            // Use one spelling for directory components on every platform.
            ohl_payload::PayloadPath::parse(&entry.archive_path)
                .map_err(|_| ImportError::Layout)?
                .portability_key()
                .clone_into(&mut entry.archive_path);
            entries.insert(entry.archive_path.clone(), entry);
        }
        let mut token = patch
            .iter()
            .map(|entry| entry.source_token)
            .max()
            .unwrap_or(0);
        let mut pending = vec![(base.files.to_path_buf(), String::new(), 0usize)];
        let mut visited = 0usize;
        let mut path_bytes = 0u64;
        let mut total_bytes = 0u64;
        let mut base_keys = BTreeSet::new();
        let mut fingerprints = BTreeMap::new();
        while let Some((directory, prefix, depth)) = pending.pop() {
            if cancellation.stop_requested() {
                return Err(ImportError::Cancelled);
            }
            if depth > 64 || !fs::symlink_metadata(&directory).is_ok_and(|meta| meta.is_dir()) {
                return Err(ImportError::Layout);
            }
            for child in fs::read_dir(directory).map_err(|_| ImportError::Store)? {
                if cancellation.stop_requested() {
                    return Err(ImportError::Cancelled);
                }
                let child = child.map_err(|_| ImportError::Store)?;
                visited = visited.checked_add(1).ok_or(ImportError::Layout)?;
                if visited > limits.maximum_entries.saturating_mul(2) {
                    return Err(ImportError::Layout);
                }
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| ImportError::Layout)?;
                let relative = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                path_bytes = path_bytes.saturating_add(relative.len() as u64);
                if path_bytes > limits.maximum_path_bytes {
                    return Err(ImportError::Layout);
                }
                let kind = child.file_type().map_err(|_| ImportError::Store)?;
                if kind.is_dir() {
                    pending.push((child.path(), relative, depth + 1));
                    continue;
                }
                if !kind.is_file() {
                    return Err(ImportError::Layout);
                }
                let normalized =
                    ohl_payload::PayloadPath::parse(&relative).map_err(|_| ImportError::Layout)?;
                let key = normalized.portability_key().to_owned();
                if !base_keys.insert(key.clone()) {
                    return Err(ImportError::Layout);
                }
                if entries.contains_key(&key) {
                    continue;
                }
                let source = MediaSource::open(&child.path()).map_err(|_| ImportError::Store)?;
                total_bytes = total_bytes.saturating_add(source.size());
                if source.size() > limits.maximum_entry_bytes
                    || total_bytes > limits.maximum_total_bytes
                {
                    return Err(ImportError::Layout);
                }
                let digest = hash_source(&source, cancellation)?;
                token = token.checked_add(1).ok_or(ImportError::Layout)?;
                fingerprints.insert(key.clone(), digest);
                entries.insert(
                    key.clone(),
                    PayloadEntryMetadata {
                        source_token: token,
                        archive_path: key,
                        size_bytes: source.size(),
                    },
                );
                result.files.insert(
                    token,
                    BaseFile {
                        path: child.path(),
                        size: source.size(),
                        digest,
                    },
                );
                if entries.len() > limits.maximum_entries {
                    return Err(ImportError::Layout);
                }
            }
        }
        let mut hash = StreamingSha256::new();
        hash.update(b"ohl-replacement-overlay-v1\0");
        hash.update(base.identity.as_bytes());
        for (path, digest) in fingerprints {
            hash.update(&(path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            hash.update(digest.as_bytes());
        }
        result.identity = Some(MediaDigest::from_bytes(hash.finalize()).to_hex());
        result.entries = entries.into_values().collect();
        Ok(result)
    }

    pub(crate) fn entries(&self) -> &[PayloadEntryMetadata] {
        &self.entries
    }
    pub(crate) fn identity(&self) -> Option<&str> {
        self.identity.as_deref()
    }
    pub(crate) fn source<'a>(&'a mut self, patch: &'a mut dyn PayloadSource) -> OverlaySource<'a> {
        OverlaySource {
            files: &self.files,
            patch,
        }
    }
}

fn hash_source(
    source: &MediaSource,
    cancellation: ImportCancellation<'_>,
) -> Result<MediaDigest, ImportError> {
    let mut hash = StreamingSha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut offset = 0;
    source.verify_unchanged().map_err(|_| ImportError::Media)?;
    while offset < source.size() {
        if cancellation.stop_requested() {
            return Err(ImportError::Cancelled);
        }
        let count = usize::try_from(source.size() - offset)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        source
            .read_exact_at(offset, &mut buffer[..count])
            .map_err(|_| ImportError::Media)?;
        hash.update(&buffer[..count]);
        offset += count as u64;
    }
    source.verify_unchanged().map_err(|_| ImportError::Media)?;
    Ok(MediaDigest::from_bytes(hash.finalize()))
}

pub(crate) struct OverlaySource<'a> {
    files: &'a BTreeMap<u64, BaseFile>,
    patch: &'a mut dyn PayloadSource,
}

impl PayloadSource for OverlaySource<'_> {
    fn stream(
        &mut self,
        media: &MediaSource,
        token: u64,
        cancellation: &ohl_payload::CancellationToken,
        sink: &mut dyn PayloadByteSink,
    ) -> bool {
        let Some(file) = self.files.get(&token) else {
            return self.patch.stream(media, token, cancellation, sink);
        };
        let Ok(source) = MediaSource::open(&file.path) else {
            return false;
        };
        if source.size() != file.size || source.verify_unchanged().is_err() {
            return false;
        }
        let mut hash = StreamingSha256::new();
        let mut buffer = vec![0u8; 64 * 1024];
        let mut offset = 0;
        while offset < file.size {
            if cancellation.stop_requested() {
                return false;
            }
            let count = usize::try_from(file.size - offset)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            if source.read_exact_at(offset, &mut buffer[..count]).is_err()
                || !sink.write_chunk(&buffer[..count])
            {
                return false;
            }
            hash.update(&buffer[..count]);
            offset += count as u64;
        }
        source.verify_unchanged().is_ok() && hash.finalize() == *file.digest.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancellationToken;

    fn prepare(base: &Path, patch: &[PayloadEntryMetadata]) -> Result<Overlay, ImportError> {
        Overlay::prepare(
            Some(BasePayload {
                identity: "synthetic-base",
                files: base,
            }),
            patch,
            &PayloadImportLimits::default(),
            ImportCancellation {
                transport: &CancellationToken::default(),
                staging: &ohl_payload::CancellationToken::default(),
            },
        )
    }

    #[test]
    fn standalone_wise_keeps_the_pe_header_inside_the_worker_window() {
        use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};
        let root = tempfile::tempdir().unwrap();
        let installer = root.path().join("synthetic.exe");
        let package = build_package(&PackageOptions::with_files(vec![SyntheticFile::new(
            b"invented/new.dat",
            vec![7; 512],
        )]));
        fs::write(&installer, package.image).unwrap();
        let media = open_installer(&installer).unwrap();
        let window = crate::locate::locate_source(
            media.source(),
            &crate::LocateLimits::default(),
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(window.base_offset(), 0);
        assert_eq!(window.length(), media.size_bytes());
    }

    #[test]
    fn unsupported_and_truncated_installers_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let installer = root.path().join("synthetic.exe");
        for bytes in [b"not a container".as_slice(), b"MZ", b""] {
            fs::write(&installer, bytes).unwrap();
            let media = open_installer(&installer).unwrap();
            assert!(
                crate::locate::locate_source(
                    media.source(),
                    &crate::LocateLimits::default(),
                    &CancellationToken::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn patch_wins_ignoring_case_and_preserves_unmodified_files() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Component")).unwrap();
        fs::write(root.path().join("Component/OLD.dat"), b"old").unwrap();
        fs::write(root.path().join("Component/keep.dat"), b"keep").unwrap();
        let patch = [PayloadEntryMetadata {
            source_token: 1,
            archive_path: "component/old.DAT".into(),
            size_bytes: 5,
        }];
        let overlay = prepare(root.path(), &patch).unwrap();
        assert_eq!(overlay.entries.len(), 2);
        assert_eq!(overlay.files.len(), 1);
        assert_eq!(overlay.entries[1].archive_path, "component/old.dat");
        assert_eq!(overlay.entries[1].source_token, 1);
        assert_eq!(
            fs::read(root.path().join("Component/OLD.dat")).unwrap(),
            b"old"
        );
    }

    #[test]
    fn changing_retained_content_changes_identity_even_at_same_size() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("keep.dat"), b"one").unwrap();
        let first = prepare(root.path(), &[]).unwrap();
        fs::write(root.path().join("keep.dat"), b"two").unwrap();
        let second = prepare(root.path(), &[]).unwrap();
        assert_ne!(first.identity, second.identity);
    }

    struct RejectPatch;
    impl PayloadSource for RejectPatch {
        fn stream(
            &mut self,
            _: &MediaSource,
            _: u64,
            _: &ohl_payload::CancellationToken,
            _: &mut dyn PayloadByteSink,
        ) -> bool {
            panic!("base token sent to patch");
        }
    }
    struct Discard;
    impl PayloadByteSink for Discard {
        fn write_chunk(&mut self, _: &[u8]) -> bool {
            true
        }
    }

    #[test]
    fn modifying_a_base_file_after_planning_refuses_publication() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("keep.dat");
        fs::write(&path, b"one").unwrap();
        let mut overlay = prepare(root.path(), &[]).unwrap();
        let token = overlay.entries[0].source_token;
        fs::write(&path, b"two").unwrap();
        let media = MediaSource::open(&path).unwrap();
        assert!(!overlay.source(&mut RejectPatch).stream(
            &media,
            token,
            &ohl_payload::CancellationToken::default(),
            &mut Discard
        ));
    }

    #[test]
    fn combined_file_directory_collisions_are_rejected_by_layout() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("folder"), b"file").unwrap();
        let patch = [PayloadEntryMetadata {
            source_token: 1,
            archive_path: "folder/new.dat".into(),
            size_bytes: 3,
        }];
        let overlay = prepare(root.path(), &patch).unwrap();
        assert!(plan_payload_layout(overlay.entries(), &PayloadImportLimits::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_base_entries_are_refused() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", root.path().join("escape")).unwrap();
        assert!(matches!(
            prepare(root.path(), &[]),
            Err(ImportError::Layout)
        ));
    }
}
