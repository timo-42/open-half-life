//! `cargo xtask campaign-smoke`: headless-screenshot every campaign map.
//!
//! Runs the built `open-half-life` binary once per map, over every chapter
//! in `ohl_campaign::CHAPTERS` plus the Hazard Course training maps
//! (`ohl_campaign::chapters::HAZARD_COURSE_MAPS`), against an already-imported payload
//! tree. Each run gets a bounded timeout and results are classified as
//! loaded/rendered, missing-map, load-error, timeout, crash, or (a capture
//! that decoded but did not match the expected size or clear the
//! non-background pixel threshold) blank-capture. A markdown summary is
//! written under `--out`, and the command exits non-zero if any map failed
//! to load.
//!
//! Logging policy: nothing here ever prints a payload file path, a
//! per-map pixel count, or any other media-derived number. The summary
//! reports only chapter-level aggregate counts per category, plus chapter
//! titles and map names (both literal facts drawn from `ohl_campaign`'s
//! sourced table; see that crate's module documentation) and the
//! sanitized fixed log lines the `open-half-life` binary itself already
//! prints (see `crates/ohl-app/src/game_run.rs` and
//! `crates/ohl-app/src/main.rs`, whose own logging policy is uniform: no
//! media-derived string ever reaches a log line). A capture's decoded
//! width/height and non-background pixel fraction are used only to decide
//! its `blank-capture` classification; neither the pixel counts nor the
//! dimensions themselves are ever written to the summary or stdout.
//!
//! Default `--jobs` policy: each worker drives its own offscreen software
//! (lavapipe) renderer, and those renderers self-contend for the same CPU
//! cores when run one-per-hardware-thread. Measured on a 16-thread host,
//! the naive default (one worker per `available_parallelism` thread) timed
//! out 26 of 93 maps that all pass when run serially. [`default_job_count`]
//! instead defaults to a quarter of the host's parallelism, clamped to
//! `[1, 4]`; `--jobs` still overrides it explicitly.

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

    /// Bounded parallelism. Defaults to a quarter of the host's available
    /// parallelism (minimum 1, maximum 4), not the full parallelism count:
    /// each worker drives an offscreen software (lavapipe) renderer, and
    /// running one worker per hardware thread makes those renderers
    /// self-contend for the same CPU cores badly enough to blow past the
    /// per-map `--timeout` (observed: 26/93 maps timed out at 16 workers on
    /// a 16-thread host vs. 0/93 run serially). Pass `--jobs` explicitly to
    /// override.
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
    /// The map loaded and a frame was rendered and captured; the capture
    /// matched the expected size and cleared the non-background pixel
    /// threshold.
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
    /// The app exited successfully, but the capture either did not decode,
    /// did not match the expected capture size, or did not clear the
    /// non-background pixel threshold (a blank or otherwise unusable
    /// frame).
    BlankCapture,
}

impl Category {
    /// Whether this counts as a passing run for the pass/fail counts.
    fn is_pass(&self) -> bool {
        matches!(self, Self::LoadedRendered)
    }

    /// A stable bucket key for aggregate counts, collapsing every
    /// [`Self::LoadError`] reason code into a single `"load-error"` bucket.
    fn bucket(&self) -> &'static str {
        match self {
            Self::LoadedRendered => "loaded",
            Self::MissingMap => "missing-map",
            Self::LoadError(_) => "load-error",
            Self::Timeout => "timeout",
            Self::Crash => "crash",
            Self::BlankCapture => "blank-capture",
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

/// The default `--jobs` worker count for a host reporting `available`
/// hardware threads (`std::thread::available_parallelism`).
///
/// Each worker drives its own offscreen software (lavapipe) renderer;
/// running one worker per hardware thread makes those renderers
/// self-contend for the same CPU cores badly enough to blow past the
/// per-map `--timeout` (measured: 26/93 maps timed out at 16 workers on a
/// 16-thread host, versus 0/93 run serially). Defaulting to a quarter of
/// the host's parallelism, clamped to `[1, 4]`, keeps the default run
/// parallel without that self-contention; `--jobs` still overrides this
/// explicitly.
fn default_job_count(available: usize) -> usize {
    (available / 4).clamp(1, 4)
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

/// One map's full report: only its chapter and classification. Nothing
/// media-derived (no pixel count, no dimension, no elapsed time) is kept
/// past this point; the summary reports chapter-level aggregate counts
/// per category only.
struct MapReport {
    chapter: &'static str,
    category: Category,
}

/// The capture size `crates/ohl-app/src/game_run.rs`'s `CAPTURE_SIZE`
/// writes every headless capture at.
const EXPECTED_CAPTURE_SIZE: (u32, u32) = (1280, 720);

/// The minimum fraction of non-background pixels a capture must clear to
/// count as rendered rather than blank. Chosen well below every real
/// capture observed in manual runs (lowest seen: ~13%), so only a
/// genuinely blank or near-blank frame (an unrendered clear color, a
/// stuck loading screen, or similar) falls under it.
const MIN_NON_BACKGROUND_FRACTION: f64 = 0.05;

/// The smallest number of distinct RGBA colors a capture that fell under
/// [`MIN_NON_BACKGROUND_FRACTION`] must still show before it is trusted as a
/// genuinely rendered (if extremely sparse and dark) frame rather than a
/// blank one.
///
/// Some legitimate maps ship a solid black (or otherwise near-uniform)
/// skybox and are viewed from a spot where only a sliver of lit geometry is
/// actually on screen; every pixel of that sliver still comes from real
/// per-fragment lighting and lightmap sampling, which scatters it across
/// many distinct, slightly-differing shades even though the frame is
/// overwhelmingly one dominant background color. A capture the renderer
/// never actually drew into — a stuck clear color, a frozen loading
/// screen — has no such variation: it is exactly one color, or at most a
/// small fixed handful from a static overlay. Requiring a real spread of
/// distinct colors before accepting a fraction this low keeps the fraction
/// check's original power to catch a truly blank capture while no longer
/// misclassifying a legitimately dark, sparsely lit scene as one.
const MIN_DISTINCT_COLORS_FOR_SPARSE_FRAME: usize = 64;

/// Decodes `path` and reports only whether it passes the capture health
/// checks: matching [`EXPECTED_CAPTURE_SIZE`], and either clearing
/// [`MIN_NON_BACKGROUND_FRACTION`] (treating the most common RGBA color as
/// the background) or, failing that, showing at least
/// [`MIN_DISTINCT_COLORS_FOR_SPARSE_FRAME`] distinct colors (a real, if very
/// sparse and dark, render). Never returns or retains the pixel counts, the
/// computed fraction, the distinct-color count, or the decoded dimensions
/// themselves — only the pass/fail boolean.
fn capture_is_healthy(path: &Path) -> bool {
    let Ok(image) = image::open(path) else {
        return false;
    };
    let rgba = image.to_rgba8();
    if (rgba.width(), rgba.height()) != EXPECTED_CAPTURE_SIZE {
        return false;
    }

    let mut counts: std::collections::HashMap<[u8; 4], u64> = std::collections::HashMap::new();
    for pixel in rgba.pixels() {
        *counts.entry(pixel.0).or_insert(0) += 1;
    }

    let total_pixels = u64::from(rgba.width()) * u64::from(rgba.height());
    if total_pixels == 0 {
        return false;
    }
    let background_count = counts.values().copied().max().unwrap_or(0);
    #[allow(clippy::cast_precision_loss)]
    let non_background_fraction = (total_pixels - background_count) as f64 / total_pixels as f64;
    non_background_fraction >= MIN_NON_BACKGROUND_FRACTION
        || counts.len() >= MIN_DISTINCT_COLORS_FOR_SPARSE_FRAME
}

/// Runs the app once for `job.map`, with a `timeout` deadline, and returns
/// its classification: the process outcome, further downgraded to
/// [`Category::BlankCapture`] when the app reported success but the
/// capture itself failed its health check.
fn run_one(
    bin: &Path,
    payload_root: &Path,
    out_dir: &Path,
    job: Job,
    frames: u32,
    timeout: Duration,
) -> MapReport {
    let png_path = out_dir.join(format!("{}.png", job.map));

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

    let outcome = RunOutcome {
        timed_out: status.is_none() && Instant::now() >= deadline,
        exit_code: status.and_then(|status| status.code()),
        last_error_line,
    };
    let category = classify(&outcome);

    let category = if matches!(category, Category::LoadedRendered) && !capture_is_healthy(&png_path)
    {
        Category::BlankCapture
    } else {
        category
    };

    MapReport {
        chapter: job.chapter,
        category,
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

/// The category buckets shown as summary columns, in a fixed order. Every
/// [`Category::LoadError`] reason code collapses into the single
/// `"load-error"` bucket (see [`Category::bucket`]): the summary reports
/// aggregate counts only, never a per-code or per-map breakdown.
const CATEGORY_BUCKETS: [(&str, &str); 6] = [
    ("loaded", "Loaded"),
    ("missing-map", "Missing-map"),
    ("load-error", "Load-error"),
    ("timeout", "Timeout"),
    ("crash", "Crash"),
    ("blank-capture", "Blank-capture"),
];

/// Counts of each category bucket within one chapter (or the whole run).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BucketCounts([usize; CATEGORY_BUCKETS.len()]);

impl BucketCounts {
    fn tally<'a>(reports: impl IntoIterator<Item = &'a MapReport>) -> Self {
        let mut counts = Self::default();
        for report in reports {
            let bucket = report.category.bucket();
            if let Some(index) = CATEGORY_BUCKETS.iter().position(|(key, _)| *key == bucket) {
                counts.0[index] += 1;
            }
        }
        counts
    }

    fn total(&self) -> usize {
        self.0.iter().sum()
    }

    fn add(&mut self, other: &Self) {
        for (slot, value) in self.0.iter_mut().zip(other.0) {
            *slot += value;
        }
    }
}

/// Writes the markdown summary: chapter-level aggregate counts per
/// category only. Never writes a file path, a per-map row, or any
/// media-derived number (pixel count, dimension, or per-map elapsed time);
/// only chapter titles (drawn from `ohl_campaign`'s sourced table) and
/// aggregate counts appear.
fn write_summary(chapters: &[ChapterSummary<'_>], total_elapsed: Duration) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    out.push_str("# Campaign smoke summary\n\n");
    let _ = writeln!(out, "Total elapsed: {:.1}s\n", total_elapsed.as_secs_f64());

    out.push_str("| Chapter |");
    for (_, label) in CATEGORY_BUCKETS {
        let _ = write!(out, " {label} |");
    }
    out.push_str(" Total |\n");
    out.push_str("|---|");
    for _ in CATEGORY_BUCKETS {
        out.push_str("---|");
    }
    out.push_str("---|\n");

    let mut grand_total = BucketCounts::default();
    for chapter in chapters {
        let counts = BucketCounts::tally(chapter.maps.iter().copied());
        grand_total.add(&counts);
        let _ = write!(out, "| {} |", chapter.title);
        for value in counts.0 {
            let _ = write!(out, " {value} |");
        }
        let _ = writeln!(out, " {} |", counts.total());
    }

    let _ = write!(out, "| **Total** |");
    for value in grand_total.0 {
        let _ = write!(out, " **{value}** |");
    }
    let _ = writeln!(out, " **{}** |", grand_total.total());

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

    let workers = args.jobs.unwrap_or_else(|| {
        default_job_count(std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
    });
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
    fn default_job_count_is_a_quarter_of_parallelism_clamped_to_one_and_four() {
        // Below the point where a quarter is at least 1, the floor wins.
        assert_eq!(default_job_count(0), 1);
        assert_eq!(default_job_count(1), 1);
        assert_eq!(default_job_count(3), 1);
        // A quarter, once it clears 1.
        assert_eq!(default_job_count(4), 1);
        assert_eq!(default_job_count(8), 2);
        assert_eq!(default_job_count(12), 3);
        // The 16-thread host this default was tuned against: 26/93 maps
        // timed out at 16 workers, 0/93 at 4.
        assert_eq!(default_job_count(16), 4);
        // Above the point where a quarter exceeds 4, the ceiling wins.
        assert_eq!(default_job_count(32), 4);
        assert_eq!(default_job_count(128), 4);
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

    fn fake_report(chapter: &'static str, category: Category) -> MapReport {
        MapReport { chapter, category }
    }

    #[test]
    fn summary_reports_chapter_level_aggregate_counts_per_category() {
        let reports = vec![
            fake_report("Black Mesa Inbound", Category::LoadedRendered),
            fake_report("Anomalous Materials", Category::MissingMap),
            fake_report("Anomalous Materials", Category::LoadedRendered),
        ];
        let titles = ["Black Mesa Inbound", "Anomalous Materials"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(3));

        // Loaded | Missing-map | Load-error | Timeout | Crash | Blank-capture | Total
        assert!(summary.contains("| Black Mesa Inbound | 1 | 0 | 0 | 0 | 0 | 0 | 1 |"));
        assert!(summary.contains("| Anomalous Materials | 1 | 1 | 0 | 0 | 0 | 0 | 2 |"));
        assert!(
            summary
                .contains("| **Total** | **2** | **1** | **0** | **0** | **0** | **0** | **3** |")
        );
    }

    #[test]
    fn summary_collapses_every_load_error_reason_code_into_one_bucket() {
        let reports = vec![
            fake_report("Blast Pit", Category::LoadError("no-gpu")),
            fake_report("Blast Pit", Category::LoadError("render-failed")),
        ];
        let titles = ["Blast Pit"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(1));

        assert!(summary.contains("| Blast Pit | 0 | 0 | 2 | 0 | 0 | 0 | 2 |"));
        // Never leaks the reason code itself.
        assert!(!summary.contains("no-gpu"));
        assert!(!summary.contains("render-failed"));
    }

    #[test]
    fn summary_counts_blank_captures_as_their_own_failure_bucket() {
        let reports = vec![fake_report("Xen", Category::BlankCapture)];
        let titles = ["Xen"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(1));

        assert!(summary.contains("| Xen | 0 | 0 | 0 | 0 | 0 | 1 | 1 |"));
    }

    #[test]
    fn summary_never_names_a_payload_file_or_a_pixel_statistic() {
        let reports = vec![fake_report(
            "Black Mesa Inbound",
            Category::LoadError("no-gpu"),
        )];
        let titles = ["Black Mesa Inbound"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(1));

        // No map name, no file name, no path, and no per-map pixel/dimension
        // figure ever appears; only chapter titles and aggregate counts do.
        assert!(!summary.contains("c0a0"));
        assert!(!summary.contains(".png"));
        assert!(!summary.contains('/'));
        assert!(!summary.contains("1280"));
        assert!(!summary.contains('%'));
    }

    #[test]
    fn empty_chapters_are_reported_with_zero_totals() {
        let reports: Vec<MapReport> = Vec::new();
        let titles = ["Interloper"];
        let chapters = group_by_chapter(&reports, &titles);
        let summary = write_summary(&chapters, Duration::from_secs(0));

        assert!(summary.contains("| Interloper | 0 | 0 | 0 | 0 | 0 | 0 | 0 |"));
    }

    #[test]
    fn capture_health_check_rejects_the_wrong_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        let image = image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 20, 30, 255]));
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write a fixture capture");

        assert!(!capture_is_healthy(&path));
    }

    #[test]
    fn capture_health_check_rejects_a_blank_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        let (width, height) = EXPECTED_CAPTURE_SIZE;
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write a fixture capture");

        assert!(!capture_is_healthy(&path));
    }

    /// A map with a legitimately near-uniform (e.g. solid-black skybox)
    /// background can still be a genuine, correctly rendered capture even
    /// though its non-background fraction falls under
    /// [`MIN_NON_BACKGROUND_FRACTION`], as long as the sliver that is on
    /// screen shows the kind of per-pixel lighting variation a real render
    /// produces (many distinct colors), rather than the handful (or one) a
    /// truly blank capture would show. This reproduces the class of bug
    /// behind a real regression: a capture that is 99%+ one dominant color
    /// but contains hundreds of distinct, real, lit shades in the rest must
    /// not be classified the same as a stuck clear-color frame.
    #[test]
    fn capture_health_check_accepts_a_sparse_but_richly_colored_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        let (width, height) = EXPECTED_CAPTURE_SIZE;
        let mut image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
        // Paint a tiny sliver (well under 1% of the frame) with many
        // distinct, slightly different shades, mirroring how real
        // per-fragment lighting scatters color across a small lit area.
        let mut shade = 0u8;
        for x in 0..width.min(200) {
            let pixel = image.get_pixel_mut(x, height - 1);
            shade = shade.wrapping_add(1);
            *pixel = image::Rgba([shade, shade / 2, shade / 3, 255]);
        }

        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write a fixture capture");

        assert!(capture_is_healthy(&path));
    }

    /// The counterpart to the sparse-but-colored case above: a capture that
    /// is almost entirely one dominant color *and* only shows a small fixed
    /// handful of other colors (well under
    /// [`MIN_DISTINCT_COLORS_FOR_SPARSE_FRAME`]) is exactly what a stuck or
    /// never-rendered frame (a static overlay over an unrendered clear
    /// color, say) looks like, and must still be rejected.
    #[test]
    fn capture_health_check_rejects_a_sparse_frame_with_few_distinct_colors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        let (width, height) = EXPECTED_CAPTURE_SIZE;
        let mut image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
        for x in 0..width.min(8) {
            *image.get_pixel_mut(x, height - 1) = image::Rgba([200, 200, 200, 255]);
        }

        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write a fixture capture");

        assert!(!capture_is_healthy(&path));
    }

    #[test]
    fn capture_health_check_accepts_a_varied_frame_at_the_expected_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        let (width, height) = EXPECTED_CAPTURE_SIZE;
        let mut image = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            if (x + y) % 3 == 0 {
                *pixel = image::Rgba([200, 100, 50, 255]);
            }
        }
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .expect("write a fixture capture");

        assert!(capture_is_healthy(&path));
    }

    #[test]
    fn capture_health_check_rejects_an_undecodable_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.png");
        std::fs::write(&path, b"not a png").expect("write a non-image file");

        assert!(!capture_is_healthy(&path));
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
