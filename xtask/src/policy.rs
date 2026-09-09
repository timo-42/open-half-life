//! Tracked-file policy check, originally `cmake/CheckRepository.cmake` in the
//! now-removed C++ build, ported here with identical rules.
//!
//! Rules (kept identical to the CMake script so both checkers agree during
//! the R2 transition period, section 4 of the migration plan recorded in
//! local design notes and not part of the repository):
//!
//! - no tracked file under `assets/`, `cache/`, or `imported/`, except the
//!   exact file `assets/README.md`;
//! - no tracked file with a prohibited proprietary-media/executable
//!   extension;
//! - no tracked file over 50 MiB;
//! - no tracked file whose first bytes match the `MZ`/`MSCF`/`IWAD`/`PWAD`/
//!   `PACK` magic signatures;
//! - no tracked file under `docs/`, `crates/`, or `xtask/`, and not the
//!   top-level `README.md`, may cite a machine-local, gitignored notes
//!   directory: every such citation is a dangling reference for anyone who
//!   does not have that directory, so the sentence around it must stand on
//!   its own instead.

use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::process::Command;

/// Path prefixes (lowercase, trailing slash) that may never be tracked.
pub const PROHIBITED_PREFIXES: &[&str] = &["assets/", "cache/", "imported/"];

/// The sole exception to [`PROHIBITED_PREFIXES`], matched case-insensitively
/// against the full lowercased relative path.
pub const ALLOWED_ASSETS_README: &str = "assets/readme.md";

/// Lowercase extensions (with leading dot, matching `cmake_path(...
/// EXTENSION)`) that may never be tracked.
pub const PROHIBITED_EXTENSIONS: &[&str] = &[
    ".bin", ".bsp", ".cab", ".cue", ".dll", ".exe", ".hdr", ".img", ".iso", ".mdl", ".mdf", ".mds",
    ".nrg", ".pak", ".spr", ".wad",
];

/// The tracked-file size ceiling in bytes (50 MiB).
pub const MAX_TRACKED_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Path prefixes (lowercase, trailing slash) scanned for a dangling
/// citation into the machine-local, gitignored notes directory.
pub const PLAN_CITATION_SCANNED_PREFIXES: &[&str] = &["docs/", "crates/", "xtask/"];

/// The sole non-prefix path also scanned for a dangling citation.
pub const PLAN_CITATION_SCANNED_FILE: &str = "readme.md";

/// The forbidden citation substring, split across two string literals so
/// this check's own source never spells the literal it forbids — a plain
/// text search for the joined string must not flag this file.
pub const FORBIDDEN_PLAN_CITATION: &str = concat!(".pla", "n/");

/// Lowercase hex-encoded magic signature prefixes that may never appear at
/// the start of a tracked file: `MZ`, `MSCF`, `IWAD`, `PWAD`, `PACK`.
pub const PROHIBITED_MAGIC_HEX_PREFIXES: &[&str] =
    &["4d5a", "4d534346", "49574144", "50574144", "5041434b"];

/// One tracked-file policy violation.
#[derive(Debug, PartialEq, Eq)]
pub enum Violation {
    /// The path lives under a private media/cache prefix.
    PrivatePath(String),
    /// The path has a prohibited proprietary-media/executable extension.
    ProhibitedExtension(String),
    /// The tracked file exceeds [`MAX_TRACKED_FILE_BYTES`].
    OversizeFile(String, u64),
    /// The tracked file starts with a prohibited magic signature.
    ProhibitedMagic(String),
    /// The tracked file could not be read to check its size or contents.
    Unreadable(String, String),
    /// The tracked file cites the machine-local, gitignored notes
    /// directory, a dangling reference for anyone without that directory.
    PlanCitation(String),
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrivatePath(path) => {
                write!(f, "private media/cache path is tracked: {path}")
            }
            Self::ProhibitedExtension(path) => {
                write!(f, "proprietary-media container is tracked: {path}")
            }
            Self::OversizeFile(path, size) => {
                write!(
                    f,
                    "unexpected tracked file over 50 MiB: {path} ({size} bytes)"
                )
            }
            Self::ProhibitedMagic(path) => {
                write!(f, "proprietary/executable signature is tracked: {path}")
            }
            Self::Unreadable(path, reason) => {
                write!(f, "tracked file could not be inspected: {path} ({reason})")
            }
            Self::PlanCitation(path) => {
                write!(
                    f,
                    "tracked file cites the machine-local notes directory: {path}"
                )
            }
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Checks one tracked, repository-relative path (forward-slash separated, as
/// `git ls-files` emits) against every rule. `repository_root` is used to
/// read the file's size and leading bytes from disk.
pub fn check_tracked_file(repository_root: &Path, relative_path: &str) -> Result<(), Violation> {
    let lower_path = relative_path.to_ascii_lowercase();

    if PROHIBITED_PREFIXES
        .iter()
        .any(|prefix| lower_path.starts_with(prefix))
        && lower_path != ALLOWED_ASSETS_README
    {
        return Err(Violation::PrivatePath(relative_path.to_string()));
    }

    if let Some(extension) = Path::new(&lower_path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        let dotted = format!(".{extension}");
        if PROHIBITED_EXTENSIONS.contains(&dotted.as_str()) {
            return Err(Violation::ProhibitedExtension(relative_path.to_string()));
        }
    }

    let full_path = repository_root.join(relative_path);
    let metadata = std::fs::metadata(&full_path)
        .map_err(|error| Violation::Unreadable(relative_path.to_string(), error.to_string()))?;
    if metadata.len() > MAX_TRACKED_FILE_BYTES {
        return Err(Violation::OversizeFile(
            relative_path.to_string(),
            metadata.len(),
        ));
    }

    let mut file = std::fs::File::open(&full_path)
        .map_err(|error| Violation::Unreadable(relative_path.to_string(), error.to_string()))?;
    let mut header = [0u8; 8];
    let read = file
        .read(&mut header)
        .map_err(|error| Violation::Unreadable(relative_path.to_string(), error.to_string()))?;
    let signature = hex_encode(&header[..read]);
    if PROHIBITED_MAGIC_HEX_PREFIXES
        .iter()
        .any(|prefix| signature.starts_with(prefix))
    {
        return Err(Violation::ProhibitedMagic(relative_path.to_string()));
    }

    let is_plan_citation_scanned = PLAN_CITATION_SCANNED_PREFIXES
        .iter()
        .any(|prefix| lower_path.starts_with(prefix))
        || lower_path == PLAN_CITATION_SCANNED_FILE;
    if is_plan_citation_scanned {
        // Non-UTF-8 tracked files under these prefixes are not documentation
        // or source text, so they cannot contain the citation; skip them
        // rather than treating them as unreadable.
        if let Ok(contents) = std::fs::read_to_string(&full_path)
            && contents.contains(FORBIDDEN_PLAN_CITATION)
        {
            return Err(Violation::PlanCitation(relative_path.to_string()));
        }
    }

    Ok(())
}

/// Runs `git -C repository_root ls-files` and checks every tracked path.
/// Returns `Ok(true)` when the check ran (with or without violations
/// collected into `violations`), and `Ok(false)` when skipped because
/// `repository_root` is not a Git checkout, mirroring the CMake script's
/// early return.
pub fn run(repository_root: &Path) -> Result<Vec<Violation>, String> {
    if !repository_root.join(".git").exists() {
        return Ok(Vec::new());
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .arg("ls-files")
        .output()
        .map_err(|error| format!("failed to run `git ls-files`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let listing = String::from_utf8_lossy(&output.stdout);
    let mut violations = Vec::new();
    for relative_path in listing.lines().filter(|line| !line.is_empty()) {
        if let Err(violation) = check_tracked_file(repository_root, relative_path) {
            violations.push(violation);
        }
    }
    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::{Violation, check_tracked_file, run};
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git available on PATH");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        git(dir.path(), &["init", "-q"]);
        git(
            dir.path(),
            &["config", "user.email", "policy-test@example.com"],
        );
        git(dir.path(), &["config", "user.name", "Policy Test"]);
        dir
    }

    fn write_and_add(dir: &Path, relative: &str, contents: &[u8]) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&path, contents).expect("write fixture file");
        git(dir, &["add", relative]);
    }

    #[test]
    fn accepts_an_ordinary_source_file() {
        let repo = init_repo();
        write_and_add(repo.path(), "src/lib.rs", b"fn main() {}\n");
        assert!(check_tracked_file(repo.path(), "src/lib.rs").is_ok());
    }

    #[test]
    fn rejects_assets_prefix() {
        let repo = init_repo();
        write_and_add(repo.path(), "assets/texture.png", b"not really png");
        assert_eq!(
            check_tracked_file(repo.path(), "assets/texture.png"),
            Err(Violation::PrivatePath("assets/texture.png".to_string()))
        );
    }

    #[test]
    fn allows_the_assets_readme_exception() {
        let repo = init_repo();
        write_and_add(repo.path(), "assets/README.md", b"place your media here\n");
        assert!(check_tracked_file(repo.path(), "assets/README.md").is_ok());
    }

    #[test]
    fn rejects_cache_and_imported_prefixes() {
        let repo = init_repo();
        write_and_add(repo.path(), "cache/manifest.json", b"{}");
        write_and_add(repo.path(), "imported/payload.dat", b"data");
        assert!(matches!(
            check_tracked_file(repo.path(), "cache/manifest.json"),
            Err(Violation::PrivatePath(_))
        ));
        assert!(matches!(
            check_tracked_file(repo.path(), "imported/payload.dat"),
            Err(Violation::PrivatePath(_))
        ));
    }

    #[test]
    fn rejects_prohibited_extensions() {
        let repo = init_repo();
        write_and_add(repo.path(), "docs/notes.WAD", b"whatever");
        assert_eq!(
            check_tracked_file(repo.path(), "docs/notes.WAD"),
            Err(Violation::ProhibitedExtension("docs/notes.WAD".to_string()))
        );
    }

    #[test]
    fn rejects_oversize_files() {
        let repo = init_repo();
        let contents = vec![0u8; 50 * 1024 * 1024 + 1];
        write_and_add(repo.path(), "big.dat", &contents);
        assert!(matches!(
            check_tracked_file(repo.path(), "big.dat"),
            Err(Violation::OversizeFile(_, _))
        ));
    }

    #[test]
    fn rejects_mz_signature() {
        let repo = init_repo();
        let mut contents = vec![b'M', b'Z'];
        contents.extend_from_slice(&[0u8; 32]);
        write_and_add(repo.path(), "payload.noext", &contents);
        assert_eq!(
            check_tracked_file(repo.path(), "payload.noext"),
            Err(Violation::ProhibitedMagic("payload.noext".to_string()))
        );
    }

    #[test]
    fn rejects_iwad_pwad_pack_mscf_signatures() {
        let repo = init_repo();
        for (name, magic) in [
            ("a.noext", b"IWAD".as_slice()),
            ("b.noext", b"PWAD".as_slice()),
            ("c.noext", b"PACK".as_slice()),
            ("d.noext", b"MSCF".as_slice()),
        ] {
            write_and_add(repo.path(), name, magic);
            assert!(
                matches!(
                    check_tracked_file(repo.path(), name),
                    Err(Violation::ProhibitedMagic(_))
                ),
                "expected {name} with magic {magic:?} to be rejected"
            );
        }
    }

    #[test]
    fn run_collects_every_violation_in_a_repository() {
        let repo = init_repo();
        write_and_add(repo.path(), "src/lib.rs", b"fn main() {}\n");
        write_and_add(repo.path(), "assets/texture.png", b"not really png");
        write_and_add(repo.path(), "docs/notes.wad", b"whatever");
        let violations = run(repo.path()).expect("policy run succeeds");
        assert_eq!(violations.len(), 2);
    }

    #[test]
    fn run_skips_non_git_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let violations = run(dir.path()).expect("skip is not an error");
        assert!(violations.is_empty());
    }

    #[test]
    fn rejects_a_dangling_local_notes_citation_in_docs_crates_xtask_or_readme() {
        let repo = init_repo();
        // Built from a split literal so this test file's own source never
        // spells the forbidden substring either.
        let citation = concat!(".pla", "n/some-note.md");
        for path in [
            "docs/ARCHITECTURE.md",
            "crates/ohl-core/src/lib.rs",
            "xtask/src/main.rs",
            "README.md",
        ] {
            write_and_add(
                repo.path(),
                path,
                format!("see `{citation}` for details\n").as_bytes(),
            );
            assert_eq!(
                check_tracked_file(repo.path(), path),
                Err(Violation::PlanCitation(path.to_string())),
                "expected {path} citing the local notes directory to be rejected"
            );
        }
    }

    #[test]
    fn accepts_docs_crates_xtask_and_readme_without_the_citation() {
        let repo = init_repo();
        for path in [
            "docs/ARCHITECTURE.md",
            "crates/ohl-core/src/lib.rs",
            "xtask/src/main.rs",
            "README.md",
        ] {
            write_and_add(repo.path(), path, b"nothing to see here\n");
            assert!(check_tracked_file(repo.path(), path).is_ok());
        }
    }

    #[test]
    fn ignores_the_citation_outside_the_scanned_prefixes() {
        let repo = init_repo();
        let citation = concat!(".pla", "n/some-note.md");
        write_and_add(
            repo.path(),
            "fuzz/notes.txt",
            format!("see `{citation}`\n").as_bytes(),
        );
        assert!(check_tracked_file(repo.path(), "fuzz/notes.txt").is_ok());
    }

    #[test]
    fn this_policy_module_does_not_trip_its_own_check() {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask is one level below root");
        assert!(
            check_tracked_file(repository_root, "xtask/src/policy.rs").is_ok(),
            "policy.rs must not contain the literal citation substring it forbids"
        );
    }
}
