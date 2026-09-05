//! `cargo xtask`: repository policy and crate-graph checks.
//!
//! - `cargo xtask policy` reimplements the former C++ build's `cmake/CheckRepository.cmake` tracked-file policy (removed with the C++ tree at M1-rs parity).
//! - `cargo xtask graph` validates the crate dependency graph against the
//!   allowed edges from `.plan/rust-architecture-r1.md` section 1.
//! - `cargo xtask worker-image` builds the freestanding isolated-worker test
//!   image and the shipping media-parser worker image, proves each is a
//!   static, non-interpreted `ET_EXEC` binary, and installs the latter at
//!   `<target>/<profile>/libexec/open-half-life/ohl-media-parser-worker`.
//! - `cargo xtask dist` builds the release binary (and, on Linux x86-64, the
//!   parser worker image) and assembles a versioned, self-contained release
//!   folder plus a `.tar.gz`/`.zip` archive under `target/dist/`.

mod dist;
mod graph;
mod policy;
mod worker_image;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The workspace root, derived from this crate's manifest directory
/// (`<root>/xtask`).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one directory below the workspace root")
        .to_path_buf()
}

fn run_policy(root: &Path) -> ExitCode {
    match policy::run(root) {
        Ok(violations) if violations.is_empty() => {
            println!("Tracked-file policy check passed");
            ExitCode::SUCCESS
        }
        Ok(violations) => {
            for violation in &violations {
                eprintln!("error: {violation}");
            }
            eprintln!(
                "Tracked-file policy check failed ({} violation(s))",
                violations.len()
            );
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_graph(root: &Path) -> ExitCode {
    let dependencies = match graph::discover_dependencies(root) {
        Ok(dependencies) => dependencies,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    let violations = graph::check_edges(&dependencies);
    if violations.is_empty() {
        println!(
            "Crate dependency graph check passed ({} crate(s) checked)",
            dependencies.len()
        );
        ExitCode::SUCCESS
    } else {
        for violation in &violations {
            eprintln!("error: {violation}");
        }
        eprintln!(
            "Crate dependency graph check failed ({} violation(s))",
            violations.len()
        );
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let root = workspace_root();
    let subcommand = std::env::args().nth(1);
    match subcommand.as_deref() {
        Some("policy") => run_policy(&root),
        Some("graph") => run_graph(&root),
        Some("worker-image") => worker_image::run(),
        Some("dist") => dist::run(&root, &std::env::args().skip(2).collect::<Vec<_>>()),
        other => {
            eprintln!("usage: cargo xtask <policy|graph|worker-image|dist>");
            if let Some(other) = other {
                eprintln!("unknown subcommand: {other}");
            }
            ExitCode::FAILURE
        }
    }
}
