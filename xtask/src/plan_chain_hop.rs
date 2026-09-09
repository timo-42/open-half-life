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
//! The report is aggregate-only, like every other summary in this crate:
//! how many routes the chain already had, how many cells the search
//! reached, how many segments and door presses the planned route holds,
//! how many plan/replay attempts it took and how long the route runs for.
//! No route file's contents, no payload path and no map name past
//! `ohl_campaign`'s own cited table is printed.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use clap::Parser;

use crate::chain_walk::{APP_BIN_NAME, assemble_chain, build_release_binary, capture_stderr};

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

    /// The search's per-round cell cap.
    #[arg(long, value_name = "N", default_value_t = 300_000)]
    cell_cap: usize,

    /// The search's round cap.
    #[arg(long, value_name = "N", default_value_t = 12)]
    round_cap: usize,

    /// Timeout for the whole run, in seconds.
    #[arg(long, default_value_t = 3_600)]
    timeout: u64,
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
const REPORT_PREFIXES: [(&str, &str); 6] = [
    ("Route plan cells: ", "Cells the search reached"),
    ("Route plan segments: ", "Walk-forward segments"),
    ("Route plan ladder climbs: ", "Ladder climbs"),
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
}

/// Parses a finished planner run's stderr, reading only the app's own
/// fixed lines and keeping no other text.
#[must_use]
pub fn parse_report(stderr: &str) -> PlanReport {
    let mut values = Vec::new();
    for (prefix, label) in REPORT_PREFIXES {
        if let Some(value) = stderr.lines().rev().find_map(|line| {
            let index = line.find(prefix)?;
            Some(
                line[index + prefix.len()..]
                    .trim_end_matches('.')
                    .to_string(),
            )
        }) {
            values.push((label, value));
        }
    }
    PlanReport {
        written: stderr.contains(WRITTEN_LINE),
        chain_incomplete: stderr.contains(CHAIN_INCOMPLETE_LINE),
        values,
    }
}

/// Renders the aggregate summary. Never prints a route file's name or
/// contents, a payload path, or any map name beyond the caller's own
/// `ohl_campaign`-table start name.
#[must_use]
pub fn write_summary(start: &str, routes: usize, report: &PlanReport, elapsed: Duration) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Chain hop plan summary\n\n");
    let _ = writeln!(out, "Start map: {start} (ohl_campaign's cited table)");
    let _ = writeln!(out, "Wall-clock elapsed: {:.1}s\n", elapsed.as_secs_f64());
    out.push_str("| Measure | Value |\n|---|---|\n");
    let _ = writeln!(out, "| Routes already in the chain | {routes} |");
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

/// Builds the release `open-half-life` binary with `dev-tools`, which is
/// where `--plan-route` lives.
fn build_dev_tools_binary(root: &Path) -> Result<PathBuf, &'static str> {
    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "ohl-app",
            "--release",
            "--features",
            "dev-tools",
        ])
        .current_dir(root)
        .status()
        .map_err(|_| "cargo build -p ohl-app --release --features dev-tools failed")?;
    if !status.success() {
        return Err("cargo build -p ohl-app --release --features dev-tools failed");
    }
    let name = if cfg!(windows) {
        format!("{APP_BIN_NAME}.exe")
    } else {
        APP_BIN_NAME.to_string()
    };
    Ok(root.join("target").join("release").join(name))
}

/// Entry point for `cargo xtask plan-chain-hop`.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("plan-chain-hop".to_string()).chain(raw_args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };

    let start = args
        .start
        .clone()
        .unwrap_or_else(|| ohl_campaign::STARTMAP.to_string());
    let routes_dir = root.join("xtask").join("chain-routes");
    let routes = match assemble_chain(&routes_dir, &start) {
        Ok(routes) => routes,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The chain's routes are named by position: `<start>.txt` is route 0,
    // so a chain of `n` routes plans hop `n` next.
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| routes_dir.join(format!("{start}-hop{}.txt", routes.len())));

    let bin = match args.bin.clone() {
        Some(bin) => bin,
        None => match build_dev_tools_binary(root).or_else(|_| build_release_binary(root)) {
            Ok(bin) => bin,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        },
    };

    println!(
        "Running {} chain route(s), then planning the next hop...",
        routes.len()
    );
    let mut command = Command::new(&bin);
    command
        .arg("--payload-root")
        .arg(&args.payload_root)
        .arg("--map")
        .arg(&start);
    for route in &routes {
        command.arg("--chain-script").arg(route);
    }
    command
        .arg("--plan-route")
        .arg(&out)
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

    let started = Instant::now();
    let stderr = capture_stderr(command, Duration::from_secs(args.timeout));
    let elapsed = started.elapsed();
    let report = parse_report(&stderr);
    print!("{}", write_summary(&start, routes.len(), &report, elapsed));

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
[info] Route plan door presses: 1.
[info] Route plan replay attempts: 4.
[info] Route plan simulated seconds: 22.6.
[info] Route plan written.
";
        let report = parse_report(stderr);
        assert!(report.written);
        assert_eq!(report.values.len(), 5);
        assert_eq!(
            report.values[0],
            ("Cells the search reached", "5578".into())
        );
        assert_eq!(
            report.values[4],
            ("Route length in game seconds", "22.6".into())
        );
    }

    #[test]
    fn a_run_that_wrote_nothing_is_a_failure() {
        let report = parse_report("[error] no route to the goal could be planned and validated\n");
        assert!(!report.written);
        assert!(!report.chain_incomplete);
        assert!(report.values.is_empty());
        let summary = write_summary("c0a0", 5, &report, Duration::from_secs(3));
        assert!(summary.contains("Fail (no route replayed to the goal"));
        assert!(summary.contains("| Routes already in the chain | 5 |"));
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
        let summary = write_summary("c0a0", 5, &report, Duration::from_secs(3));
        assert!(
            summary.contains("Fail (the chain did not arrive cleanly; nothing was planned)"),
            "the refusal reads as distinct from an ordinary planning failure"
        );
    }
}
