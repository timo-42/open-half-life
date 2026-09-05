//! Bounded ECMA-167 NSR02 preflight and a read-only UDF archive.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod adaptor;
pub mod archive;
pub mod preflight;
#[cfg(test)]
pub mod test_support;

pub use adaptor::BlockCursor;
pub use archive::{UdfArchive, UdfFile};
pub use preflight::preflight;

#[cfg(test)]
mod tests;
