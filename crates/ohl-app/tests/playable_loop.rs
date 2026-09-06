//! The production playable loop end to end: the built binary, pointed at a
//! synthetic payload tree, renders offscreen and writes a PNG.
//!
//! The map is `ohl-engine`'s project-authored synthetic fixture, staged the
//! way an imported payload looks (`<root>/<tree>/files/valve/maps/...`). No
//! byte here comes from any game installation; see `docs/CLEAN_ROOM.md`.
//!
//! Rendering needs a graphics adapter, so this is `#[ignore]`d by default
//! and opted into with `OHL_RENDER_GPU_TEST=1`, matching `ohl-render`'s and
//! `ohl-engine`'s own headless tests.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{SYNTHETIC_MAP, synthetic_map_bsp};

const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// Stages the synthetic map as a published payload tree and returns the
/// payload *root* the binary is pointed at.
fn stage_payload(root: &Path) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{SYNTHETIC_MAP}.bsp")),
        synthetic_map_bsp(),
    )
    .expect("stage the synthetic map");
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn a_headless_capture_writes_a_png() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run();
}

#[test]
fn a_headless_capture_writes_a_png_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the headless capture test");
        return;
    }
    run();
}

fn run() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let shot = directory.path().join("capture.png");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(SYNTHETIC_MAP)
        .arg("--headless-screenshot")
        .arg(&shot)
        .arg("--frames")
        .arg("3")
        .arg("--viewpoint")
        .arg("0,0,150,70,0")
        .output()
        .expect("spawn open-half-life");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the capture run failed: {stderr}");
    assert!(shot.is_file(), "no capture was written: {stderr}");

    let decoded = image::open(&shot).expect("the capture decodes as an image");
    let rgba = decoded.to_rgba8();
    assert_eq!(rgba.width(), 1280);
    assert_eq!(rgba.height(), 720);
    // The clear colour is a near-black blue; the lit synthetic room must
    // put clearly brighter pixels on screen.
    let lit = rgba.pixels().filter(|pixel| pixel.0[0] > 40).count();
    assert!(
        lit > (rgba.width() * rgba.height()) as usize / 20,
        "the capture is empty background, only {lit} lit pixels"
    );
}

/// `--script` runs headlessly, with no graphics adapter needed, and
/// `--script-log` emits the two fixed milestone lines this run always
/// prints (see `crate::script_log` in `ohl-app`; the four combat lines are
/// TODO(P1) hooks and never fire on this branch).
#[test]
fn scripted_input_runs_headlessly_and_logs_its_fixed_milestone_lines() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, "5 forward\n2 look 0 -10\n3 wait\n")
        .expect("write the scripted-input file");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(SYNTHETIC_MAP)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log")
        .output()
        .expect("spawn open-half-life");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    assert!(stderr.contains("Scripted input loaded."), "{stderr}");
    assert!(stderr.contains("Scripted input finished."), "{stderr}");
    // None of the four P1-only lines can fire without a weapon, pickup or
    // player-damage path in this tree.
    for absent in [
        "The player fired a weapon.",
        "A shot hit an entity.",
        "A pickup was collected.",
        "The player took damage.",
    ] {
        assert!(!stderr.contains(absent), "unexpectedly logged: {absent}");
    }
}

#[test]
fn an_empty_payload_root_is_a_reported_failure_not_a_panic() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    std::fs::create_dir_all(&root).expect("create the payload root");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--play")
        .output()
        .expect("spawn open-half-life");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No imported payload was found"),
        "unexpected diagnostic: {stderr}"
    );
}
