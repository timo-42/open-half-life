//! Bounded parent-serviced reads of the pinned source.
//!
//! Port of the C++ `media::ParserSourceReadBroker`. It brokers reads from the
//! exact pinned [`MediaSource`] carried by [`ValidatedMedia`]; it neither
//! creates a transport nor sends a frame.
//!
//! Rust removes two runtime checks the C++ needed: `scratch` and
//! `reply_storage` are two `&mut [u8]`, which cannot alias, so
//! `overlapping_buffers` is unrepresentable; and the reply ticket is a
//! move-only [`ReplyTicket`] carried inside [`PreparedReply`], which
//! [`SourceReadBroker::commit_reply_sent`] and
//! [`SourceReadBroker::abandon_reply`] consume — so double consumption is a
//! compile error rather than an `invalid_ticket` result.

use std::sync::Arc;

use ohl_media::ValidatedMedia;
use ohl_parser_protocol::messages::{
    MAXIMUM_READ_BYTES, READ_REPLY_PREFIX_BYTES, decode_read_request_payload,
    encode_read_reply_payload,
};
use ohl_parser_protocol::{
    FrameHeader, FrameView, MAXIMUM_CUMULATIVE_PAYLOAD_BYTES, MAXIMUM_PROTOCOL_MESSAGES,
    MessageType, ProtocolError, ProtocolStatus, ReadReply, SessionState, SourceReadPolicy,
};
use ohl_platform::{MediaSource, MediaSourceError};
use thiserror::Error;

use crate::io::sealed;
use crate::result_session::{ReadRequestOutcome, ResultSession, ResultSessionError};

/// Quotas applied to one session's parent-serviced reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceReadLimits {
    read_bytes: u32,
    requests: u64,
    reply_payload_bytes: u64,
}

impl Default for SourceReadLimits {
    fn default() -> Self {
        Self {
            read_bytes: MAXIMUM_READ_BYTES,
            requests: MAXIMUM_PROTOCOL_MESSAGES / 2,
            reply_payload_bytes: MAXIMUM_CUMULATIVE_PAYLOAD_BYTES,
        }
    }
}

impl SourceReadLimits {
    /// Validates one set of read quotas.
    ///
    /// `maximum_read_bytes` must be identical to the value advertised by the
    /// accepted typed hello for the session.
    ///
    /// # Errors
    /// [`SourceReadError::InvalidConfiguration`] for a zero or out-of-ceiling
    /// quota, or a reply quota that cannot hold one maximum-size reply.
    pub const fn new(
        maximum_read_bytes: u32,
        maximum_requests: u64,
        maximum_reply_payload_bytes: u64,
    ) -> Result<Self, SourceReadError> {
        if maximum_read_bytes == 0
            || maximum_read_bytes > MAXIMUM_READ_BYTES
            || maximum_requests == 0
            || maximum_requests > MAXIMUM_PROTOCOL_MESSAGES / 2
            || maximum_reply_payload_bytes
                < READ_REPLY_PREFIX_BYTES as u64 + maximum_read_bytes as u64
            || maximum_reply_payload_bytes > MAXIMUM_CUMULATIVE_PAYLOAD_BYTES
        {
            return Err(SourceReadError::InvalidConfiguration);
        }
        Ok(Self {
            read_bytes: maximum_read_bytes,
            requests: maximum_requests,
            reply_payload_bytes: maximum_reply_payload_bytes,
        })
    }

    /// The largest single read the worker may request.
    #[must_use]
    pub const fn maximum_read_bytes(self) -> u32 {
        self.read_bytes
    }

    /// The number of reads this session may be charged for.
    #[must_use]
    pub const fn maximum_requests(self) -> u64 {
        self.requests
    }

    /// The cumulative reply-payload quota.
    #[must_use]
    pub const fn maximum_reply_payload_bytes(self) -> u64 {
        self.reply_payload_bytes
    }

    /// The reply storage one maximum-size reply needs.
    #[must_use]
    pub const fn reply_storage_bytes(self) -> usize {
        READ_REPLY_PREFIX_BYTES + self.read_bytes as usize
    }
}

/// Every way brokering a read can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum SourceReadError {
    /// The broker cannot be built from this media, session or limits.
    #[error("invalid source-read configuration")]
    InvalidConfiguration,
    /// No reply is pending.
    #[error("invalid source-read state")]
    InvalidState,
    /// A reply is already prepared and unsent. Not terminal.
    #[error("source-read reply pending")]
    ReplyPending,
    /// The caller's scratch or reply storage is too small. Not terminal.
    #[error("source-read output too small")]
    OutputTooSmall,
    /// The session's read-count quota is exhausted.
    #[error("source-read request budget exceeded")]
    RequestBudgetExceeded,
    /// The session's reply-byte quota is exhausted.
    #[error("source-read byte budget exceeded")]
    ByteBudgetExceeded,
    /// The 32-bit read sequence for this request is exhausted.
    #[error("source-read sequence exhausted")]
    SequenceExhausted,
    /// The ticket counter is exhausted.
    #[error("source-read ticket exhausted")]
    TicketExhausted,
    /// A typed schema or the ordering rules rejected a frame.
    #[error("source-read protocol failure")]
    Protocol(#[source] ProtocolError),
    /// An invariant of this broker was violated.
    #[error("source-read internal failure")]
    Internal,
    /// The presented ticket does not name the pending reply.
    #[error("invalid source-read ticket")]
    InvalidTicket,
    /// The transport did not deliver the exact prepared reply, so session
    /// ordering is unknowable.
    #[error("source-read transport abandoned")]
    TransportAbandoned,
    /// The pinned source changed underneath the session.
    #[error("source changed")]
    SourceChanged,
    /// Reading the pinned source failed.
    #[error("source read failure")]
    SourceReadFailure,
}

impl SourceReadError {
    /// Whether observing this error retires the broker and its session.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::ReplyPending | Self::OutputTooSmall)
    }
}

/// The deterministic operation seam for source access.
///
/// Implementations receive the exact pinned source retained from
/// [`ValidatedMedia`]: they cannot replace it or select a path. The trait is
/// sealed, so only this crate provides implementations — [`NativeSourceOps`]
/// in production and
/// [`ScriptedSourceOps`](crate::testing::ScriptedSourceOps) for deterministic
/// fault handling in tests.
pub trait SourceOps: sealed::Sealed {
    /// Re-verifies the pinned object.
    ///
    /// # Errors
    /// Whatever [`MediaSource::verify_unchanged`] reports.
    fn verify_unchanged(&self, source: &MediaSource) -> Result<(), MediaSourceError>;

    /// Reads `destination.len()` bytes at `offset`.
    ///
    /// # Errors
    /// Whatever [`MediaSource::read_exact_at`] reports.
    fn read_exact_at(
        &self,
        source: &MediaSource,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<(), MediaSourceError>;
}

/// The native [`MediaSource`] methods.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeSourceOps;

impl sealed::Sealed for NativeSourceOps {}

impl SourceOps for NativeSourceOps {
    fn verify_unchanged(&self, source: &MediaSource) -> Result<(), MediaSourceError> {
        source.verify_unchanged()
    }

    fn read_exact_at(
        &self,
        source: &MediaSource,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<(), MediaSourceError> {
        source.read_exact_at(offset, destination)
    }
}

/// A move-only receipt for one prepared reply.
#[derive(Debug, PartialEq, Eq)]
pub struct ReplyTicket(u64);

/// A reply the transport has not sent yet.
///
/// The payload aliases the caller's reply storage. Successful replies contain
/// source bytes: they must stay private rather than be logged.
///
/// The receipt is single-use by construction, so the C++ `invalid_ticket`
/// path is a compile error here:
///
/// ```compile_fail
/// # use ohl_import::{PreparedReply, ResultSession, SourceReadBroker};
/// fn double_commit(
///     broker: &mut SourceReadBroker,
///     session: &mut ResultSession,
///     prepared: PreparedReply<'_>,
/// ) {
///     let _ = broker.commit_reply_sent(prepared, session);
///     let _ = broker.commit_reply_sent(prepared, session);
/// }
/// ```
#[derive(Debug)]
pub struct PreparedReply<'reply> {
    ticket: ReplyTicket,
    status: ProtocolStatus,
    header: FrameHeader,
    payload: &'reply [u8],
}

impl PreparedReply<'_> {
    /// The frame header the transport must send unchanged.
    #[must_use]
    pub const fn header(&self) -> &FrameHeader {
        &self.header
    }

    /// The frame payload the transport must send unchanged.
    #[must_use]
    pub const fn payload(&self) -> &[u8] {
        self.payload
    }

    /// The status the parent answered with.
    #[must_use]
    pub const fn status(&self) -> ProtocolStatus {
        self.status
    }
}

/// What [`SourceReadBroker::prepare`] decided about a `read_request`.
#[derive(Debug)]
pub enum PrepareOutcome<'reply> {
    /// The reply is prepared and must now be sent verbatim.
    ReplyReady(PreparedReply<'reply>),
    /// A cancel is pending: the request was consumed and no reply is owed.
    IgnoredAfterCancel,
}

#[derive(Debug)]
struct Pending {
    ticket: u64,
    request_id: u64,
    sequence: u32,
    requested_length: u32,
    status: ProtocolStatus,
}

/// Serves `read_request` messages against one pinned source.
///
/// Calls must be serialized with every direct call to the associated
/// [`ResultSession`], which is why the session is passed in explicitly rather
/// than aliased: the borrow checker enforces the C++ comment.
#[derive(Debug)]
pub struct SourceReadBroker<O: SourceOps = NativeSourceOps> {
    source: Arc<MediaSource>,
    policy: SourceReadPolicy,
    limits: SourceReadLimits,
    ops: O,
    failure: Option<SourceReadError>,
    committed_request_id: Option<u64>,
    next_sequence: u32,
    sequence_exhausted: bool,
    requests_charged: u64,
    reply_payload_bytes_charged: u64,
    ticket_counter: u64,
    pending: Option<Pending>,
}

impl SourceReadBroker<NativeSourceOps> {
    /// Builds a broker over the native source methods.
    ///
    /// # Errors
    /// See [`SourceReadBroker::with_ops`].
    pub fn new(
        media: &ValidatedMedia,
        session: &mut ResultSession,
        limits: SourceReadLimits,
    ) -> Result<Self, SourceReadError> {
        Self::with_ops(media, session, limits, NativeSourceOps)
    }
}

impl<O: SourceOps> SourceReadBroker<O> {
    /// Builds a broker over an explicit operation seam.
    ///
    /// # Errors
    /// [`SourceReadError::InvalidConfiguration`] when the pinned source
    /// disagrees with the proof, when the read policy is out of range, or when
    /// `session` is not a live, idle result session. The session is retired in
    /// that case, exactly as the C++ constructor did.
    pub fn with_ops(
        media: &ValidatedMedia,
        session: &mut ResultSession,
        limits: SourceReadLimits,
        ops: O,
    ) -> Result<Self, SourceReadError> {
        let source = Arc::clone(media.source());
        let policy = SourceReadPolicy::new(media.size_bytes(), limits.maximum_read_bytes());
        let usable = source.size() == media.size_bytes()
            && !session.is_terminal()
            && session.protocol_state() == SessionState::Idle;
        match policy {
            Ok(policy) if usable => Ok(Self {
                source,
                policy,
                limits,
                ops,
                failure: None,
                committed_request_id: None,
                next_sequence: 1,
                sequence_exhausted: false,
                requests_charged: 0,
                reply_payload_bytes_charged: 0,
                ticket_counter: 0,
                pending: None,
            }),
            _ => {
                session.worker_failed();
                Err(SourceReadError::InvalidConfiguration)
            }
        }
    }

    /// The policy the worker's requests are validated against.
    #[must_use]
    pub const fn policy(&self) -> &SourceReadPolicy {
        &self.policy
    }

    /// Whether the broker is terminally retired.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.failure.is_some()
    }

    /// The retained first failure, if any.
    #[must_use]
    pub const fn failure(&self) -> Option<SourceReadError> {
        self.failure
    }

    /// Whether a prepared reply is still unsent.
    #[must_use]
    pub const fn reply_is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Reads charged against the request quota.
    #[must_use]
    pub const fn requests_charged(&self) -> u64 {
        self.requests_charged
    }

    /// Reply bytes charged against the byte quota.
    #[must_use]
    pub const fn reply_payload_bytes_charged(&self) -> u64 {
        self.reply_payload_bytes_charged
    }

    /// Retires the broker and, unless it already ended, its session.
    ///
    /// The C++ destructor did this implicitly; here it is explicit, because a
    /// broker that borrows nothing cannot reach the session from `Drop`.
    pub fn retire(self, session: &mut ResultSession) {
        let state = session.protocol_state();
        if !session.is_terminal()
            && state != SessionState::Closed
            && state != SessionState::Cancelled
        {
            session.worker_failed();
        }
    }

    fn fail(&mut self, error: SourceReadError, session: &mut ResultSession) -> SourceReadError {
        if self.failure.is_none() {
            self.failure = Some(error);
            self.pending = None;
            if !session.is_terminal() {
                session.worker_failed();
            }
        }
        self.failure.unwrap_or(error)
    }

    fn map_session_error(error: ResultSessionError) -> SourceReadError {
        match error {
            ResultSessionError::Protocol(protocol) => SourceReadError::Protocol(protocol),
            _ => SourceReadError::Internal,
        }
    }

    /// Validates one `read_request` and, unless a cancel is pending, services
    /// it from the pinned source into `reply_storage`.
    ///
    /// # Errors
    /// [`SourceReadError::ReplyPending`] or
    /// [`SourceReadError::OutputTooSmall`] (both non-terminal), any quota
    /// failure, [`SourceReadError::Protocol`], or the retained failure.
    pub fn prepare<'reply>(
        &mut self,
        session: &mut ResultSession,
        request: &FrameView<'_>,
        scratch: &mut [u8],
        reply_storage: &'reply mut [u8],
    ) -> Result<PrepareOutcome<'reply>, SourceReadError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.pending.is_some() {
            return Err(SourceReadError::ReplyPending);
        }

        let same_request = self.committed_request_id == Some(request.header().request_id);
        if same_request && self.sequence_exhausted {
            return Err(self.fail(SourceReadError::SequenceExhausted, session));
        }
        let expected_sequence = if same_request { self.next_sequence } else { 1 };

        let cancelling = session.protocol_state() == SessionState::Cancelling;
        let Ok(decoded) = decode_read_request_payload(request, &self.policy, expected_sequence)
        else {
            // Let the session observe the same rejection before retiring.
            let session_error = session
                .accept_read_request(request, &self.policy, expected_sequence)
                .err()
                .map_or(SourceReadError::Internal, Self::map_session_error);
            return Err(self.fail(session_error, session));
        };

        let reply_size = READ_REPLY_PREFIX_BYTES + decoded.length as usize;
        if !cancelling {
            if scratch.len() < decoded.length as usize || reply_storage.len() < reply_size {
                return Err(SourceReadError::OutputTooSmall);
            }
            if self.requests_charged >= self.limits.maximum_requests() {
                return Err(self.fail(SourceReadError::RequestBudgetExceeded, session));
            }
            if reply_size as u64
                > self.limits.maximum_reply_payload_bytes() - self.reply_payload_bytes_charged
            {
                return Err(self.fail(SourceReadError::ByteBudgetExceeded, session));
            }
            if self.ticket_counter == u64::MAX {
                return Err(self.fail(SourceReadError::TicketExhausted, session));
            }
        }

        let accepted = match session.accept_read_request(request, &self.policy, expected_sequence) {
            Ok(accepted) => accepted,
            Err(error) => return Err(self.fail(Self::map_session_error(error), session)),
        };
        let accepted = match accepted {
            ReadRequestOutcome::IgnoredAfterCancel => {
                if !cancelling {
                    return Err(self.fail(SourceReadError::Internal, session));
                }
                return Ok(PrepareOutcome::IgnoredAfterCancel);
            }
            ReadRequestOutcome::Serviceable(message) => message,
        };
        if cancelling || accepted != decoded {
            return Err(self.fail(SourceReadError::Internal, session));
        }

        self.requests_charged += 1;
        self.reply_payload_bytes_charged += reply_size as u64;

        let scratch = &mut scratch[..decoded.length as usize];
        let status = self.service(decoded.offset, scratch);
        let data: &[u8] = if status == ProtocolStatus::Ok {
            scratch
        } else {
            &[]
        };
        let encoded = encode_read_reply_payload(
            &ReadReply {
                read_sequence: expected_sequence,
                status,
                data,
            },
            expected_sequence,
            decoded.length,
            reply_storage,
        );
        // Private source bytes never linger in the caller's scratch.
        scratch.fill(0);
        let encoded = match encoded {
            Ok(encoded) => encoded,
            Err(error) => return Err(self.fail(SourceReadError::Protocol(error), session)),
        };

        self.ticket_counter += 1;
        self.pending = Some(Pending {
            ticket: self.ticket_counter,
            request_id: request.header().request_id,
            sequence: expected_sequence,
            requested_length: decoded.length,
            status,
        });
        let payload_length = u32::try_from(encoded).unwrap_or(u32::MAX);
        Ok(PrepareOutcome::ReplyReady(PreparedReply {
            ticket: ReplyTicket(self.ticket_counter),
            status,
            header: FrameHeader::new(
                MessageType::ReadReply,
                request.header().session_id,
                request.header().request_id,
                payload_length,
            ),
            payload: &reply_storage[..encoded],
        }))
    }

    /// Verifies, reads, and re-verifies the pinned source.
    fn service(&self, offset: u64, destination: &mut [u8]) -> ProtocolStatus {
        if let Err(error) = self.ops.verify_unchanged(&self.source) {
            return Self::boundary_status(error);
        }
        match self.ops.read_exact_at(&self.source, offset, destination) {
            Ok(()) => match self.ops.verify_unchanged(&self.source) {
                Ok(()) => ProtocolStatus::Ok,
                Err(error) => Self::boundary_status(error),
            },
            Err(read_error) => match self.ops.verify_unchanged(&self.source) {
                // A stable source that refused the read is a read failure; an
                // unstable one is a change, whatever the read reported.
                Ok(()) => Self::read_status(read_error),
                Err(error) => Self::boundary_status(error),
            },
        }
    }

    const fn boundary_status(error: MediaSourceError) -> ProtocolStatus {
        match error {
            MediaSourceError::Changed => ProtocolStatus::SourceChanged,
            _ => ProtocolStatus::SourceReadFailed,
        }
    }

    const fn read_status(error: MediaSourceError) -> ProtocolStatus {
        match error {
            MediaSourceError::Changed
            | MediaSourceError::UnexpectedEof
            | MediaSourceError::OutOfRange => ProtocolStatus::SourceChanged,
            _ => ProtocolStatus::SourceReadFailed,
        }
    }

    /// Records that the transport accepted the exact prepared reply in full.
    ///
    /// # Errors
    /// [`SourceReadError::InvalidState`], [`SourceReadError::InvalidTicket`],
    /// [`SourceReadError::Protocol`], [`SourceReadError::SourceChanged`] or
    /// [`SourceReadError::SourceReadFailure`].
    pub fn commit_reply_sent(
        &mut self,
        prepared: PreparedReply<'_>,
        session: &mut ResultSession,
    ) -> Result<(), SourceReadError> {
        // Destructuring consumes the single-use ticket with the reply.
        let PreparedReply {
            ticket,
            status: _,
            header,
            payload,
        } = prepared;
        let ReplyTicket(ticket) = ticket;
        let pending = self.take_pending(ticket, session)?;
        let frame = FrameView::new(header, payload);
        let accepted =
            session.accept_read_reply(&frame, pending.sequence, pending.requested_length);

        if pending.status == ProtocolStatus::Ok {
            if let Err(error) = accepted {
                return Err(self.fail(Self::map_session_error(error), session));
            }
            self.committed_request_id = Some(pending.request_id);
            if pending.sequence == u32::MAX {
                self.sequence_exhausted = true;
            } else {
                self.next_sequence = pending.sequence + 1;
                self.sequence_exhausted = false;
            }
            return Ok(());
        }

        let error = match (pending.status, accepted) {
            (ProtocolStatus::SourceChanged, Err(ResultSessionError::SourceInvalidated)) => {
                SourceReadError::SourceChanged
            }
            (ProtocolStatus::SourceReadFailed, Err(ResultSessionError::SourceReadFailure)) => {
                SourceReadError::SourceReadFailure
            }
            (_, Err(error)) => Self::map_session_error(error),
            (_, Ok(())) => SourceReadError::Internal,
        };
        Err(self.fail(error, session))
    }

    /// Records that the transport did not deliver the prepared reply.
    ///
    /// This is always terminal: session ordering is no longer knowable.
    pub fn abandon_reply(
        &mut self,
        prepared: PreparedReply<'_>,
        session: &mut ResultSession,
    ) -> SourceReadError {
        let PreparedReply { ticket, .. } = prepared;
        let ReplyTicket(ticket) = ticket;
        match self.take_pending(ticket, session) {
            Ok(_) => self.fail(SourceReadError::TransportAbandoned, session),
            Err(error) => error,
        }
    }

    fn take_pending(
        &mut self,
        ticket: u64,
        session: &mut ResultSession,
    ) -> Result<Pending, SourceReadError> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let Some(pending) = self.pending.take() else {
            return Err(self.fail(SourceReadError::InvalidState, session));
        };
        if ticket != pending.ticket {
            self.pending = Some(pending);
            return Err(self.fail(SourceReadError::InvalidTicket, session));
        }
        Ok(pending)
    }
}
