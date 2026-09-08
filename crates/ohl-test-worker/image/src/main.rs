//! The isolated-worker test image, in its two native shapes.
//!
//! - On Linux x86-64 this is the freestanding `#![no_std] #![no_main]`
//!   image of [`freestanding`]: raw syscalls only, so the host's seccomp
//!   allowlist is sufficient and any other syscall is a genuine policy
//!   violation.
//! - On macOS it is the hosted `std` image of [`hosted`]: an ordinary
//!   binary linked against libSystem, which the host's macOS backend runs
//!   under the system sandbox. The same wire contract, the same modes.
//!
//! The crate-level attributes are conditional so one package (and one
//! `Cargo.lock`) serves both; `build.rs` emits the freestanding link
//! arguments only for the Linux x86-64 target.
#![cfg_attr(all(target_os = "linux", target_arch = "x86_64"), no_std, no_main)]
#![allow(unsafe_code)]

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod freestanding;

#[cfg(target_os = "macos")]
mod hosted;

#[cfg(target_os = "macos")]
fn main() -> ! {
    hosted::main()
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos")))]
fn main() -> ! {
    // No backend can launch this image here; the builder refuses to build it
    // (`BuildError::Unsupported`) before this is ever reached.
    std::process::exit(90)
}
