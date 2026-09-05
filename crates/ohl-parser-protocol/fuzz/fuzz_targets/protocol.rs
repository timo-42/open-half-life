//! Port of `tests/parser/protocol_fuzz.cpp`.
//!
//! Three harnesses share one input: a fixed-record transcript, a
//! length-prefixed frame transcript, and a single whole-buffer frame decode.
//! Every accepted frame is handed to its typed decoder with a derived,
//! adversarial context and then to a session validator, so decoding and
//! ordering are fuzzed together.

#![no_main]

use libfuzzer_sys::fuzz_target;

use ohl_parser_protocol::messages::{
    EntryBatchEntry, EntryBatchPolicy, MAXIMUM_ENTRY_BATCH_ENTRIES, MAXIMUM_ENUMERATED_ENTRIES,
    MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES, MAXIMUM_ENUMERATED_TOTAL_BYTES,
    OperationPhase, READ_REPLY_PREFIX_BYTES, SourceReadPolicy, decode_cancel_ack_payload,
    decode_cancel_payload, decode_complete_payload, decode_data_chunk_payload,
    decode_entry_batch_payload, decode_enumerate_payload, decode_hello_payload,
    decode_read_reply_payload, decode_read_request_payload, decode_ready_payload,
    decode_shutdown_payload, decode_stream_entry_payload,
};
use ohl_parser_protocol::{
    Direction, FRAME_HEADER_BYTES, FrameHeader, FrameView, MessageType, PayloadReader,
    ProtocolBudgets, ProtocolStatus, SessionId, SessionValidator, decode_frame, encode_frame,
};

const FUZZ_SESSION: u64 = 1;
const RECORD_BYTES: usize = 4;
const MAXIMUM_TRANSCRIPT_RECORDS: usize = 64;
const FRAMED_RECORD_HEADER_BYTES: usize = 3;
const MAXIMUM_FUZZ_READ_BYTES: u32 = 4_096;
const MAXIMUM_FUZZ_SOURCE_BYTES: u32 = 65_536;
const MAXIMUM_FUZZ_MISMATCH_SEQUENCE: u32 = 64;

fn load_u16(input: &[u8], offset: usize) -> u16 {
    input
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u16::from_le_bytes)
}

fn load_u32(input: &[u8], offset: usize) -> u32 {
    input
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

fn load_u64(input: &[u8], offset: usize) -> u64 {
    input
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

/// A bounded, non-zero context value derived from the payload.
fn bounded(input: &[u8], offset: usize, maximum: u32) -> u32 {
    let mut value: u32 = 0;
    let available = input.len().saturating_sub(offset);
    for index in 0..available.min(4) {
        value |= u32::from(input[offset + index]) << (index * 8);
    }
    (value % maximum) + 1
}

struct SequenceContext {
    expected_sequence: u32,
    matches_wire: bool,
}

fn sequence_context(frame: &FrameView<'_>) -> SequenceContext {
    let wire_sequence = load_u32(frame.payload(), 0);
    if wire_sequence != 0 && frame.header().request_id & 1 != 0 {
        return SequenceContext {
            expected_sequence: wire_sequence,
            matches_wire: true,
        };
    }
    let mut mismatch = bounded(frame.payload(), 4, MAXIMUM_FUZZ_MISMATCH_SEQUENCE);
    if mismatch == wire_sequence {
        mismatch = if mismatch == MAXIMUM_FUZZ_MISMATCH_SEQUENCE {
            1
        } else {
            mismatch + 1
        };
    }
    SequenceContext {
        expected_sequence: mismatch,
        matches_wire: false,
    }
}

fn request_policy(frame: &FrameView<'_>, matching_sequence: bool) -> SourceReadPolicy {
    let fallback = SourceReadPolicy::new(
        u64::from(bounded(frame.payload(), 4, MAXIMUM_FUZZ_SOURCE_BYTES)),
        bounded(frame.payload(), 12, MAXIMUM_FUZZ_READ_BYTES),
    )
    .expect("bounded context is in policy");
    if !matching_sequence {
        return fallback;
    }
    let wire_offset = load_u64(frame.payload(), 4);
    let wire_length = load_u32(frame.payload(), 12);
    if wire_length == 0
        || wire_length > MAXIMUM_FUZZ_READ_BYTES
        || wire_offset >= u64::from(MAXIMUM_FUZZ_SOURCE_BYTES)
        || u64::from(wire_length) > u64::from(MAXIMUM_FUZZ_SOURCE_BYTES) - wire_offset
    {
        return fallback;
    }
    SourceReadPolicy::new(wire_offset + u64::from(wire_length), wire_length).unwrap_or(fallback)
}

fn reply_requested_length(frame: &FrameView<'_>, matching_sequence: bool) -> u32 {
    let payload = frame.payload();
    if matching_sequence && payload.len() >= READ_REPLY_PREFIX_BYTES {
        let status = ProtocolStatus::from_wire(load_u16(payload, 4));
        let data_size = payload.len() - READ_REPLY_PREFIX_BYTES;
        if status == Ok(ProtocolStatus::Ok)
            && data_size != 0
            && data_size <= MAXIMUM_FUZZ_READ_BYTES as usize
        {
            return u32::try_from(data_size).unwrap_or(1);
        }
        if matches!(
            status,
            Ok(ProtocolStatus::SourceChanged) | Ok(ProtocolStatus::SourceReadFailed)
        ) && data_size == 0
        {
            return 1;
        }
    }
    bounded(payload, READ_REPLY_PREFIX_BYTES, MAXIMUM_FUZZ_READ_BYTES)
}

fn data_chunk_remainder(frame: &FrameView<'_>) -> u64 {
    let payload_size = frame.payload().len() as u64;
    match frame.header().request_id % 3 {
        0 => {
            if payload_size == 0 {
                1
            } else {
                payload_size
            }
        }
        1 => payload_size.saturating_sub(1),
        _ => 0,
    }
}

fn complete_context(frame: &FrameView<'_>) -> OperationPhase {
    if frame.header().request_id % 2 == 0 {
        OperationPhase::Enumerate
    } else {
        OperationPhase::Stream
    }
}

fn entry_batch_policy(frame: &FrameView<'_>) -> EntryBatchPolicy {
    let count = load_u16(frame.payload(), 0);
    let first_token = load_u64(frame.payload(), 2);
    let maximum = |previous| {
        EntryBatchPolicy::new(
            MAXIMUM_ENUMERATED_ENTRIES,
            MAXIMUM_ENUMERATED_PATH_BYTES,
            MAXIMUM_ENUMERATED_ENTRY_BYTES,
            MAXIMUM_ENUMERATED_TOTAL_BYTES,
            previous,
        )
        .expect("maximum policy is valid")
    };
    match frame.header().request_id % 4 {
        0 => maximum(None),
        1 => {
            if count != 0 && frame.payload().len() >= 10 && first_token != 0 {
                maximum(Some(first_token - 1))
            } else {
                maximum(None)
            }
        }
        2 => maximum(Some(first_token)),
        _ => {
            let remaining = if count > 1 && count <= MAXIMUM_ENTRY_BATCH_ENTRIES {
                u32::from(count) - 1
            } else {
                1
            };
            EntryBatchPolicy::new(
                remaining,
                MAXIMUM_ENUMERATED_PATH_BYTES,
                MAXIMUM_ENUMERATED_ENTRY_BYTES,
                MAXIMUM_ENUMERATED_TOTAL_BYTES,
                None,
            )
            .expect("bounded policy is valid")
        }
    }
}

fn exercise_typed_decoder(frame: &FrameView<'_>) {
    match frame.header().message_type {
        MessageType::Hello => {
            let _ = decode_hello_payload(frame);
        }
        MessageType::Ready => {
            let _ = decode_ready_payload(frame);
        }
        MessageType::Enumerate => {
            let _ = decode_enumerate_payload(frame);
        }
        MessageType::StreamEntry => {
            let _ = decode_stream_entry_payload(frame);
        }
        MessageType::ReadRequest => {
            let sequence = sequence_context(frame);
            let policy = request_policy(frame, sequence.matches_wire);
            let _ = decode_read_request_payload(frame, &policy, sequence.expected_sequence);
        }
        MessageType::ReadReply => {
            let sequence = sequence_context(frame);
            let requested_length = reply_requested_length(frame, sequence.matches_wire);
            let _ = decode_read_reply_payload(frame, sequence.expected_sequence, requested_length);
        }
        MessageType::EntryBatch => {
            let mut storage = [EntryBatchEntry::default(); MAXIMUM_ENTRY_BATCH_ENTRIES as usize];
            let _ = decode_entry_batch_payload(frame, &entry_batch_policy(frame), &mut storage);
        }
        MessageType::DataChunk => {
            let _ = decode_data_chunk_payload(frame, data_chunk_remainder(frame));
        }
        MessageType::Complete => {
            let _ = decode_complete_payload(frame, complete_context(frame));
        }
        MessageType::Cancel => {
            let _ = decode_cancel_payload(frame);
        }
        MessageType::CancelAck => {
            let _ = decode_cancel_ack_payload(frame);
        }
        MessageType::Shutdown => {
            let _ = decode_shutdown_payload(frame);
        }
    }
}

fn message_type(symbol: u8) -> Option<MessageType> {
    Some(match symbol {
        b'H' => MessageType::Hello,
        b'R' => MessageType::Ready,
        b'E' => MessageType::Enumerate,
        b'S' => MessageType::StreamEntry,
        b'Q' => MessageType::ReadRequest,
        b'Y' => MessageType::ReadReply,
        b'B' => MessageType::EntryBatch,
        b'D' => MessageType::DataChunk,
        b'C' => MessageType::Complete,
        b'X' => MessageType::Cancel,
        b'A' => MessageType::CancelAck,
        b'Z' => MessageType::Shutdown,
        other => return MessageType::from_wire(u16::from(other)).ok(),
    })
}

/// Fixed four-byte records: direction, type symbol, request id, payload size.
fn exercise_seeded_transcript(input: &[u8]) {
    let mut validator = SessionValidator::new(
        SessionId::new(FUZZ_SESSION).expect("non-zero"),
        ProtocolBudgets::new(
            MAXIMUM_TRANSCRIPT_RECORDS as u64,
            MAXIMUM_TRANSCRIPT_RECORDS as u64 * 15,
        )
        .expect("valid"),
    );
    let records = (input.len() / RECORD_BYTES).min(MAXIMUM_TRANSCRIPT_RECORDS);
    for index in 0..records {
        let record = &input[index * RECORD_BYTES..index * RECORD_BYTES + RECORD_BYTES];
        let direction = if record[0] == b'P' {
            Direction::ParentToWorker
        } else {
            Direction::WorkerToParent
        };
        let Some(message_type) = message_type(record[1]) else {
            continue;
        };
        let request_id = u64::from(record[2] & 0x0f);
        let payload_size = usize::from(record[3] & 0x0f);
        let payload = [0_u8; 15];
        let mut encoded = [0_u8; FRAME_HEADER_BYTES + 15];
        let header = FrameHeader::new(message_type, FUZZ_SESSION, request_id, payload_size as u32);
        let Ok(written) = encode_frame(&header, &payload[..payload_size], &mut encoded) else {
            continue;
        };
        if let Ok(frame) = decode_frame(&encoded[..written], FUZZ_SESSION) {
            exercise_typed_decoder(&frame);
            let _ = validator.observe(direction, frame.header());
        }
    }
}

/// Length-prefixed records carrying whole frames from the input itself.
fn exercise_framed_transcript(input: &[u8]) {
    let mut validator = SessionValidator::new(
        SessionId::new(FUZZ_SESSION).expect("non-zero"),
        ProtocolBudgets::default(),
    );
    let mut offset = 0;
    let mut records = 0;
    while records < MAXIMUM_TRANSCRIPT_RECORDS && input.len() - offset >= FRAMED_RECORD_HEADER_BYTES
    {
        let direction = if input[offset] & 1 == 0 {
            Direction::ParentToWorker
        } else {
            Direction::WorkerToParent
        };
        let frame_size = usize::from(input[offset + 1]) | (usize::from(input[offset + 2]) << 8);
        offset += FRAMED_RECORD_HEADER_BYTES;
        if frame_size > input.len() - offset {
            break;
        }
        if let Ok(frame) = decode_frame(&input[offset..offset + frame_size], FUZZ_SESSION) {
            exercise_typed_decoder(&frame);
            let _ = validator.observe(direction, frame.header());
        }
        offset += frame_size;
        records += 1;
    }
}

fuzz_target!(|data: &[u8]| {
    exercise_seeded_transcript(data);
    exercise_framed_transcript(data);

    let Ok(frame) = decode_frame(data, 0) else {
        return;
    };
    exercise_typed_decoder(&frame);

    let mut reader = PayloadReader::new(frame.payload());
    while reader.remaining() >= 8 {
        if reader.read_u64().is_err() {
            break;
        }
    }
    let _ = reader.finish();

    let Some(session_id) = SessionId::new(frame.header().session_id) else {
        return;
    };
    let mut validator = SessionValidator::new(session_id, ProtocolBudgets::default());
    let direction = if frame.payload().first().is_none_or(|byte| byte & 1 == 0) {
        Direction::ParentToWorker
    } else {
        Direction::WorkerToParent
    };
    let _ = validator.observe(direction, frame.header());
});
