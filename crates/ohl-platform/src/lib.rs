//! Native platform capabilities for the Open Half-Life Rust port.
//!
//! This crate owns the two native primitives the rest of the port is allowed
//! to rely on:
//!
//! - [`MediaSource`], a move-only, read-only capability for exactly one
//!   natively opened file object (see [`media_source`]);
//! - the atomic-directory publication primitives in [`atomic_directory`].
//!
//! It depends only on `ohl-core` inside the workspace, and reports every
//! failure through fixed, payload-free codes that convert into
//! [`ohl_core::SanitizedError`]. No error, `Debug`, or `Display` output
//! produced here ever contains a path, a media-derived byte, or an OS error
//! string.
//!
//! # `unsafe` inventory
//!
//! `ohl-platform` is the **only** library crate in the workspace where
//! `unsafe_code` is `allow` rather than the workspace-wide `forbid`; the
//! allowance lives in this crate's `Cargo.toml` because a `forbid` level
//! cannot be relaxed by a crate-level attribute. Every unsafe site in the
//! crate is listed here. All of them are Windows-only FFI calls; the Unix
//! backend, the atomic-directory primitives, the stability checker, and every
//! test are entirely safe code.
//!
//! | # | Site | Call | Why it is needed | Why it is sound |
//! |---|------|------|------------------|-----------------|
//! | 1 | `sys::windows::file_type` | `GetFileType` | Rejecting pipes, character devices, and unknown handle types before pinning. `std` exposes no equivalent. | The handle is borrowed from a live, owned [`std::fs::File`] for the duration of the call, so it is a valid open handle. The call takes no out-parameters and cannot fail in a way that writes memory. |
//! | 2 | `sys::windows::file_information` | `GetFileInformationByHandle` | Reading the volume serial number and the 64-bit file index, which together are the pinned native identity. The `std` accessors for these (`MetadataExt::volume_serial_number`, `MetadataExt::file_index`) are permanently unstable behind `windows_by_handle`. | Same borrowed-handle argument as above. The single out-parameter is a stack-allocated, fully zero-initialised `BY_HANDLE_FILE_INFORMATION` passed by exclusive reference; it is only read after the call reports success. |
//!
//! There is no `unsafe impl`, no raw-pointer dereference, no transmute, and
//! no unsafe trait method anywhere in the crate. The Unix backend reaches the
//! same information through safe `rustix` and `std::os::unix` wrappers.

pub mod atomic_directory;
pub mod media_source;
pub mod stability;

mod sys;

pub use atomic_directory::{AtomicDirectoryError, PublishOutcome, StagingDirectory};
pub use media_source::{MediaSource, MediaSourceError};
pub use stability::{SourceFingerprint, SourceStabilityError, verify_complete_source_stability};
