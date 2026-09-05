//! Shared, dependency-free foundations for the Open Half-Life Rust port:
//! sanitized diagnostics, bounded arithmetic, and SHA-256.
//!
//! This crate is `#![no_std]` plus an optional `std` feature so it can
//! compile unchanged into the freestanding parser worker as well as hosted
//! binaries. It never links `unsafe` code (enforced workspace-wide by
//! `[workspace.lints.rust] unsafe_code = "forbid"`).
//!
//! `cfg(test)` additionally enables `std` so the unit test harness links
//! normally; production `no_std` builds are unaffected.
#![cfg_attr(not(any(test, feature = "std")), no_std)]

pub mod checked;
pub mod error;
pub mod hash;

pub use checked::CheckedArithmetic;
pub use error::SanitizedError;
pub use hash::StreamingSha256;

/// The crate version, taken from `OHL_VERSION` at build time when supplied
/// (see `build.rs`), falling back to `CARGO_PKG_VERSION`.
pub const VERSION: &str = env!("OHL_CORE_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_non_empty() {
        assert!(!super::VERSION.is_empty());
    }
}
