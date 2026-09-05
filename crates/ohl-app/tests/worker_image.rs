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
//! The medium is a project-authored synthetic ISO holding one synthetic PE
//! with an InstallShield 3 Z signature in its overlay. The expectation is the
//! sanitized `unsupported` outcome: this build's dispatcher refuses every
//! enumeration, so the run must reach the worker, be refused, and exit 0
//! having published nothing.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

mod support;

use std::path::Path;
use std::process::Command;

use support::synthetic_container_iso;

const UNSUPPORTED_LINE: &str = "Payload import is not supported by this build's parser worker yet; no media executable was run.";

/// Removes the group and world write bits from one directory.
fn tighten(directory: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let Ok(metadata) = std::fs::metadata(directory) else {
        return;
    };
    let mode = metadata.permissions().mode() & !0o022;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode));
}

#[test]
#[ignore = "needs `cargo xtask worker-image`; launches a confined process"]
fn the_installed_worker_refuses_the_enumeration_and_the_app_still_succeeds() {
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
