//! Standard-library test worker: static musl on Linux and libSystem on macOS.
#![allow(unsafe_code)]

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
mod hosted;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_probes;

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
fn main() -> ! {
    hosted::main()
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")))]
fn main() -> ! {
    std::process::exit(90)
}
