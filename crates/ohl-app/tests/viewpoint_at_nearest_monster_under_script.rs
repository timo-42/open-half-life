//! `--viewpoint-at-nearest-monster` must not be a silent no-op under
//! `--script` (fidelity finding J4, recorded in local investigation
//! notes and not part of the repository).
//!
//! Before this fix, `place_viewpoint_near_nearest_monster` was called only
//! from the frame-count `capture()` path; `run_scripted()` never called it
//! at all, so combining the two flags produced an ordinary player-start
//! capture with no error, no warning, and no "No monster found" line
//! either — the flag was simply never consulted. This package proves the
//! fix: the fixed "Capture viewpoint placed near a monster." line now
//! prints under `--script` too, applied once right after the map loads
//! and before the script's own ticks run.
//!
//! Only compiled with the non-default `dev-tools` cargo feature, which
//! gates `--viewpoint-at-nearest-monster` itself.
//!
//! `--viewpoint-at-nearest-monster` requires `--headless-screenshot`
//! (clap's own `requires` wiring), which needs a graphics adapter, so —
//! matching `tests/playable_loop.rs` — this test is `#[ignore]`d by
//! default and opted into with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.
#![cfg(feature = "dev-tools")]

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{AI_MAP, ai_room_bsp};

const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// A room with a player start facing `+X` and a single monster nearby, the
/// same shape `ohl-engine`'s own `tests/monster_attacks_player.rs` builds.
fn entities() -> String {
    "{\n\"classname\" \"worldspawn\"\n}\n\
     {\n\"classname\" \"info_player_start\"\n\"origin\" \"-64 0 36\"\n\"angle\" \"0\"\n}\n\
     {\n\"classname\" \"monster_human_grunt\"\n\"origin\" \"64 0 36\"\n\"angle\" \"180\"\n}\n"
        .to_string()
}

fn stage_payload(root: &Path) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{AI_MAP}.bsp")),
        ai_room_bsp(&entities(), false),
    )
    .expect("stage the synthetic AI room");
}

fn run() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, "30 wait\n").expect("write the scripted-input file");
    let shot = directory.path().join("capture.png");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(AI_MAP)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log")
        .arg("--viewpoint-at-nearest-monster")
        .arg("96")
        .arg("--headless-screenshot")
        .arg(&shot)
        .output()
        .expect("spawn open-half-life");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    assert!(
        stderr.contains("Capture viewpoint placed near a monster."),
        "the nearest-monster placement never ran under --script: {stderr}"
    );
    assert!(stderr.contains("Scripted input finished."), "{stderr}");
    assert!(shot.is_file(), "no capture was written: {stderr}");
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn viewpoint_at_nearest_monster_applies_under_script() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run();
}

#[test]
fn viewpoint_at_nearest_monster_applies_under_script_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the nearest-monster-under-script test");
        return;
    }
    run();
}
