//! Validation and promotion of worker results.
//!
//! Port of the C++ `media::ParserResultSession`. It owns the protocol
//! validator handed over by the handshake and validates every worker result
//! against the typed schemas and the cumulative quotas, but it creates no
//! transport, reads no source, opens no destination, stages no file and
//! publishes no data.
//!
//! The C++ "the catalog view aliases session storage and is invalidated by the
//! next enumeration" contract is expressed as a borrow: [`CatalogView`]
//! borrows the session, so a stale view cannot outlive the promotion that
//! replaced it.

use std::num::NonZeroU64;

use ohl_parser_protocol::messages::{
    MAXIMUM_ENTRY_BATCH_ENTRIES, decode_cancel_ack_payload, decode_cancel_payload,
    decode_complete_payload, decode_data_chunk_payload, decode_entry_batch_payload,
    decode_enumerate_payload, decode_read_reply_payload, decode_read_request_payload,
    decode_shutdown_payload, decode_stream_entry_payload,
};
use ohl_parser_protocol::{
    Direction, EntryBatchEntry, EntryBatchPolicy, FrameView, OperationPhase, ProtocolError,
    ProtocolStatus, ReadRequest, SessionState, SessionValidator, SourceReadPolicy,
};
use thiserror::Error;

use crate::catalog::{
    Catalog, CatalogGeneration, CatalogView, EntryMetadata, ImportLimits, LayoutError,
    PlannedEntry, SourceToken, WorkerEpoch, plan_catalog,
};

/// A synchronous destination for streamed entry bytes.
///
/// A sink must accept the entire chunk before reporting success and must
/// handle OS-level partial writes internally. Rejecting a chunk may leave
/// partial staging side effects, so the caller must discard staging after an
/// unsuccessful stream.
pub trait ByteSink {
    /// Accepts one complete chunk.
    ///
    /// # Errors
    /// [`SinkRejected`] when the destination refuses or cannot accept it.
    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkRejected>;
}

/// The sink refused a chunk. Carries no destination detail by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[error("payload sink rejected a chunk")]
pub struct SinkRejected;

/// Every way result validation can fail. All variants are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum ResultSessionError {
    /// The handed-over validator was not an idle, unfailed one.
    #[error("invalid result session configuration")]
    InvalidConfiguration,
    /// The message is not meaningful in this phase.
    #[error("invalid result session state")]
    InvalidState,
    /// A typed schema or the ordering rules rejected the frame.
    #[error("result protocol failure")]
    Protocol(#[source] ProtocolError),
    /// The enumeration failed layout validation.
    #[error("result layout failure")]
    Layout(#[source] LayoutError),
    /// The planned catalog disagreed with the accepted quotas.
    #[error("result validation failure")]
    ResultValidation,
    /// A stream was requested for a token or generation the catalog does not
    /// hold.
    #[error("unknown source token")]
    UnknownSourceToken,
    /// The destination sink rejected a chunk.
    #[error("downstream failure")]
    DownstreamFailure,
    /// The stream completed before its declared size was delivered.
    #[error("incomplete stream")]
    IncompleteStream,
    /// The enumeration counter is exhausted.
    #[error("generation exhausted")]
    GenerationExhausted,
    /// The pinned source changed underneath the session.
    #[error("source invalidated")]
    SourceInvalidated,
    /// Reading the pinned source failed.
    #[error("source read failure")]
    SourceReadFailure,
    /// The worker was retired out of band.
    #[error("worker failure")]
    WorkerFailure,
}

/// What the parent may do with a validated `read_request`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRequestOutcome {
    /// The request must be serviced. Only this variant carries the decoded
    /// message; an ignored request deliberately exposes no offset.
    Serviceable(ReadRequest),
    /// A cancel is pending, so the request is consumed and ignored.
    IgnoredAfterCancel,
}

#[derive(Debug)]
struct Candidate {
    generation: CatalogGeneration,
    promotable: bool,
    remaining_entries: u32,
    remaining_path_bytes: u64,
    remaining_total_bytes: u64,
    previous_source_token: Option<u64>,
    entries: Vec<EntryMetadata>,
}

#[derive(Debug)]
struct Stream {
    remaining_bytes: u64,
}

/// Owns and validates worker result metadata for one session.
#[derive(Debug)]
pub struct ResultSession {
    protocol: SessionValidator,
    limits: ImportLimits,
    epoch: WorkerEpoch,
    failure: Option<ResultSessionError>,
    enumeration_counter: u64,
    candidate: Option<Candidate>,
    catalog: Option<(CatalogGeneration, Catalog)>,
    stream: Option<Stream>,
}

impl ResultSession {
    /// Takes sole ownership of an idle, already-charged protocol validator.
    ///
    /// # Errors
    /// [`ResultSessionError::InvalidConfiguration`] unless the validator is
    /// unfailed and in [`SessionState::Idle`], i.e. exactly what a complete
    /// typed handshake produces.
    pub fn new(
        protocol: SessionValidator,
        epoch: WorkerEpoch,
        limits: ImportLimits,
    ) -> Result<Self, ResultSessionError> {
        if protocol.error().is_some() || protocol.state() != SessionState::Idle {
            return Err(ResultSessionError::InvalidConfiguration);
        }
        Ok(Self {
            protocol,
            limits,
            epoch,
            failure: None,
            enumeration_counter: 0,
            candidate: None,
            catalog: None,
            stream: None,
        })
    }

    /// Whether the session is terminally retired.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.failure.is_some()
    }

    /// The retained first failure, if any.
    #[must_use]
    pub const fn failure(&self) -> Option<ResultSessionError> {
        self.failure
    }

    /// The protocol state of the owned validator.
    #[must_use]
    pub const fn protocol_state(&self) -> SessionState {
        self.protocol.state()
    }

    /// Bytes still owed by the active stream.
    #[must_use]
    pub fn remaining_stream_bytes(&self) -> u64 {
        self.stream
            .as_ref()
            .map_or(0, |stream| stream.remaining_bytes)
    }

    /// The promoted catalog, if one is live.
    #[must_use]
    pub fn catalog(&self) -> Option<CatalogView<'_>> {
        if self.is_terminal() {
            return None;
        }
        self.catalog
            .as_ref()
            .map(|(generation, catalog)| CatalogView::new(*generation, catalog))
    }

    fn fail<T>(&mut self, error: ResultSessionError) -> Result<T, ResultSessionError> {
        Err(self.retire(error))
    }

    fn retire(&mut self, error: ResultSessionError) -> ResultSessionError {
        if self.failure.is_none() {
            self.failure = Some(error);
            self.retire_all();
        }
        self.failure.unwrap_or(error)
    }

    fn guard(&self) -> Result<(), ResultSessionError> {
        self.failure.map_or(Ok(()), Err)
    }

    fn observe(
        &mut self,
        direction: Direction,
        frame: &FrameView<'_>,
    ) -> Result<(), ResultSessionError> {
        match self.protocol.observe(direction, frame.header()) {
            Ok(()) => Ok(()),
            Err(error) => self.fail(ResultSessionError::Protocol(error)),
        }
    }

    fn retire_all(&mut self) {
        self.catalog = None;
        self.candidate = None;
        self.stream = None;
    }

    /// Begins a candidate enumeration for an outgoing `enumerate` frame.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`],
    /// [`ResultSessionError::GenerationExhausted`], or the retained failure.
    pub fn begin_enumeration(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_enumerate_payload(frame) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        let Some(enumeration) = self
            .enumeration_counter
            .checked_add(1)
            .and_then(NonZeroU64::new)
        else {
            return self.fail(ResultSessionError::GenerationExhausted);
        };
        self.observe(Direction::ParentToWorker, frame)?;

        self.retire_all();
        self.enumeration_counter = enumeration.get();
        self.candidate = Some(Candidate {
            generation: CatalogGeneration::new(self.epoch, enumeration),
            promotable: true,
            remaining_entries: self.limits.maximum_entries(),
            remaining_path_bytes: self.limits.maximum_path_bytes(),
            remaining_total_bytes: self.limits.maximum_total_bytes(),
            previous_source_token: None,
            entries: Vec::new(),
        });
        Ok(())
    }

    /// Validates one `entry_batch` against the remaining quotas.
    ///
    /// # Errors
    /// [`ResultSessionError::InvalidState`] with no candidate enumeration,
    /// [`ResultSessionError::Protocol`], or the retained failure.
    pub fn accept_entry_batch(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        let Some(candidate) = self.candidate.as_ref() else {
            return self.fail(ResultSessionError::InvalidState);
        };
        let policy = EntryBatchPolicy::new(
            candidate.remaining_entries,
            candidate.remaining_path_bytes,
            self.limits.maximum_entry_bytes(),
            candidate.remaining_total_bytes,
            candidate.previous_source_token,
        );
        let policy = match policy {
            Ok(policy) => policy,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };

        let mut storage = [EntryBatchEntry::default(); MAXIMUM_ENTRY_BATCH_ENTRIES as usize];
        let decoded = match decode_entry_batch_payload(frame, &policy, &mut storage) {
            Ok(decoded) => decoded,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };

        let mut batch_path_bytes: u64 = 0;
        let mut batch_total_bytes: u64 = 0;
        let mut prepared = Vec::with_capacity(decoded.entries.len());
        let mut last_token = 0;
        for entry in decoded.entries {
            batch_path_bytes += entry.archive_path.len() as u64;
            batch_total_bytes += entry.size_bytes;
            last_token = entry.source_token;
            prepared.push(EntryMetadata {
                source_token: SourceToken(entry.source_token),
                archive_path: entry.archive_path.as_str().to_owned(),
                size_bytes: entry.size_bytes,
            });
        }
        let accepted_entries = u32::try_from(prepared.len()).unwrap_or(u32::MAX);

        self.observe(Direction::WorkerToParent, frame)?;

        let Some(candidate) = self.candidate.as_mut() else {
            return self.fail(ResultSessionError::InvalidState);
        };
        if candidate.promotable {
            candidate.entries.append(&mut prepared);
        }
        candidate.remaining_entries -= accepted_entries;
        candidate.remaining_path_bytes -= batch_path_bytes;
        candidate.remaining_total_bytes -= batch_total_bytes;
        candidate.previous_source_token = Some(last_token);
        Ok(())
    }

    /// Promotes the candidate enumeration into the catalog.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`], [`ResultSessionError::InvalidState`],
    /// [`ResultSessionError::Layout`],
    /// [`ResultSessionError::ResultValidation`], or the retained failure.
    pub fn complete_enumeration(
        &mut self,
        frame: &FrameView<'_>,
    ) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_complete_payload(frame, OperationPhase::Enumerate) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        let Some(candidate) = self.candidate.as_mut() else {
            return self.fail(ResultSessionError::InvalidState);
        };
        let generation = candidate.generation;
        let promotable = candidate.promotable;
        let remaining_total_bytes = candidate.remaining_total_bytes;
        // Planning consumes the candidate entries: every outcome below either
        // promotes them or retires the candidate.
        let entries = std::mem::take(&mut candidate.entries);

        let promotion = if promotable {
            let accepted_total = self.limits.maximum_total_bytes() - remaining_total_bytes;
            let planned = match plan_catalog(&entries, self.limits) {
                Ok(planned) => planned,
                Err(error) => return self.fail(ResultSessionError::Layout(error)),
            };
            if planned.entries().len() != entries.len() || planned.total_bytes() != accepted_total {
                return self.fail(ResultSessionError::ResultValidation);
            }
            Some((generation, planned))
        } else {
            None
        };

        self.observe(Direction::WorkerToParent, frame)?;

        if let Some(promotion) = promotion {
            self.catalog = Some(promotion);
        }
        self.candidate = None;
        Ok(())
    }

    /// Binds an outgoing `stream_entry` frame to a promoted catalog entry.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`],
    /// [`ResultSessionError::UnknownSourceToken`] for a stale generation or an
    /// unknown token, or the retained failure.
    pub fn begin_stream_entry(
        &mut self,
        frame: &FrameView<'_>,
        expected_generation: CatalogGeneration,
    ) -> Result<(), ResultSessionError> {
        self.guard()?;
        let decoded = match decode_stream_entry_payload(frame) {
            Ok(decoded) => decoded,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };
        let size_bytes = match self.catalog.as_ref() {
            Some((generation, catalog)) if *generation == expected_generation => catalog
                .find(SourceToken(decoded.source_token))
                .map(PlannedEntry::size_bytes),
            _ => None,
        };
        let Some(size_bytes) = size_bytes else {
            return self.fail(ResultSessionError::UnknownSourceToken);
        };

        self.observe(Direction::ParentToWorker, frame)?;
        self.stream = Some(Stream {
            remaining_bytes: size_bytes,
        });
        Ok(())
    }

    /// Validates one `data_chunk` and hands it to `destination`.
    ///
    /// # Errors
    /// [`ResultSessionError::InvalidState`] with no active stream,
    /// [`ResultSessionError::Protocol`],
    /// [`ResultSessionError::DownstreamFailure`], or the retained failure.
    pub fn accept_data_chunk(
        &mut self,
        frame: &FrameView<'_>,
        destination: &mut dyn ByteSink,
    ) -> Result<(), ResultSessionError> {
        self.guard()?;
        let Some(remaining) = self.stream.as_ref().map(|stream| stream.remaining_bytes) else {
            return self.fail(ResultSessionError::InvalidState);
        };
        let decoded = match decode_data_chunk_payload(frame, remaining) {
            Ok(decoded) => decoded,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };
        let accepted = decoded.data.len() as u64;

        self.observe(Direction::WorkerToParent, frame)?;
        if destination.write(decoded.data).is_err() {
            return self.fail(ResultSessionError::DownstreamFailure);
        }
        if let Some(stream) = self.stream.as_mut() {
            stream.remaining_bytes -= accepted;
        }
        Ok(())
    }

    /// Completes the active stream.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`], [`ResultSessionError::InvalidState`],
    /// [`ResultSessionError::IncompleteStream`], or the retained failure.
    pub fn complete_stream(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_complete_payload(frame, OperationPhase::Stream) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        let Some(remaining) = self.stream.as_ref().map(|stream| stream.remaining_bytes) else {
            return self.fail(ResultSessionError::InvalidState);
        };
        if remaining != 0 {
            return self.fail(ResultSessionError::IncompleteStream);
        }
        self.observe(Direction::WorkerToParent, frame)?;
        self.stream = None;
        Ok(())
    }

    /// Validates and orders one `read_request`. This grants the decoded offset
    /// no source authority whatsoever.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`] or the retained failure.
    pub fn accept_read_request(
        &mut self,
        frame: &FrameView<'_>,
        policy: &SourceReadPolicy,
        expected_sequence: u32,
    ) -> Result<ReadRequestOutcome, ResultSessionError> {
        self.guard()?;
        let cancelling = self.protocol.state() == SessionState::Cancelling;
        let decoded = match decode_read_request_payload(frame, policy, expected_sequence) {
            Ok(decoded) => decoded,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };
        self.observe(Direction::WorkerToParent, frame)?;
        Ok(if cancelling {
            ReadRequestOutcome::IgnoredAfterCancel
        } else {
            ReadRequestOutcome::Serviceable(decoded)
        })
    }

    /// Validates and orders the parent's own `read_reply`.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`],
    /// [`ResultSessionError::SourceInvalidated`],
    /// [`ResultSessionError::SourceReadFailure`], or the retained failure.
    pub fn accept_read_reply(
        &mut self,
        frame: &FrameView<'_>,
        expected_sequence: u32,
        requested_length: u32,
    ) -> Result<(), ResultSessionError> {
        self.guard()?;
        let decoded = match decode_read_reply_payload(frame, expected_sequence, requested_length) {
            Ok(decoded) => decoded,
            Err(error) => return self.fail(ResultSessionError::Protocol(error)),
        };
        self.observe(Direction::ParentToWorker, frame)?;
        match decoded.status {
            ProtocolStatus::SourceChanged => self.fail(ResultSessionError::SourceInvalidated),
            ProtocolStatus::SourceReadFailed => self.fail(ResultSessionError::SourceReadFailure),
            _ => Ok(()),
        }
    }

    /// Observes the parent's own `cancel`.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`] or the retained failure.
    pub fn accept_cancel(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_cancel_payload(frame) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        self.observe(Direction::ParentToWorker, frame)?;
        // The catalog is retired immediately; candidate quotas and the stream
        // remainder are kept only to validate bounded same-request result
        // frames already crossing in the duplex transport.
        self.catalog = None;
        if let Some(candidate) = self.candidate.as_mut() {
            candidate.promotable = false;
            candidate.entries = Vec::new();
        }
        Ok(())
    }

    /// Observes the worker's `cancel_ack`.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`] or the retained failure.
    pub fn accept_cancel_ack(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_cancel_ack_payload(frame) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        self.observe(Direction::WorkerToParent, frame)?;
        self.retire_all();
        Ok(())
    }

    /// Observes the parent's own `shutdown`.
    ///
    /// # Errors
    /// [`ResultSessionError::Protocol`] or the retained failure.
    pub fn accept_shutdown(&mut self, frame: &FrameView<'_>) -> Result<(), ResultSessionError> {
        self.guard()?;
        if let Err(error) = decode_shutdown_payload(frame) {
            return self.fail(ResultSessionError::Protocol(error));
        }
        self.observe(Direction::ParentToWorker, frame)?;
        self.retire_all();
        Ok(())
    }

    /// Trusted out-of-band notification that the pinned source changed.
    pub fn invalidate_source(&mut self) {
        let _ = self.retire(ResultSessionError::SourceInvalidated);
    }

    /// Trusted out-of-band notification that the worker died or misbehaved.
    pub fn worker_failed(&mut self) {
        let _ = self.retire(ResultSessionError::WorkerFailure);
    }
}
