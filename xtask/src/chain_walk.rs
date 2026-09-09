//! `cargo xtask chain-walk`: the chained campaign walk.
//!
//! The per-map scenarios `xtask/src/combat_smoke.rs` runs each start at
//! their own map's `info_player_start` with an empty inventory. A real
//! campaign never does that: the player arrives through a level change, at
//! an offset from the destination's `info_landmark`, carrying whatever the
//! previous maps gave them. Routes authored from a cold spawn therefore do
//! not compose, and earlier follow-up investigations (recorded in local
//! notes, not part of the repository) found several maps
//! "blocked" for exactly that reason — the cold load has no campaign state
//! to work with.
//!
//! This command walks the campaign the way the campaign is played: it
//! starts the built `open-half-life` binary once, on
//! `ohl_campaign::STARTMAP`, with one `--chain-script` route per hop (see
//! `crates/ohl-app/src/game_run.rs`'s `run_chained`). Route 0 runs from the
//! start map's own player start; each later route runs from where the
//! preceding route's followed level change put the player down, inside the
//! same process, so `ohl_engine::transition`'s carry machinery — health,
//! armor, weapons, ammo, the suit — is what supplies the campaign state.
//!
//! The report is aggregate-only, in the same spirit as this crate's other
//! smoke summaries: how many maps deep the chain got, how many simulated
//! seconds that took, and which of the app's two fixed terminal lines
//! ended it. No map name past `ohl_campaign`'s own publicly sourced table
//! is printed, and no route file's contents ever reach the summary.
//!
//! # Route files
//!
//! Routes live under `xtask/chain-routes/`, named by *position in the
//! chain* rather than by destination map:
//!
//! - `<start>.txt` — the route from `<start>`'s own player start.
//!   `<start>` is a name from `ohl_campaign`'s cited table (by default
//!   [`ohl_campaign::STARTMAP`]).
//! - `<start>-hop1.txt` — the route from where the first level change out
//!   of `<start>` lands, `-hop2.txt` from the second, and so on.
//!
//! Naming them ordinally is deliberate, not a convenience:
//! `docs/CLEAN_ROOM.md` rule 7 allows only lawfully public name literals in
//! this repository, and which map a `trigger_changelevel` actually lands in
//! is a fact about the user's own payload. "The map reached by the first
//! level change out of `c0a0`" says what the file is for without writing
//! down a name that has no public citation. Route file *contents* follow
//! the same rule the `xtask/smoke-scenarios/` files already do: script
//! commands, table names and route words only.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;

/// `cargo xtask chain-walk` command line.
#[derive(Debug, Parser)]
#[command(name = "chain-walk")]
struct Args {
    /// An already-imported payload store root (the directory holding a
    /// published tree's `files/` directory, or one level above it).
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// A prebuilt `open-half-life` binary. Without one, `cargo build -p
    /// ohl-app --release` is run first and the release binary is used.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// The map the chain starts on. Must be a name from `ohl_campaign`'s
    /// own cited table; defaults to [`ohl_campaign::STARTMAP`].
    #[arg(long, value_name = "NAME")]
    start: Option<String>,

    /// How many maps the chain must enter for this command to succeed.
    /// One means "the start map loaded"; two means "at least one level
    /// change was followed and the next map's own route ran".
    #[arg(long, default_value_t = 2)]
    min_depth: usize,

    /// Timeout for the whole chain run, in seconds.
    #[arg(long, default_value_t = 600)]
    timeout: u64,
}

/// The most routes one chain may hold, so a stray file cannot make the
/// walk unbounded.
pub const MAX_CHAIN_ROUTES: usize = 16;

/// Why a chain could not be assembled from `xtask/chain-routes/`.
#[derive(Debug, PartialEq, Eq)]
pub enum AssemblyError {
    /// No `<start>.txt` exists, so the chain has no first route.
    NoStartRoute,
    /// The start map is not a name from `ohl_campaign`'s cited table.
    StartNotInCampaignTable,
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoStartRoute => "no route file exists for the chain's start map",
            Self::StartNotInCampaignTable => {
                "the chain's start map is not a name from ohl_campaign's cited table"
            }
        })
    }
}

/// Whether `map` is a name this repository may write down: one of
/// `ohl_campaign`'s own publicly sourced chapter or hazard-course map
/// names (`docs/CLEAN_ROOM.md` rule 7).
#[must_use]
pub fn is_campaign_table_name(map: &str) -> bool {
    ohl_campaign::chapter_of(map).is_some()
        || ohl_campaign::chapters::HAZARD_COURSE_MAPS
            .iter()
            .any(|name| name.eq_ignore_ascii_case(map))
}

/// Assembles the chain of route files for `start`, in chain order:
/// `<start>.txt`, then `<start>-hop1.txt`, `-hop2.txt`, ... stopping at
/// the first hop with no file (or at [`MAX_CHAIN_ROUTES`]).
///
/// A gap ends the chain rather than being skipped: hop `n`'s route only
/// means anything if hop `n - 1`'s route actually delivered the player
/// there.
///
/// # Errors
/// [`AssemblyError::StartNotInCampaignTable`] when `start` is not a name
/// from `ohl_campaign`'s cited table, and [`AssemblyError::NoStartRoute`]
/// when the directory holds no route for it.
pub fn assemble_chain(routes_dir: &Path, start: &str) -> Result<Vec<PathBuf>, AssemblyError> {
    if !is_campaign_table_name(start) {
        return Err(AssemblyError::StartNotInCampaignTable);
    }
    let first = routes_dir.join(format!("{start}.txt"));
    if !first.is_file() {
        return Err(AssemblyError::NoStartRoute);
    }
    let mut routes = vec![first];
    for hop in 1..MAX_CHAIN_ROUTES {
        let candidate = routes_dir.join(format!("{start}-hop{hop}.txt"));
        if !candidate.is_file() {
            break;
        }
        routes.push(candidate);
    }
    Ok(routes)
}

/// The fixed line `run_chained` logs when a route's level change landed
/// the walk back in a map it had already entered. Always a failure here,
/// whatever depth was reached: a chain that may revisit maps could satisfy
/// any `--min-depth` by ping-ponging across a single boundary.
pub const RE_ENTERED_LINE: &str = "The chain walk re-entered a map it had already visited.";

/// The three fixed terminal lines `crates/ohl-app/src/game_run.rs`'s
/// `run_chained` ends a chain walk with, in the order this module looks
/// for them.
const TERMINAL_LINES: [&str; 3] = [
    RE_ENTERED_LINE,
    "The chain walk stopped.",
    "The chain walk has no further route.",
];

/// The fixed prefixes `run_chained`'s two aggregate report lines carry.
const DEPTH_PREFIX: &str = "Chain walk depth: ";
const SECONDS_PREFIX: &str = "Chain walk simulated seconds: ";

/// What one chain run reported, parsed from the app's own fixed lines.
#[derive(Debug, PartialEq)]
pub struct ChainReport {
    /// How many maps the chain entered, the start map included.
    pub depth: usize,
    /// How many simulated seconds the routes that ran took.
    pub seconds: f32,
    /// Which fixed terminal line ended the walk, verbatim, or `None` when
    /// the run ended without logging one at all (a crash, a timeout, or a
    /// load failure).
    pub stopped_at: Option<&'static str>,
    /// Whether the walk ended by re-entering a map it had already
    /// visited, which fails this command regardless of depth.
    pub re_entered: bool,
    /// How many "A level change was followed." lines the run logged: one
    /// per hop, a cross-check on `depth`.
    pub hops: usize,
}

/// Parses a finished chain run's stderr into a [`ChainReport`], reading
/// only the app's own fixed lines and keeping no other text.
#[must_use]
pub fn parse_report(stderr: &str) -> ChainReport {
    let value_after = |prefix: &str| -> Option<String> {
        stderr.lines().rev().find_map(|line| {
            let index = line.find(prefix)?;
            Some(
                line[index + prefix.len()..]
                    .trim_end_matches('.')
                    .to_string(),
            )
        })
    };
    ChainReport {
        depth: value_after(DEPTH_PREFIX)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        seconds: value_after(SECONDS_PREFIX)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0),
        stopped_at: TERMINAL_LINES
            .into_iter()
            .find(|line| stderr.contains(line)),
        re_entered: stderr.contains(RE_ENTERED_LINE),
        hops: stderr
            .lines()
            .filter(|line| line.contains("A level change was followed."))
            .count(),
    }
}

/// Renders the aggregate report. Never prints a route file's name or
/// contents, a payload path, or any map name beyond the caller's own
/// `ohl_campaign`-table start name.
#[must_use]
pub fn write_summary(
    start: &str,
    routes: usize,
    report: &ChainReport,
    min_depth: usize,
    elapsed: Duration,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Chain walk summary\n\n");
    let _ = writeln!(out, "Start map: {start} (ohl_campaign's cited table)");
    let _ = writeln!(out, "Wall-clock elapsed: {:.1}s\n", elapsed.as_secs_f64());
    out.push_str("| Measure | Value |\n|---|---|\n");
    let _ = writeln!(out, "| Routes assembled | {routes} |");
    let _ = writeln!(
        out,
        "| Distinct maps reached (chain depth) | {} |",
        report.depth
    );
    let _ = writeln!(out, "| Level changes followed | {} |", report.hops);
    let _ = writeln!(out, "| Elapsed game seconds | {:.1} |", report.seconds);
    let _ = writeln!(
        out,
        "| Stopped at | {} |",
        report.stopped_at.unwrap_or("(no terminal line was logged)")
    );
    let _ = writeln!(out, "| Required depth | {min_depth} |");
    let _ = writeln!(
        out,
        "| Result | {} |",
        if passed(report, min_depth) {
            "Pass"
        } else {
            "Fail"
        }
    );
    out
}

/// Whether a chain run counts as a pass: it entered at least `min_depth`
/// *distinct* maps and never re-entered one it had already been in.
#[must_use]
pub fn passed(report: &ChainReport, min_depth: usize) -> bool {
    report.depth >= min_depth && !report.re_entered
}

pub const APP_BIN_NAME: &str = "open-half-life";

/// Builds the release `open-half-life` binary and returns its path.
pub fn build_release_binary(root: &Path) -> Result<PathBuf, &'static str> {
    let status = Command::new("cargo")
        .args(["build", "-p", "ohl-app", "--release"])
        .current_dir(root)
        .status()
        .map_err(|_| "cargo build -p ohl-app --release failed")?;
    if !status.success() {
        return Err("cargo build -p ohl-app --release failed");
    }
    let name = if cfg!(windows) {
        format!("{APP_BIN_NAME}.exe")
    } else {
        APP_BIN_NAME.to_string()
    };
    Ok(root.join("target").join("release").join(name))
}

/// Runs the chain once, with a deadline, and returns the run's stderr.
fn run_chain(
    bin: &Path,
    payload_root: &Path,
    start: &str,
    routes: &[PathBuf],
    timeout: Duration,
) -> String {
    let mut command = Command::new(bin);
    command
        .arg("--payload-root")
        .arg(payload_root)
        .arg("--map")
        .arg(start);
    for route in routes {
        command.arg("--chain-script").arg(route);
    }
    command.arg("--script-log");
    capture_stderr(command, timeout)
}

/// Runs `command` with a deadline and returns whatever it wrote to
/// stderr, killing it if the deadline passes. Shared with
/// `crate::plan_chain_hop`, which drives the same binary with a different
/// argument list and reads the same kind of fixed report lines from it.
pub fn capture_stderr(mut command: Command, timeout: Duration) -> String {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return String::new();
    };
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let reader = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            // Exited, or could not be waited on at all: either way there
            // is nothing left to wait for.
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    reader.join().unwrap_or_default()
}

/// Entry point for `cargo xtask chain-walk`, given the arguments after the
/// subcommand name.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("chain-walk".to_string()).chain(raw_args.iter().cloned()),
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

    println!(
        "Walking a chain of {} route(s) from the campaign start map...",
        routes.len()
    );
    let started = Instant::now();
    let stderr = run_chain(
        &bin,
        &args.payload_root,
        &start,
        &routes,
        Duration::from_secs(args.timeout),
    );
    let elapsed = started.elapsed();
    let report = parse_report(&stderr);
    print!(
        "{}",
        write_summary(&start, routes.len(), &report, args.min_depth, elapsed)
    );

    if passed(&report, args.min_depth) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(directory: &Path, name: &str) {
        std::fs::write(directory.join(name), "1 wait\n").expect("write a route file");
    }

    #[test]
    fn assembles_the_start_route_and_every_consecutive_hop() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path();
        touch(path, "c0a0.txt");
        touch(path, "c0a0-hop1.txt");
        touch(path, "c0a0-hop2.txt");

        let routes = assemble_chain(path, "c0a0").expect("the chain assembles");
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0], path.join("c0a0.txt"));
        assert_eq!(routes[2], path.join("c0a0-hop2.txt"));
    }

    #[test]
    fn a_gap_ends_the_chain_rather_than_being_skipped() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path();
        touch(path, "c0a0.txt");
        touch(path, "c0a0-hop1.txt");
        // No `-hop2.txt`: hop 3's route describes a map hop 2 never
        // delivered the player to, so it must not be picked up.
        touch(path, "c0a0-hop3.txt");

        let routes = assemble_chain(path, "c0a0").expect("the chain assembles");
        assert_eq!(routes.len(), 2);
    }

    #[test]
    fn a_missing_start_route_is_an_error() {
        let directory = tempfile::tempdir().expect("temporary directory");
        touch(directory.path(), "c0a0-hop1.txt");
        assert_eq!(
            assemble_chain(directory.path(), "c0a0"),
            Err(AssemblyError::NoStartRoute)
        );
    }

    #[test]
    fn a_start_map_outside_the_cited_table_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        touch(directory.path(), "not_a_real_map.txt");
        assert_eq!(
            assemble_chain(directory.path(), "not_a_real_map"),
            Err(AssemblyError::StartNotInCampaignTable)
        );
        assert!(is_campaign_table_name(ohl_campaign::STARTMAP));
        assert!(is_campaign_table_name(ohl_campaign::TRAINMAP));
    }

    #[test]
    fn the_shipped_chain_assembles_and_reaches_at_least_two_maps_worth_of_routes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let routes_dir = root.join("xtask").join("chain-routes");
        let routes =
            assemble_chain(&routes_dir, ohl_campaign::STARTMAP).expect("the shipped chain exists");
        assert!(
            routes.len() >= 2,
            "the shipped chain must be able to reach a second map"
        );
        for route in &routes {
            let bytes = std::fs::read(route).expect("a shipped route file is readable");
            assert!(
                !bytes.is_empty(),
                "a shipped route file must schedule some ticks"
            );
        }
    }

    #[test]
    fn parses_the_apps_fixed_report_lines() {
        let stderr = "[info] Scripted input loaded.\n\
             [info] A level change was followed.\n\
             [info] Scripted input finished.\n\
             [info] The chain walk stopped.\n\
             [info] Chain walk depth: 2.\n\
             [info] Chain walk simulated seconds: 61.5.\n";
        let report = parse_report(stderr);
        assert_eq!(report.depth, 2);
        assert_eq!(report.hops, 1);
        assert!((report.seconds - 61.5).abs() < f32::EPSILON);
        assert_eq!(report.stopped_at, Some("The chain walk stopped."));
        assert!(!report.re_entered);
    }

    #[test]
    fn a_run_that_logged_nothing_parses_as_depth_zero() {
        let report = parse_report("");
        assert_eq!(report.depth, 0);
        assert_eq!(report.hops, 0);
        assert_eq!(report.stopped_at, None);
    }

    #[test]
    fn the_summary_reports_aggregates_only() {
        let report = ChainReport {
            depth: 2,
            seconds: 61.5,
            stopped_at: Some("The chain walk stopped."),
            re_entered: false,
            hops: 1,
        };
        let summary = write_summary("c0a0", 2, &report, 2, Duration::from_secs(9));
        assert!(summary.contains("| Distinct maps reached (chain depth) | 2 |"));
        assert!(summary.contains("| Elapsed game seconds | 61.5 |"));
        assert!(summary.contains("| Stopped at | The chain walk stopped. |"));
        assert!(summary.contains("| Result | Pass |"));
        // No route file name, no payload path, no destination map name.
        assert!(!summary.contains(".txt"));
        assert!(!summary.contains('/'));
        assert!(!summary.contains("hop1"));
    }

    #[test]
    fn the_summary_fails_below_the_required_depth() {
        let report = ChainReport {
            depth: 1,
            seconds: 3.0,
            stopped_at: Some("The chain walk stopped."),
            re_entered: false,
            hops: 0,
        };
        let summary = write_summary("c0a0", 2, &report, 2, Duration::from_secs(1));
        assert!(summary.contains("| Result | Fail |"));
    }

    #[test]
    fn a_re_entry_fails_however_deep_the_walk_got() {
        // The exact shape a ping-pong across one boundary produces: the
        // depth requirement is met, but a map repeated, so the walk made
        // no real progress and this command must not call it a pass.
        let report = ChainReport {
            depth: 2,
            seconds: 42.1,
            stopped_at: Some(RE_ENTERED_LINE),
            re_entered: true,
            hops: 2,
        };
        assert!(!passed(&report, 2));
        let summary = write_summary("c0a0", 2, &report, 2, Duration::from_secs(3));
        assert!(summary.contains("| Result | Fail |"));
        assert!(summary.contains(RE_ENTERED_LINE));
    }

    #[test]
    fn the_re_entry_line_is_recognised_when_parsing() {
        let stderr = format!(
            "[info] A level change was followed.\n\
             [info] A level change was followed.\n\
             [info] {RE_ENTERED_LINE}\n\
             [info] Chain walk depth: 2.\n\
             [info] Chain walk simulated seconds: 42.1.\n"
        );
        let report = parse_report(&stderr);
        assert!(report.re_entered);
        assert_eq!(report.stopped_at, Some(RE_ENTERED_LINE));
        // Two hops, but only two distinct maps: the aggregate the app
        // reports is the distinct one, and the hop count exposes the gap.
        assert_eq!(report.hops, 2);
        assert_eq!(report.depth, 2);
        assert!(!passed(&report, 2));
    }
}
