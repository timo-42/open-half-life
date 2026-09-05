//! InstallShield cabinet header, descriptor and table parsing.
//!
//! # Licensed derivative — not clean-room
//!
//! This crate is a Rust **translation of the MIT-licensed C library
//! Unshield** (<https://github.com/twogood/unshield>, commit `51de441`,
//! version 1.6.2, © David Eriksson and contributors). Its knowledge of the
//! InstallShield 5/6/2003 cabinet container comes from that implementation,
//! so it is a licensed derivative work and **is not clean-room**. See
//! `LICENSE-UNSHIELD`, `THIRD_PARTY_NOTICES.md` and the "Cabinet" section of
//! `docs/FORMAT_SOURCES.md`.
//!
//! ## Isolation rule
//!
//! - This crate and [`ohl-cabinet`](https://docs.rs/ohl-cabinet) are leaves of
//!   the dependency graph and may only be linked into the sandboxed parser
//!   worker. Nothing else in the workspace may depend on them.
//! - Their format knowledge must never be used as a source for, copied into,
//!   restated in, or cited by project-owned parsing code, documentation or
//!   tests. The finding in `docs/FORMAT_SOURCES.md` that project-owned
//!   cabinet parsing **may not begin** without an independent public
//!   specification is unaffected by this crate's existence.
//!
//! # Hardening beyond the reference implementation
//!
//! Unshield walks its header with raw pointers and performs no bounds checks:
//! `unshield_header_get_buffer` adds an untrusted 32-bit offset to a base
//! pointer, `unshield_create_filename_pattern` writes into a fixed 256-byte
//! buffer, and the two 71-entry offset arrays are read without validating the
//! descriptor length. Here every offset, count and length is validated
//! against both the supplied buffer and a caller-supplied [`Limits`], the
//! crate is `#![no_std]` plus `alloc`, it never opens a path or builds a
//! filename, and it is `#![forbid(unsafe_code)]`.

#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod bytes;
pub mod common;
pub mod descriptor;
pub mod error;
pub mod file;
pub mod header;
pub mod limits;
pub mod strings;
pub mod tables;
pub mod version;

#[cfg(feature = "test-support")]
pub mod testing;

pub use common::{CAB_SIGNATURE, COMMON_HEADER_SIZE, CommonHeader, MSCF_SIGNATURE};
pub use descriptor::{CabDescriptor, MIN_CAB_DESCRIPTOR_SIZE, OFFSET_COUNT};
pub use error::{FormatError, Limit};
pub use file::{
    FILE_COMPRESSED, FILE_INVALID, FILE_OBFUSCATED, FILE_SPLIT, FileDescriptor, FileFlags,
    LINK_NEXT, LINK_NONE, LINK_PREV, LinkFlags,
};
pub use header::CabinetHeader;
pub use limits::Limits;
pub use tables::{Component, DirectoryIter, FileDescriptorIter, FileGroup};
pub use version::{Layout, UNICODE_MAJOR_VERSION, Version, VersionEncoding};
