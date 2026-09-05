//! Development-only support for the freestanding Linux isolated-worker test
//! image.
//!
//! The image itself lives in [`image/`](../image), a standalone Cargo package
//! that is deliberately **not** a workspace member: a `#![no_std] #![no_main]`
//! binary can only be compiled with `panic = "abort"`, and `panic` is a
//! profile-level setting that Cargo refuses to scope to a single package
//! inside a workspace. Building it therefore means invoking `cargo` on that
//! package with an explicit `--target-dir`, which is what
//! [`build_test_worker_image`] does.
//!
//! Nothing in this crate is used by shipping code. It contains no `unsafe`
//! (the workspace-wide `unsafe_code = "forbid"` applies) and is only ever
//! reached from `ohl-platform`'s Linux test module and from
//! `cargo xtask worker-image`.

pub mod protocol;

use std::fmt;
use std::path::{Path, PathBuf};

/// Which behaviour the built image should have before it reports readiness.
///
/// Every post-readiness behaviour (echo, hang, crash, forbidden syscall,
/// chosen exit status) is selected at run time by the first byte of the first
/// frame, so exactly two compiled variants are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestWorkerVariant {
    /// Writes [`protocol::READY_ATTESTATION`] and then serves frames.
    Ready,
    /// Blocks forever without ever writing the readiness attestation.
    NeverReady,
}

impl TestWorkerVariant {
    /// Directory-name component and Cargo feature selector for this variant.
    const fn slug(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeverReady => "never-ready",
        }
    }
}

/// Why the test image could not be produced.
#[derive(Debug)]
pub enum BuildError {
    /// The image is Linux x86-64 only.
    Unsupported,
    /// The workspace layout around this crate was not what is expected.
    Layout(&'static str),
    /// `cargo` could not be spawned, or the artifact could not be staged.
    Io(std::io::Error),
    /// `cargo build` ran but failed.
    Cargo(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter
                .write_str("the freestanding test worker image is only built on Linux x86-64"),
            Self::Layout(detail) => write!(formatter, "unexpected repository layout: {detail}"),
            Self::Io(error) => write!(formatter, "failed to build the test worker image: {error}"),
            Self::Cargo(output) => {
                write!(
                    formatter,
                    "cargo failed to build the test worker image:\n{output}"
                )
            }
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

/// Where built artefacts are staged: `<target>/ohl-test-worker-image`.
fn artefact_root() -> Result<PathBuf, BuildError> {
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        return Ok(PathBuf::from(target).join("ohl-test-worker-image"));
    }
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or(BuildError::Layout(
            "crates/ohl-test-worker has no grandparent",
        ))?;
    Ok(workspace_root.join("target").join("ohl-test-worker-image"))
}

/// Builds (or rebuilds) `variant` and returns the path of a staged, read-only
/// copy of the image that satisfies the host backend's metadata policy:
/// regular file, owned by the caller, no write bits, no set-id bits.
///
/// The build is a nested `cargo` invocation on a dependency-free package with
/// its own `--target-dir`, so concurrent callers are serialised by Cargo's
/// own target-directory lock rather than by anything in this crate.
pub fn build_test_worker_image(variant: TestWorkerVariant) -> Result<PathBuf, BuildError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(BuildError::Unsupported);
    }

    let package = image_package_directory();
    let root = artefact_root()?;
    let target_directory = root.join(variant.slug());
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
    if variant == TestWorkerVariant::NeverReady {
        command.arg("--features").arg("never-ready");
    }
    // Never let the outer build's flags or wrappers leak into the image: the
    // image needs its own link arguments and must not, for example, inherit
    // an instrumentation wrapper from `cargo test`.
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

    let built = target_directory
        .join("release")
        .join("ohl-media-parser-worker");
    stage_read_only(&built, &root.join(format!("{}-image", variant.slug())))
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

/// A minimal, allocation-light ELF64 program-header summary used to prove the
/// built image really is a static, non-interpreted `ET_EXEC` binary.
#[derive(Debug, PartialEq, Eq)]
pub struct ElfSummary {
    /// `e_type`.
    pub object_type: u16,
    /// `e_machine`.
    pub machine: u16,
    /// Whether any `PT_INTERP` program header is present.
    pub has_interpreter: bool,
    /// Whether any `PT_DYNAMIC` program header is present.
    pub has_dynamic: bool,
}

/// `ET_EXEC`.
pub const ET_EXEC: u16 = 2;
/// `EM_X86_64`.
pub const EM_X86_64: u16 = 62;

/// Parses just enough of `bytes` to summarise the ELF identity.
pub fn summarise_elf(bytes: &[u8]) -> Option<ElfSummary> {
    let header: &[u8; 64] = bytes.get(..64)?.try_into().ok()?;
    if header[..4] != [0x7f, b'E', b'L', b'F'] || header[4] != 2 || header[5] != 1 {
        return None;
    }
    let read_u16 = |offset: usize| u16::from_le_bytes([header[offset], header[offset + 1]]);
    let read_u64 =
        |offset: usize| u64::from_le_bytes(header[offset..offset + 8].try_into().expect("8 bytes"));

    let program_header_offset = usize::try_from(read_u64(0x20)).ok()?;
    let entry_size = usize::from(read_u16(0x36));
    let count = usize::from(read_u16(0x38));
    if entry_size < 56 {
        return None;
    }

    let mut summary = ElfSummary {
        object_type: read_u16(0x10),
        machine: read_u16(0x12),
        has_interpreter: false,
        has_dynamic: false,
    };
    for index in 0..count {
        let start = program_header_offset.checked_add(index.checked_mul(entry_size)?)?;
        let entry = bytes.get(start..start.checked_add(4)?)?;
        let kind = u32::from_le_bytes(entry.try_into().ok()?);
        match kind {
            3 => summary.has_interpreter = true,
            2 => summary.has_dynamic = true,
            _ => {}
        }
    }
    Some(summary)
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::{EM_X86_64, ET_EXEC, TestWorkerVariant, build_test_worker_image, summarise_elf};

    #[test]
    fn the_built_image_is_a_static_non_interpreted_executable() {
        for variant in [TestWorkerVariant::Ready, TestWorkerVariant::NeverReady] {
            let path = build_test_worker_image(variant).expect("image builds");
            let bytes = std::fs::read(&path).expect("image is readable");
            let summary = summarise_elf(&bytes).expect("image is an ELF64 little-endian file");
            assert_eq!(summary.object_type, ET_EXEC, "{variant:?} must be ET_EXEC");
            assert_eq!(summary.machine, EM_X86_64, "{variant:?} must be x86-64");
            assert!(
                !summary.has_interpreter,
                "{variant:?} must have no PT_INTERP"
            );
            assert!(!summary.has_dynamic, "{variant:?} must have no PT_DYNAMIC");
        }
    }

    #[test]
    fn a_truncated_file_is_not_summarised() {
        assert!(summarise_elf(&[0x7f, b'E', b'L', b'F']).is_none());
    }
}
