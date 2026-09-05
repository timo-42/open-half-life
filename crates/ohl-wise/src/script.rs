//! The file table carried by the second inflated stream (the script binary).
//!
//! # Coverage, and what "fail closed" means here
//!
//! The only publicly documented structure inside the script binary is the
//! per-file record, whose field table is reproduced in
//! `docs/FORMAT_SOURCES.md`:
//!
//! | offset | size | field                                            |
//! |-------:|-----:|--------------------------------------------------|
//! |      0 |    2 | unknown                                          |
//! |      2 |    4 | deflate start offset (`u32`, little endian)      |
//! |      6 |    4 | deflate end offset (`u32`, little endian)        |
//! |     10 |    2 | file date (`u16`)                                |
//! |     12 |    2 | file time (`u16`)                                |
//! |     14 |    4 | inflated size (`u32`)                            |
//! |     18 |   20 | twenty zero bytes                                |
//! |     38 |    4 | CRC-32 (checked only when non-zero)              |
//! |     42 |    n | NUL-terminated destination path                  |
//! |    42+n |   m | language-specific file texts, then a NUL         |
//!
//! **No public source documents the script's opcode vocabulary, its record
//! framing, or the length of any record other than this one.** This module
//! therefore decodes *zero* opcodes: it does not walk the script as an
//! instruction stream at all, because doing so would require guessing at
//! undocumented opcode lengths and would silently mis-skip on the first
//! unknown one. Instead it *recognises* file records structurally, requiring
//! every documented invariant to hold at once: the twenty zero bytes at
//! offset 18, a non-zero inflated size within the per-stream ceiling, and a
//! non-empty NUL-terminated path of printable bytes within
//! `Limits::max_path_bytes`. Anything else in the script is skipped as opaque
//! bytes — never interpreted, never trusted.
//!
//! # Why the stored offsets are advisory
//!
//! The stored deflate start and end offsets are recorded and validated, but a
//! record is not rejected for failing that validation, and they are not the
//! primary way a file's bytes are found. REWise's README documents that "the
//! `ScriptDeflateOffset` isn't correct on some installers, there are still
//! some unknowns on how to calculate this proper", and that is reproducible:
//! on the package this crate was validated against, no constant origin
//! relates the stored values to any measured stream position, while the
//! stored inflated size and CRC-32 match measured streams for the
//! overwhelming majority of records. Each record therefore carries
//! [`FileRecord::offsets_plausible`] (non-zero start, `start < end`, `end`
//! inside the image), the table reports [`FileTable::has_monotonic_offsets`],
//! and [`crate::extract`] maps records onto streams by verifiable content
//! evidence first, falling back to the offsets only when the checksums cannot
//! decide.
//!
//! Consequently this crate claims support for exactly one script structure —
//! the file record above — across every script version that carries it, and
//! claims support for no opcode, no language/component selection, no patch
//! operation and no multi-disc continuation. Callers that need bytes for
//! which no record was recognised use the unnamed-stream fallback in
//! [`crate::extract`], which is keyed by stream index and needs no name.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::chain::StreamRecord;
use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::overlay::Overlay;

/// Size of the documented fixed part of a file record.
pub const RECORD_PREFIX_BYTES: usize = 42;
/// Offset of the twenty zero bytes within the fixed part.
const ZERO_RUN_AT: usize = 18;
/// Length of the zero run.
const ZERO_RUN_LEN: usize = 20;

/// A destination path, stored as raw bytes and never rendered.
///
/// `Debug` prints only the length, so a record can appear in a log line
/// without ever disclosing media-derived text (`docs/MEDIA_IMPORT.md`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PathBytes(Vec<u8>);

impl PathBytes {
    /// The stored bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Number of stored bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the path is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The lowercased extension bytes after the final `.`, if any, bounded to
    /// eight bytes. Used for classification only.
    #[must_use]
    pub fn extension(&self) -> Option<Vec<u8>> {
        let start = self.0.iter().rposition(|byte| *byte == b'.')? + 1;
        let extension = self.0.get(start..)?;
        if extension.is_empty() || extension.len() > 8 {
            return None;
        }
        if extension.iter().any(|byte| *byte == b'\\' || *byte == b'/') {
            return None;
        }
        Some(extension.to_ascii_lowercase())
    }
}

impl core::fmt::Debug for PathBytes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PathBytes(<redacted, {} bytes>)", self.0.len())
    }
}

/// One recognised file record.
#[derive(Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// Destination path bytes, redacted in `Debug`.
    pub path: PathBytes,
    /// Start of the file's compressed data, relative to the overlay start.
    pub deflate_start: u32,
    /// End of the file's compressed data, relative to the overlay start.
    pub deflate_end: u32,
    /// Declared inflated size.
    pub inflated_size: u32,
    /// Declared CRC-32; zero means "not checked", per the field table.
    pub crc32: u32,
    /// Whether the stored range passed validation: a non-zero start, an end
    /// after it, and an end inside the image.
    pub offsets_plausible: bool,
    /// Packed file date, uninterpreted.
    pub date: u16,
    /// Packed file time, uninterpreted.
    pub time: u16,
    /// Byte offset of the record within the script binary.
    pub script_offset: usize,
}

impl core::fmt::Debug for FileRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileRecord")
            .field("path", &self.path)
            .field("deflate_start", &self.deflate_start)
            .field("deflate_end", &self.deflate_end)
            .field("inflated_size", &self.inflated_size)
            .finish_non_exhaustive()
    }
}

impl FileRecord {
    /// Declared compressed length.
    #[must_use]
    pub const fn compressed_len(&self) -> u32 {
        self.deflate_end.saturating_sub(self.deflate_start)
    }

    /// Whether the declared CRC-32 is meaningful.
    #[must_use]
    pub const fn has_checksum(&self) -> bool {
        self.crc32 != 0
    }
}

/// Every file record recognised in one script binary.
#[derive(Debug, Clone, Default)]
pub struct FileTable {
    records: Vec<FileRecord>,
    scanned_bytes: usize,
    monotonic: bool,
    plausible: usize,
}

impl FileTable {
    /// The recognised records, in script order.
    #[must_use]
    pub fn records(&self) -> &[FileRecord] {
        &self.records
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no record was recognised.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Number of script bytes examined.
    #[must_use]
    pub const fn scanned_bytes(&self) -> usize {
        self.scanned_bytes
    }

    /// Whether every record's stored range starts at or after the previous
    /// record's end. Reported, never required.
    #[must_use]
    pub const fn has_monotonic_offsets(&self) -> bool {
        self.monotonic
    }

    /// Number of records whose stored range passed validation.
    #[must_use]
    pub const fn plausible_offset_count(&self) -> usize {
        self.plausible
    }

    /// One record by index.
    pub fn record(&self, index: usize) -> Result<&FileRecord, Error> {
        self.records.get(index).ok_or(Error::IndexOutOfRange)
    }
}

/// Measured `(inflated length, CRC-32)` pairs from a walked chain.
///
/// A record's fixed part can be recognised at up to a few adjacent offsets
/// when neighbouring fields happen to be zero, because the twenty zero bytes
/// then appear longer than they are. Confirming a candidate against measured
/// streams removes that ambiguity without trusting anything the script says.
#[derive(Debug, Clone, Default)]
pub struct StreamEvidence {
    pairs: BTreeSet<(u64, u32)>,
}

impl StreamEvidence {
    /// Collects the evidence a walked chain provides.
    #[must_use]
    pub fn from_streams(streams: &[StreamRecord]) -> Self {
        Self {
            pairs: streams
                .iter()
                .map(|stream| (stream.metrics.inflated_len, stream.metrics.computed_crc32))
                .collect(),
        }
    }

    /// Whether some measured stream has exactly this size and checksum.
    #[must_use]
    pub fn confirms(&self, inflated_size: u32, crc32: u32) -> bool {
        crc32 != 0 && self.pairs.contains(&(u64::from(inflated_size), crc32))
    }

    /// Whether any evidence was collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

fn le_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Whether `byte` may appear in a destination path: printable, no control
/// bytes, no NUL.
const fn path_byte(byte: u8) -> bool {
    byte >= 0x20 && byte != 0x7f
}

/// Attempts to read a record at `at`. Returns the record and the offset just
/// past its path terminator.
fn recognise(
    script: &[u8],
    at: usize,
    image_len: u64,
    limits: &Limits,
) -> Option<(FileRecord, usize)> {
    let fixed = script.get(at..at.checked_add(RECORD_PREFIX_BYTES)?)?;
    if fixed[ZERO_RUN_AT..ZERO_RUN_AT + ZERO_RUN_LEN]
        .iter()
        .any(|byte| *byte != 0)
    {
        return None;
    }

    let deflate_start = le_u32(fixed, 2);
    let deflate_end = le_u32(fixed, 6);
    let date = le_u16(fixed, 10);
    let time = le_u16(fixed, 12);
    let inflated_size = le_u32(fixed, 14);
    let crc32 = le_u32(fixed, 38);

    if inflated_size == 0 || u64::from(inflated_size) > limits.max_inflated_bytes_per_stream {
        return None;
    }
    let offsets_plausible =
        deflate_start != 0 && deflate_end > deflate_start && u64::from(deflate_end) <= image_len;

    let path_start = at + RECORD_PREFIX_BYTES;
    let tail = script.get(path_start..)?;
    let scan = tail.len().min(limits.max_path_bytes.saturating_add(1));
    let terminator = tail[..scan].iter().position(|byte| *byte == 0)?;
    if terminator == 0 || terminator > limits.max_path_bytes {
        return None;
    }
    let path = &tail[..terminator];
    if !path.iter().all(|byte| path_byte(*byte)) {
        return None;
    }

    Some((
        FileRecord {
            path: PathBytes(path.to_vec()),
            deflate_start,
            deflate_end,
            inflated_size,
            crc32,
            offsets_plausible,
            date,
            time,
            script_offset: at,
        },
        path_start + terminator + 1,
    ))
}

/// How far past a position a better-aligned candidate is looked for.
const ALIGNMENT_WINDOW: usize = 8;

/// Recognises every documented file record in `script`.
///
/// `overlay` bounds every stored range. A record is never rejected for a
/// stored range that fails validation — see the module documentation — but
/// the outcome is recorded on [`FileRecord::offsets_plausible`].
pub fn parse(script: &[u8], overlay: &Overlay, limits: &Limits) -> Result<FileTable, Error> {
    parse_with_evidence(script, overlay, limits, &StreamEvidence::default())
}

/// Recognises every documented file record, disambiguating candidates that
/// overlap by a few bytes with measured stream evidence.
pub fn parse_with_evidence(
    script: &[u8],
    overlay: &Overlay,
    limits: &Limits,
    evidence: &StreamEvidence,
) -> Result<FileTable, Error> {
    if script.len() > limits.max_script_bytes {
        return Err(Error::LimitExceeded(Limit::ScriptBytes));
    }
    let mut records: Vec<FileRecord> = Vec::new();
    let mut at = 0usize;

    while at < script.len() {
        // Anchor on the first position that recognises at all, then let a
        // few later positions compete: a record's fixed part can also be
        // recognised a byte or two early when a neighbouring field's high
        // bytes are zero, and the better-supported candidate must win.
        let Some((anchor_record, anchor_next)) = recognise(script, at, overlay.image_len, limits)
        else {
            at += 1;
            continue;
        };
        let score = |record: &FileRecord| -> u32 {
            let mut score = 0;
            if evidence.confirms(record.inflated_size, record.crc32) {
                score += 4;
            }
            if record.crc32 != 0 {
                score += 1;
            }
            if record.offsets_plausible {
                score += 1;
            }
            score
        };
        let mut best = (score(&anchor_record), anchor_record, anchor_next);
        for delta in 1..ALIGNMENT_WINDOW {
            if let Some((record, next)) = recognise(script, at + delta, overlay.image_len, limits) {
                let candidate = score(&record);
                if candidate > best.0 {
                    best = (candidate, record, next);
                }
            }
        }

        if records.len() as u64 >= u64::from(limits.max_file_records) {
            return Err(Error::LimitExceeded(Limit::FileRecords));
        }
        at = best.2.max(at + 1);
        records.push(best.1);
    }

    let monotonic = records
        .windows(2)
        .all(|pair| pair[1].deflate_start >= pair[0].deflate_end);
    let plausible = records
        .iter()
        .filter(|record| record.offsets_plausible)
        .count();

    Ok(FileTable {
        records,
        scanned_bytes: script.len(),
        monotonic,
        plausible,
    })
}

#[cfg(test)]
mod tests {
    use super::{FileRecord, PathBytes, parse};
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use crate::overlay::Overlay;
    use alloc::vec::Vec;

    fn overlay(len: u64) -> Overlay {
        Overlay {
            offset: 0x1000,
            len,
            image_len: 0x1000 + len,
        }
    }

    use crate::testing::encode_file_record as encode;

    #[test]
    fn recognises_records_between_opaque_bytes() {
        let mut script = alloc::vec![0x5au8; 11];
        script.extend_from_slice(&encode(b"a\\b.dat", 100, 200, 4096, 0x1111_2222));
        script.extend_from_slice(&[0x33u8; 7]);
        script.extend_from_slice(&encode(b"c.dat", 200, 300, 8192, 0));
        let table = parse(&script, &overlay(4096), &Limits::DEFAULT).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.records()[0].path.as_bytes(), b"a\\b.dat");
        assert_eq!(table.records()[0].deflate_start, 100);
        assert!(table.records()[0].has_checksum());
        assert!(!table.records()[1].has_checksum());
        assert_eq!(table.records()[0].compressed_len(), 100);
        assert_eq!(table.record(1).unwrap().inflated_size, 8192);
        assert_eq!(table.record(9).unwrap_err(), Error::IndexOutOfRange);
        assert_eq!(table.scanned_bytes(), script.len());
        assert!(!table.is_empty());
    }

    #[test]
    fn flags_ranges_outside_the_image_without_rejecting_them() {
        let script = encode(b"x.dat", 100, 9_000, 16, 0);
        let table = parse(&script, &overlay(4096), &Limits::DEFAULT).unwrap();
        assert_eq!(table.len(), 1);
        assert!(!table.records()[0].offsets_plausible);
        assert_eq!(table.plausible_offset_count(), 0);
    }

    #[test]
    fn flags_backwards_and_non_monotonic_ranges() {
        let script = encode(b"x.dat", 300, 200, 16, 0);
        let table = parse(&script, &overlay(4096), &Limits::DEFAULT).unwrap();
        assert_eq!(table.len(), 1);
        assert!(!table.records()[0].offsets_plausible);

        let mut script = encode(b"a.dat", 300, 400, 16, 0);
        script.extend_from_slice(&encode(b"b.dat", 100, 200, 16, 0));
        let table = parse(&script, &overlay(4096), &Limits::DEFAULT).unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.plausible_offset_count(), 2);
        assert!(!table.has_monotonic_offsets());
    }

    #[test]
    fn rejects_an_oversized_declared_size() {
        let script = encode(b"x.dat", 100, 200, u32::MAX, 0);
        let limits = Limits {
            max_inflated_bytes_per_stream: 1024,
            ..Limits::DEFAULT
        };
        let table = parse(&script, &overlay(4096), &limits).unwrap();
        assert!(
            table
                .records()
                .iter()
                .all(|record| u64::from(record.inflated_size) <= 1024),
            "no record may declare more than the ceiling"
        );
    }

    #[test]
    fn evidence_disambiguates_overlapping_candidates() {
        use crate::chain::StreamRecord;
        use crate::stream::{ChecksumStatus, StreamMetrics};

        let mut script = alloc::vec![0x5au8; 16];
        script.extend_from_slice(&encode(b"one.dat", 300, 400, 4096, 0x1234_5678));
        let streams = [StreamRecord {
            index: 0,
            metrics: StreamMetrics {
                compressed_offset: 0x1000,
                compressed_len: 10,
                inflated_len: 4096,
                computed_crc32: 0x1234_5678,
                stored_crc32: 0x1234_5678,
                checksum: ChecksumStatus::Match,
            },
            resync_skip: 0,
        }];
        let evidence = super::StreamEvidence::from_streams(&streams);
        assert!(!evidence.is_empty());
        assert!(evidence.confirms(4096, 0x1234_5678));
        assert!(!evidence.confirms(4096, 0));
        let table =
            super::parse_with_evidence(&script, &overlay(0x4000), &Limits::DEFAULT, &evidence)
                .unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table.records()[0].script_offset, 16);
        assert_eq!(table.records()[0].inflated_size, 4096);
    }

    #[test]
    fn rejects_unterminated_and_unprintable_paths() {
        let mut script = encode(b"x.dat", 100, 200, 16, 0);
        script.pop();
        assert!(
            parse(&script, &overlay(4096), &Limits::DEFAULT)
                .unwrap()
                .is_empty()
        );

        let script = encode(b"bad\x01name", 100, 200, 16, 0);
        assert!(
            parse(&script, &overlay(4096), &Limits::DEFAULT)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn enforces_the_script_and_record_ceilings() {
        let script = encode(b"x.dat", 100, 200, 16, 0);
        let limits = Limits {
            max_script_bytes: 4,
            ..Limits::DEFAULT
        };
        assert_eq!(
            parse(&script, &overlay(4096), &limits).unwrap_err(),
            Error::LimitExceeded(Limit::ScriptBytes)
        );

        let mut many = Vec::new();
        for index in 0..3u32 {
            many.extend_from_slice(&encode(
                b"x.dat",
                100 + index * 100,
                150 + index * 100,
                16,
                0,
            ));
        }
        let limits = Limits {
            max_file_records: 2,
            ..Limits::DEFAULT
        };
        assert_eq!(
            parse(&many, &overlay(4096), &limits).unwrap_err(),
            Error::LimitExceeded(Limit::FileRecords)
        );
    }

    #[test]
    fn debug_output_redacts_the_path() {
        let script = encode(b"secret.dat", 100, 200, 16, 0);
        let table = parse(&script, &overlay(4096), &Limits::DEFAULT).unwrap();
        let record: &FileRecord = &table.records()[0];
        let text = alloc::format!("{record:?}");
        assert!(!text.contains("secret"));
        assert!(text.contains("<redacted, 10 bytes>"));
    }

    #[test]
    fn extensions_are_bounded_and_lowercased() {
        let path = PathBytes(b"dir\\Name.TXT".to_vec());
        assert_eq!(path.extension().unwrap(), b"txt".to_vec());
        assert_eq!(path.len(), 12);
        assert!(!path.is_empty());
        assert!(PathBytes(b"dir\\name".to_vec()).extension().is_none());
        assert!(PathBytes(b"a.b\\name".to_vec()).extension().is_none());
    }
}
