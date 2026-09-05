//! `open-half-life`: the Rust composition-root binary.
//!
//! This is an M0-rs bootstrap: it parses arguments, sets up sanitized
//! logging, reports the detected platform, and refuses `--iso` with a clear
//! "not implemented yet" message rather than pretending to import media. It
//! does not yet reproduce the accepted C++ M1 ISO-detection behavior; see
//! `docs/MILESTONES.md`.

use std::path::PathBuf;

use clap::Parser;

#[cfg(feature = "dev-tools")]
mod dev_bsp;

const APP_NAME: &str = "Open Half-Life";
const VERSION: &str = env!("OHL_APP_VERSION");

/// `Open Half-Life <version>` command line.
#[derive(Debug, Parser)]
#[command(name = "Open Half-Life", version = VERSION, about = None, long_about = None)]
struct Cli {
    /// Path to a Half-Life installation ISO.
    ///
    /// Media import is not implemented in the Rust build yet; this flag is
    /// accepted so scripts and users can discover that fact rather than
    /// hitting an "unknown argument" error.
    #[arg(long)]
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

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_logging();

    tracing::info!("{APP_NAME} {VERSION}");
    tracing::info!("{}", platform_line());
    tracing::debug!(core_version = ohl_core::VERSION, "loaded ohl-core");

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

    if let Some(iso) = cli.iso {
        drop(iso);
        eprintln!("media import is not implemented in the Rust build yet");
        return std::process::ExitCode::from(2);
    }

    std::process::ExitCode::SUCCESS
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
