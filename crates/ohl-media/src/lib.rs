//! Media fingerprinting, the validated-media proof, and the metadata-only
//! provenance cache for the Open Half-Life Rust port.
//!
//! This crate is the M1/M2 `media` package. It answers exactly three
//! questions and deliberately answers no others:
//!
//! 1. **What is this source?** [`fingerprint`] hashes one pinned
//!    [`ohl_platform::MediaSource`] end to end with stability checks at the
//!    start, periodically, and at the end, and returns a [`MediaDigest`].
//! 2. **May we act on it?** [`ValidatedMedia`] is the move-only proof that
//!    binds a capability, its size, its digest, and the
//!    [`MediaDescription`] a preflight crate produced.
//! 3. **Has it been seen before?** [`prepare_import_cache`] reauthenticates
//!    the source and publishes, or reuses, a metadata-only provenance entry.
//!
//! # What this crate does not do
//!
//! It **does not parse media structure**. The ISO 9660 and UDF preflight
//! crates own that, and they hand their result over as a plain
//! [`MediaDescription`] value; this crate never learns how it was derived.
//! It **does not import payload**: no source byte is ever copied out of the
//! user's media, and the manifest says so with a fixed `payload_state` of
//! `not-imported`.
//!
//! # Mapping a preflight result
//!
//! The `ohl-media-archive`, `ohl-iso9660`, and `ohl-udf` crates own preflight
//! and report an `ohl_media_archive::MediaPreflight`. There is deliberately
//! no dependency edge from here to those crates, so the composition root does
//! the one-line mapping itself:
//!
//! ```text
//! MediaDescription::new(
//!     match preflight.media_class {
//!         ohl_media_archive::MediaClass::Udf => MediaClass::Udf,
//!         ohl_media_archive::MediaClass::Iso9660 => MediaClass::Iso9660,
//!     },
//!     preflight.filesystem.as_str(),   // already a project-owned constant
//!     VolumeLabel::sanitized(preflight.volume_label.as_str()),
//! )
//! ```
//!
//! Both sides cap a volume label at 32 characters of printable ASCII, so the
//! mapping is lossless.
//!
//! # Clean-room and privacy properties
//!
//! - No media bytes and no source path are persisted or logged. The only
//!   media-derived values that survive a run are the SHA-256 digest, the byte
//!   count, and a bounded printable-ASCII volume label — the sanitized-report
//!   fields `docs/MEDIA_IMPORT.md` allows.
//! - Every error is a fixed, payload-free code
//!   ([`MediaError`], [`ImportCacheError`]) that converts into
//!   [`ohl_core::SanitizedError`], so no diagnostic can interpolate a path or
//!   a media byte.
//! - [`CacheReport`] is a two-valued enum whose log lines are compile-time
//!   literals.
//! - No `unsafe` code: the crate inherits the workspace-wide
//!   `unsafe_code = "forbid"`.
//!
//! # Example
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use ohl_media::{CacheLayout, MediaClass, MediaDescription, ValidatedMedia, VolumeLabel};
//! use ohl_platform::MediaSource;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // The application acquires the source once and then discards the path.
//! let source = Arc::new(MediaSource::open(std::path::Path::new("/media/game.iso"))?);
//!
//! // A preflight crate recognised the container; this is its plain result.
//! let description = MediaDescription::new(
//!     MediaClass::Udf,
//!     "udf",
//!     VolumeLabel::sanitized("EXAMPLE"),
//! );
//!
//! // Hash the whole source and bind the proof, then publish metadata only.
//! let validated = ValidatedMedia::fingerprinting(source, description)?;
//! let layout = CacheLayout::user_default()?;
//! ohl_media::prepare_import_cache(&validated, &layout)?.log();
//! # Ok(())
//! # }
//! ```

pub mod description;
pub mod digest;
pub mod error;
pub mod import_cache;
pub mod validated;

pub use description::{
    BoundedAsciiLabel, LABEL_CAPACITY, MediaClass, MediaDescription, VolumeLabel,
};
pub use digest::{
    FINGERPRINT_CHUNK_BYTES, MediaDigest, STABILITY_CHECK_INTERVAL_BYTES, fingerprint,
    fingerprint_with_progress,
};
pub use error::{ImportCacheError, MediaError};
pub use import_cache::{
    CacheLayout, CacheManifest, CacheReport, ENTRIES_DIRECTORY_NAME, MANIFEST_FILE_NAME,
    MANIFEST_SCHEMA_VERSION, MAXIMUM_MANIFEST_BYTES, PAYLOAD_STATE_NOT_IMPORTED,
    prepare_import_cache,
};
pub use validated::ValidatedMedia;
