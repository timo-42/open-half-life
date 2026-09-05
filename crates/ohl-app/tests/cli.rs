//! Integration tests: the built `open-half-life` binary against synthetic
//! ISO 9660/Joliet images.
//!
//! Every image byte comes from `ohl_iso9660::test_support`'s
//! project-authored builder, exposed here through that crate's
//! `test-support` dev-dependency feature; no byte, name, count, or listing
//! comes from any real medium. These tests never read a real ISO.

use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use ohl_iso9660::test_support::{self as fixture, Options};

/// Bytes for a synthetic image that neither the ISO 9660 nor the UDF
/// preflight recognizes: an all-zero buffer the right size to be a
/// plausible-looking image but matching no signature either preflight
/// checks for.
fn invalid_image_bytes() -> Vec<u8> {
    vec![0u8; 2_048 * 32]
}

fn write_temp_file(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    file.write_all(bytes).expect("write image");
    file.flush().expect("flush image");
    file
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_open-half-life")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .output()
        .expect("spawn open-half-life")
}

#[test]
fn success_path_mounts_and_prepares_the_cache() {
    let image = fixture::make_image(Options::default());
    let file = write_temp_file(&image);
    let cache_dir = tempfile::tempdir().expect("temp cache dir");

    let iso_path = file.path().to_str().expect("utf-8 temp path").to_owned();
    let cache_path = cache_dir
        .path()
        .to_str()
        .expect("utf-8 cache path")
        .to_owned();

    let output = run(&["--iso", &iso_path, "--cache", &cache_path]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "expected success, got {:?}: {stderr}",
        output.status
    );
    assert!(
        stderr.contains("Mounted read-only media image."),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("Prepared metadata-only media cache."),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains(
            "No supported payload container was found in the media; nothing was imported."
        ),
        "stderr: {stderr}"
    );

    // The manifest must exist, and must not contain the temp file's path or
    // its directory.
    let manifest = find_manifest(cache_dir.path()).expect("manifest exists");
    let manifest_contents = std::fs::read_to_string(&manifest).expect("read manifest");
    assert!(
        !manifest_contents.contains(&iso_path),
        "manifest leaked the source path: {manifest_contents}"
    );
    let temp_dir_name = std::env::temp_dir();
    let temp_dir_str = temp_dir_name.to_string_lossy();
    assert!(
        !manifest_contents.contains(temp_dir_str.as_ref()),
        "manifest leaked the temp directory: {manifest_contents}"
    );

    // A second run against the same cache directory reuses the entry.
    let output = run(&["--iso", &iso_path, "--cache", &cache_path]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "expected success: {stderr}");
    assert!(
        stderr.contains("Reused metadata-only media cache."),
        "stderr: {stderr}"
    );
}

/// Walks `root` looking for the one file named after `ohl_media`'s manifest
/// file name, without depending on `ohl-media` as a dependency of this test
/// binary.
fn find_manifest(root: &Path) -> Option<std::path::PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|name| name.to_str()) == Some("provenance.json") {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn invalid_image_fails_preflight() {
    let file = write_temp_file(&invalid_image_bytes());
    let cache_dir = tempfile::tempdir().expect("temp cache dir");
    let iso_path = file.path().to_str().expect("utf-8 temp path");
    let cache_path = cache_dir.path().to_str().expect("utf-8 cache path");

    let output = run(&["--iso", iso_path, "--cache", cache_path]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("Media preflight failed:"),
        "stderr: {stderr}"
    );
}

#[test]
fn missing_file_fails_preflight() {
    let cache_dir = tempfile::tempdir().expect("temp cache dir");
    let missing = cache_dir.path().join("does-not-exist.iso");
    let missing_str = missing.to_str().expect("utf-8 path");
    let cache_path = cache_dir.path().to_str().expect("utf-8 cache path");

    let output = run(&["--iso", missing_str, "--cache", cache_path]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("Media preflight failed:"),
        "stderr: {stderr}"
    );
}

#[test]
fn directory_fails_preflight() {
    let dir = tempfile::tempdir().expect("temp dir");
    let cache_dir = tempfile::tempdir().expect("temp cache dir");
    let dir_str = dir.path().to_str().expect("utf-8 path");
    let cache_path = cache_dir.path().to_str().expect("utf-8 cache path");

    let output = run(&["--iso", dir_str, "--cache", cache_path]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("Media preflight failed:"),
        "stderr: {stderr}"
    );
}
