//! Link configuration for the freestanding worker image.
//!
//! The image must be a static, non-PIE `ET_EXEC` ELF with no `PT_INTERP` and
//! no `PT_DYNAMIC`, because the host backend verifies exactly that before it
//! is willing to `execveat(2)` the file. Emitting the flags from a build
//! script keeps them attached to this package instead of leaking into a
//! `.cargo/config.toml` that Cargo would apply to every crate built from the
//! same working directory.
//!
//! The default `cc` linker driver is used (the same one every other binary in
//! the repository already links with); `-nostdlib` removes the C runtime
//! start files and the default libraries, so no C library is linked in.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for argument in [
        "-nostdlib",
        "-static",
        "-no-pie",
        "-Wl,-e,_start",
        "-Wl,--build-id=none",
    ] {
        println!("cargo::rustc-link-arg-bins={argument}");
    }
}
