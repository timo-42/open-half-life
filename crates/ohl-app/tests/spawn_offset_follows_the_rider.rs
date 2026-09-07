//! `--spawn-offset` must ride along with the player, not freeze in world
//! space (J1, `.plan/fidelity-round-8.md`).
//!
//! Before this fix, `--spawn-offset` was applied once via
//! `Game::set_viewpoint` (which enables noclip) right after the map
//! loaded, and then never touched again: on a map with a moving brush
//! entity (a `func_train`/tram), the player kept riding the mover away
//! while the frozen, noclipped camera stayed behind. This package proves
//! the fix two ways, entirely through the built binary's own log lines
//! (`--script-log`):
//!
//! - a sane offset (well above the player's own head) never lands inside
//!   solid geometry across an entire ride on `ohl-engine`'s synthetic
//!   mover fixture, even though the train has already moved well away
//!   from its spawn point by the end of the script;
//! - a badly-chosen offset that lands *inside* the train's own moving
//!   brush is still correctly caught by the "starts"/"ends inside solid
//!   geometry" warnings, proving those checks are actually exercised by
//!   this test setup rather than trivially passing because nothing here
//!   can ever be solid.
//!
//! `--spawn-offset` requires `--headless-screenshot` (clap's own
//! `requires` wiring), which needs a graphics adapter, so — matching
//! `tests/playable_loop.rs` — both tests here are `#[ignore]`d by default
//! and opted into with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{MOVER_MAP, MOVER_SEGMENT_LENGTH, MOVER_SPEED, mover_train_bsp};

const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// Ticks/second the scripted-input path (and the mover's own physics)
/// advances at; matches `ohl-app::game_run::CAPTURE_STEP`/
/// `ohl_engine::TICK_SECONDS`.
const TICKS_PER_SECOND: f32 = 60.0;

/// Comfortably more than one full segment's travel time, so the train has
/// finished its one-segment ride (and come to rest) well before the script
/// ends.
fn ride_ticks() -> u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ticks = ((MOVER_SEGMENT_LENGTH / MOVER_SPEED) * TICKS_PER_SECOND) as u32;
    ticks + 120
}

fn stage_payload(root: &Path) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(maps.join(format!("{MOVER_MAP}.bsp")), mover_train_bsp())
        .expect("stage the synthetic mover map");
}

fn run_scripted(root: &Path, script: &Path, screenshot: &Path, spawn_offset: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(root)
        .arg("--map")
        .arg(MOVER_MAP)
        .arg("--script")
        .arg(script)
        .arg("--script-log")
        .arg("--spawn-offset")
        .arg(spawn_offset)
        .arg("--headless-screenshot")
        .arg(screenshot)
        .output()
        .expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    stderr
}

fn a_sane_spawn_offset_rides_the_train_without_ever_landing_in_solid_geometry() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, format!("{} wait\n", ride_ticks()))
        .expect("write the scripted-input file");
    let shot = directory.path().join("capture.png");

    // Comfortably above the player's own head, so the offset never
    // overlaps the player or the train's own brush.
    let stderr = run_scripted(&root, &script_path, &shot, "0,0,40,0,0");

    assert!(stderr.contains("Scripted input finished."), "{stderr}");
    // Confirms the mover actually carried the player somewhere over the
    // course of the script, so this is a real ride, not a stall.
    assert!(stderr.contains("The player is riding a mover."), "{stderr}");
    assert!(
        !stderr.contains("Capture viewpoint starts inside solid geometry."),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Capture viewpoint ends inside solid geometry."),
        "{stderr}"
    );
}

fn a_spawn_offset_landing_inside_the_train_is_still_caught_as_solid() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, format!("{} wait\n", ride_ticks()))
        .expect("write the scripted-input file");
    let shot = directory.path().join("capture.png");

    // The player's eye rests well above the train's own brush (which
    // spans roughly -8..8 units above the train's origin); an offset this
    // far down places the rendered eye inside the train's own moving
    // collision brush for the whole ride, proving the checks below are
    // not vacuously true on this fixture.
    let stderr = run_scripted(&root, &script_path, &shot, "0,0,-68,0,0");

    assert!(stderr.contains("Scripted input finished."), "{stderr}");
    assert!(
        stderr.contains("Capture viewpoint starts inside solid geometry."),
        "{stderr}"
    );
    assert!(
        stderr.contains("Capture viewpoint ends inside solid geometry."),
        "{stderr}"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn spawn_offset_rides_the_train() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    a_sane_spawn_offset_rides_the_train_without_ever_landing_in_solid_geometry();
    a_spawn_offset_landing_inside_the_train_is_still_caught_as_solid();
}

#[test]
fn spawn_offset_rides_the_train_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the spawn-offset rider tests");
        return;
    }
    a_sane_spawn_offset_rides_the_train_without_ever_landing_in_solid_geometry();
    a_spawn_offset_landing_inside_the_train_is_still_caught_as_solid();
}
