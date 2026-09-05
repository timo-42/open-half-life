//! Target-triple resolution for `cargo xtask dist --target <triple>`.

/// The kind of archive and binary name suffix a target triple implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// `.exe` binaries, packaged as a `.zip`.
    Windows,
    /// No binary suffix, packaged as a `.tar.gz`.
    Unix,
}

/// Classifies `triple` by whether it names a Windows target.
///
/// Every target triple this project cross-compiles for spells the OS as its
/// third (`x86_64-pc-windows-msvc`) or, for some historical GNU triples,
/// embeds `windows` as a distinct component; checking for the `windows`
/// component covers both without hard-coding the exact triple list.
#[must_use]
pub fn classify(triple: &str) -> Platform {
    if triple.split('-').any(|component| component == "windows") {
        Platform::Windows
    } else {
        Platform::Unix
    }
}

/// The binary file name for `crate_binary_name` on `platform`.
#[must_use]
pub fn binary_file_name(crate_binary_name: &str, platform: Platform) -> String {
    match platform {
        Platform::Windows => format!("{crate_binary_name}.exe"),
        Platform::Unix => crate_binary_name.to_owned(),
    }
}

/// Why the host triple could not be determined.
#[derive(Debug)]
pub struct HostTripleError(pub String);

impl std::fmt::Display for HostTripleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to determine the host target triple: {}",
            self.0
        )
    }
}

impl std::error::Error for HostTripleError {}

/// Parses the `host: <triple>` line `rustc -vV` prints, the same
/// self-description `rustup`/`cargo` use to pick a default target.
#[must_use]
pub fn parse_host_triple(verbose_version: &str) -> Option<&str> {
    verbose_version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
}

/// Asks `rustc -vV` for the host's default target triple.
///
/// # Errors
/// [`HostTripleError`] when `rustc` cannot be spawned, exits unsuccessfully,
/// or its output has no `host:` line.
pub fn host_triple() -> Result<String, HostTripleError> {
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("-vV")
            .output()
            .map_err(|error| HostTripleError(error.to_string()))?;
    if !output.status.success() {
        return Err(HostTripleError(format!(
            "rustc -vV exited with {}",
            output.status
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_host_triple(&text)
        .map(str::to_owned)
        .ok_or_else(|| HostTripleError("no `host:` line in `rustc -vV` output".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{Platform, binary_file_name, classify, parse_host_triple};

    #[test]
    fn classifies_msvc_and_gnu_windows_triples_as_windows() {
        assert_eq!(classify("x86_64-pc-windows-msvc"), Platform::Windows);
        assert_eq!(classify("x86_64-pc-windows-gnu"), Platform::Windows);
    }

    #[test]
    fn classifies_linux_and_macos_triples_as_unix() {
        assert_eq!(classify("x86_64-unknown-linux-gnu"), Platform::Unix);
        assert_eq!(classify("aarch64-apple-darwin"), Platform::Unix);
    }

    #[test]
    fn binary_file_name_adds_exe_only_on_windows() {
        assert_eq!(
            binary_file_name("open-half-life", Platform::Windows),
            "open-half-life.exe"
        );
        assert_eq!(
            binary_file_name("open-half-life", Platform::Unix),
            "open-half-life"
        );
    }

    #[test]
    fn parses_the_host_line_from_rustc_verbose_version() {
        let sample = "rustc 1.98.1 (deadbeef 2026-08-05)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.98.1\n";
        assert_eq!(parse_host_triple(sample), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn missing_host_line_yields_none() {
        assert_eq!(parse_host_triple("binary: rustc\nrelease: 1.98.1\n"), None);
    }
}
