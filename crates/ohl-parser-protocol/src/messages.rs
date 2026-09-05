//! The twelve typed OWP/1 message schemas.
//!
//! Every decoder here is canonical and failure-atomic: it validates the frame
//! header, the declared-versus-supplied payload length, the schema shape, and
//! the caller's budget before producing a message, and it never writes to
//! caller storage on any failing path.
//!
//! Budgets that the C++ contract passed as plain structs are validated
//! *constructors* here ([`SourceReadPolicy::new`], [`EntryBatchPolicy::new`],
//! [`OperationPhase`]), so an out-of-range budget cannot reach a decoder at
//! all.

use core::fmt;

use ohl_core::CheckedArithmetic as _;

use crate::frame::FrameView;
use crate::payload::{PayloadReader, PayloadWriter};
use crate::{
    MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType, ProtocolError, ProtocolPhase, ProtocolStatus,
};

/// Payload bytes of a `hello` frame.
pub const HELLO_PAYLOAD_BYTES: usize = 12;
/// Payload bytes of a `ready` frame.
pub const READY_PAYLOAD_BYTES: usize = 0;
/// Payload bytes of an `enumerate` frame.
pub const ENUMERATE_PAYLOAD_BYTES: usize = 0;
/// Payload bytes of a `stream_entry` frame.
pub const STREAM_ENTRY_PAYLOAD_BYTES: usize = 8;
/// Payload bytes of a `read_request` frame.
pub const READ_REQUEST_PAYLOAD_BYTES: usize = 16;
/// Fixed prefix bytes of a `read_reply` frame.
pub const READ_REPLY_PREFIX_BYTES: usize = 6;
/// Fixed prefix bytes of an `entry_batch` frame.
pub const ENTRY_BATCH_PREFIX_BYTES: usize = 2;
/// Fixed prefix bytes of one entry inside an `entry_batch` frame.
pub const ENTRY_BATCH_ENTRY_PREFIX_BYTES: usize = 18;
/// Payload bytes of a `complete` frame.
pub const COMPLETE_PAYLOAD_BYTES: usize = 4;
/// Payload bytes of a `cancel` frame.
pub const CANCEL_PAYLOAD_BYTES: usize = 0;
/// Payload bytes of a `cancel_ack` frame.
pub const CANCEL_ACK_PAYLOAD_BYTES: usize = 0;
/// Payload bytes of a `shutdown` frame.
pub const SHUTDOWN_PAYLOAD_BYTES: usize = 0;

/// The largest single source read the protocol can carry in one reply.
pub const MAXIMUM_READ_BYTES: u32 = MAXIMUM_FRAME_PAYLOAD_BYTES - 6;
const _: () = assert!(READ_REPLY_PREFIX_BYTES == 6);
/// The largest `data_chunk` payload (256 KiB).
pub const MAXIMUM_DATA_CHUNK_BYTES: usize = 256 * 1024;
/// The largest number of entries in one `entry_batch` frame.
pub const MAXIMUM_ENTRY_BATCH_ENTRIES: u16 = 256;
/// The largest number of entries one enumeration may report.
pub const MAXIMUM_ENUMERATED_ENTRIES: u32 = 50_000;
/// The largest archive spelling, in bytes.
pub const MAXIMUM_ENTRY_BATCH_PATH_BYTES: u64 = 4_096;
/// The largest cumulative archive-spelling bytes per enumeration (64 MiB).
pub const MAXIMUM_ENUMERATED_PATH_BYTES: u64 = 64 * 1024 * 1024;
/// The largest single enumerated entry (8 GiB).
pub const MAXIMUM_ENUMERATED_ENTRY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// The largest cumulative enumerated bytes (32 GiB).
pub const MAXIMUM_ENUMERATED_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// The two request phases a `complete` frame may terminate.
///
/// The C++ contract passed a [`ProtocolPhase`] and rejected the other three
/// values at runtime; here the invalid contexts are unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPhase {
    /// A bounded enumeration request.
    Enumerate,
    /// A bounded single-entry stream request.
    Stream,
}

impl From<OperationPhase> for ProtocolPhase {
    fn from(phase: OperationPhase) -> Self {
        match phase {
            OperationPhase::Enumerate => Self::Enumerate,
            OperationPhase::Stream => Self::Stream,
        }
    }
}

impl TryFrom<ProtocolPhase> for OperationPhase {
    type Error = ProtocolError;

    /// # Errors
    /// [`ProtocolError::InvalidBudget`] for handshake, source-read and
    /// complete, which never name a top-level request.
    fn try_from(phase: ProtocolPhase) -> Result<Self, ProtocolError> {
        match phase {
            ProtocolPhase::Enumerate => Ok(Self::Enumerate),
            ProtocolPhase::Stream => Ok(Self::Stream),
            _ => Err(ProtocolError::InvalidBudget),
        }
    }
}

/// The parent-side budget every `read_request`/`read_reply` is checked
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceReadPolicy {
    source_size: u64,
    maximum_read_bytes: u32,
}

impl SourceReadPolicy {
    /// Validates a read budget.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidBudget`] when the source is empty, the read
    /// ceiling is zero, or the ceiling exceeds [`MAXIMUM_READ_BYTES`].
    pub const fn new(source_size: u64, maximum_read_bytes: u32) -> Result<Self, ProtocolError> {
        if source_size == 0 || maximum_read_bytes == 0 || maximum_read_bytes > MAXIMUM_READ_BYTES {
            return Err(ProtocolError::InvalidBudget);
        }
        Ok(Self {
            source_size,
            maximum_read_bytes,
        })
    }

    /// The pinned source size in bytes.
    #[must_use]
    pub const fn source_size(&self) -> u64 {
        self.source_size
    }

    /// The largest single read the worker may request.
    #[must_use]
    pub const fn maximum_read_bytes(&self) -> u32 {
        self.maximum_read_bytes
    }
}

/// The enumeration budget every `entry_batch` is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryBatchPolicy {
    remaining_entries: u32,
    remaining_path_bytes: u64,
    maximum_entry_bytes: u64,
    remaining_total_bytes: u64,
    previous_source_token: Option<u64>,
}

impl EntryBatchPolicy {
    /// Validates an enumeration budget.
    ///
    /// `previous_source_token` is `Some` once an earlier batch was accepted;
    /// the next batch must then start strictly above it.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidBudget`] when any component is zero where the
    /// contract forbids it, or above its ceiling.
    pub const fn new(
        remaining_entries: u32,
        remaining_path_bytes: u64,
        maximum_entry_bytes: u64,
        remaining_total_bytes: u64,
        previous_source_token: Option<u64>,
    ) -> Result<Self, ProtocolError> {
        if remaining_entries == 0
            || remaining_entries > MAXIMUM_ENUMERATED_ENTRIES
            || remaining_path_bytes == 0
            || remaining_path_bytes > MAXIMUM_ENUMERATED_PATH_BYTES
            || maximum_entry_bytes == 0
            || maximum_entry_bytes > MAXIMUM_ENUMERATED_ENTRY_BYTES
            || remaining_total_bytes > MAXIMUM_ENUMERATED_TOTAL_BYTES
        {
            return Err(ProtocolError::InvalidBudget);
        }
        Ok(Self {
            remaining_entries,
            remaining_path_bytes,
            maximum_entry_bytes,
            remaining_total_bytes,
            previous_source_token,
        })
    }

    /// The entries this enumeration may still report.
    #[must_use]
    pub const fn remaining_entries(&self) -> u32 {
        self.remaining_entries
    }

    /// The archive-spelling bytes this enumeration may still report.
    #[must_use]
    pub const fn remaining_path_bytes(&self) -> u64 {
        self.remaining_path_bytes
    }

    /// The largest single entry this enumeration may report.
    #[must_use]
    pub const fn maximum_entry_bytes(&self) -> u64 {
        self.maximum_entry_bytes
    }

    /// The entry bytes this enumeration may still report.
    #[must_use]
    pub const fn remaining_total_bytes(&self) -> u64 {
        self.remaining_total_bytes
    }

    /// The last accepted source token, if any batch was accepted already.
    #[must_use]
    pub const fn previous_source_token(&self) -> Option<u64> {
        self.previous_source_token
    }
}

/// An untrusted, printable-ASCII archive spelling.
///
/// This is *not* a validated destination path and conveys no filesystem
/// authority; only the import layout planner may turn one into a path.
/// `Debug` prints the length, never the spelling.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveSpelling<'frame>(&'frame [u8]);

impl<'frame> ArchiveSpelling<'frame> {
    /// The placeholder held by unwritten [`EntryBatchEntry`] storage. It is
    /// the one spelling [`ArchiveSpelling::new`] would reject, so it can never
    /// be mistaken for decoded media data.
    pub const EMPTY: Self = Self(&[]);

    /// Validates one archive spelling.
    ///
    /// # Errors
    /// [`ProtocolError::NoncanonicalValue`] when empty, longer than
    /// [`MAXIMUM_ENTRY_BATCH_PATH_BYTES`], or containing a byte outside
    /// printable ASCII (`0x20..=0x7e`).
    pub const fn new(bytes: &'frame [u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() || bytes.len() as u64 > MAXIMUM_ENTRY_BATCH_PATH_BYTES {
            return Err(ProtocolError::NoncanonicalValue);
        }
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte < 0x20 || byte > 0x7e {
                return Err(ProtocolError::NoncanonicalValue);
            }
            index += 1;
        }
        Ok(Self(bytes))
    }

    /// The spelling bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &'frame [u8] {
        self.0
    }

    /// The spelling as `str`; printable ASCII is always valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> &'frame str {
        core::str::from_utf8(self.0).unwrap_or("")
    }

    /// The spelling length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether this is the empty placeholder.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for ArchiveSpelling<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the spelling: it is media-derived.
        f.debug_struct("ArchiveSpelling")
            .field("len", &self.0.len())
            .finish()
    }
}

/// `hello`: opens the session and pins the source budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hello {
    /// The pinned source size in bytes.
    pub source_size: u64,
    /// The largest single read the worker may request.
    pub maximum_read_bytes: u32,
}

/// `stream_entry`: begins a bounded single-entry stream request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamEntry {
    /// The opaque token of the entry to stream.
    pub source_token: u64,
}

/// `read_request`: asks the parent for a bounded source read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadRequest {
    /// The read sequence number; must match the expected one exactly.
    pub read_sequence: u32,
    /// The source offset to read from.
    pub offset: u64,
    /// The number of bytes to read.
    pub length: u32,
}

/// `read_reply`: answers exactly one outstanding [`ReadRequest`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReadReply<'frame> {
    /// The sequence number of the request being answered.
    pub read_sequence: u32,
    /// `Ok`, `SourceChanged` or `SourceReadFailed`.
    pub status: ProtocolStatus,
    /// The read bytes, borrowed from the frame; empty unless `status` is
    /// [`ProtocolStatus::Ok`].
    pub data: &'frame [u8],
}

impl fmt::Debug for ReadReply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadReply")
            .field("read_sequence", &self.read_sequence)
            .field("status", &self.status)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// One enumerated entry inside an [`EntryBatch`].
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct EntryBatchEntry<'frame> {
    /// The opaque token identifying this entry.
    pub source_token: u64,
    /// The entry's size in bytes.
    pub size_bytes: u64,
    /// The untrusted archive spelling, borrowed from the frame.
    pub archive_path: ArchiveSpelling<'frame>,
}

impl fmt::Debug for EntryBatchEntry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EntryBatchEntry")
            .field("source_token", &self.source_token)
            .field("size_bytes", &self.size_bytes)
            .field("archive_path", &self.archive_path)
            .finish()
    }
}

/// `entry_batch`: a bounded, strictly increasing batch of entries.
///
/// The entries borrow the caller's storage for `'storage`, and each archive
/// spelling borrows the frame for `'frame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryBatch<'frame, 'storage> {
    /// The entries in this batch.
    pub entries: &'storage [EntryBatchEntry<'frame>],
}

/// `data_chunk`: a bounded chunk of streamed entry bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DataChunk<'frame> {
    /// The chunk bytes, borrowed from the frame.
    pub data: &'frame [u8],
}

impl fmt::Debug for DataChunk<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataChunk")
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// `complete`: the terminal frame of a top-level request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Complete {
    /// Must be [`ProtocolStatus::Ok`] on the wire.
    pub status: ProtocolStatus,
    /// Must be [`ProtocolPhase::Complete`] on the wire.
    pub phase: ProtocolPhase,
}

impl Default for Complete {
    fn default() -> Self {
        Self {
            status: ProtocolStatus::InternalFailure,
            phase: ProtocolPhase::Handshake,
        }
    }
}

fn validate_frame(frame: &FrameView<'_>, expected: MessageType) -> Result<(), ProtocolError> {
    let header = frame.header();
    header.validate()?;
    let payload = frame.payload();
    if payload.len() > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }
    if payload.len() < header.payload_length as usize {
        return Err(ProtocolError::TruncatedPayload);
    }
    if payload.len() > header.payload_length as usize {
        return Err(ProtocolError::TrailingBytes);
    }
    if header.message_type != expected {
        return Err(ProtocolError::UnexpectedMessage);
    }
    Ok(())
}

fn decode_empty(frame: &FrameView<'_>, expected: MessageType) -> Result<(), ProtocolError> {
    validate_frame(frame, expected)?;
    PayloadReader::new(frame.payload()).finish()
}

// ---------------------------------------------------------------- hello ----

/// Encodes a `hello` payload.
///
/// # Errors
/// [`ProtocolError::NoncanonicalValue`] when the message is not a valid
/// [`SourceReadPolicy`], or [`ProtocolError::OutputTooSmall`]. Nothing is
/// written on failure.
pub fn encode_hello_payload(
    message: &Hello,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    SourceReadPolicy::new(message.source_size, message.maximum_read_bytes)
        .map_err(|_| ProtocolError::NoncanonicalValue)?;
    let destination = destination
        .get_mut(..HELLO_PAYLOAD_BYTES)
        .ok_or(ProtocolError::OutputTooSmall)?;
    let mut writer = PayloadWriter::new(destination);
    writer.write_u64(message.source_size)?;
    writer.write_u32(message.maximum_read_bytes)?;
    Ok(HELLO_PAYLOAD_BYTES)
}

/// Decodes a `hello` payload.
///
/// # Errors
/// Any frame error, [`ProtocolError::PayloadUnderflow`],
/// [`ProtocolError::PayloadTrailingBytes`], or
/// [`ProtocolError::NoncanonicalValue`] for an out-of-range budget.
pub fn decode_hello_payload(frame: &FrameView<'_>) -> Result<Hello, ProtocolError> {
    validate_frame(frame, MessageType::Hello)?;
    let mut reader = PayloadReader::new(frame.payload());
    let message = Hello {
        source_size: reader.read_u64()?,
        maximum_read_bytes: reader.read_u32()?,
    };
    reader.finish()?;
    SourceReadPolicy::new(message.source_size, message.maximum_read_bytes)
        .map_err(|_| ProtocolError::NoncanonicalValue)?;
    Ok(message)
}

// --------------------------------------------------- empty-payload kinds ----

/// Encodes a `ready` payload (zero bytes); `destination` is untouched.
#[must_use]
pub const fn encode_ready_payload() -> usize {
    READY_PAYLOAD_BYTES
}

/// Decodes a `ready` payload.
///
/// # Errors
/// Any frame error, or [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_ready_payload(frame: &FrameView<'_>) -> Result<(), ProtocolError> {
    decode_empty(frame, MessageType::Ready)
}

/// Encodes an `enumerate` payload (zero bytes).
#[must_use]
pub const fn encode_enumerate_payload() -> usize {
    ENUMERATE_PAYLOAD_BYTES
}

/// Decodes an `enumerate` payload.
///
/// # Errors
/// Any frame error, or [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_enumerate_payload(frame: &FrameView<'_>) -> Result<(), ProtocolError> {
    decode_empty(frame, MessageType::Enumerate)
}

/// Encodes a `cancel` payload (zero bytes).
#[must_use]
pub const fn encode_cancel_payload() -> usize {
    CANCEL_PAYLOAD_BYTES
}

/// Decodes a `cancel` payload.
///
/// # Errors
/// Any frame error, or [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_cancel_payload(frame: &FrameView<'_>) -> Result<(), ProtocolError> {
    decode_empty(frame, MessageType::Cancel)
}

/// Encodes a `cancel_ack` payload (zero bytes).
#[must_use]
pub const fn encode_cancel_ack_payload() -> usize {
    CANCEL_ACK_PAYLOAD_BYTES
}

/// Decodes a `cancel_ack` payload.
///
/// # Errors
/// Any frame error, or [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_cancel_ack_payload(frame: &FrameView<'_>) -> Result<(), ProtocolError> {
    decode_empty(frame, MessageType::CancelAck)
}

/// Encodes a `shutdown` payload (zero bytes).
#[must_use]
pub const fn encode_shutdown_payload() -> usize {
    SHUTDOWN_PAYLOAD_BYTES
}

/// Decodes a `shutdown` payload.
///
/// # Errors
/// Any frame error, or [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_shutdown_payload(frame: &FrameView<'_>) -> Result<(), ProtocolError> {
    decode_empty(frame, MessageType::Shutdown)
}

// --------------------------------------------------------- stream_entry ----

/// Encodes a `stream_entry` payload.
///
/// # Errors
/// [`ProtocolError::OutputTooSmall`]; nothing is written then.
pub fn encode_stream_entry_payload(
    message: &StreamEntry,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    let destination = destination
        .get_mut(..STREAM_ENTRY_PAYLOAD_BYTES)
        .ok_or(ProtocolError::OutputTooSmall)?;
    PayloadWriter::new(destination).write_u64(message.source_token)?;
    Ok(STREAM_ENTRY_PAYLOAD_BYTES)
}

/// Decodes a `stream_entry` payload. The token is opaque: every 64-bit value
/// is accepted.
///
/// # Errors
/// Any frame error, [`ProtocolError::PayloadUnderflow`] or
/// [`ProtocolError::PayloadTrailingBytes`].
pub fn decode_stream_entry_payload(frame: &FrameView<'_>) -> Result<StreamEntry, ProtocolError> {
    validate_frame(frame, MessageType::StreamEntry)?;
    let mut reader = PayloadReader::new(frame.payload());
    let message = StreamEntry {
        source_token: reader.read_u64()?,
    };
    reader.finish()?;
    Ok(message)
}

// --------------------------------------------------------- read_request ----

fn validate_read_request(
    message: &ReadRequest,
    policy: &SourceReadPolicy,
    expected_sequence: u32,
) -> Result<(), ProtocolError> {
    if expected_sequence == 0 {
        return Err(ProtocolError::InvalidBudget);
    }
    let fits = message
        .offset
        .checked_add_bounded(u64::from(message.length))
        .is_ok_and(|end| end <= policy.source_size());
    if message.read_sequence == 0
        || message.read_sequence != expected_sequence
        || message.length == 0
        || message.length > policy.maximum_read_bytes()
        || message.offset >= policy.source_size()
        || !fits
    {
        return Err(ProtocolError::NoncanonicalValue);
    }
    Ok(())
}

/// Encodes a `read_request` payload.
///
/// # Errors
/// [`ProtocolError::InvalidBudget`] for a zero expected sequence,
/// [`ProtocolError::NoncanonicalValue`] for an out-of-policy read, or
/// [`ProtocolError::OutputTooSmall`]. Nothing is written on failure.
pub fn encode_read_request_payload(
    message: &ReadRequest,
    policy: &SourceReadPolicy,
    expected_sequence: u32,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    validate_read_request(message, policy, expected_sequence)?;
    let destination = destination
        .get_mut(..READ_REQUEST_PAYLOAD_BYTES)
        .ok_or(ProtocolError::OutputTooSmall)?;
    let mut writer = PayloadWriter::new(destination);
    writer.write_u32(message.read_sequence)?;
    writer.write_u64(message.offset)?;
    writer.write_u32(message.length)?;
    Ok(READ_REQUEST_PAYLOAD_BYTES)
}

/// Decodes a `read_request` payload against the parent's read budget.
///
/// # Errors
/// Any frame error, [`ProtocolError::InvalidBudget`],
/// [`ProtocolError::PayloadUnderflow`],
/// [`ProtocolError::PayloadTrailingBytes`] or
/// [`ProtocolError::NoncanonicalValue`].
pub fn decode_read_request_payload(
    frame: &FrameView<'_>,
    policy: &SourceReadPolicy,
    expected_sequence: u32,
) -> Result<ReadRequest, ProtocolError> {
    validate_frame(frame, MessageType::ReadRequest)?;
    if expected_sequence == 0 {
        return Err(ProtocolError::InvalidBudget);
    }
    let mut reader = PayloadReader::new(frame.payload());
    let message = ReadRequest {
        read_sequence: reader.read_u32()?,
        offset: reader.read_u64()?,
        length: reader.read_u32()?,
    };
    reader.finish()?;
    validate_read_request(&message, policy, expected_sequence)?;
    Ok(message)
}

// ----------------------------------------------------------- read_reply ----

const fn reply_status_allowed(status: ProtocolStatus) -> bool {
    matches!(
        status,
        ProtocolStatus::Ok | ProtocolStatus::SourceChanged | ProtocolStatus::SourceReadFailed
    )
}

const fn validate_reply_context(
    expected_sequence: u32,
    requested_length: u32,
) -> Result<(), ProtocolError> {
    if expected_sequence == 0 || requested_length == 0 || requested_length > MAXIMUM_READ_BYTES {
        return Err(ProtocolError::InvalidBudget);
    }
    Ok(())
}

fn validate_read_reply(
    message: &ReadReply<'_>,
    expected_sequence: u32,
    requested_length: u32,
) -> Result<(), ProtocolError> {
    validate_reply_context(expected_sequence, requested_length)?;
    if message.read_sequence == 0
        || message.read_sequence != expected_sequence
        || !reply_status_allowed(message.status)
    {
        return Err(ProtocolError::NoncanonicalValue);
    }
    let shape_ok = if message.status == ProtocolStatus::Ok {
        message.data.len() == requested_length as usize
    } else {
        message.data.is_empty()
    };
    if shape_ok {
        Ok(())
    } else {
        Err(ProtocolError::NoncanonicalValue)
    }
}

/// Encodes a `read_reply` payload.
///
/// # Errors
/// [`ProtocolError::PayloadTooLarge`] above the frame ceiling,
/// [`ProtocolError::InvalidBudget`], [`ProtocolError::NoncanonicalValue`] for
/// a disallowed status or wrong data length, or
/// [`ProtocolError::OutputTooSmall`]. Nothing is written on failure.
pub fn encode_read_reply_payload(
    message: &ReadReply<'_>,
    expected_sequence: u32,
    requested_length: u32,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    let payload_size = message
        .data
        .len()
        .checked_add_bounded(READ_REPLY_PREFIX_BYTES)
        .map_err(|_| ProtocolError::PayloadTooLarge)?;
    if payload_size > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }
    validate_read_reply(message, expected_sequence, requested_length)?;
    let destination = destination
        .get_mut(..payload_size)
        .ok_or(ProtocolError::OutputTooSmall)?;
    let mut writer = PayloadWriter::new(destination);
    writer.write_u32(message.read_sequence)?;
    writer.write_status(message.status)?;
    writer.write_bytes(message.data)?;
    Ok(payload_size)
}

/// Decodes a `read_reply` payload, borrowing its data from the frame.
///
/// # Errors
/// Any frame error, [`ProtocolError::InvalidBudget`],
/// [`ProtocolError::PayloadUnderflow`] or
/// [`ProtocolError::NoncanonicalValue`].
pub fn decode_read_reply_payload<'frame>(
    frame: &FrameView<'frame>,
    expected_sequence: u32,
    requested_length: u32,
) -> Result<ReadReply<'frame>, ProtocolError> {
    validate_frame(frame, MessageType::ReadReply)?;
    validate_reply_context(expected_sequence, requested_length)?;
    let mut reader = PayloadReader::new(frame.payload());
    let read_sequence = reader.read_u32()?;
    let status = reader.read_status()?;
    let data = reader.read_bytes(reader.remaining())?;
    reader.finish()?;
    let message = ReadReply {
        read_sequence,
        status,
        data,
    };
    validate_read_reply(&message, expected_sequence, requested_length)?;
    Ok(message)
}

// ---------------------------------------------------------- entry_batch ----

#[derive(Default)]
struct EntryBatchAccumulator {
    path_bytes: u64,
    total_bytes: u64,
    previous_source_token: Option<u64>,
}

impl EntryBatchAccumulator {
    fn start(policy: &EntryBatchPolicy) -> Self {
        Self {
            path_bytes: 0,
            total_bytes: 0,
            previous_source_token: policy.previous_source_token(),
        }
    }

    fn accept(
        &mut self,
        source_token: u64,
        size_bytes: u64,
        path: &[u8],
        policy: &EntryBatchPolicy,
    ) -> Result<(), ProtocolError> {
        ArchiveSpelling::new(path)?;
        let path_bytes = path.len() as u64;
        let path_budget = policy
            .remaining_path_bytes()
            .checked_sub_bounded(self.path_bytes)
            .map_err(|_| ProtocolError::NoncanonicalValue)?;
        let total_budget = policy
            .remaining_total_bytes()
            .checked_sub_bounded(self.total_bytes)
            .map_err(|_| ProtocolError::NoncanonicalValue)?;
        if size_bytes > policy.maximum_entry_bytes()
            || path_bytes > path_budget
            || size_bytes > total_budget
            || self
                .previous_source_token
                .is_some_and(|previous| source_token <= previous)
        {
            return Err(ProtocolError::NoncanonicalValue);
        }
        self.path_bytes += path_bytes;
        self.total_bytes += size_bytes;
        self.previous_source_token = Some(source_token);
        Ok(())
    }
}

fn entry_batch_payload_size(entries: &[EntryBatchEntry<'_>]) -> Result<usize, ProtocolError> {
    let mut payload_size = ENTRY_BATCH_PREFIX_BYTES;
    for entry in entries {
        payload_size = payload_size
            .checked_add_bounded(ENTRY_BATCH_ENTRY_PREFIX_BYTES + entry.archive_path.len())
            .map_err(|_| ProtocolError::PayloadTooLarge)?;
    }
    Ok(payload_size)
}

/// Encodes an `entry_batch` payload.
///
/// # Errors
/// [`ProtocolError::NoncanonicalValue`] for an empty, oversized, out-of-order
/// or out-of-budget batch, [`ProtocolError::PayloadTooLarge`] above the frame
/// ceiling, or [`ProtocolError::OutputTooSmall`]. Nothing is written on
/// failure.
pub fn encode_entry_batch_payload(
    message: &EntryBatch<'_, '_>,
    policy: &EntryBatchPolicy,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    let entries = message.entries;
    if entries.is_empty()
        || entries.len() > MAXIMUM_ENTRY_BATCH_ENTRIES as usize
        || entries.len() > policy.remaining_entries() as usize
    {
        return Err(ProtocolError::NoncanonicalValue);
    }
    let mut accumulator = EntryBatchAccumulator::start(policy);
    for entry in entries {
        accumulator.accept(
            entry.source_token,
            entry.size_bytes,
            entry.archive_path.as_bytes(),
            policy,
        )?;
    }
    let payload_size = entry_batch_payload_size(entries)?;
    if payload_size > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }
    let destination = destination
        .get_mut(..payload_size)
        .ok_or(ProtocolError::OutputTooSmall)?;

    let mut writer = PayloadWriter::new(destination);
    let entry_count = u16::try_from(entries.len()).map_err(|_| ProtocolError::NoncanonicalValue)?;
    writer.write_u16(entry_count)?;
    for entry in entries {
        writer.write_u64(entry.source_token)?;
        writer.write_u64(entry.size_bytes)?;
        let path_size = u16::try_from(entry.archive_path.len())
            .map_err(|_| ProtocolError::NoncanonicalValue)?;
        writer.write_u16(path_size)?;
        writer.write_bytes(entry.archive_path.as_bytes())?;
    }
    Ok(payload_size)
}

fn validate_entry_batch_payload(
    payload: &[u8],
    policy: &EntryBatchPolicy,
) -> Result<u16, ProtocolError> {
    let mut reader = PayloadReader::new(payload);
    let entry_count = reader.read_u16()?;
    if entry_count == 0
        || entry_count > MAXIMUM_ENTRY_BATCH_ENTRIES
        || u32::from(entry_count) > policy.remaining_entries()
    {
        return Err(ProtocolError::NoncanonicalValue);
    }
    let mut accumulator = EntryBatchAccumulator::start(policy);
    for _ in 0..entry_count {
        let source_token = reader.read_u64()?;
        let size_bytes = reader.read_u64()?;
        let path_size = reader.read_u16()?;
        let path = reader.read_bytes(path_size as usize)?;
        accumulator.accept(source_token, size_bytes, path, policy)?;
    }
    reader.finish()?;
    Ok(entry_count)
}

/// Decodes an `entry_batch` payload into caller-supplied entry storage.
///
/// The whole payload is validated before a single entry is written, so a
/// rejected batch leaves `storage` untouched. Each decoded archive spelling
/// borrows the frame; the entries borrow `storage`.
///
/// # Errors
/// Any frame error, [`ProtocolError::NoncanonicalValue`],
/// [`ProtocolError::PayloadUnderflow`],
/// [`ProtocolError::PayloadTrailingBytes`] or
/// [`ProtocolError::OutputTooSmall`] when `storage` is shorter than the batch.
pub fn decode_entry_batch_payload<'frame, 'storage>(
    frame: &FrameView<'frame>,
    policy: &EntryBatchPolicy,
    storage: &'storage mut [EntryBatchEntry<'frame>],
) -> Result<EntryBatch<'frame, 'storage>, ProtocolError> {
    validate_frame(frame, MessageType::EntryBatch)?;
    let entry_count = validate_entry_batch_payload(frame.payload(), policy)?;
    if storage.len() < entry_count as usize {
        return Err(ProtocolError::OutputTooSmall);
    }

    // The first pass proved every read below succeeds and every value is in
    // policy. Populate caller storage only after that and the capacity check.
    let mut reader = PayloadReader::new(frame.payload());
    reader.read_u16()?;
    for slot in storage.iter_mut().take(entry_count as usize) {
        let source_token = reader.read_u64()?;
        let size_bytes = reader.read_u64()?;
        let path_size = reader.read_u16()?;
        let path = reader.read_bytes(path_size as usize)?;
        *slot = EntryBatchEntry {
            source_token,
            size_bytes,
            archive_path: ArchiveSpelling::new(path)?,
        };
    }
    Ok(EntryBatch {
        entries: &storage[..entry_count as usize],
    })
}

// ----------------------------------------------------------- data_chunk ----

fn validate_data_chunk(
    message: &DataChunk<'_>,
    remaining_entry_bytes: u64,
) -> Result<(), ProtocolError> {
    if remaining_entry_bytes == 0 {
        return Err(ProtocolError::InvalidBudget);
    }
    if message.data.is_empty()
        || message.data.len() > MAXIMUM_DATA_CHUNK_BYTES
        || message.data.len() as u64 > remaining_entry_bytes
    {
        return Err(ProtocolError::NoncanonicalValue);
    }
    Ok(())
}

/// Encodes a `data_chunk` payload.
///
/// # Errors
/// [`ProtocolError::InvalidBudget`] for a zero remainder,
/// [`ProtocolError::NoncanonicalValue`] for an empty or oversized chunk, or
/// [`ProtocolError::OutputTooSmall`]. Nothing is written on failure.
pub fn encode_data_chunk_payload(
    message: &DataChunk<'_>,
    remaining_entry_bytes: u64,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    validate_data_chunk(message, remaining_entry_bytes)?;
    let destination = destination
        .get_mut(..message.data.len())
        .ok_or(ProtocolError::OutputTooSmall)?;
    PayloadWriter::new(destination).write_bytes(message.data)?;
    Ok(message.data.len())
}

/// Decodes a `data_chunk` payload, borrowing its bytes from the frame.
///
/// # Errors
/// Any frame error, [`ProtocolError::InvalidBudget`] or
/// [`ProtocolError::NoncanonicalValue`].
pub fn decode_data_chunk_payload<'frame>(
    frame: &FrameView<'frame>,
    remaining_entry_bytes: u64,
) -> Result<DataChunk<'frame>, ProtocolError> {
    validate_frame(frame, MessageType::DataChunk)?;
    let mut reader = PayloadReader::new(frame.payload());
    let data = reader.read_bytes(reader.remaining())?;
    reader.finish()?;
    let message = DataChunk { data };
    validate_data_chunk(&message, remaining_entry_bytes)?;
    Ok(message)
}

// ------------------------------------------------------------- complete ----

fn validate_complete(message: Complete) -> Result<(), ProtocolError> {
    if message.status != ProtocolStatus::Ok || message.phase != ProtocolPhase::Complete {
        return Err(ProtocolError::NoncanonicalValue);
    }
    Ok(())
}

/// Encodes a `complete` payload.
///
/// `expected_operation_phase` names the request being completed; it is part
/// of the caller's context, not of the wire image.
///
/// # Errors
/// [`ProtocolError::NoncanonicalValue`] unless the message is exactly
/// `(Ok, Complete)`, or [`ProtocolError::OutputTooSmall`]. Nothing is written
/// on failure.
pub fn encode_complete_payload(
    message: &Complete,
    expected_operation_phase: OperationPhase,
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    let _ = expected_operation_phase;
    validate_complete(*message)?;
    let destination = destination
        .get_mut(..COMPLETE_PAYLOAD_BYTES)
        .ok_or(ProtocolError::OutputTooSmall)?;
    let mut writer = PayloadWriter::new(destination);
    writer.write_status(message.status)?;
    writer.write_phase(message.phase)?;
    Ok(COMPLETE_PAYLOAD_BYTES)
}

/// Decodes a `complete` payload.
///
/// # Errors
/// Any frame error, [`ProtocolError::PayloadUnderflow`],
/// [`ProtocolError::PayloadTrailingBytes`] or
/// [`ProtocolError::NoncanonicalValue`] for any status/phase pair other than
/// `(Ok, Complete)`.
pub fn decode_complete_payload(
    frame: &FrameView<'_>,
    expected_operation_phase: OperationPhase,
) -> Result<Complete, ProtocolError> {
    let _ = expected_operation_phase;
    validate_frame(frame, MessageType::Complete)?;
    let mut reader = PayloadReader::new(frame.payload());
    let message = Complete {
        status: reader.read_status()?,
        phase: reader.read_phase()?,
    };
    reader.finish()?;
    validate_complete(message)?;
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::{
        ArchiveSpelling, DataChunk, EntryBatchEntry, EntryBatchPolicy, OperationPhase, ReadReply,
        SourceReadPolicy,
    };
    use crate::{ProtocolError, ProtocolPhase, ProtocolStatus};

    #[test]
    fn debug_never_prints_payload_or_spelling_bytes() {
        let reply = ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &[0xde, 0xad],
        };
        let rendered = format!("{reply:?}");
        assert!(rendered.contains("data_len: 2"));
        assert!(!rendered.contains("222"));

        let chunk = DataChunk { data: &[0xbe] };
        assert!(format!("{chunk:?}").contains("data_len: 1"));

        let entry = EntryBatchEntry {
            source_token: 1,
            size_bytes: 2,
            archive_path: ArchiveSpelling::new(b"secret.txt").expect("printable"),
        };
        let rendered = format!("{entry:?}");
        assert!(rendered.contains("len: 10"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn archive_spellings_reject_non_printable_and_out_of_range_lengths() {
        assert!(ArchiveSpelling::new(b" ").is_ok());
        assert!(ArchiveSpelling::new(b"~").is_ok());
        assert_eq!(
            ArchiveSpelling::new(b""),
            Err(ProtocolError::NoncanonicalValue)
        );
        for byte in [0x1f_u8, 0x7f, 0x80, 0xff] {
            assert_eq!(
                ArchiveSpelling::new(&[byte]),
                Err(ProtocolError::NoncanonicalValue)
            );
        }
    }

    #[test]
    fn operation_phase_maps_both_ways() {
        assert_eq!(
            ProtocolPhase::from(OperationPhase::Enumerate),
            ProtocolPhase::Enumerate
        );
        assert_eq!(
            OperationPhase::try_from(ProtocolPhase::Stream),
            Ok(OperationPhase::Stream)
        );
        for phase in [
            ProtocolPhase::Handshake,
            ProtocolPhase::SourceRead,
            ProtocolPhase::Complete,
        ] {
            assert_eq!(
                OperationPhase::try_from(phase),
                Err(ProtocolError::InvalidBudget)
            );
        }
    }

    #[test]
    fn policies_reject_out_of_range_components() {
        assert!(SourceReadPolicy::new(1, 1).is_ok());
        assert_eq!(
            SourceReadPolicy::new(0, 1),
            Err(ProtocolError::InvalidBudget)
        );
        assert_eq!(
            EntryBatchPolicy::new(0, 1, 1, 0, None),
            Err(ProtocolError::InvalidBudget)
        );
        assert!(EntryBatchPolicy::new(1, 1, 1, 0, None).is_ok());
    }
}
