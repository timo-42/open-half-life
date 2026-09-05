//! Read-only VFS mounts over pinned media sources.
//!
//! `ohl-vfs` is the single uniform surface the rest of the engine uses to
//! open a mounted archive. [`Mount::open`] detects the media class by running
//! the ISO 9660 preflight (`ohl_iso9660::preflight`) and then the UDF
//! preflight (`ohl_udf::preflight`) against the same pinned
//! [`ohl_platform::MediaSource`], opens the matching reader, and exposes one
//! API ([`Mount::list_page`], [`Mount::continue_list`], [`Mount::open_file`])
//! regardless of which reader answered.
//!
//! This crate is a Rust port of the C++ facade in
//! `src/vfs/include/ohl/vfs/udf_archive.hpp` and (from PR #14)
//! `src/vfs/include/ohl/vfs/media_archive.hpp`, and of the semantics recorded
//! in `docs/ARCHITECTURE.md`'s "Bounded read-only VFS" section: directory
//! enumeration is bounded and opaque, `list_page()` returns a
//! [`ohl_media_archive::DirectoryCursor`] only when another page is
//! available, cursors are valid only against the mount (or one of its
//! [`Mount::share`] clones) that produced them, and every failure is a
//! sanitized [`ohl_core::SanitizedError`] that carries no media-derived name
//! or path.
//!
//! Path normalization and the fixed classification vocabulary are not
//! reimplemented here: they are delegated entirely to `ohl-media-archive`
//! (re-exported below), so an absolute, `.`, `..`, or empty path component is
//! rejected exactly as the C++ facade rejected it.
#![forbid(unsafe_code)]

mod block_reader;
mod file;
mod mount;

pub use block_reader::{DEFAULT_VERIFY_INTERVAL_BLOCKS, MediaSourceBlockReader};
pub use file::MediaFile;
pub use mount::Mount;

pub use ohl_media_archive::{
    DirectoryCursor, DirectoryEntry, DirectoryLimits, DirectoryPage, EntryType,
    FilesystemDescription, MediaClass, VolumeLabel, is_single_path_component, normalize_path,
};
