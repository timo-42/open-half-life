//! Shared read-only media-archive vocabulary.
//!
//! Both project-owned media readers (`ohl-iso9660` for ECMA-119 volumes and
//! `ohl-udf` for ECMA-167 volumes) expose the same block source, the same
//! bounded directory-listing model, the same path rules, and the same fixed
//! classification vocabulary. Keeping that vocabulary in one tiny crate lets
//! `ohl-vfs` treat both media classes uniformly without either reader
//! depending on the other.
//!
//! Nothing here performs I/O against an operating system: a caller supplies a
//! [`BlockReader`], which is the only way bytes ever enter these crates. No
//! type in this crate carries media-derived bytes in a diagnostic; every
//! failure is an `ohl_core::SanitizedError`.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod archive;
pub mod block;
pub mod class;
pub mod limits;
pub mod listing;
pub mod path;

pub use archive::{MediaArchive, MediaFileHandle};
pub use block::{BLOCK_SIZE, BLOCK_SIZE_U32, BLOCK_SIZE_U64, Block, BlockReader};
pub use class::{FilesystemDescription, MediaClass, MediaPreflight, VolumeLabel};
pub use limits::DirectoryLimits;
pub use listing::{DirectoryCursor, DirectoryEntry, DirectoryPage, EntryType, MountId};
pub use path::{is_single_path_component, normalize_path};
