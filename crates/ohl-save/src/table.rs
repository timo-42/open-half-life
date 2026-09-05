//! The section table: one fixed-width entry per section.

use crate::bytes::{Reader, Writer};
use crate::error::Result;

/// The on-disk width in bytes of one [`SectionEntry`]: `tag` (4) + `offset`
/// (8) + `length` (8) + `sha256` (32).
pub(crate) const ENTRY_LEN: usize = 4 + 8 + 8 + 32;

/// One section-table entry: a caller-defined tag, the section's absolute
/// byte offset and length within the file, and its SHA-256 digest.
///
/// This is both the on-disk table row and the type returned by
/// [`crate::SaveReader::sections`] for listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionEntry {
    /// The caller-defined section type tag. Values below
    /// [`crate::MIN_APPLICATION_TAG`] are reserved for this crate's own
    /// future use.
    pub tag: u32,
    /// Absolute byte offset of the section's first byte within the file.
    pub offset: u64,
    /// Length in bytes of the section.
    pub length: u64,
    /// SHA-256 digest of the section's bytes.
    pub sha256: [u8; 32],
}

impl SectionEntry {
    pub(crate) fn encode(&self, writer: &mut Writer) {
        writer.u32(self.tag);
        writer.u64(self.offset);
        writer.u64(self.length);
        writer.array32(&self.sha256);
    }

    pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            tag: reader.u32()?,
            offset: reader.u64()?,
            length: reader.u64()?,
            sha256: reader.array32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_encode_decode() {
        let entry = SectionEntry {
            tag: 42,
            offset: 100,
            length: 8,
            sha256: [9u8; 32],
        };
        let mut writer = Writer::new();
        entry.encode(&mut writer);
        let bytes = writer.into_bytes();
        assert_eq!(bytes.len(), ENTRY_LEN);
        let mut reader = Reader::new(&bytes);
        assert_eq!(SectionEntry::decode(&mut reader).unwrap(), entry);
    }
}
