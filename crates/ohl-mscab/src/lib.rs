//! Clean-room, bounds-checked reader for the Microsoft Cabinet (MS-CAB)
//! container.
//!
//! The crate is `#![no_std]` plus `alloc` so it links unchanged into the
//! sandboxed parser worker as well as hosted binaries, links no `unsafe` code
//! (enforced workspace-wide), never opens a path, and never panics on
//! malformed input: every fallible operation returns a fixed [`CabError`]
//! that carries no cabinet-derived bytes.
//!
//! # Scope
//!
//! * `CFHEADER` / `CFFOLDER` / `CFFILE` / `CFDATA` parsing, with the
//!   documented `CFDATA` checksum verified whenever it is supplied.
//! * Decompression of uncompressed (`tcompTYPE_NONE`), MSZIP and LZX folders.
//!   Quantum is out of scope and reports [`CabError::Unsupported`].
//! * Streaming extraction with bounded buffers and a cancellation check
//!   between data blocks.
//!
//! Byte access goes through the caller-supplied [`VolumeSource`]; a cabinet
//! *set* whose folder continues into another volume is expressed as a list of
//! [`FolderSegment`]s that the caller assembles, because only the caller knows
//! how `CFHEADER.iCabinet` and the next-cabinet name map onto its volumes.
//!
//! # Provenance
//!
//! Implemented only from Microsoft's public documentation of the format:
//! "Microsoft Cabinet Format", "Microsoft LZX Data Compression Format",
//! [MS-MCI] and [MS-PATCH], plus [RFC1951] for DEFLATE. See
//! `docs/FORMAT_SOURCES.md`. No other implementation was consulted, and every
//! test fixture is synthesised by this crate's own writer.
//!
//! ```
//! use ohl_mscab::{Cabinet, FolderStream, Limits, NeverCancelled, SliceSource};
//! # fn demo(bytes: &[u8]) -> Result<(), ohl_mscab::CabError> {
//! let source = SliceSource::new(bytes);
//! let cabinet = Cabinet::parse(&source, 0, &Limits::default())?;
//! let mut stream = FolderStream::from_cabinet(&cabinet, &source, 0, 0, Limits::default())?;
//! let mut buffer = [0u8; 4096];
//! while stream.read(&mut buffer, &NeverCancelled)? > 0 {}
//! # Ok(())
//! # }
//! ```
#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

mod bytes;
mod cabinet;
mod data;
mod error;
mod extract;
mod limits;
pub mod lzx;
mod mszip;
mod source;

#[cfg(feature = "test-support")]
pub mod lzx_writer;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use cabinet::{
    ATTR_ARCHIVE, ATTR_EXEC, ATTR_HIDDEN, ATTR_NAME_IS_UTF, ATTR_READ_ONLY, ATTR_SYSTEM, Cabinet,
    Compression, DateTime, FLAG_NEXT_CABINET, FLAG_PREV_CABINET, FLAG_RESERVE_PRESENT, FileEntry,
    Folder, FolderRef, Header, IFOLD_CONTINUED_FROM_PREV, IFOLD_CONTINUED_PREV_AND_NEXT,
    IFOLD_CONTINUED_TO_NEXT, SIGNATURE,
};
pub use data::checksum;
pub use error::{CabError, Result};
pub use extract::{ExtractionStats, FileStream, FolderSegment, FolderStream, extract_file};
pub use limits::{Limits, MAX_BLOCK_COMPRESSED, MAX_BLOCK_UNCOMPRESSED};
pub use mszip::SIGNATURE as MSZIP_SIGNATURE;
pub use source::{Cancellation, NeverCancelled, SliceSetSource, SliceSource, VolumeSource};
