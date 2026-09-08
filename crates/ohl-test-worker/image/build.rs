//! Require the fixed Linux runtime target. The image builder supplies its
//! static relocation and link flags together; direct tests use Cargo defaults.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("linux") {
        assert_eq!(
            target, "x86_64-unknown-linux-musl",
            "Linux worker requires the fixed musl target"
        );
    }
}
