//! Clean-room reader for Wise Installation System packages.
//!
//! A Wise package is an ordinary PE (or NE) executable with data appended
//! after its last section. That overlay begins with a Wise-specific header,
//! after which raw DEFLATE streams follow one another to end of file, each
//! terminated by a CRC-32 of its own inflated bytes. The first inflated
//! stream is a device-independent bitmap used by the installer's user
//! interface and is skipped; the second is a script binary carrying, among
//! much that is undocumented, per-file records with a destination path,
//! compressed start and end offsets, an inflated size and a CRC-32.
//!
//! # Provenance
//!
//! This crate is written **only** from public documentation, listed with
//! sections and terms in `docs/FORMAT_SOURCES.md` under "Wise Installation
//! System packages": the Just Solve "Wise installer package" page (CC0), the
//! prose and field tables of the REWise README, the `rewise(1)` manual page,
//! the exwise 0.5 README, RFC 1951 for DEFLATE (decoded by `miniz_oxide`) and
//! the published CRC-32 definition (ITU-T V.42 / RFC 1952 section 8), which
//! is implemented here from the polynomial. No implementation source, no
//! ImHex pattern and no decompiled code was read or copied. `THIRD_PARTY_NOTICES.md`
//! records that this crate contains no third-party code.
//!
//! # Design rules
//!
//! - Nothing here opens a path, names a file, or executes anything: the
//!   caller supplies bytes through [`ImageSource`] and receives bytes through
//!   [`Sink`].
//! - Every walk is bounded by a caller-supplied [`Limits`] and can be
//!   cancelled between chunks through [`Cancellation`].
//! - Every failure is a fixed [`Error`] code that carries no media-derived
//!   bytes; destination paths are exposed as [`PathBytes`], whose `Debug`
//!   prints only a length.
//! - The crate is `#![no_std]` plus `alloc` and `#![forbid(unsafe_code)]`, so
//!   it can be linked into the freestanding parser worker unchanged.
//!
//! # Coverage
//!
//! Supported: PE stubs whose overlay is located from the section table (or
//! supplied by the caller), the non-`zip` overlay layout, the stream chain
//! with bounded resynchronisation, and the documented per-file script record.
//! Deliberately unsupported, and rejected rather than guessed at: the
//! `zip`-enabled variant ([`Error::ZipVariantUnsupported`]), NE stubs,
//! multi-disc continuations, patch operations, and every script opcode — see
//! [`script`] for exactly what is and is not decoded.
//!
//! ```
//! use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};
//! use ohl_wise::{Limits, NeverCancelled, SliceSource, read_package};
//!
//! let built = build_package(&PackageOptions::with_files(vec![SyntheticFile::new(
//!     b"dir\\one.dat",
//!     vec![7u8; 512],
//! )]));
//! let mut source = SliceSource::new(&built.image);
//! let package = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled)?;
//! assert_eq!(package.summary().streams, 3);
//! assert_eq!(package.file_table().len(), 1);
//! # Ok::<(), ohl_wise::Error>(())
//! ```
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod chain;
pub mod crc32;
pub mod error;
pub mod extract;
pub mod header;
pub mod limits;
pub mod overlay;
pub mod package;
pub mod script;
pub mod source;
pub mod stream;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use chain::{Chain, ChainEvent, StreamRecord};
pub use crc32::{Crc32, crc32};
pub use error::{Error, Limit};
pub use extract::{Entry, FileMap, FileReader, MatchKind, OffsetOrigin};
pub use header::{PackageHeader, ZIP_LOCAL_FILE_SIGNATURE, locate_first_stream};
pub use limits::{DEFAULT_CHUNK_BYTES, DEFAULT_HEADER_SCAN_BYTES, Limits};
pub use overlay::{DOS_SIGNATURE, Overlay, PE_SIGNATURE, find_overlay, overlay_at};
pub use package::{ChainSummary, Package, read_package};
pub use script::{FileRecord, FileTable, PathBytes, StreamEvidence};
pub use source::{Cancellation, Discard, ImageSource, NeverCancelled, Sink, SliceSource};
pub use stream::{ChecksumStatus, StreamMetrics, StreamReader};
