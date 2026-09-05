//! The OWP/1 session-ordering validator.
//!
//! Both peers run one [`SessionValidator`] per session and feed it every
//! header they send and receive. It enforces the handshake, one top-level
//! request at a time, one outstanding read at a time, monotonically
//! increasing request ids, the message and byte budgets, and the duplex
//! completion/cancellation race rules.
//!
//! Completion has deterministic priority when `complete` and `cancel` cross
//! in the duplex transport. A `complete` observed while cancelling finishes
//! normally; one same-request `cancel` observed immediately after `complete`
//! is stale and is consumed without a `cancel_ack`. Starting another request
//! closes that race window. `cancel_ack` remains terminal only when it
//! arrives before `complete`. While cancellation is pending, only
//! same-request traffic can cross. A reply may resolve a read that was
//! already outstanding at cancellation, after which result and completion
//! frames may finish normally. A read request first seen after cancellation
//! cannot be serviced and requires `cancel_ack` termination. If `cancel_ack`
//! overtakes the already-enqueued reply for a pre-cancel read, the cancelled
//! session accepts that same-request reply exactly once only to drain the
//! transport. A post-cancel read request never opens this drain window, and
//! accepting the reply, `shutdown`, or any terminal failure closes it.

use core::num::NonZeroU64;

use crate::frame::FrameHeader;
use crate::{
    MAXIMUM_CUMULATIVE_PAYLOAD_BYTES, MAXIMUM_PROTOCOL_MESSAGES, MessageType, ProtocolError,
};

/// A non-zero session id. Zero is rejected by the wire format, so it is
/// unrepresentable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(NonZeroU64);

impl SessionId {
    /// Builds a session id, rejecting zero.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The session id as a plain integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// The per-session message and payload ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolBudgets {
    maximum_messages: u64,
    maximum_payload_bytes: u64,
}

impl ProtocolBudgets {
    /// Validates a budget pair.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidBudget`] when either component is zero or
    /// above its protocol ceiling.
    pub const fn new(
        maximum_messages: u64,
        maximum_payload_bytes: u64,
    ) -> Result<Self, ProtocolError> {
        if maximum_messages == 0
            || maximum_messages > MAXIMUM_PROTOCOL_MESSAGES
            || maximum_payload_bytes == 0
            || maximum_payload_bytes > MAXIMUM_CUMULATIVE_PAYLOAD_BYTES
        {
            return Err(ProtocolError::InvalidBudget);
        }
        Ok(Self {
            maximum_messages,
            maximum_payload_bytes,
        })
    }

    /// The message ceiling.
    #[must_use]
    pub const fn maximum_messages(&self) -> u64 {
        self.maximum_messages
    }

    /// The cumulative payload ceiling.
    #[must_use]
    pub const fn maximum_payload_bytes(&self) -> u64 {
        self.maximum_payload_bytes
    }
}

impl Default for ProtocolBudgets {
    fn default() -> Self {
        Self {
            maximum_messages: MAXIMUM_PROTOCOL_MESSAGES,
            maximum_payload_bytes: MAXIMUM_CUMULATIVE_PAYLOAD_BYTES,
        }
    }
}

/// Which peer sent the frame being observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The privileged parent sent it.
    ParentToWorker,
    /// The sandboxed worker sent it.
    WorkerToParent,
}

/// The session lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Nothing observed yet; only `hello` from the parent is allowed.
    AwaitingHello,
    /// `hello` seen; only `ready` from the worker is allowed.
    AwaitingReady,
    /// Handshake complete, no request active.
    Idle,
    /// An `enumerate` request is active.
    Enumerating,
    /// A `stream_entry` request is active.
    Streaming,
    /// A `cancel` was observed and no terminal frame has answered it yet.
    Cancelling,
    /// A `cancel_ack` terminated the request; only `shutdown` (and at most
    /// one drained late read reply) may follow.
    Cancelled,
    /// `shutdown` was observed.
    Closed,
    /// A rule was violated; the failure is sticky.
    Failed,
}

/// Validates one side's view of an OWP/1 session.
///
/// The session id and budgets are validated types ([`SessionId`],
/// [`ProtocolBudgets`]), so a validator can never be built with an
/// out-of-range budget or a zero session.
#[derive(Debug)]
pub struct SessionValidator {
    session_id: SessionId,
    budgets: ProtocolBudgets,
    state: SessionState,
    error: Option<ProtocolError>,
    active_request_id: u64,
    last_request_id: u64,
    completed_request_id: u64,
    late_read_reply_request_id: u64,
    message_count: u64,
    payload_bytes: u64,
    active_result_type: MessageType,
    read_in_flight: bool,
    accept_late_cancel: bool,
    crossed_read_request_seen: bool,
}

impl SessionValidator {
    /// Starts a session validator.
    #[must_use]
    pub const fn new(session_id: SessionId, budgets: ProtocolBudgets) -> Self {
        Self {
            session_id,
            budgets,
            state: SessionState::AwaitingHello,
            error: None,
            active_request_id: 0,
            last_request_id: 0,
            completed_request_id: 0,
            late_read_reply_request_id: 0,
            message_count: 0,
            payload_bytes: 0,
            active_result_type: MessageType::Complete,
            read_in_flight: false,
            accept_late_cancel: false,
            crossed_read_request_seen: false,
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// The sticky failure, if any.
    #[must_use]
    pub const fn error(&self) -> Option<ProtocolError> {
        self.error
    }

    /// The active top-level request id, or zero.
    #[must_use]
    pub const fn active_request_id(&self) -> u64 {
        self.active_request_id
    }

    /// Messages observed so far.
    #[must_use]
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    /// Cumulative payload bytes observed so far.
    #[must_use]
    pub const fn payload_bytes(&self) -> u64 {
        self.payload_bytes
    }

    fn fail(&mut self, error: ProtocolError) -> Result<(), ProtocolError> {
        self.late_read_reply_request_id = 0;
        self.error = Some(error);
        self.state = SessionState::Failed;
        Err(error)
    }

    fn charge(&mut self, header: &FrameHeader) -> Result<(), ProtocolError> {
        if self.message_count >= self.budgets.maximum_messages() {
            return self.fail(ProtocolError::MessageBudgetExceeded);
        }
        self.message_count += 1;
        if u64::from(header.payload_length)
            > self.budgets.maximum_payload_bytes() - self.payload_bytes
        {
            return self.fail(ProtocolError::ByteBudgetExceeded);
        }
        self.payload_bytes += u64::from(header.payload_length);
        Ok(())
    }

    fn begin_request(
        &mut self,
        header: &FrameHeader,
        state: SessionState,
    ) -> Result<(), ProtocolError> {
        if header.request_id <= self.last_request_id {
            return self.fail(ProtocolError::RequestIdNotMonotonic);
        }
        self.last_request_id = header.request_id;
        self.active_request_id = header.request_id;
        self.completed_request_id = 0;
        self.late_read_reply_request_id = 0;
        self.accept_late_cancel = false;
        self.read_in_flight = false;
        self.crossed_read_request_seen = false;
        self.active_result_type = if state == SessionState::Enumerating {
            MessageType::EntryBatch
        } else {
            MessageType::DataChunk
        };
        self.state = state;
        Ok(())
    }

    fn complete_request(&mut self, accept_late_cancel: bool) {
        self.completed_request_id = self.active_request_id;
        self.active_request_id = 0;
        self.late_read_reply_request_id = 0;
        self.read_in_flight = false;
        self.crossed_read_request_seen = false;
        self.accept_late_cancel = accept_late_cancel;
        self.state = SessionState::Idle;
    }

    fn observe_active(
        &mut self,
        direction: Direction,
        header: &FrameHeader,
        result_type: MessageType,
    ) -> Result<(), ProtocolError> {
        if matches!(
            header.message_type,
            MessageType::Enumerate | MessageType::StreamEntry
        ) {
            return self.fail(ProtocolError::RequestAlreadyActive);
        }
        if header.request_id == 0 {
            return self.fail(ProtocolError::UnexpectedMessage);
        }
        if header.request_id != self.active_request_id {
            return self.fail(ProtocolError::WrongRequestId);
        }
        match (header.message_type, direction) {
            (MessageType::ReadRequest, Direction::WorkerToParent) => {
                if self.read_in_flight {
                    return self.fail(ProtocolError::ReadAlreadyActive);
                }
                self.read_in_flight = true;
                Ok(())
            }
            (MessageType::ReadReply, Direction::ParentToWorker) => {
                if !self.read_in_flight {
                    return self.fail(ProtocolError::NoReadInFlight);
                }
                self.read_in_flight = false;
                Ok(())
            }
            (message_type, Direction::WorkerToParent) if message_type == result_type => {
                if self.read_in_flight {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                Ok(())
            }
            (MessageType::Complete, Direction::WorkerToParent) => {
                if self.read_in_flight {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                // Completion wins a legitimate duplex race with cancellation.
                // Until a new top-level request starts, one same-request
                // cancel may therefore be observed after this completion and
                // is consumed as stale without an acknowledgement.
                self.complete_request(true);
                Ok(())
            }
            (MessageType::Cancel, Direction::ParentToWorker) => {
                self.crossed_read_request_seen = false;
                self.state = SessionState::Cancelling;
                Ok(())
            }
            _ => self.fail(ProtocolError::UnexpectedMessage),
        }
    }

    fn observe_idle(
        &mut self,
        direction: Direction,
        header: &FrameHeader,
    ) -> Result<(), ProtocolError> {
        if direction != Direction::ParentToWorker {
            return self.fail(ProtocolError::UnexpectedMessage);
        }
        match header.message_type {
            MessageType::Enumerate => self.begin_request(header, SessionState::Enumerating),
            MessageType::StreamEntry => self.begin_request(header, SessionState::Streaming),
            MessageType::Shutdown => {
                self.state = SessionState::Closed;
                Ok(())
            }
            MessageType::Cancel if self.accept_late_cancel => {
                if header.request_id != self.completed_request_id {
                    return self.fail(ProtocolError::WrongRequestId);
                }
                self.accept_late_cancel = false;
                Ok(())
            }
            _ => self.fail(ProtocolError::UnexpectedMessage),
        }
    }

    fn observe_cancelling(
        &mut self,
        direction: Direction,
        header: &FrameHeader,
    ) -> Result<(), ProtocolError> {
        if header.request_id != self.active_request_id {
            return self.fail(ProtocolError::WrongRequestId);
        }
        let result_type = self.active_result_type;
        match (header.message_type, direction) {
            (MessageType::CancelAck, Direction::WorkerToParent) => {
                self.late_read_reply_request_id = if self.read_in_flight {
                    self.active_request_id
                } else {
                    0
                };
                self.active_request_id = 0;
                self.read_in_flight = false;
                self.crossed_read_request_seen = false;
                self.state = SessionState::Cancelled;
                Ok(())
            }
            (MessageType::Complete, Direction::WorkerToParent) => {
                if self.read_in_flight || self.crossed_read_request_seen {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                // The worker committed completion before observing the
                // cancel. The complete frame is the terminal response; no
                // cancel_ack follows.
                self.complete_request(false);
                Ok(())
            }
            (MessageType::ReadRequest, Direction::WorkerToParent) => {
                if self.read_in_flight || self.crossed_read_request_seen {
                    return self.fail(ProtocolError::ReadAlreadyActive);
                }
                self.crossed_read_request_seen = true;
                Ok(())
            }
            (MessageType::ReadReply, Direction::ParentToWorker) => {
                // Only a reply already crossing for the read that preceded
                // cancellation can resolve that read and permit normal
                // completion.
                if !self.read_in_flight {
                    return self.fail(ProtocolError::NoReadInFlight);
                }
                self.read_in_flight = false;
                Ok(())
            }
            (message_type, Direction::WorkerToParent) if message_type == result_type => {
                if self.read_in_flight || self.crossed_read_request_seen {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                // Bounded result frames already in flight may precede the
                // completion that wins a race with cancellation.
                Ok(())
            }
            _ => self.fail(ProtocolError::UnexpectedMessage),
        }
    }

    fn observe_cancelled(
        &mut self,
        direction: Direction,
        header: &FrameHeader,
    ) -> Result<(), ProtocolError> {
        if direction == Direction::ParentToWorker {
            match header.message_type {
                MessageType::Shutdown => {
                    self.late_read_reply_request_id = 0;
                    self.state = SessionState::Closed;
                    return Ok(());
                }
                MessageType::ReadReply if self.late_read_reply_request_id != 0 => {
                    if header.request_id != self.late_read_reply_request_id {
                        return self.fail(ProtocolError::WrongRequestId);
                    }
                    self.late_read_reply_request_id = 0;
                    return Ok(());
                }
                _ => {}
            }
        }
        self.fail(ProtocolError::TerminalState)
    }

    /// Observes one frame header in `direction`.
    ///
    /// # Errors
    /// The first rule this frame violates. Failures are sticky: once a
    /// session fails, every later observation reports the same error and the
    /// state stays [`SessionState::Failed`].
    pub fn observe(
        &mut self,
        direction: Direction,
        header: &FrameHeader,
    ) -> Result<(), ProtocolError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.state == SessionState::Closed {
            return self.fail(ProtocolError::TerminalState);
        }
        if let Err(error) = header.validate() {
            return self.fail(error);
        }
        if header.session_id != self.session_id.get() {
            return self.fail(ProtocolError::WrongSessionId);
        }
        self.charge(header)?;

        match self.state {
            SessionState::AwaitingHello => {
                if direction != Direction::ParentToWorker
                    || header.message_type != MessageType::Hello
                {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                self.state = SessionState::AwaitingReady;
                Ok(())
            }
            SessionState::AwaitingReady => {
                if direction != Direction::WorkerToParent
                    || header.message_type != MessageType::Ready
                {
                    return self.fail(ProtocolError::UnexpectedMessage);
                }
                self.state = SessionState::Idle;
                Ok(())
            }
            SessionState::Idle => self.observe_idle(direction, header),
            SessionState::Enumerating => {
                self.observe_active(direction, header, MessageType::EntryBatch)
            }
            SessionState::Streaming => {
                self.observe_active(direction, header, MessageType::DataChunk)
            }
            SessionState::Cancelling => self.observe_cancelling(direction, header),
            SessionState::Cancelled => self.observe_cancelled(direction, header),
            SessionState::Closed | SessionState::Failed => self.fail(ProtocolError::TerminalState),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, ProtocolBudgets, SessionId, SessionState, SessionValidator};
    use crate::frame::FrameHeader;
    use crate::{MessageType, ProtocolError};

    fn validator() -> SessionValidator {
        SessionValidator::new(
            SessionId::new(7).expect("non-zero"),
            ProtocolBudgets::default(),
        )
    }

    #[test]
    fn zero_session_ids_are_unrepresentable() {
        assert!(SessionId::new(0).is_none());
        assert_eq!(
            ProtocolBudgets::new(0, 1),
            Err(ProtocolError::InvalidBudget)
        );
    }

    #[test]
    fn handshake_then_shutdown_closes_the_session() {
        let mut validator = validator();
        assert_eq!(
            validator.observe(
                Direction::ParentToWorker,
                &FrameHeader::new(MessageType::Hello, 7, 0, 0)
            ),
            Ok(())
        );
        assert_eq!(validator.state(), SessionState::AwaitingReady);
        assert_eq!(
            validator.observe(
                Direction::WorkerToParent,
                &FrameHeader::new(MessageType::Ready, 7, 0, 0)
            ),
            Ok(())
        );
        assert_eq!(validator.state(), SessionState::Idle);
        assert_eq!(
            validator.observe(
                Direction::ParentToWorker,
                &FrameHeader::new(MessageType::Shutdown, 7, 0, 0)
            ),
            Ok(())
        );
        assert_eq!(validator.state(), SessionState::Closed);
        assert_eq!(validator.message_count(), 3);
    }
}
