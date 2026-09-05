//! Clean-room, bounds-checked decoding of InstallShield 3 "Z" archives and of
//! the PKWARE Data Compression Library "imploded" streams they contain.
//!
//! # Provenance
//!
//! This crate is written from public documentation only: the Archive Team
//! file-format wiki pages "InstallShield Z" and "PKWARE DCL Implode", the
//! `ISArchiveV3.h` structure description the former links, and the format
//! description distributed with Mark Adler's `blast` in `zlib/contrib`
//! (itself based on Ben Rudiak-Gould's 2001 comp.compression description).
//! No implementation's source code was translated. See
//! `docs/FORMAT_SOURCES.md` for the full record, the adopt/translate/write
//! decision and its evidence, and `LICENSE-BLAST` for the notice covering the
//! one artefact taken from a licensed source: the format's three fixed
//! Huffman codebooks, which are data about the format rather than code.
//!
//! # Shape
//!
//! - The archive is usually embedded in the overlay of an installer
//!   executable, so nothing assumes it starts at offset zero:
//!   [`find_signature`] scans a caller-supplied [`ArchiveSource`] in bounded
//!   windows and [`Archive::open`] parses at the resulting base offset.
//! - Every count, offset and length read out of an archive is validated
//!   against [`Limits`] before it sizes an allocation or bounds a loop.
//! - Names are returned as bounded, opaque byte strings ([`Name`]) whose
//!   `Debug` output is redacted; this crate never logs one.
//! - Extraction streams bounded chunks and polls a [`Cancellation`] token
//!   between them.
//! - Every failure is a fixed, project-defined [`Error`] carrying nothing
//!   archive-derived.
//!
//! # Unsupported
//!
//! Multi-volume ("split") archives are recognised and rejected, not decoded.
//! The header records a volume count, a volume number and a split address
//! range, and each entry records a split flag plus its first and last volume;
//! [`Archive::open`] refuses an archive whose header carries any of those
//! markers, and [`Archive::open_entry`] refuses an entry that claims bytes in
//! more than one volume. Supporting them would need a volume-number-to-bytes
//! mapping that this crate deliberately does not have.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod bytes;

pub mod archive;
pub mod error;
pub mod explode;
pub mod header;
pub mod limits;
pub mod source;
pub mod toc;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use archive::{Archive, EntryReader};
pub use error::{Error, Limit, Result, SourceError};
pub use explode::{Explode, MAX_MATCH, MAX_WINDOW, Progress, explode_to_vec};
pub use header::{ArchiveHeader, DATA_START, HEADER_SIZE, SIGNATURE};
pub use limits::{DEFAULT_CHUNK_BYTES, Limits};
pub use source::{
    ArchiveSource, Cancellation, NeverCancelled, SliceSource, find_signature, find_signature_from,
};
pub use toc::{ATTRIBUTE_UNCOMPRESSED, Directory, Entry, Name, TableOfContents};
