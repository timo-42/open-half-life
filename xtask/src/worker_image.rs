//! `cargo xtask worker-image`: build and audit the freestanding worker image.
//!
//! The Linux isolated-worker backend refuses to execute anything that is not
//! a statically linked, non-interpreted `ET_EXEC` x86-64 ELF, so a regression
//! in the image's link configuration would show up as an opaque
//! `ServiceIdentityMismatch` at launch time. This command turns that into an
//! explicit, standalone check with a readable failure.

use std::process::ExitCode;

use ohl_test_worker::{
    EM_X86_64, ET_EXEC, TestWorkerVariant, build_test_worker_image, summarise_elf,
};

/// Builds every image variant and verifies its ELF identity.
pub fn run() -> ExitCode {
    let variants = [TestWorkerVariant::Ready, TestWorkerVariant::NeverReady];
    let mut failures = 0usize;

    for variant in variants {
        let path = match build_test_worker_image(variant) {
            Ok(path) => path,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("error: failed to read {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let Some(summary) = summarise_elf(&bytes) else {
            eprintln!(
                "error: {} is not an ELF64 little-endian file",
                path.display()
            );
            failures += 1;
            continue;
        };

        for (condition, detail) in [
            (summary.object_type == ET_EXEC, "must be ET_EXEC"),
            (summary.machine == EM_X86_64, "must target x86-64"),
            (!summary.has_interpreter, "must have no PT_INTERP"),
            (!summary.has_dynamic, "must have no PT_DYNAMIC"),
        ] {
            if !condition {
                eprintln!("error: {} {detail}", path.display());
                failures += 1;
            }
        }
        println!("{variant:?}: {} ({} bytes)", path.display(), bytes.len());
    }

    if failures == 0 {
        println!("Worker image check passed ({} variant(s))", variants.len());
        ExitCode::SUCCESS
    } else {
        eprintln!("Worker image check failed ({failures} violation(s))");
        ExitCode::FAILURE
    }
}
