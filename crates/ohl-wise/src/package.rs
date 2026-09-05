//! One bounded pass over a whole package: overlay, header, chain, script.
//!
//! This is a convenience over the individual modules; callers that need to
//! stream the chain without materialising a record list use [`crate::chain`]
//! directly.

use alloc::vec::Vec;

use crate::chain::{Chain, ChainEvent, StreamRecord};
use crate::error::{Error, Limit};
use crate::extract::{FileMap, FileReader};
use crate::header::{PackageHeader, locate_first_stream};
use crate::limits::Limits;
use crate::overlay::{Overlay, find_overlay, overlay_at};
use crate::script::{FileTable, StreamEvidence, parse_with_evidence};
use crate::source::{Cancellation, ImageSource};
use crate::stream::{ChecksumStatus, inflate_stream};

/// Bounded aggregates describing one walked package. Every field is a count
/// or a size; nothing here is media-derived text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChainSummary {
    /// Streams decoded.
    pub streams: u32,
    /// Streams whose trailing checksum matched.
    pub crc_matches: u32,
    /// Streams whose trailing checksum did not match.
    pub crc_mismatches: u32,
    /// Streams whose trailing checksum could not be read.
    pub crc_absent: u32,
    /// Resynchronisation events.
    pub resyncs: u32,
    /// Total inflated bytes across the chain.
    pub total_inflated_bytes: u64,
    /// Compressed bytes covered by the chain.
    pub covered_bytes: u64,
}

/// A package opened for reading.
#[derive(Debug, Clone)]
pub struct Package {
    overlay: Overlay,
    header: PackageHeader,
    streams: Vec<StreamRecord>,
    summary: ChainSummary,
    table: FileTable,
    map: FileMap,
    limits: Limits,
}

impl Package {
    /// Where the overlay starts.
    #[must_use]
    pub const fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// The located header.
    #[must_use]
    pub const fn header(&self) -> &PackageHeader {
        &self.header
    }

    /// Every walked stream.
    #[must_use]
    pub fn streams(&self) -> &[StreamRecord] {
        &self.streams
    }

    /// Bounded aggregates for the walk.
    #[must_use]
    pub const fn summary(&self) -> ChainSummary {
        self.summary
    }

    /// The recognised file records.
    #[must_use]
    pub const fn file_table(&self) -> &FileTable {
        &self.table
    }

    /// Records mapped onto streams.
    #[must_use]
    pub const fn file_map(&self) -> &FileMap {
        &self.map
    }

    /// Opens the file behind record `index`.
    pub fn open_file(&self, index: usize) -> Result<FileReader, Error> {
        self.map.open_file(&self.table, index, self.limits)
    }

    /// Opens chain stream `index` directly.
    pub fn open_stream(&self, index: u32) -> Result<FileReader, Error> {
        self.map.open_stream(index, self.limits)
    }
}

/// Walks a package from a caller-supplied source.
///
/// When `overlay_offset` is `Some`, that offset is used as given (validated
/// against the image length); otherwise it is computed from the PE section
/// table.
pub fn read_package<S: ImageSource, C: Cancellation>(
    source: &mut S,
    overlay_offset: Option<u64>,
    limits: Limits,
    cancel: &C,
) -> Result<Package, Error> {
    let overlay = match overlay_offset {
        Some(offset) => overlay_at(source, offset)?,
        None => find_overlay(source, &limits)?,
    };
    let header = locate_first_stream(source, &overlay, &limits, cancel)?;

    let mut chain = Chain::new(header.first_stream_offset, overlay.end(), limits);
    let mut streams = Vec::new();
    let mut summary = ChainSummary::default();
    while let Some(event) = chain.next_event(source, cancel) {
        match event? {
            ChainEvent::Resynced { .. } => summary.resyncs += 1,
            ChainEvent::Stream(record) => {
                match record.checksum() {
                    ChecksumStatus::Match => summary.crc_matches += 1,
                    ChecksumStatus::Mismatch => summary.crc_mismatches += 1,
                    ChecksumStatus::Absent => summary.crc_absent += 1,
                }
                streams.push(record);
            }
        }
    }
    summary.streams = chain.stream_count();
    summary.total_inflated_bytes = chain.total_inflated_bytes();
    summary.covered_bytes = chain.cursor().saturating_sub(header.first_stream_offset);

    // The second stream is the script binary; a package with fewer streams
    // simply has no file table, and its streams stay reachable by index.
    let table = match streams.get(1) {
        Some(script) => {
            let mut bytes: Vec<u8> = Vec::new();
            let script_limits = Limits {
                max_inflated_bytes_per_stream: limits
                    .max_inflated_bytes_per_stream
                    .min(limits.max_script_bytes as u64),
                ..limits
            };
            match inflate_stream(source, script.offset(), script_limits, &mut bytes, cancel) {
                Ok(_) => parse_with_evidence(
                    &bytes,
                    &overlay,
                    &limits,
                    &StreamEvidence::from_streams(&streams),
                )?,
                Err(Error::LimitExceeded(Limit::InflatedBytesPerStream)) => {
                    return Err(Error::LimitExceeded(Limit::ScriptBytes));
                }
                Err(error) => return Err(error),
            }
        }
        None => FileTable::default(),
    };

    let map = FileMap::build(&table, &streams, &overlay);
    Ok(Package {
        overlay,
        header,
        streams,
        summary,
        table,
        map,
        limits,
    })
}

#[cfg(test)]
mod tests {
    use super::read_package;
    use crate::limits::Limits;
    use crate::source::{NeverCancelled, SliceSource};
    use crate::testing::{PackageOptions, SyntheticFile, build_package};
    use alloc::vec;
    use alloc::vec::Vec;

    #[test]
    fn reads_a_synthetic_package_end_to_end() {
        let files = vec![
            SyntheticFile::new(b"maps\\one.dat", vec![1u8; 4096]),
            SyntheticFile::new(b"cfg\\two.cfg", b"alpha beta gamma".repeat(16)),
        ];
        let package = build_package(&PackageOptions::with_files(files.clone()));
        let mut source = SliceSource::new(&package.image);
        let read = read_package(&mut source, None, Limits::DEFAULT, &NeverCancelled).unwrap();

        assert_eq!(read.overlay().offset, package.overlay_offset);
        assert_eq!(
            read.header().first_stream_offset,
            package.first_stream_offset
        );
        assert_eq!(read.summary().streams, 4);
        assert_eq!(read.summary().crc_matches, 4);
        assert_eq!(read.file_table().len(), 2);
        assert_eq!(read.file_map().mapped_count(), 2);

        for (index, file) in files.iter().enumerate() {
            let mut reader = read.open_file(index).unwrap();
            let mut out: Vec<u8> = Vec::new();
            reader
                .read_all(&mut source, &mut out, &NeverCancelled, Limits::DEFAULT)
                .unwrap();
            assert_eq!(out, file.content);
        }
    }
}
