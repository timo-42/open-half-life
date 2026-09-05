//! A project-owned, versioned save-file container for Open Half-Life.
//!
//! **This is not the id Tech/GoldSrc `.sav`/`.hl1` save format.** It is a
//! from-scratch, project-designed binary container: a fixed magic, a format
//! version, a bounded header, a tagged section table with a per-section
//! SHA-256 digest, the section payloads, and a whole-file SHA-256 trailer.
//! See [`container`] for the exact on-disk layout and the versioning rules,
//! and `docs/ARCHITECTURE.md`'s "Save files" paragraph for how this fits the
//! rest of the Rust port.
//!
//! # Quick start
//!
//! ```
//! use ohl_save::{Header, Limits, SaveReader, SaveWriter};
//!
//! let header = Header {
//!     game_version: "0.1.0".to_string(),
//!     created_at_unix_secs: 0,
//!     map_identity: "sample-map".to_string(),
//!     title: "Sample Save".to_string(),
//!     thumbnail: Vec::new(),
//! };
//!
//! let mut writer = SaveWriter::begin(header);
//! writer.add_section(16, b"raw section bytes").unwrap();
//! let bytes = writer.finish(&Limits::default()).unwrap();
//!
//! let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
//! assert_eq!(reader.section(16).unwrap(), b"raw section bytes");
//! ```
//!
//! [`SaveSlot`] adds a small filesystem layer on top: a default save
//! directory (via `directories`), atomic write-then-rename publication, and
//! bounded listing of the slots present in a directory.

mod bytes;
mod container;
mod error;
mod header;
mod limits;
mod slot;
mod table;

pub use container::{
    FORMAT_MAJOR, FORMAT_MINOR, MAGIC, MIN_APPLICATION_TAG, SaveReader, SaveWriter,
};
pub use error::{Result, SaveError};
pub use header::{
    Header, MAX_GAME_VERSION_LEN, MAX_MAP_IDENTITY_LEN, MAX_THUMBNAIL_LEN, MAX_TITLE_LEN,
};
pub use limits::Limits;
pub use slot::{
    AUTOSAVE_SLOT_NAME, QUICKSAVE_SLOT_NAME, SaveSlot, SlotListing, validate_slot_name,
};
pub use table::SectionEntry;
