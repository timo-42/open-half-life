//! Service-image resolution shared by the Unix backends.
//!
//! Both native backends (Linux x86-64 and macOS) locate a service image at
//! the same compile-fixed place - `<directory of the current executable>/`
//! `libexec/open-half-life/<image>` - and apply the same trust policy to
//! every path component and to the image's own metadata before they look at
//! a single byte of it. What differs is the executable *format* check
//! (static `ET_EXEC` ELF on Linux, `MH_EXECUTE` Mach-O on macOS), which each
//! backend supplies as a [`FormatCheck`] run on the already-open descriptor,
//! so what is inspected is exactly what will be executed.
//!
//! Nothing here is `unsafe`; the module is plain `rustix` calls.

use super::{IsolatedWorkerError, IsolatedWorkerService};

use std::fs::File;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};

/// Install location of a service image, relative to the directory holding the
/// running executable. Compile-fixed: no caller and no environment variable
/// can influence it in a shipping build.
pub(super) const SERVICE_IMAGE_RELATIVE_DIRECTORIES: [&str; 2] = ["libexec", "open-half-life"];

/// File name of the media-parser service image.
pub(super) const MEDIA_PARSER_IMAGE_NAME: &str = "ohl-media-parser-worker";

/// A backend's executable-format verification, run on the open image and its
/// size. Returning `false` fails the launch with
/// [`IsolatedWorkerError::ServiceIdentityMismatch`].
pub(super) type FormatCheck = fn(&File, u64) -> bool;

/// A resolved, verified image: the descriptor that will be executed and the
/// path it was resolved from (only a backend that must name the image by
/// path, such as the macOS sandbox profile, ever looks at the latter).
pub(super) struct VerifiedImage {
    pub(super) file: File,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) path: PathBuf,
}

const FILE_TYPE_MASK: u32 = 0o170_000;
const DIRECTORY_TYPE: u32 = 0o040_000;
const REGULAR_TYPE: u32 = 0o100_000;

/// `st_mode` as `u32` on both backends (`mode_t` is 16 bits wide on macOS).
fn mode_bits(status: &rustix::fs::Stat) -> u32 {
    #[cfg(target_os = "macos")]
    {
        u32::from(status.st_mode)
    }
    #[cfg(not(target_os = "macos"))]
    {
        status.st_mode
    }
}

pub(super) fn open_error(error: rustix::io::Errno) -> IsolatedWorkerError {
    if error == rustix::io::Errno::NOENT {
        IsolatedWorkerError::ServiceUnavailable
    } else {
        IsolatedWorkerError::ServiceIdentityMismatch
    }
}

/// A directory on the install path must be a directory owned by root or by
/// this user and writable by nobody else.
pub(super) fn trusted_directory(descriptor: BorrowedFd<'_>) -> bool {
    let Ok(status) = rustix::fs::fstat(descriptor) else {
        return false;
    };
    let mode = mode_bits(&status);
    mode & FILE_TYPE_MASK == DIRECTORY_TYPE
        && (status.st_uid == 0 || status.st_uid == rustix::process::geteuid().as_raw())
        && mode & (0o020 | 0o002 | 0o4000 | 0o2000) == 0
}

/// A service image must be a regular file owned by root or by this user, with
/// no write bit, no set-id bit, and at least one execute bit.
pub(super) fn trusted_image_metadata(status: &rustix::fs::Stat) -> bool {
    let mode = mode_bits(status);
    mode & FILE_TYPE_MASK == REGULAR_TYPE
        && (status.st_uid == 0 || status.st_uid == rustix::process::geteuid().as_raw())
        && mode & 0o222 == 0
        && mode & (0o4000 | 0o2000) == 0
        && mode & 0o111 != 0
}

/// Applies the metadata policy and `check` to an already-open descriptor.
pub(super) fn verify_open_image(
    descriptor: OwnedFd,
    check: FormatCheck,
) -> Result<File, IsolatedWorkerError> {
    let status =
        rustix::fs::fstat(&descriptor).map_err(|_| IsolatedWorkerError::ServiceIdentityMismatch)?;
    if !trusted_image_metadata(&status) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }
    let size =
        u64::try_from(status.st_size).map_err(|_| IsolatedWorkerError::ServiceIdentityMismatch)?;
    let file = File::from(descriptor);
    if !check(&file, size) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }
    Ok(file)
}

/// Opens `path` with `O_NOFOLLOW` and applies the full metadata plus format
/// policy. The returned file is the descriptor that will be executed.
///
/// Only the tests need to name an image by path. Shipping code reaches an
/// image exclusively through [`resolve_service_image`], which is compile-fixed
/// and honours no environment variable, so a deployed binary cannot be
/// redirected at a different program.
#[cfg(test)]
pub(super) fn open_verified_image(
    path: &Path,
    check: FormatCheck,
) -> Result<File, IsolatedWorkerError> {
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    verify_open_image(descriptor, check)
}

/// The image file name for `service`.
pub(super) const fn image_name(service: IsolatedWorkerService) -> &'static str {
    match service {
        IsolatedWorkerService::MediaParser => MEDIA_PARSER_IMAGE_NAME,
    }
}

/// Walks `<base>/libexec/open-half-life/<image>` one `O_NOFOLLOW` component
/// at a time, verifying each directory on the way, then verifies the image
/// with `check`.
///
/// `base` is the directory holding the current executable, as the caller
/// resolved it; the returned path is `base` joined with the fixed components,
/// so a caller that needs to name the image later names exactly what was
/// walked.
pub(super) fn resolve_service_image_under(
    base: &Path,
    service: IsolatedWorkerService,
    check: FormatCheck,
) -> Result<VerifiedImage, IsolatedWorkerError> {
    let name = image_name(service);

    let mut directory = rustix::fs::open(
        base,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    let mut path = base.to_path_buf();
    for component in SERVICE_IMAGE_RELATIVE_DIRECTORIES {
        if !trusted_directory(directory.as_fd()) {
            return Err(IsolatedWorkerError::ServiceIdentityMismatch);
        }
        directory = rustix::fs::openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(open_error)?;
        path.push(component);
    }
    if !trusted_directory(directory.as_fd()) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }

    let image = rustix::fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    path.push(name);
    let file = verify_open_image(image, check)?;
    Ok(VerifiedImage { file, path })
}

/// [`resolve_service_image_under`] rooted at the directory of the current
/// executable. The macOS backend canonicalises that directory first (its
/// sandbox profile names the image by real path) and so calls
/// [`resolve_service_image_under`] itself.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(super) fn resolve_service_image(
    service: IsolatedWorkerService,
    check: FormatCheck,
) -> Result<VerifiedImage, IsolatedWorkerError> {
    let executable =
        std::env::current_exe().map_err(|_| IsolatedWorkerError::ServiceUnavailable)?;
    let base = executable
        .parent()
        .ok_or(IsolatedWorkerError::ServiceUnavailable)?;
    resolve_service_image_under(base, service, check)
}
