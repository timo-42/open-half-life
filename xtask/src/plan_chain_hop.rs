//! `cargo xtask plan-chain-hop`: plan the chain's *next* route with the
//! in-engine route planner.
//!
//! `cargo xtask chain-walk` (see `crate::chain_walk`) walks the campaign
//! with one hand-authored route per hop and reports how many maps deep it
//! got. Authoring the *next* route has always been the hard part: it can
//! only be written from the arrival point the previous routes leave the
//! player at, which is a state no cold `--map <name>` load reproduces, and
//! local investigation notes (not part of the repository) record two
//! hand-written navigation probes failing to walk a route the
//! reachability report says exists.
//!
//! This command is that job, run by the engine instead: it assembles the
//! same chain `chain-walk` does, runs it in one process, and hands the
//! arrival point to `--plan-route` (`ohl_engine::route_plan` plus
//! `ohl_app`'s closed-loop replay validation). What comes out is a route
//! file for the next hop — or nothing at all, because the planner writes
//! only a script whose replay actually fired the level change.
//!
//! `--aggregate-only` emits a fixed admission verdict and allowlisted error
//! codes. A successful marker requires a successful child, a complete clean
//! prefix, and a fresh file matching the serializer's bounded controller
//! subset. Every attempt uses an owned directory in ignored `target/`; the
//! destination is created without overwrite only after all checks pass.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;

use crate::chain_walk::{
    CapturedRun, ChildEnd, MAX_CHAIN_ROUTES, assemble_chain, build_chain_binary, capture_stderr,
    seconds, start_inventory_row, unsigned,
};

/// `cargo xtask plan-chain-hop` command line.
#[derive(Debug, Parser)]
#[command(name = "plan-chain-hop")]
struct Args {
    /// An already-imported payload store root.
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// Where to write the planned route. Defaults to the next hop's own
    /// file under `xtask/chain-routes/`, named by position in the chain
    /// exactly as `cargo xtask chain-walk` expects it.
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,

    /// A prebuilt `open-half-life` binary, which must be a `dev-tools`
    /// build (`--plan-route` exists only there). Without one, the release
    /// binary is built with that feature first.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// The map the chain starts on; defaults to `ohl_campaign::STARTMAP`.
    #[arg(long, value_name = "NAME")]
    start: Option<String>,

    /// Plan a route to this classname instead of `trigger_changelevel`.
    #[arg(long, value_name = "CLASSNAME")]
    goal: Option<String>,

    /// How many plan/replay attempts the planner's closed loop may take.
    #[arg(long, value_name = "N")]
    attempts: Option<usize>,

    /// How many of one plan's own travelling segments each attempt
    /// commits before it replays and plans again (`--plan-segments`).
    ///
    /// Worth raising on a hop whose map answers a one-segment loop with
    /// the same plan every time: committing a few segments at once walks
    /// the player somewhere the next plan is genuinely different from.
    #[arg(long, value_name = "N")]
    segments: Option<usize>,

    /// The search's per-round cell cap.
    #[arg(long, value_name = "N", default_value_t = 300_000)]
    cell_cap: usize,

    /// The search's round cap.
    #[arg(long, value_name = "N", default_value_t = 12)]
    round_cap: usize,

    /// Timeout for the whole run, in seconds.
    #[arg(long, default_value_t = 3_600)]
    timeout: u64,

    /// A `--start-inventory` list handed to the app at the *start* map's
    /// load, carried onward by the chain exactly as a picked-up weapon
    /// would be. **Empty by default**, matching what `cargo xtask
    /// chain-walk` will walk the planned route with: the routes collect
    /// what the maps offer for themselves
    /// (`ohl_engine::route_plan`'s pickup detours), and planning a hop
    /// against a loadout the walk will not have is planning against the
    /// wrong state. [`crate::chain_walk::CHAIN_START_INVENTORY`] is the
    /// harness aid this used to default to; see there.
    #[arg(long, value_name = "LIST", default_value = "")]
    start_inventory: String,

    /// Emit only a fixed planner verdict and allowlisted error codes.
    #[arg(long)]
    aggregate_only: bool,
}

/// The fixed line the app logs once a planned route has been written.
pub const WRITTEN_LINE: &str = "Route plan written.";

/// The fixed line `crates/ohl-app/src/game_run.rs` logs (as its returned
/// error) instead of planning at all, when the chain run first ran stopped
/// short or re-entered a map: planning from that state would plan from
/// wherever the interrupted route happened to leave the player, not the
/// hop's own clean arrival point. No file is written when this line
/// appears.
pub const CHAIN_INCOMPLETE_LINE: &str =
    "Route plan refused: the chain did not arrive cleanly, so nothing was planned.";

/// The fixed prefixes the app's own planner report lines carry.
const REPORT_PREFIXES: [(&str, &str); 7] = [
    ("Route plan cells: ", "Cells the search reached"),
    ("Route plan segments: ", "Walk-forward segments"),
    ("Route plan ladder climbs: ", "Ladder climbs"),
    ("Route plan pickup detours: ", "Pickup detours"),
    ("Route plan door presses: ", "Door presses"),
    ("Route plan replay attempts: ", "Plan/replay attempts"),
    (
        "Route plan simulated seconds: ",
        "Route length in game seconds",
    ),
];

/// What one planner run reported, parsed from the app's own fixed lines.
#[derive(Debug, PartialEq, Eq)]
pub struct PlanReport {
    /// Whether a route file was actually written.
    pub written: bool,
    /// Whether the run refused to plan at all because the chain it ran
    /// first stopped short or re-entered a map ([`CHAIN_INCOMPLETE_LINE`]):
    /// a distinct failure from "the planner tried and found no route".
    pub chain_incomplete: bool,
    /// Each report line's label and value, in the order above.
    pub values: Vec<(&'static str, String)>,
    /// Exactly one marker and every required numeric field were valid.
    pub valid: bool,
}

/// Parses a finished planner run's stderr, reading only the app's own
/// fixed lines and keeping no other text.
#[must_use]
pub fn parse_report(stderr: &str) -> PlanReport {
    let mut fields: [Option<String>; 7] = std::array::from_fn(|_| None);
    let (mut markers, mut refusals, mut invalid) = (0, 0, false);
    for raw in stderr.lines() {
        if let Some(error) = raw.strip_prefix("[error] ")
            && (error == "Route plan refused" || error.starts_with("Route plan refused:"))
        {
            if error == CHAIN_INCOMPLETE_LINE {
                refusals += 1;
            } else {
                invalid = true;
            }
        }
        let Some(line) = raw.strip_prefix("[info] ") else {
            continue;
        };
        if line.starts_with(WRITTEN_LINE.trim_end_matches('.')) {
            if line == WRITTEN_LINE {
                markers += 1;
            } else {
                invalid = true;
            }
        }
        for (index, (prefix, _)) in REPORT_PREFIXES.iter().enumerate() {
            let Some(value) = line.strip_prefix(prefix) else {
                invalid |= line.starts_with(prefix.trim_end_matches(": "));
                continue;
            };
            let value = value.strip_suffix('.');
            let parsed = if index == 6 {
                value.and_then(seconds).map(|number| format!("{number:.1}"))
            } else {
                value
                    .and_then(unsigned::<u64>)
                    .map(|number| number.to_string())
            };
            invalid |= fields[index].is_some() || parsed.is_none();
            fields[index] = parsed;
        }
    }
    let valid = !invalid && markers == 1 && refusals == 0 && fields.iter().all(Option::is_some);
    PlanReport {
        written: valid,
        chain_incomplete: refusals != 0,
        valid,
        values: REPORT_PREFIXES
            .into_iter()
            .zip(fields)
            .filter_map(|((_, label), value)| value.map(|value| (label, value)))
            .collect(),
    }
}

/// Renders the aggregate summary. Never prints a route file's name or
/// contents, a payload path, or any map name beyond the caller's own
/// `ohl_campaign`-table start name.
#[must_use]
pub fn write_summary(
    start: &str,
    routes: usize,
    report: &PlanReport,
    start_inventory: Option<&str>,
    elapsed: Duration,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Chain hop plan summary\n\n");
    let _ = writeln!(out, "Start map: {start} (ohl_campaign's cited table)");
    let _ = writeln!(out, "Wall-clock elapsed: {:.1}s\n", elapsed.as_secs_f64());
    out.push_str("| Measure | Value |\n|---|---|\n");
    let _ = writeln!(out, "| Routes already in the chain | {routes} |");
    // The same row, labelled the same way, as `cargo xtask chain-walk`'s.
    out.push_str(&start_inventory_row(start_inventory));
    for (label, value) in &report.values {
        let _ = writeln!(out, "| {label} | {value} |");
    }
    let _ = writeln!(
        out,
        "| Result | {} |",
        if report.written {
            "Pass (a validated route was written)"
        } else if report.chain_incomplete {
            "Fail (the chain did not arrive cleanly; nothing was planned)"
        } else {
            "Fail (no route replayed to the goal; nothing was written)"
        }
    );
    out
}

/// Entry point for `cargo xtask plan-chain-hop`.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    run_with(root, raw_args, &mut std::io::stdout(), capture_stderr)
}

// This exact header and the bounded subset below are the serializer's published
// artifact contract, not a second implementation of the general app parser.
const CANDIDATE_HEADER: &str = "\
# A machine-planned chain-walk route (--plan-route / cargo xtask
# plan-chain-hop), not a hand-authored one.
#
# A bounded, cost-ordered walk over this map's live collision model
# (ohl_engine::route_plan, the same edge model --reachability-report
# triages with) found a path from the arrival point this route starts at
# to the map's own level-change trigger, straightened it into runs,
# converted the runs into the lines below, and replayed them in-process
# until the level change actually fired. The route below is the one that
# fired; nothing else was written.
#
# Commands only: no name, path or coordinate from any payload appears
# here, and none was ever printed (docs/CLEAN_ROOM.md). The planner's own
# search coordinates never left the process that produced this file.
#
# A line's leading number is a count of ticks, each one CAPTURE_STEP
# (1/60 s) of simulated time -- not seconds.
\n";

const CANDIDATE_LIMIT: u64 = 256 * 1024;

fn valid_candidate(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Some(body) = text.strip_prefix(CANDIDATE_HEADER) else {
        return false;
    };
    let (mut lines, mut total) = (0_u32, 0_u32);
    for line in body.lines() {
        let words: Vec<_> = line.split_ascii_whitespace().collect();
        let Some(ticks) = words.first().and_then(|value| unsigned::<u32>(value)) else {
            return false;
        };
        if ticks == 0 {
            return false;
        }
        let Some(sum) = total.checked_add(ticks) else {
            return false;
        };
        total = sum;
        lines += 1;
        if total > 100_000 || lines > 4096 {
            return false;
        }
        match &words[1..] {
            ["forward" | "back" | "wait" | "guard" | "use"] | ["forward", "jump"] => {}
            ["look", "0", yaw] => {
                // The serializer emits two fractional digits and a bounded
                // relative turn. General script tokens/metadata are forbidden.
                let unsigned_yaw = yaw.strip_prefix('-').unwrap_or(yaw);
                let Some((_, fraction)) = unsigned_yaw.split_once('.') else {
                    return false;
                };
                if fraction.len() != 2 || seconds(unsigned_yaw).is_none_or(|yaw| yaw > 180.0) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    lines > 0 && text.ends_with('\n')
}

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Only this attempt's fresh, ignored directory is removed on drop.
struct CandidateStage {
    directory: PathBuf,
    file: PathBuf,
}

impl CandidateStage {
    fn new(root: &Path) -> Result<Self, &'static str> {
        let parent = root.join("target/chain-plan-staging");
        std::fs::create_dir_all(&parent).map_err(|_| "candidate-stage")?;
        for _ in 0..100 {
            let ordinal = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = parent.join(format!("attempt-{}-{ordinal}", std::process::id()));
            match std::fs::create_dir(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        file: directory.join("candidate.txt"),
                        directory,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("candidate-stage"),
            }
        }
        Err("candidate-stage")
    }

    fn admit(&self, out: &Path) -> Result<(), &'static str> {
        use std::io::{Read as _, Write as _};
        let metadata = std::fs::symlink_metadata(&self.file).map_err(|_| "candidate-missing")?;
        if !metadata.file_type().is_file() || metadata.len() > CANDIDATE_LIMIT {
            return Err("candidate-grammar");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&self.file)
            .map_err(|_| "candidate-read")?
            .take(CANDIDATE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "candidate-read")?;
        if bytes.len() as u64 > CANDIDATE_LIMIT || !valid_candidate(&bytes) {
            return Err("candidate-grammar");
        }
        // create_new is the no-overwrite seam on every supported filesystem.
        // Only a file owned by this admission can be removed on write failure.
        let mut destination = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(out)
            .map_err(|_| "candidate-output")?;
        if destination.write_all(&bytes).is_err() || destination.sync_all().is_err() {
            drop(destination);
            let _ = std::fs::remove_file(out);
            return Err("candidate-output");
        }
        Ok(())
    }
}

impl Drop for CandidateStage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn plan_command(
    args: &Args,
    bin: &Path,
    start: &str,
    routes: &[PathBuf],
    candidate: &Path,
) -> Command {
    let mut command = Command::new(bin);
    command
        .arg("--payload-root")
        .arg(&args.payload_root)
        .arg("--map")
        .arg(start);
    for route in routes {
        command.arg("--chain-script").arg(route);
    }
    if !args.start_inventory.is_empty() {
        command.arg("--start-inventory").arg(&args.start_inventory);
    }
    command
        .arg("--plan-route")
        .arg(candidate)
        .arg("--reachability-cell-cap")
        .arg(args.cell_cap.to_string())
        .arg("--reachability-round-cap")
        .arg(args.round_cap.to_string());
    if let Some(goal) = &args.goal {
        command.arg("--plan-goal").arg(goal);
    }
    if let Some(attempts) = args.attempts {
        command.arg("--plan-attempts").arg(attempts.to_string());
    }
    if let Some(segments) = args.segments {
        command.arg("--plan-segments").arg(segments.to_string());
    }
    command
}

fn run_with<W: std::io::Write>(
    root: &Path,
    raw_args: &[String],
    output: &mut W,
    mut capture: impl FnMut(Command, Duration) -> CapturedRun,
) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("plan-chain-hop".to_string()).chain(raw_args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };
    let aggregate_only = args.aggregate_only;
    let fail = |output: &mut W, code: &str| {
        let _ = writeln!(output, "error: {code}");
        if aggregate_only {
            let _ = writeln!(output, "| Result | Not written |");
        }
        ExitCode::FAILURE
    };
    let start = args
        .start
        .clone()
        .unwrap_or_else(|| ohl_campaign::STARTMAP.to_string());
    let routes_dir = root.join("xtask/chain-routes");
    let Ok(routes) = assemble_chain(&routes_dir, &start) else {
        return fail(output, "chain-assembly");
    };
    if routes.len() == MAX_CHAIN_ROUTES {
        return fail(output, "chain-capacity");
    }
    let out = match args.out.clone() {
        Some(out) => out,
        None if start.eq_ignore_ascii_case(ohl_campaign::STARTMAP) => {
            routes_dir.join(format!("hop-{:04}.txt", routes.len()))
        }
        None => routes_dir.join(format!("{start}-hop{}.txt", routes.len())),
    };
    // Neither a stale destination nor a symlink can count as this attempt.
    if std::fs::symlink_metadata(&out).is_ok() {
        return fail(output, "candidate-exists");
    }
    let stage = match CandidateStage::new(root) {
        Ok(stage) => stage,
        Err(code) => return fail(output, code),
    };
    let bin = match args.bin.clone() {
        Some(bin) => bin,
        None => match build_chain_binary(root) {
            Ok(bin) => bin,
            Err(_) => return fail(output, "child-build"),
        },
    };
    let command = plan_command(&args, &bin, &start, &routes, &stage.file);
    let started = Instant::now();
    let captured = capture(command, Duration::from_secs(args.timeout));
    let mut report = parse_report(&captured.stderr);
    let prefix = crate::chain_walk::parse_report(&captured.stderr);
    let error = if captured.end != ChildEnd::Exited(0) {
        Some(captured.end.code())
    } else if !report.valid {
        Some("planner-report")
    } else if prefix.stopped_at != Some("The chain walk has no further route.")
        || prefix.depth != routes.len() + 1
    {
        Some("planner-prefix")
    } else {
        stage.admit(&out).err()
    };
    report.written = error.is_none();
    if let Some(code) = error {
        let _ = writeln!(output, "error: {code}");
    }
    if args.aggregate_only {
        let _ = writeln!(
            output,
            "| Result | {} |",
            if report.written {
                "Validated"
            } else {
                "Not written"
            }
        );
    } else {
        let _ = write!(
            output,
            "{}",
            write_summary(
                &start,
                routes.len(),
                &report,
                Some(args.start_inventory.as_str()).filter(|list| !list.is_empty()),
                started.elapsed()
            )
        );
    }
    if report.written {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_apps_own_report_lines() {
        let stderr = "\
[info] Route plan cells: 5578.
[info] Route plan segments: 11.
[info] Route plan ladder climbs: 0.
[info] Route plan pickup detours: 0.
[info] Route plan door presses: 1.
[info] Route plan replay attempts: 4.
[info] Route plan simulated seconds: 22.6.
[info] Route plan written.
";
        let report = parse_report(stderr);
        assert!(report.written);
        assert_eq!(report.values.len(), 7);
        assert_eq!(
            report.values[0],
            ("Cells the search reached", "5578".into())
        );
        assert_eq!(
            report.values[6],
            ("Route length in game seconds", "22.6".into())
        );
    }

    #[test]
    fn a_run_that_wrote_nothing_is_a_failure() {
        let report = parse_report("[error] no route to the goal could be planned and validated\n");
        assert!(!report.written);
        assert!(!report.chain_incomplete);
        assert!(report.values.is_empty());
        let summary = write_summary("c0a0", 5, &report, None, Duration::from_secs(3));
        assert!(summary.contains("Fail (no route replayed to the goal"));
        assert!(summary.contains("| Routes already in the chain | 5 |"));
    }

    /// The loadout row reads exactly as `cargo xtask chain-walk`'s does:
    /// the harness aid, a caller-supplied list, or `(none)` — never a
    /// harness-aid label on a list that is not the harness aid, and never
    /// a summary silent about what the chain carried.
    #[test]
    fn the_start_inventory_row_matches_the_chain_walks() {
        let report = parse_report("");
        let row =
            |list: Option<&str>| write_summary("c0a0", 5, &report, list, Duration::from_secs(1));
        assert!(
            row(Some(crate::chain_walk::CHAIN_START_INVENTORY))
                .contains("| Start inventory (harness aid) |")
        );
        let other = row(Some("weapon_shotgun"));
        assert!(other.contains("| Start inventory (caller-supplied) | weapon_shotgun |"));
        assert!(!other.contains("harness aid"));
        assert!(row(None).contains("| Start inventory | (none) |"));
    }

    /// A chain that stopped short or re-entered a map leaves the planner
    /// refusing to plan at all (`crates/ohl-app/src/game_run.rs`'s
    /// `PLAN_REFUSED_INCOMPLETE_CHAIN`), rather than planning from wherever
    /// the interrupted route happened to leave the player. That refusal has
    /// to be told apart from an ordinary "no route found" failure: it
    /// blames the chain, not the map.
    #[test]
    fn a_chain_that_did_not_arrive_cleanly_refuses_to_plan() {
        let stderr = format!("[error] {CHAIN_INCOMPLETE_LINE}\n");
        let report = parse_report(&stderr);
        assert!(!report.written, "nothing was written");
        assert!(
            report.chain_incomplete,
            "the fixed refusal line was recognised"
        );
        assert!(report.values.is_empty());
        let summary = write_summary("c0a0", 5, &report, None, Duration::from_secs(3));
        assert!(
            summary.contains("Fail (the chain did not arrive cleanly; nothing was planned)"),
            "the refusal reads as distinct from an ordinary planning failure"
        );
    }
}

#[cfg(test)]
pub(crate) fn candidate_header() -> &'static str {
    CANDIDATE_HEADER
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use crate::chain_walk::child_fixtures::{POISONS, command};

    fn setup() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir_all(root.path().join("xtask/chain-routes")).expect("routes");
        std::fs::write(
            root.path().join("xtask/chain-routes/hop-0000.txt"),
            b"1 wait\n",
        )
        .expect("route");
        root
    }

    fn args() -> Vec<String> {
        [
            "--payload-root",
            POISONS[2],
            "--bin",
            POISONS[2],
            "--start-inventory",
            POISONS[1],
            "--aggregate-only",
        ]
        .map(String::from)
        .into()
    }

    fn drive(root: &Path, raw: &[String], case: &str) -> (ExitCode, String) {
        let mut output = Vec::new();
        let exit = run_with(root, raw, &mut output, |app, _| {
            let arguments: Vec<_> = app.get_args().collect();
            let index = arguments
                .iter()
                .position(|arg| *arg == "--plan-route")
                .expect("candidate argument");
            let mut child = command(case);
            child.env("OHL_SYNTHETIC_OUT", arguments[index + 1]);
            capture_stderr(
                child,
                if case == "timeout" {
                    Duration::from_secs(1)
                } else {
                    Duration::from_secs(5)
                },
            )
        });
        (exit, String::from_utf8(output).expect("utf8"))
    }

    // Rust normalizes source CRLF pairs before tokenization; filesystem reads do
    // not. This cross-check follows that source contract, preserving lone CR.
    fn source_has_planner_header(source: &str) -> bool {
        let header = CANDIDATE_HEADER
            .strip_suffix('\n')
            .expect("extra separator");
        source.replace("\r\n", "\n").contains(header)
    }

    fn assert_private(output: &str) {
        for poison in POISONS {
            assert!(!output.contains(poison));
        }
        assert!(!output.contains(ohl_campaign::STARTMAP));
        assert!(!output.contains("chain-plan-staging"));
        assert!(!output.contains("hop-0001"));
        assert!(!output.contains("Cells the search reached"));
        assert!(!output.contains("Inventory on arrival"));
    }

    #[test]
    fn chain_planner_full_entrypoint_admits_only_fresh_valid_success_and_reports_no_poison() {
        for (case, success) in [
            ("valid", true),
            ("nonzero", false),
            ("timeout", false),
            ("overflow", false),
            ("missing", false),
            ("grammar", false),
            ("suffix", false),
        ] {
            let root = setup();
            let (exit, output) = drive(root.path(), &args(), case);
            assert_eq!(
                exit == ExitCode::SUCCESS,
                success,
                "synthetic child case {case}"
            );
            assert_private(&output);
            assert!(output.contains(if success {
                "| Result | Validated |"
            } else {
                "| Result | Not written |"
            }));
            let admitted = root.path().join("xtask/chain-routes/hop-0001.txt");
            assert_eq!(admitted.is_file(), success);
            if success {
                assert!(valid_candidate(
                    &std::fs::read(admitted).expect("candidate")
                ));
            }
            let staging = root.path().join("target/chain-plan-staging");
            assert_eq!(std::fs::read_dir(staging).expect("staging").count(), 0);
        }
    }

    #[test]
    fn chain_planner_completion_checks_also_apply_to_compatibility_output() {
        let root = setup();
        let raw: Vec<_> = args()
            .into_iter()
            .filter(|arg| arg != "--aggregate-only")
            .collect();
        let (exit, output) = drive(root.path(), &raw, "nonzero");
        assert_eq!(exit, ExitCode::FAILURE);
        assert!(output.contains("| Result | Fail"));
        assert!(!root.path().join("xtask/chain-routes/hop-0001.txt").exists());
    }

    #[test]
    fn chain_planner_stale_output_is_preserved_and_never_launches_a_child() {
        let root = setup();
        let stale = root.path().join("xtask/chain-routes/hop-0001.txt");
        std::fs::write(&stale, b"synthetic old route").expect("stale");
        let mut raw = args();
        raw.extend(["--out".to_string(), stale.to_string_lossy().into_owned()]);
        let mut output = Vec::new();
        let exit = run_with(root.path(), &raw, &mut output, |_, _| {
            panic!("stale output never launches")
        });
        assert_eq!(exit, ExitCode::FAILURE);
        assert_eq!(
            std::fs::read(&stale).expect("preserved stale"),
            b"synthetic old route"
        );
        assert_private(&String::from_utf8(output).expect("utf8"));
    }

    #[test]
    fn chain_planner_stage_admission_cannot_overwrite_and_only_cleans_its_owned_attempt() {
        let root = setup();
        let out = root.path().join("synthetic-output.txt");
        let unrelated = root.path().join("target/chain-plan-staging/unrelated");
        std::fs::create_dir_all(&unrelated).expect("unrelated");
        let stage = CandidateStage::new(root.path()).expect("stage");
        assert!(!stage.file.exists());
        assert_eq!(stage.admit(&out), Err("candidate-missing"));
        std::fs::write(&stage.file, format!("{CANDIDATE_HEADER}1 wait\n"))
            .expect("stage candidate");
        std::fs::write(&out, b"old").expect("racing output");
        assert_eq!(stage.admit(&out), Err("candidate-output"));
        assert_eq!(std::fs::read(&out).expect("preserved"), b"old");
        let own = stage.directory.clone();
        drop(stage);
        assert!(!own.exists());
        assert!(unrelated.exists());
        let first = CandidateStage::new(root.path()).expect("first owned attempt");
        std::fs::write(&first.file, b"first attempt").expect("first file");
        let second = CandidateStage::new(root.path()).expect("second owned attempt");
        assert_ne!(first.directory, second.directory);
        assert!(!second.file.exists());
        drop(second);
        assert_eq!(
            std::fs::read(&first.file).expect("first preserved"),
            b"first attempt"
        );
    }

    #[test]
    fn chain_planner_bounded_serializer_subset_rejects_metadata_and_malformed_commands() {
        let valid = "2 look 0 -90.00\n1 use\n3 forward jump\n2 back\n4 guard\n5 wait\n";
        assert!(valid_candidate(
            format!("{CANDIDATE_HEADER}{valid}").as_bytes()
        ));
        for body in [
            "",
            "0 wait\n",
            "1 unknown\n",
            "1 forward # synthetic-suffix-secret\n",
            "1 guard attack\n",
            "1 look 0 NaN\n",
            "1 look 0 181.00\n",
            "1 look 1 0.00\n",
            "100001 wait\n",
            "4294967296 wait\n",
            "# metadata\n1 wait\n",
            "1 wait",
            "1 forward jump use\n",
        ] {
            assert!(
                !valid_candidate(format!("{CANDIDATE_HEADER}{body}").as_bytes()),
                "malformed synthetic candidate"
            );
        }
        assert!(!valid_candidate(b"1 wait\n"));
        assert!(!valid_candidate(&[0xff]));
        let too_many = format!("{CANDIDATE_HEADER}{}", "1 wait\n".repeat(4097));
        assert!(!valid_candidate(too_many.as_bytes()));
        let root = setup();
        let stage = CandidateStage::new(root.path()).expect("stage");
        let oversized = format!("{CANDIDATE_HEADER}1{}wait\n", " ".repeat(256 * 1024));
        assert!(
            valid_candidate(oversized.as_bytes()),
            "grammar alone cannot enforce a byte limit"
        );
        std::fs::write(&stage.file, oversized).expect("oversize");
        assert_eq!(
            stage.admit(&root.path().join("out.txt")),
            Err("candidate-grammar")
        );
        // The serializer header is checked against source, without linking the app
        // or adding its engine/render dependencies to xtask's graph.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace");
        let source = std::fs::read_to_string(workspace.join("crates/ohl-app/src/route_planner.rs"))
            .expect("serializer source");
        assert!(source_has_planner_header(&source));
        let lf = source.replace("\r\n", "\n");
        assert!(source_has_planner_header(&lf));
        assert!(source_has_planner_header(&lf.replace('\n', "\r\n")));
        assert!(!source_has_planner_header(
            &lf.replace("# Commands only:", "# Commands changed:")
        ));
        assert!(!source_has_planner_header(
            &lf.replace("# Commands only:", "# Commands \ronly:")
        ));
        let candidate = format!("{CANDIDATE_HEADER}1 wait\n");
        assert!(
            !valid_candidate(candidate.replace('\n', "\r\n").as_bytes()),
            "candidate bytes remain exact"
        );
    }

    #[test]
    fn chain_planner_report_requires_all_exact_unique_numbers_and_marker() {
        let valid = "[info] Route plan cells: 1.\n[info] Route plan segments: 1.\n[info] Route plan ladder climbs: 0.\n[info] Route plan pickup detours: 0.\n[info] Route plan door presses: 0.\n[info] Route plan replay attempts: 1.\n[info] Route plan simulated seconds: 1.0.\n[info] Route plan written.\n";
        assert!(parse_report(valid).valid);
        assert!(
            parse_report(&format!(
                "{valid}[error] Route plan refusedness is synthetic.\n"
            ))
            .valid
        );
        for text in [
            valid.replace("[info] Route plan written.\n", ""),
            valid.replace("[info] Route plan cells: 1.\n", ""),
            format!("{valid}[info] Route plan written.\n"),
            format!("{valid}[info] Route plan cells: 1.\n"),
            format!("{valid}[error] {CHAIN_INCOMPLETE_LINE}\n"),
            format!("{valid}[error] Route plan refused\n"),
            valid.replace("cells: 1.", "cells: 1. synthetic-suffix-secret"),
            valid.replace("seconds: 1.0.", "seconds: NaN."),
            valid.replace("seconds: 1.0.", "seconds: inf."),
            valid.replace("written.", "written. synthetic-suffix-secret"),
            valid.replace("[info]", "synthetic-prefix [info]"),
        ] {
            let report = parse_report(&text);
            assert!(!report.valid);
            assert!(!report.written);
            let summary = write_summary("synthetic-start", 1, &report, None, Duration::ZERO);
            assert!(!summary.contains(POISONS[4]));
        }
    }

    #[test]
    fn chain_planner_refuses_incomplete_dead_reentered_prefix_or_wrong_depth_even_with_a_written_marker()
     {
        for (terminal, depth) in [
            ("The chain walk stopped.", 2),
            (crate::chain_walk::SECTION_ENDED_LINE, 2),
            (crate::chain_walk::ARRIVED_DEAD_LINE, 2),
            (crate::chain_walk::RE_ENTERED_LINE, 2),
            ("The chain walk has no further route.", 1),
        ] {
            let root = setup();
            let mut output = Vec::new();
            let exit = run_with(root.path(), &args(), &mut output, |app, _| {
                let arguments: Vec<_> = app.get_args().collect();
                let index = arguments
                    .iter()
                    .position(|arg| *arg == "--plan-route")
                    .expect("candidate");
                let mut child = command("valid");
                child.env("OHL_SYNTHETIC_OUT", arguments[index + 1]);
                let mut captured = capture_stderr(child, Duration::from_secs(5));
                captured.stderr = captured
                    .stderr
                    .replace("The chain walk has no further route.", terminal)
                    .replace(
                        "Chain walk depth: 2.",
                        &format!("Chain walk depth: {depth}."),
                    );
                captured
            });
            assert_eq!(exit, ExitCode::FAILURE);
            assert!(!root.path().join("xtask/chain-routes/hop-0001.txt").exists());
            assert_private(&String::from_utf8(output).expect("utf8"));
        }
    }

    #[test]
    fn chain_planner_full_entrypoint_rejects_truncated_refusal_and_preserves_neighbor_namespace() {
        use std::fmt::Write as _;
        for (line, success) in [
            ("[error] Route plan refused", false),
            ("[error] Route plan refusedness is synthetic.", true),
        ] {
            let root = setup();
            let mut output = Vec::new();
            let exit = run_with(root.path(), &args(), &mut output, |app, _| {
                let arguments: Vec<_> = app.get_args().collect();
                let index = arguments
                    .iter()
                    .position(|arg| *arg == "--plan-route")
                    .expect("candidate");
                let mut child = command("valid");
                child.env("OHL_SYNTHETIC_OUT", arguments[index + 1]);
                let mut captured = capture_stderr(child, Duration::from_secs(5));
                let _ = writeln!(captured.stderr, "{line}");
                captured
            });
            assert_eq!(exit == ExitCode::SUCCESS, success);
            assert_eq!(
                root.path().join("xtask/chain-routes/hop-0001.txt").exists(),
                success
            );
            let output = String::from_utf8(output).expect("utf8");
            assert_private(&output);
            assert!(!output.contains("refusedness"));
            if !success {
                assert_eq!(output, "error: planner-report\n| Result | Not written |\n");
            }
        }
    }

    #[test]
    fn chain_planner_typed_spawn_wait_and_read_outcomes_refuse_even_complete_reports() {
        for end in [
            ChildEnd::SpawnFailed,
            ChildEnd::WaitFailed,
            ChildEnd::ReadFailed,
        ] {
            let root = setup();
            let mut output = Vec::new();
            let exit = run_with(root.path(), &args(), &mut output, |app, _| {
                let arguments: Vec<_> = app.get_args().collect();
                let index = arguments
                    .iter()
                    .position(|arg| *arg == "--plan-route")
                    .expect("candidate");
                let mut child = command("valid");
                child.env("OHL_SYNTHETIC_OUT", arguments[index + 1]);
                let mut captured = capture_stderr(child, Duration::from_secs(5));
                captured.end = end;
                captured
            });
            assert_eq!(exit, ExitCode::FAILURE);
            let output = String::from_utf8(output).expect("utf8");
            assert!(output.contains(end.code()));
            assert_private(&output);
            assert!(!root.path().join("xtask/chain-routes/hop-0001.txt").exists());
        }
    }

    #[test]
    fn chain_planner_cap_is_a_fixed_failure_before_child_launch() {
        let root = setup();
        for ordinal in 1..MAX_CHAIN_ROUTES {
            std::fs::write(
                root.path()
                    .join(format!("xtask/chain-routes/hop-{ordinal:04}.txt")),
                b"1 wait\n",
            )
            .expect("route");
        }
        let mut output = Vec::new();
        assert_eq!(
            run_with(root.path(), &args(), &mut output, |_, _| panic!(
                "capacity never spawns"
            )),
            ExitCode::FAILURE
        );
        let output = String::from_utf8(output).expect("utf8");
        assert!(output.contains("chain-capacity"));
        assert_private(&output);
    }
}
