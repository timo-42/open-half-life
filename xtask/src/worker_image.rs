//! `cargo xtask worker-image`: build and audit the freestanding worker image.
//!
//! The Linux isolated-worker backend refuses to execute anything that is not
//! a statically linked, non-interpreted `ET_EXEC` x86-64 ELF, so a regression
//! in the image's link configuration would show up as an opaque
//! `ServiceIdentityMismatch` at launch time. This command turns that into an
//! explicit, standalone check with a readable failure.
//!
//! Two images are covered:
//!
//! - the `ohl-test-worker` fixture image, in its two startup variants;
//! - the shipping `ohl-media-parser-worker` image, which is additionally
//!   proved to reference no libc symbol, to define none of the well-known
//!   symbols a statically linked libc would bring in, and to have no
//!   undefined symbol at all, and is then installed at
//!   `<directory of this executable>/libexec/open-half-life/`, which is
//!   exactly where the backend resolves it.

use std::process::ExitCode;

use ohl_parser_worker::{
    IMAGE_NAME, IMAGE_RELATIVE_DIRECTORIES, build_parser_worker_image, install_parser_worker_image,
};
use ohl_test_worker::{
    EM_X86_64, ET_EXEC, TestWorkerVariant, build_test_worker_image, summarise_elf,
};

/// Symbol names that may never appear in the parser worker image. Each one
/// would mean a C library, or a syscall outside the seccomp allowlist, had
/// been linked in.
pub const FORBIDDEN_SYMBOL_NAMES: [&str; 6] = ["open", "openat", "ioctl", "socket", "mmap", "brk"];

/// One symbol-table finding.
#[derive(Debug, PartialEq, Eq)]
pub enum SymbolViolation {
    /// The image references a symbol nothing defines.
    Undefined(String),
    /// The image names a symbol from [`FORBIDDEN_SYMBOL_NAMES`].
    Forbidden(String),
}

impl std::fmt::Display for SymbolViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undefined(name) => write!(formatter, "undefined symbol `{name}`"),
            Self::Forbidden(name) => write!(formatter, "forbidden symbol `{name}`"),
        }
    }
}

const SECTION_HEADER_BYTES: usize = 64;
const SYMBOL_BYTES: usize = 24;
const SHT_SYMTAB: u32 = 2;

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

/// Reads the NUL-terminated string at `offset` inside the string table
/// `table`.
fn string_at(table: &[u8], offset: usize) -> Option<&str> {
    let rest = table.get(offset..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    core::str::from_utf8(&rest[..end]).ok()
}

/// Every undefined or forbidden symbol name in `bytes`.
///
/// The image is linked with `strip = "debuginfo"` precisely so `.symtab`
/// survives; an image without one is rejected rather than silently passed,
/// because a missing symbol table would make this check vacuous.
///
/// # Errors
/// A fixed reason string when `bytes` is not an ELF64 little-endian file with
/// a readable symbol table.
pub fn symbol_violations(bytes: &[u8]) -> Result<Vec<SymbolViolation>, &'static str> {
    let header = bytes
        .get(..SECTION_HEADER_BYTES)
        .ok_or("truncated header")?;
    if header[..4] != [0x7f, b'E', b'L', b'F'] || header[4] != 2 || header[5] != 1 {
        return Err("not an ELF64 little-endian file");
    }
    let section_offset = usize::try_from(read_u64(header, 0x28).ok_or("truncated header")?)
        .map_err(|_| "unreadable section header offset")?;
    let entry_size = usize::from(read_u16(header, 0x3a).ok_or("truncated header")?);
    let count = usize::from(read_u16(header, 0x3c).ok_or("truncated header")?);
    if entry_size < SECTION_HEADER_BYTES {
        return Err("short section headers");
    }

    let section = |index: usize| -> Option<(u32, usize, usize, usize)> {
        let base = section_offset.checked_add(index.checked_mul(entry_size)?)?;
        let kind = read_u32(bytes, base.checked_add(4)?)?;
        let link = usize::try_from(read_u32(bytes, base.checked_add(0x28)?)?).ok()?;
        let offset = usize::try_from(read_u64(bytes, base.checked_add(0x18)?)?).ok()?;
        let size = usize::try_from(read_u64(bytes, base.checked_add(0x20)?)?).ok()?;
        Some((kind, offset, size, link))
    };

    let mut violations = Vec::new();
    let mut seen_symbol_table = false;
    for index in 0..count {
        let (kind, offset, size, link) = section(index).ok_or("truncated section header")?;
        if kind != SHT_SYMTAB {
            continue;
        }
        seen_symbol_table = true;
        let (_, string_offset, string_size, _) = section(link).ok_or("missing string table")?;
        let strings = bytes
            .get(string_offset..string_offset.checked_add(string_size).ok_or("bad strtab")?)
            .ok_or("truncated string table")?;
        let table = bytes
            .get(offset..offset.checked_add(size).ok_or("bad symtab")?)
            .ok_or("truncated symbol table")?;
        for entry in table.as_chunks::<SYMBOL_BYTES>().0 {
            let name_offset = usize::try_from(read_u32(entry, 0).ok_or("truncated symbol")?)
                .map_err(|_| "unreadable symbol name offset")?;
            let section_index = read_u16(entry, 6).ok_or("truncated symbol")?;
            let Some(name) = string_at(strings, name_offset) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            if section_index == 0 {
                violations.push(SymbolViolation::Undefined(name.to_owned()));
            }
            if FORBIDDEN_SYMBOL_NAMES.contains(&name) {
                violations.push(SymbolViolation::Forbidden(name.to_owned()));
            }
        }
    }
    if !seen_symbol_table {
        return Err("no symbol table: the image must keep `.symtab`");
    }
    Ok(violations)
}

/// Checks the ELF identity of one already-built image.
fn check_identity(path: &std::path::Path, bytes: &[u8]) -> usize {
    let Some(summary) = summarise_elf(bytes) else {
        eprintln!(
            "error: {} is not an ELF64 little-endian file",
            path.display()
        );
        return 1;
    };
    let mut failures = 0usize;
    for (condition, detail) in [
        (summary.object_type == ET_EXEC, "must be ET_EXEC"),
        (summary.machine == EM_X86_64, "must target x86-64"),
        (!summary.has_interpreter, "must have no PT_INTERP"),
        (!summary.has_dynamic, "must have no PT_DYNAMIC"),
    ] {
        if !condition {
            eprintln!("error: {} {detail}", path.display());
            failures += 1;
        }
    }
    failures
}

/// Strips a group- or other-write bit from `mode` if either is set, leaving
/// every other bit (including any already-restrictive read/execute bits)
/// untouched. Returns `None` when `mode` already has neither bit set, so the
/// caller can skip a needless `chmod`.
///
/// An ambient `umask 002` makes `cargo build` leave `target/<profile>`
/// group-writable (mode `0o775`); the isolated-worker launcher walks every
/// path component from `target/<profile>` down to the installed image and
/// refuses any that is group- or world-writable, so this has to be corrected
/// before the image is installed underneath it.
///
/// Only meaningful on Unix (mode bits do not exist elsewhere), so this is
/// cfg'd out entirely off Unix rather than left as unreachable dead code.
#[cfg(unix)]
fn strip_group_other_write_mode(mode: u32) -> Option<u32> {
    const GROUP_OTHER_WRITE: u32 = 0o022;
    let normalized = mode & !GROUP_OTHER_WRITE;
    (normalized != mode).then_some(normalized)
}

/// Applies [`strip_group_other_write_mode`] to `path` on disk, doing nothing
/// if it is already free of group- and other-write bits.
#[cfg(unix)]
fn normalize_directory_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("failed to read permissions of {}: {error}", path.display()))?;
    let mode = metadata.permissions().mode();
    if let Some(normalized) = strip_group_other_write_mode(mode) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(normalized)).map_err(
            |error| {
                format!(
                    "failed to normalise permissions of {}: {error}",
                    path.display()
                )
            },
        )?;
    }
    Ok(())
}

/// Off Unix there is no mode-bit concept to normalise, so this is a no-op;
/// unlike the Unix version it cannot fail, hence the plain `()` return
/// instead of an always-`Ok` `Result` (which `clippy::unnecessary_wraps`
/// rightly flags).
#[cfg(not(unix))]
fn normalize_directory_permissions(_path: &std::path::Path) {}

/// Builds, audits and installs the shipping media-parser worker image.
fn run_parser_worker_image() -> Result<usize, String> {
    let built = build_parser_worker_image().map_err(|error| error.to_string())?;
    let bytes = std::fs::read(&built)
        .map_err(|error| format!("failed to read {}: {error}", built.display()))?;
    let mut failures = check_identity(&built, &bytes);

    match symbol_violations(&bytes) {
        Ok(violations) => {
            for violation in &violations {
                eprintln!("error: {}: {violation}", built.display());
            }
            failures += violations.len();
        }
        Err(reason) => {
            eprintln!("error: {}: {reason}", built.display());
            failures += 1;
        }
    }

    // A statically linked C library leaves no PT_INTERP and no PT_DYNAMIC
    // behind, so the identity check above cannot see it; its own symbols can.
    match ohl_test_worker::static_libc_symbols(&bytes) {
        Ok(found) => {
            for name in &found {
                eprintln!(
                    "error: {}: statically linked libc symbol `{name}`",
                    built.display()
                );
            }
            failures += found.len();
        }
        Err(reason) => {
            eprintln!("error: {}: {reason}", built.display());
            failures += 1;
        }
    }

    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to locate this executable: {error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| "this executable has no parent directory".to_owned())?;
    // `target/<profile>` is the first path component the launcher walks; an
    // ambient `umask 002` leaves it group-writable, which the launcher
    // refuses. `install_parser_worker_image` only controls the directories it
    // creates beneath it (`libexec/open-half-life`), so this one has to be
    // fixed here.
    #[cfg(unix)]
    normalize_directory_permissions(directory)?;
    #[cfg(not(unix))]
    normalize_directory_permissions(directory);
    let installed =
        install_parser_worker_image(&built, directory).map_err(|error| error.to_string())?;
    // Belt and braces: re-normalise the directories the install step just
    // created, in case a future change there stops forcing an exact mode.
    let mut created = directory.to_path_buf();
    for component in IMAGE_RELATIVE_DIRECTORIES {
        created.push(component);
        #[cfg(unix)]
        normalize_directory_permissions(&created)?;
        #[cfg(not(unix))]
        normalize_directory_permissions(&created);
    }
    println!(
        "{IMAGE_NAME}: {} ({} bytes) -> {}",
        built.display(),
        bytes.len(),
        installed.display()
    );
    Ok(failures)
}

/// Builds every image variant and verifies its ELF identity.
pub fn run() -> ExitCode {
    let variants = [TestWorkerVariant::Ready, TestWorkerVariant::NeverReady];
    let mut failures = 0usize;

    for variant in variants {
        let path = match build_test_worker_image(variant) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("error: failed to read {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let Some(summary) = summarise_elf(&bytes) else {
            eprintln!(
                "error: {} is not an ELF64 little-endian file",
                path.display()
            );
            failures += 1;
            continue;
        };

        for (condition, detail) in [
            (summary.object_type == ET_EXEC, "must be ET_EXEC"),
            (summary.machine == EM_X86_64, "must target x86-64"),
            (!summary.has_interpreter, "must have no PT_INTERP"),
            (!summary.has_dynamic, "must have no PT_DYNAMIC"),
        ] {
            if !condition {
                eprintln!("error: {} {detail}", path.display());
                failures += 1;
            }
        }
        println!("{variant:?}: {} ({} bytes)", path.display(), bytes.len());
    }

    match run_parser_worker_image() {
        Ok(count) => failures += count,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    }

    if failures == 0 {
        println!(
            "Worker image check passed ({} test variant(s) + the media-parser image)",
            variants.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("Worker image check failed ({failures} violation(s))");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{SymbolViolation, symbol_violations};

    const SECTION_HEADER_BYTES: usize = 64;

    // `strip_group_other_write_mode` only exists on Unix (see its `cfg`), so
    // these tests are gated the same way rather than failing to resolve the
    // name on Windows/macOS.
    #[cfg(unix)]
    mod permission_normalisation {
        use super::super::strip_group_other_write_mode;

        #[test]
        fn strips_a_group_writable_mode_left_by_umask_002() {
            // `cargo build` under `umask 002` leaves `target/<profile>` at
            // `0o775`; only the group-write bit must go.
            assert_eq!(strip_group_other_write_mode(0o775), Some(0o755));
        }

        #[test]
        fn strips_an_other_writable_mode() {
            assert_eq!(strip_group_other_write_mode(0o707), Some(0o705));
        }

        #[test]
        fn strips_both_group_and_other_write_bits_at_once() {
            assert_eq!(strip_group_other_write_mode(0o777), Some(0o755));
        }

        #[test]
        fn leaves_an_already_clean_mode_untouched() {
            assert_eq!(strip_group_other_write_mode(0o755), None);
        }

        #[test]
        fn preserves_bits_outside_group_and_other_write() {
            // A directory with no group or other access at all (`0o700`)
            // must not gain any new permission: only a set write bit is ever
            // cleared.
            assert_eq!(strip_group_other_write_mode(0o700), None);
        }
    }

    /// Builds a minimal ELF64 little-endian fixture whose only sections are a
    /// string table and a symbol table holding `symbols` as
    /// `(name, section_index)` pairs (`0` means undefined).
    fn fixture(symbols: &[(&str, u16)]) -> Vec<u8> {
        let mut strings = vec![0_u8];
        let mut offsets = Vec::new();
        for (name, _) in symbols {
            offsets.push(u32::try_from(strings.len()).expect("a small fixture"));
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
        }

        // One leading null symbol, as every real symbol table has.
        let mut table = vec![0_u8; 24];
        for (offset, (_, section_index)) in offsets.iter().zip(symbols) {
            table.extend_from_slice(&offset.to_le_bytes());
            table.push(0x10); // STB_GLOBAL | STT_NOTYPE
            table.push(0);
            table.extend_from_slice(&section_index.to_le_bytes());
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
        bytes[0x10..0x12].copy_from_slice(&2_u16.to_le_bytes()); // ET_EXEC
        bytes[0x12..0x14].copy_from_slice(&62_u16.to_le_bytes()); // EM_X86_64
        bytes[0x28..0x30].copy_from_slice(
            &u64::try_from(section_offset)
                .expect("a small fixture")
                .to_le_bytes(),
        );
        bytes[0x3a..0x3c].copy_from_slice(
            &u16::try_from(SECTION_HEADER_BYTES)
                .expect("a 64-byte header")
                .to_le_bytes(),
        );
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
    fn a_clean_symbol_table_has_no_violations() {
        let bytes = fixture(&[("_start", 1), ("memcpy", 1)]);
        assert_eq!(symbol_violations(&bytes), Ok(Vec::new()));
    }

    #[test]
    fn undefined_and_forbidden_symbols_are_reported() {
        let bytes = fixture(&[("__libc_start_main", 0), ("openat", 1), ("socket", 0)]);
        let violations = symbol_violations(&bytes).expect("the fixture parses");
        assert!(violations.contains(&SymbolViolation::Undefined("__libc_start_main".to_owned())));
        assert!(violations.contains(&SymbolViolation::Forbidden("openat".to_owned())));
        assert!(violations.contains(&SymbolViolation::Undefined("socket".to_owned())));
        assert!(violations.contains(&SymbolViolation::Forbidden("socket".to_owned())));
    }

    #[test]
    fn an_image_without_a_symbol_table_is_rejected() {
        let mut bytes = fixture(&[("_start", 1)]);
        // Turn the symbol table into a plain progbits section.
        let section_offset = u64::from_le_bytes(bytes[0x28..0x30].try_into().expect("shoff"));
        let symtab =
            usize::try_from(section_offset).expect("a small fixture") + 2 * SECTION_HEADER_BYTES;
        bytes[symtab + 4..symtab + 8].copy_from_slice(&1_u32.to_le_bytes());
        assert!(symbol_violations(&bytes).is_err());
    }

    #[test]
    fn a_non_elf_file_is_rejected() {
        assert!(symbol_violations(&[0_u8; 8]).is_err());
        assert!(symbol_violations(&[0x7f_u8; 64]).is_err());
    }
}
