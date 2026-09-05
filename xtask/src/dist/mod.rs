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

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use target::Platform;

const BINARY_CRATE_NAME: &str = "ohl-app";
const BINARY_NAME: &str = "open-half-life";

/// `cargo xtask dist [--target <triple>]`.
struct Args {
    /// `None` means "build for the host".
    target: Option<String>,
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut target = None;
    let mut iter = raw.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--target requires a value".to_owned())?;
                target = Some(value.clone());
            }
            other if other.starts_with("--target=") => {
                target = Some(other["--target=".len()..].to_owned());
            }
            other => return Err(format!("unrecognised argument `{other}`")),
        }
    }
    Ok(Args { target })
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
    match run_inner(root, raw_args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(root: &Path, raw_args: &[String]) -> Result<(), String> {
    let args = parse_args(raw_args)?;
    let version = version::resolve();
    let target_triple = match &args.target {
        Some(triple) => triple.clone(),
        None => target::host_triple().map_err(|error| error.to_string())?,
    };
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

    let dist_output_root = root.join("target").join("dist");
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
