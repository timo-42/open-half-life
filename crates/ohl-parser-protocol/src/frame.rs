//! Fixed frame header encoding and bounded frame decoding.
//!
//! The 32-byte header is mapped with [`zerocopy`] so the byte layout is
//! declared once, checked at compile time, and parsed without `unsafe` and
//! without copying anything but the header itself.

use core::fmt;

use zerocopy::byteorder::little_endian::{U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::{
    FRAME_HEADER_BYTES, FRAME_MAGIC, MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType,
    PROTOCOL_MAJOR_VERSION, PROTOCOL_MINOR_VERSION, ProtocolError,
};

/// The wire image of the fixed header. Little-endian, packed, unaligned.
#[derive(FromBytes, IntoBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawFrameHeader {
    magic: [u8; 4],
    major_version: U16,
    minor_version: U16,
    message_type: U16,
    flags: U16,
    payload_length: U32,
    session_id: U64,
    request_id: U64,
}

const _: () = assert!(size_of::<RawFrameHeader>() == FRAME_HEADER_BYTES);

/// A decoded, still-untrusted frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Major protocol version; only [`PROTOCOL_MAJOR_VERSION`] is accepted.
    pub major_version: u16,
    /// Minor protocol version; only [`PROTOCOL_MINOR_VERSION`] is accepted.
    pub minor_version: u16,
    /// The message kind.
    pub message_type: MessageType,
    /// Reserved; must be zero.
    pub flags: u16,
    /// Declared payload length in bytes.
    pub payload_length: u32,
    /// Session id; must be non-zero.
    pub session_id: u64,
    /// Request id; zero exactly for hello/ready/shutdown.
    pub request_id: u64,
}

impl FrameHeader {
    /// Builds a header for `message_type` at the current protocol version.
    #[must_use]
    pub const fn new(
        message_type: MessageType,
        session_id: u64,
        request_id: u64,
        payload_length: u32,
    ) -> Self {
        Self {
            major_version: PROTOCOL_MAJOR_VERSION,
            minor_version: PROTOCOL_MINOR_VERSION,
            message_type,
            flags: 0,
            payload_length,
            session_id,
            request_id,
        }
    }

    /// Validates the complete fixed header, including the payload ceiling,
    /// before a transport allocates or reads payload storage.
    ///
    /// # Errors
    /// The first failing rule, in the order version, flags, payload ceiling,
    /// session id, request id.
    pub const fn validate(&self) -> Result<(), ProtocolError> {
        if self.major_version != PROTOCOL_MAJOR_VERSION
            || self.minor_version != PROTOCOL_MINOR_VERSION
        {
            return Err(ProtocolError::UnsupportedVersion);
        }
        if self.flags != 0 {
            return Err(ProtocolError::ReservedFlags);
        }
        if self.payload_length > MAXIMUM_FRAME_PAYLOAD_BYTES {
            return Err(ProtocolError::PayloadTooLarge);
        }
        if self.session_id == 0 {
            return Err(ProtocolError::InvalidSessionId);
        }
        let zero_request = self.message_type.requires_zero_request_id();
        if (zero_request && self.request_id != 0) || (!zero_request && self.request_id == 0) {
            return Err(ProtocolError::InvalidRequestId);
        }
        Ok(())
    }

    /// Encodes this header into exactly [`FRAME_HEADER_BYTES`] bytes.
    ///
    /// # Errors
    /// Any error from [`FrameHeader::validate`]; nothing is written then.
    pub fn encode(&self, destination: &mut [u8; FRAME_HEADER_BYTES]) -> Result<(), ProtocolError> {
        self.validate()?;
        let raw = RawFrameHeader {
            magic: FRAME_MAGIC,
            major_version: U16::new(self.major_version),
            minor_version: U16::new(self.minor_version),
            message_type: U16::new(self.message_type.to_wire()),
            flags: U16::new(self.flags),
            payload_length: U32::new(self.payload_length),
            session_id: U64::new(self.session_id),
            request_id: U64::new(self.request_id),
        };
        destination.copy_from_slice(raw.as_bytes());
        Ok(())
    }
}

/// Encodes a header into `destination`.
///
/// # Errors
/// Any error from [`FrameHeader::validate`].
pub fn encode_frame_header(
    header: &FrameHeader,
    destination: &mut [u8; FRAME_HEADER_BYTES],
) -> Result<(), ProtocolError> {
    header.encode(destination)
}

/// Decodes and fully validates a fixed header.
///
/// # Errors
/// [`ProtocolError::InvalidMagic`], [`ProtocolError::UnsupportedVersion`],
/// [`ProtocolError::UnknownMessageType`] or any error from
/// [`FrameHeader::validate`].
pub fn decode_frame_header(bytes: &[u8; FRAME_HEADER_BYTES]) -> Result<FrameHeader, ProtocolError> {
    let raw = RawFrameHeader::ref_from_bytes(bytes).map_err(|_| ProtocolError::TruncatedHeader)?;
    if raw.magic != FRAME_MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }
    let major_version = raw.major_version.get();
    let minor_version = raw.minor_version.get();
    // The version gate precedes the message-type table so a future version's
    // frames report the version, not an unknown type.
    if major_version != PROTOCOL_MAJOR_VERSION || minor_version != PROTOCOL_MINOR_VERSION {
        return Err(ProtocolError::UnsupportedVersion);
    }
    let header = FrameHeader {
        major_version,
        minor_version,
        message_type: MessageType::from_wire(raw.message_type.get())?,
        flags: raw.flags.get(),
        payload_length: raw.payload_length.get(),
        session_id: raw.session_id.get(),
        request_id: raw.request_id.get(),
    };
    header.validate()?;
    Ok(header)
}

/// A validated header plus a borrowed view of its payload bytes.
///
/// The payload borrows the transport's frame storage for `'frame`; nothing is
/// copied. `Debug` deliberately reports only the payload length.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrameView<'frame> {
    header: FrameHeader,
    payload: &'frame [u8],
}

impl<'frame> FrameView<'frame> {
    /// Pairs a header with payload bytes.
    ///
    /// No consistency between `header.payload_length` and `payload.len()` is
    /// assumed here: every typed decoder revalidates it, so a transport that
    /// assembles frames itself cannot bypass the length checks.
    #[must_use]
    pub const fn new(header: FrameHeader, payload: &'frame [u8]) -> Self {
        Self { header, payload }
    }

    /// The validated header.
    #[must_use]
    pub const fn header(&self) -> &FrameHeader {
        &self.header
    }

    /// The borrowed payload bytes.
    #[must_use]
    pub const fn payload(&self) -> &'frame [u8] {
        self.payload
    }
}

impl fmt::Debug for FrameView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print payload bytes: they are media-derived.
        f.debug_struct("FrameView")
            .field("header", &self.header)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// Encodes `header` and `payload` into `destination`, returning the number of
/// bytes written.
///
/// # Errors
/// [`ProtocolError::PayloadTooLarge`] above the frame ceiling,
/// [`ProtocolError::NoncanonicalValue`] when `header.payload_length` disagrees
/// with `payload.len()`, [`ProtocolError::OutputTooSmall`], or any error from
/// [`FrameHeader::validate`]. Nothing is written on failure.
pub fn encode_frame(
    header: &FrameHeader,
    payload: &[u8],
    destination: &mut [u8],
) -> Result<usize, ProtocolError> {
    if payload.len() > MAXIMUM_FRAME_PAYLOAD_BYTES as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }
    if header.payload_length as usize != payload.len() {
        return Err(ProtocolError::NoncanonicalValue);
    }
    header.validate()?;
    let frame_size = FRAME_HEADER_BYTES + payload.len();
    if destination.len() < frame_size {
        return Err(ProtocolError::OutputTooSmall);
    }
    let (header_bytes, rest) = destination.split_at_mut(FRAME_HEADER_BYTES);
    let header_bytes: &mut [u8; FRAME_HEADER_BYTES] = header_bytes
        .try_into()
        .map_err(|_| ProtocolError::OutputTooSmall)?;
    header.encode(header_bytes)?;
    rest[..payload.len()].copy_from_slice(payload);
    Ok(frame_size)
}

/// Decodes exactly one frame out of `bytes`.
///
/// `bytes` must hold the frame and nothing else: trailing bytes are rejected,
/// so a transport cannot smuggle a second frame past the session validator.
/// A non-zero `expected_session_id` additionally pins the session.
///
/// # Errors
/// [`ProtocolError::TruncatedHeader`], any header error,
/// [`ProtocolError::WrongSessionId`], [`ProtocolError::TruncatedPayload`] or
/// [`ProtocolError::TrailingBytes`].
pub fn decode_frame(
    bytes: &[u8],
    expected_session_id: u64,
) -> Result<FrameView<'_>, ProtocolError> {
    let header_bytes: &[u8; FRAME_HEADER_BYTES] = bytes
        .get(..FRAME_HEADER_BYTES)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ProtocolError::TruncatedHeader)?;
    let header = decode_frame_header(header_bytes)?;
    if expected_session_id != 0 && header.session_id != expected_session_id {
        return Err(ProtocolError::WrongSessionId);
    }
    let frame_size = FRAME_HEADER_BYTES + header.payload_length as usize;
    if bytes.len() < frame_size {
        return Err(ProtocolError::TruncatedPayload);
    }
    if bytes.len() != frame_size {
        return Err(ProtocolError::TrailingBytes);
    }
    Ok(FrameView::new(
        header,
        &bytes[FRAME_HEADER_BYTES..frame_size],
    ))
}

#[cfg(test)]
mod tests {
    use super::{FrameHeader, decode_frame, decode_frame_header, encode_frame};
    use crate::{FRAME_HEADER_BYTES, MessageType, ProtocolError};

    #[test]
    fn debug_of_a_frame_view_never_prints_payload_bytes() {
        let mut encoded = [0_u8; FRAME_HEADER_BYTES + 3];
        let header = FrameHeader::new(MessageType::DataChunk, 9, 1, 3);
        let payload = [0xde_u8, 0xad, 0xbe];
        encode_frame(&header, &payload, &mut encoded).expect("valid frame");
        let view = decode_frame(&encoded, 9).expect("valid frame");
        let rendered = format!("{view:?}");
        assert!(rendered.contains("payload_len: 3"));
        assert!(!rendered.contains("222"));
        assert!(!rendered.contains("173"));
    }

    #[test]
    fn header_decoding_reports_a_short_buffer() {
        assert_eq!(
            decode_frame(&[0_u8; FRAME_HEADER_BYTES - 1], 0),
            Err(ProtocolError::TruncatedHeader)
        );
    }

    #[test]
    fn round_trip_preserves_every_header_field() {
        let header = FrameHeader::new(MessageType::EntryBatch, u64::MAX, 42, 0);
        let mut bytes = [0_u8; FRAME_HEADER_BYTES];
        header.encode(&mut bytes).expect("valid header");
        assert_eq!(decode_frame_header(&bytes), Ok(header));
    }
}
