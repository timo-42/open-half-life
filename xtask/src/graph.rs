//! Crate dependency-graph policy.
//!
//! Encodes the acyclic edge table from `.plan/rust-architecture-r1.md`
//! section 1 as data, so crates not yet created (most of the table, at R2)
//! are still validated automatically the moment they are added under
//! `crates/`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The crate that is allowed to depend on every other crate in the table.
pub const COMPOSITION_ROOT: &str = "ohl-app";

/// Allowed direct intra-workspace dependency edges, `crate -> [dependency]`.
/// Every crate name that will ever exist in `crates/` should appear here so
/// the check degrades gracefully (skips) rather than silently passing for
/// crates the table has not been told about yet... except that an *unknown*
/// crate (one under `crates/` but absent from this table) is itself treated
/// as a violation: see [`check_edges`].
pub const ALLOWED_EDGES: &[(&str, &[&str])] = &[
    ("ohl-core", &[]),
    ("ohl-parser-protocol", &["ohl-core"]),
    ("ohl-parser-worker-service", &["ohl-parser-protocol"]),
    ("ohl-parser-worker", &["ohl-parser-worker-service"]),
    // The container back ends the worker image hosts: the dispatcher logic
    // lives here, outside the freestanding image, so it can be unit-tested on
    // the host through the real service (R4.7b).
    (
        "ohl-parser-backends",
        &[
            "ohl-core",
            "ohl-parser-protocol",
            "ohl-parser-worker-service",
            "ohl-payload",
            "ohl-wise",
            "ohl-mscab",
            "ohl-isz",
        ],
    ),
    // `ohl-media-archive` holds the block-source trait, the bounded listing
    // model, the path rules and the fixed classification vocabulary that both
    // media readers and `ohl-vfs` share. Keeping it separate is what lets the
    // two readers stay independent of each other (R3.2).
    ("ohl-media-archive", &["ohl-core"]),
    ("ohl-iso9660", &["ohl-core", "ohl-media-archive"]),
    ("ohl-udf", &["ohl-core", "ohl-media-archive"]),
    // Clean-room MS-CAB container decoder used by the sandboxed parser
    // worker; it only needs the shared sanitized diagnostics.
    ("ohl-mscab", &["ohl-core"]),
    ("ohl-cabinet-format", &["ohl-core"]),
    // Clean-room InstallShield 3 "Z" archive and PKWARE DCL explode decoder.
    // Unlike the cabinet crates it is written from public documentation, so
    // it is an ordinary project-owned leaf over `ohl-core` (R4).
    ("ohl-isz", &["ohl-core"]),
    // Clean-room Wise Installation System package decoder: PE overlay, the
    // CRC-32-terminated DEFLATE stream chain, the script file table and
    // bounded extraction. Written from public documentation, so it is an
    // ordinary project-owned leaf over `ohl-core` (R4).
    ("ohl-wise", &["ohl-core"]),
    ("ohl-cabinet", &["ohl-cabinet-format"]),
    ("ohl-platform", &["ohl-core"]),
    // Development-only: builds the freestanding isolated-worker test image.
    // It has no dependencies and nothing shipping may depend on it (only
    // `[dev-dependencies]`, which this table deliberately does not inspect).
    ("ohl-test-worker", &[]),
    (
        "ohl-vfs",
        &[
            "ohl-core",
            "ohl-platform",
            "ohl-media-archive",
            "ohl-iso9660",
            "ohl-udf",
        ],
    ),
    ("ohl-media", &["ohl-platform", "ohl-core"]),
    // `ohl-payload` owns the payload path policy, layout planning, component
    // selection, and the transactional staging layer over `ohl-platform`'s
    // create-new staging and no-replace publication primitives (R4.6).
    // `ohl-import` composes it into an import session.
    ("ohl-payload", &["ohl-core", "ohl-platform"]),
    (
        "ohl-import",
        &[
            "ohl-core",
            "ohl-media",
            "ohl-payload",
            "ohl-vfs",
            "ohl-parser-protocol",
            "ohl-platform",
            // Recognition only: the parent confirms that a PE overlay begins
            // a Wise stream chain before it hands the worker a window over
            // it. Every decode happens in the confined worker.
            "ohl-wise",
        ],
    ),
    ("ohl-formats", &["ohl-core"]),
    // GoldSrc-style asset filesystem: resolves game-relative asset paths
    // over an imported payload's loose files and PAK archives, using
    // `ohl-formats`' PAK directory decoder. Read-only and independent of
    // `ohl-vfs` (which mounts the pinned installation medium itself, not
    // its extracted payload).
    ("ohl-assets", &["ohl-core", "ohl-formats"]),
    ("ohl-world", &["ohl-formats", "ohl-vfs"]),
    // Clip-hull tracing and player movement: the hulls come from
    // `ohl-formats`' BSP30 reader and diagnostics from `ohl-core`.
    // `ohl-vfs` stays listed for the map loading path this crate will use
    // once maps arrive through the real pipeline.
    ("ohl-physics", &["ohl-core", "ohl-formats", "ohl-vfs"]),
    // Navigation node graph, A* and local steering. It needs the hull
    // traces (`ohl-physics`) to validate links per hull and the BSP entity
    // parser (`ohl-formats`) for the one helper that reads `info_node`
    // seeds; it deliberately does *not* depend on `ohl-game`, so the AI
    // layer can compose it without a cycle.
    ("ohl-nav", &["ohl-core", "ohl-physics", "ohl-formats"]),
    ("ohl-render", &["ohl-core", "ohl-world"]),
    ("ohl-audio", &["ohl-core"]),
    // Versioned save-file container: bounded header, tagged section table
    // with per-section and whole-file SHA-256 integrity, and an atomic-write
    // save-slot directory. Project-owned format, independent of the world/
    // gameplay crates it will eventually be driven by.
    ("ohl-save", &["ohl-core"]),
    ("ohl-input", &["ohl-core"]),
    ("ohl-game", &["ohl-core", "ohl-formats", "ohl-world"]),
    // Sourced campaign data (chapter/map sequence, difficulty, skill.cfg
    // table): plain data plus small bounded lookups, so it only needs the
    // shared diagnostics primitive. Deliberately not allowed to depend on
    // `ohl-formats`; see `crates/ohl-campaign/src/skill_table.rs`.
    ("ohl-campaign", &["ohl-core"]),
    // Monster AI core: conditions, senses, the schedule/task runner, squads
    // and the movement glue. It reads world visibility from `ohl-world`,
    // traces through `ohl-physics`, attaches its components to the
    // `ohl-game` entity registry rather than owning a second entity world,
    // and routes monster movement over `ohl-nav`'s node graph (package 7.6)
    // via `monsters::nav_bridge::NavBridge`. Deliberately no edge to
    // `ohl-render`, `ohl-audio` or `ohl-ui`: presentation is pulled from the
    // event list `AiWorld::tick` returns.
    (
        "ohl-ai",
        &[
            "ohl-core",
            "ohl-physics",
            "ohl-world",
            "ohl-game",
            "ohl-nav",
        ],
    ),
    // Combat skeleton (M7.1): damage model, hit resolution against world
    // hulls and posed studio hitboxes, and a bounded combat-event queue.
    // No edge to `ohl-render`, `ohl-audio` or `ohl-ui`: presentation is
    // pulled by the composition root from the event queue.
    (
        "ohl-combat",
        &["ohl-core", "ohl-physics", "ohl-world", "ohl-game"],
    ),
    ("ohl-ui", &["ohl-core"]),
    // The game state: the one crate allowed to compose world, entities, map
    // logic, physics and rendering into a tickable loop, so `ohl-app` stays
    // a thin composition root. It sits directly below `ohl-app` and nothing
    // else may depend on it.
    (
        "ohl-engine",
        &[
            "ohl-core",
            "ohl-world",
            "ohl-render",
            "ohl-physics",
            "ohl-game",
            "ohl-assets",
            "ohl-formats",
            "ohl-campaign",
            "ohl-audio",
            "ohl-ui",
            // Campaign flow (M8.2): the engine composes its save payload
            // into `ohl-save`'s container, which stays a pure container
            // crate and never learns about game state.
            "ohl-save",
        ],
    ),
    // M7.4's HUD/audio/viewmodel bridge: consumes `ohl-combat`'s
    // `CombatEvent`/`WeaponAction`/pickup output and produces
    // `ohl-ui::HudState` updates, sound cues (keyed by asset path, not an
    // `ohl-audio::PlayRequest` itself; see the crate's docs) and viewmodel
    // actions. Kept out of `ohl-app`/`ohl-engine` so neither `ohl-ui` nor
    // `ohl-audio` needs an edge back to `ohl-combat`.
    (
        "ohl-gameplay",
        &["ohl-core", "ohl-combat", "ohl-game", "ohl-ui", "ohl-audio"],
    ),
    // Player systems (health/armor, fall damage, drowning, contact damage,
    // HEV suit voice events, flashlight, long jump ownership). It reads the
    // movement step's reports from `ohl-physics`, `trigger_hurt`/
    // `func_ladder` map data from `ohl-game`, saves through `ohl-save`, and
    // projects the HUD into `ohl-ui` behind an optional feature. No edge to
    // `ohl-audio`: suit speech is emitted as events the host maps.
    (
        "ohl-player",
        &["ohl-core", "ohl-physics", "ohl-game", "ohl-save", "ohl-ui"],
    ),
    (COMPOSITION_ROOT, &[]),
];

/// One disallowed dependency edge.
#[derive(Debug, PartialEq, Eq)]
pub enum GraphViolation {
    /// `from` depends on `to`, which is not among its allowed edges.
    DisallowedEdge { from: String, to: String },
    /// `crate_name` exists under `crates/` but is not named in
    /// [`ALLOWED_EDGES`] at all.
    UnknownCrate { crate_name: String },
}

impl fmt::Display for GraphViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisallowedEdge { from, to } => {
                write!(f, "{from} -> {to} is not an allowed crate dependency edge")
            }
            Self::UnknownCrate { crate_name } => {
                write!(
                    f,
                    "crate `{crate_name}` is not listed in the allowed dependency-edge table"
                )
            }
        }
    }
}

fn known_crate_names() -> Vec<&'static str> {
    ALLOWED_EDGES.iter().map(|(name, _)| *name).collect()
}

/// Validates `dependencies` (crate name -> direct intra-workspace
/// dependency names) against [`ALLOWED_EDGES`].
#[must_use]
pub fn check_edges(dependencies: &BTreeMap<String, Vec<String>>) -> Vec<GraphViolation> {
    let known = known_crate_names();
    let mut violations = Vec::new();

    for (crate_name, deps) in dependencies {
        let Some((_, allowed)) = ALLOWED_EDGES.iter().find(|(name, _)| *name == crate_name) else {
            violations.push(GraphViolation::UnknownCrate {
                crate_name: crate_name.clone(),
            });
            continue;
        };

        for dep in deps {
            let is_allowed = if crate_name == COMPOSITION_ROOT {
                dep != crate_name && known.contains(&dep.as_str())
            } else {
                allowed.contains(&dep.as_str())
            };
            if !is_allowed {
                violations.push(GraphViolation::DisallowedEdge {
                    from: crate_name.clone(),
                    to: dep.clone(),
                });
            }
        }
    }

    violations
}

/// Errors reading or parsing a crate's `Cargo.toml`.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("failed to read {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}", path = path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("{path} is missing [package].name", path = path.display())]
    MissingPackageName { path: PathBuf },
}

fn parse_crate(cargo_toml_path: &Path) -> Result<(String, Vec<String>), DiscoveryError> {
    let content =
        std::fs::read_to_string(cargo_toml_path).map_err(|source| DiscoveryError::Io {
            path: cargo_toml_path.to_path_buf(),
            source,
        })?;
    let document: toml::Table = content.parse().map_err(|source| DiscoveryError::Toml {
        path: cargo_toml_path.to_path_buf(),
        source: Box::new(source),
    })?;

    let name = document
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| DiscoveryError::MissingPackageName {
            path: cargo_toml_path.to_path_buf(),
        })?
        .to_string();

    let mut dependencies = Vec::new();
    if let Some(table) = document.get("dependencies").and_then(toml::Value::as_table) {
        for (dependency_name, dependency_value) in table {
            let is_path_dependency = dependency_value
                .as_table()
                .is_some_and(|dependency_table| dependency_table.contains_key("path"));
            if is_path_dependency {
                dependencies.push(dependency_name.clone());
            }
        }
    }

    Ok((name, dependencies))
}

/// Reads every `crates/*/Cargo.toml` under `workspace_root` and returns each
/// crate's direct intra-workspace path dependencies. Crates not yet created
/// are simply absent from the result.
pub fn discover_dependencies(
    workspace_root: &Path,
) -> Result<BTreeMap<String, Vec<String>>, DiscoveryError> {
    let crates_dir = workspace_root.join("crates");
    let mut dependencies = BTreeMap::new();
    if !crates_dir.is_dir() {
        return Ok(dependencies);
    }

    let entries = std::fs::read_dir(&crates_dir).map_err(|source| DiscoveryError::Io {
        path: crates_dir.clone(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| DiscoveryError::Io {
            path: crates_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let cargo_toml = path.join("Cargo.toml");
        if !cargo_toml.is_file() {
            continue;
        }
        let (name, deps) = parse_crate(&cargo_toml)?;
        dependencies.insert(name, deps);
    }

    Ok(dependencies)
}

#[cfg(test)]
mod tests {
    use super::{COMPOSITION_ROOT, GraphViolation, check_edges, discover_dependencies};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    #[test]
    fn allows_edges_from_the_architecture_table() {
        let mut deps = BTreeMap::new();
        deps.insert("ohl-vfs".to_string(), vec!["ohl-platform".to_string()]);
        deps.insert("ohl-platform".to_string(), vec!["ohl-core".to_string()]);
        deps.insert("ohl-core".to_string(), vec![]);
        assert!(check_edges(&deps).is_empty());
    }

    #[test]
    fn rejects_an_edge_not_in_the_table() {
        let mut deps = BTreeMap::new();
        deps.insert("ohl-core".to_string(), vec!["ohl-vfs".to_string()]);
        let violations = check_edges(&deps);
        assert_eq!(
            violations,
            vec![GraphViolation::DisallowedEdge {
                from: "ohl-core".to_string(),
                to: "ohl-vfs".to_string(),
            }]
        );
    }

    #[test]
    fn allows_the_media_archive_edges_the_readers_need() {
        let mut deps = BTreeMap::new();
        deps.insert(
            "ohl-iso9660".to_string(),
            vec!["ohl-core".to_string(), "ohl-media-archive".to_string()],
        );
        deps.insert(
            "ohl-udf".to_string(),
            vec!["ohl-core".to_string(), "ohl-media-archive".to_string()],
        );
        deps.insert(
            "ohl-media-archive".to_string(),
            vec!["ohl-core".to_string()],
        );
        assert!(check_edges(&deps).is_empty());
    }

    #[test]
    fn rejects_an_edge_between_the_two_media_readers() {
        let mut deps = BTreeMap::new();
        deps.insert("ohl-udf".to_string(), vec!["ohl-iso9660".to_string()]);
        assert_eq!(check_edges(&deps).len(), 1);
    }

    #[test]
    fn rejects_the_forbidden_vfs_to_media_edge() {
        let mut deps = BTreeMap::new();
        deps.insert("ohl-vfs".to_string(), vec!["ohl-media".to_string()]);
        let violations = check_edges(&deps);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn rejects_a_crate_absent_from_the_table() {
        let mut deps = BTreeMap::new();
        deps.insert("ohl-mystery".to_string(), vec![]);
        let violations = check_edges(&deps);
        assert_eq!(
            violations,
            vec![GraphViolation::UnknownCrate {
                crate_name: "ohl-mystery".to_string(),
            }]
        );
    }

    #[test]
    fn composition_root_may_depend_on_any_known_crate() {
        let mut deps = BTreeMap::new();
        deps.insert(
            COMPOSITION_ROOT.to_string(),
            vec!["ohl-core".to_string(), "ohl-world".to_string()],
        );
        assert!(check_edges(&deps).is_empty());
    }

    #[test]
    fn composition_root_cannot_depend_on_an_unknown_crate() {
        let mut deps = BTreeMap::new();
        deps.insert(
            COMPOSITION_ROOT.to_string(),
            vec!["ohl-mystery".to_string()],
        );
        assert_eq!(check_edges(&deps).len(), 1);
    }

    #[test]
    fn every_crate_under_crates_appears_in_the_allowed_edge_table() {
        // Discovers the crates actually tracked under `crates/` in this
        // checkout and asserts each one is a key in `ALLOWED_EDGES`, so a new
        // crate that forgets to update the table fails here instead of
        // silently passing `check_edges` as an "unknown crate" only when
        // something happens to depend on it.
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask lives one directory below the workspace root");
        let dependencies =
            discover_dependencies(workspace_root).expect("real crate manifests parse");
        assert!(
            !dependencies.is_empty(),
            "expected to discover at least one crate under crates/"
        );
        let known = super::known_crate_names();
        let missing: Vec<&String> = dependencies
            .keys()
            .filter(|name| !known.contains(&name.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "crate(s) under crates/ missing from ALLOWED_EDGES: {missing:?}"
        );
    }

    #[test]
    fn discovers_dependencies_from_real_crate_manifests() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("crates/ohl-core")).unwrap();
        fs::write(
            dir.path().join("crates/ohl-core/Cargo.toml"),
            "[package]\nname = \"ohl-core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("crates/ohl-platform")).unwrap();
        fs::write(
            dir.path().join("crates/ohl-platform/Cargo.toml"),
            "[package]\nname = \"ohl-platform\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nohl-core = { path = \"../ohl-core\" }\n",
        )
        .unwrap();

        let dependencies = discover_dependencies(dir.path()).expect("discovery succeeds");
        assert_eq!(dependencies.get("ohl-core"), Some(&Vec::new()));
        assert_eq!(
            dependencies.get("ohl-platform"),
            Some(&vec!["ohl-core".to_string()])
        );
        assert!(check_edges(&dependencies).is_empty());
    }
}
