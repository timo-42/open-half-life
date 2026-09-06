//! `--follow-level-change`: a headless/scripted run either follows a
//! `trigger_changelevel` onto its destination map or stays put, matching
//! `crate::game_run`'s own logging policy.
//!
//! The fixture is `ohl-engine`'s own project-authored, project-owned
//! `test_support::script_room_bsp`/`script_room_entities` two-map building
//! blocks (a flat room with no obstacles, published under
//! `test_support::SCRIPT_MAP` and `test_support::NEXT_MAP`, joined by
//! `test_support::LANDMARK`), the same fixture family `ohl-engine`'s own
//! `tests/scripted_input.rs` uses for scripted movement. No byte here comes
//! from any game installation; see `docs/CLEAN_ROOM.md`.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{
    LANDMARK, NEXT_MAP, SCRIPT_MAP, TOUCH_CHANGELEVEL_MAP, door_behind_touch_trigger_bsp,
    script_room_bsp, script_room_entities, synthetic_map_bsp, touch_changelevel_entities,
};

/// The `func_button`'s reach, a `trigger_changelevel` and a landmark, both
/// maps declaring the same landmark name so the transition has somewhere
/// to place the player.
fn map_a_entities() -> String {
    let extra = format!(
        "{{\n\"classname\" \"func_button\"\n\"target\" \"ohl_exit\"\n\
         \"origin\" \"0 150 36\"\n\"speed\" \"50\"\n\"wait\" \"1\"\n\"delay\" \"0\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"0 0 0\"\n}}\n\
         {{\n\"classname\" \"trigger_changelevel\"\n\"targetname\" \"ohl_exit\"\n\
         \"map\" \"{NEXT_MAP}\"\n\"landmark\" \"{LANDMARK}\"\n}}\n"
    );
    script_room_entities([0.0, 0.0, 36.0], &extra)
}

/// The destination map: just a player start and the shared landmark.
fn next_map_entities() -> String {
    let extra = format!(
        "{{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"0 0 0\"\n}}\n"
    );
    script_room_entities([0.0, 0.0, 36.0], &extra)
}

/// A scripted walk from the map A spawn to its button and a press: turn 90
/// degrees over 30 ticks (facing the button, which sits on the `+Y` axis
/// from spawn), walk forward 40 ticks (comfortably inside the button's use
/// radius by then, calibrated against this exact fixture), then `use`.
/// Calibrated so the run finishes well inside `ohl-app`'s scripted-input
/// limits with room to spare.
const WALK_INTO_BUTTON_SCRIPT: &str = "30 look 0 90\n40 forward\n1 use\n20 wait\n";

/// Stages both maps as a published payload tree and returns the payload
/// *root* the binary is pointed at.
fn stage_payload(root: &Path) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{SCRIPT_MAP}.bsp")),
        script_room_bsp(&map_a_entities()),
    )
    .expect("stage map A");
    std::fs::write(
        maps.join(format!("{NEXT_MAP}.bsp")),
        script_room_bsp(&next_map_entities()),
    )
    .expect("stage the destination map");
}

fn run_walk_script(follow: bool) -> String {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, WALK_INTO_BUTTON_SCRIPT).expect("write the scripted-input file");

    let mut command = Command::new(env!("CARGO_BIN_EXE_open-half-life"));
    command
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(SCRIPT_MAP)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log");
    if follow {
        command.arg("--follow-level-change");
    }

    let output = command.output().expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    stderr
}

#[test]
fn walking_into_the_changelevel_trigger_follows_with_the_flag() {
    let stderr = run_walk_script(true);
    assert!(
        stderr.contains("A level change was followed."),
        "expected the follow line: {stderr}"
    );
    assert!(
        !stderr.contains("A level change fired during capture; it was not followed."),
        "the \"not followed\" line should not fire once the change was followed: {stderr}"
    );
}

#[test]
fn walking_into_the_changelevel_trigger_stays_put_without_the_flag() {
    let stderr = run_walk_script(false);
    assert!(
        stderr.contains("A level change fired during capture; it was not followed."),
        "expected the default \"not followed\" line: {stderr}"
    );
    assert!(
        !stderr.contains("A level change was followed."),
        "the follow line must not fire without --follow-level-change: {stderr}"
    );
}

// --- Touch-fired `trigger_changelevel` (no `use` involved at all) --------
//
// `walking_into_the_changelevel_trigger_*` above covers the pre-existing
// `use` path (a `func_button` targeting a `trigger_changelevel`); the pair
// below covers the touch path this crate's own fix adds: a plain
// `trigger_changelevel` volume the player's own bounding box overlaps,
// with nothing pressed at all, using `ohl-engine`'s
// `test_support::touch_changelevel_entities`/`door_behind_touch_trigger_bsp`
// fixture (the same one `crates/ohl-engine/tests/changelevel_touch.rs`
// exercises directly against `Game::tick`). `--follow-level-change` does
// not care which activation path produced the `GameEvent::LevelChange`, so
// this only needs to confirm the flag still gates the transition the same
// way once the event's source is a touch instead of a `use`.

/// A scripted straight walk from the fixture's player start (`-96 0 36`,
/// already facing `+X`) into the touch-trigger volume at `x` in
/// `[64, 128]` — the same 300-tick forward walk
/// `crates/ohl-engine/tests/touch_trigger_door.rs` and
/// `crates/ohl-engine/tests/changelevel_touch.rs` already use for this
/// geometry.
const WALK_INTO_TOUCH_VOLUME_SCRIPT: &str = "300 forward\n20 wait\n";

/// Stages the touch-`trigger_changelevel` source map and the same
/// destination map fixture `ohl-engine`'s own `game_loop.rs` tests use,
/// under [`TOUCH_CHANGELEVEL_MAP`]/[`NEXT_MAP`].
fn stage_touch_payload(root: &Path) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{TOUCH_CHANGELEVEL_MAP}.bsp")),
        door_behind_touch_trigger_bsp(&touch_changelevel_entities(NEXT_MAP)),
    )
    .expect("stage the touch-changelevel source map");
    std::fs::write(maps.join(format!("{NEXT_MAP}.bsp")), synthetic_map_bsp())
        .expect("stage the destination map");
}

fn run_touch_walk_script(follow: bool) -> String {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_touch_payload(&root);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, WALK_INTO_TOUCH_VOLUME_SCRIPT)
        .expect("write the scripted-input file");

    let mut command = Command::new(env!("CARGO_BIN_EXE_open-half-life"));
    command
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(TOUCH_CHANGELEVEL_MAP)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log");
    if follow {
        command.arg("--follow-level-change");
    }

    let output = command.output().expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the scripted run failed: {stderr}");
    stderr
}

#[test]
fn walking_into_the_touch_changelevel_volume_follows_with_the_flag() {
    let stderr = run_touch_walk_script(true);
    assert!(
        stderr.contains("A level change was followed."),
        "expected the follow line: {stderr}"
    );
    assert!(
        !stderr.contains("A level change fired during capture; it was not followed."),
        "the \"not followed\" line should not fire once the change was followed: {stderr}"
    );
}

#[test]
fn walking_into_the_touch_changelevel_volume_stays_put_without_the_flag() {
    let stderr = run_touch_walk_script(false);
    assert!(
        stderr.contains("A level change fired during capture; it was not followed."),
        "expected the default \"not followed\" line: {stderr}"
    );
    assert!(
        !stderr.contains("A level change was followed."),
        "the follow line must not fire without --follow-level-change: {stderr}"
    );
}
