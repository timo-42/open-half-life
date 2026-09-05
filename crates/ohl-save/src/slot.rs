//! Save slots: a directory of named `.ohlsave` files, published atomically.
//!
//! # Publication guarantee
//!
//! [`SaveSlot::write`] writes the full container to a temporary file in the
//! same directory, then publishes it with [`std::fs::rename`]. On every
//! POSIX target this crate builds for, `rename(2)` replacing an existing
//! regular file on the same filesystem is atomic: a concurrent reader always
//! observes either the old file or the new one in full, never a partial
//! write, and never a gap where the name does not exist. On Windows,
//! [`std::fs::rename`] maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`
//! for a source and destination that are both regular files; Microsoft does
//! not document this as atomic, but because the temporary file is written
//! and closed in full before the rename call, a reader can still only ever
//! observe a complete old or new file through the published name, not a
//! partially written one. Both targets require the temporary file and the
//! destination to be on the same filesystem/volume, which is guaranteed
//! here because the temporary file is always created in the same directory
//! as the destination.
//!
//! This module does not implement the create-only, never-replace semantics
//! `ohl_platform::atomic_directory` provides for directories: save slots are
//! meant to be overwritten (an autosave or quicksave replaces its previous
//! contents), so replace-on-publish is the wanted behavior here, not a
//! bug.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::container::SaveReader;
use crate::error::{Result, SaveError};
use crate::header::Header;
use crate::limits::Limits;

/// The conventional autosave slot name.
pub const AUTOSAVE_SLOT_NAME: &str = "autosave";
/// The conventional quicksave slot name.
pub const QUICKSAVE_SLOT_NAME: &str = "quicksave";

/// File extension (without the leading dot) used for save-slot files.
const SLOT_EXTENSION: &str = "ohlsave";
/// Maximum byte length of a slot name.
const MAX_SLOT_NAME_LEN: usize = 64;
/// Maximum number of slots [`SaveSlot::list`] will enumerate before
/// reporting [`SaveError::LimitExceeded`] instead of silently truncating.
const MAX_LISTED_SLOTS: usize = 4_096;

/// One entry returned by [`SaveSlot::list`]: a slot name and its validated
/// header, without the section payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotListing {
    /// The slot name, without directory or extension.
    pub name: String,
    /// The header parsed from the slot's file.
    pub header: Header,
}

/// A directory of save-slot files.
#[derive(Debug, Clone)]
pub struct SaveSlot {
    dir: PathBuf,
}

impl SaveSlot {
    /// Creates a handle over `dir`. The directory need not exist yet;
    /// [`SaveSlot::write`] creates it (and any missing parents) on demand.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The default per-user save directory, `<data dir>/saves`, using the
    /// OS-conventional application data directory resolved by the
    /// `directories` crate. Returns `None` if no data directory could be
    /// determined for the current user (mirroring `directories`' own
    /// fallible contract).
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("io.github", "open-half-life", "open-half-life")
            .map(|dirs| dirs.data_dir().join("saves"))
    }

    /// The directory this handle operates on.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file_name(name: &str) -> Result<String> {
        validate_slot_name(name)?;
        Ok(format!("{name}.{SLOT_EXTENSION}"))
    }

    fn path_for(&self, name: &str) -> Result<PathBuf> {
        Ok(self.dir.join(Self::file_name(name)?))
    }

    fn temp_path(&self, name: &str) -> PathBuf {
        let nonce = temp_nonce();
        self.dir
            .join(format!(".{name}.{SLOT_EXTENSION}.tmp-{nonce}"))
    }

    /// Atomically writes `bytes` to the slot named `name`, replacing any
    /// previous contents. See the [module documentation](self) for the
    /// exact publication guarantee.
    ///
    /// # Errors
    ///
    /// [`SaveError::InvalidSlotName`] for a malformed `name`, or
    /// [`SaveError::Io`] if creating the directory, writing the temporary
    /// file, or the publishing rename fails.
    pub fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let destination = self.path_for(name)?;
        fs::create_dir_all(&self.dir).map_err(|_| SaveError::Io)?;
        let temp_path = self.temp_path(name);
        let write_result = fs::write(&temp_path, bytes);
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
            return Err(SaveError::Io);
        }
        if let Err(_error) = fs::rename(&temp_path, &destination) {
            let _ = fs::remove_file(&temp_path);
            return Err(SaveError::Io);
        }
        Ok(())
    }

    /// Reads the raw bytes of the slot named `name`.
    ///
    /// # Errors
    ///
    /// [`SaveError::InvalidSlotName`] for a malformed `name`,
    /// [`SaveError::SlotNotFound`] if it does not exist, or
    /// [`SaveError::Io`] for any other filesystem failure.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = self.path_for(name)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(SaveError::SlotNotFound),
            Err(_) => Err(SaveError::Io),
        }
    }

    /// Deletes the slot named `name`.
    ///
    /// # Errors
    ///
    /// [`SaveError::InvalidSlotName`] for a malformed `name`,
    /// [`SaveError::SlotNotFound`] if it does not exist, or
    /// [`SaveError::Io`] for any other filesystem failure.
    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.path_for(name)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(SaveError::SlotNotFound),
            Err(_) => Err(SaveError::Io),
        }
    }

    /// Lists every valid slot in the directory: files named `<name>.ohlsave`
    /// whose name passes [`validate_slot_name`] and whose contents parse as
    /// a valid container under `limits`. Files that fail either check are
    /// skipped rather than causing an error, since a directory of save
    /// slots may reasonably contain files this crate does not own.
    ///
    /// # Errors
    ///
    /// [`SaveError::Io`] if the directory cannot be read, or
    /// [`SaveError::LimitExceeded`] if more than a fixed bound
    /// (`4096`) of matching files are present.
    pub fn list(&self, limits: &Limits) -> Result<Vec<SlotListing>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(SaveError::Io),
        };

        let mut out = Vec::new();
        let mut matched = 0usize;
        for entry in entries {
            let entry = entry.map_err(|_| SaveError::Io)?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(SLOT_EXTENSION) {
                continue;
            }
            matched += 1;
            if matched > MAX_LISTED_SLOTS {
                return Err(SaveError::LimitExceeded);
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if validate_slot_name(stem).is_err() {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(reader) = SaveReader::open(&bytes, limits) else {
                continue;
            };
            out.push(SlotListing {
                name: stem.to_string(),
                header: reader.header().clone(),
            });
        }
        Ok(out)
    }
}

/// Validates a save-slot name: non-empty, at most [`MAX_SLOT_NAME_LEN`]
/// bytes, and composed only of ASCII letters, digits, `-`, or `_`. This
/// rules out path separators, `.`/`..`, and NUL, so a slot name can never
/// escape the save directory.
///
/// # Errors
///
/// [`SaveError::InvalidSlotName`] if any rule above is violated.
pub fn validate_slot_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_SLOT_NAME_LEN {
        return Err(SaveError::InvalidSlotName);
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(SaveError::InvalidSlotName);
    }
    Ok(())
}

/// A process/time-derived value unique enough to keep concurrent writers'
/// temporary file names from colliding. It needs no cryptographic
/// randomness: the temporary name is never a security boundary, only the
/// eventual `rename` publication is.
fn temp_nonce() -> String {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{}-{time}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::SaveWriter;

    fn sample_bytes(title: &str) -> Vec<u8> {
        let header = Header {
            game_version: "0.1.0".to_string(),
            created_at_unix_secs: 1,
            map_identity: "sample-map".to_string(),
            title: title.to_string(),
            thumbnail: Vec::new(),
        };
        let mut writer = SaveWriter::begin(header);
        writer.add_section(16, b"data").unwrap();
        writer.finish(&Limits::default()).unwrap()
    }

    #[test]
    fn write_read_and_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let slot = SaveSlot::new(dir.path());
        let bytes = sample_bytes("Save One");

        slot.write("save1", &bytes).unwrap();
        assert_eq!(slot.read("save1").unwrap(), bytes);

        slot.delete("save1").unwrap();
        assert_eq!(slot.read("save1").unwrap_err(), SaveError::SlotNotFound);
        assert_eq!(slot.delete("save1").unwrap_err(), SaveError::SlotNotFound);
    }

    #[test]
    fn write_replaces_existing_contents() {
        let dir = tempfile::tempdir().unwrap();
        let slot = SaveSlot::new(dir.path());
        slot.write(QUICKSAVE_SLOT_NAME, &sample_bytes("First"))
            .unwrap();
        slot.write(QUICKSAVE_SLOT_NAME, &sample_bytes("Second"))
            .unwrap();

        let bytes = slot.read(QUICKSAVE_SLOT_NAME).unwrap();
        let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
        assert_eq!(reader.header().title, "Second");
    }

    #[test]
    fn invalid_slot_names_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let slot = SaveSlot::new(dir.path());
        for bad in ["", "../escape", "a/b", "a.b", "a\0b", &"x".repeat(100)] {
            assert_eq!(
                slot.write(bad, b"x").unwrap_err(),
                SaveError::InvalidSlotName
            );
        }
    }

    #[test]
    fn listing_reports_headers_and_skips_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let slot = SaveSlot::new(dir.path());
        slot.write(AUTOSAVE_SLOT_NAME, &sample_bytes("Auto"))
            .unwrap();
        slot.write(QUICKSAVE_SLOT_NAME, &sample_bytes("Quick"))
            .unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"unrelated").unwrap();
        std::fs::write(dir.path().join("corrupt.ohlsave"), b"not a save").unwrap();

        let mut listing = slot.list(&Limits::default()).unwrap();
        listing.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(listing.len(), 2);
        assert_eq!(listing[0].name, AUTOSAVE_SLOT_NAME);
        assert_eq!(listing[0].header.title, "Auto");
        assert_eq!(listing[1].name, QUICKSAVE_SLOT_NAME);
        assert_eq!(listing[1].header.title, "Quick");
    }

    #[test]
    fn listing_an_absent_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let slot = SaveSlot::new(missing);
        assert_eq!(slot.list(&Limits::default()).unwrap(), Vec::new());
    }

    #[test]
    fn default_dir_ends_in_saves() {
        if let Some(dir) = SaveSlot::default_dir() {
            assert_eq!(
                dir.file_name().and_then(|name| name.to_str()),
                Some("saves")
            );
        }
    }
}
