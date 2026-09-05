//! The parent hello/ready exchange.
//!
//! Port of the C++ `media::perform_parser_parent_handshake` and
//! `ParserParentHandshakeProof`. Exactly one synchronous exchange happens per
//! call; the caller must own exclusive access to a fresh channel for its
//! duration.
//!
//! The proof is move-only, and [`HandshakeProof::take_protocol`] consumes it,
//! so the C++ `valid_`/moved-from bookkeeping disappears: a proof can be used
//! exactly once, checked by the compiler.

use std::marker::PhantomData;
use std::time::Instant;

use ohl_media::ValidatedMedia;
use ohl_parser_protocol::messages::{
    HELLO_PAYLOAD_BYTES, decode_ready_payload, encode_hello_payload,
};

/// `hello` payload length as a frame-header field.
const HELLO_PAYLOAD_LENGTH: u32 = 12;
const _: () = assert!(HELLO_PAYLOAD_LENGTH as usize == HELLO_PAYLOAD_BYTES);
use ohl_parser_protocol::{
    Direction, FrameHeader, Hello, MessageType, ProtocolBudgets, ProtocolError, SessionId,
    SessionState, SessionValidator, SourceReadPolicy,
};
use thiserror::Error;

use crate::frame_channel::{ChannelError, FrameBuffer, FrameChannel};
use crate::io::{CancellationToken, ExactIo};
use crate::source_read_broker::SourceReadLimits;

/// Every way the parent handshake can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum HandshakeError {
    /// The media, limits or budgets cannot open a session.
    #[error("invalid handshake configuration")]
    InvalidConfiguration,
    /// The worker's answer violated OWP/1.
    #[error("handshake protocol failure")]
    Protocol(#[source] ProtocolError),
    /// The channel failed or was already poisoned.
    #[error("handshake channel failure")]
    Channel(#[source] ChannelError),
    /// A parent-side invariant was violated after a complete exchange.
    #[error("handshake internal failure")]
    Internal,
}

/// A successful binding between the typed hello, the exact channel that
/// carried it, and the validator that observed it.
///
/// The proof owns no source and no process capability. Read the policy and
/// limits before taking the validator, then build the source broker from the
/// same [`ValidatedMedia`] and these exact limits.
#[derive(Debug)]
pub struct HandshakeProof<T: ExactIo> {
    protocol: SessionValidator,
    channel: usize,
    session_id: SessionId,
    source_read_limits: SourceReadLimits,
    source_read_policy: SourceReadPolicy,
    marker: PhantomData<fn() -> T>,
}

impl<T: ExactIo> HandshakeProof<T> {
    /// Whether this proof was produced by exactly `channel`.
    #[must_use]
    pub fn matches_channel(&self, channel: &FrameChannel<T>) -> bool {
        std::ptr::from_ref(channel) as usize == self.channel
            && channel.session_id() == self.session_id
    }

    /// The session the exchange opened.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The read quotas the hello advertised.
    #[must_use]
    pub const fn source_read_limits(&self) -> SourceReadLimits {
        self.source_read_limits
    }

    /// The read policy the hello pinned.
    #[must_use]
    pub const fn source_read_policy(&self) -> SourceReadPolicy {
        self.source_read_policy
    }

    /// Transfers the idle, already-charged validator. Consumes the proof.
    #[must_use]
    pub fn take_protocol(self) -> SessionValidator {
        self.protocol
    }
}

/// Performs exactly one parent hello/ready exchange.
///
/// `buffer` is not scrubbed. After payload I/O begins or typed-ready
/// validation fails it may hold an attacker-controlled prefix followed by
/// stale bytes; [`FrameBuffer`] then refuses further receives until it is
/// reinitialized. A failure after channel interaction terminally aborts the
/// channel; process termination and reap remain the caller's responsibility.
///
/// # Errors
/// [`HandshakeError::InvalidConfiguration`] before any I/O,
/// [`HandshakeError::Channel`], [`HandshakeError::Protocol`] or
/// [`HandshakeError::Internal`].
pub fn perform_parent_handshake<T: ExactIo>(
    channel: &FrameChannel<T>,
    media: &ValidatedMedia,
    source_read_limits: SourceReadLimits,
    protocol_budgets: ProtocolBudgets,
    buffer: &mut FrameBuffer,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<HandshakeProof<T>, HandshakeError> {
    if let Some(failure) = channel.failure() {
        return Err(HandshakeError::Channel(failure));
    }
    if media.source().size() != media.size_bytes()
        || protocol_budgets.maximum_messages() < 2
        || protocol_budgets.maximum_payload_bytes() < u64::from(HELLO_PAYLOAD_LENGTH)
    {
        return Err(HandshakeError::InvalidConfiguration);
    }
    let source_read_policy =
        SourceReadPolicy::new(media.size_bytes(), source_read_limits.maximum_read_bytes())
            .map_err(|_| HandshakeError::InvalidConfiguration)?;

    let mut protocol = SessionValidator::new(channel.session_id(), protocol_budgets);
    let mut payload = [0_u8; HELLO_PAYLOAD_BYTES];
    encode_hello_payload(
        &Hello {
            source_size: source_read_policy.source_size(),
            maximum_read_bytes: source_read_policy.maximum_read_bytes(),
        },
        &mut payload,
    )
    .map_err(HandshakeError::Protocol)?;

    let header = FrameHeader::new(
        MessageType::Hello,
        channel.session_id().get(),
        0,
        HELLO_PAYLOAD_LENGTH,
    );
    channel
        .send(&header, &payload, deadline, cancellation)
        .map_err(|error| abort(channel, HandshakeError::Channel(error)))?;
    protocol
        .observe(Direction::ParentToWorker, &header)
        .map_err(|error| abort(channel, HandshakeError::Protocol(error)))?;

    let received = channel
        .receive(buffer, deadline, cancellation)
        .map_err(|error| abort(channel, HandshakeError::Channel(error)))?;
    decode_ready_payload(&received)
        .map_err(|error| abort(channel, HandshakeError::Protocol(error)))?;
    protocol
        .observe(Direction::WorkerToParent, received.header())
        .map_err(|error| abort(channel, HandshakeError::Protocol(error)))?;

    if protocol.state() != SessionState::Idle
        || protocol.message_count() != 2
        || protocol.payload_bytes() != u64::from(HELLO_PAYLOAD_LENGTH)
    {
        return Err(abort(channel, HandshakeError::Internal));
    }

    Ok(HandshakeProof {
        protocol,
        channel: std::ptr::from_ref(channel) as usize,
        session_id: channel.session_id(),
        source_read_limits,
        source_read_policy,
        marker: PhantomData,
    })
}

fn abort<T: ExactIo>(channel: &FrameChannel<T>, error: HandshakeError) -> HandshakeError {
    channel.abort();
    error
}
