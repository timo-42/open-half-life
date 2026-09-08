//! Keep the Linux fixture identical to the production static musl ELF shape.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("linux") {
        assert_eq!(
            target, "x86_64-unknown-linux-musl",
            "Linux worker requires the fixed musl target"
        );
        for argument in ["-static", "-no-pie", "-Wl,--build-id=none"] {
            println!("cargo::rustc-link-arg-bins={argument}");
        }
    }
}
