//! Walking the chain of DEFLATE streams that fills the overlay.
//!
//! Documented behaviour: "after each DEFLATEd data entry there is a CRC32 for
//! the inflated data. The DEFLATEd data + CRC32 continues until EOF."
//!
//! The walk is bounded on every axis (stream count, compressed bytes per
//! stream, inflated bytes per stream, total inflated bytes) and is cancelled
//! between chunks. It never trusts a declared length: each stream's extent is
//! whatever the decoder consumed.
//!
//! ## Resynchronisation
//!
//! Real packages are not always byte-exact: a stream can be followed by one
//! or two padding bytes before the next one begins. When the bytes at the
//! cursor do not inflate, or inflate but fail their checksum, the walk retries
//! at up to `Limits::max_resync_skip` (three by default) later offsets and
//! reports a [`ChainEvent::Resynced`] when a later offset succeeds. If no
//! offset succeeds, a stream that inflated cleanly is still yielded with its
//! checksum status, and one that did not inflate at all ends the walk with an
//! error. Nothing is ever silently skipped.

use ohl_core::CheckedArithmetic as _;

use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::source::{Cancellation, Discard, ImageSource};
use crate::stream::{ChecksumStatus, StreamMetrics, inflate_stream};

/// Fewest overlay bytes that could still hold another stream.
const MIN_STREAM_BYTES: u64 = 3;

/// One walked stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRecord {
    /// Zero-based position in the chain.
    pub index: u32,
    /// Measurements of the stream.
    pub metrics: StreamMetrics,
    /// Bytes skipped before this stream to resynchronise, zero normally.
    pub resync_skip: u8,
}

impl StreamRecord {
    /// Offset of the first compressed byte.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.metrics.compressed_offset
    }

    /// Inflated size in bytes.
    #[must_use]
    pub const fn inflated_len(&self) -> u64 {
        self.metrics.inflated_len
    }

    /// Whether the trailing checksum matched.
    #[must_use]
    pub const fn checksum(&self) -> ChecksumStatus {
        self.metrics.checksum
    }
}

/// One step of the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainEvent {
    /// A stream was decoded.
    Stream(StreamRecord),
    /// The walk skipped `skipped` bytes to find the next stream.
    Resynced {
        /// Index the resynchronised stream will be given.
        index: u32,
        /// Number of bytes skipped, between one and `max_resync_skip`.
        skipped: u8,
    },
}

/// A bounded iterator over the overlay's stream chain.
#[derive(Debug)]
pub struct Chain {
    limits: Limits,
    cursor: u64,
    end: u64,
    index: u32,
    total_inflated: u64,
    resyncs: u32,
    finished: bool,
    pending: Option<StreamRecord>,
}

impl Chain {
    /// A walk starting at `first_stream_offset` and ending at `end`, which is
    /// normally the image length.
    #[must_use]
    pub const fn new(first_stream_offset: u64, end: u64, limits: Limits) -> Self {
        Self {
            limits,
            cursor: first_stream_offset,
            end,
            index: 0,
            total_inflated: 0,
            resyncs: 0,
            finished: false,
            pending: None,
        }
    }

    /// Number of streams yielded so far.
    #[must_use]
    pub const fn stream_count(&self) -> u32 {
        self.index
    }

    /// Total inflated bytes across the walk so far.
    #[must_use]
    pub const fn total_inflated_bytes(&self) -> u64 {
        self.total_inflated
    }

    /// Number of resynchronisations so far.
    #[must_use]
    pub const fn resync_count(&self) -> u32 {
        self.resyncs
    }

    /// Offset the next attempt will start at.
    #[must_use]
    pub const fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Decodes the next event, or `None` at the end of the overlay.
    pub fn next_event<S: ImageSource, C: Cancellation>(
        &mut self,
        source: &mut S,
        cancel: &C,
    ) -> Option<Result<ChainEvent, Error>> {
        if let Some(record) = self.pending.take() {
            return Some(Ok(ChainEvent::Stream(record)));
        }
        if self.finished {
            return None;
        }
        match self.step(source, cancel) {
            Ok(None) => {
                self.finished = true;
                None
            }
            Ok(Some(event)) => Some(Ok(event)),
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }

    fn attempt<S: ImageSource, C: Cancellation>(
        &self,
        source: &mut S,
        cancel: &C,
        offset: u64,
    ) -> Result<StreamMetrics, Error> {
        inflate_stream(source, offset, self.limits, &mut Discard, cancel)
    }

    fn accept(&mut self, metrics: StreamMetrics, skip: u8) -> Result<ChainEvent, Error> {
        self.total_inflated = self
            .total_inflated
            .checked_add_bounded(metrics.inflated_len)?;
        if self.total_inflated > self.limits.max_total_inflated_bytes {
            return Err(Error::LimitExceeded(Limit::TotalInflatedBytes));
        }
        let record = StreamRecord {
            index: self.index,
            metrics,
            resync_skip: skip,
        };
        self.index = self.index.checked_add_bounded(1)?;
        self.cursor = metrics.next_offset();
        if skip > 0 {
            self.resyncs = self.resyncs.checked_add_bounded(1)?;
            self.pending = Some(record);
            return Ok(ChainEvent::Resynced {
                index: record.index,
                skipped: skip,
            });
        }
        Ok(ChainEvent::Stream(record))
    }

    fn step<S: ImageSource, C: Cancellation>(
        &mut self,
        source: &mut S,
        cancel: &C,
    ) -> Result<Option<ChainEvent>, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.cursor >= self.end || self.end.saturating_sub(self.cursor) < MIN_STREAM_BYTES {
            return Ok(None);
        }
        if self.index >= self.limits.max_streams {
            return Err(Error::LimitExceeded(Limit::Streams));
        }

        let first = self.attempt(source, cancel, self.cursor);
        if let Ok(metrics) = first
            && metrics.checksum == ChecksumStatus::Match
        {
            return self.accept(metrics, 0).map(Some);
        }
        if let Err(error @ (Error::Cancelled | Error::LimitExceeded(_))) = first {
            return Err(error);
        }

        for skip in 1..=self.limits.max_resync_skip {
            let offset = self.cursor.checked_add_bounded(u64::from(skip))?;
            if offset >= self.end {
                break;
            }
            match self.attempt(source, cancel, offset) {
                Ok(metrics) if metrics.checksum == ChecksumStatus::Match => {
                    return self.accept(metrics, skip).map(Some);
                }
                Err(error @ (Error::Cancelled | Error::LimitExceeded(_))) => return Err(error),
                _ => {}
            }
        }

        match first {
            // It decoded, but its checksum did not match (or the image ended
            // before one could be read). Yield it, flagged, and continue.
            Ok(metrics) => self.accept(metrics, 0).map(Some),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Chain, ChainEvent};
    use crate::crc32::crc32;
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use crate::source::{NeverCancelled, SliceSource};
    use crate::stream::ChecksumStatus;
    use alloc::vec::Vec;

    fn push_stream(bytes: &mut Vec<u8>, plain: &[u8], good: bool) {
        bytes.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(plain, 6));
        let crc = if good { crc32(plain) } else { crc32(plain) ^ 1 };
        bytes.extend_from_slice(&crc.to_le_bytes());
    }

    fn walk(image: &[u8], limits: Limits) -> (Vec<ChainEvent>, Option<Error>) {
        let mut source = SliceSource::new(image);
        let mut chain = Chain::new(0, image.len() as u64, limits);
        let mut events = Vec::new();
        let mut failure = None;
        while let Some(event) = chain.next_event(&mut source, &NeverCancelled) {
            match event {
                Ok(event) => events.push(event),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        (events, failure)
    }

    #[test]
    fn walks_consecutive_streams_to_the_end() {
        let mut image = Vec::new();
        for value in 0..4u8 {
            push_stream(&mut image, &alloc::vec![value; 300], true);
        }
        let (events, failure) = walk(&image, Limits::DEFAULT);
        assert!(failure.is_none());
        assert_eq!(events.len(), 4);
        for (index, event) in events.iter().enumerate() {
            let ChainEvent::Stream(record) = event else {
                panic!("expected a stream event")
            };
            assert_eq!(record.index, u32::try_from(index).unwrap());
            assert_eq!(record.checksum(), ChecksumStatus::Match);
            assert_eq!(record.inflated_len(), 300);
            assert_eq!(record.resync_skip, 0);
        }
    }

    #[test]
    fn resynchronises_over_a_pad_byte() {
        let mut image = Vec::new();
        push_stream(&mut image, &alloc::vec![1u8; 200], true);
        image.push(0x00);
        push_stream(&mut image, &alloc::vec![2u8; 200], true);
        let (events, failure) = walk(&image, Limits::DEFAULT);
        assert!(failure.is_none());
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[1],
            ChainEvent::Resynced {
                index: 1,
                skipped: 1
            }
        ));
        let ChainEvent::Stream(record) = events[2] else {
            panic!("expected the resynchronised stream")
        };
        assert_eq!(record.resync_skip, 1);
        assert_eq!(record.checksum(), ChecksumStatus::Match);
    }

    #[test]
    fn reports_a_mismatch_and_keeps_walking() {
        let mut image = Vec::new();
        push_stream(&mut image, &alloc::vec![1u8; 200], false);
        push_stream(&mut image, &alloc::vec![2u8; 200], true);
        let (events, failure) = walk(&image, Limits::DEFAULT);
        assert!(failure.is_none());
        assert_eq!(events.len(), 2);
        let ChainEvent::Stream(first) = events[0] else {
            panic!("expected a stream event")
        };
        assert_eq!(first.checksum(), ChecksumStatus::Mismatch);
    }

    #[test]
    fn stops_on_undecodable_bytes() {
        let mut image = Vec::new();
        push_stream(&mut image, &alloc::vec![1u8; 200], true);
        image.extend_from_slice(&[0xffu8; 64]);
        let (events, failure) = walk(&image, Limits::DEFAULT);
        assert_eq!(events.len(), 1);
        assert_eq!(failure, Some(Error::DecompressionFailed));
    }

    #[test]
    fn enforces_the_stream_ceiling() {
        let mut image = Vec::new();
        for _ in 0..3 {
            push_stream(&mut image, &alloc::vec![9u8; 100], true);
        }
        let limits = Limits {
            max_streams: 2,
            ..Limits::DEFAULT
        };
        let (events, failure) = walk(&image, limits);
        assert_eq!(events.len(), 2);
        assert_eq!(failure, Some(Error::LimitExceeded(Limit::Streams)));
    }

    #[test]
    fn enforces_the_total_inflated_ceiling() {
        let mut image = Vec::new();
        for _ in 0..3 {
            push_stream(&mut image, &alloc::vec![9u8; 1000], true);
        }
        let limits = Limits {
            max_total_inflated_bytes: 1500,
            ..Limits::DEFAULT
        };
        let (_events, failure) = walk(&image, limits);
        assert_eq!(
            failure,
            Some(Error::LimitExceeded(Limit::TotalInflatedBytes))
        );
    }
}
