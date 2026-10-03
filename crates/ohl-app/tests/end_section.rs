//! A `trigger_endsection` ends a scripted run where it stands.
//!
//! TWHL's `trigger_endsection` page documents the entity as one that "ends the
//! current game and returns the player to the game's main menu"
//! (`docs/FORMAT_SOURCES.md`, "Map entities the registry used to drop"). A
//! headless `--script` run has no menu to return to, so there the section
//! ending is the end of the run: none of the script's remaining ticks run. The
//! fixture proves it by scheduling a level change *after* the section ends — a
//! run that kept ticking would report the change firing.
//!
//! The map is `ohl-engine`'s project-authored brush-floor fixture, staged
//! the way an imported payload looks. No byte here comes from any game
//! installation; see `docs/CLEAN_ROOM.md`.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{
    SYNTHETIC_MAP, killable_brush_floor_bsp, synthetic_map_bsp_with_extra_entity,
};

const MAP: &str = "ohlendsectionrunsynth";

/// The line `ohl-app` logs when a section ends.
const SECTION_ENDED: &str = "The section ended.";

/// The line `ohl-app` logs when a level change fires and is not followed.
const CHANGE_NOT_FOLLOWED: &str = "A level change fired during capture; it was not followed.";

/// A floor, a spawn, a USE-only `trigger_changelevel` fired by a
/// `trigger_auto` a second in, and — when `end_section` — a
/// `trigger_endsection` fired by another `trigger_auto` half a second in.
fn entities(end_section: bool) -> String {
    let mut out = String::from(
        "{\n\"classname\" \"worldspawn\"\n}\n\
         {\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 40\"\n\"angle\" \"0\"\n}\n\
         {\n\"classname\" \"func_wall\"\n\"model\" \"*1\"\n}\n\
         {\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_change\"\n\
         \"map\" \"ohlnowheresynth\"\n\"spawnflags\" \"2\"\n}\n\
         {\n\"classname\" \"trigger_auto\"\n\"target\" \"ohl_change\"\n\"delay\" \"1\"\n}\n",
    );
    if end_section {
        out.push_str(
            "{\n\"classname\" \"trigger_endsection\"\n\"targetname\" \"ohl_end\"\n\
             \"section\" \"ohl_test_section\"\n\"spawnflags\" \"1\"\n}\n\
             {\n\"classname\" \"trigger_auto\"\n\"target\" \"ohl_end\"\n\"delay\" \"0.5\"\n}\n",
        );
    }
    out
}

fn stage_payload(root: &Path, end_section: bool) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{MAP}.bsp")),
        killable_brush_floor_bsp(&entities(end_section)),
    )
    .expect("stage the synthetic map");
}

/// The opt-in a GPU-backed capture test needs, as `playable_loop.rs` uses.
const GPU_OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// Runs a three-second `wait` script over the fixture and returns stderr.
fn run(end_section: bool) -> String {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root, end_section);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, "180 wait\n").expect("write the scripted-input file");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(MAP)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log")
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    stderr
}

/// The control: with no `trigger_endsection` the scheduled level change
/// fires inside the script's three seconds, so the assertion below is
/// about the section ending and not about a fixture that never fires.
#[test]
fn without_an_end_section_the_scheduled_change_fires() {
    let stderr = run(false);
    assert!(stderr.contains(CHANGE_NOT_FOLLOWED), "{stderr}");
    assert!(!stderr.contains(SECTION_ENDED), "{stderr}");
}

/// The section ends half a second in, once, and the run stops there: the
/// level change scheduled for a second in never gets a tick to fire on.
#[test]
fn an_ended_section_ends_the_scripted_run_where_it_stands() {
    let stderr = run(true);
    assert_eq!(
        stderr.matches(SECTION_ENDED).count(),
        1,
        "the section ends exactly once: {stderr}"
    );
    assert!(
        !stderr.contains(CHANGE_NOT_FOLLOWED),
        "ticks ran after the section ended: {stderr}"
    );
    assert!(stderr.contains("Scripted input finished."), "{stderr}");
}

/// A still capture stops at the section end too: it keeps the frame of the
/// tick the section ended on, and never ticks on to the level change the
/// fixture schedules after it. Rendering needs a graphics adapter, so this
/// runs only when opted into, like `playable_loop.rs`'s capture test.
#[test]
fn a_capture_stops_at_the_section_end_when_opted_in() {
    if std::env::var_os(GPU_OPT_IN).is_none() {
        eprintln!("set {GPU_OPT_IN}=1 to run the end-section capture test");
        return;
    }
    // The brush-floor fixture has nothing to draw, so the capture uses the
    // lit synthetic room instead, whose own named `trigger_changelevel`
    // the second `trigger_auto` fires.
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{SYNTHETIC_MAP}.bsp")),
        synthetic_map_bsp_with_extra_entity(
            "ohlnowheresynth",
            "{\n\"classname\" \"trigger_endsection\"\n\"targetname\" \"ohl_end\"\n\
             \"section\" \"ohl_test_section\"\n\"spawnflags\" \"1\"\n}\n\
             {\n\"classname\" \"trigger_auto\"\n\"target\" \"ohl_end\"\n\"delay\" \"0.5\"\n}\n\
             {\n\"classname\" \"trigger_auto\"\n\"target\" \"ohl_exit\"\n\"delay\" \"1\"\n}\n",
        ),
    )
    .expect("stage the synthetic map");
    let shot = directory.path().join("capture.png");

    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(SYNTHETIC_MAP)
        .arg("--headless-screenshot")
        .arg(&shot)
        .arg("--frames")
        .arg("180")
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the capture failed: {stderr}");
    assert!(shot.is_file(), "the last frame is still written: {stderr}");
    assert_eq!(stderr.matches(SECTION_ENDED).count(), 1, "{stderr}");
    assert!(
        !stderr.contains(CHANGE_NOT_FOLLOWED),
        "the capture ticked on after the section ended: {stderr}"
    );
}
