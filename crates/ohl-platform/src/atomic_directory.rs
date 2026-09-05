//! Race-resistant directory publication primitives.
//!
//! # Scope of this module
//!
//! The C++ tree's `AtomicDirectoryStore` is a full transactional interface
//! (plan/probe/open/seal/publish/abort with a 1200-line POSIX backend). Only
//! its two *primitives* are ported here, because they are the part every
//! other package needs and the part whose guarantees are platform-specific:
//!
//! 1. staging a private directory that is created exclusively, so two racing
//!    writers can never share one;
//! 2. publishing that directory to its final name with an operation that
//!    **never replaces** an existing destination.
//!
//! The transactional layer above them — plans, entry sinks, completion
//! metadata, probing, and recovery — is deliberately **not** ported in this
//! package; it belongs with the import/cache subsystem that defines the plan
//! vocabulary. [`StagingDirectory`] is the interface that layer will build
//! on, so it can be added without changing anything here.
//!
//! # Exact publication guarantees per platform
//!
//! | Target | Operation | Guarantee |
//! |--------|-----------|-----------|
//! | Linux | `renameat2(..., RENAME_NOREPLACE)` | Atomic. The kernel fails with `EEXIST` if the destination exists, so there is no window between the check and the rename. Older kernels and filesystems without `renameat2` support report [`AtomicDirectoryError::Unsupported`] rather than silently degrading. |
//! | macOS | `renameatx_np(..., RENAME_EXCL)` | Atomic, same contract as Linux. Filesystems that do not implement it report [`AtomicDirectoryError::Unsupported`]. |
//! | Windows | [`std::fs::rename`] | Not atomic against an arbitrary destination, but safe for the case that matters: `std::fs::rename` maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`, and `MOVEFILE_REPLACE_EXISTING` **cannot replace an existing directory** — the call fails. A destination that exists as a *file* would be replaced, so this backend additionally refuses up front if the destination exists at all. That pre-check is advisory (it can be raced); the directory case is enforced by the OS. |
//! | Other | — | [`AtomicDirectoryError::Unsupported`]. |
//!
//! On no platform does a failed publication touch the destination, and on no
//! platform does a successful publication remove anything.
//!
//! # Trust requirements
//!
//! As in C++: the parent directory of the staging and destination names must
//! be trusted against mutation by untrusted processes running as the same
//! effective user for the lifetime of the operation. Neither platform offers
//! a conditional unlink-by-identity that would let this code clean up safely
//! in a hostile namespace.

use std::fs;
use std::path::{Path, PathBuf};

use ohl_core::SanitizedError;

/// The number of distinct staging names tried before giving up.
const STAGING_ATTEMPTS: u32 = 128;

/// A sanitized atomic-directory failure code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AtomicDirectoryError {
    /// The caller asked for something the primitive cannot express, such as a
    /// destination with no parent directory or a non-final component in a
    /// name.
    InvalidState,
    /// A native operation failed.
    IoFailure,
    /// The destination is not a name this primitive may publish to.
    UnsafeDestination,
    /// A native resource limit was reached, including exhausting the staging
    /// name attempts.
    ResourceExhausted,
    /// This target or filesystem has no no-replace publication operation.
    Unsupported,
}

impl AtomicDirectoryError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::InvalidState => "atomic directory operation was requested in an invalid state",
            Self::IoFailure => "atomic directory operation failed",
            Self::UnsafeDestination => "atomic directory destination is not publishable",
            Self::ResourceExhausted => "a native resource limit was reached",
            Self::Unsupported => "no-replace directory publication is not supported here",
        }
    }
}

impl core::fmt::Display for AtomicDirectoryError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for AtomicDirectoryError {}

impl From<AtomicDirectoryError> for SanitizedError {
    fn from(error: AtomicDirectoryError) -> Self {
        match error {
            AtomicDirectoryError::InvalidState | AtomicDirectoryError::UnsafeDestination => {
                Self::InvalidInput
            }
            AtomicDirectoryError::Unsupported => Self::Unsupported,
            AtomicDirectoryError::IoFailure | AtomicDirectoryError::ResourceExhausted => {
                Self::Internal
            }
        }
    }
}

/// The result of an attempted publication.
#[derive(Debug)]
pub enum PublishOutcome {
    /// The staging directory now *is* the destination. This is the commit
    /// point; the staging directory no longer exists under its old name, and
    /// the published destination can no longer be removed by this crate.
    Published,
    /// The destination already existed. Nothing was changed, and ownership of
    /// the untouched staging directory is handed back so the loser of the
    /// race can inspect, retry, or abort it. Dropping it removes it.
    DestinationExists(StagingDirectory),
}

/// A privately owned, exclusively created staging directory.
///
/// Dropping the value without publishing removes the directory and its
/// contents on a best-effort basis; call [`StagingDirectory::abort`] instead
/// when the removal outcome matters. After a successful
/// [`StagingDirectory::publish_no_replace`] the value is consumed, so a
/// published destination can never be removed by this type.
#[derive(Debug)]
pub struct StagingDirectory {
    /// The staging path. Only this type resolves it.
    path: PathBuf,
}

impl StagingDirectory {
    /// Creates a new staging directory inside `parent`.
    ///
    /// Each candidate name is created with `mkdir`, which fails if the name
    /// already exists, so the directory is owned exclusively by this caller:
    /// no check-then-create window exists. `prefix` must be a single
    /// component with no separators.
    ///
    /// # Errors
    ///
    /// [`AtomicDirectoryError::InvalidState`] for an unusable `prefix`,
    /// [`AtomicDirectoryError::ResourceExhausted`] when every attempted name
    /// was taken, and [`AtomicDirectoryError::IoFailure`] otherwise.
    pub fn create(parent: &Path, prefix: &str) -> Result<Self, AtomicDirectoryError> {
        if prefix.is_empty()
            || prefix.contains('/')
            || prefix.contains('\\')
            || prefix.contains('\0')
        {
            return Err(AtomicDirectoryError::InvalidState);
        }
        if parent.as_os_str().is_empty() {
            return Err(AtomicDirectoryError::InvalidState);
        }

        for attempt in 0..STAGING_ATTEMPTS {
            let candidate = parent.join(format!("{prefix}.{}.staging", staging_nonce(attempt)));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self { path: candidate }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) if error.kind() == std::io::ErrorKind::OutOfMemory => {
                    return Err(AtomicDirectoryError::ResourceExhausted);
                }
                Err(_) => return Err(AtomicDirectoryError::IoFailure),
            }
        }
        Err(AtomicDirectoryError::ResourceExhausted)
    }

    /// The staging directory's path, for writing content into it.
    ///
    /// Callers must only create entries *below* this path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Publishes the staging directory to `destination` without ever
    /// replacing an existing destination.
    ///
    /// On [`PublishOutcome::Published`] the transaction has committed and
    /// `self` is consumed. On [`PublishOutcome::DestinationExists`] the
    /// destination is untouched and the staging directory is returned so the
    /// caller can reuse or abort it. An error likewise returns the staging
    /// directory: a failed publication never orphans it.
    ///
    /// See the [module documentation](self) for the exact per-platform
    /// guarantee.
    ///
    /// # Errors
    ///
    /// The staging directory is handed back alongside every error code; see
    /// [`AtomicDirectoryError`].
    pub fn publish_no_replace(
        mut self,
        destination: &Path,
    ) -> Result<PublishOutcome, (Self, AtomicDirectoryError)> {
        if destination.as_os_str().is_empty() || destination.parent().is_none() {
            return Err((self, AtomicDirectoryError::UnsafeDestination));
        }
        match rename_no_replace(&self.path, destination) {
            Ok(true) => {
                // The tree now lives under the destination name. Disarm the
                // best-effort removal so `Drop` cannot touch it.
                self.disarm();
                Ok(PublishOutcome::Published)
            }
            Ok(false) => Ok(PublishOutcome::DestinationExists(self)),
            Err(error) => Err((self, error)),
        }
    }

    /// Clears the staging path so [`Drop`] performs no removal.
    fn disarm(&mut self) -> PathBuf {
        std::mem::take(&mut self.path)
    }

    /// Removes the staging directory and its contents.
    ///
    /// Idempotent in the sense that a directory that is already gone is a
    /// success. This never removes a published destination: publication
    /// consumes the value.
    ///
    /// # Errors
    ///
    /// [`AtomicDirectoryError::IoFailure`] when the tree could not be
    /// removed; it is then left in place for diagnosis.
    pub fn abort(mut self) -> Result<(), AtomicDirectoryError> {
        let path = self.disarm();
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AtomicDirectoryError::IoFailure),
        }
    }
}

impl Drop for StagingDirectory {
    /// Removes the staging tree unless the value has been disarmed by a
    /// successful publication or an explicit [`StagingDirectory::abort`].
    fn drop(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A per-attempt nonce for staging names.
///
/// The names only have to be distinct among concurrent writers; exclusivity
/// comes from `mkdir` itself, not from the name being unguessable, so a
/// process/time-derived value is sufficient and needs no randomness source.
fn staging_nonce(attempt: u32) -> String {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{}-{time}-{attempt}", std::process::id())
}

/// Renames `from` to `to`, refusing to replace an existing `to`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn rename_no_replace(from: &Path, to: &Path) -> Result<bool, AtomicDirectoryError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(true),
        Err(Errno::EXIST | Errno::NOTEMPTY) => Ok(false),
        // The kernel or the filesystem does not implement the flag. Reporting
        // this rather than falling back to a plain rename is deliberate: a
        // plain rename would replace the destination.
        Err(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP) => {
            Err(AtomicDirectoryError::Unsupported)
        }
        Err(Errno::NOENT) => Err(AtomicDirectoryError::InvalidState),
        Err(Errno::XDEV) => Err(AtomicDirectoryError::UnsafeDestination),
        Err(Errno::NOMEM | Errno::NOSPC) => Err(AtomicDirectoryError::ResourceExhausted),
        Err(_) => Err(AtomicDirectoryError::IoFailure),
    }
}

/// Renames `from` to `to`, refusing to replace an existing `to`.
///
/// See the module documentation for why the advisory pre-check is sound for
/// the directory case this primitive publishes.
#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> Result<bool, AtomicDirectoryError> {
    match to.try_exists() {
        Ok(true) => return Ok(false),
        Ok(false) => {}
        Err(_) => return Err(AtomicDirectoryError::IoFailure),
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(true),
        Err(error) => Err(match error.kind() {
            // `MoveFileEx` reports these when the destination appeared, or
            // exists as a directory it may not replace.
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied => {
                return Ok(false);
            }
            std::io::ErrorKind::NotFound => AtomicDirectoryError::InvalidState,
            std::io::ErrorKind::OutOfMemory => AtomicDirectoryError::ResourceExhausted,
            _ => AtomicDirectoryError::IoFailure,
        }),
    }
}

/// No no-replace publication operation exists on this target.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn rename_no_replace(_from: &Path, _to: &Path) -> Result<bool, AtomicDirectoryError> {
    Err(AtomicDirectoryError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::{AtomicDirectoryError, PublishOutcome, StagingDirectory};
    use std::fs;
    use std::path::Path;

    #[test]
    fn staging_is_created_exclusively_inside_the_parent() {
        let root = tempfile::tempdir().expect("temporary directory");
        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        assert!(staging.path().is_dir());
        assert_eq!(staging.path().parent(), Some(root.path()));

        let second = StagingDirectory::create(root.path(), "import").expect("second staging");
        assert_ne!(staging.path(), second.path());
    }

    #[test]
    fn a_prefix_with_a_separator_is_rejected() {
        let root = tempfile::tempdir().expect("temporary directory");
        for prefix in ["", "a/b", "a\\b"] {
            assert_eq!(
                StagingDirectory::create(root.path(), prefix).expect_err("rejected"),
                AtomicDirectoryError::InvalidState
            );
        }
    }

    #[test]
    fn dropping_staging_removes_it() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = {
            let staging = StagingDirectory::create(root.path(), "import").expect("staging");
            fs::write(staging.path().join("entry.bin"), b"content").expect("entry");
            staging.path().to_path_buf()
        };
        assert!(!path.exists(), "drop must remove the staging tree");
    }

    #[test]
    fn abort_removes_staging_and_is_tolerant_of_an_absent_tree() {
        let root = tempfile::tempdir().expect("temporary directory");
        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        let path = staging.path().to_path_buf();
        staging.abort().expect("abort");
        assert!(!path.exists());

        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        let path = staging.path().to_path_buf();
        fs::remove_dir(&path).expect("external removal");
        staging.abort().expect("abort tolerates an absent tree");
    }

    #[test]
    fn publication_moves_the_whole_tree_to_the_destination() {
        let root = tempfile::tempdir().expect("temporary directory");
        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        let staged_path = staging.path().to_path_buf();
        fs::write(staging.path().join("entry.bin"), b"content").expect("entry");

        let destination = root.path().join("published");
        let outcome = staging.publish_no_replace(&destination).expect("publish");
        assert!(matches!(outcome, PublishOutcome::Published));
        assert!(!staged_path.exists(), "staging name is gone after publish");
        assert_eq!(
            fs::read(destination.join("entry.bin")).expect("published entry"),
            b"content"
        );
    }

    #[test]
    fn publication_never_replaces_an_existing_destination() {
        let root = tempfile::tempdir().expect("temporary directory");
        let destination = root.path().join("published");
        fs::create_dir(&destination).expect("existing destination");
        fs::write(destination.join("existing.bin"), b"original").expect("existing entry");

        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        fs::write(staging.path().join("entry.bin"), b"replacement").expect("entry");
        let staged_path = staging.path().to_path_buf();

        let outcome = staging.publish_no_replace(&destination).expect("publish");
        let PublishOutcome::DestinationExists(returned) = outcome else {
            panic!("an existing destination must never be replaced");
        };
        assert_eq!(
            fs::read(destination.join("existing.bin")).expect("original entry"),
            b"original"
        );
        assert!(
            !destination.join("entry.bin").exists(),
            "the losing writer must not have contributed any entry"
        );
        assert_eq!(returned.path(), staged_path);
        assert!(
            staged_path.is_dir(),
            "a refused publication hands the staging tree back intact"
        );
        drop(returned);
        assert!(!staged_path.exists(), "dropping the loser removes it");
    }

    #[test]
    fn a_publication_race_has_exactly_one_winner() {
        let root = tempfile::tempdir().expect("temporary directory");
        let destination = root.path().join("published");

        let mut published = 0;
        let mut refused = 0;
        for index in 0..8 {
            let staging = StagingDirectory::create(root.path(), "import").expect("staging");
            fs::write(staging.path().join("entry.bin"), format!("{index}")).expect("entry");
            match staging.publish_no_replace(&destination).expect("publish") {
                PublishOutcome::Published => published += 1,
                PublishOutcome::DestinationExists(_) => refused += 1,
            }
        }
        assert_eq!(published, 1);
        assert_eq!(refused, 7);
        assert_eq!(
            fs::read(destination.join("entry.bin")).expect("winner entry"),
            b"0"
        );
    }

    #[test]
    fn an_empty_destination_is_refused_and_returns_the_staging_directory() {
        let root = tempfile::tempdir().expect("temporary directory");
        let staging = StagingDirectory::create(root.path(), "import").expect("staging");
        let staged_path = staging.path().to_path_buf();

        let (returned, error) = staging
            .publish_no_replace(Path::new(""))
            .expect_err("an empty destination is refused");
        assert_eq!(error, AtomicDirectoryError::UnsafeDestination);
        assert_eq!(returned.path(), staged_path);
        assert!(staged_path.is_dir(), "a refused publish keeps staging");
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            AtomicDirectoryError::InvalidState,
            AtomicDirectoryError::IoFailure,
            AtomicDirectoryError::UnsafeDestination,
            AtomicDirectoryError::ResourceExhausted,
            AtomicDirectoryError::Unsupported,
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
