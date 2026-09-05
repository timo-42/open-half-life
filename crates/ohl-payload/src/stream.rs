//! Bounded chunk streaming for one planned entry.
//!
//! Streaming is where an archive reader and a destination file meet, and it is
//! deliberately the narrowest interface in the crate. A [`PayloadSource`] is
//! handed the pinned [`MediaSource`], the entry's opaque source token, and a
//! sink — and *not* the destination path. It cannot learn where its bytes
//! land, so a compromised or merely buggy reader has no way to influence the
//! destination.
//!
//! [`stream_payload_entry`] wraps the caller's destination in a bounded sink
//! that enforces the plan:
//!
//! - a chunk that would push the entry past its declared size is refused
//!   *before* the destination sees any of it ([`PayloadStreamError::Overflow`]);
//! - a source that returns success with too few bytes is
//!   [`PayloadStreamError::Underflow`];
//! - cancellation is polled before the first chunk, before each accepted
//!   chunk, and once after the source returns, and takes precedence over a
//!   source's own success ([`PayloadStreamError::Cancelled`]);
//! - the byte count reported on every path counts only chunks the destination
//!   accepted in full.
//!
//! A partial or refused stream may have left bytes in the destination. That is
//! the caller's problem to discard, and [`crate::stage`] does so by aborting
//! the whole staging transaction rather than trying to rewind one file.

use ohl_platform::MediaSource;

use crate::cancel::CancellationToken;
use crate::layout::PlannedPayloadEntry;

/// A destination for one entry's bytes.
///
/// An implementation must accept a chunk in full or refuse it: there is no
/// partial success, and handling an OS-level short write is the
/// implementation's job. Refusing may leave earlier bytes behind.
pub trait PayloadByteSink {
    /// Accepts one whole chunk, or refuses it by returning `false`.
    fn write_chunk(&mut self, bytes: &[u8]) -> bool;
}

impl<T: PayloadByteSink + ?Sized> PayloadByteSink for &mut T {
    fn write_chunk(&mut self, bytes: &[u8]) -> bool {
        (**self).write_chunk(bytes)
    }
}

/// A producer of one entry's bytes.
///
/// Implementations receive the exact pinned capability staging was given plus
/// the opaque token from layout planning. They must cooperate with
/// cancellation, stop and return `false` as soon as the sink refuses a chunk,
/// and must not retain the source or the sink after returning.
pub trait PayloadSource {
    /// Streams the entry named by `source_token` into `sink`.
    ///
    /// Returns `false` for any failure the source itself detects.
    fn stream(
        &mut self,
        media_source: &MediaSource,
        source_token: u64,
        cancellation: &CancellationToken,
        sink: &mut dyn PayloadByteSink,
    ) -> bool;
}

/// Why streaming one entry did not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PayloadStreamError {
    /// The source reported a failure.
    SourceFailure,
    /// The destination refused a chunk.
    DestinationFailure,
    /// A chunk would have exceeded the entry's declared size.
    Overflow,
    /// The source succeeded with fewer bytes than declared.
    Underflow,
    /// A stop was requested at one of the polling points.
    Cancelled,
}

impl PayloadStreamError {
    /// The fixed, payload-free message for this code.
    const fn message(self) -> &'static str {
        match self {
            Self::SourceFailure => "payload source failed to stream an entry",
            Self::DestinationFailure => "payload destination refused a chunk",
            Self::Overflow => "payload source offered more bytes than declared",
            Self::Underflow => "payload source offered fewer bytes than declared",
            Self::Cancelled => "payload streaming was cancelled",
        }
    }
}

impl core::fmt::Display for PayloadStreamError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl core::error::Error for PayloadStreamError {}

impl From<PayloadStreamError> for ohl_core::SanitizedError {
    fn from(error: PayloadStreamError) -> Self {
        match error {
            PayloadStreamError::Overflow | PayloadStreamError::Underflow => Self::InvalidInput,
            PayloadStreamError::SourceFailure
            | PayloadStreamError::DestinationFailure
            | PayloadStreamError::Cancelled => Self::Internal,
        }
    }
}

/// The outcome of streaming one entry.
///
/// `bytes_written` is meaningful on failure too: it is exactly the number of
/// bytes the destination accepted in full before the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadStreamOutcome {
    /// The failing rule, or `None` when the entry streamed exactly.
    pub error: Option<PayloadStreamError>,
    /// Bytes the destination accepted in full.
    pub bytes_written: u64,
}

impl PayloadStreamOutcome {
    /// Whether the entry streamed to its exact declared size.
    pub const fn complete(&self) -> bool {
        self.error.is_none()
    }
}

/// The sink handed to the source: it enforces the declared size and
/// cancellation, and forwards nothing that would break either.
struct BoundedSink<'a> {
    /// The entry's declared size.
    declared_size: u64,
    /// The token polled before each accepted chunk.
    cancellation: &'a CancellationToken,
    /// The caller's destination.
    destination: &'a mut dyn PayloadByteSink,
    /// Bytes the destination accepted in full.
    bytes_written: u64,
    /// The first refusal, which the caller reads back after streaming.
    error: Option<PayloadStreamError>,
}

impl PayloadByteSink for BoundedSink<'_> {
    fn write_chunk(&mut self, bytes: &[u8]) -> bool {
        if self.error.is_some() {
            return false;
        }
        if self.cancellation.stop_requested() {
            self.error = Some(PayloadStreamError::Cancelled);
            return false;
        }
        let remaining = self.declared_size - self.bytes_written;
        let offered = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if offered > remaining {
            // Refused before the destination is touched: an over-long chunk
            // must never reach a create-new file that is sized by the plan.
            self.error = Some(PayloadStreamError::Overflow);
            return false;
        }
        if !self.destination.write_chunk(bytes) {
            self.error = Some(PayloadStreamError::DestinationFailure);
            return false;
        }
        self.bytes_written += offered;
        true
    }
}

/// Streams one planned entry into `destination` without exposing the
/// destination to `source`.
///
/// Success requires an exact final byte count. See the [module
/// documentation](self) for the failure precedence, which is: a bound or
/// cancellation observed inside the stream, then a cancellation observed after
/// it, then the source's own failure, then an inexact byte count.
pub fn stream_payload_entry(
    entry: &PlannedPayloadEntry,
    media_source: &MediaSource,
    source: &mut dyn PayloadSource,
    cancellation: &CancellationToken,
    destination: &mut dyn PayloadByteSink,
) -> PayloadStreamOutcome {
    if cancellation.stop_requested() {
        // Neither the source nor the destination is touched, so a
        // pre-cancelled entry leaves nothing to discard.
        return PayloadStreamOutcome {
            error: Some(PayloadStreamError::Cancelled),
            bytes_written: 0,
        };
    }

    let mut bounded = BoundedSink {
        declared_size: entry.size_bytes,
        cancellation,
        destination,
        bytes_written: 0,
        error: None,
    };
    let source_succeeded =
        source.stream(media_source, entry.source_token, cancellation, &mut bounded);
    let bytes_written = bounded.bytes_written;
    let bounded_error = bounded.error;

    let error = if let Some(error) = bounded_error {
        Some(error)
    } else if cancellation.stop_requested() {
        Some(PayloadStreamError::Cancelled)
    } else if !source_succeeded {
        Some(PayloadStreamError::SourceFailure)
    } else if bytes_written == entry.size_bytes {
        None
    } else {
        Some(PayloadStreamError::Underflow)
    };

    PayloadStreamOutcome {
        error,
        bytes_written,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PayloadByteSink, PayloadSource, PayloadStreamError, PayloadStreamOutcome,
        stream_payload_entry,
    };
    use crate::cancel::{CancellationSource, CancellationToken};
    use crate::layout::PlannedPayloadEntry;
    use crate::path::PayloadPath;
    use crate::test_support::pinned_source;
    use ohl_platform::MediaSource;

    fn entry(token: u64, path: &str, size: u64) -> PlannedPayloadEntry {
        PlannedPayloadEntry {
            source_token: token,
            path: PayloadPath::parse(path).expect("valid path"),
            size_bytes: size,
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        fail_on_call: usize,
        calls: usize,
        bytes: Vec<u8>,
    }

    impl PayloadByteSink for RecordingSink {
        fn write_chunk(&mut self, bytes: &[u8]) -> bool {
            self.calls += 1;
            if self.fail_on_call != 0 && self.calls == self.fail_on_call {
                return false;
            }
            self.bytes.extend_from_slice(bytes);
            true
        }
    }

    struct SyntheticSource<'a> {
        expected_media_source: *const MediaSource,
        chunks: Vec<Vec<u8>>,
        succeed: bool,
        expected_token: CancellationToken,
        request_stop: Option<&'a CancellationSource>,
        request_after_chunk: usize,
        request_before_return: bool,
        calls: usize,
        observed_token: u64,
        contract_ok: bool,
    }

    impl<'a> SyntheticSource<'a> {
        fn new(media_source: &MediaSource, chunks: Vec<Vec<u8>>) -> Self {
            Self {
                expected_media_source: core::ptr::from_ref(media_source),
                chunks,
                succeed: true,
                expected_token: CancellationToken::default(),
                request_stop: None,
                request_after_chunk: 0,
                request_before_return: false,
                calls: 0,
                observed_token: 0,
                contract_ok: false,
            }
        }

        fn failing(mut self) -> Self {
            self.succeed = false;
            self
        }

        fn expecting(mut self, token: CancellationToken) -> Self {
            self.expected_token = token;
            self
        }

        fn requesting_after(mut self, source: &'a CancellationSource, chunk: usize) -> Self {
            self.request_stop = Some(source);
            self.request_after_chunk = chunk;
            self
        }

        fn requesting_before_return(mut self, source: &'a CancellationSource) -> Self {
            self.request_stop = Some(source);
            self.request_before_return = true;
            self
        }
    }

    impl PayloadSource for SyntheticSource<'_> {
        fn stream(
            &mut self,
            media_source: &MediaSource,
            source_token: u64,
            cancellation: &CancellationToken,
            sink: &mut dyn PayloadByteSink,
        ) -> bool {
            self.calls += 1;
            self.observed_token = source_token;
            self.contract_ok = core::ptr::eq(
                core::ptr::from_ref(media_source),
                self.expected_media_source,
            ) && *cancellation == self.expected_token;
            if !self.contract_ok {
                return false;
            }
            for (index, chunk) in self.chunks.clone().into_iter().enumerate() {
                if !sink.write_chunk(&chunk) {
                    return false;
                }
                if let Some(source) = self.request_stop
                    && self.request_after_chunk == index + 1
                {
                    source.request_stop();
                }
            }
            if let Some(source) = self.request_stop
                && self.request_before_return
            {
                source.request_stop();
            }
            self.succeed
        }
    }

    fn assert_outcome(
        outcome: &PayloadStreamOutcome,
        error: Option<PayloadStreamError>,
        bytes_written: u64,
    ) {
        assert_eq!(outcome.error, error);
        assert_eq!(outcome.bytes_written, bytes_written);
        assert_eq!(outcome.complete(), error.is_none());
    }

    #[test]
    fn an_exact_stream_forwards_the_token_and_hides_the_destination() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();
        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2, 3]]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(42, "unused/destination", 3),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, None, 3);
        assert!(source.contract_ok);
        assert_eq!(source.observed_token, 42);
        assert_eq!(destination.bytes, [1, 2, 3]);
    }

    #[test]
    fn chunked_empty_and_zero_byte_streams_complete_exactly() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();

        let mut source = SyntheticSource::new(media_source, vec![vec![1], vec![2, 3], vec![4]]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(7, "not/source-visible", 4),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, None, 4);
        assert_eq!(destination.bytes, [1, 2, 3, 4]);
        assert_eq!(destination.calls, 3);

        let mut source = SyntheticSource::new(media_source, Vec::new());
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(8, "empty", 0),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, None, 0);
        assert_eq!(destination.calls, 0);

        let mut source =
            SyntheticSource::new(media_source, vec![Vec::new(), vec![1, 2], Vec::new()]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(9, "empty-chunks", 2),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, None, 2);
        assert_eq!(destination.bytes, [1, 2]);
        assert_eq!(destination.calls, 3);
    }

    #[test]
    fn an_inexact_byte_count_is_refused_in_both_directions() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();

        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2]]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(10, "short", 3),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Underflow), 2);

        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2], vec![3, 4]]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(11, "long", 3),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Overflow), 2);
        assert_eq!(destination.bytes, [1, 2]);
        assert_eq!(destination.calls, 1);

        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2, 3]]);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(12, "first-chunk-overflow", 2),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Overflow), 0);
        assert_eq!(destination.calls, 0);
        assert!(destination.bytes.is_empty());
    }

    #[test]
    fn destination_and_source_failures_are_distinguished() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();

        let mut source = SyntheticSource::new(media_source, vec![vec![1], vec![2, 3]]);
        let mut destination = RecordingSink {
            fail_on_call: 2,
            ..RecordingSink::default()
        };
        let outcome = stream_payload_entry(
            &entry(13, "destination-failure", 3),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::DestinationFailure), 1);
        assert_eq!(destination.bytes, [1]);

        let mut source = SyntheticSource::new(media_source, vec![vec![1]]).failing();
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(14, "source-failure", 3),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::SourceFailure), 1);

        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2]]).failing();
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(15, "source-failure-after-exact", 2),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::SourceFailure), 2);
        assert_eq!(destination.bytes, [1, 2]);
    }

    #[test]
    fn the_maximum_source_token_is_forwarded_intact() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();
        let mut source = SyntheticSource::new(media_source, Vec::new());
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(u64::MAX, "maximum-token", 0),
            media_source,
            &mut source,
            &CancellationToken::default(),
            &mut destination,
        );
        assert!(outcome.complete());
        assert_eq!(source.observed_token, u64::MAX);
    }

    #[test]
    fn cancellation_wins_before_during_and_after_the_stream() {
        let fixture = pinned_source(b"payload-fixture");
        let media_source = fixture.media_source();

        let stop = CancellationSource::new();
        assert!(stop.request_stop());
        let mut source = SyntheticSource::new(media_source, vec![vec![1]]).expecting(stop.token());
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(21, "pre-cancel", 1),
            media_source,
            &mut source,
            &stop.token(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Cancelled), 0);
        assert_eq!(source.calls, 0);
        assert_eq!(destination.calls, 0);

        let stop = CancellationSource::new();
        let mut source = SyntheticSource::new(media_source, vec![vec![1], vec![2, 3, 4]])
            .expecting(stop.token())
            .requesting_after(&stop, 1);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(22, "cancel-during", 3),
            media_source,
            &mut source,
            &stop.token(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Cancelled), 1);
        assert_eq!(destination.bytes, [1]);
        assert_eq!(destination.calls, 1);

        let stop = CancellationSource::new();
        let mut source = SyntheticSource::new(media_source, vec![vec![1, 2]])
            .expecting(stop.token())
            .requesting_before_return(&stop);
        let mut destination = RecordingSink::default();
        let outcome = stream_payload_entry(
            &entry(23, "cancel-after", 2),
            media_source,
            &mut source,
            &stop.token(),
            &mut destination,
        );
        assert_outcome(&outcome, Some(PayloadStreamError::Cancelled), 2);
        assert_eq!(destination.bytes, [1, 2]);
    }

    #[test]
    fn every_message_is_a_fixed_literal() {
        for error in [
            PayloadStreamError::SourceFailure,
            PayloadStreamError::DestinationFailure,
            PayloadStreamError::Overflow,
            PayloadStreamError::Underflow,
            PayloadStreamError::Cancelled,
        ] {
            assert!(!error.to_string().is_empty());
            let _: ohl_core::SanitizedError = error.into();
        }
    }
}
