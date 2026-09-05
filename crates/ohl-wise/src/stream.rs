//! One raw DEFLATE stream plus its trailing CRC-32.
//!
//! Documented shape (see `docs/FORMAT_SOURCES.md`): after the Wise header the
//! overlay holds raw DEFLATE data without file headers, and each compressed
//! entry is followed by a CRC-32 of the inflated bytes; entries continue to
//! end of file.
//!
//! Nothing here trusts a declared size: the compressed length is whatever the
//! DEFLATE decoder consumed before the final block, the inflated length is
//! whatever it produced, and both are capped by [`Limits`].

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use ohl_core::CheckedArithmetic as _;

use crate::crc32::Crc32;
use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::source::{Cancellation, ImageSource, Sink};

/// Whether a stream's trailing checksum agreed with its inflated bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksumStatus {
    /// The trailing checksum matched.
    Match,
    /// The trailing checksum did not match.
    Mismatch,
    /// The image ended before a trailing checksum could be read.
    Absent,
}

impl ChecksumStatus {
    /// Whether the checksum matched.
    #[must_use]
    pub const fn is_match(self) -> bool {
        matches!(self, Self::Match)
    }
}

/// Sizes and checksums measured while inflating one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamMetrics {
    /// Offset of the first compressed byte within the image.
    pub compressed_offset: u64,
    /// Number of compressed bytes the decoder consumed.
    pub compressed_len: u64,
    /// Number of inflated bytes produced.
    pub inflated_len: u64,
    /// CRC-32 computed over the inflated bytes.
    pub computed_crc32: u32,
    /// CRC-32 read from the four bytes after the compressed data.
    pub stored_crc32: u32,
    /// Whether the two agree.
    pub checksum: ChecksumStatus,
}

impl StreamMetrics {
    /// Offset just past this stream's trailing checksum, that is where the
    /// next stream begins.
    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        let trailer = match self.checksum {
            ChecksumStatus::Absent => 0,
            _ => 4,
        };
        self.compressed_offset + self.compressed_len + trailer
    }
}

/// A bounded, resumable reader over one raw DEFLATE stream.
pub struct StreamReader {
    state: Box<InflateState>,
    limits: Limits,
    start: u64,
    read_cursor: u64,
    input: Vec<u8>,
    input_pos: usize,
    input_len: usize,
    consumed: u64,
    inflated: u64,
    crc: Crc32,
    finished: bool,
    stalled: bool,
}

impl core::fmt::Debug for StreamReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamReader")
            .field("compressed_len", &self.consumed)
            .field("inflated_len", &self.inflated)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl StreamReader {
    /// A reader over the stream starting at `offset`.
    #[must_use]
    pub fn new(offset: u64, limits: Limits) -> Self {
        let chunk = limits.chunk_bytes();
        Self {
            state: InflateState::new_boxed(DataFormat::Raw),
            limits,
            start: offset,
            read_cursor: offset,
            input: vec![0u8; chunk],
            input_pos: 0,
            input_len: 0,
            consumed: 0,
            inflated: 0,
            crc: Crc32::new(),
            finished: false,
            stalled: false,
        }
    }

    /// Whether the final DEFLATE block has been decoded.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }

    /// Reuses this reader for the stream starting at `offset`.
    ///
    /// Every measurement, the decoder state and the staging buffer are reset,
    /// so a caller that walks many streams allocates one reader instead of
    /// one per stream. That matters for the freestanding worker, whose whole
    /// heap is a fixed arena.
    pub fn restart(&mut self, offset: u64) {
        self.state.reset(DataFormat::Raw);
        self.start = offset;
        self.read_cursor = offset;
        self.input_pos = 0;
        self.input_len = 0;
        self.consumed = 0;
        self.inflated = 0;
        self.crc = Crc32::new();
        self.finished = false;
        self.stalled = false;
    }

    /// Inflated bytes produced so far.
    #[must_use]
    pub const fn inflated_len(&self) -> u64 {
        self.inflated
    }

    /// Compressed bytes consumed so far.
    #[must_use]
    pub const fn compressed_len(&self) -> u64 {
        self.consumed
    }

    fn refill<S: ImageSource>(&mut self, source: &mut S) -> Result<bool, Error> {
        let read = source.read_at(self.read_cursor, &mut self.input)?;
        self.input_pos = 0;
        self.input_len = read;
        if read == 0 {
            return Ok(false);
        }
        self.read_cursor = self.read_cursor.checked_add_bounded(read as u64)?;
        Ok(true)
    }

    /// Inflates the next bounded chunk into `out`, returning the number of
    /// bytes written. Zero means the stream ended.
    pub fn read<S: ImageSource, C: Cancellation>(
        &mut self,
        source: &mut S,
        cancel: &C,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        if self.finished || out.is_empty() {
            return Ok(0);
        }
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if self.input_pos == self.input_len && !self.refill(source)? {
                return Err(Error::Truncated);
            }

            let available = &self.input[self.input_pos..self.input_len];
            let result = inflate(&mut self.state, available, out, MZFlush::None);
            self.input_pos = self
                .input_pos
                .checked_add(result.bytes_consumed)
                .ok_or(Error::DecompressionFailed)?;
            self.consumed = self
                .consumed
                .checked_add_bounded(result.bytes_consumed as u64)?;
            if self.consumed > self.limits.max_compressed_bytes_per_stream {
                return Err(Error::LimitExceeded(Limit::CompressedBytesPerStream));
            }

            let written = result.bytes_written;
            if written > out.len() {
                return Err(Error::DecompressionFailed);
            }
            self.inflated = self.inflated.checked_add_bounded(written as u64)?;
            if self.inflated > self.limits.max_inflated_bytes_per_stream {
                return Err(Error::LimitExceeded(Limit::InflatedBytesPerStream));
            }
            self.crc.update(&out[..written]);

            match result.status {
                Ok(MZStatus::StreamEnd) => {
                    self.finished = true;
                    return Ok(written);
                }
                Ok(_) => {}
                Err(_) => return Err(Error::DecompressionFailed),
            }

            if written > 0 {
                return Ok(written);
            }
            if result.bytes_consumed == 0 {
                // No progress at all: allow exactly one refill attempt before
                // declaring the stream undecodable, so a stalled decoder can
                // never spin.
                if self.stalled {
                    return Err(Error::DecompressionFailed);
                }
                self.stalled = true;
                self.input_pos = self.input_len;
            } else {
                self.stalled = false;
            }
        }
    }

    /// Reads the trailing checksum and reports the stream's measurements.
    /// The stream must have ended.
    pub fn finish<S: ImageSource>(&mut self, source: &mut S) -> Result<StreamMetrics, Error> {
        if !self.finished {
            return Err(Error::Truncated);
        }
        let trailer_at = self.start.checked_add_bounded(self.consumed)?;
        let mut trailer = [0u8; 4];
        let read = source.read_at(trailer_at, &mut trailer)?;
        let computed = self.crc.finish();
        let (stored, checksum) = if read == 4 {
            let stored = u32::from_le_bytes(trailer);
            let status = if stored == computed {
                ChecksumStatus::Match
            } else {
                ChecksumStatus::Mismatch
            };
            (stored, status)
        } else {
            (0, ChecksumStatus::Absent)
        };
        Ok(StreamMetrics {
            compressed_offset: self.start,
            compressed_len: self.consumed,
            inflated_len: self.inflated,
            computed_crc32: computed,
            stored_crc32: stored,
            checksum,
        })
    }
}

/// Inflates the whole stream at `offset` into `sink`, in chunks of at most
/// `limits.max_chunk_bytes`, and reports its measurements.
pub fn inflate_stream<S: ImageSource, K: Sink, C: Cancellation>(
    source: &mut S,
    offset: u64,
    limits: Limits,
    sink: &mut K,
    cancel: &C,
) -> Result<StreamMetrics, Error> {
    let mut reader = StreamReader::new(offset, limits);
    let mut buffer = vec![0u8; limits.chunk_bytes()];
    loop {
        let written = reader.read(source, cancel, &mut buffer)?;
        if written > 0 {
            sink.write(&buffer[..written])?;
        }
        if reader.is_finished() {
            break;
        }
        if written == 0 {
            break;
        }
    }
    reader.finish(source)
}

#[cfg(test)]
mod tests {
    use super::{ChecksumStatus, inflate_stream};
    use crate::crc32::crc32;
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use crate::source::{Discard, NeverCancelled, SliceSource};
    use alloc::vec::Vec;

    fn packed(plain: &[u8], good_crc: bool) -> Vec<u8> {
        let mut bytes = miniz_oxide::deflate::compress_to_vec(plain, 6);
        let crc = if good_crc { crc32(plain) } else { 0xdead_beef };
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }

    #[test]
    fn inflates_and_confirms_a_stream() {
        let plain = alloc::vec![0x41u8; 5000];
        let image = packed(&plain, true);
        let mut source = SliceSource::new(&image);
        let mut out: Vec<u8> = Vec::new();
        let metrics =
            inflate_stream(&mut source, 0, Limits::DEFAULT, &mut out, &NeverCancelled).unwrap();
        assert_eq!(out, plain);
        assert_eq!(metrics.checksum, ChecksumStatus::Match);
        assert!(metrics.checksum.is_match());
        assert_eq!(metrics.inflated_len, 5000);
        assert_eq!(metrics.next_offset(), image.len() as u64);
    }

    #[test]
    fn reports_a_checksum_mismatch_without_failing() {
        let image = packed(b"bounded", false);
        let mut source = SliceSource::new(&image);
        let metrics = inflate_stream(
            &mut source,
            0,
            Limits::DEFAULT,
            &mut Discard,
            &NeverCancelled,
        )
        .unwrap();
        assert_eq!(metrics.checksum, ChecksumStatus::Mismatch);
    }

    #[test]
    fn reports_an_absent_trailer() {
        let mut image = miniz_oxide::deflate::compress_to_vec(b"tail", 6);
        image.truncate(image.len());
        let mut source = SliceSource::new(&image);
        let metrics = inflate_stream(
            &mut source,
            0,
            Limits::DEFAULT,
            &mut Discard,
            &NeverCancelled,
        )
        .unwrap();
        assert_eq!(metrics.checksum, ChecksumStatus::Absent);
    }

    #[test]
    fn rejects_a_truncated_stream() {
        let mut image = packed(&alloc::vec![7u8; 9000], true);
        image.truncate(12);
        let mut source = SliceSource::new(&image);
        assert_eq!(
            inflate_stream(
                &mut source,
                0,
                Limits::DEFAULT,
                &mut Discard,
                &NeverCancelled
            ),
            Err(Error::Truncated)
        );
    }

    #[test]
    fn rejects_garbage() {
        let image = [0xffu8; 64];
        let mut source = SliceSource::new(&image);
        assert_eq!(
            inflate_stream(
                &mut source,
                0,
                Limits::DEFAULT,
                &mut Discard,
                &NeverCancelled
            ),
            Err(Error::DecompressionFailed)
        );
    }

    #[test]
    fn enforces_the_inflated_ceiling() {
        let image = packed(&alloc::vec![0u8; 100_000], true);
        let mut source = SliceSource::new(&image);
        let limits = Limits {
            max_inflated_bytes_per_stream: 1024,
            ..Limits::DEFAULT
        };
        assert_eq!(
            inflate_stream(&mut source, 0, limits, &mut Discard, &NeverCancelled),
            Err(Error::LimitExceeded(Limit::InflatedBytesPerStream))
        );
    }

    #[test]
    fn a_restarted_reader_decodes_the_next_stream() {
        let first = alloc::vec![0x41u8; 700];
        let second = alloc::vec![0x42u8; 900];
        let mut image = packed(&first, true);
        let second_at = image.len() as u64;
        image.extend_from_slice(&packed(&second, true));
        let mut source = SliceSource::new(&image);

        let mut reader = super::StreamReader::new(0, Limits::DEFAULT);
        let mut buffer = alloc::vec![0u8; 256];
        let mut out: Vec<u8> = Vec::new();
        loop {
            let written = reader
                .read(&mut source, &NeverCancelled, &mut buffer)
                .unwrap();
            out.extend_from_slice(&buffer[..written]);
            if reader.is_finished() {
                break;
            }
        }
        assert_eq!(out, first);
        assert_eq!(
            reader.finish(&mut source).unwrap().checksum,
            ChecksumStatus::Match
        );

        reader.restart(second_at);
        assert!(!reader.is_finished());
        assert_eq!(reader.inflated_len(), 0);
        assert_eq!(reader.compressed_len(), 0);
        out.clear();
        loop {
            let written = reader
                .read(&mut source, &NeverCancelled, &mut buffer)
                .unwrap();
            out.extend_from_slice(&buffer[..written]);
            if reader.is_finished() {
                break;
            }
        }
        assert_eq!(out, second);
        let metrics = reader.finish(&mut source).unwrap();
        assert_eq!(metrics.checksum, ChecksumStatus::Match);
        assert_eq!(metrics.compressed_offset, second_at);
    }

    #[test]
    fn honours_cancellation() {
        struct Always;
        impl crate::source::Cancellation for Always {
            fn is_cancelled(&self) -> bool {
                true
            }
        }
        let image = packed(b"x", true);
        let mut source = SliceSource::new(&image);
        assert_eq!(
            inflate_stream(&mut source, 0, Limits::DEFAULT, &mut Discard, &Always),
            Err(Error::Cancelled)
        );
    }
}
