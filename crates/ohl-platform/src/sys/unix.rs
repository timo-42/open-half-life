//! The Unix backend (Linux and macOS share this code path).
//!
//! Acquisition is a single `openat`-family call with `O_NOFOLLOW`, so a
//! symbolic link in the *final* path component is rejected atomically by the
//! kernel instead of by a check-then-open race. Intermediate components use
//! ordinary resolution, exactly as the C++ contract documented.
//!
//! This module contains no `unsafe` code: `rustix` and `std::os::unix`
//! provide safe wrappers for every syscall it needs.

use rustix::fs::{FileType, Mode, OFlags};
use rustix::io::Errno;
use std::fs::File;
use std::os::unix::fs::FileExt as _;
use std::path::Path;

use super::shared::NativeSnapshot;
use super::{MAX_NATIVE_READ, reject_unusable_path};
use crate::media_source::MediaSourceError;

/// Maps an acquisition `errno` onto the sanitized open-failure taxonomy.
///
/// Three errnos are reported as [`MediaSourceError::NotRegularFile`] because
/// on this platform they are the kernel refusing the object's *type*, not an
/// I/O failure:
///
/// - `ELOOP` is exactly how `O_NOFOLLOW` refuses a symbolic link in the final
///   component;
/// - `ENXIO` is how Linux refuses `open` on a socket file;
/// - `EOPNOTSUPP` is how macOS refuses the same thing.
///
/// Without this the caller could not distinguish "you selected a socket" from
/// a transient failure, and would report the wrong thing to the user.
fn map_open_error(error: Errno) -> MediaSourceError {
    match error {
        Errno::NOENT | Errno::NOTDIR => MediaSourceError::NotFound,
        Errno::LOOP | Errno::NXIO | Errno::OPNOTSUPP => MediaSourceError::NotRegularFile,
        Errno::MFILE | Errno::NFILE | Errno::NOMEM => MediaSourceError::ResourceExhausted,
        _ => MediaSourceError::OpenFailed,
    }
}

/// Widens the platform-dependent `struct stat` integer types.
///
/// `st_dev`, `st_ino`, and the `st_mtime` pair differ in width and signedness
/// between Linux and macOS, so the casts are concentrated here rather than
/// repeated at each field.
#[allow(
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::unnecessary_cast,
    reason = "st_* field widths and signedness differ per Unix target"
)]
fn snapshot_from_stat(status: &rustix::fs::Stat) -> Result<NativeSnapshot, MediaSourceError> {
    let size_bytes = u64::try_from(status.st_size).map_err(|_| MediaSourceError::OpenFailed)?;
    Ok(NativeSnapshot {
        identity: (status.st_dev as u64, status.st_ino as u64),
        size_bytes,
        change_stamp: (status.st_mtime as i64, status.st_mtime_nsec as i64),
        is_regular_file: FileType::from_raw_mode(status.st_mode) == FileType::RegularFile,
    })
}

/// One pinned, read-only native file object.
///
/// The [`File`] owns the descriptor; dropping it closes the descriptor and is
/// the only way it is ever closed. The acquisition path is not stored.
#[derive(Debug)]
pub(crate) struct PinnedFile {
    file: File,
}

impl PinnedFile {
    /// Acquires `path` exactly once and pins the resulting object.
    ///
    /// The flags are the C++ set: `O_RDONLY` (no writes are ever possible
    /// through this descriptor), `O_CLOEXEC` (the descriptor never crosses an
    /// `exec`), `O_NONBLOCK` (opening a FIFO with no writer must not hang
    /// before the type check rejects it), `O_NOCTTY` (opening a terminal must
    /// not make it this process's controlling terminal), and `O_NOFOLLOW`.
    pub(crate) fn open(path: &Path) -> Result<(Self, NativeSnapshot), MediaSourceError> {
        reject_unusable_path(path)?;

        let descriptor = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(map_open_error)?;

        let pinned = Self {
            file: File::from(descriptor),
        };
        let snapshot = pinned.snapshot()?;
        if !snapshot.is_regular_file {
            return Err(MediaSourceError::NotRegularFile);
        }
        Ok((pinned, snapshot))
    }

    /// Re-observes the pinned object through the retained descriptor.
    ///
    /// This never resolves a path, so it observes the same object even after
    /// the original name has been renamed, unlinked, or replaced.
    pub(crate) fn snapshot(&self) -> Result<NativeSnapshot, MediaSourceError> {
        let status = rustix::fs::fstat(&self.file).map_err(|_| MediaSourceError::ReadFailed)?;
        snapshot_from_stat(&status)
    }

    /// Reads at most `destination.len()` bytes at `offset`.
    ///
    /// Returns the number of bytes transferred; `0` means the object reported
    /// end of file before the requested range was satisfied. Positional reads
    /// use `pread`, so no shared seek cursor is consulted or modified and
    /// concurrent reads through a shared handle cannot interfere.
    pub(crate) fn read_at(
        &self,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<usize, MediaSourceError> {
        let bounded = destination.len().min(MAX_NATIVE_READ);
        loop {
            match self.file.read_at(&mut destination[..bounded], offset) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(match error.kind() {
                        std::io::ErrorKind::OutOfMemory => MediaSourceError::ResourceExhausted,
                        _ => MediaSourceError::ReadFailed,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaSourceError, PinnedFile, map_open_error};
    use rustix::io::Errno;
    use std::io::Write as _;

    #[test]
    fn open_errors_are_classified_without_leaking_errno() {
        assert_eq!(map_open_error(Errno::NOENT), MediaSourceError::NotFound);
        assert_eq!(map_open_error(Errno::NOTDIR), MediaSourceError::NotFound);
        for type_rejection in [Errno::LOOP, Errno::NXIO, Errno::OPNOTSUPP] {
            assert_eq!(
                map_open_error(type_rejection),
                MediaSourceError::NotRegularFile
            );
        }
        assert_eq!(
            map_open_error(Errno::MFILE),
            MediaSourceError::ResourceExhausted
        );
        assert_eq!(
            map_open_error(Errno::NFILE),
            MediaSourceError::ResourceExhausted
        );
        assert_eq!(map_open_error(Errno::ACCESS), MediaSourceError::OpenFailed);
    }

    #[test]
    fn the_pinned_descriptor_is_close_on_exec() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("cloexec.fixture");
        let mut file = std::fs::File::create(&path).expect("fixture");
        file.write_all(b"OHL synthetic native inheritance fixture\n")
            .expect("fixture bytes");
        drop(file);

        let (pinned, _) = PinnedFile::open(&path).expect("acquisition");
        let flags = rustix::io::fcntl_getfd(&pinned.file).expect("descriptor flags");
        assert!(
            flags.contains(rustix::io::FdFlags::CLOEXEC),
            "the pinned descriptor must not cross an exec boundary"
        );
    }

    #[test]
    fn a_character_device_is_not_a_regular_file() {
        assert_eq!(
            PinnedFile::open(std::path::Path::new("/dev/null")).map(|_| ()),
            Err(MediaSourceError::NotRegularFile)
        );
    }
}
