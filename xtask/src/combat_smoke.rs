//! `cargo xtask combat-smoke`: the headless scripted-input smoke.
//!
//! Runs the built `open-half-life` binary once per scripted scenario under
//! `xtask/smoke-scenarios/*.txt` (see `docs/m79-design.md` §7-8, package
//! P4a), against an already-imported payload tree, with `--script-log`.
//! Each run's stderr is checked against the exact fixed milestone lines
//! documented there, and classified pass/fail. No screenshot is taken and
//! no GPU is required: a scripted run with no `--headless-screenshot`
//! ticks the simulation headlessly (`crates/ohl-app/src/game_run.rs`).
//!
//! Classification and reporting shapes are shared with
//! `campaign_smoke.rs` (`Category`, `sanitize_error_code`); this command's
//! own summary reports scenario names (project-authored, from
//! `xtask/smoke-scenarios/`) and pass/fail buckets only. Logging policy
//! is the same as everywhere else in this project: no media-derived
//! string, count or size ever reaches the summary.
//!
//! Each [`Scenario`] names its own expected present/absent milestone-line
//! sets rather than one fixed pair for every scenario, so a later scenario
//! that does expect one of the six M7.9 P4b lines (weapon-fired/shot-hit/
//! monster-damage/monster-died/pickup/player-damage) can move it from its
//! own `absent` set to its own `present` one without touching the others;
//! see [`scenarios`]'s own doc comment for why none of the three today do.

use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;

/// `cargo xtask combat-smoke` command line.
#[derive(Debug, Parser)]
#[command(name = "combat-smoke")]
struct Args {
    /// An already-imported payload store root (the directory holding a
    /// published tree's `files/` directory, or one level above it).
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// Directory the results are written under (currently just a marker
    /// file; the summary itself is also printed to stdout).
    #[arg(long, value_name = "DIR", default_value = "target/combat-smoke")]
    out: PathBuf,

    /// A prebuilt `open-half-life` binary. Without one, `cargo build -p
    /// ohl-app --release` is run first and the release binary is used.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// Per-scenario timeout, in seconds.
    #[arg(long, default_value_t = 60)]
    timeout: u64,
}

/// One scenario: its project-authored name, the scripted-input file under
/// `xtask/smoke-scenarios/`, the map it runs over (a literal from
/// `ohl_campaign`'s own sourced table; see that crate's module
/// documentation for the citations), and which of the eight fixed
/// milestone lines (`docs/m79-design.md` §7) this scenario's script is
/// expected to make present versus absent.
struct Scenario {
    name: &'static str,
    file: &'static str,
    map: &'static str,
    present: &'static [&'static str],
    absent: &'static [&'static str],
    /// Passes `--follow-level-change` to the run
    /// (`crates/ohl-app/src/main.rs`), so a `trigger_changelevel` this
    /// scenario's script reaches actually loads its destination map and
    /// logs "A level change was followed." instead of staying on the
    /// original map. `false` for every scenario except the two that
    /// assert that line present.
    follow_level_change: bool,
    /// A `--start-inventory` list (`crates/ohl-app/src/main.rs`) given to
    /// the player right after the map loads, so a scenario can model the
    /// inventory a real campaign run would have carried in from an
    /// earlier map. Empty for every scenario that doesn't need one.
    start_inventory: &'static [&'static str],
}

/// The two lines every scenario's `--script-log` run always emits: the
/// script loaded and finished markers.
const BASE_PRESENT: [&str; 2] = ["Scripted input loaded.", "Scripted input finished."];

/// The eleven milestone lines a scenario that never fires, hits, damages
/// or picks up anything, that opens no door with a `use` press, that never
/// leaves the player embedded in solid geometry or riding a mover, that is
/// never moved by a `trigger_teleport`, and that never follows a level
/// change, is expected never to log. A
/// scenario that does expect one of these present removes it from its own
/// `absent` list instead.
///
/// "A level change was followed." joined this list (the spawn-to-exit
/// progression scenarios) alongside the scenarios that assert it
/// *present*, which are the ones run with `--follow-level-change`; every
/// other scenario's script either never reaches a `trigger_changelevel` or
/// is not run with that flag, so it must never log this line.
///
/// "The player is inside solid geometry." joined this list (M9) so that
/// every scenario in this file — not only the chapter-walk ones added
/// alongside it — asserts the PR #91 class of bug (the player falling
/// through a brush entity's floor and coming to rest embedded in solid
/// geometry) absent.
///
/// "The player opened a door." joined this list (M9, `TODO(black-box)`
/// item 25) alongside the scenarios that assert it *present*: only the
/// scenarios that run on "c1a0", "c1a3", "c2a2" and "c2a5" press `use` at
/// all, so every other one must never report a door opened by proximity.
/// Item 30 (touch-activated doors) briefly moved two more scenarios here
/// to a present set of their own, on the theory that their forward walk
/// touches a real closed door along the way; a PR #122 review round found
/// every door either walk actually opened carried a `targetname` — a case
/// item 30's own cited touch rule excludes from touch-opening ("...unless
/// they have a name, in which's case they require to be triggered
/// manually") — so both scenarios were reverted to this constant. See
/// [`WALK_PRESENT_DOOR_OPENED`]'s own doc comment for the two scenarios
/// that still use it (both `use` presses, unaffected by that correction).
///
/// "The player was teleported." joined this list alongside the one
/// scenario that asserts it *present* (see [`TELEPORTED_PRESENT`]): only
/// that map's own opening sequence moves the player with a
/// `trigger_teleport`, so every other scenario must never report one.
///
/// "The player is riding a mover." joined this list once mover-riders
/// (`crates/ohl-physics`'s `PlayerState::ground_brush`,
/// `crates/ohl-engine`'s `Level::brush_velocity`) landed: none of the
/// scenarios that use this constant stands on a moving
/// `func_train`/`func_tracktrain`/`func_plat`/lift `func_door`, so none
/// should log it. See `crates/ohl-physics/tests/mover_riders.rs` and
/// `crates/ohl-engine/tests/mover_riders.rs` for the mechanism exercised
/// against a real (synthetic) `func_train` instead.
///
/// The three scenarios that run on `ohl_campaign::STARTMAP` (`"c0a0"`) do
/// *not* use this constant: the player rides that map's opening tram, so
/// they assert "The player is riding a mover." *present* instead. See
/// [`START_MAP_PRESENT`]'s own doc comment.
const BASE_ABSENT: [&str; 11] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "A level change was followed.",
    "The player was teleported.",
];

/// The fixed lines every scenario that runs on `ohl_campaign::STARTMAP`
/// ("look around in the first chapter start", "walk from spawn in Black
/// Mesa Inbound" and "ride the opening tram to the level change", the last
/// of which adds one more line of its own; see
/// [`RIDE_TO_LEVEL_CHANGE_PRESENT`]) expects present, beyond
/// [`BASE_PRESENT`].
///
/// In the real game the player starts standing inside the map's opening
/// tram and rides it, and this project now reproduces that: a
/// `func_tracktrain` is placed on the first node of its own path at spawn
/// rather than left wherever its brushes were compiled (see
/// `crates/ohl-engine/src/render.rs`'s `track_train_transform` and
/// `crates/ohl-engine/tests/train_spawn_placement.rs`), so the tram's
/// collision brush is under the player's spawn, the map's own trigger
/// chain starts it, and the player is carried along without this
/// scenario's script pressing a single movement key. Both lines below
/// therefore fire from the ride alone.
///
/// This replaces the `TODO` that used to sit on
/// `START_MAP_ABSENT`, which carved "The player is riding a
/// mover." out of the absent set rather than codify a known-broken intro
/// as expected behaviour. The two gaps it recorded are closed: the map's
/// trains do start, and the player does stand on one at spawn. The walk
/// scenario on the same map is carried by the same tram, whatever its
/// script presses, so it uses these sets too.
const START_MAP_PRESENT: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player is riding a mover.",
];

/// [`BASE_ABSENT`], minus "The player is riding a mover.", which
/// [`START_MAP_PRESENT`] asserts present instead.
const START_MAP_ABSENT: [&str; 10] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player opened a door.",
    "A level change was followed.",
    "The player was teleported.",
];

/// [`START_MAP_PRESENT`] plus the line the one scenario that rides the
/// opening tram all the way to its end reaches: the ride carried the
/// player through a level boundary and the destination map loaded
/// (`crates/ohl-app/src/game_run.rs`'s `handle_level_change`, gated on
/// `--follow-level-change`, which [`Scenario::follow_level_change`] passes
/// for that scenario alone).
///
/// This is the campaign's own first progression gate, and the only
/// scenario in this file that asserts a level change at all. The ride
/// stopping short of it — a `path_track` carrying the documented default
/// "New Train Speed" of `0` being read as an order to stop rather than as
/// "no speed change" — left the passenger sealed in a tram that never
/// arrived, with no input able to recover; see
/// `crates/ohl-engine/tests/zero_speed_path_node.rs` for the same
/// mechanism against a synthetic fixture.
const RIDE_TO_LEVEL_CHANGE_PRESENT: [&str; 5] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player is riding a mover.",
    "A level change was followed.",
];

/// [`START_MAP_ABSENT`] minus "A level change was followed.", which
/// [`RIDE_TO_LEVEL_CHANGE_PRESENT`] asserts present instead.
const RIDE_TO_LEVEL_CHANGE_ABSENT: [&str; 9] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player opened a door.",
    "The player was teleported.",
];

/// The fixed line every M9 chapter-walk scenario expects present beyond
/// [`BASE_PRESENT`]: the player's eye position actually left its spawn
/// point (`crates/ohl-app/src/script_log.rs`). This is the scenario set's
/// own evidence that the scripted walk moved the player rather than just
/// idling — a static capture or a stuck-at-spawn walk would not show it.
const WALK_PRESENT: [&str; 3] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
];

/// [`WALK_PRESENT`] plus the line the campaign's last map's own opening
/// sequence reaches with no input at all: a `trigger_teleport` the player
/// already stands in moved them out of a sealed start volume
/// (`crates/ohl-app/src/script_log.rs`). Before teleport volumes were
/// implemented, that map printed no milestone line whatsoever over a
/// fifty-second unconditioned wait; the volume fired its `target` every
/// cooldown and nothing on the far end of that fire did anything.
const TELEPORTED_PRESENT: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player was teleported.",
    "The player moved from the spawn point.",
];

/// [`BASE_ABSENT`] minus the one line [`TELEPORTED_PRESENT`] moves to its
/// own present set. Used by the two scenarios that run on the campaign's
/// last map: the one that waits and presses nothing, and the chapter walk
/// that starts in the same volume and so is moved by it before its own
/// first step.
const TELEPORTED_ABSENT: [&str; 10] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "A level change was followed.",
];

/// [`WALK_PRESENT`] plus the two lines this scenario's own walk (in
/// "Power Up") happens to reach: a monster in the crowbar's swing path
/// took damage and died. See `xtask/smoke-scenarios/walk_power_up.txt`'s
/// own header for why.
const WALK_PRESENT_MONSTER_ENCOUNTER: [&str; 5] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "A monster took damage.",
    "A monster died.",
];

/// [`BASE_ABSENT`] minus the two lines [`WALK_PRESENT_MONSTER_ENCOUNTER`]
/// moves to its own present set.
const WALK_ABSENT_MONSTER_ENCOUNTER: [&str; 9] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "A level change was followed.",
    "The player was teleported.",
];

/// [`WALK_PRESENT`] plus the line this scenario's own walk (in
/// "Questionable Ethics") happens to reach: a source of player damage.
/// See `xtask/smoke-scenarios/walk_questionable_ethics.txt`'s own header
/// for why.
const WALK_PRESENT_PLAYER_DAMAGED: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player took damage.",
];

/// [`BASE_ABSENT`] minus the one line [`WALK_PRESENT_PLAYER_DAMAGED`]
/// moves to its own present set.
const WALK_ABSENT_PLAYER_DAMAGED: [&str; 10] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "A level change was followed.",
    "The player was teleported.",
];

/// [`WALK_PRESENT`] plus the line the walk out of `ohl_campaign::TRAINMAP`'s
/// start reaches: a `trigger_*` volume fired because the player's own hull
/// walked into it (`crates/ohl-app/src/script_log.rs`).
///
/// That line is what makes this scenario pin the world-baked brush-mover
/// placement rather than merely re-walk a map. The player starts standing
/// inside a brush mover authored in absolute world space; with that
/// placement wrong the mover is displaced by the full magnitude of its own
/// path coordinates at load, the player falls out of it into a sealed space
/// no input can leave, and — measured, with
/// `ohl_game::pose::track_train_transform`'s world-baked branch forced
/// `false` and nothing else changed — the player still slides far enough to
/// log "The player moved from the spawn point." but never reaches a single
/// touch volume. The two lines together are the discriminating pair.
///
/// "The player walked into a trigger volume." is deliberately *not* in
/// [`BASE_ABSENT`]: most scenarios in this file walk through a map's own
/// touch volumes as a matter of course, so it is asserted where it carries
/// meaning and left unconstrained elsewhere — the same treatment "The
/// player moved from the spawn point." already gets.
const WALK_PRESENT_TOUCH_TRIGGER: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player walked into a trigger volume.",
];

/// The fixed lines a scenario that does pick up and fire a weapon expects
/// present, beyond [`BASE_PRESENT`].
///
/// "A shot hit an entity." joined this list once the player collided with
/// solid brush entities as well as with worldspawn: before that the walk in
/// this scenario dropped through the map's floors and ended somewhere with
/// nothing in the swing's reach, so the swing connected with nothing. With
/// the walk now following the floor the scenario was tuned against, the
/// swing lands, which is the behaviour this smoke is meant to observe.
const FIRE_AND_PICKUP_PRESENT: [&str; 5] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "A pickup was collected.",
    "The player fired a weapon.",
    "A shot hit an entity.",
];

/// [`WALK_PRESENT`] plus the line this scenario's own walk (in
/// `xtask/smoke-scenarios/ladder_t0a0a.txt`) reaches: attaching to a
/// `func_ladder`. See that file's own header for the technique.
const WALK_PRESENT_LADDER: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player is on a ladder.",
];

/// [`WALK_PRESENT`] plus "The player opened a door.", used by
/// `xtask/smoke-scenarios/use_rotating_door_anomalous_materials.txt`: it
/// walks up to a real `func_door_rotating` and opens it with a `use` press
/// through the engine's own proximity path. That path only finds an
/// "origin brush" entity at all once its proximity point is computed from
/// the same placed pose the renderer and the collision model use
/// (`ohl_game::pose::brush_center`); see `docs/FORMAT_SOURCES.md`'s
/// `TODO(black-box)` item 25 and that scenario file's own header.
///
/// `walk_anomalous_materials.txt` and `walk_surface_tension.txt` briefly
/// used this set too (M9.9, touch-activated doors): each one's own
/// ~20-40s forward walk happens to carry the player's hull into a real
/// closed door along the way, which `ohl_game::logic::Simulation::
/// touch_doors` opened on contact at the time. A PR #122 review round
/// found every door either walk actually opened carried a `targetname` —
/// a real map's door gated behind a trigger chain, which
/// `docs/FORMAT_SOURCES.md` item 30's own cited touch rule excludes from
/// touch-opening once read in full ("...unless they have a name, in
/// which's case they require to be triggered manually") — so both
/// scenarios moved back to [`WALK_PRESENT`]/[`BASE_ABSENT`], their
/// original (pre-M9.9) sets, confirmed by re-running both directly with
/// `--script-log` against the real payload with neither logging "The
/// player opened a door." any more.
const WALK_PRESENT_DOOR_OPENED: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player opened a door.",
];

/// [`BASE_ABSENT`] minus the one line [`WALK_PRESENT_DOOR_OPENED`] moves to
/// its own present set.
const WALK_ABSENT_DOOR_OPENED: [&str; 10] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "A level change was followed.",
    "The player was teleported.",
];

/// [`WALK_PRESENT`] plus the line a door-free spawn-to-exit walk (in
/// `xtask/smoke-scenarios/progress_c1a1_reach_changelevel.txt` and, from a
/// later reachability-triage pass,
/// `xtask/smoke-scenarios/progress_c3a1_reach_changelevel.txt`) reaches: a
/// `trigger_changelevel` followed end to end with `--follow-level-change`
/// (`crates/ohl-app/src/game_run.rs`'s `handle_level_change`). These are the
/// scenarios in this file whose script actually rides a chapter's
/// spawn-to-exit route through to the next map, rather than only walking
/// partway; see each scenario file's own header for its route.
const LEVEL_CHANGE_PRESENT: [&str; 4] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "A level change was followed.",
];

/// [`BASE_ABSENT`] minus the one line [`LEVEL_CHANGE_PRESENT`] moves to its
/// own present set.
const LEVEL_CHANGE_ABSENT: [&str; 10] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "The player was teleported.",
];

/// [`LEVEL_CHANGE_PRESENT`] plus "The player opened a door.": the scenarios
/// that walk a chapter's first map from its player start, through a door
/// they open with a `use` press, to that map's own `trigger_changelevel`
/// (`xtask/smoke-scenarios/reach_level_change_anomalous_materials.txt` and,
/// from a later reachability-triage pass,
/// `xtask/smoke-scenarios/progress_c2a5_reach_changelevel.txt`). See the
/// first file's own header for the route-authoring technique, and for why a
/// walk that only advances straight ahead on that map stops at a wall
/// instead.
const DOOR_AND_LEVEL_CHANGE_PRESENT: [&str; 5] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player opened a door.",
    "A level change was followed.",
];

/// [`BASE_ABSENT`] minus the two lines [`DOOR_AND_LEVEL_CHANGE_PRESENT`]
/// moves to its own present set.
const DOOR_AND_LEVEL_CHANGE_ABSENT: [&str; 9] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player was teleported.",
];

/// [`LEVEL_CHANGE_PRESENT`] plus "A monster took damage.": the scenario
/// that walks a chapter's first map from its player start to its own
/// `trigger_changelevel` and passes near a monster along the way. See
/// `xtask/smoke-scenarios/progress_c2a1_reach_changelevel.txt`'s own
/// header for the route.
///
/// Re-authored for M9.11 (`docs/FORMAT_SOURCES.md` item 33,
/// `func_monsterclip` no longer solid to the player): this map carries 29
/// `func_monsterclip` entities, and the route this scenario originally
/// replayed was flown against the pre-M9.11 collision model, which
/// (incorrectly) also treated `func_monsterclip` as solid to the player,
/// so it stopped reaching its `trigger_changelevel` once that was fixed.
/// Rather than weaken this scenario's own assertions, the route itself was
/// re-authored fresh against the corrected engine
/// (`.plan/progress-probe-2.md`'s two-`Game` planner/autopilot technique;
/// see the scenario file's own header for the full account) — this
/// constant, and the present/absent split below it, are therefore
/// unchanged from before that milestone.
const LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER: [&str; 5] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "A monster took damage.",
    "A level change was followed.",
];

/// [`BASE_ABSENT`] minus the two lines
/// [`LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER`] moves to its own present
/// set.
const LEVEL_CHANGE_ABSENT_MONSTER_ENCOUNTER: [&str; 9] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "The player was teleported.",
];

/// [`DOOR_AND_LEVEL_CHANGE_PRESENT`] plus "A monster took damage.": the
/// scenario that walks a chapter's first map from its player start,
/// through a door it opens with a `use` press, past a monster along the
/// way, to that map's own `trigger_changelevel`. See
/// `xtask/smoke-scenarios/progress_c2a2_reach_changelevel.txt`'s own
/// header for the route.
const DOOR_AND_LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER: [&str; 6] = [
    "Scripted input loaded.",
    "Scripted input finished.",
    "The player moved from the spawn point.",
    "The player opened a door.",
    "A monster took damage.",
    "A level change was followed.",
];

/// [`BASE_ABSENT`] minus the three lines
/// [`DOOR_AND_LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER`] moves to its own
/// present set.
const DOOR_AND_LEVEL_CHANGE_ABSENT_MONSTER_ENCOUNTER: [&str; 8] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player was teleported.",
];

/// [`BASE_ABSENT`] minus the three lines [`FIRE_AND_PICKUP_PRESENT`] moves
/// to its own present set: the swing lands, but nothing in this scenario
/// takes enough damage to report a monster hurt or killed, and nothing in
/// it can damage the player either. Still includes "The player is inside
/// solid geometry." and "The player is riding a mover.": this scenario's
/// own regression guard for the PR #91 class of bug and for mover-riders,
/// same as every other scenario in this file.
const FIRE_AND_PICKUP_ABSENT: [&str; 8] = [
    "A monster took damage.",
    "A monster died.",
    "The player took damage.",
    "The player is inside solid geometry.",
    "The player is riding a mover.",
    "The player opened a door.",
    "A level change was followed.",
    "The player was teleported.",
];

/// The scenarios this command runs, in order. Map names come only from
/// `ohl_campaign`'s cited table: `ohl_campaign::TRAINMAP` for the training
/// start, `ohl_campaign::STARTMAP` for the first chapter's start,
/// `"c1a1"` (Unforeseen Consequences, `ohl_campaign::CHAPTERS`'s second
/// chapter's first map) for the first monster encounter, and `"t0a0b1"`
/// (one of `ohl_campaign::HAZARD_COURSE_MAPS`'s own cited map names) for
/// picking up and firing a weapon.
///
/// M7.9 P4b wired "The player fired a weapon."/"A shot hit an entity." end
/// to end (`crates/ohl-app/src/script_log.rs`), and the fourth scenario
/// below (`xtask/smoke-scenarios/pick_up_and_fire_a_weapon.txt`) is the
/// scripted walk to an actual weapon pickup on that map that reaches the
/// first of those two lines through this harness's own real-payload path
/// (a crowbar swing routes through the same melee branch a hitscan shot
/// does; see that file's own header for why "A shot hit an entity." still
/// is not reached). `crates/ohl-engine/tests/save_sections.rs` continues to
/// exercise the same counters end to end against this package's own
/// synthetic fixture.
///
/// The remaining nineteen scenarios (M9) are a moving-player walk for each
/// of the 18 story chapters in `ohl_campaign::CHAPTERS` whose first map is
/// confirmed (every chapter except Interloper, whose starting map prefix
/// is deliberately left unverified; see that constant's own module
/// documentation), plus one for the Hazard Course
/// (`ohl_campaign::TRAINMAP`). Each walks forward with short steps and
/// periodic turns for roughly 20-40 simulated seconds — the technique that
/// exposed the PR #90 (touch triggers never firing from movement) and PR
/// #91 (the player falling through a brush entity's floor) regressions
/// that static captures and load smokes had missed for days. Three of the
/// nineteen (`xtask/smoke-scenarios/walk_power_up.txt`,
/// `walk_forget_about_freeman.txt`, `walk_xen.txt`) turn before or instead
/// of advancing straight ahead: each file's own header explains why (a
/// dead-end player start, a solid corner in the walk's path, or a nearby
/// platform edge), tuned only against that one map's own player-start-
/// relative geometry and never recorded here beyond a turn/walk
/// technique.
///
/// The last scenario (PR #103, the ladder hull probe) reaches and climbs
/// an actual `func_ladder` on the Hazard Course's "t0a0a" map (see
/// `xtask/smoke-scenarios/ladder_t0a0a.txt`'s own header), the real-payload
/// counterpart to that PR's synthetic hull-overlap fixtures.
///
/// One further scenario (the `TODO(black-box)` item 25 fix) walks up to a
/// real `func_door_rotating` on "c1a0" and opens it with a `use` press,
/// through the engine's own `ohl_game::find_usable_within` proximity path
/// — the real-payload counterpart of
/// `crates/ohl-engine/tests/rotating_door.rs`'s synthetic fixture. It and
/// four of the progression scenarios below (on "c1a0", "c1a3", "c2a2" and
/// "c2a5") are the only scenarios in this file that press `use` at all,
/// and the only ones that assert "The player opened a door." present:
/// every real door any other scenario's own forward walk (no `use` press)
/// has been found to touch along its route carries a `targetname`, which
/// `docs/FORMAT_SOURCES.md` item 30's own cited touch rule excludes from
/// touch-opening (see [`WALK_PRESENT_DOOR_OPENED`]'s own doc comment for
/// the two scenarios that briefly asserted otherwise, and why that was
/// reverted). Every other scenario in this file asserts the line absent.
///
/// One further scenario rides `ohl_campaign::STARTMAP`'s opening tram to
/// its end and follows the level change it reaches (see
/// [`RIDE_TO_LEVEL_CHANGE_PRESENT`]): the campaign's own first
/// progression gate, reached without a single movement key.
///
/// Two further scenarios (M9, the auto-start gaps) cover the two maps an
/// earlier read-only investigation found doing nothing at all. The first
/// waits thirty simulated seconds on the campaign's last map and presses
/// nothing: its opening `trigger_teleport` moves the player out of a
/// sealed start volume on its own, once the `master` gating that volume
/// goes active at the pace the map's own fan-out delays set (see
/// [`TELEPORTED_PRESENT`]). The second walks four simulated seconds
/// forward on `ohl_campaign::TRAINMAP` and leaves its start area, which
/// only exists once a brush mover authored in absolute world space stays
/// where it was compiled instead of being displaced by the full magnitude
/// of its own path coordinates at load. It asserts "The player walked into
/// a trigger volume." present, which is the half of its claim that
/// actually fails without that placement (see
/// [`WALK_PRESENT_TOUCH_TRIGGER`]); see
/// `crates/ohl-engine/tests/train_spawn_placement.rs` for the same
/// mechanism against a synthetic fixture.
///
/// All 37 scenarios in this file — the four pre-existing ones included —
/// assert "The player is inside solid geometry." absent: this scenario
/// set's own regression guard for the PR #91 class of bug. 34 of the 37
/// also assert "The player is riding a mover." absent, since none of them
/// stands on a moving brush entity; the three that run on
/// `ohl_campaign::STARTMAP` assert it *present* instead, because the
/// player spawns inside that map's opening tram and rides it (see
/// [`START_MAP_PRESENT`]'s own doc comment).
///
/// Ten scenarios — one per progression route — assert "A level change
/// was followed." present, and they are the only ten whose
/// [`Scenario::follow_level_change`] is `true`; every other scenario
/// asserts that line absent instead, since none of their scripts reaches a
/// `trigger_changelevel` they are run with the flag for.
///
/// The first walks a turn-then-forward route from spawn on "c1a1"
/// (Unforeseen Consequences) to a `trigger_changelevel` reached within a
/// few simulated seconds, then follows it end to end through
/// `crates/ohl-app/src/game_run.rs`'s `handle_level_change`. See
/// `xtask/smoke-scenarios/progress_c1a1_reach_changelevel.txt`'s own
/// header for the route in words.
///
/// The second does the same for "c1a0" (Anomalous Materials, the second
/// chapter in that same cited table), whose route
/// is longer and needs a door opened with `use` partway along it — so it is
/// the only scenario here that asserts both that line and "The player
/// opened a door." present. Its map was previously believed to be blocked
/// short of any `trigger_changelevel` by a forward-movement stop; tracing
/// that stop showed worldspawn geometry with no brush entity within four
/// times `ohl_engine::USE_RADIUS` — a wall the earlier scripted walk simply
/// walked into — and the route below reaches the exit with the engine as it
/// stands. See that scenario file's own header.
///
/// The third presses nothing at all: it rides `ohl_campaign::STARTMAP`'s
/// opening tram from spawn to the level boundary the ride itself crosses,
/// which is the first map's whole progression. That route only exists
/// once a `path_track` carrying the documented default "New Train Speed"
/// of `0` is read as "no speed change" rather than as an order to stop —
/// read literally, the ride parked partway and left the passenger with
/// nowhere to walk. See
/// `crates/ohl-engine/tests/zero_speed_path_node.rs` for the same
/// mechanism against a synthetic fixture.
///
/// Five more (a second investigation pass, PR authored after PR #121's
/// `--reachability-report` dev tool landed) cover the next five chapters'
/// first maps whose route a breadth-first reachability walk showed
/// reachable: "c1a3" ("We've Got Hostiles!") and "c2a2" (On A Rail), each
/// needing a door opened with `use` along the way, the latter also passing
/// a monster; "c2a1" (Power Up), which also passes a monster but needs no
/// door; and "c1a4" (Blast Pit) and "c2a3" (Apprehension), whose routes
/// need neither. See each scenario file's own header for its route in
/// words. The same investigation found "c1a2" (Office Complex) not
/// reachable by that walk — its frontier is left with only non-door
/// entities (a swinging obstacle, a pushable, further buttons) after every
/// use-openable door is gone — so it has no scenario here; see
/// `docs/MILESTONES.md`.
///
/// A third reachability-triage pass (`.plan/progress-probe-3.md`, using PR
/// #121's `--reachability-report` dev tool) found two more chapters' first
/// maps reachable from spawn to their own `trigger_changelevel`: Surface
/// Tension (c2a5), whose route opens a door along the way (so it asserts
/// "The player opened a door." present, the same as "c1a0"'s route above),
/// and "Forget About Freeman!" (c3a1), whose route needs no door. The same
/// pass found three further maps **not** reachable by this walk — Residue
/// Processing (c2a4) and Xen (c4a1), sealed by static geometry or a
/// fall/jump longer than the reachability walk's own conservative drop
/// bound, and Lambda Core (c3a2), blocked by `func_breakable` having no
/// shoot-to-destroy behaviour anywhere in this engine yet — and one,
/// Questionable Ethics (c2a4d), confirmed reachable by
/// `--reachability-report` but not turned into a working route within that
/// probe's own tool budget; documented in `docs/MILESTONES.md`, none of
/// them gets a scenario here.
#[allow(
    clippy::too_many_lines,
    reason = "one Scenario literal per M9 chapter-walk scenario, plus the four \
              pre-existing ones, plus the ten progression scenarios; splitting \
              the list would only add indirection"
)]
fn scenarios() -> [Scenario; 37] {
    [
        Scenario {
            name: "the endgame's opening teleport fires on its own",
            file: "endgame_opening_teleport.txt",
            map: "c5a1",
            present: &TELEPORTED_PRESENT,
            absent: &TELEPORTED_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "leave the hazard course start",
            file: "leave_hazard_course_start.txt",
            map: ohl_campaign::TRAINMAP,
            present: &WALK_PRESENT_TOUCH_TRIGGER,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk forward in the training start",
            file: "training_start.txt",
            map: ohl_campaign::TRAINMAP,
            present: &BASE_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "look around in the first chapter start",
            file: "first_chapter_start.txt",
            map: ohl_campaign::STARTMAP,
            present: &START_MAP_PRESENT,
            absent: &START_MAP_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "approach the first monster encounter",
            file: "approach_first_monster.txt",
            map: "c1a1",
            present: &BASE_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "pick up and fire a weapon in the hazard course",
            file: "pick_up_and_fire_a_weapon.txt",
            map: "t0a0b1",
            present: &FIRE_AND_PICKUP_PRESENT,
            absent: &FIRE_AND_PICKUP_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Black Mesa Inbound",
            file: "walk_black_mesa_inbound.txt",
            map: "c0a0",
            present: &START_MAP_PRESENT,
            absent: &START_MAP_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "ride the opening tram to the level change",
            file: "ride_tram_to_level_change.txt",
            map: ohl_campaign::STARTMAP,
            present: &RIDE_TO_LEVEL_CHANGE_PRESENT,
            absent: &RIDE_TO_LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "open a rotating door with use in Anomalous Materials",
            file: "use_rotating_door_anomalous_materials.txt",
            map: "c1a0",
            present: &WALK_PRESENT_DOOR_OPENED,
            absent: &WALK_ABSENT_DOOR_OPENED,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "reach a level change in Anomalous Materials",
            file: "reach_level_change_anomalous_materials.txt",
            map: "c1a0",
            present: &DOOR_AND_LEVEL_CHANGE_PRESENT,
            absent: &DOOR_AND_LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Anomalous Materials",
            file: "walk_anomalous_materials.txt",
            map: "c1a0",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Unforeseen Consequences",
            file: "walk_unforeseen_consequences.txt",
            map: "c1a1",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Office Complex",
            file: "walk_office_complex.txt",
            map: "c1a2",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in \"We've Got Hostiles!\"",
            file: "walk_hostiles.txt",
            map: "c1a3",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Blast Pit",
            file: "walk_blast_pit.txt",
            map: "c1a4",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Power Up",
            file: "walk_power_up.txt",
            map: "c2a1",
            present: &WALK_PRESENT_MONSTER_ENCOUNTER,
            absent: &WALK_ABSENT_MONSTER_ENCOUNTER,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in On A Rail",
            file: "walk_on_a_rail.txt",
            map: "c2a2",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Apprehension",
            file: "walk_apprehension.txt",
            map: "c2a3",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Residue Processing",
            file: "walk_residue_processing.txt",
            map: "c2a4",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Questionable Ethics",
            file: "walk_questionable_ethics.txt",
            map: "c2a4d",
            present: &WALK_PRESENT_PLAYER_DAMAGED,
            absent: &WALK_ABSENT_PLAYER_DAMAGED,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Surface Tension",
            file: "walk_surface_tension.txt",
            map: "c2a5",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in \"Forget About Freeman!\"",
            file: "walk_forget_about_freeman.txt",
            map: "c3a1",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Lambda Core",
            file: "walk_lambda_core.txt",
            map: "c3a2",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Xen",
            file: "walk_xen.txt",
            map: "c4a1",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Gonarch's Lair",
            file: "walk_gonarchs_lair.txt",
            map: "c4a2",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Nihilanth",
            file: "walk_nihilanth.txt",
            map: "c4a3",
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in Endgame",
            file: "walk_endgame.txt",
            map: "c5a1",
            present: &TELEPORTED_PRESENT,
            absent: &TELEPORTED_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn in the Hazard Course",
            file: "walk_hazard_course.txt",
            map: ohl_campaign::TRAINMAP,
            present: &WALK_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "reach and climb a ladder in the Hazard Course",
            file: "ladder_t0a0a.txt",
            map: "t0a0a",
            present: &WALK_PRESENT_LADDER,
            absent: &BASE_ABSENT,
            follow_level_change: false,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in Unforeseen Consequences",
            file: "progress_c1a1_reach_changelevel.txt",
            map: "c1a1",
            present: &LEVEL_CHANGE_PRESENT,
            absent: &LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in \"We've Got Hostiles!\"",
            file: "progress_c1a3_reach_changelevel.txt",
            map: "c1a3",
            present: &DOOR_AND_LEVEL_CHANGE_PRESENT,
            absent: &DOOR_AND_LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in Surface Tension",
            file: "progress_c2a5_reach_changelevel.txt",
            map: "c2a5",
            present: &DOOR_AND_LEVEL_CHANGE_PRESENT,
            absent: &DOOR_AND_LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in Blast Pit",
            file: "progress_c1a4_reach_changelevel.txt",
            map: "c1a4",
            present: &LEVEL_CHANGE_PRESENT,
            absent: &LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in Power Up",
            file: "progress_c2a1_reach_changelevel.txt",
            map: "c2a1",
            present: &LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER,
            absent: &LEVEL_CHANGE_ABSENT_MONSTER_ENCOUNTER,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in On A Rail",
            file: "progress_c2a2_reach_changelevel.txt",
            map: "c2a2",
            present: &DOOR_AND_LEVEL_CHANGE_PRESENT_MONSTER_ENCOUNTER,
            absent: &DOOR_AND_LEVEL_CHANGE_ABSENT_MONSTER_ENCOUNTER,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in Apprehension",
            file: "progress_c2a3_reach_changelevel.txt",
            map: "c2a3",
            present: &LEVEL_CHANGE_PRESENT,
            absent: &LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
        Scenario {
            name: "walk from spawn to a followed level change in \"Forget About Freeman!\"",
            file: "progress_c3a1_reach_changelevel.txt",
            map: "c3a1",
            present: &LEVEL_CHANGE_PRESENT,
            absent: &LEVEL_CHANGE_ABSENT,
            follow_level_change: true,
            start_inventory: &[],
        },
    ]
}

/// One scenario's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Category {
    /// The run exited successfully and every expected line was present or
    /// absent as documented.
    Pass,
    /// The run exited successfully, but a milestone-line expectation
    /// failed.
    UnexpectedLines,
    /// The app exited with a failure and a sanitized reason code.
    LoadError(&'static str),
    /// The run did not finish within the per-scenario timeout.
    Timeout,
    /// The process ended abnormally (killed by a signal, or could not be
    /// spawned at all).
    Crash,
}

impl Category {
    fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }

    fn bucket(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::UnexpectedLines => "unexpected-lines",
            Self::LoadError(_) => "load-error",
            Self::Timeout => "timeout",
            Self::Crash => "crash",
        }
    }
}

/// Known fixed failure messages the app logs (see
/// `crates/ohl-app/src/game_run.rs` and `crates/ohl-app/src/main.rs`),
/// mapped to a short, stable, sanitized code. Anything not on this list
/// still gets a code (`"unspecified"`), never the raw message.
const KNOWN_ERROR_CODES: &[(&str, &str)] = &[
    (
        "the start map could not be loaded from the payload",
        "missing-map",
    ),
    (
        "the payload directory could not be indexed",
        "payload-index",
    ),
    ("the script file could not be read", "script-unreadable"),
    ("the script file could not be parsed", "script-invalid"),
    ("no usable graphics adapter is available", "no-gpu"),
    (
        "No imported payload was found. Import one first by passing --iso PATH.",
        "no-payload",
    ),
    (
        "Payload location failed: no per-user data directory is available",
        "no-data-dir",
    ),
];

/// Maps a fixed error line to a short, sanitized code. Never returns any
/// substring of `line` itself: only the fixed codes in
/// [`KNOWN_ERROR_CODES`], or the catch-all `"unspecified"`.
fn sanitize_error_code(line: &str) -> &'static str {
    for (message, code) in KNOWN_ERROR_CODES {
        if line.contains(message) {
            return code;
        }
    }
    "unspecified"
}

/// The outcome of running the app once for one scenario: the exit status
/// shape, the last `[error]`-prefixed fixed log line if any, and whether
/// the milestone-line expectations held. Never keeps the raw stderr text
/// past classification.
struct RunOutcome {
    timed_out: bool,
    exit_code: Option<i32>,
    last_error_line: Option<String>,
    lines_as_expected: bool,
}

fn classify(outcome: &RunOutcome) -> Category {
    if outcome.timed_out {
        return Category::Timeout;
    }
    match outcome.exit_code {
        Some(0) => {
            if outcome.lines_as_expected {
                Category::Pass
            } else {
                Category::UnexpectedLines
            }
        }
        Some(_failure) => {
            let line = outcome.last_error_line.as_deref().unwrap_or("");
            Category::LoadError(sanitize_error_code(line))
        }
        None => Category::Crash,
    }
}

/// One scenario's full report: only its name and classification.
struct ScenarioReport {
    name: &'static str,
    category: Category,
}

/// Builds the `open-half-life` invocation for `scenario`: the fixed
/// `--payload-root`/`--map`/`--script`/`--script-log` quartet, plus
/// `--follow-level-change` and `--start-inventory` only when the scenario
/// actually asks for them, so a scenario that needs neither runs the
/// binary exactly as it always has.
fn build_command(
    bin: &Path,
    payload_root: &Path,
    script_path: &Path,
    scenario: &Scenario,
) -> Command {
    let mut command = Command::new(bin);
    command
        .arg("--payload-root")
        .arg(payload_root)
        .arg("--map")
        .arg(scenario.map)
        .arg("--script")
        .arg(script_path)
        .arg("--script-log");
    if scenario.follow_level_change {
        command.arg("--follow-level-change");
    }
    if !scenario.start_inventory.is_empty() {
        command
            .arg("--start-inventory")
            .arg(scenario.start_inventory.join(","));
    }
    command
}

/// Runs the app once for `scenario`, with a `timeout` deadline.
fn run_one(
    bin: &Path,
    payload_root: &Path,
    scenarios_dir: &Path,
    scenario: &Scenario,
    timeout: Duration,
) -> ScenarioReport {
    let script_path = scenarios_dir.join(scenario.file);

    let mut command = build_command(bin, payload_root, &script_path, scenario);
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return ScenarioReport {
            name: scenario.name,
            category: Category::Crash,
        };
    };

    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr_reader = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let stderr_text = stderr_reader.join().unwrap_or_default();

    let last_error_line = stderr_text
        .lines()
        .rfind(|line| line.starts_with("[error]"))
        .map(ToString::to_string);

    let lines_as_expected = scenario
        .present
        .iter()
        .all(|line| stderr_text.contains(line))
        && scenario
            .absent
            .iter()
            .all(|line| !stderr_text.contains(line));

    let outcome = RunOutcome {
        timed_out: status.is_none() && Instant::now() >= deadline,
        exit_code: status.and_then(|status| status.code()),
        last_error_line,
        lines_as_expected,
    };

    ScenarioReport {
        name: scenario.name,
        category: classify(&outcome),
    }
}

/// The category buckets shown as summary columns, in a fixed order.
/// [`Category::LoadError`] reason codes all collapse into the single
/// `"load-error"` bucket: the summary reports aggregate counts only, never
/// a per-code breakdown.
const CATEGORY_BUCKETS: [(&str, &str); 4] = [
    ("pass", "Pass"),
    ("unexpected-lines", "Unexpected-lines"),
    ("load-error", "Load-error"),
    ("timeout", "Timeout"),
];

/// Writes the summary: one row per scenario (its project-authored name and
/// pass/fail bucket) plus an aggregate total row. Never writes a payload
/// path or any media-derived number.
fn write_summary(reports: &[ScenarioReport], total_elapsed: Duration) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Combat smoke summary\n\n");
    let _ = writeln!(out, "Total elapsed: {:.1}s\n", total_elapsed.as_secs_f64());

    out.push_str("| Scenario | Result |\n|---|---|\n");
    let mut totals = [0usize; CATEGORY_BUCKETS.len() + 1];
    for report in reports {
        let bucket = report.category.bucket();
        let label = CATEGORY_BUCKETS
            .iter()
            .find(|(key, _)| *key == bucket)
            .map_or("Crash", |(_, label)| *label);
        let _ = writeln!(out, "| {} | {label} |", report.name);
        if let Some(index) = CATEGORY_BUCKETS.iter().position(|(key, _)| *key == bucket) {
            totals[index] += 1;
        } else {
            *totals.last_mut().expect("at least one slot") += 1;
        }
    }

    out.push_str("\n| Result | Count |\n|---|---|\n");
    for (index, (_, label)) in CATEGORY_BUCKETS.iter().enumerate() {
        let _ = writeln!(out, "| {label} | {} |", totals[index]);
    }
    let _ = writeln!(out, "| Crash | {} |", totals[CATEGORY_BUCKETS.len()]);

    out
}

/// A build failure, reported without expanding into a raw `cargo` error
/// dump.
#[derive(Debug)]
struct BuildFailed;

impl fmt::Display for BuildFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cargo build -p ohl-app --release failed")
    }
}

const APP_BIN_NAME: &str = "open-half-life";

/// Builds the release `open-half-life` binary and returns its path.
fn build_release_binary(root: &Path) -> Result<PathBuf, BuildFailed> {
    let status = Command::new("cargo")
        .args(["build", "-p", "ohl-app", "--release"])
        .current_dir(root)
        .status()
        .map_err(|_| BuildFailed)?;
    if !status.success() {
        return Err(BuildFailed);
    }
    let name = if cfg!(windows) {
        format!("{APP_BIN_NAME}.exe")
    } else {
        APP_BIN_NAME.to_string()
    };
    Ok(root.join("target").join("release").join(name))
}

/// Entry point for `cargo xtask combat-smoke`, given the arguments after
/// the subcommand name.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("combat-smoke".to_string()).chain(raw_args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };

    let bin = match args.bin.clone() {
        Some(bin) => bin,
        None => match build_release_binary(root) {
            Ok(bin) => bin,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        },
    };

    let out_dir = if args.out.is_absolute() {
        args.out.clone()
    } else {
        root.join(&args.out)
    };
    if let Err(error) = std::fs::create_dir_all(&out_dir) {
        eprintln!("error: could not create the output directory: {error}");
        return ExitCode::FAILURE;
    }

    let scenarios_dir = root.join("xtask").join("smoke-scenarios");
    let timeout = Duration::from_secs(args.timeout);
    let scenario_list = scenarios();
    println!(
        "Running the combat smoke over {} scenario(s)...",
        scenario_list.len()
    );

    let started = Instant::now();
    let reports: Vec<ScenarioReport> = scenario_list
        .iter()
        .map(|scenario| run_one(&bin, &args.payload_root, &scenarios_dir, scenario, timeout))
        .collect();
    let total_elapsed = started.elapsed();

    let summary = write_summary(&reports, total_elapsed);
    let summary_path = out_dir.join("SUMMARY.md");
    if let Err(error) = std::fs::write(&summary_path, &summary) {
        eprintln!("error: could not write the summary: {error}");
        return ExitCode::FAILURE;
    }

    let any_failed = reports.iter().any(|report| !report.category.is_pass());
    println!("{summary}");
    if any_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(
        timed_out: bool,
        exit_code: Option<i32>,
        last_error_line: Option<&str>,
        lines_as_expected: bool,
    ) -> RunOutcome {
        RunOutcome {
            timed_out,
            exit_code,
            last_error_line: last_error_line.map(ToString::to_string),
            lines_as_expected,
        }
    }

    #[test]
    fn classifies_success_with_expected_lines_as_pass() {
        assert_eq!(
            classify(&outcome(false, Some(0), None, true)),
            Category::Pass
        );
    }

    #[test]
    fn classifies_success_with_unexpected_lines_as_a_failure() {
        assert_eq!(
            classify(&outcome(false, Some(0), None, false)),
            Category::UnexpectedLines
        );
    }

    #[test]
    fn classifies_a_failure_exit_from_the_fixed_message() {
        let result = classify(&outcome(
            false,
            Some(1),
            Some("[error] the start map could not be loaded from the payload"),
            false,
        ));
        assert_eq!(result, Category::LoadError("missing-map"));
    }

    #[test]
    fn classifies_unknown_failure_message_without_leaking_it() {
        let result = classify(&outcome(
            false,
            Some(1),
            Some("[error] some unexpected raw message with a path in it"),
            false,
        ));
        assert_eq!(result, Category::LoadError("unspecified"));
    }

    #[test]
    fn classifies_timeout_before_checking_the_exit_code() {
        assert_eq!(
            classify(&outcome(true, Some(0), None, true)),
            Category::Timeout
        );
    }

    #[test]
    fn classifies_signal_kill_as_crash() {
        assert_eq!(classify(&outcome(false, None, None, true)), Category::Crash);
    }

    fn scenario_with(
        follow_level_change: bool,
        start_inventory: &'static [&'static str],
    ) -> Scenario {
        Scenario {
            name: "test scenario",
            file: "test.txt",
            map: "t0a0b1",
            present: &BASE_PRESENT,
            absent: &BASE_ABSENT,
            follow_level_change,
            start_inventory,
        }
    }

    /// A scenario whose `start_inventory` field is empty runs exactly as
    /// every scenario always has: no `--start-inventory` argument at all.
    #[test]
    fn build_command_omits_start_inventory_when_the_field_is_empty() {
        let scenario = scenario_with(false, &[]);
        let command = build_command(
            Path::new("open-half-life"),
            Path::new("/payload"),
            Path::new("/scenarios/test.txt"),
            &scenario,
        );
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert!(
            !args.contains(&std::ffi::OsStr::new("--start-inventory")),
            "expected no --start-inventory argument, got {args:?}"
        );
    }

    /// A scenario with a non-empty `start_inventory` field passes
    /// `--start-inventory` with its items joined by commas — the list
    /// format `ohl_engine::parse_start_inventory` expects.
    #[test]
    fn build_command_passes_start_inventory_when_the_field_is_set() {
        let scenario = scenario_with(false, &["weapon_shotgun", "ammo_buckshot"]);
        let command = build_command(
            Path::new("open-half-life"),
            Path::new("/payload"),
            Path::new("/scenarios/test.txt"),
            &scenario,
        );
        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        let flag_index = args
            .iter()
            .position(|arg| *arg == std::ffi::OsStr::new("--start-inventory"))
            .expect("--start-inventory should be present");
        assert_eq!(
            args[flag_index + 1],
            std::ffi::OsStr::new("weapon_shotgun,ammo_buckshot")
        );
    }

    fn fake_report(name: &'static str, category: Category) -> ScenarioReport {
        ScenarioReport { name, category }
    }

    #[test]
    fn summary_reports_one_row_per_scenario_and_aggregate_counts() {
        let reports = vec![
            fake_report("walk forward in the training start", Category::Pass),
            fake_report(
                "look around in the first chapter start",
                Category::UnexpectedLines,
            ),
        ];
        let summary = write_summary(&reports, Duration::from_secs(2));
        assert!(summary.contains("| walk forward in the training start | Pass |"));
        assert!(summary.contains("| look around in the first chapter start | Unexpected-lines |"));
        assert!(summary.contains("| Pass | 1 |"));
        assert!(summary.contains("| Unexpected-lines | 1 |"));
    }

    #[test]
    fn summary_never_names_a_payload_file_or_a_pixel_statistic() {
        let reports = vec![fake_report(
            "walk forward in the training start",
            Category::LoadError("no-gpu"),
        )];
        let summary = write_summary(&reports, Duration::from_secs(1));

        // No payload path, no file name, no pixel/dimension figure — only
        // scenario names (project-authored) and aggregate counts appear.
        assert!(!summary.contains(".txt"));
        assert!(!summary.contains(".png"));
        assert!(!summary.contains("payload_root"));
        assert!(!summary.contains('/'));
        assert!(!summary.contains("1280"));
        assert!(!summary.contains("no-gpu"));
    }

    #[test]
    fn sanitize_error_code_never_leaks_an_unknown_message() {
        assert_eq!(
            sanitize_error_code("some raw diagnostic with /a/path in it"),
            "unspecified"
        );
    }

    #[test]
    fn every_scenario_file_is_present_under_xtask_smoke_scenarios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let scenarios_dir = root.join("xtask").join("smoke-scenarios");
        for scenario in scenarios() {
            assert!(
                scenarios_dir.join(scenario.file).is_file(),
                "missing scenario file: {}",
                scenario.file
            );
        }
    }
}
