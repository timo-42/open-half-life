//! Per-platform backends behind the [`crate::media_source`] contract.
//!
//! Each backend exposes the same three operations over one pinned native file
//! object: acquisition ([`PinnedFile::open`]), an identity/size/change
//! snapshot ([`PinnedFile::snapshot`]), and a bounded positional read
//! ([`PinnedFile::read_at`]). Nothing here retains the acquisition path.

use crate::media_source::MediaSourceError;

mod shared;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(not(any(unix, windows)))]
mod unsupported;

#[cfg(unix)]
pub(crate) use unix::PinnedFile;
#[cfg(windows)]
pub(crate) use windows::PinnedFile;

#[cfg(not(any(unix, windows)))]
pub(crate) use unsupported::PinnedFile;

pub(crate) use shared::NativeSnapshot;

/// The maximum number of bytes handed to a single native read call.
///
/// Both backends loop over larger requests, so this only bounds one syscall.
pub(crate) const MAX_NATIVE_READ: usize = 1 << 30;

/// Rejects acquisition paths that no backend may hand to the OS.
///
/// An empty path is classified as [`MediaSourceError::NotFound`] rather than
/// as an open failure so that callers cannot distinguish "you passed nothing"
/// from "it is not there"; an embedded NUL byte cannot be expressed as a
/// native path at all.
pub(crate) fn reject_unusable_path(path: &std::path::Path) -> Result<(), MediaSourceError> {
    if path.as_os_str().is_empty() {
        return Err(MediaSourceError::NotFound);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        if path.as_os_str().as_bytes().contains(&0) {
            return Err(MediaSourceError::OpenFailed);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err(MediaSourceError::OpenFailed);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MediaSourceError, reject_unusable_path};
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    #[test]
    fn empty_paths_are_not_found() {
        assert_eq!(
            reject_unusable_path(Path::new("")),
            Err(MediaSourceError::NotFound)
        );
    }

    #[test]
    fn ordinary_paths_are_accepted() {
        assert_eq!(reject_unusable_path(Path::new("/tmp/example")), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn interior_nul_bytes_are_rejected() {
        use std::os::unix::ffi::OsStringExt as _;

        let path = PathBuf::from(OsString::from_vec(b"/tmp/a\0b".to_vec()));
        assert_eq!(
            reject_unusable_path(&path),
            Err(MediaSourceError::OpenFailed)
        );
    }

    #[cfg(windows)]
    #[test]
    fn interior_nul_units_are_rejected() {
        use std::os::windows::ffi::OsStringExt as _;

        let path = PathBuf::from(OsString::from_wide(&[0x43, 0x3a, 0x5c, 0x61, 0x0000, 0x62]));
        assert_eq!(
            reject_unusable_path(&path),
            Err(MediaSourceError::OpenFailed)
        );
    }

    #[test]
    fn unused_imports_are_referenced_on_every_platform() {
        let _ = OsString::new();
        let _ = PathBuf::new();
    }
}
