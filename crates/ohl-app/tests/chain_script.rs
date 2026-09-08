//! `--chain-script`: a sequence of scripted-input routes run across level
//! changes in one process, each route starting where the previous route's
//! followed level change put the player down.
//!
//! The fixture is `ohl-engine`'s own project-authored, project-owned
//! touch-`trigger_changelevel` building block (a flat room whose far end
//! is a `trigger_changelevel` volume, joined to a second flat room), the
//! same one `tests/follow_level_change.rs` uses for the single-script
//! case. No byte here comes from any game installation; see
//! `docs/CLEAN_ROOM.md`.

use std::path::Path;
use std::process::Command;

use ohl_engine::test_support::{
    NEXT_MAP, TOUCH_CHANGELEVEL_MAP, door_behind_touch_trigger_bsp, synthetic_map_bsp,
    touch_changelevel_entities,
};

/// The first route: the same 300-tick forward walk from the fixture's
/// player start into its touch-trigger volume that
/// `tests/follow_level_change.rs` uses. The trailing wait is slack a
/// working route never reaches, since a chain route ends at its level
/// change.
const WALK_INTO_TOUCH_VOLUME: &str = "300 forward\n20 wait\n";

/// The second route: the destination map holds no trigger of its own, so
/// this route can only ever run out of ticks — which is exactly the
/// "chain stopped" case this test wants to observe at depth 2.
const WAIT_IN_THE_DESTINATION: &str = "60 wait\n";

/// A route that presses nothing at all, so the walk never reaches the
/// fixture's trigger volume and the chain stops on its very first map.
const STAND_STILL: &str = "60 wait\n";

/// Route 0 for the handover test: a forward-walk budget far longer than
/// the walk actually needs to reach the fixture's trigger volume, so a
/// chain that fails to hand over at the level change is separated from one
/// that does by a large, unambiguous number of ticks.
const LONG_WALK_INTO_TOUCH_VOLUME: &str = "900 forward\n";

/// Route 1 for the handover test: a fixed, easily counted stretch of
/// simulated time that can only be spent if route 1 ran at all.
const LONG_WAIT_IN_THE_DESTINATION: &str = "600 wait\n";

fn stage_payload(root: &Path) {
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

/// Like [`stage_payload`], but the destination map carries a touch
/// `trigger_changelevel` of its own pointing straight back at the source
/// map — the synthetic equivalent of a route that walks back into the
/// boundary it just arrived through.
fn stage_ping_pong_payload(root: &Path) {
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
    std::fs::write(
        maps.join(format!("{NEXT_MAP}.bsp")),
        door_behind_touch_trigger_bsp(&touch_changelevel_entities(TOUCH_CHANGELEVEL_MAP)),
    )
    .expect("stage a destination map that points back");
}

/// The simulated seconds a finished chain run reported, from the app's own
/// fixed aggregate line.
fn simulated_seconds(stderr: &str) -> f32 {
    stderr
        .lines()
        .find_map(|line| {
            let tail = line.split_once("Chain walk simulated seconds: ")?.1;
            tail.trim_end_matches('.').parse().ok()
        })
        .unwrap_or_else(|| panic!("no simulated-seconds line: {stderr}"))
}

/// Runs a chain of `routes` (in order) from the fixture's source map and
/// returns the run's stderr.
fn run_chain(routes: &[&str]) -> String {
    run_chain_over(routes, stage_payload)
}

/// [`run_chain`], with a caller-chosen payload stager.
fn run_chain_over(routes: &[&str], stage: fn(&Path)) -> String {
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path().join("payload");
    stage(&root);

    let mut command = Command::new(env!("CARGO_BIN_EXE_open-half-life"));
    command
        .arg("--payload-root")
        .arg(&root)
        .arg("--map")
        .arg(TOUCH_CHANGELEVEL_MAP);
    for (index, route) in routes.iter().enumerate() {
        let path = directory.path().join(format!("route{index}.txt"));
        std::fs::write(&path, route).expect("write a route file");
        command.arg("--chain-script").arg(&path);
    }
    command.arg("--script-log");

    let output = command.output().expect("spawn open-half-life");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the chain run failed: {stderr}");
    stderr
}

#[test]
fn a_two_route_chain_follows_one_level_change_and_reports_depth_two() {
    let stderr = run_chain(&[WALK_INTO_TOUCH_VOLUME, WAIT_IN_THE_DESTINATION]);
    assert!(
        stderr.contains("A level change was followed."),
        "the first route's level change should have been followed: {stderr}"
    );
    assert!(
        stderr.contains("Chain walk depth: 2."),
        "two maps were entered: {stderr}"
    );
    assert!(
        stderr.contains("The chain walk stopped."),
        "the second route runs out of ticks with no further level change: {stderr}"
    );
    assert!(
        !stderr.contains("The chain walk has no further route."),
        "only one terminal line may be logged: {stderr}"
    );
    assert!(
        !stderr.contains("The player is inside solid geometry."),
        "the transition must not land the player in solid geometry: {stderr}"
    );
}

#[test]
fn a_chain_whose_first_route_reaches_nothing_stops_on_the_first_map() {
    let stderr = run_chain(&[STAND_STILL, WAIT_IN_THE_DESTINATION]);
    assert!(
        !stderr.contains("A level change was followed."),
        "standing still reaches no trigger in this fixture: {stderr}"
    );
    assert!(
        stderr.contains("Chain walk depth: 1."),
        "only the start map was entered: {stderr}"
    );
    assert!(
        stderr.contains("The chain walk stopped."),
        "expected the fixed stop line: {stderr}"
    );
}

#[test]
fn a_chain_that_runs_out_of_routes_reports_no_further_route() {
    // A one-route chain whose single route does reach its level change:
    // the walk succeeded, and the chain ended only because nothing was
    // authored for the map it arrived in.
    let stderr = run_chain(&[WALK_INTO_TOUCH_VOLUME]);
    assert!(
        stderr.contains("Chain walk depth: 2."),
        "the followed level change counts as a second map: {stderr}"
    );
    assert!(
        stderr.contains("The chain walk has no further route."),
        "expected the fixed out-of-routes line: {stderr}"
    );
    assert!(
        !stderr.contains("The chain walk stopped."),
        "a chain that ran every route it was given did not stop short: {stderr}"
    );
}

#[test]
fn a_route_hands_over_to_the_next_one_at_the_level_change_it_reaches() {
    // The handover — route n stopping at its level change so route n+1
    // starts at the arrival point — is the whole point of a chain, and
    // nothing else in this file discriminates it: following the hop but
    // letting route 0 keep ticking on the destination map (the
    // `stop_on_level_change: false` mutant of `run_chained`) still
    // follows one level change and still ends at depth 2.
    //
    // The two-sided bound below does discriminate it, using the app's own
    // simulated-seconds aggregate. Route 0 budgets 900 forward ticks but
    // reaches the fixture's trigger volume in well under 300 of them;
    // route 1 then spends exactly 600 ticks waiting.
    //
    //   handing over:  (ticks to the trigger, < 300) + 600  <  1500 ticks
    //   not handing over:            900 + 600             =  1500 ticks
    //
    // So a total inside [600, 900) ticks — [10 s, 15 s) — is reachable
    // only if route 0 stopped early *and* route 1 ran afterwards. The
    // mutant lands at exactly 25 s and fails the upper bound; a chain
    // that dropped route 1 entirely would fall below the lower one.
    let stderr = run_chain(&[LONG_WALK_INTO_TOUCH_VOLUME, LONG_WAIT_IN_THE_DESTINATION]);
    let seconds = simulated_seconds(&stderr);
    assert!(
        seconds >= 10.0,
        "route 1's own 600 ticks did not run after the hop: {seconds} s"
    );
    assert!(
        seconds < 15.0,
        "route 0 kept ticking past its level change instead of handing over: {seconds} s"
    );
    assert!(
        stderr.contains("Chain walk depth: 2."),
        "the chain still entered two maps: {stderr}"
    );
}

#[test]
fn a_route_that_walks_back_through_its_arrival_boundary_fails_as_a_re_entry() {
    // Both fixture maps carry a touch `trigger_changelevel` pointing at
    // the other, so route 1's forward walk from the arrival point lands
    // back in the map the chain started in. Counting map *entries* would
    // call that depth 3 and let any `--min-depth` be satisfied by
    // ping-ponging across one boundary; the walk must instead stop and
    // say so.
    let stderr = run_chain_over(
        &[WALK_INTO_TOUCH_VOLUME, WALK_INTO_TOUCH_VOLUME],
        stage_ping_pong_payload,
    );
    assert!(
        stderr.contains("The chain walk re-entered a map it had already visited."),
        "expected the fixed re-entry line: {stderr}"
    );
    assert!(
        stderr.contains("Chain walk depth: 2."),
        "two distinct maps were entered, however many times: {stderr}"
    );
    assert!(
        !stderr.contains("The chain walk has no further route."),
        "a re-entry is a failure, not a clean end: {stderr}"
    );
    assert!(
        !stderr.contains("The chain walk stopped."),
        "only the re-entry terminal line may be logged: {stderr}"
    );
}

#[test]
fn the_walk_reports_its_simulated_seconds_as_a_bounded_aggregate() {
    let stderr = run_chain(&[WALK_INTO_TOUCH_VOLUME, WAIT_IN_THE_DESTINATION]);
    let line = stderr
        .lines()
        .find(|line| line.contains("Chain walk simulated seconds: "))
        .expect("the seconds line is logged");
    let value: f32 = line
        .rsplit("Chain walk simulated seconds: ")
        .next()
        .and_then(|tail| tail.trim_end_matches('.').parse().ok())
        .expect("the seconds line carries a number");
    // Both routes together schedule 380 ticks at 1/60 s; the first ends
    // early at its level change, so the total is a positive number below
    // that ceiling.
    assert!(
        value > 0.0 && value <= 380.0 / 60.0,
        "unexpected simulated-seconds total: {line}"
    );
}
