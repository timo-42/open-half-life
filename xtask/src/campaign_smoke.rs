//! `cargo xtask campaign-smoke`: headless-screenshot every campaign map.
//!
//! Runs the built `open-half-life` binary once per map, over every chapter
//! in `ohl_campaign::CHAPTERS` plus the Hazard Course training maps
//! (`ohl_campaign::chapters::HAZARD_COURSE_MAPS`), against an already-imported payload
//! tree. Each run gets a bounded timeout and results are classified as
//! loaded/rendered, missing-map, load-error, timeout or crash. A markdown
//! summary is written under `--out`, and the command exits non-zero if any
//! map failed to load.
//!
//! Logging policy: nothing here ever prints a payload file path. The only
//! names that appear in the summary are chapter titles and map names, both
//! literal facts drawn from `ohl_campaign`'s sourced table (see that
//! crate's module documentation), and the sanitized fixed log lines the
//! `open-half-life` binary itself already prints (see
//! `crates/ohl-app/src/game_run.rs` and `crates/ohl-app/src/main.rs`,
//! whose own logging policy is uniform: no media-derived string ever
//! reaches a log line).

use std::collections::VecDeque;
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;

/// `cargo xtask campaign-smoke` command line.
#[derive(Debug, Parser)]
#[command(name = "campaign-smoke")]
struct Args {
    /// An already-imported payload store root (the directory holding a
    /// published tree's `files/` directory, or one level above it).
    #[arg(long, value_name = "DIR")]
    payload_root: PathBuf,

    /// Directory the PNG captures and `SUMMARY.md` are written under.
    #[arg(long, value_name = "DIR", default_value = "target/campaign-smoke")]
    out: PathBuf,

    /// Frames each capture advances before it is written.
    #[arg(long, default_value_t = 8)]
    frames: u32,

    /// Bounded parallelism; defaults to the host's available parallelism.
    #[arg(long)]
    jobs: Option<usize>,

    /// A prebuilt `open-half-life` binary. Without one, `cargo build -p
    /// ohl-app --release` is run first and the release binary is used.
    #[arg(long, value_name = "PATH")]
    bin: Option<PathBuf>,

    /// Per-map timeout, in seconds.
    #[arg(long, default_value_t = 120)]
    timeout: u64,
}

/// One map's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Category {
    /// The map loaded and a frame was rendered and captured.
    LoadedRendered,
    /// The map name is not published in this payload.
    MissingMap,
    /// The app exited with a failure and a sanitized reason code.
    LoadError(&'static str),
    /// The run did not finish within the per-map timeout.
    Timeout,
    /// The process ended abnormally (killed by a signal, or could not be
    /// spawned at all).
    Crash,
}

impl Category {
    /// Whether this counts as a passing run for the pass/fail counts.
    fn is_pass(&self) -> bool {
        matches!(self, Self::LoadedRendered)
    }

    fn label(&self) -> String {
        match self {
            Self::LoadedRendered => "loaded".to_string(),
            Self::MissingMap => "missing-map".to_string(),
            Self::LoadError(code) => format!("load-error ({code})"),
            Self::Timeout => "timeout".to_string(),
            Self::Crash => "crash".to_string(),
        }
    }
}

/// The fixed message `crates/ohl-app/src/game_run.rs::run` returns when the
/// requested map is not published in the payload.
const MISSING_MAP_MESSAGE: &str = "the start map could not be loaded from the payload";

/// Known fixed failure messages the app logs (see `crates/ohl-app/src/
/// main.rs` and `crates/ohl-app/src/game_run.rs`), mapped to a short,
/// stable, sanitized code. Anything not on this list still gets a code
/// (`"unspecified"`), never the raw message.
const KNOWN_ERROR_CODES: &[(&str, &str)] = &[
    (MISSING_MAP_MESSAGE, "missing-map"),
    (
        "the payload directory could not be indexed",
        "payload-index",
    ),
    ("no usable graphics adapter is available", "no-gpu"),
    (
        "no offscreen target could be created",
        "no-offscreen-target",
    ),
    ("the frame could not be rendered", "render-failed"),
    ("the frame could not be read back", "readback-failed"),
    ("the capture could not be written", "capture-write-failed"),
    (
        "No imported payload was found. Import one first by passing --iso PATH.",
        "no-payload",
    ),
    (
        "Payload location failed: no per-user data directory is available",
        "no-data-dir",
    ),
    ("Media cache preparation failed", "cache-failed"),
    ("Media preflight failed", "preflight-failed"),
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

/// The outcome of running the app once for one map, stripped of anything
/// that could carry a payload path: only the exit status shape and the
/// last `[error]`-prefixed fixed log line, if any.
struct RunOutcome {
    timed_out: bool,
    exit_code: Option<i32>,
    last_error_line: Option<String>,
}

/// Classifies a run purely from its outcome shape, never from raw output
/// beyond the one already-sanitized fixed line the app printed.
fn classify(outcome: &RunOutcome) -> Category {
    if outcome.timed_out {
        return Category::Timeout;
    }
    match outcome.exit_code {
        Some(0) => Category::LoadedRendered,
        Some(_failure) => {
            let line = outcome.last_error_line.as_deref().unwrap_or("");
            if line.contains(MISSING_MAP_MESSAGE) {
                Category::MissingMap
            } else {
                Category::LoadError(sanitize_error_code(line))
            }
        }
        None => Category::Crash,
    }
}

/// One job: a map to capture, tagged with the chapter it belongs to.
#[derive(Debug, Clone, Copy)]
struct Job {
    chapter: &'static str,
    map: &'static str,
}

/// The name given to the synthetic chapter grouping the Hazard Course
/// training maps, which are not part of `ohl_campaign::CHAPTERS` (per that
/// crate's own documentation: `trainmap` is selected independently of the
/// main chapter count).
const TRAINING_CHAPTER: &str = "Hazard Course (training)";

/// Builds the full job list: every chapter's maps, in campaign order, plus
/// the training maps as a trailing synthetic chapter.
fn all_jobs() -> Vec<Job> {
    let mut jobs = Vec::new();
    for chapter in ohl_campaign::CHAPTERS {
        for map in chapter.maps {
            jobs.push(Job {
                chapter: chapter.title,
                map,
            });
        }
    }
    for map in ohl_campaign::chapters::HAZARD_COURSE_MAPS {
        jobs.push(Job {
            chapter: TRAINING_CHAPTER,
            map,
        });
    }
    jobs
}

/// The ordered list of chapter titles a summary should report a row for,
/// including chapters with zero maps (for example Interloper, whose
/// starting map is deliberately left unconfirmed) and the synthetic
/// training chapter.
fn all_chapter_titles() -> Vec<&'static str> {
    let mut titles: Vec<&'static str> = ohl_campaign::CHAPTERS
        .iter()
        .map(|chapter| chapter.title)
        .collect();
    titles.push(TRAINING_CHAPTER);
    titles
}

/// One map's full report: its classification, timing, and (only when it
/// rendered) the capture's pixel statistics.
struct MapReport {
    chapter: &'static str,
    map: &'static str,
    category: Category,
    elapsed: Duration,
    image_stats: Option<Result<ImageStats, String>>,
}

/// Pixel-level statistics computed from a capture PNG with the `image`
/// crate. Contains no file path.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ImageStats {
    width: u32,
    height: u32,
    distinct_colors: usize,
    percent_non_background: f64,
}

/// Decodes `path` and computes its [`ImageStats`], treating the most
/// common RGBA color as the background.
fn analyze_capture(path: &Path) -> Result<ImageStats, String> {
    let image = image::open(path).map_err(|_| "the capture could not be decoded".to_string())?;
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());

    let mut counts: std::collections::HashMap<[u8; 4], u64> = std::collections::HashMap::new();
    for pixel in rgba.pixels() {
        *counts.entry(pixel.0).or_insert(0) += 1;
    }

    let total_pixels = u64::from(width) * u64::from(height);
    let background_count = counts.values().copied().max().unwrap_or(0);
    #[allow(clippy::cast_precision_loss)]
    let percent_non_background = if total_pixels == 0 {
        0.0
    } else {
        100.0 * (total_pixels - background_count) as f64 / total_pixels as f64
    };

    Ok(ImageStats {
        width,
        height,
        distinct_colors: counts.len(),
        percent_non_background,
    })
}

/// Runs the app once for `job.map`, with a `timeout` deadline, and returns
/// its classification, elapsed time and (when it rendered) the capture's
/// image statistics.
fn run_one(
    bin: &Path,
    payload_root: &Path,
    out_dir: &Path,
    job: Job,
    frames: u32,
    timeout: Duration,
) -> MapReport {
    let png_path = out_dir.join(format!("{}.png", job.map));
    let start = Instant::now();

    let Ok(mut child) = Command::new(bin)
        .arg("--payload-root")
        .arg(payload_root)
        .arg("--map")
        .arg(job.map)
        .arg("--headless-screenshot")
        .arg(&png_path)
        .arg("--frames")
        .arg(frames.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return MapReport {
            chapter: job.chapter,
            map: job.map,
            category: Category::Crash,
            elapsed: start.elapsed(),
            image_stats: None,
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
    let elapsed = start.elapsed();
    let stderr_text = stderr_reader.join().unwrap_or_default();

    let last_error_line = stderr_text
        .lines()
        .rfind(|line| line.starts_with("[error]"))
        .map(ToString::to_string);

    let outcome = RunOutcome {
        timed_out: status.is_none() && Instant::now() >= deadline,
        exit_code: status.and_then(|status| status.code()),
        last_error_line,
    };
    let category = classify(&outcome);

    let image_stats =
        matches!(category, Category::LoadedRendered).then(|| analyze_capture(&png_path));

    MapReport {
        chapter: job.chapter,
        map: job.map,
        category,
        elapsed,
        image_stats,
    }
}

/// Runs every job with bounded parallelism across `workers` threads.
fn run_all(
    bin: &Path,
    payload_root: &Path,
    out_dir: &Path,
    jobs: Vec<Job>,
    frames: u32,
    timeout: Duration,
    workers: usize,
) -> Vec<MapReport> {
    let queue = Arc::new(Mutex::new(jobs.into_iter().collect::<VecDeque<_>>()));
    let results = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for _ in 0..workers.max(1) {
        let queue = Arc::clone(&queue);
        let results = Arc::clone(&results);
        let bin = bin.to_path_buf();
        let payload_root = payload_root.to_path_buf();
        let out_dir = out_dir.to_path_buf();
        handles.push(thread::spawn(move || {
            loop {
                let job = { queue.lock().expect("queue lock").pop_front() };
                let Some(job) = job else { break };
                let report = run_one(&bin, &payload_root, &out_dir, job, frames, timeout);
                results.lock().expect("results lock").push(report);
            }
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }

    Arc::try_unwrap(results)
        .unwrap_or_else(|arc| Mutex::new(arc.lock().expect("results lock").drain(..).collect()))
        .into_inner()
        .expect("results mutex")
}

/// One chapter's rows, in the order they should be summarized.
struct ChapterSummary<'a> {
    title: &'static str,
    maps: Vec<&'a MapReport>,
}

/// Groups reports by chapter, preserving `all_chapter_titles`'s order, and
/// keeping each chapter's maps in the order jobs were built (campaign
/// order).
fn group_by_chapter<'a>(
    reports: &'a [MapReport],
    titles: &[&'static str],
) -> Vec<ChapterSummary<'a>> {
    titles
        .iter()
        .map(|&title| ChapterSummary {
            title,
            maps: reports
                .iter()
                .filter(|report| report.chapter == title)
                .collect(),
        })
        .collect()
}

/// Writes the markdown summary: an overview table of per-chapter pass/fail
/// counts, then one section per chapter with each map's classification and
/// (when rendered) its capture's pixel statistics. Never writes a file
/// path; only chapter titles and map names, both drawn from
/// `ohl_campaign`'s sourced table.
fn write_summary(chapters: &[ChapterSummary<'_>], total_elapsed: Duration) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Campaign smoke summary\n\n");
    let _ = writeln!(out, "Total elapsed: {:.1}s\n", total_elapsed.as_secs_f64());

    out.push_str("| Chapter | Pass | Fail | Total |\n");
    out.push_str("|---|---|---|---|\n");
    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    for chapter in chapters {
        let pass = chapter.maps.iter().filter(|m| m.category.is_pass()).count();
        let fail = chapter.maps.len() - pass;
        total_pass += pass;
        total_fail += fail;
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            chapter.title,
            pass,
            fail,
            chapter.maps.len()
        );
    }
    let _ = writeln!(
        out,
        "| **Total** | **{total_pass}** | **{total_fail}** | **{}** |\n",
        total_pass + total_fail
    );

    for chapter in chapters {
        if chapter.maps.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {}\n", chapter.title);
        out.push_str("| Map | Result | Elapsed | Dimensions | Colours | Non-background % |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for map in &chapter.maps {
            let (dims, colours, percent) = match &map.image_stats {
                Some(Ok(stats)) => (
                    format!("{}x{}", stats.width, stats.height),
                    stats.distinct_colors.to_string(),
                    format!("{:.1}%", stats.percent_non_background),
                ),
                Some(Err(_)) => ("unreadable".to_string(), "-".to_string(), "-".to_string()),
                None => ("-".to_string(), "-".to_string(), "-".to_string()),
            };
            let _ = writeln!(
                out,
                "| {} | {} | {:.1}s | {} | {} | {} |",
                map.map,
                map.category.label(),
                map.elapsed.as_secs_f64(),
                dims,
                colours,
                percent
            );
        }
        out.push('\n');
    }

    out
}

/// A build failure, reported without expanding into a raw `cargo` error
/// dump (this command's own logging policy: no payload path, but a build
/// failure is not media-derived, so it is reported plainly).
#[derive(Debug)]
struct BuildFailed;

impl fmt::Display for BuildFailed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cargo build -p ohl-app --release failed")
    }
}

/// The binary name `cargo build -p ohl-app --release` produces (see
/// `crates/ohl-app/Cargo.toml`'s `[[bin]] name = "open-half-life"`).
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

/// Entry point for `cargo xtask campaign-smoke`, given the arguments after
/// the subcommand name.
pub fn run(root: &Path, raw_args: &[String]) -> ExitCode {
    let args = match Args::try_parse_from(
        std::iter::once("campaign-smoke".to_string()).chain(raw_args.iter().cloned()),
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

    let workers = args
        .jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    let timeout = Duration::from_secs(args.timeout);

    let jobs = all_jobs();
    let job_count = jobs.len();
    println!("Running the campaign smoke over {job_count} map(s), {workers} worker(s) wide...");

    let started = Instant::now();
    let reports = run_all(
        &bin,
        &args.payload_root,
        &out_dir,
        jobs,
        args.frames,
        timeout,
        workers,
    );
    let total_elapsed = started.elapsed();

    let titles = all_chapter_titles();
    let chapters = group_by_chapter(&reports, &titles);
    let summary = write_summary(&chapters, total_elapsed);

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
    ) -> RunOutcome {
        RunOutcome {
            timed_out,
            exit_code,
            last_error_line: last_error_line.map(ToString::to_string),
        }
    }

    #[test]
    fn classifies_success_as_loaded_rendered() {
        assert_eq!(
            classify(&outcome(false, Some(0), None)),
            Category::LoadedRendered
        );
    }

    #[test]
    fn classifies_missing_map_from_the_fixed_message() {
        let result = classify(&outcome(
            false,
            Some(1),
            Some(&format!("[error] {MISSING_MAP_MESSAGE}")),
        ));
        assert_eq!(result, Category::MissingMap);
    }

    #[test]
    fn classifies_other_failures_as_sanitized_load_errors() {
        let result = classify(&outcome(
            false,
            Some(1),
            Some("[error] no usable graphics adapter is available"),
        ));
        assert_eq!(result, Category::LoadError("no-gpu"));
    }

    #[test]
    fn classifies_unknown_failure_message_without_leaking_it() {
        let result = classify(&outcome(
            false,
            Some(1),
            Some("[error] some unexpected raw message with a path in it"),
        ));
        assert_eq!(result, Category::LoadError("unspecified"));
    }

    #[test]
    fn classifies_missing_error_line_as_unspecified_load_error() {
        assert_eq!(
            classify(&outcome(false, Some(1), None)),
            Category::LoadError("unspecified")
        );
    }

    #[test]
    fn classifies_timeout_before_checking_the_exit_code() {
        assert_eq!(classify(&outcome(true, Some(0), None)), Category::Timeout);
    }

    #[test]
    fn classifies_signal_kill_as_crash() {
        assert_eq!(classify(&outcome(false, None, None)), Category::Crash);
    }

    fn fake_report(chapter: &'static str, map: &'static str, category: Category) -> MapReport {
        let image_stats = matches!(category, Category::LoadedRendered).then(|| {
            Ok(ImageStats {
                width: 1280,
                height: 720,
                distinct_colors: 4096,
                percent_non_background: 42.5,
            })
        });
        MapReport {
            chapter,
            map,
            category,
            elapsed: Duration::from_millis(250),
            image_stats,
        }
    }

    #[test]
    fn summary_reports_per_chapter_pass_fail_counts() {
        let reports = vec![
            fake_report("Black Mesa Inbound", "c0a0", Category::LoadedRendered),
            fake_report("Anomalous Materials", "c1a0", Category::MissingMap),
            fake_report("Anomalous Materials", "c1a0b", Category::LoadedRendered),
        ];
        let titles = ["Black Mesa Inbound", "Anomalous Materials"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(3));

        assert!(summary.contains("| Black Mesa Inbound | 1 | 0 | 1 |"));
        assert!(summary.contains("| Anomalous Materials | 1 | 1 | 2 |"));
        assert!(summary.contains("| **Total** | **2** | **1** | **3** |"));
    }

    #[test]
    fn summary_includes_capture_pixel_statistics_for_rendered_maps() {
        let reports = vec![fake_report(
            "Black Mesa Inbound",
            "c0a0",
            Category::LoadedRendered,
        )];
        let titles = ["Black Mesa Inbound"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(1));

        assert!(summary.contains("1280x720"));
        assert!(summary.contains("4096"));
        assert!(summary.contains("42.5%"));
    }

    #[test]
    fn summary_never_names_a_payload_file_beyond_the_map_name() {
        let reports = vec![fake_report(
            "Black Mesa Inbound",
            "c0a0",
            Category::LoadError("no-gpu"),
        )];
        let titles = ["Black Mesa Inbound"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(1));

        assert!(summary.contains("c0a0"));
        assert!(!summary.contains(".png"));
        assert!(!summary.contains('/'));
    }

    #[test]
    fn empty_chapters_are_reported_with_zero_totals_but_no_map_section() {
        let reports: Vec<MapReport> = Vec::new();
        let titles = ["Interloper"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(0));

        assert!(summary.contains("| Interloper | 0 | 0 | 0 |"));
        assert!(!summary.contains("## Interloper"));
    }

    #[test]
    fn all_jobs_covers_every_chapter_map_and_the_training_maps() {
        let jobs = all_jobs();
        let expected_chapter_maps: usize = ohl_campaign::CHAPTERS
            .iter()
            .map(|chapter| chapter.maps.len())
            .sum();
        assert_eq!(
            jobs.len(),
            expected_chapter_maps + ohl_campaign::chapters::HAZARD_COURSE_MAPS.len()
        );
        assert!(jobs.iter().any(|job| job.map == ohl_campaign::TRAINMAP));
        assert!(jobs.iter().any(|job| job.chapter == TRAINING_CHAPTER));
    }
}
