//! The metadata-only provenance cache.
//!
//! # What is stored, and what is not
//!
//! A cache entry is *one JSON file of metadata*. It records the digest, the
//! size, the recognised class, the project-owned filesystem name, a sanitized
//! volume label, and when the entry was created. It records **no source
//! path**, **no media bytes**, **no internal names**, and **no recipe
//! content**, which is what `docs/MEDIA_IMPORT.md` requires of anything that
//! survives a run. Payload import is a separate, unimplemented package; the
//! manifest says so with `payload_state: "not-imported"`.
//!
//! # The publication protocol
//!
//! 1. Reauthenticate the pinned source: [`ohl_platform::MediaSource::verify_unchanged`],
//!    then a complete rehash through
//!    [`ohl_platform::verify_complete_source_stability`] that must reproduce
//!    the digest inside the proof. Nothing is published on mismatch.
//! 2. Create the entries directory, refusing any relative path, `.`/`..`
//!    component, symbolic link, or non-directory component on the way.
//! 3. If the content-addressed entry directory already exists, verify its
//!    manifest and report [`CacheReport::Reused`]. This is the idempotent
//!    fast path and takes no lock.
//! 4. Otherwise take an exclusive advisory lock on the per-digest lock file,
//!    re-check the entry under it, stage a private directory, write the
//!    manifest into it, and publish it with
//!    [`ohl_platform::StagingDirectory::publish_no_replace`].
//! 5. If publication reports that the destination now exists, another writer
//!    won: verify its manifest and reuse it. A cache entry is never replaced,
//!    and a losing writer's staging tree is removed rather than merged.
//!
//! The lock is an optimisation and a contention signal, not the correctness
//! boundary: correctness comes from the no-replace publication, which holds
//! even against a writer that ignores the lock entirely.

use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt as _;
use ohl_platform::{PublishOutcome, StagingDirectory, verify_complete_source_stability};

use crate::description::{MediaClass, MediaDescription, VolumeLabel};
use crate::digest::MediaDigest;
use crate::error::ImportCacheError;
use crate::validated::ValidatedMedia;

/// The manifest schema this build writes and is willing to read.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// The manifest file name inside a cache entry.
pub const MANIFEST_FILE_NAME: &str = "provenance.json";

/// The directory below the cache root that holds content-addressed entries.
pub const ENTRIES_DIRECTORY_NAME: &str = "sources";

/// The largest manifest this build will read.
///
/// A manifest is a handful of fixed fields plus a 32-character label, so the
/// real size is a few hundred bytes. Anything past this bound is refused
/// without being read, so a hostile or corrupted file in the cache cannot
/// make the process allocate.
pub const MAXIMUM_MANIFEST_BYTES: u64 = 4 * 1024;

/// The fixed payload state a metadata-only entry records.
pub const PAYLOAD_STATE_NOT_IMPORTED: &str = "not-imported";

/// The staging directory prefix used while publishing an entry.
const STAGING_PREFIX: &str = "entry";

/// The application name used to discover the per-user cache directory.
const APPLICATION_NAME: &str = "open-half-life";

/// Where cache entries live.
///
/// Construct it either from the platform's per-user cache directory
/// ([`CacheLayout::user_default`]) or from an explicit `--cache` override
/// ([`CacheLayout::with_root`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayout {
    root: PathBuf,
}

impl CacheLayout {
    /// Uses an explicit cache root, as supplied by `--cache`.
    ///
    /// # Errors
    ///
    /// [`ImportCacheError::UnsafeCachePath`] when `root` is relative, empty,
    /// or contains a `.` or `..` component. The path is not touched here;
    /// directory creation and the symbolic-link checks happen during
    /// publication.
    pub fn with_root(root: impl Into<PathBuf>) -> Result<Self, ImportCacheError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(ImportCacheError::UnsafeCachePath);
        }
        for component in root.components() {
            if matches!(component, Component::CurDir | Component::ParentDir) {
                return Err(ImportCacheError::UnsafeCachePath);
            }
        }
        Ok(Self { root })
    }

    /// Uses the platform's per-user cache directory.
    ///
    /// This is `~/.cache/open-half-life` on Linux (honouring
    /// `XDG_CACHE_HOME`), `~/Library/Caches/open-half-life` on macOS, and
    /// `%LOCALAPPDATA%\open-half-life\cache` on Windows.
    ///
    /// # Errors
    ///
    /// [`ImportCacheError::CacheUnavailable`] when the platform reports no
    /// usable home or cache directory, and
    /// [`ImportCacheError::UnsafeCachePath`] when the directory it reports is
    /// not an acceptable absolute path.
    pub fn user_default() -> Result<Self, ImportCacheError> {
        let directories = directories::ProjectDirs::from("", "", APPLICATION_NAME)
            .ok_or(ImportCacheError::CacheUnavailable)?;
        Self::with_root(directories.cache_dir())
    }

    /// The cache root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding every content-addressed entry.
    #[must_use]
    pub fn entries_directory(&self) -> PathBuf {
        self.root.join(ENTRIES_DIRECTORY_NAME)
    }

    /// The entry directory for one digest.
    #[must_use]
    pub fn entry_directory(&self, digest: &MediaDigest) -> PathBuf {
        self.entries_directory().join(digest.to_hex())
    }

    /// The manifest path inside one entry.
    #[must_use]
    pub fn manifest_path(&self, digest: &MediaDigest) -> PathBuf {
        self.entry_directory(digest).join(MANIFEST_FILE_NAME)
    }

    /// The advisory publication lock file for one digest.
    ///
    /// The lock file is a sibling of the entry directory rather than a child
    /// of it, because the entry directory only comes into existence at the
    /// publication commit point.
    #[must_use]
    pub fn lock_path(&self, digest: &MediaDigest) -> PathBuf {
        self.entries_directory()
            .join(format!("{}.lock", digest.to_hex()))
    }
}

/// The metadata-only provenance manifest.
///
/// Every field is either project-owned or a bounded, sanitized restatement of
/// something the user already authorized this run to read. There is
/// deliberately no path, no name, and no byte of content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheManifest {
    /// The schema version; see [`MANIFEST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The SHA-256 of the complete source, as 64 lowercase hex characters.
    pub digest: MediaDigest,
    /// The pinned size of the source in bytes.
    pub size_bytes: u64,
    /// The recognised container class.
    pub class: MediaClass,
    /// The project-owned filesystem name.
    pub filesystem: VolumeLabel,
    /// The sanitized volume label.
    pub label: VolumeLabel,
    /// When the entry was created, in whole seconds since the Unix epoch.
    pub created_unix_seconds: u64,
    /// Fixed marker that no payload was imported; see
    /// [`PAYLOAD_STATE_NOT_IMPORTED`].
    pub payload_state: VolumeLabel,
}

/// Only `schema_version` is read on the first parsing pass, so a manifest
/// from a future or foreign build is rejected by version rather than by an
/// opaque parse failure.
#[derive(serde::Deserialize)]
struct SchemaProbe {
    schema_version: u32,
}

impl CacheManifest {
    /// Builds the manifest for one proof.
    ///
    /// # Errors
    ///
    /// [`ImportCacheError::InvalidRequest`] when the description's fixed
    /// filesystem name does not fit the bounded label.
    pub fn for_media(
        media: &ValidatedMedia,
        created_unix_seconds: u64,
    ) -> Result<Self, ImportCacheError> {
        let description: &MediaDescription = media.description();
        Ok(Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            digest: *media.digest(),
            size_bytes: media.size_bytes(),
            class: description.class,
            filesystem: VolumeLabel::new(description.filesystem)
                .map_err(|_| ImportCacheError::InvalidRequest)?,
            label: description.label,
            created_unix_seconds,
            payload_state: VolumeLabel::new(PAYLOAD_STATE_NOT_IMPORTED)
                .map_err(|_| ImportCacheError::InvalidRequest)?,
        })
    }

    /// Whether this manifest describes the same media as `media`.
    ///
    /// The creation timestamp is deliberately excluded: an entry written a
    /// week ago still describes the same media.
    #[must_use]
    pub fn describes(&self, media: &ValidatedMedia) -> bool {
        let description = media.description();
        self.schema_version == MANIFEST_SCHEMA_VERSION
            && self.digest == *media.digest()
            && self.size_bytes == media.size_bytes()
            && self.class == description.class
            && self.filesystem.as_str() == description.filesystem
            && self.label == description.label
            && self.payload_state.as_str() == PAYLOAD_STATE_NOT_IMPORTED
    }

    /// Renders the manifest as pretty-printed JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// [`ImportCacheError::ManifestWriteFailed`] if the value cannot be
    /// serialized, which the field types make unreachable in practice.
    pub fn to_json(&self) -> Result<String, ImportCacheError> {
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|_| ImportCacheError::ManifestWriteFailed)?;
        json.push('\n');
        Ok(json)
    }

    /// Parses a manifest.
    ///
    /// # Errors
    ///
    /// [`ImportCacheError::ManifestTooLarge`] past
    /// [`MAXIMUM_MANIFEST_BYTES`], [`ImportCacheError::ManifestSchemaUnsupported`]
    /// for a manifest that declares another schema version, and
    /// [`ImportCacheError::ManifestConflict`] for anything unparsable,
    /// including a tampered file and one carrying unknown fields.
    pub fn parse(bytes: &[u8]) -> Result<Self, ImportCacheError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAXIMUM_MANIFEST_BYTES {
            return Err(ImportCacheError::ManifestTooLarge);
        }
        let probe: SchemaProbe =
            serde_json::from_slice(bytes).map_err(|_| ImportCacheError::ManifestConflict)?;
        if probe.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ImportCacheError::ManifestSchemaUnsupported);
        }
        serde_json::from_slice(bytes).map_err(|_| ImportCacheError::ManifestConflict)
    }
}

/// What one call to [`prepare_import_cache`] did.
///
/// The report carries no media-derived data at all, so it is always safe to
/// log; [`CacheReport::message`] is the sanitized line the C++ application
/// emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheReport {
    /// This call published the entry.
    Created,
    /// The entry already existed and its manifest matched.
    Reused,
}

impl CacheReport {
    /// The sanitized log line for this outcome.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Created => "Prepared metadata-only media cache.",
            Self::Reused => "Reused metadata-only media cache.",
        }
    }

    /// Whether the entry already existed.
    #[must_use]
    pub const fn is_reused(self) -> bool {
        matches!(self, Self::Reused)
    }

    /// Emits [`CacheReport::message`] at info level.
    pub fn log(self) {
        tracing::info!(target: "ohl_media::import_cache", "{}", self.message());
    }
}

impl core::fmt::Display for CacheReport {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

/// Reauthenticates `media` and publishes its metadata-only cache entry.
///
/// The call is idempotent: preparing the same media twice publishes once and
/// then reports [`CacheReport::Reused`] without touching the existing entry.
///
/// See the [module documentation](self) for the full protocol.
///
/// # Errors
///
/// Every failure is one of the fixed [`ImportCacheError`] codes. On any error
/// no entry is created and no existing entry is modified.
pub fn prepare_import_cache(
    media: &ValidatedMedia,
    layout: &CacheLayout,
) -> Result<CacheReport, ImportCacheError> {
    // 1. The proof is evidence about the past. Re-establish it now.
    media.verify_unchanged()?;
    verify_complete_source_stability(media.source().as_ref(), &media.source_fingerprint())?;

    // 2. The entries directory, with the C++ tree's component checks.
    let entries = layout.entries_directory();
    ensure_directory_tree(&entries)?;

    // 3. The lock-free idempotent fast path.
    let entry = layout.entry_directory(media.digest());
    if entry.try_exists().unwrap_or(false) {
        return verify_entry(&entry, media).map(|()| CacheReport::Reused);
    }

    // 4. Exclusive publication.
    let lock = PublicationLock::acquire(&layout.lock_path(media.digest()))?;
    let report = publish_entry(&entry, &entries, media);
    drop(lock);
    report
}

/// Stages and publishes the entry. The caller holds the publication lock.
fn publish_entry(
    entry: &Path,
    entries: &Path,
    media: &ValidatedMedia,
) -> Result<CacheReport, ImportCacheError> {
    // Another writer may have finished between the fast path and the lock.
    if entry.try_exists().unwrap_or(false) {
        return verify_entry(entry, media).map(|()| CacheReport::Reused);
    }

    let created_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let manifest = CacheManifest::for_media(media, created_unix_seconds)?.to_json()?;

    let staging = StagingDirectory::create(entries, STAGING_PREFIX)
        .map_err(|_| ImportCacheError::CacheCreateFailed)?;
    write_manifest(staging.path(), manifest.as_bytes())?;

    match staging.publish_no_replace(entry) {
        Ok(PublishOutcome::Published) => Ok(CacheReport::Created),
        // A writer that ignored the lock won the race. Its entry stands; the
        // staging tree is dropped, never merged.
        Ok(PublishOutcome::DestinationExists(staging)) => {
            drop(staging);
            verify_entry(entry, media).map(|()| CacheReport::Reused)
        }
        Err((_, _)) => Err(ImportCacheError::ManifestWriteFailed),
    }
}

/// Writes the manifest into the private staging directory.
///
/// The bytes go to a temporary file first and are then persisted under the
/// final name with a no-clobber rename, so nothing in the staged tree is ever
/// truncated in place.
fn write_manifest(staging: &Path, manifest: &[u8]) -> Result<(), ImportCacheError> {
    let mut temporary = tempfile::NamedTempFile::new_in(staging)
        .map_err(|_| ImportCacheError::ManifestWriteFailed)?;
    temporary
        .write_all(manifest)
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|_| ImportCacheError::ManifestWriteFailed)?;
    temporary
        .persist_noclobber(staging.join(MANIFEST_FILE_NAME))
        .map_err(|_| ImportCacheError::ManifestWriteFailed)?;
    Ok(())
}

/// Verifies that an existing entry describes `media`.
fn verify_entry(entry: &Path, media: &ValidatedMedia) -> Result<(), ImportCacheError> {
    let manifest = read_manifest(&entry.join(MANIFEST_FILE_NAME))?;
    if manifest.describes(media) {
        Ok(())
    } else {
        Err(ImportCacheError::ManifestConflict)
    }
}

/// Reads a manifest through the bounded, no-follow boundary.
fn read_manifest(path: &Path) -> Result<CacheManifest, ImportCacheError> {
    // `symlink_metadata` does not follow the final component, so a manifest
    // replaced by a symbolic link is a conflict rather than a read of
    // whatever it points at.
    let metadata = fs::symlink_metadata(path).map_err(|_| ImportCacheError::ManifestConflict)?;
    if !metadata.is_file() {
        return Err(ImportCacheError::ManifestConflict);
    }
    if metadata.len() > MAXIMUM_MANIFEST_BYTES {
        return Err(ImportCacheError::ManifestTooLarge);
    }
    let bytes = fs::read(path).map_err(|_| ImportCacheError::ManifestConflict)?;
    CacheManifest::parse(&bytes)
}

/// Creates every component of `path`, refusing an unsafe one.
///
/// This is the port of the C++ `ensure_directory_tree`, and it carries the
/// same caveat recorded in `docs/ARCHITECTURE.md`: standard-library component
/// checks are not a fully pinned native traversal, so they are a defence
/// against accident and casual misconfiguration rather than against a hostile
/// process racing the walk.
fn ensure_directory_tree(path: &Path) -> Result<(), ImportCacheError> {
    if !path.is_absolute() {
        return Err(ImportCacheError::UnsafeCachePath);
    }

    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::CurDir | Component::ParentDir => {
                return Err(ImportCacheError::UnsafeCachePath);
            }
            Component::Normal(name) => current.push(name),
        }
        ensure_directory(&current)?;
    }
    Ok(())
}

/// Creates one component if it is absent, and refuses it if it exists as
/// anything other than a real directory.
fn ensure_directory(path: &Path) -> Result<(), ImportCacheError> {
    match fs::symlink_metadata(path) {
        // `symlink_metadata` never follows, so `is_dir` is false for a
        // symbolic link even when it points at a directory.
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // Anything that exists but is not a real directory, and any status
        // the component cannot even be queried for, is unsafe to walk into.
        Ok(_) | Err(_) => return Err(ImportCacheError::UnsafeCachePath),
    }
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        // Another writer created it first; accept it only if it really is a
        // directory now.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() => Ok(()),
                Ok(_) | Err(_) => Err(ImportCacheError::UnsafeCachePath),
            }
        }
        Err(_) => Err(ImportCacheError::CacheCreateFailed),
    }
}

/// An exclusive advisory lock held for the duration of one publication.
///
/// There is no explicit unlock: dropping the value closes the only descriptor
/// this process opened for the lock file, and both `flock` on Unix and
/// `LockFileEx` on Windows release a lock when its handle is closed. Closing
/// is also the only release path that survives a panic between staging and
/// publication.
#[derive(Debug)]
struct PublicationLock {
    #[allow(dead_code, reason = "the lock is held for as long as the file is open")]
    file: fs::File,
}

impl PublicationLock {
    /// Takes the lock without blocking.
    fn acquire(path: &Path) -> Result<Self, ImportCacheError> {
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|_| ImportCacheError::CacheCreateFailed)?;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(Self { file }),
            // Another writer is publishing this exact digest. Reporting it is
            // better than blocking a startup path behind a full rehash.
            Ok(false) => Err(ImportCacheError::CacheBusy),
            Err(_) => Err(ImportCacheError::CacheCreateFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CacheLayout, CacheManifest, CacheReport, ENTRIES_DIRECTORY_NAME, ImportCacheError,
        MANIFEST_FILE_NAME, MANIFEST_SCHEMA_VERSION, MAXIMUM_MANIFEST_BYTES,
        PAYLOAD_STATE_NOT_IMPORTED, ensure_directory_tree,
    };
    use crate::description::{MediaClass, VolumeLabel};
    use crate::digest::MediaDigest;

    fn sample_manifest() -> CacheManifest {
        CacheManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            digest: MediaDigest::from_bytes([7u8; 32]),
            size_bytes: 614_400,
            class: MediaClass::Udf,
            filesystem: VolumeLabel::new("udf").expect("printable"),
            label: VolumeLabel::new("CACHE\"TEST").expect("printable"),
            created_unix_seconds: 1_700_000_000,
            payload_state: VolumeLabel::new(PAYLOAD_STATE_NOT_IMPORTED).expect("printable"),
        }
    }

    #[test]
    fn report_messages_mirror_the_cpp_application() {
        assert_eq!(
            CacheReport::Created.message(),
            "Prepared metadata-only media cache."
        );
        assert_eq!(
            CacheReport::Reused.to_string(),
            "Reused metadata-only media cache."
        );
        assert!(CacheReport::Reused.is_reused());
        assert!(!CacheReport::Created.is_reused());
        CacheReport::Created.log();
    }

    #[test]
    fn a_relative_or_dotted_root_is_refused() {
        assert_eq!(
            CacheLayout::with_root("relative/cache").expect_err("relative"),
            ImportCacheError::UnsafeCachePath
        );
        let dotted = if cfg!(windows) {
            r"C:\cache\..\escape"
        } else {
            "/cache/../escape"
        };
        assert_eq!(
            CacheLayout::with_root(dotted).expect_err("dotted"),
            ImportCacheError::UnsafeCachePath
        );
        assert_eq!(
            ensure_directory_tree(std::path::Path::new("relative")).expect_err("relative"),
            ImportCacheError::UnsafeCachePath
        );
    }

    #[test]
    fn the_layout_is_content_addressed() {
        let root = if cfg!(windows) { r"C:\cache" } else { "/cache" };
        let layout = CacheLayout::with_root(root).expect("absolute root");
        let digest = MediaDigest::from_bytes([0xab; 32]);
        let entry = layout.entry_directory(&digest);
        assert!(entry.ends_with(digest.to_hex()));
        assert_eq!(
            entry.parent().expect("entries directory"),
            layout.entries_directory()
        );
        assert!(layout.entries_directory().ends_with(ENTRIES_DIRECTORY_NAME));
        assert_eq!(
            layout.manifest_path(&digest),
            entry.join(MANIFEST_FILE_NAME)
        );
        assert_eq!(
            layout.lock_path(&digest).parent(),
            Some(layout.entries_directory().as_path())
        );
        assert_eq!(layout.root(), std::path::Path::new(root));
    }

    #[test]
    fn a_manifest_round_trips_and_stays_well_under_the_bound() {
        let manifest = sample_manifest();
        let json = manifest.to_json().expect("serialized");
        assert!(json.ends_with('\n'));
        assert!(u64::try_from(json.len()).expect("small") < MAXIMUM_MANIFEST_BYTES);
        assert_eq!(
            CacheManifest::parse(json.as_bytes()).expect("parsed"),
            manifest
        );
    }

    #[test]
    fn a_manifest_carries_no_path_and_no_payload() {
        let json = sample_manifest().to_json().expect("serialized");
        assert!(json.contains("\"not-imported\""));
        assert!(json.contains("CACHE\\\"TEST"), "labels are JSON-escaped");
        assert!(!json.contains(".iso"));
        assert!(!json.contains('/'));
    }

    #[test]
    fn a_tampered_or_foreign_manifest_is_rejected_by_a_fixed_code() {
        assert_eq!(
            CacheManifest::parse(b"tampered\n").expect_err("not json"),
            ImportCacheError::ManifestConflict
        );
        assert_eq!(
            CacheManifest::parse(b"{}").expect_err("no version"),
            ImportCacheError::ManifestConflict
        );
        assert_eq!(
            CacheManifest::parse(br#"{"schema_version": 2}"#).expect_err("foreign schema"),
            ImportCacheError::ManifestSchemaUnsupported
        );

        let mut oversized = sample_manifest().to_json().expect("serialized");
        oversized.push_str(&" ".repeat(usize::try_from(MAXIMUM_MANIFEST_BYTES).unwrap_or(4096)));
        assert_eq!(
            CacheManifest::parse(oversized.as_bytes()).expect_err("oversized"),
            ImportCacheError::ManifestTooLarge
        );
    }

    #[test]
    fn an_unknown_field_is_a_conflict_rather_than_being_ignored() {
        let json = sample_manifest().to_json().expect("serialized");
        let extended = json.replacen('{', "{\n  \"source_path\": \"/home/user/media.iso\",", 1);
        assert_eq!(
            CacheManifest::parse(extended.as_bytes()).expect_err("unknown field"),
            ImportCacheError::ManifestConflict
        );
    }
}
