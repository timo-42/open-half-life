//! The trusted parent composition for one handshaken worker lifetime.
//!
//! Port of the C++ `media::ParserParentSession`, expressed as a **typestate**
//! as `.plan/rust-architecture-r1.md` section 2 requires: transitions consume
//! the session and return the next state, so
//!
//! - `receive_one()` does not exist on [`Idle`] — a worker cannot pre-send a
//!   guessed next-request result, and the C++ `invalid_state` result is gone;
//! - `request_cancel()` exists only while a request is active;
//! - two overlapping calls are impossible, so the C++ `concurrent_operation`
//!   result, its transaction mutex and its condition variable are all gone;
//! - the stream sink is a parameter of the [`Streaming`] receive rather than a
//!   retained pointer, so it cannot outlive the stream or dangle.
//!
//! Two C++ transient errors become terminal here, deliberately fail-closed:
//! exhausting the request-id space and presenting undersized buffers are
//! programming or hostile conditions, not recoverable states.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Instant;

use ohl_media::ValidatedMedia;
use ohl_parser_protocol::messages::{STREAM_ENTRY_PAYLOAD_BYTES, encode_stream_entry_payload};

/// `stream_entry` payload length as a frame-header field.
const STREAM_ENTRY_PAYLOAD_LENGTH: u32 = 8;
const _: () = assert!(STREAM_ENTRY_PAYLOAD_LENGTH as usize == STREAM_ENTRY_PAYLOAD_BYTES);
use ohl_parser_protocol::{
    FrameHeader, FrameView, MessageType, ProtocolError, SessionId, StreamEntry,
};
use thiserror::Error;

use crate::catalog::{CatalogGeneration, CatalogView, ImportLimits, SourceToken, WorkerEpoch};
use crate::frame_channel::{ChannelError, FrameBuffer, FrameChannel};
use crate::handshake::HandshakeProof;
use crate::io::{CancellationToken, ExactIo};
use crate::result_session::{ByteSink, ResultSession, ResultSessionError};
use crate::source_read_broker::{
    NativeSourceOps, PrepareOutcome, SourceOps, SourceReadBroker, SourceReadError, SourceReadLimits,
};

/// Every way a parent session operation can fail. All variants are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The proof, channel, media or limits do not compose a session.
    #[error("invalid parent session configuration")]
    InvalidConfiguration,
    /// The 64-bit request-id space is exhausted.
    #[error("request id exhausted")]
    RequestIdExhausted,
    /// The supplied buffers are smaller than this session's read quota.
    #[error("session buffers too small")]
    BuffersTooSmall,
    /// The worker sent a frame the ordering rules forbid.
    #[error("parent session protocol failure")]
    Protocol(#[source] ProtocolError),
    /// The channel failed.
    #[error("parent session channel failure")]
    Channel(#[source] ChannelError),
    /// Result validation failed.
    #[error("parent session result failure")]
    Result(#[source] ResultSessionError),
    /// Servicing a source read failed.
    #[error("parent session source failure")]
    Source(#[source] SourceReadError),
    /// The worker was retired out of band.
    #[error("worker failure")]
    WorkerFailure,
    /// The pinned source was invalidated out of band.
    #[error("source invalidated")]
    SourceInvalidated,
}

/// The three receive buffers one session needs, sized from its own limits.
///
/// Owning them together is what makes the C++ `output_too_small` and
/// `overlapping_buffers` results unreachable: the buffers are disjoint fields
/// of one value and each is sized by construction.
#[derive(Debug)]
pub struct SessionBuffers {
    receive: FrameBuffer,
    scratch: Vec<u8>,
    reply: Vec<u8>,
    maximum_read_bytes: u32,
}

impl SessionBuffers {
    /// Allocates buffers for a session running under `limits`.
    #[must_use]
    pub fn new(limits: SourceReadLimits) -> Self {
        Self {
            receive: FrameBuffer::new(),
            scratch: vec![0; limits.maximum_read_bytes() as usize],
            reply: vec![0; limits.reply_storage_bytes()],
            maximum_read_bytes: limits.maximum_read_bytes(),
        }
    }

    /// Scrubs the reply storage, which may hold private source bytes.
    pub fn scrub_reply(&mut self) {
        self.reply.fill(0);
    }

    /// Reinitializes the receive storage after a failed receive.
    pub fn reinit_receive(&mut self) {
        self.receive.reinit();
    }
}

/// The compile-time lifecycle phase of a [`ParserSession`].
///
/// Sealed: the phase set is exactly the OWP/1 lifecycle.
pub trait SessionPhase: crate::io::sealed::Sealed {
    /// Whether dropping a session in this phase must retire the worker.
    const RETIRE_ON_DROP: bool;
    /// The runtime name of this phase.
    const KIND: SessionPhaseKind;
}

/// The runtime spelling of a typestate phase, for logs and assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionPhaseKind {
    /// No request is active.
    Idle,
    /// An `enumerate` request is active.
    Enumerating,
    /// A `stream_entry` request is active.
    Streaming,
    /// A `cancel` was sent and no terminal frame has answered it yet.
    Cancelling,
    /// A `cancel_ack` terminated the request.
    Cancelled,
    /// `shutdown` was sent.
    Closed,
}

macro_rules! phases {
    ($($name:ident => ($retire:expr, $kind:ident)),* $(,)?) => {
        $(
            /// A typestate marker; see [`SessionPhaseKind`].
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct $name;

            impl crate::io::sealed::Sealed for $name {}

            impl SessionPhase for $name {
                const RETIRE_ON_DROP: bool = $retire;
                const KIND: SessionPhaseKind = SessionPhaseKind::$kind;
            }
        )*
    };
}

phases! {
    Idle => (true, Idle),
    Enumerating => (true, Enumerating),
    Streaming => (true, Streaming),
    Cancelling => (true, Cancelling),
    Cancelled => (true, Cancelled),
    Closed => (false, Closed),
}

#[derive(Debug)]
struct SessionCore<T: ExactIo, O: SourceOps> {
    channel: Arc<FrameChannel<T>>,
    results: ResultSession,
    reads: SourceReadBroker<O>,
    limits: SourceReadLimits,
    next_request_id: u64,
    request_ids_exhausted: bool,
    active_request_id: u64,
}

impl<T: ExactIo, O: SourceOps> SessionCore<T, O> {
    fn allocate_request_id(&mut self) -> Result<u64, SessionError> {
        if self.request_ids_exhausted {
            return Err(SessionError::RequestIdExhausted);
        }
        let request_id = self.next_request_id;
        if request_id == u64::MAX {
            self.request_ids_exhausted = true;
        } else {
            self.next_request_id = request_id + 1;
        }
        Ok(request_id)
    }

    /// Retires every authority this session holds and interrupts channel I/O.
    fn retire(&mut self, error: SessionError) -> SessionError {
        self.results.worker_failed();
        self.channel.abort();
        error
    }
}

/// A parent session in phase `S` over channel transport `T`.
///
/// Dropping a live session retires the result state and aborts the channel; it
/// owns no process lifecycle operation, so reaping stays with
/// [`crate::ProcessSession`].
#[derive(Debug)]
pub struct ParserSession<S: SessionPhase, T: ExactIo, O: SourceOps = NativeSourceOps> {
    core: Option<Box<SessionCore<T, O>>>,
    phase: PhantomData<fn() -> S>,
}

/// A session that failed terminally. It holds the retained failure and no
/// further protocol authority.
#[derive(Debug)]
pub struct TerminalSession<T: ExactIo, O: SourceOps = NativeSourceOps> {
    core: Box<SessionCore<T, O>>,
    error: SessionError,
}

impl<T: ExactIo, O: SourceOps> TerminalSession<T, O> {
    /// The failure that retired the session.
    #[must_use]
    pub const fn error(&self) -> SessionError {
        self.error
    }

    /// The retained result-session failure, if one was recorded.
    #[must_use]
    pub const fn result_failure(&self) -> Option<ResultSessionError> {
        self.core.results.failure()
    }

    /// The retained source-read failure, if one was recorded.
    #[must_use]
    pub const fn source_failure(&self) -> Option<SourceReadError> {
        self.core.reads.failure()
    }

    /// The retained channel failure, if the channel was poisoned.
    #[must_use]
    pub fn channel_failure(&self) -> Option<ChannelError> {
        self.core.channel.failure()
    }
}

impl<S: SessionPhase, T: ExactIo, O: SourceOps> Drop for ParserSession<S, T, O> {
    fn drop(&mut self) {
        if S::RETIRE_ON_DROP
            && let Some(core) = self.core.as_mut()
        {
            let _ = core.retire(SessionError::WorkerFailure);
        }
    }
}

impl<S: SessionPhase, T: ExactIo, O: SourceOps> ParserSession<S, T, O> {
    fn wrap(core: Box<SessionCore<T, O>>) -> Self {
        Self {
            core: Some(core),
            phase: PhantomData,
        }
    }

    fn core(&self) -> &SessionCore<T, O> {
        self.core.as_ref().expect("session core outlives its phase")
    }

    fn take(mut self) -> Box<SessionCore<T, O>> {
        self.core.take().expect("session core outlives its phase")
    }

    fn advance<N: SessionPhase>(
        mut core: Box<SessionCore<T, O>>,
        outcome: Result<(), SessionError>,
    ) -> Result<ParserSession<N, T, O>, TerminalSession<T, O>> {
        match outcome {
            Ok(()) => Ok(ParserSession::wrap(core)),
            Err(error) => {
                let error = core.retire(error);
                Err(TerminalSession { core, error })
            }
        }
    }

    /// The phase of this session.
    #[must_use]
    pub const fn phase(&self) -> SessionPhaseKind {
        S::KIND
    }

    /// The session id every frame carries.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.core().channel.session_id()
    }

    /// The active top-level request id, or zero.
    #[must_use]
    pub fn active_request_id(&self) -> u64 {
        self.core().active_request_id
    }

    /// Reads charged against this session's request quota.
    #[must_use]
    pub const fn requests_charged(&self) -> u64 {
        match self.core.as_ref() {
            Some(core) => core.reads.requests_charged(),
            None => 0,
        }
    }

    /// The promoted catalog, if one is live.
    ///
    /// The view borrows the session, so it cannot outlive the enumeration,
    /// cancellation, failure or shutdown that invalidates it.
    #[must_use]
    pub fn catalog(&self) -> Option<CatalogView<'_>> {
        self.core().results.catalog()
    }

    /// Trusted out-of-band notification that the worker died or misbehaved.
    #[must_use]
    pub fn notify_worker_failed(self) -> TerminalSession<T, O> {
        let mut core = self.take();
        let error = core.retire(SessionError::WorkerFailure);
        TerminalSession { core, error }
    }

    /// Trusted out-of-band notification that the pinned source changed.
    #[must_use]
    pub fn invalidate_source(self) -> TerminalSession<T, O> {
        let mut core = self.take();
        core.results.invalidate_source();
        core.channel.abort();
        TerminalSession {
            core,
            error: SessionError::SourceInvalidated,
        }
    }
}

/// One frame consumed by a receive, before the phase is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Received {
    Progress,
    ReadReplied,
    ReadIgnored,
    Complete,
    CancelAck,
}

/// What one [`Enumerating`] or [`Streaming`] receive produced.
#[derive(Debug)]
pub enum RequestStep<S: SessionPhase, T: ExactIo, O: SourceOps = NativeSourceOps> {
    /// A bounded result frame was validated; the request continues.
    Progress(ParserSession<S, T, O>),
    /// A source read was serviced and answered; the request continues.
    ReadReplied(ParserSession<S, T, O>),
    /// The request completed; the session is idle again.
    Complete(ParserSession<Idle, T, O>),
}

/// What one [`Cancelling`] receive produced.
#[derive(Debug)]
pub enum CancelStep<T: ExactIo, O: SourceOps = NativeSourceOps> {
    /// A result frame that was already in flight was validated.
    Progress(ParserSession<Cancelling, T, O>),
    /// A crossed source read was consumed without being serviced.
    ReadIgnored(ParserSession<Cancelling, T, O>),
    /// A read reply that was already crossing resolved the read.
    ReadReplied(ParserSession<Cancelling, T, O>),
    /// Cancellation was acknowledged.
    Acknowledged(ParserSession<Cancelled, T, O>),
    /// Completion won the race with cancellation; no `cancel_ack` follows.
    Complete(ParserSession<Idle, T, O>),
}

/// Which top-level request a receive belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveRequest {
    Enumeration,
    Stream,
}

fn receive_one_inner<T: ExactIo, O: SourceOps>(
    core: &mut SessionCore<T, O>,
    buffers: &mut SessionBuffers,
    sink: Option<&mut dyn ByteSink>,
    active: ActiveRequest,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<Received, SessionError> {
    if buffers.maximum_read_bytes < core.limits.maximum_read_bytes() {
        return Err(SessionError::BuffersTooSmall);
    }
    let frame = core
        .channel
        .receive(&mut buffers.receive, deadline, cancellation)
        .map_err(SessionError::Channel)?;

    match frame.header().message_type {
        MessageType::EntryBatch => {
            core.results
                .accept_entry_batch(&frame)
                .map_err(SessionError::Result)?;
            Ok(Received::Progress)
        }
        MessageType::DataChunk => {
            let sink = sink.ok_or(SessionError::Protocol(ProtocolError::UnexpectedMessage))?;
            core.results
                .accept_data_chunk(&frame, sink)
                .map_err(SessionError::Result)?;
            Ok(Received::Progress)
        }
        MessageType::Complete => {
            match active {
                ActiveRequest::Enumeration => core.results.complete_enumeration(&frame),
                ActiveRequest::Stream => core.results.complete_stream(&frame),
            }
            .map_err(SessionError::Result)?;
            Ok(Received::Complete)
        }
        MessageType::CancelAck => {
            core.results
                .accept_cancel_ack(&frame)
                .map_err(SessionError::Result)?;
            Ok(Received::CancelAck)
        }
        MessageType::ReadRequest => {
            let prepared = core
                .reads
                .prepare(
                    &mut core.results,
                    &frame,
                    &mut buffers.scratch,
                    &mut buffers.reply,
                )
                .map_err(SessionError::Source)?;
            let prepared = match prepared {
                PrepareOutcome::IgnoredAfterCancel => return Ok(Received::ReadIgnored),
                PrepareOutcome::ReplyReady(prepared) => prepared,
            };
            match core.channel.send(
                prepared.header(),
                prepared.payload(),
                deadline,
                cancellation,
            ) {
                Ok(()) => {
                    core.reads
                        .commit_reply_sent(prepared, &mut core.results)
                        .map_err(SessionError::Source)?;
                    Ok(Received::ReadReplied)
                }
                Err(error) => {
                    // A partially delivered reply makes ordering unknowable.
                    let _ = core.reads.abandon_reply(prepared, &mut core.results);
                    Err(SessionError::Channel(error))
                }
            }
        }
        MessageType::Hello
        | MessageType::Ready
        | MessageType::Enumerate
        | MessageType::StreamEntry
        | MessageType::ReadReply
        | MessageType::Cancel
        | MessageType::Shutdown => Err(SessionError::Protocol(ProtocolError::UnexpectedMessage)),
    }
}

fn send_transaction<T: ExactIo, O: SourceOps>(
    core: &mut SessionCore<T, O>,
    header: &FrameHeader,
    payload: &[u8],
    observe: impl FnOnce(&mut ResultSession, &FrameView<'_>) -> Result<(), ResultSessionError>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<(), SessionError> {
    let frame = FrameView::new(*header, payload);
    observe(&mut core.results, &frame).map_err(SessionError::Result)?;
    core.channel
        .send(header, payload, deadline, cancellation)
        .map_err(SessionError::Channel)
}

impl<T: ExactIo, O: SourceOps> ParserSession<Idle, T, O> {
    /// Sends `enumerate` and begins a candidate enumeration.
    ///
    /// # Errors
    /// The retired session; see [`SessionError`].
    pub fn begin_enumeration(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Enumerating, T, O>, TerminalSession<T, O>> {
        let mut core = self.take();
        let outcome = core.allocate_request_id().and_then(|request_id| {
            let header = FrameHeader::new(
                MessageType::Enumerate,
                core.channel.session_id().get(),
                request_id,
                0,
            );
            send_transaction(
                &mut core,
                &header,
                &[],
                ResultSession::begin_enumeration,
                deadline,
                cancellation,
            )?;
            core.active_request_id = request_id;
            Ok(())
        });
        Self::advance(core, outcome)
    }

    /// Sends `stream_entry` for one promoted catalog entry.
    ///
    /// `generation` must be the exact generation the catalog was promoted
    /// with, so a stale catalog user is rejected even when a restarted worker
    /// reuses the same token.
    ///
    /// # Errors
    /// The retired session; see [`SessionError`].
    pub fn begin_stream(
        self,
        generation: CatalogGeneration,
        source_token: SourceToken,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Streaming, T, O>, TerminalSession<T, O>> {
        let mut core = self.take();
        let mut payload = [0_u8; STREAM_ENTRY_PAYLOAD_BYTES];
        let outcome = encode_stream_entry_payload(
            &StreamEntry {
                source_token: source_token.0,
            },
            &mut payload,
        )
        .map_err(SessionError::Protocol)
        .and_then(|_| core.allocate_request_id())
        .and_then(|request_id| {
            let header = FrameHeader::new(
                MessageType::StreamEntry,
                core.channel.session_id().get(),
                request_id,
                STREAM_ENTRY_PAYLOAD_LENGTH,
            );
            send_transaction(
                &mut core,
                &header,
                &payload,
                |results, frame| results.begin_stream_entry(frame, generation),
                deadline,
                cancellation,
            )?;
            core.active_request_id = request_id;
            Ok(())
        });
        Self::advance(core, outcome)
    }

    /// Sends `shutdown`. This closes the protocol session only: it does not
    /// close, terminate, wait for or reap the worker.
    ///
    /// # Errors
    /// The retired session; see [`SessionError`].
    pub fn shutdown(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>> {
        shutdown_session(self, deadline, cancellation)
    }
}

impl<T: ExactIo, O: SourceOps> ParserSession<Cancelled, T, O> {
    /// Sends `shutdown` after a cancellation was acknowledged.
    ///
    /// # Errors
    /// The retired session; see [`SessionError`].
    pub fn shutdown(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>> {
        shutdown_session(self, deadline, cancellation)
    }
}

fn shutdown_session<S: SessionPhase, T: ExactIo, O: SourceOps>(
    session: ParserSession<S, T, O>,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>> {
    let mut core = session.take();
    let header = FrameHeader::new(MessageType::Shutdown, core.channel.session_id().get(), 0, 0);
    let outcome = send_transaction(
        &mut core,
        &header,
        &[],
        ResultSession::accept_shutdown,
        deadline,
        cancellation,
    );
    core.active_request_id = 0;
    ParserSession::<S, T, O>::advance(core, outcome)
}

macro_rules! request_phase {
    ($phase:ident, $active:expr, $sink_doc:literal) => {
        impl<T: ExactIo, O: SourceOps> ParserSession<$phase, T, O> {
            /// Receives and synchronously consumes exactly one worker frame.
            ///
            #[doc = $sink_doc]
            ///
            /// No frame or payload view escapes this call. Any scratch prefix
            /// written for a source read is scrubbed before returning; the
            /// reply storage is not, may retain private source bytes, and must
            /// be sanitized ([`SessionBuffers::scrub_reply`]) before it is
            /// logged or reused outside the private transport path.
            ///
            /// # Errors
            /// The retired session; see [`SessionError`].
            pub fn receive_one(
                self,
                buffers: &mut SessionBuffers,
                sink: Option<&mut dyn ByteSink>,
                deadline: Instant,
                cancellation: &CancellationToken,
            ) -> Result<RequestStep<$phase, T, O>, TerminalSession<T, O>> {
                let mut core = self.take();
                let received =
                    receive_one_inner(&mut core, buffers, sink, $active, deadline, cancellation);
                match received {
                    Ok(Received::Progress) => Ok(RequestStep::Progress(ParserSession::wrap(core))),
                    Ok(Received::ReadReplied) => {
                        Ok(RequestStep::ReadReplied(ParserSession::wrap(core)))
                    }
                    Ok(Received::Complete) => {
                        core.active_request_id = 0;
                        Ok(RequestStep::Complete(ParserSession::wrap(core)))
                    }
                    Ok(Received::ReadIgnored | Received::CancelAck) => {
                        let error =
                            core.retire(SessionError::Protocol(ProtocolError::UnexpectedMessage));
                        Err(TerminalSession { core, error })
                    }
                    Err(error) => {
                        let error = core.retire(error);
                        Err(TerminalSession { core, error })
                    }
                }
            }

            /// Sends `cancel` for the active request.
            ///
            /// # Errors
            /// The retired session; see [`SessionError`].
            pub fn request_cancel(
                self,
                deadline: Instant,
                cancellation: &CancellationToken,
            ) -> Result<ParserSession<Cancelling, T, O>, TerminalSession<T, O>> {
                let mut core = self.take();
                let header = FrameHeader::new(
                    MessageType::Cancel,
                    core.channel.session_id().get(),
                    core.active_request_id,
                    0,
                );
                let outcome = send_transaction(
                    &mut core,
                    &header,
                    &[],
                    ResultSession::accept_cancel,
                    deadline,
                    cancellation,
                );
                Self::advance(core, outcome)
            }
        }
    };
}

request_phase!(
    Enumerating,
    ActiveRequest::Enumeration,
    "`sink` is unused while enumerating and should be `None`."
);
request_phase!(
    Streaming,
    ActiveRequest::Stream,
    "`sink` receives every validated `data_chunk`; a `None` sink rejects one."
);

impl<T: ExactIo, O: SourceOps> ParserSession<Cancelling, T, O> {
    /// Drains the cancellation race: bounded result frames already in flight,
    /// a crossed read request, a late read reply, and finally either
    /// `cancel_ack` or the `complete` that won the race.
    ///
    /// # Errors
    /// The retired session; see [`SessionError`].
    pub fn receive_one(
        self,
        buffers: &mut SessionBuffers,
        sink: Option<&mut dyn ByteSink>,
        active_was_stream: bool,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<CancelStep<T, O>, TerminalSession<T, O>> {
        let mut core = self.take();
        let active = if active_was_stream {
            ActiveRequest::Stream
        } else {
            ActiveRequest::Enumeration
        };
        match receive_one_inner(&mut core, buffers, sink, active, deadline, cancellation) {
            Ok(Received::Progress) => Ok(CancelStep::Progress(ParserSession::wrap(core))),
            Ok(Received::ReadIgnored) => Ok(CancelStep::ReadIgnored(ParserSession::wrap(core))),
            Ok(Received::ReadReplied) => Ok(CancelStep::ReadReplied(ParserSession::wrap(core))),
            Ok(Received::CancelAck) => {
                core.active_request_id = 0;
                Ok(CancelStep::Acknowledged(ParserSession::wrap(core)))
            }
            Ok(Received::Complete) => {
                core.active_request_id = 0;
                Ok(CancelStep::Complete(ParserSession::wrap(core)))
            }
            Err(error) => {
                let error = core.retire(error);
                Err(TerminalSession { core, error })
            }
        }
    }
}

/// Builds a session from a handshake proof over the native source methods.
///
/// # Errors
/// [`SessionError::InvalidConfiguration`] when the proof does not belong to
/// `channel`, the channel is already poisoned, the media disagrees with the
/// proof, or the protocol validator is not idle. The channel is aborted then.
pub fn create_parser_session<T: ExactIo>(
    proof: HandshakeProof<T>,
    channel: Arc<FrameChannel<T>>,
    media: &ValidatedMedia,
    worker_epoch: WorkerEpoch,
    import_limits: ImportLimits,
) -> Result<ParserSession<Idle, T, NativeSourceOps>, SessionError> {
    create_parser_session_with_ops(
        proof,
        channel,
        media,
        worker_epoch,
        import_limits,
        NativeSourceOps,
    )
}

/// Builds a session over an explicit source-operation seam.
///
/// # Errors
/// See [`create_parser_session`].
pub fn create_parser_session_with_ops<T: ExactIo, O: SourceOps>(
    proof: HandshakeProof<T>,
    channel: Arc<FrameChannel<T>>,
    media: &ValidatedMedia,
    worker_epoch: WorkerEpoch,
    import_limits: ImportLimits,
    ops: O,
) -> Result<ParserSession<Idle, T, O>, SessionError> {
    let limits = proof.source_read_limits();
    let policy = proof.source_read_policy();
    if !proof.matches_channel(&channel)
        || channel.is_terminal()
        || media.source().size() != media.size_bytes()
        || policy.source_size() != ops.window_length(media.source())
        || policy.maximum_read_bytes() != limits.maximum_read_bytes()
    {
        channel.abort();
        return Err(SessionError::InvalidConfiguration);
    }

    let build = ResultSession::new(proof.take_protocol(), worker_epoch, import_limits).and_then(
        |mut results| match SourceReadBroker::with_ops(media, &mut results, limits, ops) {
            Ok(reads) => Ok((results, reads)),
            Err(_) => Err(ResultSessionError::InvalidConfiguration),
        },
    );
    let Ok((results, reads)) = build else {
        channel.abort();
        return Err(SessionError::InvalidConfiguration);
    };

    Ok(ParserSession::wrap(Box::new(SessionCore {
        channel,
        results,
        reads,
        limits,
        next_request_id: 1,
        request_ids_exhausted: false,
        active_request_id: 0,
    })))
}
