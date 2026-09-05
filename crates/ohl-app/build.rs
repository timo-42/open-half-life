//! Stamps the binary version from `OHL_VERSION` (set by CI from `vergit`, the
//! same mechanism `src/app/CMakeLists.txt` uses for the C++ build via
//! `OHL_VERSION_OVERRIDE`), falling back to `CARGO_PKG_VERSION` for local
//! developer builds.

fn main() {
    let version = std::env::var("OHL_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").expect("cargo sets this"));
    println!("cargo::rustc-env=OHL_APP_VERSION={version}");
    println!("cargo::rerun-if-env-changed=OHL_VERSION");
}
