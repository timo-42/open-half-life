//! Third-party license collection for the `licenses/` folder in a release.
//!
//! `cargo metadata` reports the `license` field every crate on crates.io (or
//! any other registry) declares in its own `Cargo.toml`; this module walks
//! that list, skips this workspace's own crates (they have no `source`,
//! since they are path dependencies), and bundles a copy of each dependency's
//! own `LICENSE*`/`COPYING*`/`NOTICE*` files from the local Cargo registry
//! cache when Cargo has already fetched its source (which it always has by
//! the time `cargo xtask dist` runs, since it just finished building with
//! it).

use std::io;
use std::path::{Path, PathBuf};

/// One third-party dependency's declared license, ready to be written into
/// the release's `licenses/` folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LicenseEntry {
    pub name: String,
    pub version: String,
    /// The crate's `license` field (an SPDX expression), when it declared
    /// one directly rather than only a `license-file`.
    pub license: Option<String>,
}

impl LicenseEntry {
    /// The directory name this entry's bundled files live under:
    /// `<name>-<version>`, matching the registry cache's own naming so the
    /// two are trivially cross-referenced.
    #[must_use]
    pub fn directory_name(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

/// Runs `cargo metadata` against the workspace at `root` and returns the
/// parsed result.
///
/// # Errors
/// A description of whatever `cargo_metadata` reports: `cargo` not found,
/// the manifest not resolvable, or the metadata failing to parse.
pub fn load_metadata(root: &Path) -> Result<cargo_metadata::Metadata, String> {
    cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .exec()
        .map_err(|error| error.to_string())
}

/// Collects one [`LicenseEntry`] per non-workspace package in `metadata`,
/// sorted and deduplicated by `(name, version)`.
///
/// A package with no `source` is a path or workspace member (this project's
/// own crates, or `xtask` itself) and is deliberately excluded: this is a
/// *third-party* notice folder.
#[must_use]
pub fn collect_license_entries(metadata: &cargo_metadata::Metadata) -> Vec<LicenseEntry> {
    let mut entries: Vec<LicenseEntry> = metadata
        .packages
        .iter()
        .filter(|package| package.source.is_some())
        .map(|package| LicenseEntry {
            name: package.name.to_string(),
            version: package.version.to_string(),
            license: package.license.clone(),
        })
        .collect();
    entries.sort();
    entries.dedup();
    entries
}

/// Names Cargo's own convention recognises as a license or notice file,
/// matched case-insensitively against a file's leading component (so
/// `LICENSE`, `LICENSE-MIT`, `LICENSE.txt`, `COPYING.LESSER` and
/// `NOTICE.md` all match).
const LICENSE_LIKE_PREFIXES: [&str; 4] = ["license", "licence", "copying", "notice"];

fn is_license_like(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    LICENSE_LIKE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// Finds the extracted registry source directory for `name`-`version` under
/// `registry_src_root` (`<CARGO_HOME>/registry/src`), whose immediate
/// children are one directory per registry (e.g.
/// `index.crates.io-<hash>`), each holding one `<name>-<version>` directory
/// per fetched crate.
#[must_use]
pub fn find_registry_src_dir(
    registry_src_root: &Path,
    name: &str,
    version: &str,
) -> Option<PathBuf> {
    let target = format!("{name}-{version}");
    let registries = std::fs::read_dir(registry_src_root).ok()?;
    for registry in registries.filter_map(Result::ok) {
        let candidate = registry.path().join(&target);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Every top-level license-like file directly inside `source_dir`.
#[must_use]
pub fn license_like_files(source_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(source_dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_license_like)
        })
        .collect();
    files.sort();
    files
}

/// Writes the `licenses/` folder at `dest`: a `SUMMARY.txt` listing every
/// entry's declared license, and one `<name>-<version>/` subdirectory per
/// entry holding a generated `LICENSE-INFO.txt` plus a copy of any
/// license-like file found under `registry_src_root` for that crate.
///
/// # Errors
/// Any [`io::Error`] from creating directories, writing `SUMMARY.txt`, or
/// copying a bundled file.
pub fn write_licenses_folder(
    entries: &[LicenseEntry],
    registry_src_root: &Path,
    dest: &Path,
) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;

    let mut summary = String::new();
    for entry in entries {
        summary.push_str(&entry.name);
        summary.push(' ');
        summary.push_str(&entry.version);
        summary.push_str(": ");
        summary.push_str(entry.license.as_deref().unwrap_or("UNSPECIFIED"));
        summary.push('\n');
    }
    std::fs::write(dest.join("SUMMARY.txt"), summary)?;

    for entry in entries {
        let entry_dir = dest.join(entry.directory_name());
        std::fs::create_dir_all(&entry_dir)?;
        std::fs::write(
            entry_dir.join("LICENSE-INFO.txt"),
            format!(
                "{} {}\nLicense: {}\n",
                entry.name,
                entry.version,
                entry.license.as_deref().unwrap_or("UNSPECIFIED")
            ),
        )?;

        if let Some(source_dir) =
            find_registry_src_dir(registry_src_root, &entry.name, &entry.version)
        {
            for file in license_like_files(&source_dir) {
                if let Some(file_name) = file.file_name() {
                    std::fs::copy(&file, entry_dir.join(file_name))?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        LicenseEntry, collect_license_entries, find_registry_src_dir, is_license_like,
        license_like_files, write_licenses_folder,
    };

    fn fixture_metadata(json: &str) -> cargo_metadata::Metadata {
        serde_json::from_str(json).expect("valid cargo-metadata fixture JSON")
    }

    /// A minimal but well-formed `cargo metadata --format-version 1` fixture
    /// with one workspace crate (no `source`, a path dependency) and two
    /// third-party crates, one of them appearing twice (as a normal and a
    /// dev-dependency resolution artefact would) to exercise dedup.
    const FIXTURE_JSON: &str = r#"{
        "packages": [
            {
                "name": "xtask",
                "version": "0.1.0",
                "id": "path+file:///repo/xtask#0.1.0",
                "license": null,
                "license_file": null,
                "description": null,
                "source": null,
                "dependencies": [],
                "targets": [],
                "features": {},
                "manifest_path": "/repo/xtask/Cargo.toml",
                "categories": [],
                "keywords": [],
                "readme": null,
                "repository": null,
                "homepage": null,
                "documentation": null,
                "edition": "2024",
                "links": null,
                "default_run": null,
                "rust_version": null,
                "metadata": null,
                "publish": null
            },
            {
                "name": "thiserror",
                "version": "2.0.20",
                "id": "registry+https://github.com/rust-lang/crates.io-index#thiserror@2.0.20",
                "license": "MIT OR Apache-2.0",
                "license_file": null,
                "description": null,
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "dependencies": [],
                "targets": [],
                "features": {},
                "manifest_path": "/home/user/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/thiserror-2.0.20/Cargo.toml",
                "categories": [],
                "keywords": [],
                "readme": null,
                "repository": null,
                "homepage": null,
                "documentation": null,
                "edition": "2024",
                "links": null,
                "default_run": null,
                "rust_version": null,
                "metadata": null,
                "publish": null
            },
            {
                "name": "thiserror",
                "version": "2.0.20",
                "id": "registry+https://github.com/rust-lang/crates.io-index#thiserror@2.0.20",
                "license": "MIT OR Apache-2.0",
                "license_file": null,
                "description": null,
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "dependencies": [],
                "targets": [],
                "features": {},
                "manifest_path": "/home/user/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/thiserror-2.0.20/Cargo.toml",
                "categories": [],
                "keywords": [],
                "readme": null,
                "repository": null,
                "homepage": null,
                "documentation": null,
                "edition": "2024",
                "links": null,
                "default_run": null,
                "rust_version": null,
                "metadata": null,
                "publish": null
            },
            {
                "name": "unlicensed-dep",
                "version": "1.2.3",
                "id": "registry+https://github.com/rust-lang/crates.io-index#unlicensed-dep@1.2.3",
                "license": null,
                "license_file": null,
                "description": null,
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "dependencies": [],
                "targets": [],
                "features": {},
                "manifest_path": "/home/user/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/unlicensed-dep-1.2.3/Cargo.toml",
                "categories": [],
                "keywords": [],
                "readme": null,
                "repository": null,
                "homepage": null,
                "documentation": null,
                "edition": "2024",
                "links": null,
                "default_run": null,
                "rust_version": null,
                "metadata": null,
                "publish": null
            }
        ],
        "workspace_members": ["path+file:///repo/xtask#0.1.0"],
        "resolve": null,
        "workspace_root": "/repo",
        "target_directory": "/repo/target",
        "build_directory": null,
        "version": 1
    }"#;

    #[test]
    fn excludes_path_dependencies_and_dedups_registry_ones() {
        let metadata = fixture_metadata(FIXTURE_JSON);
        let entries = collect_license_entries(&metadata);
        assert_eq!(
            entries,
            vec![
                LicenseEntry {
                    name: "thiserror".to_owned(),
                    version: "2.0.20".to_owned(),
                    license: Some("MIT OR Apache-2.0".to_owned()),
                },
                LicenseEntry {
                    name: "unlicensed-dep".to_owned(),
                    version: "1.2.3".to_owned(),
                    license: None,
                },
            ]
        );
    }

    #[test]
    fn is_license_like_matches_common_names_case_insensitively() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "license.txt",
            "COPYING",
            "COPYING.LESSER",
            "NOTICE.md",
            "Licence",
        ] {
            assert!(is_license_like(name), "{name} should match");
        }
        for name in ["Cargo.toml", "README.md", "src/lib.rs"] {
            assert!(!is_license_like(name), "{name} should not match");
        }
    }

    #[test]
    fn finds_the_registry_src_directory_across_registries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let registry_dir = tmp.path().join("index.crates.io-1949cf8c6b5b557f");
        let crate_dir = registry_dir.join("thiserror-2.0.20");
        std::fs::create_dir_all(&crate_dir).expect("create fixture dir");
        std::fs::write(crate_dir.join("LICENSE-MIT"), b"MIT text").expect("write license");

        let found = find_registry_src_dir(tmp.path(), "thiserror", "2.0.20");
        assert_eq!(found, Some(crate_dir));

        assert_eq!(find_registry_src_dir(tmp.path(), "missing", "1.0.0"), None);
    }

    #[test]
    fn license_like_files_lists_only_matching_top_level_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("LICENSE-MIT"), b"a").expect("write");
        std::fs::write(tmp.path().join("LICENSE-APACHE"), b"b").expect("write");
        std::fs::write(tmp.path().join("Cargo.toml"), b"c").expect("write");
        std::fs::create_dir(tmp.path().join("src")).expect("mkdir");

        let files = license_like_files(tmp.path());
        let names: Vec<_> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, vec!["LICENSE-APACHE", "LICENSE-MIT"]);
    }

    #[test]
    fn writes_a_summary_and_per_crate_bundle_with_registry_files() {
        let cargo_home = tempfile::tempdir().expect("tempdir");
        let registry_src_root = cargo_home.path().join("registry").join("src");
        let crate_dir = registry_src_root
            .join("index.crates.io-1949cf8c6b5b557f")
            .join("thiserror-2.0.20");
        std::fs::create_dir_all(&crate_dir).expect("mkdir");
        std::fs::write(crate_dir.join("LICENSE-MIT"), b"MIT text").expect("write");

        let dest = tempfile::tempdir().expect("tempdir");
        let entries = vec![
            LicenseEntry {
                name: "thiserror".to_owned(),
                version: "2.0.20".to_owned(),
                license: Some("MIT OR Apache-2.0".to_owned()),
            },
            LicenseEntry {
                name: "unlicensed-dep".to_owned(),
                version: "1.2.3".to_owned(),
                license: None,
            },
        ];
        write_licenses_folder(&entries, &registry_src_root, dest.path()).expect("write");

        let summary =
            std::fs::read_to_string(dest.path().join("SUMMARY.txt")).expect("read summary");
        assert!(summary.contains("thiserror 2.0.20: MIT OR Apache-2.0"));
        assert!(summary.contains("unlicensed-dep 1.2.3: UNSPECIFIED"));

        let bundled = dest.path().join("thiserror-2.0.20").join("LICENSE-MIT");
        assert_eq!(
            std::fs::read(bundled).expect("read bundled license"),
            b"MIT text"
        );

        let info = std::fs::read_to_string(
            dest.path()
                .join("thiserror-2.0.20")
                .join("LICENSE-INFO.txt"),
        )
        .expect("read info");
        assert!(info.contains("MIT OR Apache-2.0"));

        // No registry source was staged for `unlicensed-dep`, so its
        // directory must still exist (from the generated info file) but
        // carry no bundled file.
        let unlicensed_dir = dest.path().join("unlicensed-dep-1.2.3");
        assert!(unlicensed_dir.join("LICENSE-INFO.txt").is_file());
        assert_eq!(std::fs::read_dir(&unlicensed_dir).unwrap().count(), 1);
    }
}
