//! The bounded, fixed-shape save-file header.

use crate::bytes::{Reader, Writer};
use crate::error::{Result, SaveError};

/// Maximum byte length of [`Header::game_version`].
pub const MAX_GAME_VERSION_LEN: usize = 64;
/// Maximum byte length of [`Header::map_identity`].
pub const MAX_MAP_IDENTITY_LEN: usize = 128;
/// Maximum byte length of [`Header::title`].
pub const MAX_TITLE_LEN: usize = 128;
/// Maximum byte length of the reserved [`Header::thumbnail`] slot.
pub const MAX_THUMBNAIL_LEN: usize = 65_536;

/// The bounded save-file header.
///
/// Every variable-length field is written with an explicit length prefix
/// and validated against a fixed maximum on both write and read, so a
/// header can never itself make the file's declared size unbounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// A caller-supplied identifier for the game/engine build that wrote the
    /// file (for example a semantic version string). Opaque to this crate.
    pub game_version: String,
    /// Creation time as a Unix timestamp in whole seconds. This crate does
    /// not interpret it; callers own the clock source and timezone policy.
    pub created_at_unix_secs: u64,
    /// A caller-supplied identifier for the map/level the save was created
    /// in. Opaque to this crate.
    pub map_identity: String,
    /// A caller-supplied human-readable chapter or save title.
    pub title: String,
    /// Reserved thumbnail slot. This version of the format stores whatever
    /// bytes the caller supplies (bounded by [`MAX_THUMBNAIL_LEN`]) without
    /// interpreting them; no thumbnail encoding is defined yet, so writers
    /// that do not have one should pass an empty `Vec`.
    pub thumbnail: Vec<u8>,
}

impl Header {
    /// Validates every field against its fixed maximum.
    ///
    /// # Errors
    ///
    /// [`SaveError::HeaderInvalid`] if any field exceeds its maximum length.
    pub fn validate(&self) -> Result<()> {
        if self.game_version.len() > MAX_GAME_VERSION_LEN
            || self.map_identity.len() > MAX_MAP_IDENTITY_LEN
            || self.title.len() > MAX_TITLE_LEN
            || self.thumbnail.len() > MAX_THUMBNAIL_LEN
        {
            return Err(SaveError::HeaderInvalid);
        }
        Ok(())
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<()> {
        self.validate()?;
        writer.bounded_string(&self.game_version)?;
        writer.u64(self.created_at_unix_secs);
        writer.bounded_string(&self.map_identity)?;
        writer.bounded_string(&self.title)?;
        writer.bounded_bytes(&self.thumbnail)?;
        Ok(())
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let game_version = reader.bounded_string(MAX_GAME_VERSION_LEN)?;
        let created_at_unix_secs = reader.u64()?;
        let map_identity = reader.bounded_string(MAX_MAP_IDENTITY_LEN)?;
        let title = reader.bounded_string(MAX_TITLE_LEN)?;
        let thumbnail = reader.bounded_bytes(MAX_THUMBNAIL_LEN)?;
        Ok(Self {
            game_version,
            created_at_unix_secs,
            map_identity,
            title,
            thumbnail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        Header {
            game_version: "0.1.0".to_string(),
            created_at_unix_secs: 1_234,
            map_identity: "sample-map".to_string(),
            title: "Sample Save".to_string(),
            thumbnail: Vec::new(),
        }
    }

    #[test]
    fn round_trips_through_encode_decode() {
        let header = sample();
        let mut writer = Writer::new();
        header.encode(&mut writer).unwrap();
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        assert_eq!(Header::decode(&mut reader).unwrap(), header);
        assert_eq!(reader.position(), bytes.len());
    }

    #[test]
    fn over_length_field_is_rejected_on_encode() {
        let mut header = sample();
        header.game_version = "x".repeat(MAX_GAME_VERSION_LEN + 1);
        assert_eq!(header.validate().unwrap_err(), SaveError::HeaderInvalid);
    }
}
