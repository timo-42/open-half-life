//! The media-parser worker image.
//!
//! Linux x86-64 and macOS share the ordinary hosted `std` implementation in
//! [`hosted`]. On Linux, the builder explicitly selects
//! `x86_64-unknown-linux-musl` and scopes static, non-PIE code generation to
//! that build. On macOS it links normally against libSystem. Both backends
//! confine the process before it starts.
//!
//! Both host exactly one `run_parser_worker_service` lifetime over
//! descriptor 3 with `ohl_parser_backends::ContainerDispatcher`, attest
//! readiness on descriptor 4, parse no arguments, read no environment, open
//! no file, and exit with the statuses in `contract.rs`.
//!
#![allow(unsafe_code)]

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod hosted;

#[cfg(target_os = "macos")]
mod hosted;

#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), target_os = "macos"))]
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
