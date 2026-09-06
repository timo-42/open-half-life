//! `cargo xtask dist`: builds the release binary (and, on Linux x86-64, the
//! parser worker image via `ohl_parser_worker`), assembles a versioned,
//! self-contained release folder under `target/dist/`, and archives it.
//!
//! See [`crate::dist::layout`] for the exact on-disk layout.

mod archive;
mod layout;
mod licenses;
mod target;
mod version;

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use target::Platform;

const BINARY_CRATE_NAME: &str = "ohl-app";
const BINARY_NAME: &str = "open-half-life";

/// The default `--out-dir`, relative to the workspace root.
const DEFAULT_OUT_DIR: &str = "target/dist";

/// `cargo xtask dist [--target <triple>] [--out-dir <dir>] [--print-target]`.
#[derive(Debug, Parser)]
#[command(
    name = "dist",
    about = "Builds a versioned, self-contained release archive"
)]
struct Args {
    /// Target triple to build the release binary for. Defaults to the host
    /// triple (`rustc -vV`'s own `host:` line).
    #[arg(long, value_name = "TRIPLE")]
    target: Option<String>,

    /// Directory the release folder and archive are written under. Created
    /// if it does not already exist. Must not resolve under this
    /// workspace's `assets/`, `cache/`, or `imported/` directories (the
    /// same untracked payload/cache locations `cargo xtask policy` keeps
    /// off limits to tracked files).
    #[arg(long, value_name = "DIR", default_value = DEFAULT_OUT_DIR)]
    out_dir: PathBuf,

    /// Print the resolved target triple this invocation would build for,
    /// then exit without building or packaging anything.
    #[arg(long)]
    print_target: bool,
}

/// Removes `.`/`..` components from `path` purely lexically (no filesystem
/// access), so a not-yet-created `--out-dir` can still be checked against
/// [`crate::policy::PROHIBITED_PREFIXES`] before `create_dir_all` runs.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push(component);
                }
            }
            other => result.push(other),
        }
    }
    result
}

/// Resolves `out_dir` (as given on the command line, absolute or relative
/// to `root`) to an absolute, lexically normalized path, rejecting one that
/// falls under this workspace's payload/cache directories
/// ([`crate::policy::PROHIBITED_PREFIXES`]: `assets/`, `cache/`,
/// `imported/`) — a release archive has no business living where imported
/// media or provenance-sensitive local state does.
fn resolve_out_dir(root: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let absolute = if out_dir.is_absolute() {
        out_dir.to_path_buf()
    } else {
        root.join(out_dir)
    };
    let normalized = lexically_normalize(&absolute);

    for prefix in crate::policy::PROHIBITED_PREFIXES {
        let forbidden = lexically_normalize(&root.join(prefix.trim_end_matches('/')));
        if normalized == forbidden || normalized.starts_with(&forbidden) {
            return Err(format!(
                "--out-dir must not resolve under `{prefix}` (this project keeps payload/cache data there); got {}",
                normalized.display()
            ));
        }
    }
    Ok(normalized)
}

/// The Cargo `target/` directory a given `--target` build's artefacts land
/// under, mirroring Cargo's own `target/<triple>/<profile>` layout (host
/// builds skip the triple component).
fn binary_output_dir(root: &Path, target_triple: Option<&str>) -> PathBuf {
    let mut dir = root.join("target");
    if let Some(triple) = target_triple {
        dir = dir.join(triple);
    }
    dir.join("release")
}

/// Runs `cargo build --release -p ohl-app [--target <triple>]`.
fn build_release_binary(root: &Path, target_triple: Option<&str>) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = std::process::Command::new(cargo);
    command
        .current_dir(root)
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg(BINARY_CRATE_NAME);
    if let Some(triple) = target_triple {
        command.arg("--target").arg(triple);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to spawn cargo: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build failed with {status}"));
    }

    let platform = target_triple.map_or(host_platform(), target::classify);
    let file_name = target::binary_file_name(BINARY_NAME, platform);
    Ok(binary_output_dir(root, target_triple).join(file_name))
}

/// The platform implied by the toolchain actually running (used only when no
/// `--target` override narrows it down another way).
fn host_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Unix
    }
}

/// `<CARGO_HOME>/registry/src`, the root the license bundler searches under.
fn registry_src_root() -> Result<PathBuf, String> {
    let cargo_home = if let Some(value) = std::env::var_os("CARGO_HOME") {
        PathBuf::from(value)
    } else {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| "neither CARGO_HOME, HOME nor USERPROFILE is set".to_owned())?;
        PathBuf::from(home).join(".cargo")
    };
    Ok(cargo_home.join("registry").join("src"))
}

/// Runs the whole `cargo xtask dist` pipeline.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("dist".to_string()).chain(raw_args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            // clap uses exit code 0 for `--help`/`--version` and 2 for a
            // genuine usage error; honour whichever it picked rather than
            // always failing (the M9.3 task requires `--help`/`-h` to exit
            // 0, matching every other well-behaved CLI).
            return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(1));
        }
    };

    match run_inner(root, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(root: &Path, args: &Args) -> Result<(), String> {
    let target_triple = match &args.target {
        Some(triple) => triple.clone(),
        None => target::host_triple().map_err(|error| error.to_string())?,
    };

    if args.print_target {
        println!("{target_triple}");
        return Ok(());
    }

    let dist_output_root = resolve_out_dir(root, &args.out_dir)?;
    std::fs::create_dir_all(&dist_output_root).map_err(|error| {
        format!(
            "could not create --out-dir {}: {error}",
            dist_output_root.display()
        )
    })?;

    let version = version::resolve();
    let platform = target::classify(&target_triple);
    let host_is_linux_x86_64 = cfg!(all(target_os = "linux", target_arch = "x86_64"));
    let building_for_host = args.target.is_none();

    println!("Building {BINARY_NAME} {version} for {target_triple}...");
    let binary_path = build_release_binary(root, args.target.as_deref())?;
    if !binary_path.is_file() {
        return Err(format!(
            "expected release binary at {} but it does not exist",
            binary_path.display()
        ));
    }

    // The freestanding parser worker image is only ever produced on a
    // Linux x86-64 host, and only makes sense to bundle when the release
    // itself targets Linux x86-64 too (a cross-compiled release for another
    // target could not run it anyway).
    let worker_image_path = if host_is_linux_x86_64
        && building_for_host
        && matches!(platform, Platform::Unix)
        && target_triple.contains("linux")
    {
        println!("Building the parser worker image...");
        Some(
            ohl_parser_worker::build_parser_worker_image()
                .map_err(|error| format!("parser worker image: {error}"))?,
        )
    } else {
        println!(
            "Skipping the parser worker image (Linux x86-64 host builds targeting Linux only)"
        );
        None
    };

    println!("Collecting third-party license metadata...");
    let metadata = licenses::load_metadata(root)?;
    let license_entries = licenses::collect_license_entries(&metadata);
    let registry_src_root = registry_src_root()?;

    let binary_file_name = target::binary_file_name(BINARY_NAME, platform);
    let inputs = layout::DistInputs {
        binary_path: &binary_path,
        binary_file_name: &binary_file_name,
        worker_image_path: worker_image_path.as_deref(),
        license_path: &root.join("LICENSE"),
        third_party_notices_path: &root.join("THIRD_PARTY_NOTICES.md"),
        license_entries: &license_entries,
        registry_src_root: &registry_src_root,
        version: &version,
        target_triple: &target_triple,
    };

    println!("Assembling the release layout...");
    let dist_dir =
        layout::assemble(&inputs, &dist_output_root).map_err(|error| error.to_string())?;

    println!("Writing SHA256SUMS...");
    archive::write_sha256sums(&dist_dir).map_err(|error| error.to_string())?;

    let archive_extension = match platform {
        Platform::Windows => "zip",
        Platform::Unix => "tar.gz",
    };
    let archive_path = dist_output_root.join(format!(
        "{}.{archive_extension}",
        layout::release_folder_name(&version, &target_triple)
    ));
    println!("Writing {}...", archive_path.display());
    match platform {
        Platform::Windows => archive::create_zip(&dist_dir, &archive_path),
        Platform::Unix => archive::create_tar_gz(&dist_dir, &archive_path),
    }
    .map_err(|error| error.to_string())?;

    println!("Done: {}", dist_dir.display());
    println!("Archive: {}", archive_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser as _;

    use super::{Args, lexically_normalize, resolve_out_dir};

    #[test]
    fn help_flag_exits_zero() {
        let error = Args::try_parse_from(["dist", "--help"]).unwrap_err();
        assert_eq!(error.exit_code(), 0);
    }

    #[test]
    fn short_help_flag_exits_zero() {
        let error = Args::try_parse_from(["dist", "-h"]).unwrap_err();
        assert_eq!(error.exit_code(), 0);
    }

    #[test]
    fn unknown_flag_is_still_a_usage_error() {
        let error = Args::try_parse_from(["dist", "--bogus"]).unwrap_err();
        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn out_dir_defaults_to_target_dist() {
        let args = Args::try_parse_from(["dist"]).unwrap();
        assert_eq!(args.out_dir, Path::new("target/dist"));
    }

    #[test]
    fn existing_target_flag_still_parses() {
        let args = Args::try_parse_from(["dist", "--target", "x86_64-unknown-linux-gnu"])
            .expect("--target must keep working");
        assert_eq!(args.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn print_target_flag_parses() {
        let args = Args::try_parse_from(["dist", "--print-target"]).unwrap();
        assert!(args.print_target);
    }

    #[test]
    fn lexically_normalize_collapses_parent_components() {
        assert_eq!(
            lexically_normalize(Path::new("/a/b/../c/./d")),
            Path::new("/a/c/d")
        );
    }

    #[test]
    fn resolve_out_dir_accepts_a_plain_relative_directory() {
        let root = Path::new("/repo");
        assert_eq!(
            resolve_out_dir(root, Path::new("target/dist")).unwrap(),
            Path::new("/repo/target/dist")
        );
    }

    #[test]
    fn resolve_out_dir_rejects_assets() {
        let root = Path::new("/repo");
        let error = resolve_out_dir(root, Path::new("assets/dist")).unwrap_err();
        assert!(error.contains("assets/"), "unexpected message: {error}");
    }

    #[test]
    fn resolve_out_dir_rejects_cache_via_dotdot_traversal() {
        let root = Path::new("/repo");
        // `target/../cache/dist` normalizes to `/repo/cache/dist`, which
        // must be caught even though the literal string starts with
        // `target/`.
        let error = resolve_out_dir(root, Path::new("target/../cache/dist")).unwrap_err();
        assert!(error.contains("cache/"), "unexpected message: {error}");
    }

    #[test]
    fn resolve_out_dir_rejects_imported() {
        let root = Path::new("/repo");
        assert!(resolve_out_dir(root, Path::new("imported")).is_err());
    }

    #[test]
    fn resolve_out_dir_accepts_an_absolute_path_outside_the_workspace() {
        let root = Path::new("/repo");
        assert_eq!(
            resolve_out_dir(root, Path::new("/tmp/ohl-dist")).unwrap(),
            Path::new("/tmp/ohl-dist")
        );
    }
}
