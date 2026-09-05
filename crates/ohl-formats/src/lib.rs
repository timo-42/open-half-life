//! Clean-room, `no_std` + `alloc` decoders for GoldSrc BSP v30 maps and
//! WAD3 texture packages.
//!
//! Every decoder in this crate is a borrowing, zero-copy view over caller-
//! supplied bytes: no lump, entry, or count is ever trusted without first
//! validating it against the actual buffer, and no accessor panics on
//! malformed input — every fallible operation returns [`error::FormatError`]
//! instead. See `docs/FORMAT_SOURCES.md` ("GoldSrc BSP v30 and WAD3") for the
//! public documentation this crate was implemented from, and
//! `docs/CLEAN_ROOM.md` for the project's clean-room policy.
#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

pub mod bsp30;
pub mod error;
pub mod palette;
pub mod wad3;

mod miptex_body;
mod util;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use error::FormatError;
pub use palette::Rgb8;
