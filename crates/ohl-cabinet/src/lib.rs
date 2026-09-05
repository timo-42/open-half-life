//! Streaming InstallShield cabinet extraction over a caller-supplied volume
//! source.
//!
//! # Licensed derivative — not clean-room
//!
//! This crate is a Rust **translation of the MIT-licensed C library
//! Unshield** (<https://github.com/twogood/unshield>, commit `51de441`,
//! version 1.6.2, © David Eriksson and contributors). Its knowledge of the
//! InstallShield 5/6/2003 cabinet container — volume headers, the
//! length-prefixed raw DEFLATE chunk framing, the obfuscation keystream and
//! the split-volume rules — comes from that implementation, so it is a
//! licensed derivative work and **is not clean-room**. See
//! `LICENSE-UNSHIELD`, `THIRD_PARTY_NOTICES.md` and the "Cabinet" section of
//! `docs/FORMAT_SOURCES.md`.
//!
//! ## Isolation rule
//!
//! - This crate and `ohl-cabinet-format` are leaves of the dependency graph
//!   and may only be linked into the sandboxed parser worker. Nothing else in
//!   the workspace may depend on them.
//! - Their format knowledge must never be used as a source for, copied into,
//!   restated in, or cited by project-owned parsing code, documentation or
//!   tests. The finding in `docs/FORMAT_SOURCES.md` that project-owned
//!   cabinet parsing **may not begin** without an independent public
//!   specification is unaffected by this crate's existence.
//!
//! # Hardening beyond the reference implementation
//!
//! The reference implementation opens files itself from a `printf`-style
//! filename pattern, scans directories case-insensitively, and reads
//! unbounded amounts into fixed buffers. Here the crate never opens a path or
//! builds a filename: the caller maps a volume number to bytes through
//! [`VolumeSource`], every read is bounded by [`Limits`], split-volume and
//! split-link walks are cycle-checked, and the crate is `#![no_std]` plus
//! `alloc` and `#![forbid(unsafe_code)]`.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod error;
pub mod extract;
pub mod limits;
pub mod obfuscation;
pub mod volume;

#[cfg(feature = "inflate")]
mod inflate;

#[cfg(feature = "test-support")]
pub mod testing;

pub use error::{Error, Limit, VolumeError};
pub use extract::{CabinetReader, FileReader};
pub use limits::{DEFAULT_CHUNK_BYTES, Limits};
pub use obfuscation::{deobfuscate, obfuscate};
pub use volume::{
    NO_LAST_FILE_OFFSET, VOLUME_HEADER_SIZE_V5, VOLUME_HEADER_SIZE_V6, VolumeHeader, VolumeSource,
};

/// Re-export of the header parser this crate extracts against.
pub use ohl_cabinet_format as format;
