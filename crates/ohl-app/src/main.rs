//! `open-half-life`: the Rust composition-root binary.
//!
//! This is the M1-rs milestone: it parses arguments (or prompts on stdin, as
//! the C++ build did), acquires the media path exactly once into a pinned
//! [`ohl_platform::MediaSource`], classifies it with the ISO 9660 preflight
//! and then the UDF preflight, fingerprints and binds a
//! [`ohl_media::ValidatedMedia`] proof, mounts it read-only through
//! [`ohl_vfs::Mount`], and publishes or reuses a metadata-only provenance
//! cache entry. It does not import payload yet; see `docs/MILESTONES.md`.
//!
//! Every failure is logged as a single sanitized line and maps to the same
//! exit codes the C++ `src/app/main.cpp` used: `2` for a command-line usage
//! error, `1` for a media, mount, or cache failure, `0` on success.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia};
use ohl_platform::MediaSource;
use ohl_vfs::{DirectoryLimits, MediaSourceBlockReader, Mount};

#[cfg(feature = "dev-tools")]
mod dev_bsp;
#[cfg(feature = "dev-tools")]
mod dev_mdl;

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
    if let Some(path) = cli.dev_mdl.as_deref() {
        tracing::warn!("development model viewer: media pipeline is bypassed");
        return match dev_mdl::run(path, cli.dev_bsp.as_deref(), &cli.dev_wad) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                std::process::ExitCode::from(3)
            }
        };
    }

    #[cfg(feature = "dev-tools")]
    if let Some(path) = cli.dev_bsp.as_deref() {
        // The path itself is never logged: the project's logging policy is
        // uniform, and a user-supplied path is still untrusted input.
        tracing::warn!("development map viewer: media pipeline is bypassed");
        return match dev_bsp::run(path, &cli.dev_wad) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                std::process::ExitCode::from(3)
            }
        };
    }

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

    let layout = match cli.cache {
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

    match ohl_media::prepare_import_cache(&validated, &layout) {
        Ok(report) => report.log(),
        Err(error) => {
            tracing::error!("Media cache preparation failed: {error}");
            return ExitCode::from(EXIT_FAILURE);
        }
    }

    tracing::info!("Payload import is not implemented yet; no media executable was run.");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging();
    run(cli)
}

#[cfg(test)]
mod tests {
    use super::platform_line;

    #[test]
    fn platform_line_has_expected_shape() {
        let line = platform_line();
        assert!(line.starts_with("Platform: "));
        assert!(line.split_whitespace().count() >= 3);
    }
}
