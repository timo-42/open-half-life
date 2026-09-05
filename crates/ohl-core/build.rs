//! Re-emits the crate version so `env!("OHL_CORE_VERSION")` is always
//! available in `src/lib.rs`, falling back to `CARGO_PKG_VERSION` when no
//! override is supplied by the workspace build.

fn main() {
    let version = std::env::var("OHL_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").expect("cargo sets this"));
    println!("cargo::rustc-env=OHL_CORE_VERSION={version}");
    println!("cargo::rerun-if-env-changed=OHL_VERSION");
}
