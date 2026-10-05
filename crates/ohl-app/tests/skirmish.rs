//! `--skirmish` end to end through the binary: with no `--map` the run
//! finds the payload's deathmatch map by its spawn points (passing over a
//! single-player map published beside it), plays a headless scripted match
//! against bots, and logs only fixed lines and aggregate counts — never a
//! map name.
//!
//! The maps are `ohl-engine`'s project-authored fixtures, staged the way an
//! imported payload looks. No byte here comes from any game installation;
//! see `docs/CLEAN_ROOM.md`.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{deathmatch_room_bsp, synthetic_map_bsp};

/// The arena's name. Sorted after the single-player map's, so finding it
/// proves the run looked at spawn points rather than taking the first map.
const ARENA: &str = "ohlzarenasynth";
const SINGLE_PLAYER: &str = "ohlasingle";

fn stage_payload(root: &Path, with_arena: bool) {
    let maps = root
        .join("ohl-synthetic")
        .join("files")
        .join("valve")
        .join("maps");
    std::fs::create_dir_all(&maps).expect("create the payload tree");
    std::fs::write(
        maps.join(format!("{SINGLE_PLAYER}.bsp")),
        synthetic_map_bsp(),
    )
    .expect("stage the single-player map");
    if with_arena {
        let corners = [
            (-192.0, -192.0),
            (192.0, -192.0),
            (-192.0, 192.0),
            (192.0, 192.0),
        ];
        std::fs::write(
            maps.join(format!("{ARENA}.bsp")),
            deathmatch_room_bsp(&corners, false, ""),
        )
        .expect("stage the arena");
    }
}

fn run(with_arena: bool, extra: &[&str]) -> (bool, String) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage_payload(&root, with_arena);
    let script_path = directory.path().join("script.txt");
    std::fs::write(&script_path, "1200 wait\n").expect("write the scripted-input file");
    let output = Command::new(env!("CARGO_BIN_EXE_open-half-life"))
        .arg("--payload-root")
        .arg(&root)
        .arg("--skirmish")
        .args(extra)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log")
        .output()
        .expect("spawn open-half-life");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `stderr` with its terminal colour sequences (`ESC [ ... m`) removed, so a
/// structured field reads as `name=value`.
fn plain(stderr: &str) -> String {
    let mut out = String::with_capacity(stderr.len());
    let mut chars = stderr.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for skipped in chars.by_ref() {
                if skipped == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn a_headless_skirmish_finds_the_arena_and_reports_aggregates_only() {
    let (ok, stderr) = run(true, &["--bots", "3", "--force-respawn"]);
    assert!(ok, "the skirmish run failed: {stderr}");
    assert!(stderr.contains("Skirmish started."), "{stderr}");
    assert!(stderr.contains("Skirmish summary."), "{stderr}");
    assert!(plain(&stderr).contains("combatants=4"), "{stderr}");
    assert!(!stderr.contains(ARENA), "a map name was logged: {stderr}");
    assert!(!stderr.contains("Entity world empty"), "{stderr}");
}

#[test]
fn a_payload_without_a_deathmatch_map_fails_with_a_fixed_reason() {
    let (ok, stderr) = run(false, &[]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("the payload publishes no deathmatch map to start a skirmish on"),
        "{stderr}"
    );
}

#[test]
fn a_single_player_map_named_for_a_skirmish_still_plays_from_its_start() {
    // The documented fallback: with no `info_player_deathmatch`, players
    // spawn at the `info_player_start` instead.
    let (ok, stderr) = run(true, &["--map", SINGLE_PLAYER, "--bots", "1"]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("Skirmish summary."), "{stderr}");
}

#[test]
fn arenas_are_addressed_by_number_and_running_out_is_a_fixed_failure() {
    let (ok, stderr) = run(true, &["--arena", "1", "--bots", "1"]);
    assert!(ok, "{stderr}");
    assert!(stderr.contains("Skirmish summary."), "{stderr}");
    let (ok, stderr) = run(true, &["--arena", "2", "--bots", "1"]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("the payload publishes fewer deathmatch maps than --arena asks for"),
        "{stderr}"
    );
}
