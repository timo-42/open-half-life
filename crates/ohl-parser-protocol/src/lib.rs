//! OWP/1: the Open Half-Life parser-worker wire protocol.
//!
//! This crate is the Rust port of the C++ contract in
//! `src/parser/include/ohl/parser/protocol.hpp` and `protocol_messages.hpp`.
//! It carries the whole trust boundary between the privileged parent and the
//! sandboxed parser worker, so it is deliberately minimal:
//!
//! - `#![no_std]` **and allocation-free** — the identical code compiles into
//!   the freestanding worker binary, which has no allocator;
//! - `#![forbid(unsafe_code)]` — fixed headers are parsed with [`zerocopy`],
//!   variable payloads with hand-written bounded readers
//!   ([`payload::PayloadReader`]), so no pointer arithmetic exists to audit;
//! - every decoder is *canonical*: it rejects trailing bytes, short payloads,
//!   out-of-range enum values, non-zero reserved header fields and any value
//!   outside the caller-supplied budget, and it never mutates caller storage
//!   on failure;
//! - every error is a fixed, project-defined code ([`ProtocolError`]) whose
//!   `Display` and `Debug` never interpolate media-derived bytes. The same
//!   rule applies to the message types: [`Debug`] for a payload-carrying
//!   message prints its length, never its contents.
//!
//! The C++ "span aliases frame storage" comments become real lifetimes here:
//! [`messages::DataChunk<'frame>`], [`messages::ReadReply<'frame>`] and
//! [`messages::EntryBatch<'frame, 'storage>`] borrow the frame buffer (and,
//! for entry batches, the caller's entry storage) instead of owning it.
//!
//! # Layout
//!
//! Every integer on the wire is unsigned little-endian. A frame is a
//! [`FRAME_HEADER_BYTES`]-byte header followed by exactly
//! `payload_length` bytes:
//!
//! | offset | size | field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `OHLP` ([`FRAME_MAGIC`]) |
//! | 4 | 2 | major version (`1`) |
//! | 6 | 2 | minor version (`0`) |
//! | 8 | 2 | [`MessageType`] |
//! | 10 | 2 | flags, reserved, must be zero |
//! | 12 | 4 | payload length (`<= `[`MAXIMUM_FRAME_PAYLOAD_BYTES`]) |
//! | 16 | 8 | session id (non-zero) |
//! | 24 | 8 | request id (zero exactly for hello/ready/shutdown) |

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod frame;
pub mod messages;
pub mod payload;
pub mod session;

pub use frame::{FrameHeader, FrameView, decode_frame, decode_frame_header, encode_frame};
pub use messages::{
    ArchiveSpelling, Complete, DataChunk, EntryBatch, EntryBatchEntry, EntryBatchPolicy, Hello,
    OperationPhase, ReadReply, ReadRequest, SourceReadPolicy, StreamEntry,
};
pub use payload::{PayloadReader, PayloadWriter};
pub use session::{Direction, ProtocolBudgets, SessionId, SessionState, SessionValidator};

use thiserror::Error;

/// Bytes in the fixed frame header.
pub const FRAME_HEADER_BYTES: usize = 32;

/// The four magic bytes that begin every frame.
pub const FRAME_MAGIC: [u8; 4] = *b"OHLP";

/// The only supported major protocol version.
pub const PROTOCOL_MAJOR_VERSION: u16 = 1;

/// The only supported minor protocol version.
pub const PROTOCOL_MINOR_VERSION: u16 = 0;

/// The largest payload a single frame may declare or carry (1 MiB).
pub const MAXIMUM_FRAME_PAYLOAD_BYTES: u32 = 1 << 20;

/// The default ceiling on messages observed in one session.
pub const MAXIMUM_PROTOCOL_MESSAGES: u64 = 1 << 20;

/// The default ceiling on cumulative payload bytes in one session (64 GiB).
pub const MAXIMUM_CUMULATIVE_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// A message kind. Unknown wire values are rejected by
/// [`MessageType::from_wire`], so an unknown type is unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum MessageType {
    /// Parent -> worker: opens the session and states the source budget.
    Hello = 0x0001,
    /// Worker -> parent: handshake accepted.
    Ready = 0x0002,
    /// Parent -> worker: begin a bounded enumeration request.
    Enumerate = 0x0010,
    /// Parent -> worker: begin a bounded single-entry stream request.
    StreamEntry = 0x0011,
    /// Worker -> parent: ask the parent to read from the pinned source.
    ReadRequest = 0x0020,
    /// Parent -> worker: answer exactly one outstanding read request.
    ReadReply = 0x0021,
    /// Worker -> parent: a bounded batch of enumerated entries.
    EntryBatch = 0x0030,
    /// Worker -> parent: a bounded chunk of streamed entry bytes.
    DataChunk = 0x0031,
    /// Worker -> parent: the active request finished.
    Complete = 0x0032,
    /// Parent -> worker: cancel the active request.
    Cancel = 0x0040,
    /// Worker -> parent: cancellation acknowledged (terminal).
    CancelAck = 0x0041,
    /// Parent -> worker: close the session.
    Shutdown = 0x0042,
}

impl MessageType {
    /// Decodes a wire value.
    ///
    /// # Errors
    /// [`ProtocolError::UnknownMessageType`] for any value outside the table.
    pub const fn from_wire(value: u16) -> Result<Self, ProtocolError> {
        Ok(match value {
            0x0001 => Self::Hello,
            0x0002 => Self::Ready,
            0x0010 => Self::Enumerate,
            0x0011 => Self::StreamEntry,
            0x0020 => Self::ReadRequest,
            0x0021 => Self::ReadReply,
            0x0030 => Self::EntryBatch,
            0x0031 => Self::DataChunk,
            0x0032 => Self::Complete,
            0x0040 => Self::Cancel,
            0x0041 => Self::CancelAck,
            0x0042 => Self::Shutdown,
            _ => return Err(ProtocolError::UnknownMessageType),
        })
    }

    /// The wire value of this message type.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        self as u16
    }

    /// Whether this message kind must carry a zero request id.
    #[must_use]
    pub const fn requires_zero_request_id(self) -> bool {
        matches!(self, Self::Hello | Self::Ready | Self::Shutdown)
    }
}

/// A protocol-level status code carried by typed payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum ProtocolStatus {
    /// The operation succeeded.
    Ok = 0,
    /// The request is not supported by this worker.
    Unsupported = 1,
    /// The request was malformed.
    InvalidRequest = 2,
    /// The parser refused the media.
    ParserRejected = 3,
    /// A budget was exhausted.
    BudgetExceeded = 4,
    /// The operation was cancelled.
    Cancelled = 5,
    /// The pinned source changed underneath the session.
    SourceChanged = 6,
    /// Reading the pinned source failed.
    SourceReadFailed = 7,
    /// The produced result failed validation.
    ResultValidationFailed = 8,
    /// An internal invariant was violated.
    InternalFailure = 9,
}

impl ProtocolStatus {
    /// Decodes a wire value.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`] for any value outside the table.
    pub const fn from_wire(value: u16) -> Result<Self, ProtocolError> {
        Ok(match value {
            0 => Self::Ok,
            1 => Self::Unsupported,
            2 => Self::InvalidRequest,
            3 => Self::ParserRejected,
            4 => Self::BudgetExceeded,
            5 => Self::Cancelled,
            6 => Self::SourceChanged,
            7 => Self::SourceReadFailed,
            8 => Self::ResultValidationFailed,
            9 => Self::InternalFailure,
            _ => return Err(ProtocolError::NoncanonicalValue),
        })
    }

    /// The wire value of this status.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        self as u16
    }
}

/// The phase a completion or read reply refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum ProtocolPhase {
    /// The handshake.
    Handshake = 0,
    /// A bounded enumeration request.
    Enumerate = 1,
    /// A bounded single-entry stream request.
    Stream = 2,
    /// A parent-serviced source read.
    SourceRead = 3,
    /// The terminal phase of a finished request.
    Complete = 4,
}

impl ProtocolPhase {
    /// Decodes a wire value.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`] for any value outside the table.
    pub const fn from_wire(value: u16) -> Result<Self, ProtocolError> {
        Ok(match value {
            0 => Self::Handshake,
            1 => Self::Enumerate,
            2 => Self::Stream,
            3 => Self::SourceRead,
            4 => Self::Complete,
            _ => return Err(ProtocolError::NoncanonicalValue),
        })
    }

    /// The wire value of this phase.
    #[must_use]
    pub const fn to_wire(self) -> u16 {
        self as u16
    }
}

/// Every way an OWP/1 frame, payload, budget or ordering rule can be
/// violated.
///
/// Each variant is a fixed, project-defined code; no variant carries data, so
/// neither `Display` nor `Debug` can ever leak media-derived bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// Fewer than [`FRAME_HEADER_BYTES`] bytes were available.
    #[error("truncated header")]
    TruncatedHeader,
    /// The frame did not begin with [`FRAME_MAGIC`].
    #[error("invalid magic")]
    InvalidMagic,
    /// The frame declared a version other than 1.0.
    #[error("unsupported version")]
    UnsupportedVersion,
    /// The frame declared a message type outside [`MessageType`].
    #[error("unknown message type")]
    UnknownMessageType,
    /// The reserved flags field was non-zero.
    #[error("reserved flags")]
    ReservedFlags,
    /// A payload exceeded [`MAXIMUM_FRAME_PAYLOAD_BYTES`].
    #[error("payload too large")]
    PayloadTooLarge,
    /// Fewer bytes were supplied than the header declared.
    #[error("truncated payload")]
    TruncatedPayload,
    /// More bytes were supplied than the header declared.
    #[error("trailing bytes")]
    TrailingBytes,
    /// The session id was zero.
    #[error("invalid session id")]
    InvalidSessionId,
    /// The request id was zero for an operation frame, or non-zero for a
    /// handshake frame.
    #[error("invalid request id")]
    InvalidRequestId,
    /// The frame belonged to a different session.
    #[error("wrong session id")]
    WrongSessionId,
    /// The frame belonged to a different request.
    #[error("wrong request id")]
    WrongRequestId,
    /// A new request did not use a strictly greater request id.
    #[error("request id not monotonic")]
    RequestIdNotMonotonic,
    /// The message is not allowed in this state or direction.
    #[error("unexpected message")]
    UnexpectedMessage,
    /// A second top-level request was started while one was active.
    #[error("request already active")]
    RequestAlreadyActive,
    /// A second read was requested while one was outstanding.
    #[error("read already active")]
    ReadAlreadyActive,
    /// A read reply arrived with no outstanding read request.
    #[error("no read in flight")]
    NoReadInFlight,
    /// The session message budget was exhausted.
    #[error("message budget exceeded")]
    MessageBudgetExceeded,
    /// The session cumulative payload budget was exhausted.
    #[error("byte budget exceeded")]
    ByteBudgetExceeded,
    /// A caller-supplied budget or context was itself out of range.
    #[error("invalid budget")]
    InvalidBudget,
    /// The destination buffer was too small for the encoded bytes.
    #[error("output too small")]
    OutputTooSmall,
    /// A payload field ran past the end of the payload.
    #[error("payload underflow")]
    PayloadUnderflow,
    /// The payload was not fully consumed by its schema.
    #[error("payload trailing bytes")]
    PayloadTrailingBytes,
    /// A field held a value the canonical encoding forbids.
    #[error("noncanonical value")]
    NoncanonicalValue,
    /// The session already reached a terminal state.
    #[error("terminal state")]
    TerminalState,
}

impl From<ProtocolError> for ohl_core::SanitizedError {
    fn from(error: ProtocolError) -> Self {
        match error {
            ProtocolError::MessageBudgetExceeded
            | ProtocolError::ByteBudgetExceeded
            | ProtocolError::PayloadTooLarge => Self::Unsupported,
            ProtocolError::TerminalState => Self::NotFound,
            _ => Self::InvalidInput,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageType, ProtocolError, ProtocolPhase, ProtocolStatus};

    #[test]
    fn message_types_round_trip_through_the_wire() {
        for expected in [
            MessageType::Hello,
            MessageType::Ready,
            MessageType::Enumerate,
            MessageType::StreamEntry,
            MessageType::ReadRequest,
            MessageType::ReadReply,
            MessageType::EntryBatch,
            MessageType::DataChunk,
            MessageType::Complete,
            MessageType::Cancel,
            MessageType::CancelAck,
            MessageType::Shutdown,
        ] {
            assert_eq!(MessageType::from_wire(expected.to_wire()), Ok(expected));
        }
        assert_eq!(
            MessageType::from_wire(0xffff),
            Err(ProtocolError::UnknownMessageType)
        );
        assert_eq!(
            MessageType::from_wire(0),
            Err(ProtocolError::UnknownMessageType)
        );
    }

    #[test]
    fn statuses_and_phases_reject_values_outside_the_table() {
        for value in 0..=9_u16 {
            assert!(ProtocolStatus::from_wire(value).is_ok());
        }
        assert_eq!(
            ProtocolStatus::from_wire(10),
            Err(ProtocolError::NoncanonicalValue)
        );
        for value in 0..=4_u16 {
            assert!(ProtocolPhase::from_wire(value).is_ok());
        }
        assert_eq!(
            ProtocolPhase::from_wire(5),
            Err(ProtocolError::NoncanonicalValue)
        );
    }

    #[test]
    fn zero_request_id_kinds_match_the_contract() {
        assert!(MessageType::Hello.requires_zero_request_id());
        assert!(MessageType::Ready.requires_zero_request_id());
        assert!(MessageType::Shutdown.requires_zero_request_id());
        assert!(!MessageType::Enumerate.requires_zero_request_id());
        assert!(!MessageType::Complete.requires_zero_request_id());
    }
}
