//! Host-side support for the freestanding Linux media-parser worker image.
//!
//! The image itself lives in [`image/`](../image), a standalone Cargo package
//! that is deliberately **not** a workspace member, for the same reasons
//! `ohl-test-worker/image` is not one:
//!
//! - a `#![no_std] #![no_main]` binary can only be compiled with
//!   `panic = "abort"`, and `panic` is a profile-level setting Cargo refuses
//!   to scope to a single package inside a workspace;
//! - the workspace root manifest is owned by another work package and must
//!   not grow a `[profile]` section for one binary;
//! - the image needs a package-local `unsafe_code = "allow"` against the
//!   workspace-wide `forbid`.
//!
//! Building it therefore means invoking `cargo` on that package with an
//! explicit `--target-dir`, which is what [`build_parser_worker_image`] does.
//! The link configuration (`-nostdlib -static -no-pie` with the default `cc`
//! driver, emitted from the image's own `build.rs` as
//! `cargo::rustc-link-arg-bins`) stays attached to that package, so a plain
//! `cargo build --workspace` on Linux, macOS or Windows never sees it and no
//! global `RUSTFLAGS` is ever required.
//!
//! # Install location
//!
//! `ohl-platform`'s isolated-worker backend resolves the media-parser image
//! at `<directory of the current executable>/libexec/open-half-life/`
//! `ohl-media-parser-worker`, walking one `O_NOFOLLOW` component at a time.
//! [`install_parser_worker_image`] writes exactly that layout, and
//! `cargo xtask worker-image` installs it next to the build profile's
//! binaries. Nothing here contains `unsafe`: the workspace-wide
//! `unsafe_code = "forbid"` applies to this crate.

pub mod contract;

use std::fmt;
use std::path::{Path, PathBuf};

pub use contract::{
    CHANNEL_FD, READY_ATTESTATION, READY_FD, WORKER_CLEAN_EXIT, WORKER_INTERNAL_FAILURE_EXIT,
    WORKER_PROTOCOL_FAILURE_EXIT, WORKER_TRANSPORT_FAILURE_EXIT, WORKER_UNSUPPORTED_EXIT,
};

/// File name of the media-parser service image, as the backend expects it.
pub const IMAGE_NAME: &str = "ohl-media-parser-worker";

/// Directory components appended to the directory holding the executable that
/// launches a worker.
pub const IMAGE_RELATIVE_DIRECTORIES: [&str; 2] = ["libexec", "open-half-life"];

/// Why the worker image could not be produced or installed.
#[derive(Debug)]
#[non_exhaustive]
pub enum BuildError {
    /// The image is Linux x86-64 only.
    Unsupported,
    /// The repository layout around this crate was not what is expected.
    Layout(&'static str),
    /// `cargo` could not be spawned, or the artefact could not be staged.
    Io(std::io::Error),
    /// `cargo build` ran but failed.
    Cargo(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("the freestanding parser worker image is Linux x86-64 only")
            }
            Self::Layout(detail) => write!(formatter, "unexpected repository layout: {detail}"),
            Self::Io(error) => {
                write!(
                    formatter,
                    "failed to build the parser worker image: {error}"
                )
            }
            Self::Cargo(output) => write!(
                formatter,
                "cargo failed to build the parser worker image:\n{output}"
            ),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<std::io::Error> for BuildError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Absolute path of the standalone image package.
fn image_package_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("image")
}

/// The workspace root, derived from this crate's manifest directory.
fn workspace_root() -> Result<PathBuf, BuildError> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or(BuildError::Layout(
            "crates/ohl-parser-worker has no grandparent",
        ))
}

/// Where the nested build stages artefacts: `<target>/ohl-parser-worker-image`.
fn artefact_root() -> Result<PathBuf, BuildError> {
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        return Ok(PathBuf::from(target).join("ohl-parser-worker-image"));
    }
    Ok(workspace_root()?
        .join("target")
        .join("ohl-parser-worker-image"))
}

/// Builds (or rebuilds) the release image and returns the path of a staged,
/// read-only copy that satisfies the backend's metadata policy: regular file,
/// owned by the caller, no write bits, no set-id bits.
///
/// The build is a nested `cargo` invocation on a package with its own
/// `--target-dir`, so concurrent callers are serialised by Cargo's own
/// target-directory lock rather than by anything here.
///
/// # Errors
/// [`BuildError`] for a non-Linux host, an unexpected layout, a failed
/// `cargo build`, or a failed copy.
pub fn build_parser_worker_image() -> Result<PathBuf, BuildError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(BuildError::Unsupported);
    }

    let package = image_package_directory();
    let root = artefact_root()?;
    let target_directory = root.join("build");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());

    let mut command = std::process::Command::new(cargo);
    command
        .current_dir(&package)
        .arg("build")
        .arg("--release")
        .arg("--locked")
        .arg("--offline")
        .arg("--target-dir")
        .arg(&target_directory);
    // Never let the outer build's flags or wrappers leak into the image: it
    // needs its own link arguments and must not inherit, say, an
    // instrumentation wrapper from `cargo test`.
    for variable in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_MAKEFLAGS",
    ] {
        command.env_remove(variable);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(BuildError::Cargo(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let built = target_directory.join("release").join(IMAGE_NAME);
    stage_read_only(&built, &root.join(IMAGE_NAME))
}

/// The path the backend will resolve for an executable living in
/// `executable_directory`.
#[must_use]
pub fn installed_image_path(executable_directory: &Path) -> PathBuf {
    let mut path = executable_directory.to_path_buf();
    for component in IMAGE_RELATIVE_DIRECTORIES {
        path.push(component);
    }
    path.push(IMAGE_NAME);
    path
}

/// Installs `image` at [`installed_image_path`] for `executable_directory`.
///
/// The two intermediate directories are created if needed and forced to mode
/// `0o755`: the backend walks the chain with `O_NOFOLLOW` and refuses any
/// group- or world-writable component, which is exactly what an ambient
/// `umask 002` would otherwise produce.
///
/// # Errors
/// [`BuildError::Io`] when a directory or the file cannot be written.
pub fn install_parser_worker_image(
    image: &Path,
    executable_directory: &Path,
) -> Result<PathBuf, BuildError> {
    let mut directory = executable_directory.to_path_buf();
    for component in IMAGE_RELATIVE_DIRECTORIES {
        directory.push(component);
        std::fs::create_dir_all(&directory)?;
        set_trusted_directory_mode(&directory)?;
    }
    stage_read_only(image, &directory.join(IMAGE_NAME))
}

/// Builds the image and installs it for `executable_directory`.
///
/// # Errors
/// Any [`BuildError`] from the build or the install step.
pub fn build_and_install_parser_worker_image(
    executable_directory: &Path,
) -> Result<PathBuf, BuildError> {
    let built = build_parser_worker_image()?;
    install_parser_worker_image(&built, executable_directory)
}

/// Forces `path` to mode `0o755`, the loosest mode the backend accepts for a
/// directory on the image's resolution path.
///
/// # Errors
/// [`BuildError::Io`] when the mode cannot be set, and
/// [`BuildError::Unsupported`] off Unix.
pub fn set_trusted_directory_mode(path: &Path) -> Result<(), BuildError> {
    set_directory_mode(path)
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> Result<(), BuildError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> Result<(), BuildError> {
    Err(BuildError::Unsupported)
}

/// Copies `source` to `destination` through a temporary file and leaves the
/// result mode `0o555`.
fn stage_read_only(source: &Path, destination: &Path) -> Result<PathBuf, BuildError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    std::fs::copy(source, &temporary)?;
    set_read_only_executable(&temporary)?;
    std::fs::rename(&temporary, destination)?;
    Ok(destination.to_path_buf())
}

#[cfg(unix)]
fn set_read_only_executable(path: &Path) -> Result<(), BuildError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o555))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_read_only_executable(_path: &Path) -> Result<(), BuildError> {
    Err(BuildError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::{
        IMAGE_NAME, WORKER_CLEAN_EXIT, WORKER_INTERNAL_FAILURE_EXIT, WORKER_PROTOCOL_FAILURE_EXIT,
        WORKER_TRANSPORT_FAILURE_EXIT, WORKER_UNSUPPORTED_EXIT, installed_image_path,
    };
    use std::path::Path;

    #[test]
    fn the_install_path_matches_the_backend_resolution_rule() {
        let path = installed_image_path(Path::new("/opt/ohl/bin"));
        assert_eq!(
            path,
            Path::new("/opt/ohl/bin/libexec/open-half-life").join(IMAGE_NAME)
        );
    }

    #[test]
    fn the_exit_statuses_mirror_the_cxx_worker() {
        assert_eq!(WORKER_CLEAN_EXIT, 0);
        assert_eq!(WORKER_PROTOCOL_FAILURE_EXIT, 64);
        assert_eq!(WORKER_UNSUPPORTED_EXIT, 65);
        assert_eq!(WORKER_TRANSPORT_FAILURE_EXIT, 66);
        assert_eq!(WORKER_INTERNAL_FAILURE_EXIT, 70);
    }
}
