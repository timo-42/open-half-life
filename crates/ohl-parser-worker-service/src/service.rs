//! The worker-side session state machine.
//!
//! One call to [`run_parser_worker_service`] runs one complete lifetime:
//! `hello` in, `ready` out, then any number of bounded requests, then either
//! `shutdown` (clean) or a fail-closed teardown.

use ohl_parser_protocol::messages::{
    HELLO_PAYLOAD_BYTES, MAXIMUM_ENTRY_BATCH_ENTRIES, MAXIMUM_ENUMERATED_ENTRIES,
    MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES, MAXIMUM_ENUMERATED_TOTAL_BYTES,
    decode_cancel_payload, decode_enumerate_payload, decode_hello_payload,
    decode_read_reply_payload, decode_shutdown_payload, decode_stream_entry_payload,
    encode_complete_payload, encode_data_chunk_payload, encode_entry_batch_payload,
    encode_read_request_payload,
};
use ohl_parser_protocol::{
    Complete, DataChunk, Direction, EntryBatch, EntryBatchEntry, EntryBatchPolicy,
    FRAME_HEADER_BYTES, FrameHeader, FrameView, Hello, MAXIMUM_FRAME_PAYLOAD_BYTES,
    MAXIMUM_PROTOCOL_MESSAGES, MessageType, OperationPhase, ProtocolBudgets, ProtocolError,
    ProtocolPhase, ProtocolStatus, ReadRequest, SessionId, SessionState, SessionValidator,
    SourceReadPolicy, decode_frame_header,
};

use crate::capability::{
    DispatchAction, DispatchError, Dispatcher, InputStatus, IoStatus, Operation, ServiceBuffers,
    Transport,
};

/// The ceiling on dispatch steps in one session; it matches the protocol's
/// message ceiling, so a dispatcher cannot outrun the wire budget.
pub const MAXIMUM_DISPATCH_STEPS: u64 = MAXIMUM_PROTOCOL_MESSAGES;

/// Why one worker service lifetime ended without a canonical shutdown.
///
/// Every variant is a fixed, project-defined code carrying no media-derived
/// bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ServiceError {
    /// The buffers or limits were rejected before any I/O happened.
    InvalidConfiguration,
    /// The transport reported a peer close or a failure.
    TransportFailure,
    /// A frame, payload or ordering rule was violated.
    ProtocolFailure,
    /// The dispatcher cannot serve the request at all.
    DispatchUnsupported,
    /// The dispatcher failed, or produced an action the service rejected.
    DispatchFailure,
    /// The parent answered a read request with a failure status.
    SourceFailure,
    /// The dispatch step budget was exhausted.
    DispatchBudgetExceeded,
    /// A service invariant was violated.
    InternalFailure,
}

/// The report of a fail-closed lifetime.
///
/// By the time this is returned the transport was aborted exactly once and,
/// if a request was still active, the dispatcher was cancelled exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceFailure {
    /// The failure class.
    pub error: ServiceError,
    /// The protocol rule that was violated, when one was.
    pub protocol_error: Option<ProtocolError>,
    /// The transport status that ended the lifetime.
    pub io_status: IoStatus,
    /// The dispatcher status that ended the lifetime, when one did.
    pub dispatch_error: Option<DispatchError>,
    /// The negotiated session id, or zero when the handshake never got that
    /// far.
    pub session_id: u64,
    /// Dispatch steps performed before the failure.
    pub dispatch_steps: u64,
}

impl ServiceFailure {
    const fn new(error: ServiceError) -> Self {
        Self {
            error,
            protocol_error: None,
            io_status: IoStatus::Ok,
            dispatch_error: None,
            session_id: 0,
            dispatch_steps: 0,
        }
    }

    const fn transport(status: IoStatus) -> Self {
        let mut failure = Self::new(ServiceError::TransportFailure);
        failure.io_status = status;
        failure
    }

    const fn protocol(error: ProtocolError) -> Self {
        let mut failure = Self::new(ServiceError::ProtocolFailure);
        failure.protocol_error = Some(error);
        failure
    }

    const fn dispatch(error: DispatchError) -> Self {
        let mut failure = Self::new(match error {
            DispatchError::Unsupported => ServiceError::DispatchUnsupported,
            DispatchError::Failed => ServiceError::DispatchFailure,
        });
        failure.dispatch_error = Some(error);
        failure
    }

    /// A dispatcher action the service refused to put on the wire.
    const fn rejected_action(error: ProtocolError) -> Self {
        let mut failure = Self::new(ServiceError::DispatchFailure);
        failure.protocol_error = Some(error);
        failure.dispatch_error = Some(DispatchError::Failed);
        failure
    }

    const fn internal(error: ProtocolError) -> Self {
        let mut failure = Self::new(ServiceError::InternalFailure);
        failure.protocol_error = Some(error);
        failure
    }
}

/// The report of a canonical lifetime, ended by `shutdown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSummary {
    /// The negotiated session id.
    pub session_id: u64,
    /// Dispatch steps performed in the session.
    pub dispatch_steps: u64,
}

/// The budgets one lifetime runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceLimits {
    /// The wire budgets handed to the session validator.
    pub protocol_budgets: ProtocolBudgets,
    /// The dispatch-step ceiling; must be in `1..=MAXIMUM_DISPATCH_STEPS`.
    pub maximum_dispatch_steps: u64,
}

impl Default for ServiceLimits {
    fn default() -> Self {
        Self {
            protocol_budgets: ProtocolBudgets::default(),
            maximum_dispatch_steps: MAXIMUM_DISPATCH_STEPS,
        }
    }
}

impl ServiceLimits {
    const fn is_valid(&self) -> bool {
        self.maximum_dispatch_steps != 0
            && self.maximum_dispatch_steps <= MAXIMUM_DISPATCH_STEPS
            // The handshake alone costs two messages and one hello payload.
            && self.protocol_budgets.maximum_messages() >= 2
            && self.protocol_budgets.maximum_payload_bytes() >= HELLO_PAYLOAD_BYTES as u64
    }
}

/// The running enumeration budget.
///
/// The protocol's [`EntryBatchPolicy`] is a validated type that forbids a
/// zero entry or path remainder, so the service tracks raw remainders here
/// and materialises a policy only while one is still spendable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnumerationBudget {
    remaining_entries: u32,
    remaining_path_bytes: u64,
    remaining_total_bytes: u64,
    previous_source_token: Option<u64>,
}

impl EnumerationBudget {
    const fn start() -> Self {
        Self {
            remaining_entries: MAXIMUM_ENUMERATED_ENTRIES,
            remaining_path_bytes: MAXIMUM_ENUMERATED_PATH_BYTES,
            remaining_total_bytes: MAXIMUM_ENUMERATED_TOTAL_BYTES,
            previous_source_token: None,
        }
    }

    fn policy(&self) -> Result<EntryBatchPolicy, ProtocolError> {
        EntryBatchPolicy::new(
            self.remaining_entries,
            self.remaining_path_bytes,
            MAXIMUM_ENUMERATED_ENTRY_BYTES,
            self.remaining_total_bytes,
            self.previous_source_token,
        )
    }

    /// The budget left after `entries` is accepted, or `None` on overspend.
    fn spend(&self, entries: &[EntryBatchEntry<'_>]) -> Option<Self> {
        let mut next = *self;
        for entry in entries {
            next.remaining_entries = next.remaining_entries.checked_sub(1)?;
            next.remaining_path_bytes = next
                .remaining_path_bytes
                .checked_sub(entry.archive_path.len() as u64)?;
            next.remaining_total_bytes =
                next.remaining_total_bytes.checked_sub(entry.size_bytes)?;
            next.previous_source_token = Some(entry.source_token);
        }
        Some(next)
    }
}

/// What the session loop should do after one parent frame was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Shutdown,
}

/// A frame could not be received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiveError {
    Io(IoStatus),
    Protocol(ProtocolError),
}

impl From<ReceiveError> for ServiceFailure {
    fn from(error: ReceiveError) -> Self {
        match error {
            ReceiveError::Io(status) => Self::transport(status),
            ReceiveError::Protocol(error) => Self::protocol(error),
        }
    }
}

/// Reads exactly one frame, leaving its payload in `receive_payload`.
///
/// A header that is malformed or belongs to another session is rejected
/// *before* its payload is consumed, so a peer cannot make the worker read an
/// attacker-chosen byte count off the channel.
fn receive_frame<T: Transport>(
    transport: &mut T,
    receive_payload: &mut [u8],
    expected_session_id: u64,
) -> Result<FrameHeader, ReceiveError> {
    let mut header_bytes = [0_u8; FRAME_HEADER_BYTES];
    match transport.read_exact(&mut header_bytes) {
        IoStatus::Ok => {}
        status => return Err(ReceiveError::Io(status)),
    }
    let header = decode_frame_header(&header_bytes).map_err(ReceiveError::Protocol)?;
    if expected_session_id != 0 && header.session_id != expected_session_id {
        return Err(ReceiveError::Protocol(ProtocolError::WrongSessionId));
    }
    let payload = receive_payload
        .get_mut(..header.payload_length as usize)
        .ok_or(ReceiveError::Protocol(ProtocolError::PayloadTooLarge))?;
    if !payload.is_empty() {
        match transport.read_exact(payload) {
            IoStatus::Ok => {}
            status => return Err(ReceiveError::Io(status)),
        }
    }
    Ok(header)
}

/// Observes, encodes and writes one worker-to-parent frame.
fn send_frame<T: Transport>(
    transport: &mut T,
    validator: &mut SessionValidator,
    header: &FrameHeader,
    payload: &[u8],
) -> Result<(), ServiceFailure> {
    if payload.len() != header.payload_length as usize
        || payload.len() > MAXIMUM_FRAME_PAYLOAD_BYTES as usize
    {
        return Err(ServiceFailure::protocol(ProtocolError::NoncanonicalValue));
    }
    validator
        .observe(Direction::WorkerToParent, header)
        .map_err(ServiceFailure::protocol)?;
    let mut header_bytes = [0_u8; FRAME_HEADER_BYTES];
    header
        .encode(&mut header_bytes)
        .map_err(ServiceFailure::protocol)?;
    match transport.write_all(&header_bytes) {
        IoStatus::Ok => {}
        status => return Err(ServiceFailure::transport(status)),
    }
    if !payload.is_empty() {
        match transport.write_all(payload) {
            IoStatus::Ok => {}
            status => return Err(ServiceFailure::transport(status)),
        }
    }
    Ok(())
}

/// The header fields and scratch space every prepared operation frame needs.
struct FrameContext<'send> {
    session_id: u64,
    request_id: u64,
    send_payload: &'send mut [u8],
}

/// One encoded, not yet sent, worker-to-parent operation frame plus the state
/// change to commit once it reaches the wire.
struct PreparedFrame {
    header: FrameHeader,
    payload_length: usize,
    commit: Commit,
}

impl PreparedFrame {
    fn new(
        message_type: MessageType,
        context: &FrameContext<'_>,
        payload_length: usize,
        commit: Commit,
    ) -> Result<Self, ServiceFailure> {
        let length = u32::try_from(payload_length)
            .map_err(|_| ServiceFailure::internal(ProtocolError::PayloadTooLarge))?;
        Ok(Self {
            header: FrameHeader::new(message_type, context.session_id, context.request_id, length),
            payload_length,
            commit,
        })
    }
}

/// The state change a [`PreparedFrame`] commits after a successful write.
enum Commit {
    ReadRequest { sequence: u32, length: u32 },
    EntryBatch(EnumerationBudget),
    DataChunk { remaining_stream_bytes: u64 },
    Complete,
}

struct Service<'buffers, T: Transport, D: Dispatcher> {
    transport: T,
    dispatch: D,
    receive_payload: &'buffers mut [u8],
    send_payload: &'buffers mut [u8],
    limits: ServiceLimits,
    validator: SessionValidator,
    source_policy: SourceReadPolicy,
    enumeration: EnumerationBudget,
    active_operation: Operation,
    session_id: u64,
    active_request_id: u64,
    remaining_stream_bytes: u64,
    dispatch_steps: u64,
    next_read_sequence: u32,
    pending_read_sequence: u32,
    pending_read_length: u32,
    dispatch_active: bool,
    pending_read: bool,
    late_read_reply: bool,
}

impl<T: Transport, D: Dispatcher> Service<'_, T, D> {
    /// Runs the post-handshake lifetime.
    fn run(&mut self) -> Result<(), ServiceFailure> {
        loop {
            match self.validator.state() {
                SessionState::Enumerating | SessionState::Streaming => {
                    self.run_active_operation()?;
                }
                _ => {
                    let header =
                        receive_frame(&mut self.transport, self.receive_payload, self.session_id)?;
                    if self.handle_parent_frame(header)? == Flow::Shutdown {
                        return Ok(());
                    }
                }
            }
        }
    }

    fn handle_parent_frame(&mut self, header: FrameHeader) -> Result<Flow, ServiceFailure> {
        match header.message_type {
            MessageType::Enumerate => self.begin_operation(header, Operation::Enumerate),
            MessageType::StreamEntry => self.begin_operation(header, Operation::Stream),
            MessageType::Cancel => self.handle_cancel(header),
            MessageType::ReadReply => self.handle_late_read_reply(header),
            MessageType::Shutdown => self.handle_shutdown(header),
            _ => Err(ServiceFailure::protocol(ProtocolError::UnexpectedMessage)),
        }
    }

    fn begin_operation(
        &mut self,
        header: FrameHeader,
        operation: Operation,
    ) -> Result<Flow, ServiceFailure> {
        let source_token = {
            let frame = FrameView::new(
                header,
                &self.receive_payload[..header.payload_length as usize],
            );
            match operation {
                Operation::Enumerate => {
                    decode_enumerate_payload(&frame).map_err(ServiceFailure::protocol)?;
                    0
                }
                Operation::Stream => {
                    decode_stream_entry_payload(&frame)
                        .map_err(ServiceFailure::protocol)?
                        .source_token
                }
            }
        };
        self.validator
            .observe(Direction::ParentToWorker, &header)
            .map_err(ServiceFailure::protocol)?;

        let stream_size = self
            .dispatch
            .begin(operation, source_token, &self.source_policy)
            .map_err(ServiceFailure::dispatch)?;
        let size_is_canonical = match operation {
            Operation::Enumerate => stream_size == 0,
            Operation::Stream => stream_size <= MAXIMUM_ENUMERATED_ENTRY_BYTES,
        };
        if !size_is_canonical {
            // `begin` succeeded, so the dispatcher owns a request it must be
            // told to abandon. `dispatch_active` stays false, so the
            // fail-closed path cannot cancel it a second time.
            self.dispatch.cancel();
            return Err(ServiceFailure::rejected_action(
                ProtocolError::NoncanonicalValue,
            ));
        }

        self.dispatch_active = true;
        self.active_operation = operation;
        self.active_request_id = header.request_id;
        self.remaining_stream_bytes = stream_size;
        self.enumeration = EnumerationBudget::start();
        self.next_read_sequence = 1;
        self.pending_read = false;
        self.late_read_reply = false;
        Ok(Flow::Continue)
    }

    fn run_active_operation(&mut self) -> Result<(), ServiceFailure> {
        match self.transport.probe_input() {
            InputStatus::PeerClosed => {
                return Err(ServiceFailure::transport(IoStatus::PeerClosed));
            }
            InputStatus::Failed => return Err(ServiceFailure::transport(IoStatus::Failed)),
            InputStatus::Available => {
                let header =
                    receive_frame(&mut self.transport, self.receive_payload, self.session_id)?;
                if header.message_type != MessageType::Cancel {
                    return Err(ServiceFailure::protocol(ProtocolError::UnexpectedMessage));
                }
                self.handle_cancel(header)?;
                return Ok(());
            }
            InputStatus::Unavailable => {}
        }

        if self.dispatch_steps >= self.limits.maximum_dispatch_steps {
            return Err(ServiceFailure::new(ServiceError::DispatchBudgetExceeded));
        }
        self.dispatch_steps += 1;

        let prepared = {
            let mut context = FrameContext {
                session_id: self.session_id,
                request_id: self.active_request_id,
                send_payload: self.send_payload,
            };
            // The action borrows the dispatcher, so it is encoded into the
            // send buffer before the dispatcher can be touched again; there
            // is no way to read a view the dispatcher has invalidated.
            match self.dispatch.step().map_err(ServiceFailure::dispatch)? {
                DispatchAction::NeedRead { offset, length } => prepare_read_request(
                    offset,
                    length,
                    self.pending_read,
                    self.next_read_sequence,
                    &self.source_policy,
                    &mut context,
                )?,
                DispatchAction::EntryBatch(entries) => prepare_entry_batch(
                    entries,
                    self.active_operation,
                    &self.enumeration,
                    &mut context,
                )?,
                DispatchAction::DataChunk(data) => prepare_data_chunk(
                    data,
                    self.active_operation,
                    self.remaining_stream_bytes,
                    &mut context,
                )?,
                DispatchAction::Complete => prepare_complete(
                    self.active_operation,
                    self.remaining_stream_bytes,
                    &mut context,
                )?,
            }
        };

        // The outstanding read is recorded before the write, so a cancel that
        // crosses the request on the wire still finds the read in flight.
        if let Commit::ReadRequest { sequence, length } = prepared.commit {
            self.pending_read = true;
            self.pending_read_sequence = sequence;
            self.pending_read_length = length;
        }
        send_frame(
            &mut self.transport,
            &mut self.validator,
            &prepared.header,
            &self.send_payload[..prepared.payload_length],
        )?;

        match prepared.commit {
            Commit::ReadRequest { .. } => self.await_read_reply_or_cancel(),
            Commit::EntryBatch(next) => {
                self.enumeration = next;
                Ok(())
            }
            Commit::DataChunk {
                remaining_stream_bytes,
            } => {
                self.remaining_stream_bytes = remaining_stream_bytes;
                Ok(())
            }
            Commit::Complete => {
                self.dispatch.end();
                self.dispatch_active = false;
                self.active_request_id = 0;
                Ok(())
            }
        }
    }

    fn await_read_reply_or_cancel(&mut self) -> Result<(), ServiceFailure> {
        let header = receive_frame(&mut self.transport, self.receive_payload, self.session_id)?;
        match header.message_type {
            MessageType::Cancel => {
                self.handle_cancel(header)?;
                Ok(())
            }
            MessageType::ReadReply => self.accept_read_reply(header, false),
            _ => Err(ServiceFailure::protocol(ProtocolError::UnexpectedMessage)),
        }
    }

    fn accept_read_reply(
        &mut self,
        header: FrameHeader,
        discard: bool,
    ) -> Result<(), ServiceFailure> {
        if !self.pending_read {
            return Err(ServiceFailure::protocol(ProtocolError::NoReadInFlight));
        }
        let frame = FrameView::new(
            header,
            &self.receive_payload[..header.payload_length as usize],
        );
        let reply =
            decode_read_reply_payload(&frame, self.pending_read_sequence, self.pending_read_length)
                .map_err(ServiceFailure::protocol)?;
        self.validator
            .observe(Direction::ParentToWorker, &header)
            .map_err(ServiceFailure::protocol)?;

        self.pending_read = false;
        self.late_read_reply = false;
        // The sequence saturates at zero, which the next read request
        // rejects; it never silently wraps back onto a live sequence.
        self.next_read_sequence = self.pending_read_sequence.wrapping_add(1);
        self.pending_read_sequence = 0;
        self.pending_read_length = 0;

        if discard {
            return Ok(());
        }
        if reply.status != ProtocolStatus::Ok {
            return Err(ServiceFailure::new(ServiceError::SourceFailure));
        }
        self.dispatch
            .accept_read_reply(&reply)
            .map_err(ServiceFailure::dispatch)
    }

    fn handle_cancel(&mut self, header: FrameHeader) -> Result<Flow, ServiceFailure> {
        {
            let frame = FrameView::new(
                header,
                &self.receive_payload[..header.payload_length as usize],
            );
            decode_cancel_payload(&frame).map_err(ServiceFailure::protocol)?;
        }
        let prior_state = self.validator.state();
        self.validator
            .observe(Direction::ParentToWorker, &header)
            .map_err(ServiceFailure::protocol)?;
        if prior_state == SessionState::Idle {
            // A cancel that lost the race with `complete`: consumed, never
            // acknowledged.
            return Ok(Flow::Continue);
        }
        if !self.dispatch_active {
            return Err(ServiceFailure::new(ServiceError::InternalFailure));
        }

        self.dispatch.cancel();
        self.dispatch_active = false;
        self.late_read_reply = self.pending_read;
        let ack = FrameHeader::new(
            MessageType::CancelAck,
            self.session_id,
            header.request_id,
            0,
        );
        send_frame(&mut self.transport, &mut self.validator, &ack, &[])?;
        self.active_request_id = 0;
        Ok(Flow::Continue)
    }

    fn handle_late_read_reply(&mut self, header: FrameHeader) -> Result<Flow, ServiceFailure> {
        if self.validator.state() != SessionState::Cancelled || !self.late_read_reply {
            return Err(ServiceFailure::protocol(ProtocolError::UnexpectedMessage));
        }
        self.accept_read_reply(header, true)?;
        Ok(Flow::Continue)
    }

    fn handle_shutdown(&mut self, header: FrameHeader) -> Result<Flow, ServiceFailure> {
        {
            let frame = FrameView::new(
                header,
                &self.receive_payload[..header.payload_length as usize],
            );
            decode_shutdown_payload(&frame).map_err(ServiceFailure::protocol)?;
        }
        self.validator
            .observe(Direction::ParentToWorker, &header)
            .map_err(ServiceFailure::protocol)?;
        self.pending_read = false;
        self.late_read_reply = false;
        self.transport.close_io();
        Ok(Flow::Shutdown)
    }
}

fn prepare_read_request(
    offset: u64,
    length: u32,
    pending_read: bool,
    next_read_sequence: u32,
    source_policy: &SourceReadPolicy,
    context: &mut FrameContext<'_>,
) -> Result<PreparedFrame, ServiceFailure> {
    let fits = u64::from(length) <= source_policy.source_size().saturating_sub(offset);
    if pending_read
        || next_read_sequence == 0
        || length == 0
        || length > source_policy.maximum_read_bytes()
        || offset >= source_policy.source_size()
        || !fits
    {
        return Err(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ));
    }
    let message = ReadRequest {
        read_sequence: next_read_sequence,
        offset,
        length,
    };
    let payload_length = encode_read_request_payload(
        &message,
        source_policy,
        next_read_sequence,
        context.send_payload,
    )
    .map_err(ServiceFailure::rejected_action)?;
    PreparedFrame::new(
        MessageType::ReadRequest,
        context,
        payload_length,
        Commit::ReadRequest {
            sequence: next_read_sequence,
            length,
        },
    )
}

fn prepare_entry_batch(
    entries: &[EntryBatchEntry<'_>],
    operation: Operation,
    enumeration: &EnumerationBudget,
    context: &mut FrameContext<'_>,
) -> Result<PreparedFrame, ServiceFailure> {
    if operation != Operation::Enumerate
        || entries.is_empty()
        || entries.len() > MAXIMUM_ENTRY_BATCH_ENTRIES as usize
        || entries.len() > enumeration.remaining_entries as usize
    {
        return Err(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ));
    }
    // Every bounded scalar length is checked before the batch is measured or
    // handed to the encoder. `ArchiveSpelling` already forbids a spelling
    // above the ceiling or outside printable ASCII, so only the empty
    // placeholder is still rejectable here.
    if entries.iter().any(|entry| entry.archive_path.is_empty()) {
        return Err(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ));
    }
    let policy = enumeration
        .policy()
        .map_err(ServiceFailure::rejected_action)?;
    let payload_length =
        encode_entry_batch_payload(&EntryBatch { entries }, &policy, context.send_payload)
            .map_err(ServiceFailure::rejected_action)?;
    let next = enumeration
        .spend(entries)
        .ok_or(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ))?;
    PreparedFrame::new(
        MessageType::EntryBatch,
        context,
        payload_length,
        Commit::EntryBatch(next),
    )
}

fn prepare_data_chunk(
    data: &[u8],
    operation: Operation,
    remaining_stream_bytes: u64,
    context: &mut FrameContext<'_>,
) -> Result<PreparedFrame, ServiceFailure> {
    if operation != Operation::Stream || data.is_empty() {
        return Err(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ));
    }
    let payload_length = encode_data_chunk_payload(
        &DataChunk { data },
        remaining_stream_bytes,
        context.send_payload,
    )
    .map_err(ServiceFailure::rejected_action)?;
    let remaining = remaining_stream_bytes
        .checked_sub(payload_length as u64)
        .ok_or(ServiceFailure::rejected_action(
            ProtocolError::NoncanonicalValue,
        ))?;
    PreparedFrame::new(
        MessageType::DataChunk,
        context,
        payload_length,
        Commit::DataChunk {
            remaining_stream_bytes: remaining,
        },
    )
}

fn prepare_complete(
    operation: Operation,
    remaining_stream_bytes: u64,
    context: &mut FrameContext<'_>,
) -> Result<PreparedFrame, ServiceFailure> {
    let phase = match operation {
        Operation::Enumerate => OperationPhase::Enumerate,
        Operation::Stream => {
            if remaining_stream_bytes != 0 {
                return Err(ServiceFailure::rejected_action(
                    ProtocolError::NoncanonicalValue,
                ));
            }
            OperationPhase::Stream
        }
    };
    let message = Complete {
        status: ProtocolStatus::Ok,
        phase: ProtocolPhase::Complete,
    };
    let payload_length = encode_complete_payload(&message, phase, context.send_payload)
        .map_err(ServiceFailure::internal)?;
    PreparedFrame::new(
        MessageType::Complete,
        context,
        payload_length,
        Commit::Complete,
    )
}

/// Consumes `hello` and emits `ready`.
fn handshake<T: Transport>(
    transport: &mut T,
    receive_payload: &mut [u8],
    limits: ServiceLimits,
) -> Result<(SessionValidator, SessionId, SourceReadPolicy), ServiceFailure> {
    let header = receive_frame(transport, receive_payload, 0)?;
    let hello: Hello = {
        let frame = FrameView::new(header, &receive_payload[..header.payload_length as usize]);
        decode_hello_payload(&frame).map_err(ServiceFailure::protocol)?
    };
    // `decode_hello_payload` validated the header and the budget pair, so
    // neither of the next two steps can fail; they are written as fallible
    // conversions rather than assertions so the invariant stays checked.
    let session_id = SessionId::new(header.session_id)
        .ok_or(ServiceFailure::protocol(ProtocolError::InvalidSessionId))?;
    let source_policy = SourceReadPolicy::new(hello.source_size, hello.maximum_read_bytes)
        .map_err(ServiceFailure::protocol)?;

    let mut validator = SessionValidator::new(session_id, limits.protocol_budgets);
    let ready = FrameHeader::new(MessageType::Ready, session_id.get(), 0, 0);
    let established = validator
        .observe(Direction::ParentToWorker, &header)
        .map_err(ServiceFailure::protocol)
        .and_then(|()| send_frame(transport, &mut validator, &ready, &[]));
    match established {
        Ok(()) => Ok((validator, session_id, source_policy)),
        Err(mut failure) => {
            failure.session_id = session_id.get();
            Err(failure)
        }
    }
}

/// Runs exactly one worker service lifetime over `transport`.
///
/// The handshake is exact: the first frame must be a canonical `hello`, and
/// the reply is exactly one `ready`. After that the parent drives the session
/// until it sends `shutdown`.
///
/// Invalid configuration is rejected before any read, write or probe. Every
/// non-clean return aborts the transport exactly once, and cancels the
/// dispatcher exactly once when a request was still active. Nothing is
/// dispatched before the handshake completes, so a handshake failure never
/// touches the dispatcher.
///
/// # Errors
/// A [`ServiceFailure`] describing the first rule that was violated. An
/// `unsupported` dispatcher is terminal and emits no frame for the request it
/// refused.
pub fn run_parser_worker_service<T: Transport, D: Dispatcher>(
    mut transport: T,
    dispatch: D,
    buffers: ServiceBuffers<'_>,
    limits: ServiceLimits,
) -> Result<ServiceSummary, ServiceFailure> {
    let ServiceBuffers {
        receive_payload,
        send_payload,
    } = buffers;
    if receive_payload.len() < MAXIMUM_FRAME_PAYLOAD_BYTES as usize
        || send_payload.len() < MAXIMUM_FRAME_PAYLOAD_BYTES as usize
        || !limits.is_valid()
    {
        transport.abort_io();
        return Err(ServiceFailure::new(ServiceError::InvalidConfiguration));
    }

    let (validator, session_id, source_policy) =
        match handshake(&mut transport, receive_payload, limits) {
            Ok(established) => established,
            Err(failure) => {
                transport.abort_io();
                return Err(failure);
            }
        };

    let mut service = Service {
        transport,
        dispatch,
        receive_payload,
        send_payload,
        limits,
        validator,
        source_policy,
        enumeration: EnumerationBudget::start(),
        active_operation: Operation::Enumerate,
        session_id: session_id.get(),
        active_request_id: 0,
        remaining_stream_bytes: 0,
        dispatch_steps: 0,
        next_read_sequence: 1,
        pending_read_sequence: 0,
        pending_read_length: 0,
        dispatch_active: false,
        pending_read: false,
        late_read_reply: false,
    };
    match service.run() {
        Ok(()) => Ok(ServiceSummary {
            session_id: service.session_id,
            dispatch_steps: service.dispatch_steps,
        }),
        Err(mut failure) => {
            if service.dispatch_active {
                service.dispatch.cancel();
                service.dispatch_active = false;
            }
            service.transport.abort_io();
            failure.session_id = service.session_id;
            failure.dispatch_steps = service.dispatch_steps;
            Err(failure)
        }
    }
}
