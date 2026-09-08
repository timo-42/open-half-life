//! Link configuration for the hosted media-parser worker image.
//!
//! The image must be a static, non-PIE `ET_EXEC` ELF with no `PT_INTERP` and
//! no `PT_DYNAMIC`, because `ohl-platform`'s isolated-worker backend verifies
//! exactly that before it is willing to `execveat(2)` the file.
//!
//! Emitting the flags from a build script keeps them attached to this package
//! instead of leaking into a `.cargo/config.toml` that Cargo would apply to
//! every crate built from the same working directory, and means no global
//! `RUSTFLAGS` is ever required: `cargo build --workspace` on any host builds
//! the workspace, and only `cargo xtask worker-image` (or the crate's own
//! builder) reaches this package.
//!
//! The default `cc` linker driver is used - the same one every other binary
//! in the repository already links with. The Rust standard library comes from
//! the explicitly selected musl target; `-static` and `-no-pie` make its
//! normal Rust entry point an `ET_EXEC`, while `--build-id=none` keeps the
//! image reproducible.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // The macOS image is an ordinary dynamically linked `std` binary. Only
    // the Linux x86-64 musl target needs these image-local link arguments.
    // `CARGO_CFG_*` describe the *target* of this build, which is what
    // matters here.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_os != "linux" || target_arch != "x86_64" {
        return;
    }
    for argument in ["-static", "-no-pie", "-Wl,--build-id=none"] {
        println!("cargo::rustc-link-arg-bins={argument}");
    }
}
