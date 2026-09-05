//! The composition root against the *real* installed parser-worker image.
//!
//! This is the only test that launches a confined process. It is `#[ignore]`d
//! because it needs the image installed beside the built binary first, and it
//! runs only on the one tuple that has a containment backend:
//!
//! ```text
//! cargo xtask worker-image
//! cargo test -p ohl-app --test worker_image -- --ignored
//! ```
//!
//! Both media are project-authored synthetic ISOs holding one synthetic PE:
//!
//! - one whose overlay carries a whole synthetic Wise package, which must be
//!   located, enumerated, streamed and published, with every published file's
//!   checksum matching what the writer put in;
//! - one whose overlay only *starts* with an InstallShield 3 Z signature and
//!   is not an archive, which the worker must refuse, leaving the run to exit
//!   0 having published nothing.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

mod support;

use std::path::Path;
use std::process::Command;

use support::{synthetic_container_iso, synthetic_wise_files, synthetic_wise_iso};

const UNSUPPORTED_LINE: &str = "Payload import is not supported for this medium's container format; no media executable was run.";

/// Removes the group and world write bits from one directory.
fn tighten(directory: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let Ok(metadata) = std::fs::metadata(directory) else {
        return;
    };
    let mode = metadata.permissions().mode() & !0o022;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode));
}

/// The installed image, with every directory on its path tightened.
fn installed_image() -> std::path::PathBuf {
    let binary = env!("CARGO_BIN_EXE_open-half-life");
    let installed = std::path::Path::new(binary)
        .parent()
        .expect("binary directory")
        .join("libexec")
        .join("open-half-life")
        .join("ohl-media-parser-worker");
    assert!(
        installed.exists(),
        "run `cargo xtask worker-image` first: {}",
        installed.display()
    );

    // The launcher refuses a group- or world-writable component anywhere on
    // the image's path, and cargo creates `target/<profile>` group-writable
    // under the common `umask 002`. Tightening it changes nothing cargo
    // relies on and is what a real installation would already have.
    tighten(installed.parent().expect("image directory"));
    tighten(
        installed
            .parent()
            .and_then(Path::parent)
            .expect("libexec directory"),
    );
    tighten(Path::new(binary).parent().expect("binary directory"));
    installed
}

#[test]
#[ignore = "needs `cargo xtask worker-image`; launches a confined process"]
fn the_installed_worker_refuses_a_container_it_cannot_decode() {
    let binary = env!("CARGO_BIN_EXE_open-half-life");
    let _installed = installed_image();

    let directory = tempfile::tempdir().expect("temporary directory");
    let iso = directory.path().join("synthetic.iso");
    std::fs::write(&iso, synthetic_container_iso()).expect("synthetic iso fixture");
    let payload_root = directory.path().join("payload");

    let output = Command::new(binary)
        .args([
            "--iso",
            iso.to_str().expect("utf-8 path"),
            "--cache",
            directory.path().join("cache").to_str().expect("utf-8 path"),
            "--payload-root",
            payload_root.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "expected exit 0: {stderr}");
    assert!(stderr.contains(UNSUPPORTED_LINE), "stderr: {stderr}");

    // Nothing may be published.
    let published: Vec<String> = std::fs::read_dir(&payload_root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        published.iter().all(|name| !name.starts_with("ohl-tree-")),
        "{published:?}"
    );
}

/// The `files` directory of the one published payload tree under `root`.
fn published_tree(root: &Path) -> std::path::PathBuf {
    let mut trees: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .expect("the payload root exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("ohl-tree-"))
        })
        .collect();
    assert_eq!(trees.len(), 1, "exactly one published tree: {trees:?}");
    trees.remove(0).join("files")
}

#[test]
#[ignore = "needs `cargo xtask worker-image`; launches a confined process"]
fn the_installed_worker_imports_a_synthetic_wise_package() {
    let binary = env!("CARGO_BIN_EXE_open-half-life");
    let _installed = installed_image();

    let directory = tempfile::tempdir().expect("temporary directory");
    let iso = directory.path().join("wise.iso");
    std::fs::write(&iso, synthetic_wise_iso()).expect("synthetic iso fixture");
    let payload_root = directory.path().join("payload");

    let output = Command::new(binary)
        .args([
            "--iso",
            iso.to_str().expect("utf-8 path"),
            "--cache",
            directory.path().join("cache").to_str().expect("utf-8 path"),
            "--payload-root",
            payload_root.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "expected exit 0: {stderr}");
    assert!(stderr.contains("Payload imported."), "stderr: {stderr}");
    assert!(
        stderr.contains("Payload import complete."),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains(UNSUPPORTED_LINE), "stderr: {stderr}");

    // Every file the writer put in is published under its recorded relative
    // path, with the exact bytes it was given.
    let files = published_tree(&payload_root);
    for file in synthetic_wise_files() {
        let relative = String::from_utf8(file.path.clone()).expect("ascii path");
        let mut published = files.clone();
        for component in relative.split('\\') {
            published.push(component);
        }
        let staged = std::fs::read(&published)
            .unwrap_or_else(|error| panic!("published {}: {error}", published.display()));
        assert_eq!(
            ohl_wise::crc32(&staged),
            ohl_wise::crc32(&file.content),
            "checksum of {}",
            published.display()
        );
        assert_eq!(staged.len(), file.content.len());
    }

    // The package's unclaimed streams — its bitmap and its script — are
    // published under the reserved directory rather than dropped.
    assert!(files.join("unnamed").is_dir());

    // A second run finds the payload already published and does no work.
    let second = Command::new(binary)
        .args([
            "--iso",
            iso.to_str().expect("utf-8 path"),
            "--cache",
            directory.path().join("cache").to_str().expect("utf-8 path"),
            "--payload-root",
            payload_root.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "expected exit 0: {stderr}");
    assert!(
        stderr.contains("Payload already imported."),
        "stderr: {stderr}"
    );
}
