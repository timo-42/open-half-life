//! `cargo xtask skirmish-smoke`: one headless local skirmish (deathmatch
//! against bots, `ohl_engine::skirmish`) on every deathmatch arena an
//! already-imported payload publishes.
//!
//! Arenas are addressed by number (`--arena N`, counting from 1 in the app's
//! own map-name order) until the app reports it has run out, so this tool
//! never learns, prints or stores a map name (`docs/CLEAN_ROOM.md`). Each run
//! is an idle player who respawns by themself (`--force-respawn`) among
//! `--bots` bots, with no frag or time limit, for `--seconds` of simulated
//! time; it passes when the app exits cleanly, logs its aggregate
//! "Skirmish summary." line, and that line shows at least one death and at
//! least one bot with a frag — bots that navigate, find someone and win a
//! fight on that arena without any help. The summary reports only those
//! aggregate counts per arena number.

use std::fmt::{self, Write as _};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;

/// `cargo xtask skirmish-smoke` command line.
#[derive(Debug, Parser)]
#[command(name = "skirmish-smoke")]
struct Args {
    /// An already-imported payload store root (the directory holding a
    /// published tree's `files/` directory, or one level above it).
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// Directory the summary (and the idle script every run plays) is
    /// written under.
    #[arg(long, value_name = "DIR", default_value = "target/skirmish-smoke")]
    out: PathBuf,

    /// A prebuilt `open-half-life` binary. Without one, `cargo build -p
    /// ohl-app --release` is run first and the release binary is used.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// Simulated seconds each arena plays.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=1600))]
    seconds: u32,

    /// Bots per arena.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=15))]
    bots: u8,

    /// Per-arena timeout, in seconds.
    #[arg(long, default_value_t = 300)]
    timeout: u64,

    /// The most arenas visited, so a payload that somehow never runs out
    /// cannot keep this running forever.
    #[arg(long, default_value_t = 64)]
    max_arenas: u16,
}

/// The fixed failure the app reports for an `--arena` past the last one
/// (`ohl-app`'s `game_run::NO_SUCH_ARENA`).
const NO_SUCH_ARENA: &str = "the payload publishes fewer deathmatch maps than --arena asks for";

/// The fixed failure the app reports when the payload publishes no
/// deathmatch map at all.
const NO_ARENA: &str = "the payload publishes no deathmatch map to start a skirmish on";

/// The app's fixed per-run summary line (`ohl-app`'s `skirmish::log_summary`).
const SUMMARY: &str = "Skirmish summary.";

/// The simulation steps per simulated second a `--script` tick count
/// stands for (`ohl-app`'s `CAPTURE_STEP`, 1/60 s).
const TICKS_PER_SECOND: u32 = 60;

/// What one arena's run came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Clean exit, a summary, at least one death and one scoring bot.
    Pass,
    /// It ran, but nobody died or no bot scored.
    NoKills,
    /// The app failed before or while playing (a fixed error line).
    Failed,
    /// It ran past the timeout and was killed.
    Timeout,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "Pass",
            Self::NoKills => "No-kills",
            Self::Failed => "Failed",
            Self::Timeout => "Timeout",
        }
    }
}

/// The aggregate counts one summary line carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    deaths: u64,
    frags: i64,
    scoring_bots: u64,
}

/// One arena's row in the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArenaReport {
    arena: u16,
    outcome: Outcome,
    tally: Tally,
    elapsed: Duration,
}

/// `text` with its terminal colour sequences (`ESC [ ... m`) removed, so a
/// structured log field reads as `name=value`.
fn strip_colour(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
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

/// The counts on the summary line of `stderr`, or `None` without one.
fn parse_summary(stderr: &str) -> Option<Tally> {
    let plain = strip_colour(stderr);
    let line = plain.lines().find(|line| line.contains(SUMMARY))?;
    let mut tally = Tally::default();
    for field in line.split_whitespace() {
        let Some((name, value)) = field.split_once('=') else {
            continue;
        };
        match name {
            "deaths" => tally.deaths = value.parse().ok()?,
            "frags" => tally.frags = value.parse().ok()?,
            "scoring_bots" => tally.scoring_bots = value.parse().ok()?,
            _ => {}
        }
    }
    Some(tally)
}

/// What a finished run came to, from its exit and its summary.
fn classify(success: bool, timed_out: bool, tally: Option<Tally>) -> Outcome {
    if timed_out {
        return Outcome::Timeout;
    }
    match tally {
        Some(tally) if success && tally.deaths > 0 && tally.scoring_bots > 0 => Outcome::Pass,
        Some(_) if success => Outcome::NoKills,
        _ => Outcome::Failed,
    }
}

/// What one invocation of the app produced.
struct Run {
    success: bool,
    timed_out: bool,
    stderr: String,
}

fn run_arena(bin: &Path, args: &Args, script: &Path, arena: u16) -> Option<Run> {
    let mut child = Command::new(bin)
        .arg("--payload-root")
        .arg(&args.payload_root)
        .arg("--skirmish")
        .arg("--arena")
        .arg(arena.to_string())
        .arg("--bots")
        .arg(args.bots.to_string())
        .args(["--force-respawn", "--frag-limit", "0", "--time-limit", "0"])
        .arg("--script")
        .arg(script)
        .arg("--script-log")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stderr = child.stderr.take()?;
    let reader = thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        buffer
    });
    let deadline = Instant::now() + Duration::from_secs(args.timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    Some(Run {
        success: status.is_some_and(|status| status.success()),
        timed_out: status.is_none(),
        stderr: reader.join().unwrap_or_default(),
    })
}

/// The summary table: one row per arena number, aggregate counts only.
fn write_summary(reports: &[ArenaReport], elapsed: Duration) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Skirmish smoke\n");
    let _ = writeln!(
        out,
        "| Arena | Result | Deaths | Frags | Scoring bots | Seconds |"
    );
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
    for report in reports {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {:.1} |",
            report.arena,
            report.outcome.label(),
            report.tally.deaths,
            report.tally.frags,
            report.tally.scoring_bots,
            report.elapsed.as_secs_f64()
        );
    }
    let passed = reports
        .iter()
        .filter(|report| report.outcome == Outcome::Pass)
        .count();
    let _ = writeln!(
        out,
        "\n{passed}/{} arena(s) passed in {:.1}s.",
        reports.len(),
        elapsed.as_secs_f64()
    );
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

/// Entry point for `cargo xtask skirmish-smoke`, given the arguments after
/// the subcommand name.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("skirmish-smoke".to_string()).chain(raw_args.iter().cloned()),
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
    let script = out_dir.join("idle.txt");
    if let Err(error) = std::fs::write(
        &script,
        format!("{} wait\n", args.seconds * TICKS_PER_SECOND),
    ) {
        eprintln!("error: could not write the idle script: {error}");
        return ExitCode::FAILURE;
    }

    println!("Running the skirmish smoke over every published arena...");
    let started = Instant::now();
    let mut reports = Vec::new();
    for arena in 1..=args.max_arenas {
        let arena_started = Instant::now();
        let Some(run) = run_arena(&bin, &args, &script, arena) else {
            eprintln!("error: the app could not be started");
            return ExitCode::FAILURE;
        };
        if !run.success && (run.stderr.contains(NO_SUCH_ARENA) || run.stderr.contains(NO_ARENA)) {
            break;
        }
        let outcome = classify(run.success, run.timed_out, parse_summary(&run.stderr));
        reports.push(ArenaReport {
            arena,
            outcome,
            tally: parse_summary(&run.stderr).unwrap_or_default(),
            elapsed: arena_started.elapsed(),
        });
        println!("arena {arena}: {}", outcome.label());
    }

    let summary = write_summary(&reports, started.elapsed());
    if let Err(error) = std::fs::write(out_dir.join("SUMMARY.md"), &summary) {
        eprintln!("error: could not write the summary: {error}");
        return ExitCode::FAILURE;
    }
    println!("{summary}");
    if reports.is_empty() {
        eprintln!("error: the payload publishes no deathmatch arena");
        return ExitCode::FAILURE;
    }
    if reports.iter().all(|report| report.outcome == Outcome::Pass) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, Tally, classify, parse_summary, strip_colour, write_summary};

    #[test]
    fn colour_sequences_are_stripped() {
        assert_eq!(
            strip_colour("\u{1b}[3mdeaths\u{1b}[0m\u{1b}[2m=\u{1b}[0m4"),
            "deaths=4"
        );
    }

    #[test]
    fn the_summary_line_is_read_field_by_field() {
        let stderr = "[info] Skirmish started.\n\
                      [info] Skirmish summary. \u{1b}[3mcombatants\u{1b}[0m=6 deaths=12 frags=-1 scoring_bots=3 human_frags=0\n";
        assert_eq!(
            parse_summary(stderr),
            Some(Tally {
                deaths: 12,
                frags: -1,
                scoring_bots: 3,
            })
        );
        assert_eq!(parse_summary("[info] Map loaded.\n"), None);
    }

    #[test]
    fn only_a_clean_run_with_a_death_and_a_scoring_bot_passes() {
        let busy = Some(Tally {
            deaths: 3,
            frags: 3,
            scoring_bots: 2,
        });
        let quiet = Some(Tally::default());
        assert_eq!(classify(true, false, busy), Outcome::Pass);
        assert_eq!(classify(true, false, quiet), Outcome::NoKills);
        assert_eq!(classify(false, false, busy), Outcome::Failed);
        assert_eq!(classify(true, false, None), Outcome::Failed);
        assert_eq!(classify(false, true, None), Outcome::Timeout);
    }

    #[test]
    fn the_summary_names_arenas_by_number_only() {
        let summary = write_summary(
            &[super::ArenaReport {
                arena: 3,
                outcome: Outcome::Pass,
                tally: Tally {
                    deaths: 5,
                    frags: 4,
                    scoring_bots: 2,
                },
                elapsed: std::time::Duration::from_secs(12),
            }],
            std::time::Duration::from_secs(12),
        );
        assert!(summary.contains("| 3 | Pass | 5 | 4 | 2 | 12.0 |"));
        assert!(summary.contains("1/1 arena(s) passed"));
    }
}
