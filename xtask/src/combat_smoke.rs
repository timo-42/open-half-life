//! `cargo xtask combat-smoke`: the headless scripted-input smoke.
//!
//! Runs the built `open-half-life` binary once per scripted scenario under
//! `assets/smoke-scenarios/*.txt` (see `docs/m79-design.md` §7-8, package
//! P4a), against an already-imported payload tree, with `--script-log`.
//! Each run's stderr is checked against the exact fixed milestone lines
//! documented there, and classified pass/fail. No screenshot is taken and
//! no GPU is required: a scripted run with no `--headless-screenshot`
//! ticks the simulation headlessly (`crates/ohl-app/src/game_run.rs`).
//!
//! Classification and reporting shapes are shared with
//! `campaign_smoke.rs` (`Category`, `sanitize_error_code`); this command's
//! own summary reports scenario names (project-authored, from
//! `assets/smoke-scenarios/`) and pass/fail buckets only. Logging policy
//! is the same as everywhere else in this project: no media-derived
//! string, count or size ever reaches the summary.

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
/// `assets/smoke-scenarios/`, and the map it runs over (a literal from
/// `ohl_campaign`'s own sourced table; see that crate's module
/// documentation for the citations).
struct Scenario {
    name: &'static str,
    file: &'static str,
    map: &'static str,
}

/// The scenarios this command runs, in order. Map names come only from
/// `ohl_campaign`'s cited table: `ohl_campaign::TRAINMAP` for the training
/// start, `ohl_campaign::STARTMAP` for the first chapter's start, and
/// `"c1a1"` (Unforeseen Consequences, `ohl_campaign::CHAPTERS`'s second
/// chapter's first map) for the first monster encounter.
fn scenarios() -> [Scenario; 3] {
    [
        Scenario {
            name: "walk forward in the training start",
            file: "training_start.txt",
            map: ohl_campaign::TRAINMAP,
        },
        Scenario {
            name: "look around in the first chapter start",
            file: "first_chapter_start.txt",
            map: ohl_campaign::STARTMAP,
        },
        Scenario {
            name: "approach the first monster encounter",
            file: "approach_first_monster.txt",
            map: "c1a1",
        },
    ]
}

/// The eight fixed milestone lines `docs/m79-design.md` §7 documents.
/// Every scenario in this tree expects exactly the same two present and
/// six absent, because none of the six needs (weapon firing, hitscan,
/// pickups, player damage) exists on this branch yet — M7.9 P1 (PR #70)
/// is not merged; see `crates/ohl-app/src/script_log.rs`.
const ALWAYS_PRESENT: [&str; 2] = ["Scripted input loaded.", "Scripted input finished."];
const ALWAYS_ABSENT: [&str; 6] = [
    "The player fired a weapon.",
    "A shot hit an entity.",
    "A monster took damage.",
    "A monster died.",
    "A pickup was collected.",
    "The player took damage.",
];

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

/// Runs the app once for `scenario`, with a `timeout` deadline.
fn run_one(
    bin: &Path,
    payload_root: &Path,
    scenarios_dir: &Path,
    scenario: &Scenario,
    timeout: Duration,
) -> ScenarioReport {
    let script_path = scenarios_dir.join(scenario.file);

    let Ok(mut child) = Command::new(bin)
        .arg("--payload-root")
        .arg(payload_root)
        .arg("--map")
        .arg(scenario.map)
        .arg("--script")
        .arg(&script_path)
        .arg("--script-log")
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

    let lines_as_expected = ALWAYS_PRESENT.iter().all(|line| stderr_text.contains(line))
        && ALWAYS_ABSENT.iter().all(|line| !stderr_text.contains(line));

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

    let scenarios_dir = root.join("assets").join("smoke-scenarios");
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
    fn every_scenario_file_is_present_under_assets_smoke_scenarios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let scenarios_dir = root.join("assets").join("smoke-scenarios");
        for scenario in scenarios() {
            assert!(
                scenarios_dir.join(scenario.file).is_file(),
                "missing scenario file: {}",
                scenario.file
            );
        }
    }
}
