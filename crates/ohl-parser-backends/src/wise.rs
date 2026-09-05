//! The Wise package back end, driven by the pull-model protocol.
//!
//! A Wise installer is a PE image whose overlay holds a chain of raw DEFLATE
//! streams, each followed by a CRC-32 of its own inflated bytes, and whose
//! second stream is a script binary carrying the file records. Nothing in it
//! is indexed: the only way to learn what a package contains is to inflate
//! the whole chain once, measuring every stream, and then match the script's
//! records against those measurements. That is what [`WiseBackend::advance`]
//! does, one bounded unit of work per call.
//!
//! # Why the walk is re-implemented here
//!
//! `ohl_wise::read_package` and `ohl_wise::Chain` are written for a blocking
//! source: they call `read_at` and expect bytes. This crate cannot block, so
//! the walk is expressed as a resumable state machine over
//! [`ohl_wise::StreamReader`], which *is* resumable — a read that misses the
//! window leaves the reader untouched, so the same call simply runs again
//! once the parent has answered. Everything else — the overlay walk, the
//! header scan, the script parser, the record-to-stream mapping and the
//! checksum arithmetic — is used from `ohl-wise` unchanged.
//!
//! The stream chain's resynchronisation policy is reproduced from
//! [`ohl_wise::chain`]: a stream that does not inflate, or whose checksum
//! does not match, is retried at up to `Limits::max_resync_skip` later
//! offsets before the first attempt's result is accepted as-is or the walk
//! fails.
//!
//! # Memory
//!
//! One [`ohl_wise::StreamReader`] is allocated per backend and reused for
//! every stream through `restart`, because the worker image's whole heap is a
//! fixed arena with no reclamation. The only other growing allocations are
//! the script binary, the stream table and the file table, all bounded by
//! [`ohl_wise::Limits`].

use alloc::vec;
use alloc::vec::Vec;

use ohl_wise::chain::StreamRecord;
use ohl_wise::script::{FileTable, StreamEvidence, parse_with_evidence};
use ohl_wise::{
    ChecksumStatus, Error as WiseError, FileMap, Limit, Limits, NeverCancelled, Overlay,
    StreamMetrics, StreamReader, find_overlay, locate_first_stream,
};

use crate::window::WindowSource;

/// The first token of the reserved range that names an unnamed chain stream.
///
/// Record tokens are script-record indexes, bounded by
/// `Limits::max_file_records`, so the two ranges cannot meet.
pub const UNNAMED_TOKEN_BASE: u64 = 1 << 32;

/// One resolved, offerable entry of a Wise package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WiseEntry {
    /// The opaque token the parent will present to stream it.
    pub token: u64,
    /// The measured inflated size, which is exactly what streaming emits.
    pub size_bytes: u64,
    /// Where the entry's compressed bytes start in the image.
    pub compressed_offset: u64,
    /// The record's declared checksum, or zero when it declares none.
    pub declared_crc32: u32,
}

/// What one call to [`WiseBackend::advance`] achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// More work remains; call again.
    Working,
    /// A source read is armed on the window; answer it and call again.
    NeedRead,
    /// The package is walked and its entries are resolved.
    Ready,
}

/// Where the walk is.
#[derive(Debug)]
enum Stage {
    Overlay,
    Header(Overlay),
    Walk(Walk),
    Script(Script),
    Ready,
}

/// The resumable chain walk.
#[derive(Debug)]
struct Walk {
    overlay: Overlay,
    cursor: u64,
    index: u32,
    total_inflated: u64,
    /// The resynchronisation skip the current attempt uses.
    skip: u8,
    /// Whether a reader is already running for the current attempt.
    started: bool,
    /// The unskipped attempt's result, kept while later skips are tried.
    first: Option<Result<StreamMetrics, WiseError>>,
}

/// The resumable script inflate that follows the walk.
#[derive(Debug)]
struct Script {
    overlay: Overlay,
    started: bool,
    bytes: Vec<u8>,
}

/// The Wise back end.
pub struct WiseBackend {
    limits: Limits,
    stage: Stage,
    reader: StreamReader,
    scratch: Vec<u8>,
    streams: Vec<StreamRecord>,
    table: FileTable,
    map: Option<FileMap>,
}

impl core::fmt::Debug for WiseBackend {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WiseBackend")
            .field("stage", &self.stage)
            .field("streams", &self.streams.len())
            .field("records", &self.table.len())
            .finish_non_exhaustive()
    }
}

/// Whether `error` only means "the window did not hold those bytes".
const fn is_window_miss(error: &WiseError) -> bool {
    matches!(error, WiseError::SourceFailed)
}

impl WiseBackend {
    /// A back end that will walk the package in the pinned source.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            scratch: vec![0u8; limits.chunk_bytes()],
            reader: StreamReader::new(0, limits),
            limits,
            stage: Stage::Overlay,
            streams: Vec::new(),
            table: FileTable::default(),
            map: None,
        }
    }

    /// The walked streams, in chain order.
    #[must_use]
    pub fn streams(&self) -> &[StreamRecord] {
        &self.streams
    }

    /// Performs one bounded unit of the walk.
    ///
    /// # Errors
    /// A fixed [`WiseError`]. A [`WiseError::SourceFailed`] is never returned:
    /// a window miss becomes [`Advance::NeedRead`].
    pub fn advance(&mut self, source: &mut WindowSource) -> Result<Advance, WiseError> {
        match core::mem::replace(&mut self.stage, Stage::Ready) {
            Stage::Overlay => self.advance_overlay(source),
            Stage::Header(overlay) => self.advance_header(source, overlay),
            Stage::Walk(walk) => self.advance_walk(source, walk),
            Stage::Script(script) => self.advance_script(source, script),
            Stage::Ready => {
                self.stage = Stage::Ready;
                Ok(Advance::Ready)
            }
        }
    }

    /// Locates the overlay, over one pinned window at the image's start.
    ///
    /// `find_overlay` treats a source failure as "not an executable", so it
    /// cannot be resumed across a window move; it runs pinned instead, which
    /// bounds the PE header region to one window exactly as the parent's own
    /// header-prefix ceiling does.
    fn advance_overlay(&mut self, source: &mut WindowSource) -> Result<Advance, WiseError> {
        if !source.ensure_at(0) {
            self.stage = Stage::Overlay;
            return Ok(Advance::NeedRead);
        }
        source.pin();
        let located = find_overlay(source, &self.limits);
        source.unpin();
        let overlay = located?;
        self.stage = Stage::Header(overlay);
        Ok(Advance::Working)
    }

    fn advance_header(
        &mut self,
        source: &mut WindowSource,
        overlay: Overlay,
    ) -> Result<Advance, WiseError> {
        if !source.ensure_at(overlay.offset) {
            self.stage = Stage::Header(overlay);
            return Ok(Advance::NeedRead);
        }
        // The scan tries to inflate candidate offsets and treats a failure as
        // "not a stream", so it runs over one pinned window too: the first
        // stream must be locatable inside it, which the format's own
        // header-scan ceiling already assumes.
        source.pin();
        let located = locate_first_stream(source, &overlay, &self.limits, &NeverCancelled);
        source.unpin();
        let header = located?;
        self.stage = Stage::Walk(Walk {
            cursor: header.first_stream_offset,
            overlay,
            index: 0,
            total_inflated: 0,
            skip: 0,
            started: false,
            first: None,
        });
        Ok(Advance::Working)
    }

    /// Drives the active reader until the stream ends, the window misses, or
    /// the attempt fails.
    fn drive_attempt(
        &mut self,
        source: &mut WindowSource,
    ) -> Result<Option<StreamMetrics>, WiseError> {
        loop {
            if self.reader.is_finished() {
                return self.reader.finish(source).map(Some);
            }
            let written = self
                .reader
                .read(source, &NeverCancelled, &mut self.scratch)?;
            if written == 0 && !self.reader.is_finished() {
                return Err(WiseError::Truncated);
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one state machine; splitting it would hide the walk's shape"
    )]
    fn advance_walk(
        &mut self,
        source: &mut WindowSource,
        mut walk: Walk,
    ) -> Result<Advance, WiseError> {
        // Fewest overlay bytes that could still hold another stream, as in
        // `ohl_wise::chain`.
        const MINIMUM_STREAM_BYTES: u64 = 3;

        let end = walk.overlay.end();
        if walk.cursor >= end || end.saturating_sub(walk.cursor) < MINIMUM_STREAM_BYTES {
            self.stage = Stage::Script(Script {
                overlay: walk.overlay,
                started: false,
                bytes: Vec::new(),
            });
            return Ok(Advance::Working);
        }
        if walk.index >= self.limits.max_streams {
            return Err(WiseError::LimitExceeded(Limit::Streams));
        }

        let attempt_offset = walk.cursor.saturating_add(u64::from(walk.skip));
        if attempt_offset >= end {
            return self.finish_attempts(walk);
        }
        if !walk.started {
            self.reader.restart(attempt_offset);
            walk.started = true;
        }

        let outcome = self.drive_attempt(source);
        match outcome {
            Ok(Some(metrics)) if metrics.checksum == ChecksumStatus::Match => {
                self.accept(&mut walk, metrics)?;
                self.stage = Stage::Walk(walk);
                Ok(Advance::Working)
            }
            Ok(Some(metrics)) => {
                if walk.skip == 0 {
                    walk.first = Some(Ok(metrics));
                }
                self.next_attempt(walk)
            }
            Ok(None) => Err(WiseError::Truncated),
            Err(error) if is_window_miss(&error) => {
                self.stage = Stage::Walk(walk);
                Ok(Advance::NeedRead)
            }
            Err(error @ (WiseError::Cancelled | WiseError::LimitExceeded(_))) => Err(error),
            Err(error) => {
                if walk.skip == 0 {
                    walk.first = Some(Err(error));
                }
                self.next_attempt(walk)
            }
        }
    }

    /// Moves on to the next resynchronisation offset, or concludes the stream.
    fn next_attempt(&mut self, mut walk: Walk) -> Result<Advance, WiseError> {
        if walk.skip < self.limits.max_resync_skip {
            walk.skip += 1;
            walk.started = false;
            self.stage = Stage::Walk(walk);
            return Ok(Advance::Working);
        }
        self.finish_attempts(walk)
    }

    /// Accepts the unskipped attempt's result once every skip was tried.
    fn finish_attempts(&mut self, mut walk: Walk) -> Result<Advance, WiseError> {
        match walk.first.take() {
            // It inflated but its checksum did not match: yield it, flagged,
            // exactly as `ohl_wise::chain` does, and keep walking.
            Some(Ok(metrics)) => {
                walk.skip = 0;
                walk.started = false;
                self.accept(&mut walk, metrics)?;
                self.stage = Stage::Walk(walk);
                Ok(Advance::Working)
            }
            Some(Err(error)) => Err(error),
            None => Err(WiseError::Truncated),
        }
    }

    /// Records one walked stream and moves the cursor past it.
    fn accept(&mut self, walk: &mut Walk, metrics: StreamMetrics) -> Result<(), WiseError> {
        walk.total_inflated = walk
            .total_inflated
            .checked_add(metrics.inflated_len)
            .ok_or(WiseError::LimitExceeded(Limit::TotalInflatedBytes))?;
        if walk.total_inflated > self.limits.max_total_inflated_bytes {
            return Err(WiseError::LimitExceeded(Limit::TotalInflatedBytes));
        }
        self.streams.push(StreamRecord {
            index: walk.index,
            metrics,
            resync_skip: walk.skip,
        });
        walk.index = walk
            .index
            .checked_add(1)
            .ok_or(WiseError::LimitExceeded(Limit::Streams))?;
        walk.cursor = metrics.next_offset();
        walk.skip = 0;
        walk.started = false;
        walk.first = None;
        Ok(())
    }

    fn advance_script(
        &mut self,
        source: &mut WindowSource,
        mut script: Script,
    ) -> Result<Advance, WiseError> {
        // The second stream is the script binary; a package with fewer
        // streams simply has no file table and keeps its streams reachable by
        // index.
        let Some(record) = self.streams.get(1).copied() else {
            self.finish_mapping(&script.overlay, &[]);
            return Ok(Advance::Ready);
        };
        if !script.started {
            self.reader.restart(record.offset());
            script.started = true;
        }
        let ceiling = self
            .limits
            .max_inflated_bytes_per_stream
            .min(self.limits.max_script_bytes as u64);
        loop {
            if self.reader.is_finished() {
                let bytes = core::mem::take(&mut script.bytes);
                self.finish_mapping(&script.overlay, &bytes);
                return Ok(Advance::Ready);
            }
            match self.reader.read(source, &NeverCancelled, &mut self.scratch) {
                Ok(0) if !self.reader.is_finished() => return Err(WiseError::Truncated),
                Ok(written) => {
                    if script.bytes.len() as u64 + written as u64 > ceiling {
                        return Err(WiseError::LimitExceeded(Limit::ScriptBytes));
                    }
                    script.bytes.extend_from_slice(&self.scratch[..written]);
                }
                Err(error) if is_window_miss(&error) => {
                    self.stage = Stage::Script(script);
                    return Ok(Advance::NeedRead);
                }
                Err(WiseError::LimitExceeded(Limit::InflatedBytesPerStream)) => {
                    return Err(WiseError::LimitExceeded(Limit::ScriptBytes));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Parses the script and maps its records onto the walked streams.
    fn finish_mapping(&mut self, overlay: &Overlay, script: &[u8]) {
        self.table = if script.is_empty() {
            FileTable::default()
        } else {
            parse_with_evidence(
                script,
                overlay,
                &self.limits,
                &StreamEvidence::from_streams(&self.streams),
            )
            // A script this crate cannot parse is not a failure: the chain is
            // still walked, so every stream stays reachable as an unnamed
            // entry.
            .unwrap_or_default()
        };
        self.map = Some(FileMap::build(&self.table, &self.streams, overlay));
        self.stage = Stage::Ready;
    }

    /// The recorded name bytes of `token`, or `None` for an unnamed stream.
    #[must_use]
    pub fn recorded_name(&self, token: u64) -> Option<&[u8]> {
        if token >= UNNAMED_TOKEN_BASE {
            return None;
        }
        let index = usize::try_from(token).ok()?;
        self.table
            .records()
            .get(index)
            .map(|record| record.path.as_bytes())
    }

    /// Every resolved entry, records first and unnamed streams after them, in
    /// strictly increasing token order.
    ///
    /// A stream whose trailing checksum did not match at walk time is *not*
    /// offered: streaming it would verify the same checksum and fail the
    /// request, which would cost the whole import an entry the package itself
    /// says is damaged. The walk still counts it, so nothing is silently
    /// renumbered.
    #[must_use]
    pub fn entries(&self) -> Vec<WiseEntry> {
        let Some(map) = self.map.as_ref() else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for entry in map.list() {
            let (Some(stream_index), Some(size)) = (entry.stream_index, entry.stream_inflated_size)
            else {
                // A record no stream backs cannot be streamed, so it is not
                // offered; its bytes stay reachable as an unnamed stream.
                continue;
            };
            let Some(stream) = self.streams.get(stream_index as usize) else {
                continue;
            };
            if stream.checksum() != ChecksumStatus::Match {
                continue;
            }
            let Ok(token) = u64::try_from(entry.record_index) else {
                continue;
            };
            entries.push(WiseEntry {
                token,
                size_bytes: size,
                compressed_offset: stream.offset(),
                declared_crc32: entry.declared_crc32,
            });
        }
        for index in map.unnamed_streams() {
            let Some(stream) = self.streams.get(*index as usize) else {
                continue;
            };
            if stream.checksum() != ChecksumStatus::Match {
                continue;
            }
            entries.push(WiseEntry {
                token: UNNAMED_TOKEN_BASE.saturating_add(u64::from(*index)),
                size_bytes: stream.inflated_len(),
                compressed_offset: stream.offset(),
                declared_crc32: 0,
            });
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::{Advance, UNNAMED_TOKEN_BASE, WiseBackend};
    use crate::window::{DEFAULT_WINDOW_BYTES, WindowSource};
    use alloc::vec;
    use alloc::vec::Vec;
    use ohl_wise::Limits;
    use ohl_wise::testing::{PackageOptions, SyntheticFile, build_package};

    /// Runs the whole walk against an in-memory image, answering every miss.
    fn walk(image: &[u8], capacity: usize) -> (WiseBackend, WindowSource, usize) {
        let mut source = WindowSource::new(image.len() as u64, capacity);
        let mut backend = WiseBackend::new(Limits::DEFAULT);
        let mut reads = 0;
        for _ in 0..100_000 {
            match backend.advance(&mut source).expect("the walk succeeds") {
                Advance::Working => {}
                Advance::NeedRead => {
                    let pending = source.take_pending().expect("a miss arms a read");
                    reads += 1;
                    let start = usize::try_from(pending.offset).expect("host offset");
                    let end = start + pending.length as usize;
                    assert!(source.deliver(pending.offset, &image[start..end]));
                }
                Advance::Ready => return (backend, source, reads),
            }
        }
        panic!("the walk did not finish");
    }

    #[test]
    fn a_synthetic_package_walks_to_the_same_result_as_the_blocking_reader() {
        let files = vec![
            SyntheticFile::new(b"maps\\one.dat", vec![1u8; 40_000]),
            SyntheticFile::new(b"cfg\\two.cfg", b"alpha beta gamma".repeat(64)),
        ];
        let built = build_package(&PackageOptions::with_files(files.clone()));
        let reference = ohl_wise::read_package(
            &mut ohl_wise::SliceSource::new(&built.image),
            None,
            Limits::DEFAULT,
            &ohl_wise::NeverCancelled,
        )
        .expect("the blocking reader reads it");

        let (backend, _source, reads) = walk(&built.image, DEFAULT_WINDOW_BYTES);
        assert!(reads > 0);
        assert_eq!(backend.streams().len(), reference.streams().len());
        for (walked, expected) in backend.streams().iter().zip(reference.streams()) {
            assert_eq!(walked.metrics, expected.metrics);
        }

        // The two records, plus the bitmap and the script, which no record
        // claims and which stay reachable as unnamed streams.
        let entries = backend.entries();
        assert_eq!(entries.len(), files.len() + 2);
        let named: Vec<_> = entries
            .iter()
            .filter(|entry| entry.token < UNNAMED_TOKEN_BASE)
            .collect();
        assert_eq!(named.len(), files.len());
        for (entry, file) in named.iter().zip(&files) {
            assert_eq!(entry.size_bytes, file.content.len() as u64);
        }
    }

    #[test]
    fn a_tiny_window_only_costs_more_reads() {
        // Incompressible content, so the image is genuinely larger than the
        // narrow window.
        let mut state = 0x1234_5678u32;
        let content: Vec<u8> = (0..200_000)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let built = build_package(&PackageOptions::with_files(vec![SyntheticFile::new(
            b"data\\one.bin",
            content,
        )]));
        let (wide, _, wide_reads) = walk(&built.image, DEFAULT_WINDOW_BYTES);
        let (narrow, _, narrow_reads) = walk(&built.image, crate::window::MINIMUM_WINDOW_BYTES);
        assert!(
            narrow_reads > wide_reads,
            "narrow {narrow_reads} wide {wide_reads}"
        );
        assert_eq!(wide.entries(), narrow.entries());
    }

    #[test]
    fn unnamed_streams_are_offered_in_the_reserved_token_range() {
        let built = build_package(&PackageOptions::with_files(Vec::new()));
        let (backend, _, _) = walk(&built.image, DEFAULT_WINDOW_BYTES);
        assert!(
            backend
                .entries()
                .iter()
                .all(|entry| entry.token >= UNNAMED_TOKEN_BASE)
        );
    }
}
