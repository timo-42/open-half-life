//! The versioned save-file container: magic, format version, bounded
//! header, section table, sections, and a whole-file SHA-256 trailer.
//!
//! # On-disk layout
//!
//! ```text
//! magic                 8 bytes,  fixed b"OHLSAVE\0"
//! format_major          u16 LE
//! format_minor          u16 LE
//! header                bounded, see `crate::header::Header`
//! section_count         u32 LE
//! section_table[count]  52 bytes each: tag(u32) offset(u64) length(u64) sha256([u8;32])
//! section_data[count]   concatenated, in table order
//! trailer_sha256        32 bytes, SHA-256 of every byte above
//! ```
//!
//! All multi-byte integers are little-endian. Every offset in the table is
//! absolute from the start of the file. This layout is a project-owned
//! design written for Open Half-Life; it has no relationship to id
//! Tech/GoldSrc's `.sav`/`.hl1` save format.
//!
//! # Versioning
//!
//! `format_major` gates compatibility: [`SaveReader::open`] rejects any file
//! whose major version differs from [`FORMAT_MAJOR`] with
//! [`SaveError::UnsupportedMajorVersion`], because a major bump is defined
//! to mean the layout above changed incompatibly. `format_minor` is
//! forward-compatible within a major version: readers tolerate a minor
//! version other than their own, and tolerate section-table entries whose
//! tag is reserved for this crate's own future use (see
//! [`MIN_APPLICATION_TAG`]) that this build does not otherwise interpret —
//! their structural and digest integrity is still verified, but they are
//! excluded from ordinary lookups and counted by
//! [`SaveReader::unknown_section_count`] instead of causing a hard failure.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::bytes::{Reader, Writer};
use crate::error::{Result, SaveError};
use crate::header::Header;
use crate::limits::Limits;
use crate::table::{ENTRY_LEN, SectionEntry};

/// Fixed 8-byte magic every save file starts with.
pub const MAGIC: [u8; 8] = *b"OHLSAVE\0";
/// The major format version this build of the crate writes and accepts. A
/// file with a different major version is always rejected.
pub const FORMAT_MAJOR: u16 = 1;
/// The minor format version this build of the crate writes.
pub const FORMAT_MINOR: u16 = 0;
/// Section tags below this value are reserved for this crate's own current
/// or future use; [`SaveWriter::add_section`] refuses them. Application code
/// should use tags at or above this value.
pub const MIN_APPLICATION_TAG: u32 = 16;

/// Length in bytes of the whole-file SHA-256 trailer.
const TRAILER_LEN: usize = 32;

/// Builds a save-file container: [`SaveWriter::begin`], any number of
/// [`SaveWriter::add_section`]/[`SaveWriter::add_section_serde`] calls, then
/// [`SaveWriter::finish`].
#[derive(Debug, Clone)]
pub struct SaveWriter {
    header: Header,
    sections: Vec<(u32, Vec<u8>)>,
}

impl SaveWriter {
    /// Starts a new container with the given header. The header is not
    /// validated until [`SaveWriter::finish`]; call
    /// [`Header::validate`] directly for earlier feedback.
    #[must_use]
    pub fn begin(header: Header) -> Self {
        Self {
            header,
            sections: Vec::new(),
        }
    }

    /// Adds a raw section under `tag`.
    ///
    /// # Errors
    ///
    /// [`SaveError::ReservedTag`] if `tag` is below [`MIN_APPLICATION_TAG`],
    /// or [`SaveError::DuplicateTag`] if `tag` was already added.
    pub fn add_section(&mut self, tag: u32, bytes: &[u8]) -> Result<&mut Self> {
        if tag < MIN_APPLICATION_TAG {
            return Err(SaveError::ReservedTag);
        }
        if self.sections.iter().any(|(existing, _)| *existing == tag) {
            return Err(SaveError::DuplicateTag);
        }
        self.sections.push((tag, bytes.to_vec()));
        Ok(self)
    }

    /// Adds a section under `tag`, encoding `value` with `postcard`.
    ///
    /// # Errors
    ///
    /// As [`SaveWriter::add_section`], plus [`SaveError::Codec`] if
    /// `postcard` encoding fails.
    pub fn add_section_serde<T: Serialize>(&mut self, tag: u32, value: &T) -> Result<&mut Self> {
        let bytes = postcard::to_allocvec(value).map_err(|_| SaveError::Codec)?;
        self.add_section(tag, &bytes)
    }

    /// Finishes the container, validating it against `limits`, and returns
    /// the encoded bytes.
    ///
    /// # Errors
    ///
    /// [`SaveError::HeaderInvalid`] if the header fails validation, or
    /// [`SaveError::LimitExceeded`] if the section count, any single
    /// section, or the whole encoded file would exceed `limits`.
    pub fn finish(self, limits: &Limits) -> Result<Vec<u8>> {
        let section_count = self.sections.len();
        let section_count_u32 =
            u32::try_from(section_count).map_err(|_| SaveError::LimitExceeded)?;
        if section_count_u32 > limits.max_sections {
            return Err(SaveError::LimitExceeded);
        }
        for (_, bytes) in &self.sections {
            let length = u64::try_from(bytes.len()).map_err(|_| SaveError::LimitExceeded)?;
            if length > limits.max_section_bytes {
                return Err(SaveError::LimitExceeded);
            }
        }

        let mut writer = Writer::new();
        writer.raw(&MAGIC);
        writer.u16(FORMAT_MAJOR);
        writer.u16(FORMAT_MINOR);
        self.header.encode(&mut writer)?;
        writer.u32(section_count_u32);

        let header_end = writer.len();
        let table_len = section_count
            .checked_mul(ENTRY_LEN)
            .ok_or(SaveError::LimitExceeded)?;
        let data_start = header_end
            .checked_add(table_len)
            .ok_or(SaveError::LimitExceeded)?;

        let mut entries = Vec::with_capacity(section_count);
        let mut offset = data_start;
        for (tag, bytes) in &self.sections {
            let length = bytes.len();
            let sha256 = ohl_core::StreamingSha256::digest(bytes);
            let offset_u64 = u64::try_from(offset).map_err(|_| SaveError::LimitExceeded)?;
            let length_u64 = u64::try_from(length).map_err(|_| SaveError::LimitExceeded)?;
            entries.push(SectionEntry {
                tag: *tag,
                offset: offset_u64,
                length: length_u64,
                sha256,
            });
            offset = offset.checked_add(length).ok_or(SaveError::LimitExceeded)?;
        }

        for entry in &entries {
            entry.encode(&mut writer);
        }
        debug_assert_eq!(writer.len(), data_start);
        for (_, bytes) in &self.sections {
            writer.raw(bytes);
        }

        let total_before_trailer = writer.len();
        let total_len = total_before_trailer
            .checked_add(TRAILER_LEN)
            .ok_or(SaveError::LimitExceeded)?;
        let total_len_u64 = u64::try_from(total_len).map_err(|_| SaveError::LimitExceeded)?;
        if total_len_u64 > limits.max_file_bytes {
            return Err(SaveError::LimitExceeded);
        }

        let trailer = ohl_core::StreamingSha256::digest(writer.bytes());
        writer.array32(&trailer);
        Ok(writer.into_bytes())
    }
}

/// Opens and validates a save-file container without copying section
/// payloads: [`SaveReader::section`] and [`SaveReader::deserialize`] borrow
/// directly from the input buffer.
#[derive(Debug)]
pub struct SaveReader<'a> {
    header: Header,
    entries: Vec<SectionEntry>,
    bytes: &'a [u8],
    unknown_section_count: u32,
    format_minor: u16,
}

impl<'a> SaveReader<'a> {
    /// Parses and fully validates `bytes` as a save-file container.
    ///
    /// Validates the magic, major version, every bounded header field,
    /// every section-table entry's offset and length against the file size
    /// and `limits`, every section's SHA-256 digest, and the whole-file
    /// trailer digest.
    ///
    /// # Errors
    ///
    /// See [`SaveError`] for the exact failure codes; this never panics on
    /// any input, including truncated or adversarially crafted bytes.
    pub fn open(bytes: &'a [u8], limits: &Limits) -> Result<Self> {
        let file_len = bytes.len();
        let file_len_u64 = u64::try_from(file_len).map_err(|_| SaveError::LimitExceeded)?;
        if file_len_u64 > limits.max_file_bytes {
            return Err(SaveError::LimitExceeded);
        }
        if file_len < TRAILER_LEN {
            return Err(SaveError::Truncated);
        }
        let content_len = file_len - TRAILER_LEN;
        let content_len_u64 = u64::try_from(content_len).map_err(|_| SaveError::LimitExceeded)?;

        let mut reader = Reader::new(bytes);
        let magic = reader.take(MAGIC.len())?;
        if magic != MAGIC.as_slice() {
            return Err(SaveError::BadMagic);
        }
        let major = reader.u16()?;
        let minor = reader.u16()?;
        if major != FORMAT_MAJOR {
            return Err(SaveError::UnsupportedMajorVersion);
        }

        let header = Header::decode(&mut reader)?;

        let section_count_u32 = reader.u32()?;
        if section_count_u32 > limits.max_sections {
            return Err(SaveError::LimitExceeded);
        }
        let section_count =
            usize::try_from(section_count_u32).map_err(|_| SaveError::LimitExceeded)?;

        let mut entries = Vec::with_capacity(section_count.min(4_096));
        for _ in 0..section_count {
            entries.push(SectionEntry::decode(&mut reader)?);
        }
        let data_start = reader.position();
        let data_start_u64 = u64::try_from(data_start).map_err(|_| SaveError::LimitExceeded)?;

        let mut seen_tags: Vec<u32> = Vec::with_capacity(entries.len());
        let mut unknown_section_count: u32 = 0;
        for entry in &entries {
            if seen_tags.contains(&entry.tag) {
                return Err(SaveError::TableInvalid);
            }
            seen_tags.push(entry.tag);

            if entry.length > limits.max_section_bytes {
                return Err(SaveError::LimitExceeded);
            }
            if entry.offset < data_start_u64 {
                return Err(SaveError::TableInvalid);
            }
            let end = ohl_core::checked::add(entry.offset, entry.length)?;
            if end > content_len_u64 {
                return Err(SaveError::TableInvalid);
            }

            let start_usize = usize::try_from(entry.offset).map_err(|_| SaveError::TableInvalid)?;
            let end_usize = usize::try_from(end).map_err(|_| SaveError::TableInvalid)?;
            let slice = bytes
                .get(start_usize..end_usize)
                .ok_or(SaveError::TableInvalid)?;
            let digest = ohl_core::StreamingSha256::digest(slice);
            if digest != entry.sha256 {
                return Err(SaveError::SectionDigestMismatch);
            }

            if entry.tag < MIN_APPLICATION_TAG {
                unknown_section_count = unknown_section_count.saturating_add(1);
            }
        }

        let trailer_expected = ohl_core::StreamingSha256::digest(&bytes[..content_len]);
        let trailer_actual = &bytes[content_len..];
        if trailer_actual != trailer_expected {
            return Err(SaveError::TrailerMismatch);
        }

        Ok(Self {
            header,
            entries,
            bytes,
            unknown_section_count,
            format_minor: minor,
        })
    }

    /// The validated header.
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The file's `format_minor` value, for callers that want to branch on
    /// it directly rather than only on [`SaveReader::unknown_section_count`].
    #[must_use]
    pub const fn format_minor(&self) -> u16 {
        self.format_minor
    }

    /// All present section-table entries, including reserved-tag entries
    /// this build does not interpret. Bounded by the `limits` passed to
    /// [`SaveReader::open`].
    #[must_use]
    pub fn sections(&self) -> &[SectionEntry] {
        &self.entries
    }

    /// The number of table entries whose tag is reserved (below
    /// [`MIN_APPLICATION_TAG`]) that this build does not otherwise
    /// interpret. Their structural and digest integrity was still verified
    /// during [`SaveReader::open`].
    #[must_use]
    pub const fn unknown_section_count(&self) -> u32 {
        self.unknown_section_count
    }

    fn find(&self, tag: u32) -> Result<&SectionEntry> {
        self.entries
            .iter()
            .find(|entry| entry.tag == tag)
            .ok_or(SaveError::SectionNotFound)
    }

    /// Borrows the raw bytes of the section tagged `tag`.
    ///
    /// # Errors
    ///
    /// [`SaveError::SectionNotFound`] if no section has that tag.
    pub fn section(&self, tag: u32) -> Result<&'a [u8]> {
        let entry = self.find(tag)?;
        let end = ohl_core::checked::add(entry.offset, entry.length)?;
        let start = usize::try_from(entry.offset).map_err(|_| SaveError::TableInvalid)?;
        let end = usize::try_from(end).map_err(|_| SaveError::TableInvalid)?;
        self.bytes.get(start..end).ok_or(SaveError::TableInvalid)
    }

    /// Decodes the section tagged `tag` with `postcard`.
    ///
    /// # Errors
    ///
    /// As [`SaveReader::section`], plus [`SaveError::Codec`] if `postcard`
    /// decoding fails.
    pub fn deserialize<T: DeserializeOwned>(&self, tag: u32) -> Result<T> {
        let bytes = self.section(tag)?;
        postcard::from_bytes(bytes).map_err(|_| SaveError::Codec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            game_version: "0.1.0".to_string(),
            created_at_unix_secs: 42,
            map_identity: "sample-map".to_string(),
            title: "Sample".to_string(),
            thumbnail: Vec::new(),
        }
    }

    #[test]
    fn round_trips_raw_and_serde_sections() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"raw-bytes").unwrap();
        writer
            .add_section_serde(17, &(1u32, "value".to_string()))
            .unwrap();
        let bytes = writer.finish(&Limits::default()).unwrap();

        let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
        assert_eq!(reader.header(), &header());
        assert_eq!(reader.section(16).unwrap(), b"raw-bytes");
        assert_eq!(
            reader.deserialize::<(u32, String)>(17).unwrap(),
            (1u32, "value".to_string())
        );
        assert_eq!(reader.unknown_section_count(), 0);
        assert_eq!(reader.sections().len(), 2);
    }

    #[test]
    fn reserved_tag_is_refused() {
        let mut writer = SaveWriter::begin(header());
        assert_eq!(
            writer.add_section(0, b"x").unwrap_err(),
            SaveError::ReservedTag
        );
    }

    #[test]
    fn duplicate_tag_is_refused() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        assert_eq!(
            writer.add_section(16, b"b").unwrap_err(),
            SaveError::DuplicateTag
        );
    }

    #[test]
    fn unknown_tag_lookup_is_not_found() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        let bytes = writer.finish(&Limits::default()).unwrap();
        let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
        assert_eq!(reader.section(99).unwrap_err(), SaveError::SectionNotFound);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        let mut bytes = writer.finish(&Limits::default()).unwrap();
        bytes[0] ^= 0xFF;
        assert_eq!(
            SaveReader::open(&bytes, &Limits::default()).unwrap_err(),
            SaveError::BadMagic
        );
    }

    #[test]
    fn major_version_mismatch_is_rejected() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        let mut bytes = writer.finish(&Limits::default()).unwrap();
        bytes[8] = bytes[8].wrapping_add(1);
        assert_eq!(
            SaveReader::open(&bytes, &Limits::default()).unwrap_err(),
            SaveError::UnsupportedMajorVersion
        );
    }

    #[test]
    fn minor_version_and_reserved_sections_are_tolerated() {
        // Hand-assemble a file with a newer minor version and one
        // reserved-tag section this build does not interpret, simulating a
        // future minor version that added its own bookkeeping section. It
        // must still open successfully, with the reserved section counted
        // as "unknown" rather than causing a failure.
        let extra_bytes = b"extra".to_vec();
        let app_bytes = b"app-data".to_vec();

        let mut fresh = Writer::new();
        fresh.raw(&MAGIC);
        fresh.u16(FORMAT_MAJOR);
        fresh.u16(FORMAT_MINOR + 1);
        header().encode(&mut fresh).unwrap();
        fresh.u32(2);
        let table_start = fresh.len();
        let data_start = table_start + ENTRY_LEN * 2;

        let reserved_entry = SectionEntry {
            tag: 1,
            offset: u64::try_from(data_start).unwrap(),
            length: u64::try_from(extra_bytes.len()).unwrap(),
            sha256: ohl_core::StreamingSha256::digest(&extra_bytes),
        };
        let app_entry = SectionEntry {
            tag: 16,
            offset: u64::try_from(data_start + extra_bytes.len()).unwrap(),
            length: u64::try_from(app_bytes.len()).unwrap(),
            sha256: ohl_core::StreamingSha256::digest(&app_bytes),
        };
        reserved_entry.encode(&mut fresh);
        app_entry.encode(&mut fresh);
        fresh.raw(&extra_bytes);
        fresh.raw(&app_bytes);
        let trailer = ohl_core::StreamingSha256::digest(fresh.bytes());
        fresh.array32(&trailer);
        let bytes = fresh.into_bytes();

        let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
        assert_eq!(reader.format_minor(), FORMAT_MINOR + 1);
        assert_eq!(reader.unknown_section_count(), 1);
        assert_eq!(reader.section(16).unwrap(), &app_bytes[..]);
    }

    #[test]
    fn section_digest_tamper_is_rejected() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        let mut bytes = writer.finish(&Limits::default()).unwrap();
        let last = bytes.len() - 1 - 32; // last byte of section data, before trailer
        bytes[last] ^= 0xFF;
        assert_eq!(
            SaveReader::open(&bytes, &Limits::default()).unwrap_err(),
            SaveError::SectionDigestMismatch
        );
    }

    #[test]
    fn trailer_tamper_is_rejected() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        let mut bytes = writer.finish(&Limits::default()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert_eq!(
            SaveReader::open(&bytes, &Limits::default()).unwrap_err(),
            SaveError::TrailerMismatch
        );
    }

    #[test]
    fn table_tamper_is_rejected() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"a").unwrap();
        writer.add_section(17, b"b").unwrap();
        let mut bytes = writer.finish(&Limits::default()).unwrap();
        // Flip a byte inside the second table entry's offset field so it no
        // longer matches the section it points to but the file otherwise
        // stays well-formed; either a table or a digest failure is
        // acceptable, but it must not silently succeed.
        let flip_at = bytes.len() - 32 - 20;
        bytes[flip_at] ^= 0xFF;
        assert!(SaveReader::open(&bytes, &Limits::default()).is_err());
    }

    #[test]
    fn truncation_never_panics() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, b"hello world").unwrap();
        let bytes = writer.finish(&Limits::default()).unwrap();
        for len in 0..bytes.len() {
            let _ = SaveReader::open(&bytes[..len], &Limits::default());
        }
    }

    #[test]
    fn section_count_over_limit_is_rejected_on_open() {
        let mut writer = SaveWriter::begin(header());
        for tag in 16..24 {
            writer.add_section(tag, b"x").unwrap();
        }
        let bytes = writer.finish(&Limits::default()).unwrap();
        let tight = Limits {
            max_sections: 4,
            ..Limits::default()
        };
        assert_eq!(
            SaveReader::open(&bytes, &tight).unwrap_err(),
            SaveError::LimitExceeded
        );
    }

    #[test]
    fn section_count_over_limit_is_rejected_on_finish() {
        let mut writer = SaveWriter::begin(header());
        for tag in 16..24 {
            writer.add_section(tag, b"x").unwrap();
        }
        let tight = Limits {
            max_sections: 4,
            ..Limits::default()
        };
        assert_eq!(writer.finish(&tight).unwrap_err(), SaveError::LimitExceeded);
    }

    #[test]
    fn oversized_section_is_rejected_on_finish() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, &vec![0u8; 1_024]).unwrap();
        let tight = Limits {
            max_section_bytes: 16,
            ..Limits::default()
        };
        assert_eq!(writer.finish(&tight).unwrap_err(), SaveError::LimitExceeded);
    }

    #[test]
    fn truncation_at_every_64_byte_boundary_never_panics() {
        let mut writer = SaveWriter::begin(header());
        writer.add_section(16, &vec![0xAB_u8; 300]).unwrap();
        writer
            .add_section_serde(17, &("chapter", 7_u32, vec![1_u8, 2, 3]))
            .unwrap();
        let bytes = writer.finish(&Limits::default()).unwrap();
        assert!(
            bytes.len() > 64 * 4,
            "fixture should span several boundaries"
        );

        let mut boundary = 0usize;
        while boundary <= bytes.len() {
            let _ = SaveReader::open(&bytes[..boundary], &Limits::default());
            boundary += 64;
        }
        // The exact final length must still open successfully; truncation
        // logic must not have been accidentally exercised on the untruncated
        // fixture too.
        assert!(SaveReader::open(&bytes, &Limits::default()).is_ok());
    }

    proptest::proptest! {
        /// Arbitrary bytes handed to `SaveReader::open` must never panic,
        /// regardless of length or content.
        #[test]
        fn open_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..2_048)) {
            let _ = SaveReader::open(&bytes, &Limits::default());
        }

        /// A container built from arbitrary section bytes and header
        /// strings round-trips exactly: the reader must reproduce the same
        /// header and section bytes the writer was given.
        #[test]
        fn round_trip_identity(
            game_version in "[a-zA-Z0-9. -]{0,32}",
            map_identity in "[a-zA-Z0-9. -]{0,32}",
            title in "[a-zA-Z0-9. -]{0,32}",
            created_at_unix_secs in proptest::prelude::any::<u64>(),
            section_a in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
            section_b in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
        ) {
            let header = Header {
                game_version,
                created_at_unix_secs,
                map_identity,
                title,
                thumbnail: Vec::new(),
            };
            let mut writer = SaveWriter::begin(header.clone());
            writer.add_section(16, &section_a).unwrap();
            writer.add_section(17, &section_b).unwrap();
            let bytes = writer.finish(&Limits::default()).unwrap();

            let reader = SaveReader::open(&bytes, &Limits::default()).unwrap();
            proptest::prop_assert_eq!(reader.header(), &header);
            proptest::prop_assert_eq!(reader.section(16).unwrap(), section_a.as_slice());
            proptest::prop_assert_eq!(reader.section(17).unwrap(), section_b.as_slice());
        }
    }
}
