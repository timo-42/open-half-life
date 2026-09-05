//! The Windows backend.
//!
//! Acquisition opens the selected path with `FILE_FLAG_OPEN_REPARSE_POINT`,
//! so a reparse point in the final component is opened *as the reparse point*
//! rather than followed, and is then rejected by its attributes. That is this
//! platform's equivalent of the Unix `O_NOFOLLOW` rejection: the decision is
//! made from the object that was actually opened, never from a separate
//! pre-open probe that could be raced.
//!
//! Positional reads use `FileExt::seek_read`, which supplies an explicit
//! `OVERLAPPED` offset to `ReadFile`. Each read therefore observes the offset
//! it was given; this code never consults the handle's shared file pointer.
//!
//! `std` opens the handle non-inheritable, so it never crosses a
//! `CreateProcess` boundary even when a child is created with handle
//! inheritance enabled.
//!
//! # Unsafe sites in this file
//!
//! [`file_type`] and [`file_information`] contain the crate's only two
//! `unsafe` blocks; both are justified at their definitions and listed in the
//! crate-level inventory in `lib.rs`.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::{FileExt as _, OpenOptionsExt as _};
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_RANDOM_ACCESS, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TYPE_DISK, GetFileInformationByHandle, GetFileType,
};

use super::shared::NativeSnapshot;
use super::shared::windows_facts::WindowsFileFacts;
use super::{MAX_NATIVE_READ, reject_unusable_path};
use crate::media_source::MediaSourceError;

/// `GENERIC_READ`, spelled locally so this crate needs no additional
/// `windows-sys` feature for a single constant.
const GENERIC_READ: u32 = 0x8000_0000;

/// Returns the `GetFileType` classification of `file`'s handle.
///
/// # Unsafe site 1 of 2
///
/// `GetFileType` is a pure FFI query with no out-parameters. Its handle
/// argument is borrowed from `file`, which is alive for the whole call and
/// owns an open handle, so the argument is valid by construction. The call
/// writes through no pointer and so cannot violate any Rust invariant; the
/// returned `u32` is only compared against a documented constant.
fn file_type(file: &File) -> u32 {
    let handle = file.as_raw_handle() as HANDLE;
    unsafe { GetFileType(handle) }
}

/// Returns `file`'s `BY_HANDLE_FILE_INFORMATION`, or `None` on failure.
///
/// # Unsafe site 2 of 2
///
/// This is the only available way to read `dwVolumeSerialNumber` and the
/// 64-bit file index, which together are the pinned native identity: the
/// corresponding `std::os::windows::fs::MetadataExt` accessors are
/// permanently unstable behind the `windows_by_handle` gate.
///
/// The handle argument is borrowed from the live, owned `file`, exactly as in
/// site 1. The single out-parameter is a stack-allocated
/// `BY_HANDLE_FILE_INFORMATION` created through its safe derived `Default`
/// implementation and passed as a raw pointer taken from an exclusive borrow,
/// so no aliasing is possible and the callee may only write within the
/// struct. The value is read back only when the call reports success.
fn file_information(file: &File) -> Option<BY_HANDLE_FILE_INFORMATION> {
    let handle = file.as_raw_handle() as HANDLE;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let succeeded = unsafe { GetFileInformationByHandle(handle, &raw mut information) };
    if succeeded == 0 {
        return None;
    }
    Some(information)
}

/// Maps a `std::io` acquisition failure onto the sanitized taxonomy.
fn map_open_error(error: &std::io::Error) -> MediaSourceError {
    match error.kind() {
        std::io::ErrorKind::NotFound => MediaSourceError::NotFound,
        std::io::ErrorKind::OutOfMemory => MediaSourceError::ResourceExhausted,
        _ => MediaSourceError::OpenFailed,
    }
}

/// Recombines the split 32-bit halves of a native information block.
fn facts_from_information(
    information: &BY_HANDLE_FILE_INFORMATION,
    is_disk: bool,
) -> WindowsFileFacts {
    WindowsFileFacts {
        is_disk,
        attributes: information.dwFileAttributes,
        volume_serial: information.dwVolumeSerialNumber,
        file_index: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
        size_bytes: (u64::from(information.nFileSizeHigh) << 32)
            | u64::from(information.nFileSizeLow),
        last_write_ticks: (u64::from(information.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(information.ftLastWriteTime.dwLowDateTime),
    }
}

/// One pinned, read-only native file object.
///
/// The [`File`] owns the handle; dropping it closes the handle and is the
/// only way it is ever closed. The acquisition path is not stored.
#[derive(Debug)]
pub(crate) struct PinnedFile {
    file: File,
}

impl PinnedFile {
    /// Acquires `path` exactly once and pins the resulting object.
    pub(crate) fn open(path: &Path) -> Result<(Self, NativeSnapshot), MediaSourceError> {
        reject_unusable_path(path)?;

        let file = OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(
                FILE_ATTRIBUTE_NORMAL
                    | FILE_FLAG_RANDOM_ACCESS
                    | FILE_FLAG_BACKUP_SEMANTICS
                    | FILE_FLAG_OPEN_REPARSE_POINT,
            )
            .open(path)
            .map_err(|error| map_open_error(&error))?;

        let pinned = Self { file };
        let snapshot = pinned.snapshot()?;
        if !snapshot.is_regular_file {
            return Err(MediaSourceError::NotRegularFile);
        }
        Ok((pinned, snapshot))
    }

    /// Re-observes the pinned object through the retained handle.
    ///
    /// This never resolves a path, so it observes the same object even after
    /// the original name has been renamed, deleted, or replaced.
    pub(crate) fn snapshot(&self) -> Result<NativeSnapshot, MediaSourceError> {
        let is_disk = file_type(&self.file) == FILE_TYPE_DISK;
        let information = file_information(&self.file).ok_or(MediaSourceError::ReadFailed)?;
        Ok(facts_from_information(&information, is_disk).into_snapshot())
    }

    /// Reads at most `destination.len()` bytes at `offset`.
    ///
    /// Returns the number of bytes transferred; `0` means the object reported
    /// end of file before the requested range was satisfied.
    pub(crate) fn read_at(
        &self,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<usize, MediaSourceError> {
        let bounded = destination.len().min(MAX_NATIVE_READ);
        loop {
            match self.file.seek_read(&mut destination[..bounded], offset) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(match error.kind() {
                        std::io::ErrorKind::OutOfMemory => MediaSourceError::ResourceExhausted,
                        std::io::ErrorKind::UnexpectedEof => MediaSourceError::UnexpectedEof,
                        _ => MediaSourceError::ReadFailed,
                    });
                }
            }
        }
    }
}
