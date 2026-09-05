//! Bounded ECMA-119 preflight and a read-only ISO 9660 / Joliet archive.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod adaptor;
pub mod archive;
pub mod preflight;
#[cfg(test)]
pub mod test_support;

pub use adaptor::BlockCursor;
pub use archive::{Iso9660Archive, Iso9660File};
pub use preflight::{DescriptorGeometry, Iso9660Preflight, preflight};

#[cfg(test)]
mod tests;
