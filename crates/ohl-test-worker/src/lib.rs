//! Development-only support for the isolated-worker test image.
//!
//! The image itself lives in [`image/`](../image), a standalone Cargo package
//! that is deliberately **not** a workspace member, so its abort-on-panic
//! release profile stays private. Linux uses the fixed static musl target;
//! macOS uses the system runtime. Both run the same standard-library fixture.
//!
//! Nothing in this crate is used by shipping code. It contains no `unsafe`
//! (the workspace-wide `unsafe_code = "forbid"` applies) and is only ever
//! reached from `ohl-platform`'s Linux and macOS test modules and from
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
    /// The image is built on Linux x86-64 and macOS only.
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
            Self::Unsupported => {
                formatter.write_str("the test worker image is only built on Linux x86-64 and macOS")
            }
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

/// Whether this host can build (and its `ohl-platform` backend can launch)
/// the image: Linux x86-64 with the musl target installed, or macOS.
#[must_use]
pub const fn image_host_supported() -> bool {
    cfg!(any(
        all(target_os = "linux", target_arch = "x86_64"),
        target_os = "macos"
    ))
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
    if !image_host_supported() {
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
    scrub_build_environment(&mut command);
    if cfg!(target_os = "linux") {
        command.arg("--target").arg("x86_64-unknown-linux-musl");
        command.env(
            "CARGO_ENCODED_RUSTFLAGS",
            [
                "-Crelocation-model=static",
                "-Clink-arg=-static",
                "-Clink-arg=-no-pie",
                "-Clink-arg=-Wl,--build-id=none",
            ]
            .join("\x1f"),
        );
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(BuildError::Cargo(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }

    let artifact_directory = if cfg!(target_os = "linux") {
        target_directory.join("x86_64-unknown-linux-musl")
    } else {
        target_directory
    };
    let built = artifact_directory
        .join("release")
        .join("ohl-media-parser-worker");
    stage_read_only(&built, &root.join(format!("{}-image", variant.slug())))
}

/// Environment variables removed by name before the nested `cargo` runs.
///
/// The image must be built from its manifest and the builder's fixed flags:
/// an inherited compiler, wrapper, linker, or flag set could quietly
/// turn it into a dynamically linked, instrumented, or differently targeted
/// binary that the host backend would then refuse to execute (or, worse,
/// would execute with an unintended runtime attached).
const SCRUBBED_ENVIRONMENT_NAMES: [&str; 21] = [
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTDOCFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_LINKER",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_TARGET_DIR",
    "CARGO_MAKEFLAGS",
    "CC",
    "CXX",
    "CFLAGS",
    "CXXFLAGS",
    "AR",
    "RANLIB",
];

/// Name prefixes and suffixes removed in addition to
/// [`SCRUBBED_ENVIRONMENT_NAMES`]: everything `LD*` (`LD_PRELOAD`,
/// `LD_LIBRARY_PATH`, `LD_AUDIT`, `LDFLAGS`, ...) and every per-target
/// `CARGO_TARGET_<TRIPLE>_{RUSTFLAGS,LINKER,RUNNER,AR}`, whose middle
/// component cannot be spelled out ahead of time.
fn is_scrubbed_environment_name(name: &str) -> bool {
    SCRUBBED_ENVIRONMENT_NAMES.contains(&name)
        || name.starts_with("LD")
        || (name.starts_with("CARGO_TARGET_")
            && (name.ends_with("_RUSTFLAGS")
                || name.ends_with("_LINKER")
                || name.ends_with("_RUNNER")
                || name.ends_with("_AR")))
}

/// Removes every variable [`is_scrubbed_environment_name`] names from the
/// nested build. Names that are not valid UTF-8 cannot match a variable Cargo
/// or `rustc` reads, so they are left alone.
fn scrub_build_environment(command: &mut std::process::Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_scrubbed_environment_name) {
            command.env_remove(&name);
        }
    }
}

/// Recognizable C-runtime symbol names, retained for symbol-table diagnostics.
/// These are expected in a static musl standard-library worker and are not a
/// rejection criterion; ELF interpreter and dynamic headers determine whether
/// an external runtime is required.
pub const STATIC_LIBC_SYMBOL_NAMES: [&str; 8] = [
    "__libc_start_main",
    "__libc_csu_init",
    "__libc_csu_fini",
    "__libc_init_first",
    "_IO_2_1_stdout_",
    "_IO_2_1_stderr_",
    "__cxa_finalize",
    "malloc",
];

/// Every name from [`STATIC_LIBC_SYMBOL_NAMES`] that `bytes` *defines* or
/// references, in symbol-table order and without duplicates.
///
/// This diagnostic can identify a statically linked runtime even when it has
/// no dynamic ELF marker. The shipping worker audit permits these symbols.
///
/// # Errors
///
/// A fixed reason string when `bytes` is not an ELF64 little-endian file, or
/// when its section or symbol tables are truncated. A file with no symbol
/// table at all yields `Ok(&[])`: the caller decides whether a stripped image
/// is acceptable (`cargo xtask worker-image` separately requires `.symtab` on
/// the shipping image).
pub fn static_libc_symbols(bytes: &[u8]) -> Result<Vec<&'static str>, &'static str> {
    const SECTION_HEADER_BYTES: usize = 64;
    const SYMBOL_BYTES: usize = 24;
    const SHT_SYMTAB: u32 = 2;
    const SHT_DYNSYM: u32 = 11;

    let read_u16 = |offset: usize| -> Option<u16> {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let read_u32 = |offset: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let read_u64 = |offset: usize| -> Option<u64> {
        Some(u64::from_le_bytes(
            bytes.get(offset..offset + 8)?.try_into().ok()?,
        ))
    };

    let header = bytes
        .get(..SECTION_HEADER_BYTES)
        .ok_or("truncated header")?;
    if header[..4] != [0x7f, b'E', b'L', b'F'] || header[4] != 2 || header[5] != 1 {
        return Err("not an ELF64 little-endian file");
    }
    let section_offset = usize::try_from(read_u64(0x28).ok_or("truncated header")?)
        .map_err(|_| "unreadable section header offset")?;
    let entry_size = usize::from(read_u16(0x3a).ok_or("truncated header")?);
    let count = usize::from(read_u16(0x3c).ok_or("truncated header")?);
    if entry_size < SECTION_HEADER_BYTES {
        return Err("short section headers");
    }

    let section = |index: usize| -> Option<(u32, usize, usize, usize)> {
        let base = section_offset.checked_add(index.checked_mul(entry_size)?)?;
        let kind = read_u32(base.checked_add(4)?)?;
        let offset = usize::try_from(read_u64(base.checked_add(0x18)?)?).ok()?;
        let size = usize::try_from(read_u64(base.checked_add(0x20)?)?).ok()?;
        let link = usize::try_from(read_u32(base.checked_add(0x28)?)?).ok()?;
        Some((kind, offset, size, link))
    };

    let mut found: Vec<&'static str> = Vec::new();
    for index in 0..count {
        let (kind, offset, size, link) = section(index).ok_or("truncated section header")?;
        if kind != SHT_SYMTAB && kind != SHT_DYNSYM {
            continue;
        }
        let (_, string_offset, string_size, _) = section(link).ok_or("missing string table")?;
        let strings = bytes
            .get(string_offset..string_offset.checked_add(string_size).ok_or("bad strtab")?)
            .ok_or("truncated string table")?;
        let table = bytes
            .get(offset..offset.checked_add(size).ok_or("bad symtab")?)
            .ok_or("truncated symbol table")?;
        for entry in table.as_chunks::<SYMBOL_BYTES>().0 {
            let name_offset = usize::try_from(u32::from_le_bytes(
                entry[..4].try_into().expect("four bytes"),
            ))
            .map_err(|_| "unreadable symbol name offset")?;
            let Some(rest) = strings.get(name_offset..) else {
                continue;
            };
            let Some(end) = rest.iter().position(|byte| *byte == 0) else {
                continue;
            };
            let Ok(name) = core::str::from_utf8(&rest[..end]) else {
                continue;
            };
            if let Some(known) = STATIC_LIBC_SYMBOL_NAMES
                .iter()
                .find(|candidate| **candidate == name)
                && !found.contains(known)
            {
                found.push(known);
            }
        }
    }
    Ok(found)
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

/// A minimal Mach-O header summary, the macOS counterpart of
/// [`ElfSummary`]: enough to prove a built image is a thin 64-bit
/// `MH_EXECUTE` for the expected CPU that loads nothing beyond libSystem.
#[derive(Debug, PartialEq, Eq)]
pub struct MachOSummary {
    /// `cputype`.
    pub cpu_type: u32,
    /// `filetype`.
    pub file_type: u32,
    /// Every dynamic library named by an `LC_LOAD_DYLIB`-family command, in
    /// load-command order.
    pub dylibs: Vec<String>,
    /// Every `LC_LOAD_DYLINKER` path.
    pub dylinkers: Vec<String>,
    /// Whether any `LC_RPATH` is present.
    pub has_rpath: bool,
}

/// `MH_MAGIC_64`.
pub const MH_MAGIC_64: u32 = 0xfeed_facf;
/// `MH_EXECUTE`.
pub const MH_EXECUTE: u32 = 2;
/// `CPU_TYPE_ARM64`.
pub const CPU_TYPE_ARM64: u32 = 0x0100_000c;
/// `CPU_TYPE_X86_64`.
pub const CPU_TYPE_X86_64: u32 = 0x0100_0007;
/// The one dynamic library a hosted worker image may load.
pub const PERMITTED_MACOS_DYLIB: &str = "/usr/lib/libSystem.B.dylib";
/// The one dynamic linker a hosted worker image may name.
pub const PERMITTED_MACOS_DYLINKER: &str = "/usr/lib/dyld";

/// Summarises the Mach-O header and load commands of `bytes`, or `None` if
/// they are not a well-formed thin 64-bit Mach-O.
#[must_use]
pub fn summarise_macho(bytes: &[u8]) -> Option<MachOSummary> {
    const HEADER_BYTES: usize = 32;
    const LC_LOAD_DYLIB: u32 = 0xc;
    const LC_LOAD_DYLINKER: u32 = 0xe;
    const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
    const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
    const LC_RPATH: u32 = 0x8000_001c;
    const LC_REEXPORT_DYLIB: u32 = 0x8000_001f;
    const LC_LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;

    let read_u32 = |slice: &[u8], offset: usize| -> Option<u32> {
        Some(u32::from_le_bytes(
            slice.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let string_at = |command: &[u8], offset: usize| -> Option<String> {
        let rest = command.get(offset..)?;
        let end = rest.iter().position(|byte| *byte == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    };

    if read_u32(bytes, 0)? != MH_MAGIC_64 {
        return None;
    }
    let cpu_type = read_u32(bytes, 4)?;
    let file_type = read_u32(bytes, 12)?;
    let count = read_u32(bytes, 16)?;
    let size = usize::try_from(read_u32(bytes, 20)?).ok()?;
    let commands = bytes.get(HEADER_BYTES..HEADER_BYTES.checked_add(size)?)?;

    let mut summary = MachOSummary {
        cpu_type,
        file_type,
        dylibs: Vec::new(),
        dylinkers: Vec::new(),
        has_rpath: false,
    };
    let mut offset = 0usize;
    for _ in 0..count {
        let kind = read_u32(commands, offset)?;
        let length = usize::try_from(read_u32(commands, offset + 4)?).ok()?;
        if length < 8 {
            return None;
        }
        let command = commands.get(offset..offset.checked_add(length)?)?;
        match kind {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB
            | LC_LAZY_LOAD_DYLIB => {
                let name_offset = usize::try_from(read_u32(command, 8)?).ok()?;
                summary.dylibs.push(string_at(command, name_offset)?);
            }
            LC_LOAD_DYLINKER => {
                let name_offset = usize::try_from(read_u32(command, 8)?).ok()?;
                summary.dylinkers.push(string_at(command, name_offset)?);
            }
            LC_RPATH => summary.has_rpath = true,
            _ => {}
        }
        offset += length;
    }
    (offset == commands.len()).then_some(summary)
}

#[cfg(test)]
mod audit_tests {
    use super::{STATIC_LIBC_SYMBOL_NAMES, is_scrubbed_environment_name, static_libc_symbols};

    /// Minimal ELF64 little-endian file whose only sections are a string
    /// table and a symbol table holding `names`.
    fn fixture(names: &[&str]) -> Vec<u8> {
        const SECTION_HEADER_BYTES: usize = 64;
        let mut strings = vec![0_u8];
        let mut offsets = Vec::new();
        for name in names {
            offsets.push(u32::try_from(strings.len()).expect("a small fixture"));
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
        }
        let mut table = vec![0_u8; 24];
        for offset in &offsets {
            table.extend_from_slice(&offset.to_le_bytes());
            table.extend_from_slice(&[0x10, 0]);
            table.extend_from_slice(&1_u16.to_le_bytes());
            table.extend_from_slice(&0_u64.to_le_bytes());
            table.extend_from_slice(&0_u64.to_le_bytes());
        }

        let string_offset = SECTION_HEADER_BYTES;
        let symbol_offset = string_offset + strings.len();
        let section_offset = symbol_offset + table.len();
        let mut bytes = vec![0_u8; SECTION_HEADER_BYTES];
        bytes[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[0x28..0x30].copy_from_slice(
            &u64::try_from(section_offset)
                .expect("a small fixture")
                .to_le_bytes(),
        );
        bytes[0x3a..0x3c].copy_from_slice(&64_u16.to_le_bytes());
        bytes[0x3c..0x3e].copy_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&strings);
        bytes.extend_from_slice(&table);

        let mut section = |kind: u32, offset: usize, size: usize, link: u32| {
            let mut entry = vec![0_u8; SECTION_HEADER_BYTES];
            entry[4..8].copy_from_slice(&kind.to_le_bytes());
            entry[0x18..0x20].copy_from_slice(
                &u64::try_from(offset)
                    .expect("a small fixture")
                    .to_le_bytes(),
            );
            entry[0x20..0x28]
                .copy_from_slice(&u64::try_from(size).expect("a small fixture").to_le_bytes());
            entry[0x28..0x2c].copy_from_slice(&link.to_le_bytes());
            bytes.extend_from_slice(&entry);
        };
        section(0, 0, 0, 0);
        section(3, string_offset, strings.len(), 0);
        section(2, symbol_offset, table.len(), 1);
        bytes
    }

    #[test]
    fn a_freestanding_symbol_table_names_no_libc_symbol() {
        let bytes = fixture(&["_start", "memcpy", "ohl_test_worker_start"]);
        assert_eq!(static_libc_symbols(&bytes), Ok(Vec::new()));
    }

    #[test]
    fn a_statically_linked_libc_is_detected_by_its_own_symbols() {
        for name in STATIC_LIBC_SYMBOL_NAMES {
            let bytes = fixture(&["_start", name]);
            assert_eq!(
                static_libc_symbols(&bytes),
                Ok(vec![name]),
                "{name} must be reported"
            );
        }
    }

    #[test]
    fn each_libc_symbol_is_reported_once() {
        let bytes = fixture(&["malloc", "malloc", "__libc_start_main"]);
        assert_eq!(
            static_libc_symbols(&bytes),
            Ok(vec!["malloc", "__libc_start_main"])
        );
    }

    #[test]
    fn a_non_elf_file_is_rejected() {
        assert!(static_libc_symbols(&[0_u8; 8]).is_err());
        assert!(static_libc_symbols(&[0x7f_u8; 64]).is_err());
    }

    #[test]
    fn the_scrub_covers_compilers_linkers_and_per_target_flags() {
        for name in [
            "RUSTFLAGS",
            "RUSTC",
            "RUSTC_WRAPPER",
            "CC",
            "CFLAGS",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "LDFLAGS",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER",
        ] {
            assert!(
                is_scrubbed_environment_name(name),
                "{name} must be scrubbed"
            );
        }
        for name in ["PATH", "HOME", "CARGO", "CARGO_TARGET_DIR_SUFFIX"] {
            assert!(
                !is_scrubbed_environment_name(name),
                "{name} must be left alone"
            );
        }
    }
}
