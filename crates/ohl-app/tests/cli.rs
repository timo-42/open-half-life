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

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
mod support;

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

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
#[test]
fn missing_worker_image_reports_the_safe_cause_and_profile_matched_install_command() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("temporary directory");
    // Reach the missing-image lookup even under a group-writable umask.
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("trusted executable directory");
    let copied_binary = directory.path().join("open-half-life");
    std::fs::copy(binary(), &copied_binary).expect("copy application without its worker image");

    let iso = directory.path().join("synthetic.iso");
    std::fs::write(&iso, support::synthetic_wise_iso()).expect("synthetic iso fixture");
    let cache = directory.path().join("cache");
    let payload = directory.path().join("payload");
    let output = Command::new(&copied_binary)
        .args([
            "--iso",
            iso.to_str().expect("utf-8 path"),
            "--cache",
            cache.to_str().expect("utf-8 path"),
            "--payload-root",
            payload.to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("spawn copied open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains(
            "Payload import failed: no parser worker could be launched: isolated worker: service \
             image is not installed."
        ),
        "stderr: {stderr}"
    );
    #[cfg(debug_assertions)]
    assert!(
        stderr.contains("`cargo xtask worker-image`"),
        "debug hint did not match the binary profile: {stderr}"
    );
    #[cfg(not(debug_assertions))]
    assert!(
        stderr.contains("`cargo run --release -p xtask -- worker-image`"),
        "release hint did not match the binary profile: {stderr}"
    );
    assert!(!stderr.contains(iso.to_str().expect("utf-8 path")));
    assert!(!stderr.contains(payload.to_str().expect("utf-8 path")));
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

#[test]
fn the_difficulty_flag_only_accepts_the_three_documented_levels() {
    let output = run(&["--difficulty", "nightmare", "--play"]);
    assert!(
        !output.status.success(),
        "an unknown difficulty is rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("difficulty"),
        "the usage error names the offending flag"
    );
}

#[test]
fn the_overbright_flag_rejects_a_value_outside_its_documented_bounds() {
    let output = run(&["--overbright=-5", "--play"]);
    assert!(
        !output.status.success(),
        "a negative overbright multiplier is rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected a finite number greater than 0 and at most 8.0"),
        "the usage error is the fixed overbright message, got: {stderr}"
    );
}

#[test]
fn a_save_slot_and_a_start_map_are_mutually_exclusive() {
    let output = run(&["--load", "quicksave", "--map", "ohlsynth"]);
    assert!(
        !output.status.success(),
        "--load and --map cannot both be given"
    );
}

#[test]
fn script_log_requires_a_script() {
    let output = run(&["--script-log", "--map", "ohlsynth"]);
    assert!(
        !output.status.success(),
        "--script-log without --script is a usage error"
    );
}

#[test]
fn the_help_text_lists_the_campaign_flow_flags() {
    let output = run(&["--help"]);
    assert!(output.status.success(), "--help succeeds");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--load"), "--load is documented");
    assert!(stdout.contains("--script"), "--script is documented");
    assert!(
        stdout.contains("--difficulty"),
        "--difficulty is documented"
    );
}

#[test]
fn query_cli_selects_exact_wire_requests_without_media_and_redacts_metadata() {
    use std::net::UdpSocket;
    use std::thread;
    use std::time::Duration;

    for profile in [None, Some("a2s"), Some("won46")] {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let address = socket.local_addr().unwrap().to_string();
        let handle = thread::spawn(move || {
            let mut received = [0; 64];
            let (length, client) = socket.recv_from(&mut received).unwrap();
            // Independently authored messages, never original-server fixtures.
            let reply = if profile == Some("won46") {
                assert_eq!(&received[..length], b"\xff\xff\xff\xffdetails");
                b"\xff\xff\xff\xffmunlisted-address\0unlisted-server\0unlisted-map\0unlisted-dir\0unlisted-description\0\x02\x08\x2edl\x01\0\0\xa7".to_vec()
            } else {
                assert_eq!(
                    &received[..length],
                    b"\xff\xff\xff\xffTSource Engine Query\0"
                );
                let mut reply = b"\xff\xff\xff\xffI\x30unlisted-server\0unlisted-map\0unlisted-dir\0unlisted-description\0".to_vec();
                reply.extend_from_slice(&70_u16.to_le_bytes());
                reply.extend_from_slice(&[2, 8, 0, b'd', b'l', 1, 0]);
                reply.extend_from_slice(b"unlisted-version\0");
                reply
            };
            socket.send_to(&reply, client).unwrap();
        });
        let mut args = vec!["--query-server", address.as_str()];
        if let Some(profile) = profile {
            args.extend(["--query-profile", profile]);
        }
        let output = run(&args);
        handle.join().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let protocol = if profile == Some("won46") { 46 } else { 48 };
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "Server query succeeded: protocol={protocol}, players=2, capacity=8, password_protected=true.\n"
            )
        );
    }
}

#[test]
fn query_profiles_require_a_target_and_reject_conflicting_modes_before_import() {
    for args in [
        vec!["--query-profile", "won46"],
        vec!["--query-profile", "a2s"],
        vec![
            "--query-server",
            "127.0.0.1:1",
            "--query-profile",
            "unknown",
        ],
        vec![
            "--query-server",
            "127.0.0.1:1",
            "--query-profile",
            "a2s",
            "--query-profile",
            "won46",
        ],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let modes = vec![
        vec!["--iso", "nonexistent.iso"],
        vec!["nonexistent.iso"],
        vec!["--play"],
        vec!["--training"],
        vec!["--skirmish"],
        vec!["--map", "synthetic"],
        vec!["--load", "synthetic"],
        vec!["--script", "synthetic"],
        vec!["--chain-script", "synthetic"],
        vec!["--headless-screenshot", "synthetic.png"],
        vec!["--benchmark-seconds", "1"],
        vec!["--profile-frames"],
    ];
    #[cfg(feature = "dev-tools")]
    let modes = {
        let mut modes = modes;
        modes.extend([
            vec!["--dev-bsp", "synthetic.bsp"],
            vec!["--dev-mdl", "synthetic.mdl"],
            vec!["--reachability-report"],
            vec!["--plan-route", "synthetic.json"],
        ]);
        modes
    };
    for mode in modes {
        let mut args = vec!["--query-server", "127.0.0.1:1", "--query-profile", "won46"];
        args.extend(mode);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("Mounted"));
        assert!(!stderr.contains("Media preflight"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn query_cli_failure_does_not_publish_peer_text_or_addresses() {
    use std::net::UdpSocket;
    use std::thread;
    use std::time::Duration;

    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let address = socket.local_addr().unwrap().to_string();
    let handle = thread::spawn(move || {
        let mut received = [0; 64];
        let (length, client) = socket.recv_from(&mut received).unwrap();
        assert_eq!(&received[..length], b"\xff\xff\xff\xffdetails");
        socket
            .send_to(b"\xff\xff\xff\xffmnever-log-this-peer-text", client)
            .unwrap();
    });
    let output = run(&["--query-server", &address, "--query-profile", "won46"]);
    handle.join().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Server query failed: no supported response was received.\n"
    );
}
