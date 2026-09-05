//! `open-half-life`: the Rust composition-root binary.
//!
//! This is the M1-rs milestone: it parses arguments (or prompts on stdin, as
//! the C++ build did), acquires the media path exactly once into a pinned
//! [`ohl_platform::MediaSource`], classifies it with the ISO 9660 preflight
//! and then the UDF preflight, fingerprints and binds a
//! [`ohl_media::ValidatedMedia`] proof, mounts it read-only through
//! [`ohl_vfs::Mount`], and publishes or reuses a metadata-only provenance
//! cache entry. It then composes the R4.7a payload import: it locates one
//! container in the mounted tree, hands a confined parser worker a bounded
//! window over it, and stages whatever the worker enumerates into the
//! payload store. On Linux x86-64 that import is real end to end: the
//! confined worker recognises the container, enumerates it and streams its
//! entries, and this binary publishes the payload tree. See
//! `docs/MEDIA_IMPORT.md` and `docs/MILESTONES.md`.
//!
//! Every failure is logged as a single sanitized line and maps to the same
//! exit codes the C++ `src/app/main.cpp` used: `2` for a command-line usage
//! error, `1` for a media, mount, or cache failure, `0` on success.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia};
use ohl_payload::SelectionRecipe;
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, MediaSourceBlockReader, Mount};

#[cfg(feature = "dev-tools")]
mod dev_bsp;
#[cfg(feature = "dev-tools")]
mod dev_mdl;

/// The integration tests' synthetic fixtures, shared rather than duplicated:
/// a binary crate's unit tests cannot `use` its own `tests/` modules, so they
/// are included by path instead. Compiled only for `cargo test`.
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

const APP_NAME: &str = "Open Half-Life";
const VERSION: &str = env!("OHL_APP_VERSION");

/// Command-line usage error: `--iso` and a positional path both given, or
/// neither given and stdin had nothing to offer.
const EXIT_USAGE: u8 = 2;
/// A media, mount, or cache failure.
const EXIT_FAILURE: u8 = 1;

/// `Open Half-Life <version>` command line.
#[derive(Debug, Parser)]
#[command(name = "Open Half-Life", version = VERSION, about = None, long_about = None)]
struct Cli {
    /// Path to a Half-Life installation ISO.
    #[arg(long, conflicts_with = "path")]
    iso: Option<PathBuf>,

    /// Development only: load a BSP v30 map straight off disk and open a
    /// renderer window (press Escape to quit).
    ///
    /// This bypasses the media pipeline (no ISO validation, import, cache or
    /// VFS) and exists only while the renderer is being built. It is
    /// compiled in solely by the non-default `dev-tools` cargo feature and
    /// is therefore absent from release builds.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH")]
    dev_bsp: Option<PathBuf>,

    /// Development only: WAD3 texture packages consulted for the map's
    /// external textures. May be repeated. Without them, externally stored
    /// textures render as a checkerboard placeholder.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH", requires = "dev_bsp")]
    dev_wad: Vec<PathBuf>,

    /// Directory for the metadata-only provenance cache.
    ///
    /// Defaults to the platform's per-user cache directory
    /// (`ohl_media::CacheLayout::user_default`).
    #[arg(long)]
    cache: Option<PathBuf>,

    /// Directory the imported payload trees are published under.
    ///
    /// Defaults to a `payload` directory in the platform's per-user data
    /// directory.
    #[arg(long)]
    payload_root: Option<PathBuf>,

    /// A runtime-only, user-local TOML selection recipe.
    ///
    /// Without one, every enumerated component is included. A recipe is
    /// never shipped with the engine and is never logged; see
    /// `docs/MEDIA_IMPORT.md`.
    #[arg(long)]
    recipe: Option<PathBuf>,

    /// Path to a Half-Life installation ISO (positional form).
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Development only: load a studio model (MDL v10) straight off disk and
    /// open a renderer window showing it animating (`[` and `]` cycle the
    /// sequence, Escape quits).
    ///
    /// Combined with `--dev-bsp` the map is loaded too and the model is
    /// placed at its player start; on its own the model simply orbits in
    /// front of the camera. Like `--dev-bsp` this bypasses the media
    /// pipeline and is compiled in solely by the non-default `dev-tools`
    /// cargo feature.
    #[cfg(feature = "dev-tools")]
    #[arg(long, value_name = "PATH")]
    dev_mdl: Option<PathBuf>,
}

/// Formats an event as `[level] message`, mirroring the C++ `ohl::core::log`
/// style (`src/core/src/log.cpp`) so the two builds are easy to compare.
struct CompactEventFormat;

impl<S, N> tracing_subscriber::fmt::FormatEvent<S, N> for CompactEventFormat
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: tracing_subscriber::fmt::format::Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = match *event.metadata().level() {
            tracing::Level::TRACE => "trace",
            tracing::Level::DEBUG => "debug",
            tracing::Level::INFO => "info",
            tracing::Level::WARN => "warning",
            tracing::Level::ERROR => "error",
        };
        write!(writer, "[{level}] ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn init_logging() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .event_format(CompactEventFormat)
        .init();
}

/// Returns a platform line such as `Platform: Linux x86_64`, mirroring
/// `ohl::platform::to_string` in the C++ build.
fn platform_line() -> String {
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    format!("Platform: {os} {arch}")
}

/// Prompts on stdin for an ISO path, as the C++ build did when no path was
/// supplied on the command line.
///
/// Returns `None` when stdin is closed or the line is empty, matching the
/// C++ `prompt_for_iso`'s `std::nullopt` result.
fn prompt_for_iso() -> Option<PathBuf> {
    print!("Path to a legally obtained Half-Life ISO: ");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    let bytes_read = std::io::stdin().read_line(&mut line).ok()?;
    if bytes_read == 0 {
        return None;
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Logs a sanitized "Media preflight failed" line, mirroring the C++
/// application's fixed-prefix error reporting.
fn log_preflight_failure(message: impl std::fmt::Display) {
    tracing::error!("Media preflight failed: {message}");
}

/// The one-line mapping from a preflight crate's result to the
/// `ohl-media` description, plus the `ohl-vfs` class needed to mount without
/// re-running the preflight a third time.
struct Classification {
    vfs_class: ohl_vfs::MediaClass,
    description: MediaDescription,
}

/// Runs the ISO 9660 preflight, then the UDF preflight, over `probe`,
/// exactly as `ohl_vfs::Mount::open` does internally; this copy lets the
/// application map the result onto `ohl_media::MediaDescription` before
/// mounting via `Mount::open_as`, which skips a third redundant probe.
fn classify(
    probe: &mut MediaSourceBlockReader,
) -> Result<Classification, ohl_core::SanitizedError> {
    match ohl_iso9660::preflight(probe) {
        Ok(preflight) => {
            return Ok(map_preflight(
                ohl_vfs::MediaClass::Iso9660,
                &preflight.media,
            ));
        }
        Err(ohl_core::SanitizedError::Unsupported) => {}
        Err(error) => return Err(error),
    }

    let preflight = ohl_udf::preflight(probe)?;
    Ok(map_preflight(ohl_vfs::MediaClass::Udf, &preflight))
}

fn map_preflight(
    vfs_class: ohl_vfs::MediaClass,
    preflight: &ohl_media_archive::MediaPreflight,
) -> Classification {
    let class = match preflight.media_class {
        ohl_media_archive::MediaClass::Udf => MediaClass::Udf,
        ohl_media_archive::MediaClass::Iso9660 => MediaClass::Iso9660,
    };
    let description = MediaDescription::new(
        class,
        preflight.filesystem.as_str(),
        ohl_media::VolumeLabel::sanitized(preflight.volume_label.as_str()),
    );
    Classification {
        vfs_class,
        description,
    }
}

fn run(cli: Cli) -> ExitCode {
    tracing::info!("{APP_NAME} {VERSION}");
    tracing::info!("{}", platform_line());
    tracing::debug!(core_version = ohl_core::VERSION, "loaded ohl-core");

    #[cfg(feature = "dev-tools")]
    if let Some(code) = run_dev_tools(&cli) {
        return code;
    }

    run_media_flow(cli)
}

/// The development-only viewers, which bypass the media pipeline entirely.
///
/// Returns `Some(exit_code)` when one of them handled the invocation, and
/// `None` when the normal media flow should run. Compiled in solely by the
/// non-default `dev-tools` feature, so a release build has no such arm.
#[cfg(feature = "dev-tools")]
fn run_dev_tools(cli: &Cli) -> Option<ExitCode> {
    // Neither path is ever logged: the project's logging policy is uniform,
    // and a user-supplied path is still untrusted input.
    if let Some(path) = cli.dev_mdl.as_deref() {
        tracing::warn!("development model viewer: media pipeline is bypassed");
        return Some(
            match dev_mdl::run(path, cli.dev_bsp.as_deref(), &cli.dev_wad) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::from(3)
                }
            },
        );
    }

    if let Some(path) = cli.dev_bsp.as_deref() {
        tracing::warn!("development map viewer: media pipeline is bypassed");
        return Some(match dev_bsp::run(path, &cli.dev_wad) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::from(3)
            }
        });
    }

    None
}

/// Acquires the user's medium exactly once, validates it, and mounts it
/// read-only, then hands the proof and the mount to [`run_import_flow`].
fn run_media_flow(cli: Cli) -> ExitCode {
    let Some(iso_path) = cli.iso.or(cli.path).or_else(prompt_for_iso) else {
        tracing::error!("No ISO path provided. Use --iso PATH.");
        return ExitCode::from(EXIT_USAGE);
    };

    // The path is acquired exactly once into the pinned capability, then
    // discarded: nothing past this point ever sees it again.
    let source = match MediaSource::open(&iso_path) {
        Ok(source) => Arc::new(source),
        Err(error) => {
            log_preflight_failure(error);
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    drop(iso_path);

    let mut probe = match MediaSourceBlockReader::new(Arc::clone(&source)) {
        Ok(probe) => probe,
        Err(error) => {
            log_preflight_failure(error);
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let classification = match classify(&mut probe) {
        Ok(classification) => classification,
        Err(error) => {
            log_preflight_failure(error);
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    drop(probe);

    let validated =
        match ValidatedMedia::fingerprinting(Arc::clone(&source), classification.description) {
            Ok(validated) => validated,
            Err(error) => {
                log_preflight_failure(error);
                return ExitCode::from(EXIT_FAILURE);
            }
        };

    let mount = match Mount::open_as(
        classification.vfs_class,
        Arc::clone(&source),
        DirectoryLimits::default(),
    ) {
        Ok(mount) => mount,
        Err(error) => {
            log_preflight_failure(error);
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    if let Err(error) = mount.list_page("/") {
        log_preflight_failure(error);
        return ExitCode::from(EXIT_FAILURE);
    }

    tracing::info!("Mounted read-only media image.");

    run_import_flow(
        &validated,
        &mount,
        cli.cache,
        cli.recipe.as_deref(),
        cli.payload_root,
    )
}

/// Publishes or reuses the metadata-only provenance entry, then runs the
/// payload import against it.
fn run_import_flow(
    validated: &ValidatedMedia,
    mount: &Mount,
    cache_root: Option<PathBuf>,
    recipe_path: Option<&Path>,
    payload_root: Option<PathBuf>,
) -> ExitCode {
    let layout = match cache_root {
        Some(root) => CacheLayout::with_root(root),
        None => CacheLayout::user_default(),
    };
    let layout = match layout {
        Ok(layout) => layout,
        Err(error) => {
            tracing::error!("Media cache preparation failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    match ohl_media::prepare_import_cache(validated, &layout) {
        Ok(report) => report.log(),
        Err(error) => {
            tracing::error!("Media cache preparation failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }

    import_payload(validated, mount, &layout, recipe_path, payload_root)
}

/// The fixed line for a build whose parser worker refuses every enumeration.
const UNSUPPORTED_LINE: &str = "Payload import is not supported by this build's parser worker yet; no media executable was run.";

/// Coarse import progress, logged as fixed strings at the quarter marks.
///
/// The sink receives a fraction of the planned byte total and nothing else —
/// no name, no path, no count — so the lines it writes are fixed strings by
/// construction.
#[derive(Debug, Default)]
struct QuarterProgress {
    reported: u8,
}

impl ohl_import::ProgressSink for QuarterProgress {
    fn report(&mut self, fraction: f32) {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the fraction is clamped to 0..=1 by the pipeline"
        )]
        let quarters = (fraction.clamp(0.0, 1.0) * 4.0) as u8;
        while self.reported < quarters.min(4) {
            self.reported += 1;
            match self.reported {
                1 => tracing::info!("Payload import 25% complete."),
                2 => tracing::info!("Payload import 50% complete."),
                3 => tracing::info!("Payload import 75% complete."),
                _ => tracing::info!("Payload import 100% complete."),
            }
        }
    }
}

/// The include-everything recipe used when the user supplies none.
const DEFAULT_RECIPE: &str = "version = 1\ndefault_decision = \"include\"\n";

/// Resolves the payload store root, defaulting under the per-user data
/// directory.
fn payload_root(explicit: Option<PathBuf>) -> Result<PathBuf, &'static str> {
    if let Some(root) = explicit {
        return Ok(root);
    }
    directories::ProjectDirs::from("", "", "open-half-life")
        .map(|dirs| dirs.data_dir().join("payload"))
        .ok_or("no per-user data directory is available")
}

/// Loads the recipe, or the include-everything default.
fn load_recipe(path: Option<&Path>) -> Result<SelectionRecipe, ohl_payload::SelectionRecipeError> {
    match path {
        Some(path) => SelectionRecipe::read_from_file(path),
        // Not a shipped recipe: the neutral policy that selects whatever the
        // worker enumerated, which a user recipe then narrows.
        None => SelectionRecipe::parse(DEFAULT_RECIPE),
    }
}

/// Runs the payload import against a freshly launched confined worker.
fn import_payload(
    validated: &ValidatedMedia,
    mount: &Mount,
    layout: &CacheLayout,
    recipe_path: Option<&Path>,
    explicit_payload_root: Option<PathBuf>,
) -> ExitCode {
    if ohl_import::pipeline::recorded_payload_identity(layout, validated).is_some() {
        tracing::info!("Payload already imported.");
        return ExitCode::SUCCESS;
    }

    let root = match payload_root(explicit_payload_root) {
        Ok(root) => root,
        Err(message) => {
            tracing::error!("Payload import failed: {message}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };
    // The recipe's *contents* are never logged, only that one was rejected.
    let recipe = match load_recipe(recipe_path) {
        Ok(recipe) => recipe,
        Err(error) => {
            tracing::error!("Payload import failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let transport = ohl_import::CancellationToken::default();
    let staging = ohl_payload::CancellationToken::default();
    let cancellation = ohl_import::ImportCancellation {
        transport: &transport,
        staging: &staging,
    };
    let mut progress = QuarterProgress::default();
    let outcome = ohl_import::run_import(
        validated,
        mount,
        &recipe,
        &root,
        layout,
        cancellation,
        &mut progress,
    );
    report_import(outcome)
}

/// The same composition against a caller-supplied worker.
///
/// This exists only for the `#[cfg(test)]` seam below: it is compiled out of
/// every non-test build, so no release binary can be pointed at anything but
/// the confined worker `run_import` launches. `WorkerProcess` is sealed by
/// `ohl-import`, so even the test can only use that crate's own doubles.
#[cfg(test)]
fn import_payload_with_worker<W: ohl_import::WorkerProcess>(
    validated: &ValidatedMedia,
    mount: &Mount,
    layout: &CacheLayout,
    payload_root: &Path,
    worker: W,
) -> ExitCode {
    let recipe = load_recipe(None).expect("the built-in default recipe parses");
    let transport = ohl_import::CancellationToken::default();
    let staging = ohl_payload::CancellationToken::default();
    let allocation = ohl_import::SessionIdAllocator::new()
        .allocate()
        .expect("a fresh session identity");
    report_import(ohl_import::run_import_with_worker(
        validated,
        mount,
        &recipe,
        payload_root,
        layout,
        &ohl_import::ImportConfig::default(),
        worker,
        allocation,
        ohl_import::ImportCancellation {
            transport: &transport,
            staging: &staging,
        },
        &mut ohl_import::DiscardProgress,
    ))
}

/// Maps one import outcome onto a fixed log line and an exit code.
///
/// The report's counts are media-derived and are deliberately not logged.
fn report_import(outcome: Result<ohl_import::ImportReport, ohl_import::ImportError>) -> ExitCode {
    match outcome {
        Ok(report) => {
            if report.outcome == ohl_import::ImportOutcome::Published {
                tracing::info!("Payload imported.");
            } else {
                tracing::info!("Payload already imported.");
            }
            tracing::info!("Payload import complete.");
            ExitCode::SUCCESS
        }
        // The shipped worker refuses every enumeration. Until a real
        // dispatcher exists that is the expected outcome, not a user error.
        Err(ohl_import::ImportError::Unsupported) => {
            tracing::info!("{UNSUPPORTED_LINE}");
            ExitCode::SUCCESS
        }
        // Media this build recognises no container in is equally not a user
        // error: nothing was attempted and nothing was written.
        Err(ohl_import::ImportError::NoContainer) => {
            tracing::info!(
                "No supported payload container was found in the media; nothing was imported."
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!("Payload import failed: {error}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    run(cli)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::ExitCode;
    use std::sync::Arc;

    use ohl_import::testing::{FakeWorker, SyntheticTransport};
    use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia, VolumeLabel};
    use ohl_platform::MediaSource;
    use ohl_vfs::{DirectoryLimits, Mount};

    use super::{DEFAULT_RECIPE, load_recipe, platform_line, report_import};

    #[test]
    fn platform_line_has_expected_shape() {
        let line = platform_line();
        assert!(line.starts_with("Platform: "));
        assert!(line.split_whitespace().count() >= 3);
    }

    #[test]
    fn the_built_in_recipe_includes_every_offered_component() {
        let recipe = load_recipe(None).expect("the built-in recipe parses");
        assert_eq!(
            recipe.default_decision(),
            ohl_payload::SelectionDecision::Include
        );
        assert_eq!(recipe.rule_count(), 0);
        assert!(DEFAULT_RECIPE.contains("version = 1"));
    }

    #[test]
    fn a_worker_that_refuses_the_enumeration_is_reported_as_unsupported_and_succeeds() {
        assert_eq!(
            report_import(Err(ohl_import::ImportError::Unsupported)),
            ExitCode::SUCCESS
        );
    }

    /// Drives the composition root's import step with a worker that refuses
    /// to answer, which is exactly what the shipped parser worker does.
    #[test]
    fn the_composition_root_maps_a_refusing_worker_onto_a_successful_exit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = std::fs::canonicalize(directory.path()).expect("resolved directory");
        let image = crate::test_support::synthetic_container_iso();
        let iso = root.join("synthetic.iso");
        std::fs::write(&iso, &image).expect("synthetic iso fixture");

        let source = Arc::new(MediaSource::open(&iso).expect("pinned source"));
        let validated = ValidatedMedia::fingerprinting(
            Arc::clone(&source),
            MediaDescription::new(
                MediaClass::Iso9660,
                "iso9660",
                VolumeLabel::sanitized("SYNTHETIC"),
            ),
        )
        .expect("stable synthetic source");
        let mount = Mount::open(source, DirectoryLimits::default()).expect("mounted image");
        let layout = CacheLayout::with_root(root.join("cache")).expect("cache layout");

        // The transport answers the handshake and then closes, which is how
        // the shipped worker's `unsupported` dispatcher behaves.
        let transport = Arc::new(SyntheticTransport::new());
        let worker = FakeWorker::new(Arc::clone(&transport));
        let allocation = ohl_import::SessionIdAllocator::new()
            .allocate()
            .expect("identity");
        transport.push_frame(
            &ohl_parser_protocol::FrameHeader::new(
                ohl_parser_protocol::MessageType::Ready,
                allocation.session_id.get(),
                0,
                0,
            ),
            &[],
        );

        let code = super::import_payload_with_worker(
            &validated,
            &mount,
            &layout,
            Path::new(&root.join("payload")),
            worker.clone(),
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(worker.terminate_calls(), 1, "the worker is reaped once");
        assert!(
            ohl_import::pipeline::recorded_payload_identity(&layout, &validated).is_none(),
            "a refused import records nothing"
        );
    }
}
