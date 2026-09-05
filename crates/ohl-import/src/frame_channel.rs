//! Canonical OWP/1 framing over one trusted exact byte channel.
//!
//! Port of the C++ `media::ParserFrameChannel`. The channel owns no process
//! and no buffers: the caller keeps the receive storage ([`FrameBuffer`]) and
//! remains responsible for orderly close or terminate-and-reap.
//!
//! Differences from the C++ original, all of them removing a runtime check:
//!
//! - the session id is a [`SessionId`] (`NonZeroU64`) and the operations are
//!   an [`ExactIo`] value, so `invalid_configuration` is unrepresentable;
//! - the "storage must have capacity for the protocol maximum" precondition
//!   is enforced by [`FrameBuffer`]'s constructor, so `output_too_small`
//!   cannot be reached with a well-typed buffer;
//! - "once payload I/O begins the whole buffer is invalid" is enforced rather
//!   than documented: a failed receive marks the buffer unusable and
//!   [`FrameChannel::receive`] refuses it until [`FrameBuffer::reinit`].
//!
//! Everything else is faithful: header-first bounded I/O, terminal poisoning
//! that retains the *first* failure and aborts the transport exactly once, one
//! active operation per direction, and sanitization of impossible I/O (a
//! transfer that claims a byte count other than the requested one is a
//! transport failure, never a short frame).

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use ohl_parser_protocol::{
    FRAME_HEADER_BYTES, FrameHeader, FrameView, MAXIMUM_FRAME_PAYLOAD_BYTES, ProtocolError,
    SessionId, decode_frame_header,
};
use thiserror::Error;

use crate::io::{CancellationToken, ExactIo, IoError};

/// Every way a frame-channel operation can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum ChannelError {
    /// Another operation is already active in this direction. Not terminal.
    #[error("concurrent channel operation")]
    ConcurrentOperation,
    /// The receive buffer still holds an untrusted partial frame. Not
    /// terminal; call [`FrameBuffer::reinit`] first.
    #[error("receive buffer invalidated")]
    BufferInvalidated,
    /// A frame violated OWP/1. Terminal.
    #[error("frame protocol failure")]
    Protocol(#[source] ProtocolError),
    /// The transport failed. Terminal.
    #[error("frame transport failure")]
    Transport(#[source] IoError),
    /// The channel was aborted. Terminal.
    #[error("frame channel aborted")]
    Aborted,
}

impl ChannelError {
    /// The protocol error behind this failure, if any.
    #[must_use]
    pub const fn protocol_error(self) -> Option<ProtocolError> {
        match self {
            Self::Protocol(error) => Some(error),
            _ => None,
        }
    }

    /// Whether observing this error poisons the channel.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Protocol(_) | Self::Transport(_) | Self::Aborted)
    }
}

/// Receive storage sized for the protocol maximum.
///
/// A buffer starts usable. A receive that fails *after* payload I/O has begun
/// leaves an attacker-controlled partial prefix followed by stale prior bytes,
/// so the whole buffer becomes unusable: no part of it may be parsed or reused
/// as a frame until [`FrameBuffer::reinit`] reinitializes it.
#[derive(Debug)]
pub struct FrameBuffer {
    bytes: Box<[u8]>,
    usable: bool,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuffer {
    /// Allocates a buffer with capacity for the largest legal payload.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bytes: vec![0; MAXIMUM_FRAME_PAYLOAD_BYTES as usize].into_boxed_slice(),
            usable: true,
        }
    }

    /// Whether the buffer may be used to receive a frame.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.usable
    }

    /// Scrubs the buffer and makes it usable again.
    pub fn reinit(&mut self) {
        self.bytes.fill(0);
        self.usable = true;
    }
}

#[derive(Debug, Default)]
struct ChannelState {
    failure: Option<ChannelError>,
    send_active: bool,
    receive_active: bool,
}

/// Frames exactly one OWP/1 session over a trusted exact byte channel.
///
/// `send` and `receive` may overlap with each other but not with themselves.
#[derive(Debug)]
pub struct FrameChannel<T: ExactIo> {
    session_id: SessionId,
    io: T,
    state: Mutex<ChannelState>,
}

/// Which direction of the channel an operation occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Send,
    Receive,
}

impl<T: ExactIo> FrameChannel<T> {
    /// Binds a session id to an exact byte channel.
    pub const fn new(session_id: SessionId, io: T) -> Self {
        Self {
            session_id,
            io,
            state: Mutex::new(ChannelState {
                failure: None,
                send_active: false,
                receive_active: false,
            }),
        }
    }

    /// The pinned session id.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The underlying capability.
    pub const fn io(&self) -> &T {
        &self.io
    }

    fn state(&self) -> MutexGuard<'_, ChannelState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the channel is terminally poisoned.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state().failure.is_some()
    }

    /// The retained first failure, if the channel is poisoned.
    #[must_use]
    pub fn failure(&self) -> Option<ChannelError> {
        self.state().failure
    }

    /// Terminally poisons the channel and interrupts active I/O. Repeated
    /// calls have no additional effect.
    pub fn abort(&self) {
        let _ = self.poison(ChannelError::Aborted);
    }

    fn poison(&self, failure: ChannelError) -> ChannelError {
        let (retained, abort) = {
            let mut state = self.state();
            if let Some(existing) = state.failure {
                (existing, false)
            } else {
                state.failure = Some(failure);
                (failure, true)
            }
        };
        if abort {
            self.io.abort_io();
        }
        retained
    }

    fn begin(&self, direction: Direction) -> Result<(), ChannelError> {
        let mut state = self.state();
        if let Some(failure) = state.failure {
            return Err(failure);
        }
        let active = match direction {
            Direction::Send => &mut state.send_active,
            Direction::Receive => &mut state.receive_active,
        };
        if *active {
            return Err(ChannelError::ConcurrentOperation);
        }
        *active = true;
        Ok(())
    }

    /// Ends a successful operation, yielding to a failure retained by the
    /// concurrent direction.
    fn end_success(&self, direction: Direction) -> Result<(), ChannelError> {
        let mut state = self.state();
        Self::clear(&mut state, direction);
        state.failure.map_or(Ok(()), Err)
    }

    /// Ends a failed operation; the retained first failure wins.
    fn end_failure(&self, direction: Direction, error: ChannelError) -> ChannelError {
        let mut state = self.state();
        Self::clear(&mut state, direction);
        state.failure.unwrap_or(error)
    }

    fn clear(state: &mut ChannelState, direction: Direction) {
        match direction {
            Direction::Send => state.send_active = false,
            Direction::Receive => state.receive_active = false,
        }
    }

    /// Transfers exactly `expected` bytes, sanitizing impossible outcomes.
    fn transferred(reported: Result<usize, IoError>, expected: usize) -> Result<(), IoError> {
        match reported {
            Ok(count) if count == expected => Ok(()),
            // A "successful" transfer of the wrong length is impossible I/O:
            // never trust it as a short frame.
            Ok(_) => Err(IoError::IoFailure),
            Err(error) => Err(error),
        }
    }

    /// Sends one frame: header first, then the payload.
    ///
    /// # Errors
    /// [`ChannelError::ConcurrentOperation`] while another send is active, the
    /// retained failure of a poisoned channel, [`ChannelError::Protocol`] for
    /// a header that is not canonical, does not belong to this session or
    /// disagrees with `payload`, or [`ChannelError::Transport`].
    pub fn send(
        &self,
        header: &FrameHeader,
        payload: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), ChannelError> {
        self.begin(Direction::Send)?;
        match self.send_inner(header, payload, deadline, cancellation) {
            Ok(()) => self.end_success(Direction::Send),
            Err(error) => Err(self.end_failure(Direction::Send, error)),
        }
    }

    fn send_inner(
        &self,
        header: &FrameHeader,
        payload: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), ChannelError> {
        let mut encoded = [0_u8; FRAME_HEADER_BYTES];
        let protocol = self
            .validate_outgoing(header, payload)
            .and_then(|()| header.encode(&mut encoded));
        if let Err(error) = protocol {
            return Err(self.poison(ChannelError::Protocol(error)));
        }

        Self::transferred(
            self.io.write_all(&encoded, deadline, cancellation),
            encoded.len(),
        )
        .map_err(|error| self.poison(ChannelError::Transport(error)))?;

        if !payload.is_empty() {
            Self::transferred(
                self.io.write_all(payload, deadline, cancellation),
                payload.len(),
            )
            .map_err(|error| self.poison(ChannelError::Transport(error)))?;
        }
        Ok(())
    }

    fn validate_outgoing(&self, header: &FrameHeader, payload: &[u8]) -> Result<(), ProtocolError> {
        header.validate()?;
        if header.session_id != self.session_id.get() {
            return Err(ProtocolError::WrongSessionId);
        }
        if payload.len() > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
            return Err(ProtocolError::PayloadTooLarge);
        }
        if header.payload_length as usize != payload.len() {
            return Err(ProtocolError::NoncanonicalValue);
        }
        Ok(())
    }

    /// Receives one frame into `buffer`.
    ///
    /// The header is consumed and validated before any payload byte is read,
    /// so an untrusted length never drives storage use and never leaves the
    /// byte stream between frames. A pre-payload failure leaves `buffer`
    /// untouched and still usable; a failure during payload I/O invalidates
    /// the entire buffer.
    ///
    /// # Errors
    /// [`ChannelError::ConcurrentOperation`],
    /// [`ChannelError::BufferInvalidated`], the retained failure of a poisoned
    /// channel, [`ChannelError::Protocol`] or [`ChannelError::Transport`].
    pub fn receive<'buffer>(
        &self,
        buffer: &'buffer mut FrameBuffer,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<FrameView<'buffer>, ChannelError> {
        self.begin(Direction::Receive)?;
        if !buffer.usable {
            self.end_success(Direction::Receive)?;
            return Err(ChannelError::BufferInvalidated);
        }
        match self.receive_inner(buffer, deadline, cancellation) {
            Ok(header) => {
                let length = header.payload_length as usize;
                self.end_success(Direction::Receive)?;
                Ok(FrameView::new(header, &buffer.bytes[..length]))
            }
            Err(error) => Err(self.end_failure(Direction::Receive, error)),
        }
    }

    fn receive_inner(
        &self,
        buffer: &mut FrameBuffer,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<FrameHeader, ChannelError> {
        let mut header_bytes = [0_u8; FRAME_HEADER_BYTES];
        Self::transferred(
            self.io
                .read_exact(&mut header_bytes, deadline, cancellation),
            header_bytes.len(),
        )
        .map_err(|error| self.poison(ChannelError::Transport(error)))?;

        let header = decode_frame_header(&header_bytes)
            .and_then(|header| {
                if header.session_id == self.session_id.get() {
                    Ok(header)
                } else {
                    Err(ProtocolError::WrongSessionId)
                }
            })
            .map_err(|error| self.poison(ChannelError::Protocol(error)))?;

        let length = header.payload_length as usize;
        if length != 0 {
            // Payload mutation begins here, so the buffer is untrusted until
            // the transfer completes.
            buffer.usable = false;
            Self::transferred(
                self.io
                    .read_exact(&mut buffer.bytes[..length], deadline, cancellation),
                length,
            )
            .map_err(|error| self.poison(ChannelError::Transport(error)))?;
            buffer.usable = true;
        }
        Ok(header)
    }
}
