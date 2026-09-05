//! The transactional payload store: plans, sinks, completion, probe, publish.
//!
//! `ohl-platform` ports the two *primitives* a race-resistant publication
//! needs — a staging directory created exclusively, and a rename that never
//! replaces. This module is the transactional layer the C++
//! `AtomicDirectoryStore` puts on top of them, and it lives here rather than
//! in the platform crate because it is the payload plan vocabulary that
//! decides what a transaction contains.
//!
//! # The lifecycle
//!
//! [`PayloadStore::probe`] answers whether an identical payload is already
//! published. Otherwise [`PayloadStore::create_transaction`] hands out a
//! [`PayloadTransaction`] whose calls must follow one order:
//!
//! ```text
//! begin -> (open_file -> write_chunk* -> seal_file)* -> seal_completion
//!       -> publish_no_replace -> sync_published_parent
//! ```
//!
//! Every other order is [`PayloadStoreError::InvalidState`], including opening
//! a second file while one is open, opening an entry out of the plan's order,
//! sealing a file whose byte count does not equal its planned size, or
//! touching the transaction after publication. `abort` is valid from any
//! pre-publication state and is idempotent; after a successful publication it
//! is refused, because the published tree is no longer this transaction's to
//! remove.
//!
//! # On-disk shape
//!
//! ```text
//! <root>/ohl-tree-<hex of identity>/                           the payload
//! <root>/ohl-tree-<hex of identity>/files/                     the planned tree
//! <root>/ohl-tree-<hex of identity>/.ohl-payload-complete-v1   completion JSON
//! <root>/ohl-payload.<nonce>.staging/     an in-progress or abandoned stage
//! ```
//!
//! The identity is hex-encoded into the directory name so a name can never
//! carry a separator, a device name, or anything else a filesystem would
//! interpret. Completion metadata is written *last*, so a directory without it
//! is by construction incomplete.
//!
//! # Native safety
//!
//! Files are created with `create_new`, which fails rather than truncating an
//! existing name, and on Unix additionally with `O_NOFOLLOW`, so a symbolic
//! link planted at a destination name is an error and never a write through
//! it. Directories are created one component at a time and only when this
//! transaction has not created them already. The fsync policy matches the C++
//! one: each entry is synced before it is sealed, the completion metadata is
//! synced, the staged directories are synced deepest first, and after
//! publication the parent directory is synced so the new name itself is
//! durable — with the caveat `ohl-platform` states, that a completed sync
//! reports only that the backend's sync returned success.
//!
//! # Recovery
//!
//! A stage interrupted by a crash leaves a `*.staging` directory that no
//! process owns. [`DirectoryPayloadStore::discard_interrupted_stages`] finds
//! and removes them. It is deliberately *not* automatic: an abandoned stage is
//! never published (publication is a single rename of a completed tree), so it
//! costs only space, and a caller that may be racing another importer has to
//! decide for itself when sweeping is safe.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use ohl_platform::{AtomicDirectoryError, PublishOutcome, StagingDirectory};

/// The prefix of a published payload directory.
const PUBLISHED_PREFIX: &str = "ohl-tree-";

/// The staging-name prefix handed to [`StagingDirectory::create`].
const STAGING_PREFIX: &str = "ohl-payload";

/// The suffix [`StagingDirectory`] appends to every staging name.
const STAGING_SUFFIX: &str = ".staging";

/// The directory holding the planned tree inside a payload directory.
const FILES_DIRECTORY: &str = "files";

/// The completion metadata file name.
const COMPLETION_NAME: &str = ".ohl-payload-complete-v1";

/// The format tag inside the completion metadata.
const COMPLETION_FORMAT: &str = "ohl-payload-completion-v1";

/// The lowercase hexadecimal alphabet.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// The largest accepted completion metadata document, in bytes.
pub const MAXIMUM_COMPLETION_BYTES: usize = 4_096;

/// The largest accepted staging identity, in bytes.
pub const MAXIMUM_IDENTITY_BYTES: usize = 96;

/// The largest chunk a sink accepts in one call, in bytes.
pub const MAXIMUM_CHUNK_BYTES: usize = 1024 * 1024;

/// A sanitized store failure code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadStoreError {
    /// A call arrived out of order, or with a value the current state forbids.
    InvalidState,
    /// A native operation failed.
    IoFailure,
    /// A destination name is not one this store may write or publish.
    UnsafeDestination,
    /// A native resource limit was reached.
    ResourceExhausted,
    /// This target or filesystem has no no-replace publication operation.
    Unsupported,
}

impl PayloadStoreError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidState => "payload store operation was requested in an invalid state",
            Self::IoFailure => "payload store operation failed",
            Self::UnsafeDestination => "payload store destination is not writable",
            Self::ResourceExhausted => "a native resource limit was reached",
            Self::Unsupported => "no-replace payload publication is not supported here",
        }
    }
}

impl core::fmt::Display for PayloadStoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for PayloadStoreError {}

impl From<AtomicDirectoryError> for PayloadStoreError {
    fn from(error: AtomicDirectoryError) -> Self {
        match error {
            AtomicDirectoryError::InvalidState => Self::InvalidState,
            AtomicDirectoryError::UnsafeDestination => Self::UnsafeDestination,
            AtomicDirectoryError::ResourceExhausted => Self::ResourceExhausted,
            AtomicDirectoryError::Unsupported => Self::Unsupported,
            // `AtomicDirectoryError::IoFailure`, plus — because that enum is
            // `#[non_exhaustive]` — any code added later, until it is
            // classified here explicitly.
            _ => Self::IoFailure,
        }
    }
}

impl From<PayloadStoreError> for ohl_core::SanitizedError {
    fn from(error: PayloadStoreError) -> Self {
        match error {
            PayloadStoreError::InvalidState | PayloadStoreError::UnsafeDestination => {
                Self::InvalidInput
            }
            PayloadStoreError::Unsupported => Self::Unsupported,
            PayloadStoreError::IoFailure | PayloadStoreError::ResourceExhausted => Self::Internal,
        }
    }
}

/// One planned destination file, as a sequence of validated components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingEntry {
    /// The path components, outermost first. Never empty.
    pub components: Vec<String>,
    /// The exact size the entry must reach before it can be sealed.
    pub size_bytes: u64,
}

/// The complete description of one staging transaction.
///
/// Build one with [`StagingPlan::new`], which re-validates every component so
/// a store can never be handed a plan a layout planner would not produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingPlan {
    /// The identity that names the published directory.
    identity: String,
    /// The entries, in the order they will be written.
    entries: Vec<StagingEntry>,
    /// The exact sum of the entries' sizes.
    total_bytes: u64,
}

impl StagingPlan {
    /// Validates and builds a plan.
    ///
    /// # Errors
    ///
    /// [`PayloadStoreError::InvalidState`] for an empty or oversized identity,
    /// an identity with a byte outside the printable non-space ASCII set, an
    /// entry with no components, a component that is empty, `.`, `..`, or
    /// contains a separator or NUL, or a total size that would overflow.
    pub fn new(identity: &str, entries: Vec<StagingEntry>) -> Result<Self, PayloadStoreError> {
        if identity.is_empty()
            || identity.len() > MAXIMUM_IDENTITY_BYTES
            || !identity.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(PayloadStoreError::InvalidState);
        }
        let mut total_bytes = 0u64;
        for entry in &entries {
            if entry.components.is_empty() {
                return Err(PayloadStoreError::InvalidState);
            }
            for component in &entry.components {
                if !valid_component(component) {
                    return Err(PayloadStoreError::InvalidState);
                }
            }
            total_bytes = total_bytes
                .checked_add(entry.size_bytes)
                .ok_or(PayloadStoreError::InvalidState)?;
        }
        Ok(Self {
            identity: String::from(identity),
            entries,
            total_bytes,
        })
    }

    /// The plan's identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The plan's entries, in write order.
    pub fn entries(&self) -> &[StagingEntry] {
        &self.entries
    }

    /// The exact sum of the entries' sizes.
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// The published directory name for this plan.
    #[must_use]
    pub fn published_name(&self) -> String {
        published_directory_name(&self.identity).unwrap_or_default()
    }

    /// The exact completion metadata document for this plan.
    ///
    /// The identity is printable ASCII without `"` or `\` by construction, so
    /// the document needs no escaping and is byte-for-byte reproducible —
    /// which is what lets [`DirectoryPayloadStore::probe`] compare it exactly
    /// rather than parse it.
    fn completion_document(&self) -> String {
        format!(
            "{{\"format\":\"{COMPLETION_FORMAT}\",\"identity\":\"{}\",\"entry_count\":{},\"total_bytes\":{}}}",
            self.identity,
            self.entries.len(),
            self.total_bytes
        )
    }
}

/// Whether one path component may be resolved natively.
fn valid_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains('/')
        && !component.contains('\\')
        && !component.contains('\0')
}

/// The directory name a payload with `identity` is published under.
///
/// This is the one place the mapping lives, so a reader that only has an
/// identity — a runtime looking for an already-published tree, say — resolves
/// the same directory the staging protocol publishes.
///
/// Returns `None` for an identity that could never have been published: an
/// empty or oversized one, or one holding a byte outside printable non-space
/// ASCII.
#[must_use]
pub fn published_directory_name(identity: &str) -> Option<String> {
    if identity.is_empty()
        || identity.len() > MAXIMUM_IDENTITY_BYTES
        || !identity.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return None;
    }
    let mut name = String::from(PUBLISHED_PREFIX);
    for byte in identity.bytes() {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Some(name)
}

/// The directory holding a published payload's planned tree.
#[must_use]
pub fn published_files_directory(root: &Path, identity: &str) -> Option<PathBuf> {
    Some(
        root.join(published_directory_name(identity)?)
            .join(FILES_DIRECTORY),
    )
}

/// What a probe found at a plan's published name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbeState {
    /// Nothing is there.
    Absent,
    /// The exact planned tree and completion metadata are there.
    Matching,
    /// Something is there that is not this plan's payload: an incomplete tree,
    /// an extra or missing entry, a wrong size, or a wrong type.
    Conflict,
}

/// What a publication attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublishState {
    /// The staged tree is now the published payload. This is the commit point.
    Published,
    /// The destination already existed; nothing was changed.
    DestinationExists,
}

/// A transactional payload staging session.
///
/// See the [module documentation](self) for the required call order.
pub trait PayloadTransaction {
    /// Starts staging `plan`.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn begin(&mut self, plan: &StagingPlan) -> Result<(), PayloadStoreError>;

    /// Opens the next planned entry for writing.
    ///
    /// `components` and `expected_size` must equal the plan's next entry.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn open_file(
        &mut self,
        components: &[String],
        expected_size: u64,
    ) -> Result<(), PayloadStoreError>;

    /// Writes one whole chunk to the open entry.
    ///
    /// # Errors
    ///
    /// [`PayloadStoreError::InvalidState`] when no entry is open, when the
    /// chunk exceeds [`MAXIMUM_CHUNK_BYTES`], or when it would exceed the
    /// entry's expected size.
    fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), PayloadStoreError>;

    /// Closes the open entry, which must have reached its exact size.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn seal_file(&mut self) -> Result<(), PayloadStoreError>;

    /// Writes the completion metadata, after every entry is sealed.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn seal_completion(&mut self) -> Result<(), PayloadStoreError>;

    /// Attempts the single atomic no-replace publication.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`]. An error leaves the destination untouched
    /// and the staging tree owned and abortable.
    fn publish_no_replace(&mut self) -> Result<PublishState, PayloadStoreError>;

    /// Syncs the parent of the published name.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`]. A failure here cannot undo publication.
    fn sync_published_parent(&mut self) -> Result<(), PayloadStoreError>;

    /// Discards everything this transaction staged. Idempotent.
    ///
    /// # Errors
    ///
    /// [`PayloadStoreError::IoFailure`] when the staging tree could not be
    /// removed; it is then left for diagnosis.
    /// [`PayloadStoreError::InvalidState`] after a successful publication,
    /// which this transaction may never remove.
    fn abort(&mut self) -> Result<(), PayloadStoreError>;
}

/// A store of published payloads.
pub trait PayloadStore {
    /// Reports whether `plan`'s payload is already published.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn probe(&mut self, plan: &StagingPlan) -> Result<ProbeState, PayloadStoreError>;

    /// Creates a transaction.
    ///
    /// # Errors
    ///
    /// See [`PayloadStoreError`].
    fn create_transaction(&mut self)
    -> Result<Box<dyn PayloadTransaction + '_>, PayloadStoreError>;
}

/// A payload store rooted at one trusted local directory.
///
/// The root must be trusted against mutation by untrusted processes running as
/// the same effective user for the store's lifetime, exactly as
/// `ohl_platform::atomic_directory` requires: neither Linux nor Windows offers
/// a conditional unlink-by-identity that would make cleanup safe otherwise.
#[derive(Debug)]
pub struct DirectoryPayloadStore {
    /// The trusted root that holds published and staging directories.
    root: PathBuf,
}

impl DirectoryPayloadStore {
    /// Opens a store at an existing directory.
    ///
    /// # Errors
    ///
    /// [`PayloadStoreError::UnsafeDestination`] when `root` is absent or is
    /// not a directory, and [`PayloadStoreError::IoFailure`] when it cannot be
    /// inspected.
    pub fn open(root: &Path) -> Result<Self, PayloadStoreError> {
        let metadata = fs::symlink_metadata(root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                PayloadStoreError::UnsafeDestination
            } else {
                PayloadStoreError::IoFailure
            }
        })?;
        if !metadata.is_dir() {
            return Err(PayloadStoreError::UnsafeDestination);
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// The trusted root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Removes every abandoned staging directory under the root.
    ///
    /// Returns how many were removed. See the [module documentation](self) for
    /// why this is explicit rather than automatic.
    ///
    /// # Errors
    ///
    /// [`PayloadStoreError::IoFailure`] when the root cannot be listed, or
    /// when a staging tree could not be removed. Removal stops at the first
    /// failure so the offending tree stays available for diagnosis.
    pub fn discard_interrupted_stages(&self) -> Result<usize, PayloadStoreError> {
        let mut discarded = 0usize;
        for entry in fs::read_dir(&self.root).map_err(|_| PayloadStoreError::IoFailure)? {
            let entry = entry.map_err(|_| PayloadStoreError::IoFailure)?;
            let Some(name) = entry.file_name().to_str().map(String::from) else {
                continue;
            };
            if !name.starts_with(STAGING_PREFIX) || !name.ends_with(STAGING_SUFFIX) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|_| PayloadStoreError::IoFailure)?
                .is_dir()
            {
                continue;
            }
            fs::remove_dir_all(entry.path()).map_err(|_| PayloadStoreError::IoFailure)?;
            discarded += 1;
        }
        Ok(discarded)
    }

    /// Whether the published directory for `plan` matches it exactly.
    fn published_matches(directory: &Path, plan: &StagingPlan) -> Result<bool, PayloadStoreError> {
        let mut root_names = BTreeSet::new();
        for entry in fs::read_dir(directory).map_err(|_| PayloadStoreError::IoFailure)? {
            let entry = entry.map_err(|_| PayloadStoreError::IoFailure)?;
            root_names.insert(entry.file_name());
        }
        let expected_names = BTreeSet::from([
            std::ffi::OsString::from(COMPLETION_NAME),
            std::ffi::OsString::from(FILES_DIRECTORY),
        ]);
        if root_names != expected_names {
            return Ok(false);
        }

        let completion = directory.join(COMPLETION_NAME);
        let metadata =
            fs::symlink_metadata(&completion).map_err(|_| PayloadStoreError::IoFailure)?;
        let expected_document = plan.completion_document();
        let expected_length = u64::try_from(expected_document.len()).unwrap_or(u64::MAX);
        if !metadata.is_file() || metadata.len() != expected_length {
            return Ok(false);
        }
        if fs::read(&completion).map_err(|_| PayloadStoreError::IoFailure)?
            != expected_document.as_bytes()
        {
            return Ok(false);
        }

        let mut actual_files = BTreeSet::new();
        let mut actual_directories = BTreeSet::new();
        if !collect_tree(
            &directory.join(FILES_DIRECTORY),
            &mut String::new(),
            &mut actual_files,
            &mut actual_directories,
        )? {
            return Ok(false);
        }

        let mut expected_files = BTreeSet::new();
        let mut expected_directories = BTreeSet::new();
        for entry in plan.entries() {
            let mut parent = String::new();
            for component in &entry.components[..entry.components.len() - 1] {
                if !parent.is_empty() {
                    parent.push('/');
                }
                parent.push_str(component);
                expected_directories.insert(parent.clone());
            }
            expected_files.insert((entry.components.join("/"), entry.size_bytes));
        }
        Ok(actual_files == expected_files && actual_directories == expected_directories)
    }
}

/// Walks a published `files` tree, collecting relative names and sizes.
///
/// Returns `false` as soon as anything that is not a plain directory or plain
/// file is found: a symbolic link, a device node, or an unrepresentable name is
/// a conflict, not a payload.
fn collect_tree(
    directory: &Path,
    prefix: &mut String,
    files: &mut BTreeSet<(String, u64)>,
    directories: &mut BTreeSet<String>,
) -> Result<bool, PayloadStoreError> {
    let listing = match fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(PayloadStoreError::IoFailure),
    };
    for entry in listing {
        let entry = entry.map_err(|_| PayloadStoreError::IoFailure)?;
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            return Ok(false);
        };
        if !valid_component(&name) {
            return Ok(false);
        }
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|_| PayloadStoreError::IoFailure)?;
        let previous = prefix.len();
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(&name);
        let accepted = if metadata.is_dir() {
            directories.insert(prefix.clone());
            collect_tree(&entry.path(), prefix, files, directories)?
        } else if metadata.is_file() {
            files.insert((prefix.clone(), metadata.len()));
            true
        } else {
            false
        };
        prefix.truncate(previous);
        if !accepted {
            return Ok(false);
        }
    }
    Ok(true)
}

impl PayloadStore for DirectoryPayloadStore {
    fn probe(&mut self, plan: &StagingPlan) -> Result<ProbeState, PayloadStoreError> {
        let published = self.root.join(plan.published_name());
        let metadata = match fs::symlink_metadata(&published) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProbeState::Absent);
            }
            Err(_) => return Err(PayloadStoreError::IoFailure),
        };
        if !metadata.is_dir() {
            return Ok(ProbeState::Conflict);
        }
        Ok(if Self::published_matches(&published, plan)? {
            ProbeState::Matching
        } else {
            ProbeState::Conflict
        })
    }

    fn create_transaction(
        &mut self,
    ) -> Result<Box<dyn PayloadTransaction + '_>, PayloadStoreError> {
        Ok(Box::new(DirectoryTransaction {
            root: self.root.clone(),
            state: TransactionState::Created,
            staging: None,
            plan: None,
            created_directories: BTreeSet::new(),
            next_entry: 0,
            open_file: None,
        }))
    }
}

/// Where a [`DirectoryTransaction`] is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionState {
    /// `begin` has not been called.
    Created,
    /// Entries may be opened and sealed.
    Staging,
    /// Completion metadata is written; publication is the only step left.
    Ready,
    /// Published. Nothing further may mutate it, including `abort`.
    Published,
    /// Aborted.
    Finished,
}

/// The file currently open for writing.
#[derive(Debug)]
struct OpenFile {
    /// The native handle.
    handle: fs::File,
    /// The exact size the entry must reach.
    expected_size: u64,
    /// Bytes written so far.
    written: u64,
}

/// A transaction over one exclusively created staging directory.
#[derive(Debug)]
struct DirectoryTransaction {
    /// The trusted root, for publication.
    root: PathBuf,
    /// The lifecycle position.
    state: TransactionState,
    /// The exclusively created staging directory, until publication.
    staging: Option<StagingDirectory>,
    /// The plan being staged.
    plan: Option<StagingPlan>,
    /// Relative directory names this transaction created under `files`.
    created_directories: BTreeSet<String>,
    /// How many planned entries have been sealed.
    next_entry: usize,
    /// The open entry, if any.
    open_file: Option<OpenFile>,
}

impl DirectoryTransaction {
    /// The staging directory, or [`PayloadStoreError::InvalidState`].
    fn staging_path(&self) -> Result<&Path, PayloadStoreError> {
        self.staging
            .as_ref()
            .map(StagingDirectory::path)
            .ok_or(PayloadStoreError::InvalidState)
    }

    /// Creates the parent directories of one entry under `files` and returns
    /// the entry's own path.
    fn create_parents(&mut self, components: &[String]) -> Result<PathBuf, PayloadStoreError> {
        let mut path = self.staging_path()?.join(FILES_DIRECTORY);
        let mut relative = String::new();
        for component in &components[..components.len() - 1] {
            if !relative.is_empty() {
                relative.push('/');
            }
            relative.push_str(component);
            path.push(component);
            if self.created_directories.insert(relative.clone()) {
                // Create-new: a name this transaction has not created must not
                // already exist, so nothing pre-planted can be written into.
                fs::create_dir(&path).map_err(|error| map_io(&error))?;
            }
        }
        path.push(components[components.len() - 1].as_str());
        Ok(path)
    }
}

/// Classifies a native error without carrying any of its detail.
fn map_io(error: &std::io::Error) -> PayloadStoreError {
    match error.kind() {
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied => {
            PayloadStoreError::UnsafeDestination
        }
        std::io::ErrorKind::OutOfMemory | std::io::ErrorKind::StorageFull => {
            PayloadStoreError::ResourceExhausted
        }
        _ => PayloadStoreError::IoFailure,
    }
}

/// `O_NOFOLLOW` for the current Unix target.
///
/// Taken from the platform headers rather than a C binding, so this crate
/// keeps `forbid(unsafe_code)` and needs no `libc` dependency.
#[cfg(unix)]
const fn o_nofollow() -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        0o400_000
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        0x0100
    }
}

/// Creates one file that must not already exist and must not be a link.
fn create_new_file(path: &Path) -> Result<fs::File, PayloadStoreError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // `create_new` already refuses an existing name, and `O_NOFOLLOW`
        // makes a planted symbolic link an error instead of a write through
        // it — the two together are what make a destination name safe.
        options.custom_flags(o_nofollow());
    }
    options.open(path).map_err(|error| map_io(&error))
}

/// Syncs a directory so a name created inside it is durable.
///
/// Only Unix lets a directory be opened and synced; on other targets the
/// publication rename's own durability is all the backend offers.
// The non-Unix body never fails, but the signature is shared: making it
// infallible per target would push a `cfg` into every call site.
#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
fn sync_directory(path: &Path) -> Result<(), PayloadStoreError> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .map_err(|error| map_io(&error))?
            .sync_all()
            .map_err(|error| map_io(&error))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

impl PayloadTransaction for DirectoryTransaction {
    fn begin(&mut self, plan: &StagingPlan) -> Result<(), PayloadStoreError> {
        if self.state != TransactionState::Created {
            return Err(PayloadStoreError::InvalidState);
        }
        let staging = StagingDirectory::create(&self.root, STAGING_PREFIX)?;
        fs::create_dir(staging.path().join(FILES_DIRECTORY)).map_err(|error| map_io(&error))?;
        self.staging = Some(staging);
        self.plan = Some(plan.clone());
        self.state = TransactionState::Staging;
        Ok(())
    }

    fn open_file(
        &mut self,
        components: &[String],
        expected_size: u64,
    ) -> Result<(), PayloadStoreError> {
        if self.state != TransactionState::Staging || self.open_file.is_some() {
            return Err(PayloadStoreError::InvalidState);
        }
        let plan = self.plan.as_ref().ok_or(PayloadStoreError::InvalidState)?;
        let expected = plan
            .entries()
            .get(self.next_entry)
            .ok_or(PayloadStoreError::InvalidState)?;
        if expected.components != components || expected.size_bytes != expected_size {
            return Err(PayloadStoreError::InvalidState);
        }
        let path = self.create_parents(components)?;
        self.open_file = Some(OpenFile {
            handle: create_new_file(&path)?,
            expected_size,
            written: 0,
        });
        Ok(())
    }

    fn write_chunk(&mut self, bytes: &[u8]) -> Result<(), PayloadStoreError> {
        if self.state != TransactionState::Staging || bytes.len() > MAXIMUM_CHUNK_BYTES {
            return Err(PayloadStoreError::InvalidState);
        }
        let open = self
            .open_file
            .as_mut()
            .ok_or(PayloadStoreError::InvalidState)?;
        let offered = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if offered > open.expected_size - open.written {
            return Err(PayloadStoreError::InvalidState);
        }
        // `write_all` retries a short write, so the chunk is accepted whole or
        // not at all, as the sink contract requires.
        open.handle
            .write_all(bytes)
            .map_err(|error| map_io(&error))?;
        open.written += offered;
        Ok(())
    }

    fn seal_file(&mut self) -> Result<(), PayloadStoreError> {
        if self.state != TransactionState::Staging {
            return Err(PayloadStoreError::InvalidState);
        }
        let open = self
            .open_file
            .take()
            .ok_or(PayloadStoreError::InvalidState)?;
        if open.written != open.expected_size {
            return Err(PayloadStoreError::InvalidState);
        }
        open.handle.sync_all().map_err(|error| map_io(&error))?;
        drop(open.handle);
        self.next_entry += 1;
        Ok(())
    }

    fn seal_completion(&mut self) -> Result<(), PayloadStoreError> {
        let plan = self.plan.as_ref().ok_or(PayloadStoreError::InvalidState)?;
        if self.state != TransactionState::Staging
            || self.open_file.is_some()
            || self.next_entry != plan.entries().len()
        {
            return Err(PayloadStoreError::InvalidState);
        }
        let document = plan.completion_document();
        if document.len() > MAXIMUM_COMPLETION_BYTES {
            return Err(PayloadStoreError::InvalidState);
        }
        let staging = self.staging_path()?.to_path_buf();
        let mut file = create_new_file(&staging.join(COMPLETION_NAME))?;
        file.write_all(document.as_bytes())
            .map_err(|error| map_io(&error))?;
        file.sync_all().map_err(|error| map_io(&error))?;
        drop(file);

        // Directories are synced deepest first, then the staging root, so the
        // completion marker cannot become visible before the tree it seals.
        let mut ordered = self.created_directories.iter().cloned().collect::<Vec<_>>();
        ordered.sort_by_key(|relative| core::cmp::Reverse(relative.matches('/').count()));
        for relative in ordered {
            let mut path = staging.join(FILES_DIRECTORY);
            for component in relative.split('/') {
                path.push(component);
            }
            sync_directory(&path)?;
        }
        sync_directory(&staging.join(FILES_DIRECTORY))?;
        sync_directory(&staging)?;
        self.state = TransactionState::Ready;
        Ok(())
    }

    fn publish_no_replace(&mut self) -> Result<PublishState, PayloadStoreError> {
        if self.state != TransactionState::Ready {
            return Err(PayloadStoreError::InvalidState);
        }
        let destination = self.root.join(
            self.plan
                .as_ref()
                .ok_or(PayloadStoreError::InvalidState)?
                .published_name(),
        );
        let staging = self.staging.take().ok_or(PayloadStoreError::InvalidState)?;
        match staging.publish_no_replace(&destination) {
            Ok(PublishOutcome::Published) => {
                self.state = TransactionState::Published;
                Ok(PublishState::Published)
            }
            Ok(PublishOutcome::DestinationExists(returned)) => {
                // The loser keeps ownership of its untouched staging tree so
                // it can still be aborted deliberately.
                self.staging = Some(returned);
                Ok(PublishState::DestinationExists)
            }
            Err((returned, error)) => {
                self.staging = Some(returned);
                Err(error.into())
            }
        }
    }

    fn sync_published_parent(&mut self) -> Result<(), PayloadStoreError> {
        if self.state != TransactionState::Published {
            return Err(PayloadStoreError::InvalidState);
        }
        sync_directory(&self.root)
    }

    fn abort(&mut self) -> Result<(), PayloadStoreError> {
        if self.state == TransactionState::Published {
            return Err(PayloadStoreError::InvalidState);
        }
        self.open_file = None;
        let staging = self.staging.take();
        self.state = TransactionState::Finished;
        match staging {
            None => Ok(()),
            Some(staging) => staging.abort().map_err(PayloadStoreError::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DirectoryPayloadStore, MAXIMUM_CHUNK_BYTES, MAXIMUM_COMPLETION_BYTES,
        MAXIMUM_IDENTITY_BYTES, PayloadStore, PayloadStoreError, ProbeState, PublishState,
        StagingEntry, StagingPlan,
    };
    use std::fs;

    fn entry(path: &str, size: u64) -> StagingEntry {
        StagingEntry {
            components: path.split('/').map(String::from).collect(),
            size_bytes: size,
        }
    }

    fn plan() -> StagingPlan {
        StagingPlan::new(
            "ohl-payload-test-identity",
            vec![entry("Nested/inner.bin", 3), entry("outer.bin", 2)],
        )
        .expect("valid plan")
    }

    fn stage(store: &mut DirectoryPayloadStore, plan: &StagingPlan) -> PublishState {
        let mut transaction = store.create_transaction().expect("transaction");
        transaction.begin(plan).expect("begin");
        for (index, entry) in plan.entries().iter().enumerate() {
            transaction
                .open_file(&entry.components, entry.size_bytes)
                .expect("open");
            let size = usize::try_from(entry.size_bytes).expect("test entry size");
            let content = vec![b'a' + u8::try_from(index).expect("index"); size];
            transaction.write_chunk(&content).expect("write");
            transaction.seal_file().expect("seal");
        }
        transaction.seal_completion().expect("completion");
        let state = transaction.publish_no_replace().expect("publish");
        if state == PublishState::Published {
            transaction.sync_published_parent().expect("sync");
        }
        state
    }

    #[test]
    fn a_plan_validates_its_identity_and_components() {
        assert!(StagingPlan::new("", Vec::new()).is_err());
        assert!(StagingPlan::new(&"a".repeat(MAXIMUM_IDENTITY_BYTES + 1), Vec::new()).is_err());
        assert!(StagingPlan::new("has space", Vec::new()).is_err());
        assert!(StagingPlan::new("id", vec![entry("", 0)]).is_err());
        for bad in ["..", ".", "a\0b", "a/b", "a\\b"] {
            assert!(
                StagingPlan::new(
                    "id",
                    vec![StagingEntry {
                        components: vec![String::from(bad)],
                        size_bytes: 0,
                    }]
                )
                .is_err(),
                "component `{bad}` accepted"
            );
        }
        assert!(
            StagingPlan::new("id", vec![entry("a", u64::MAX), entry("b", 1)]).is_err(),
            "overflowing total accepted"
        );

        let valid = plan();
        assert_eq!(valid.total_bytes(), 5);
        assert_eq!(valid.identity(), "ohl-payload-test-identity");
        assert!(valid.published_name().starts_with("ohl-tree-"));
        assert!(
            valid
                .published_name()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        );
    }

    #[test]
    fn a_staged_payload_publishes_and_then_probes_as_matching() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        assert_eq!(store.probe(&plan).expect("probe"), ProbeState::Absent);
        assert_eq!(stage(&mut store, &plan), PublishState::Published);
        assert_eq!(store.probe(&plan).expect("probe"), ProbeState::Matching);

        let published = root.path().join(plan.published_name());
        assert_eq!(
            fs::read(published.join("files/Nested/inner.bin")).expect("entry"),
            b"aaa"
        );
        assert_eq!(
            fs::read(published.join("files/outer.bin")).expect("entry"),
            b"bb"
        );
        let completion =
            fs::read_to_string(published.join(".ohl-payload-complete-v1")).expect("completion");
        assert!(completion.contains("\"entry_count\":2"));
        assert!(completion.contains("\"total_bytes\":5"));
        assert!(completion.contains("ohl-payload-completion-v1"));
        assert!(completion.len() <= MAXIMUM_COMPLETION_BYTES);
    }

    #[test]
    fn publication_refuses_to_replace_an_existing_payload() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        assert_eq!(stage(&mut store, &plan), PublishState::Published);
        let published = root.path().join(plan.published_name());
        let original = fs::read(published.join("files/outer.bin")).expect("entry");

        assert_eq!(stage(&mut store, &plan), PublishState::DestinationExists);
        assert_eq!(
            fs::read(published.join("files/outer.bin")).expect("entry"),
            original
        );
        // The loser's staging tree is gone once its transaction is dropped.
        assert_eq!(
            store.discard_interrupted_stages().expect("sweep"),
            0,
            "a dropped loser leaves nothing behind"
        );
    }

    #[test]
    fn a_tampered_published_tree_probes_as_a_conflict() {
        let plan = plan();
        for tamper in [
            "extra",
            "remove-file",
            "resize-file",
            "remove-completion",
            "rewrite-completion",
            "extra-directory",
        ] {
            let root = tempfile::tempdir().expect("temporary directory");
            let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
            assert_eq!(stage(&mut store, &plan), PublishState::Published);
            let published = root.path().join(plan.published_name());
            match tamper {
                "extra" => {
                    fs::write(published.join("files/extra.bin"), b"x").expect("tamper");
                }
                "remove-file" => {
                    fs::remove_file(published.join("files/outer.bin")).expect("tamper");
                }
                "resize-file" => {
                    fs::write(published.join("files/outer.bin"), b"xxx").expect("tamper");
                }
                "remove-completion" => {
                    fs::remove_file(published.join(".ohl-payload-complete-v1")).expect("tamper");
                }
                "extra-directory" => {
                    fs::create_dir(published.join("files/extra")).expect("tamper");
                }
                _ => fs::write(published.join(".ohl-payload-complete-v1"), b"{}").expect("tamper"),
            }
            assert_eq!(
                store.probe(&plan).expect("probe"),
                ProbeState::Conflict,
                "tamper `{tamper}` was not detected"
            );
        }
    }

    #[test]
    fn a_non_directory_at_the_published_name_is_a_conflict() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        fs::write(root.path().join(plan.published_name()), b"not a payload").expect("file");
        assert_eq!(store.probe(&plan).expect("probe"), ProbeState::Conflict);
    }

    #[test]
    fn an_interrupted_stage_is_detected_and_discarded() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();

        // Simulate a crash after the first chunk: the staging tree is
        // deliberately leaked, exactly as a killed process would leave it.
        {
            let mut transaction = store.create_transaction().expect("transaction");
            transaction.begin(&plan).expect("begin");
            let first = &plan.entries()[0];
            transaction
                .open_file(&first.components, first.size_bytes)
                .expect("open");
            transaction.write_chunk(b"aaa").expect("write");
            transaction.seal_file().expect("seal");
            std::mem::forget(transaction);
        }

        // Nothing was published, and the abandoned tree is still there.
        assert_eq!(store.probe(&plan).expect("probe"), ProbeState::Absent);
        let leaked = fs::read_dir(root.path())
            .expect("listing")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".staging"))
            .count();
        assert_eq!(leaked, 1);

        assert_eq!(store.discard_interrupted_stages().expect("sweep"), 1);
        assert_eq!(store.discard_interrupted_stages().expect("sweep"), 0);
        assert_eq!(fs::read_dir(root.path()).expect("listing").count(), 0);

        // A fresh stage after recovery still publishes.
        assert_eq!(stage(&mut store, &plan), PublishState::Published);
        assert_eq!(store.probe(&plan).expect("probe"), ProbeState::Matching);
    }

    #[test]
    fn every_out_of_order_call_is_refused() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        let first = plan.entries()[0].clone();
        let mut transaction = store.create_transaction().expect("transaction");

        assert_eq!(
            transaction
                .open_file(&first.components, first.size_bytes)
                .expect_err("before begin"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.seal_completion().expect_err("before begin"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.publish_no_replace().expect_err("before begin"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction
                .sync_published_parent()
                .expect_err("before publication"),
            PayloadStoreError::InvalidState
        );

        transaction.begin(&plan).expect("begin");
        assert_eq!(
            transaction.begin(&plan).expect_err("twice"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.write_chunk(b"a").expect_err("no open file"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.seal_file().expect_err("no open file"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.seal_completion().expect_err("entries pending"),
            PayloadStoreError::InvalidState
        );
        // Entries must be opened in the plan's order, with their exact size.
        assert_eq!(
            transaction
                .open_file(&plan.entries()[1].components, plan.entries()[1].size_bytes)
                .expect_err("out of order"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction
                .open_file(&first.components, first.size_bytes + 1)
                .expect_err("wrong size"),
            PayloadStoreError::InvalidState
        );

        transaction
            .open_file(&first.components, first.size_bytes)
            .expect("open");
        assert_eq!(
            transaction
                .open_file(&first.components, first.size_bytes)
                .expect_err("second open file"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction.write_chunk(b"aaaa").expect_err("overflow"),
            PayloadStoreError::InvalidState
        );
        assert_eq!(
            transaction
                .write_chunk(&vec![0u8; MAXIMUM_CHUNK_BYTES + 1])
                .expect_err("oversized chunk"),
            PayloadStoreError::InvalidState
        );
        transaction.write_chunk(b"aa").expect("write");
        assert_eq!(
            transaction.seal_file().expect_err("short entry"),
            PayloadStoreError::InvalidState
        );
    }

    #[test]
    fn abort_is_idempotent_and_never_removes_a_published_payload() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        {
            let mut transaction = store.create_transaction().expect("transaction");
            transaction.begin(&plan).expect("begin");
            transaction.abort().expect("abort");
            transaction.abort().expect("abort again");
        }
        assert_eq!(fs::read_dir(root.path()).expect("listing").count(), 0);

        let mut transaction = store.create_transaction().expect("transaction");
        transaction.begin(&plan).expect("begin");
        for entry in plan.entries() {
            transaction
                .open_file(&entry.components, entry.size_bytes)
                .expect("open");
            let size = usize::try_from(entry.size_bytes).expect("test entry size");
            transaction.write_chunk(&vec![b'z'; size]).expect("write");
            transaction.seal_file().expect("seal");
        }
        transaction.seal_completion().expect("completion");
        assert_eq!(
            transaction.publish_no_replace().expect("publish"),
            PublishState::Published
        );
        assert_eq!(
            transaction.abort().expect_err("after publication"),
            PayloadStoreError::InvalidState
        );
        drop(transaction);
        assert!(root.path().join(plan.published_name()).is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_planted_at_a_destination_name_is_never_followed() {
        let root = tempfile::tempdir().expect("temporary directory");
        let target = tempfile::tempdir().expect("target directory");
        let victim = target.path().join("victim.bin");
        fs::write(&victim, b"original").expect("victim");

        let mut store = DirectoryPayloadStore::open(root.path()).expect("store");
        let plan = plan();
        let mut transaction = store.create_transaction().expect("transaction");
        transaction.begin(&plan).expect("begin");

        // Plant the link where the first entry's file will be created. It has
        // to go under the staging tree, which only the transaction knows, so
        // find it the way an attacker with directory access would.
        let staging = fs::read_dir(root.path())
            .expect("listing")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.to_string_lossy().ends_with(".staging"))
            .expect("staging directory");
        fs::create_dir(staging.join("files/Nested")).expect("parent");
        std::os::unix::fs::symlink(&victim, staging.join("files/Nested/inner.bin"))
            .expect("plant link");

        let first = &plan.entries()[0];
        let error = transaction
            .open_file(&first.components, first.size_bytes)
            .expect_err("a planted link must not be opened");
        assert_eq!(error, PayloadStoreError::UnsafeDestination);
        assert_eq!(fs::read(&victim).expect("victim"), b"original");
    }

    #[test]
    fn a_store_root_must_be_an_existing_directory() {
        let root = tempfile::tempdir().expect("temporary directory");
        assert_eq!(
            DirectoryPayloadStore::open(&root.path().join("absent")).expect_err("refused"),
            PayloadStoreError::UnsafeDestination
        );
        let file = root.path().join("file");
        fs::write(&file, b"x").expect("file");
        assert_eq!(
            DirectoryPayloadStore::open(&file).expect_err("refused"),
            PayloadStoreError::UnsafeDestination
        );
        let store = DirectoryPayloadStore::open(root.path()).expect("store");
        assert_eq!(store.root(), root.path());
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            PayloadStoreError::InvalidState,
            PayloadStoreError::IoFailure,
            PayloadStoreError::UnsafeDestination,
            PayloadStoreError::ResourceExhausted,
            PayloadStoreError::Unsupported,
        ] {
            assert!(!error.to_string().is_empty());
            let _: ohl_core::SanitizedError = error.into();
        }
    }
}
