//! Require the fixed Linux runtime target. The image builder supplies its
//! static relocation and link flags together; a stray `--target` override
//! (e.g. a `-gnu` triple) would otherwise only be caught later by the
//! downstream image audit instead of failing the build itself.
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
