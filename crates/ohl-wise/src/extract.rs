//! Mapping file records onto chain streams, and reading one file out.
//!
//! # How a record finds its bytes
//!
//! A record's stored deflate offsets cannot be trusted to locate a stream:
//! REWise records that the script deflate offset "isn't correct on some
//! installers", and on the package this crate was validated against no
//! constant origin relates the stored values to any measured stream. What
//! *is* verifiable is content: each stream's inflated length and CRC-32 are
//! measured while walking the chain, and each record declares both.
//!
//! So the mapping runs in two passes:
//!
//! 1. **Content evidence.** A record whose declared CRC-32 is non-zero claims
//!    the first still-unclaimed stream with exactly that CRC-32 and exactly
//!    that inflated length. Nothing here can be forged into reading the wrong
//!    bytes: the reader re-verifies both on extraction.
//! 2. **Stored offsets.** Records left over are matched by offset. Both
//!    documented origins — relative to the overlay start and absolute within
//!    the image — plus the observed "relative to the first stream" variant
//!    are counted, and whichever matches most records exactly is used.
//!
//! A record that neither pass resolves is reported as unmapped rather than
//! guessed at, and every stream that no record claims stays reachable through
//! [`FileMap::unnamed_streams`], keyed by chain index, so extraction still
//! works for packages whose script this crate cannot name.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::chain::StreamRecord;
use crate::error::Error;
use crate::limits::Limits;
use crate::overlay::Overlay;
use crate::script::{FileRecord, FileTable};
use crate::source::{Cancellation, ImageSource, Sink};
use crate::stream::{ChecksumStatus, StreamMetrics, StreamReader};

/// Which origin the script's stored offsets are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetOrigin {
    /// Offsets count from the first overlay byte.
    OverlayRelative,
    /// Offsets count from the first image byte.
    ImageAbsolute,
    /// Offsets count from the first compressed stream, that is from the end
    /// of the overlay header.
    FirstStreamRelative,
}

impl OffsetOrigin {
    /// Every origin the mapping will try, in preference order.
    pub const ALL: [Self; 3] = [
        Self::OverlayRelative,
        Self::ImageAbsolute,
        Self::FirstStreamRelative,
    ];

    const fn base(self, overlay: &Overlay, first_stream: u64) -> u64 {
        match self {
            Self::OverlayRelative => overlay.offset,
            Self::ImageAbsolute => 0,
            Self::FirstStreamRelative => first_stream,
        }
    }
}

/// How a record was matched to a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// The record's declared checksum and inflated size identified a stream.
    Content,
    /// The record's stored deflate offset identified a stream.
    Offset,
}

/// One row of [`FileMap::list`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Index into the script's file table.
    pub record_index: usize,
    /// Index of the matching chain stream, when one was found.
    pub stream_index: Option<u32>,
    /// Which evidence matched it.
    pub matched_by: Option<MatchKind>,
    /// Declared inflated size from the record.
    pub declared_inflated_size: u32,
    /// Measured inflated size of the matching stream, when mapped.
    pub stream_inflated_size: Option<u64>,
    /// Declared CRC-32; zero means the package does not check it.
    pub declared_crc32: u32,
    /// Length in bytes of the destination path, never its contents.
    pub path_len: usize,
}

/// Records mapped onto streams.
#[derive(Debug, Clone)]
pub struct FileMap {
    origin: Option<OffsetOrigin>,
    entries: Vec<Entry>,
    streams: Vec<StreamMetrics>,
    unnamed: Vec<u32>,
}

/// Number of records an origin would match, ignoring already-claimed
/// streams, used only to choose between origins.
fn offset_match_count(
    table: &FileTable,
    streams: &[StreamRecord],
    overlay: &Overlay,
    first_stream: u64,
    origin: OffsetOrigin,
) -> usize {
    let base = origin.base(overlay, first_stream);
    table
        .records()
        .iter()
        .filter(|record| record.offsets_plausible)
        .filter(|record| {
            let want = base.wrapping_add(u64::from(record.deflate_start));
            streams
                .iter()
                .any(|stream| stream.metrics.compressed_offset == want)
        })
        .count()
}

impl FileMap {
    /// Builds the mapping from a parsed script and a walked chain.
    #[must_use]
    pub fn build(table: &FileTable, streams: &[StreamRecord], overlay: &Overlay) -> Self {
        let first_stream = streams
            .first()
            .map_or(overlay.offset, |stream| stream.metrics.compressed_offset);
        let origin = OffsetOrigin::ALL
            .into_iter()
            .map(|origin| {
                (
                    offset_match_count(table, streams, overlay, first_stream, origin),
                    origin,
                )
            })
            .max_by_key(|(count, _)| *count)
            .filter(|(count, _)| *count > 0)
            .map(|(_, origin)| origin);

        let mut claimed = BTreeSet::new();
        let mut resolved: Vec<Option<(u32, MatchKind)>> = alloc::vec![None; table.len()];

        // Pass one: verifiable content evidence.
        for (record_index, record) in table.records().iter().enumerate() {
            if record.crc32 == 0 {
                continue;
            }
            let found = streams.iter().find(|stream| {
                !claimed.contains(&stream.index)
                    && stream.metrics.computed_crc32 == record.crc32
                    && stream.metrics.inflated_len == u64::from(record.inflated_size)
            });
            if let Some(stream) = found {
                claimed.insert(stream.index);
                resolved[record_index] = Some((stream.index, MatchKind::Content));
            }
        }

        // Pass two: the stored offsets, under the best-supported origin.
        if let Some(origin) = origin {
            let base = origin.base(overlay, first_stream);
            for (record_index, record) in table.records().iter().enumerate() {
                if resolved[record_index].is_some() || !record.offsets_plausible {
                    continue;
                }
                let want = base.wrapping_add(u64::from(record.deflate_start));
                let found = streams.iter().find(|stream| {
                    !claimed.contains(&stream.index) && stream.metrics.compressed_offset == want
                });
                if let Some(stream) = found {
                    claimed.insert(stream.index);
                    resolved[record_index] = Some((stream.index, MatchKind::Offset));
                }
            }
        }

        let entries = table
            .records()
            .iter()
            .enumerate()
            .map(|(record_index, record)| {
                let matched = resolved[record_index];
                Entry {
                    record_index,
                    stream_index: matched.map(|(index, _)| index),
                    matched_by: matched.map(|(_, kind)| kind),
                    declared_inflated_size: record.inflated_size,
                    stream_inflated_size: matched.and_then(|(index, _)| {
                        streams
                            .iter()
                            .find(|stream| stream.index == index)
                            .map(|stream| stream.metrics.inflated_len)
                    }),
                    declared_crc32: record.crc32,
                    path_len: record.path.len(),
                }
            })
            .collect();

        let unnamed = streams
            .iter()
            .map(|stream| stream.index)
            .filter(|index| !claimed.contains(index))
            .collect();

        Self {
            origin,
            entries,
            streams: streams.iter().map(|stream| stream.metrics).collect(),
            unnamed,
        }
    }

    /// Which origin the offset pass used, when it was able to use one.
    #[must_use]
    pub const fn origin(&self) -> Option<OffsetOrigin> {
        self.origin
    }

    /// Number of records matched by content evidence.
    #[must_use]
    pub fn content_matched_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.matched_by == Some(MatchKind::Content))
            .count()
    }

    /// Every record with its size, name length and mapped stream.
    #[must_use]
    pub fn list(&self) -> &[Entry] {
        &self.entries
    }

    /// Number of records that resolved to a stream.
    #[must_use]
    pub fn mapped_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.stream_index.is_some())
            .count()
    }

    /// Chain indexes of streams that no record claims.
    #[must_use]
    pub fn unnamed_streams(&self) -> &[u32] {
        &self.unnamed
    }

    /// Opens the file behind record `index` for bounded, checksum-verified
    /// reading.
    pub fn open_file(
        &self,
        table: &FileTable,
        index: usize,
        limits: Limits,
    ) -> Result<FileReader, Error> {
        let entry = self.entries.get(index).ok_or(Error::IndexOutOfRange)?;
        let record: &FileRecord = table.record(index)?;
        let stream_index = entry.stream_index.ok_or(Error::NoStreamForRecord)?;
        let metrics = self
            .streams
            .get(stream_index as usize)
            .ok_or(Error::NoStreamForRecord)?;
        Ok(FileReader {
            reader: StreamReader::new(metrics.compressed_offset, limits),
            declared_crc32: record.crc32,
            declared_inflated_size: record.inflated_size,
        })
    }

    /// Opens chain stream `index` directly, the fallback for packages whose
    /// script named nothing.
    pub fn open_stream(&self, index: u32, limits: Limits) -> Result<FileReader, Error> {
        let metrics = self
            .streams
            .get(index as usize)
            .ok_or(Error::IndexOutOfRange)?;
        Ok(FileReader {
            reader: StreamReader::new(metrics.compressed_offset, limits),
            declared_crc32: 0,
            declared_inflated_size: 0,
        })
    }
}

/// One file's bounded byte stream.
#[derive(Debug)]
pub struct FileReader {
    reader: StreamReader,
    declared_crc32: u32,
    declared_inflated_size: u32,
}

impl FileReader {
    /// Reads the next bounded chunk, returning zero at end of file.
    pub fn read<S: ImageSource, C: Cancellation>(
        &mut self,
        source: &mut S,
        cancel: &C,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        self.reader.read(source, cancel, out)
    }

    /// Whether the stream ended.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.reader.is_finished()
    }

    /// Verifies the trailing checksum, the record's checksum when the record
    /// declares one, and the declared inflated size when it declares one.
    pub fn finish<S: ImageSource>(&mut self, source: &mut S) -> Result<StreamMetrics, Error> {
        let metrics = self.reader.finish(source)?;
        if metrics.checksum == ChecksumStatus::Mismatch {
            return Err(Error::ChecksumMismatch);
        }
        if self.declared_crc32 != 0 && self.declared_crc32 != metrics.computed_crc32 {
            return Err(Error::ChecksumMismatch);
        }
        if self.declared_inflated_size != 0
            && u64::from(self.declared_inflated_size) != metrics.inflated_len
        {
            return Err(Error::ChecksumMismatch);
        }
        Ok(metrics)
    }

    /// Reads the whole file into `sink` in bounded chunks, then verifies it.
    pub fn read_all<S: ImageSource, K: Sink, C: Cancellation>(
        &mut self,
        source: &mut S,
        sink: &mut K,
        cancel: &C,
        limits: Limits,
    ) -> Result<StreamMetrics, Error> {
        let mut buffer = alloc::vec![0u8; limits.chunk_bytes()];
        loop {
            let written = self.read(source, cancel, &mut buffer)?;
            if written > 0 {
                sink.write(&buffer[..written])?;
            }
            if self.is_finished() || written == 0 {
                break;
            }
        }
        self.finish(source)
    }
}

#[cfg(test)]
mod tests {
    use super::{FileMap, OffsetOrigin};
    use crate::chain::StreamRecord;
    use crate::error::Error;
    use crate::limits::Limits;
    use crate::overlay::Overlay;
    use crate::script::{FileTable, parse};
    use crate::stream::{ChecksumStatus, StreamMetrics};
    use alloc::vec::Vec;

    fn overlay() -> Overlay {
        Overlay {
            offset: 0x1000,
            len: 0x1000,
            image_len: 0x2000,
        }
    }

    fn stream(index: u32, offset: u64) -> StreamRecord {
        StreamRecord {
            index,
            metrics: StreamMetrics {
                compressed_offset: offset,
                compressed_len: 20,
                inflated_len: 16,
                computed_crc32: 7,
                stored_crc32: 7,
                checksum: ChecksumStatus::Match,
            },
            resync_skip: 0,
        }
    }

    fn table(entries: &[(&[u8], u32, u32)]) -> FileTable {
        let mut script = Vec::new();
        for (path, start, end) in entries {
            script.extend_from_slice(&crate::testing::encode_file_record(
                path, *start, *end, 16, 0,
            ));
        }
        parse(&script, &overlay(), &Limits::DEFAULT).unwrap()
    }

    #[test]
    fn prefers_the_overlay_relative_origin() {
        let table = table(&[(b"a.dat".as_slice(), 0x100, 0x120)]);
        let streams = [stream(0, 0x1100)];
        let map = FileMap::build(&table, &streams, &overlay());
        assert_eq!(map.origin(), Some(OffsetOrigin::OverlayRelative));
        assert_eq!(map.mapped_count(), 1);
        assert_eq!(map.list()[0].stream_index, Some(0));
        assert_eq!(map.list()[0].path_len, 5);
        assert!(map.unnamed_streams().is_empty());
    }

    #[test]
    fn falls_back_to_the_absolute_origin() {
        let table = table(&[(b"a.dat".as_slice(), 0x100, 0x120)]);
        let streams = [stream(0, 0x100), stream(1, 0x400)];
        let map = FileMap::build(&table, &streams, &overlay());
        assert_eq!(map.origin(), Some(OffsetOrigin::ImageAbsolute));
        assert_eq!(map.mapped_count(), 1);
        assert_eq!(map.unnamed_streams(), &[1]);
    }

    #[test]
    fn reports_unmapped_records() {
        let table = table(&[(b"a.dat".as_slice(), 0x100, 0x120)]);
        let streams = [stream(0, 0x1999)];
        let map = FileMap::build(&table, &streams, &overlay());
        assert_eq!(map.mapped_count(), 0);
        assert_eq!(map.list()[0].stream_index, None);
        assert_eq!(map.unnamed_streams(), &[0]);
        assert_eq!(
            map.open_file(&table, 0, Limits::DEFAULT).err(),
            Some(Error::NoStreamForRecord)
        );
        assert_eq!(
            map.open_file(&table, 4, Limits::DEFAULT).err(),
            Some(Error::IndexOutOfRange)
        );
        assert_eq!(
            map.open_stream(9, Limits::DEFAULT).err(),
            Some(Error::IndexOutOfRange)
        );
    }
}
