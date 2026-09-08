//! The media-parser worker image, in its two native shapes.
//!
//! - On Linux x86-64 this is the freestanding `#![no_std] #![no_main]` image
//!   of [`freestanding`]: raw syscalls, a fixed `.bss` arena, no libc, so
//!   `ohl-platform`'s seccomp allowlist is sufficient and any other syscall
//!   is a genuine policy violation.
//! - On macOS it is the hosted `std` image of [`hosted`]: an ordinary binary
//!   linked against libSystem and nothing else, which `ohl-platform`'s macOS
//!   backend runs under the system sandbox with a self-imposed heap ceiling.
//!
//! Both host exactly one `run_parser_worker_service` lifetime over
//! descriptor 3 with `ohl_parser_backends::ContainerDispatcher`, attest
//! readiness on descriptor 4, parse no arguments, read no environment, open
//! no file, and exit with the statuses in `contract.rs`.
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
    // (`BuildError::Unsupported`) before this is ever reached. The status is
    // the contract's "unsupported" exit.
    std::process::exit(65)
}
