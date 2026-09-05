//! Assembles the on-disk release layout:
//!
//! ```text
//! open-half-life-<version>-<target-triple>/
//!   bin/open-half-life[.exe]
//!   libexec/open-half-life/ohl-media-parser-worker   (Linux only)
//!   LICENSE
//!   THIRD_PARTY_NOTICES.md
//!   licenses/...
//!   README-dist.md
//!   SHA256SUMS                                       (written by `archive`)
//! ```

use std::io;
use std::path::{Path, PathBuf};

use super::licenses::LicenseEntry;

/// Everything [`assemble`] needs to place one release layout on disk.
pub struct DistInputs<'a> {
    /// The already-built release binary (e.g.
    /// `target/release/open-half-life[.exe]`).
    pub binary_path: &'a Path,
    /// The name the binary keeps inside `bin/` (`open-half-life[.exe]`).
    pub binary_file_name: &'a str,
    /// The already-built parser worker image, when one was produced (Linux
    /// x86-64 only).
    pub worker_image_path: Option<&'a Path>,
    /// Repository root `LICENSE` file.
    pub license_path: &'a Path,
    /// Repository root `THIRD_PARTY_NOTICES.md` file.
    pub third_party_notices_path: &'a Path,
    /// Third-party dependency license entries, from
    /// [`super::licenses::collect_license_entries`].
    pub license_entries: &'a [LicenseEntry],
    /// `<CARGO_HOME>/registry/src`, to bundle dependency license files from.
    pub registry_src_root: &'a Path,
    pub version: &'a str,
    pub target_triple: &'a str,
}

/// Name of the file inside `libexec/open-half-life/` the launcher resolves
/// (kept in sync with `ohl_parser_worker::IMAGE_NAME`; duplicated as a
/// literal here so this module has no dependency on that crate beyond what
/// the caller already resolved).
pub const WORKER_IMAGE_FILE_NAME: &str = "ohl-media-parser-worker";

/// The top-level release folder name: `open-half-life-<version>-<triple>`.
#[must_use]
pub fn release_folder_name(version: &str, target_triple: &str) -> String {
    format!("open-half-life-{version}-{target_triple}")
}

fn readme_dist_contents(version: &str, target_triple: &str, binary_file_name: &str) -> String {
    format!(
        "# Open Half-Life {version} ({target_triple})\n\
\n\
This is a release build of Open Half-Life, a clean-room, cross-platform\n\
reimplementation of the original Half-Life single-player runtime.\n\
\n\
**No game data is included in this archive.** You must own compatible\n\
Half-Life media and provide it yourself; this project does not, and will\n\
never, bundle or download game assets.\n\
\n\
## Running\n\
\n\
```sh\n\
bin/{binary_file_name} --iso /path/to/your-owned-media.iso\n\
```\n\
\n\
or run it with no path at all, which prompts for one on stdin. Pass\n\
`--version` to print the packaged version and exit.\n\
\n\
## Where things go\n\
\n\
On first run, the binary validates the ISO 9660/Joliet or UDF image you\n\
give it, mounts it read-only, and publishes a metadata-only provenance\n\
record under the platform's standard per-user cache directory (override\n\
with `--cache /absolute/path`). No game files are copied out of the image\n\
by this record; it only remembers that the image was validated.\n\
\n\
On Linux, `libexec/open-half-life/{WORKER_IMAGE_FILE_NAME}` is a sandboxed\n\
helper the launcher uses for media parsing; it must stay alongside `bin/`\n\
in the same relative layout this archive extracts to.\n\
\n\
## Licensing\n\
\n\
See `LICENSE` for this project's own license, `THIRD_PARTY_NOTICES.md` for\n\
a summary of third-party components, and `licenses/` for every dependency's\n\
declared license and, where available, its own license file.\n\
\n\
`SHA256SUMS` lists a SHA-256 digest for every file in this archive.\n"
    )
}

/// Copies `source` to `destination`, creating parent directories as needed.
fn copy_into(source: &Path, destination: &Path) -> io::Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source, destination)?;
    Ok(())
}

/// Marks `path` executable (mode `0o755`) on Unix; a no-op elsewhere, since
/// Windows has no such bit and a `.exe` extension is what marks it runnable.
///
/// Only ever fails on Unix, so the non-Unix variant returns `()` rather than
/// an always-`Ok` `Result` (which `clippy::unnecessary_wraps` rightly
/// flags, the same tradeoff `worker_image.rs`'s
/// `normalize_directory_permissions` makes); call sites are `cfg`-gated the
/// same way to still propagate a Unix failure with `?`.
#[cfg(unix)]
fn mark_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) {}

/// Forces a created directory to mode `0o755` (not group- or other-writable)
/// on Unix; a no-op elsewhere. See [`mark_executable`] for why the non-Unix
/// variant returns `()` instead of an always-`Ok` `Result`.
#[cfg(unix)]
fn mark_directory_not_group_writable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn mark_directory_not_group_writable(_path: &Path) {}

/// Assembles the release layout under `output_root`, returning the path of
/// the created `open-half-life-<version>-<triple>` directory.
///
/// # Errors
/// Any [`io::Error`] hit while creating directories or copying/writing
/// files.
pub fn assemble(inputs: &DistInputs<'_>, output_root: &Path) -> io::Result<PathBuf> {
    let dist_dir = output_root.join(release_folder_name(inputs.version, inputs.target_triple));
    if dist_dir.exists() {
        std::fs::remove_dir_all(&dist_dir)?;
    }
    std::fs::create_dir_all(&dist_dir)?;

    let bin_dir = dist_dir.join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    #[cfg(unix)]
    mark_directory_not_group_writable(&bin_dir)?;
    #[cfg(not(unix))]
    mark_directory_not_group_writable(&bin_dir);
    let installed_binary = bin_dir.join(inputs.binary_file_name);
    copy_into(inputs.binary_path, &installed_binary)?;
    #[cfg(unix)]
    mark_executable(&installed_binary)?;
    #[cfg(not(unix))]
    mark_executable(&installed_binary);

    if let Some(worker_image_path) = inputs.worker_image_path {
        let libexec_dir = dist_dir.join("libexec").join("open-half-life");
        std::fs::create_dir_all(&libexec_dir)?;
        #[cfg(unix)]
        {
            mark_directory_not_group_writable(dist_dir.join("libexec").as_path())?;
            mark_directory_not_group_writable(&libexec_dir)?;
        }
        #[cfg(not(unix))]
        {
            mark_directory_not_group_writable(dist_dir.join("libexec").as_path());
            mark_directory_not_group_writable(&libexec_dir);
        }
        let installed_worker = libexec_dir.join(WORKER_IMAGE_FILE_NAME);
        copy_into(worker_image_path, &installed_worker)?;
        #[cfg(unix)]
        mark_executable(&installed_worker)?;
        #[cfg(not(unix))]
        mark_executable(&installed_worker);
    }

    copy_into(inputs.license_path, &dist_dir.join("LICENSE"))?;
    copy_into(
        inputs.third_party_notices_path,
        &dist_dir.join("THIRD_PARTY_NOTICES.md"),
    )?;

    super::licenses::write_licenses_folder(
        inputs.license_entries,
        inputs.registry_src_root,
        &dist_dir.join("licenses"),
    )?;

    std::fs::write(
        dist_dir.join("README-dist.md"),
        readme_dist_contents(
            inputs.version,
            inputs.target_triple,
            inputs.binary_file_name,
        ),
    )?;

    Ok(dist_dir)
}

#[cfg(test)]
mod tests {
    use super::{DistInputs, WORKER_IMAGE_FILE_NAME, assemble, release_folder_name};

    fn write_dummy(path: &std::path::Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write dummy file");
    }

    #[test]
    fn release_folder_name_matches_the_expected_convention() {
        assert_eq!(
            release_folder_name("1.2.3", "x86_64-unknown-linux-gnu"),
            "open-half-life-1.2.3-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn assembles_the_full_layout_including_the_worker_image() {
        let workdir = tempfile::tempdir().expect("tempdir");
        let binary_path = workdir.path().join("staged-binary");
        write_dummy(&binary_path, b"not a real binary");
        let worker_path = workdir.path().join("staged-worker");
        write_dummy(&worker_path, b"not a real worker image");
        let license_path = workdir.path().join("LICENSE");
        write_dummy(&license_path, b"MIT license text");
        let notices_path = workdir.path().join("THIRD_PARTY_NOTICES.md");
        write_dummy(&notices_path, b"# Third-party notices");
        let registry_src_root = workdir.path().join("registry-src");
        std::fs::create_dir_all(&registry_src_root).expect("mkdir");

        let output_root = workdir.path().join("output");
        let inputs = DistInputs {
            binary_path: &binary_path,
            binary_file_name: "open-half-life",
            worker_image_path: Some(&worker_path),
            license_path: &license_path,
            third_party_notices_path: &notices_path,
            license_entries: &[],
            registry_src_root: &registry_src_root,
            version: "0.1.0",
            target_triple: "x86_64-unknown-linux-gnu",
        };

        let dist_dir = assemble(&inputs, &output_root).expect("assemble");
        assert_eq!(
            dist_dir,
            output_root.join("open-half-life-0.1.0-x86_64-unknown-linux-gnu")
        );

        assert_eq!(
            std::fs::read(dist_dir.join("bin").join("open-half-life")).expect("read binary"),
            b"not a real binary"
        );
        assert_eq!(
            std::fs::read(
                dist_dir
                    .join("libexec")
                    .join("open-half-life")
                    .join(WORKER_IMAGE_FILE_NAME)
            )
            .expect("read worker image"),
            b"not a real worker image"
        );
        assert!(dist_dir.join("LICENSE").is_file());
        assert!(dist_dir.join("THIRD_PARTY_NOTICES.md").is_file());
        assert!(dist_dir.join("licenses").join("SUMMARY.txt").is_file());
        assert!(dist_dir.join("README-dist.md").is_file());

        let readme = std::fs::read_to_string(dist_dir.join("README-dist.md")).expect("read readme");
        assert!(readme.contains("No game data is included"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let binary_mode = std::fs::metadata(dist_dir.join("bin").join("open-half-life"))
                .expect("stat binary")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(binary_mode, 0o755);
            let libexec_dir = dist_dir.join("libexec").join("open-half-life");
            let dir_mode = std::fs::metadata(&libexec_dir)
                .expect("stat libexec dir")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o755);
        }
    }

    #[test]
    fn omits_libexec_entirely_when_no_worker_image_is_given() {
        let workdir = tempfile::tempdir().expect("tempdir");
        let binary_path = workdir.path().join("staged-binary");
        write_dummy(&binary_path, b"binary");
        let license_path = workdir.path().join("LICENSE");
        write_dummy(&license_path, b"license");
        let notices_path = workdir.path().join("THIRD_PARTY_NOTICES.md");
        write_dummy(&notices_path, b"notices");
        let registry_src_root = workdir.path().join("registry-src");
        std::fs::create_dir_all(&registry_src_root).expect("mkdir");

        let inputs = DistInputs {
            binary_path: &binary_path,
            binary_file_name: "open-half-life.exe",
            worker_image_path: None,
            license_path: &license_path,
            third_party_notices_path: &notices_path,
            license_entries: &[],
            registry_src_root: &registry_src_root,
            version: "0.1.0",
            target_triple: "x86_64-pc-windows-msvc",
        };

        let dist_dir = assemble(&inputs, &workdir.path().join("output")).expect("assemble");
        assert!(!dist_dir.join("libexec").exists());
        assert!(dist_dir.join("bin").join("open-half-life.exe").is_file());
    }
}
