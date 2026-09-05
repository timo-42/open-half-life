//! Port of `tests/parser/protocol_test.cpp`.
//!
//! Every case from the C++ suite appears here with the same fixtures and the
//! same expected error codes. Cases whose C++ failure mode is unrepresentable
//! in this port (an unknown `MessageType`/`ProtocolStatus`/`ProtocolPhase`
//! value in a *constructed* message, an out-of-range policy struct, a
//! `FrameView` carrying a prior error) are covered instead by the
//! constructor-rejection tests at the end of this file.

#![allow(
    clippy::cast_possible_truncation,
    clippy::comparison_chain,
    clippy::elidable_lifetime_names,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use ohl_parser_protocol::frame::encode_frame_header;
use ohl_parser_protocol::messages::{
    ArchiveSpelling, COMPLETE_PAYLOAD_BYTES, Complete, DataChunk, ENTRY_BATCH_ENTRY_PREFIX_BYTES,
    ENTRY_BATCH_PREFIX_BYTES, EntryBatch, EntryBatchEntry, EntryBatchPolicy, HELLO_PAYLOAD_BYTES,
    Hello, MAXIMUM_DATA_CHUNK_BYTES, MAXIMUM_ENTRY_BATCH_ENTRIES, MAXIMUM_ENTRY_BATCH_PATH_BYTES,
    MAXIMUM_ENUMERATED_ENTRIES, MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES,
    MAXIMUM_ENUMERATED_TOTAL_BYTES, MAXIMUM_READ_BYTES, OperationPhase, READ_REPLY_PREFIX_BYTES,
    READ_REQUEST_PAYLOAD_BYTES, ReadReply, ReadRequest, STREAM_ENTRY_PAYLOAD_BYTES,
    SourceReadPolicy, StreamEntry, decode_cancel_ack_payload, decode_cancel_payload,
    decode_complete_payload, decode_data_chunk_payload, decode_entry_batch_payload,
    decode_enumerate_payload, decode_hello_payload, decode_read_reply_payload,
    decode_read_request_payload, decode_ready_payload, decode_shutdown_payload,
    decode_stream_entry_payload, encode_complete_payload, encode_data_chunk_payload,
    encode_entry_batch_payload, encode_hello_payload, encode_read_reply_payload,
    encode_read_request_payload, encode_stream_entry_payload,
};
use ohl_parser_protocol::{
    Direction, FRAME_HEADER_BYTES, FrameHeader, FrameView, MAXIMUM_CUMULATIVE_PAYLOAD_BYTES,
    MAXIMUM_FRAME_PAYLOAD_BYTES, MAXIMUM_PROTOCOL_MESSAGES, MessageType, PayloadReader,
    PayloadWriter, ProtocolBudgets, ProtocolError, ProtocolPhase, ProtocolStatus, SessionId,
    SessionState, SessionValidator, decode_frame, decode_frame_header, encode_frame,
};

const SESSION: u64 = 0x0102_0304_0506_0708;

fn frame(message_type: MessageType, request_id: u64, payload_length: u32) -> FrameHeader {
    FrameHeader::new(message_type, SESSION, request_id, payload_length)
}

fn payload_frame<'a>(
    message_type: MessageType,
    payload: &'a [u8],
    request_id: u64,
) -> FrameView<'a> {
    FrameView::new(
        frame(
            message_type,
            request_id,
            u32::try_from(payload.len()).expect("test payloads fit"),
        ),
        payload,
    )
}

fn validator() -> SessionValidator {
    SessionValidator::new(
        SessionId::new(SESSION).expect("non-zero"),
        ProtocolBudgets::default(),
    )
}

fn observe(
    validator: &mut SessionValidator,
    direction: Direction,
    message_type: MessageType,
    request_id: u64,
) -> Result<(), ProtocolError> {
    validator.observe(direction, &frame(message_type, request_id, 0))
}

fn observe_sized(
    validator: &mut SessionValidator,
    direction: Direction,
    message_type: MessageType,
    request_id: u64,
    payload_length: u32,
) -> Result<(), ProtocolError> {
    validator.observe(direction, &frame(message_type, request_id, payload_length))
}

fn handshake(validator: &mut SessionValidator) {
    assert_eq!(
        observe(validator, Direction::ParentToWorker, MessageType::Hello, 0),
        Ok(())
    );
    assert_eq!(
        observe(validator, Direction::WorkerToParent, MessageType::Ready, 0),
        Ok(())
    );
    assert_eq!(validator.state(), SessionState::Idle);
}

fn handshaked() -> SessionValidator {
    let mut validator = validator();
    handshake(&mut validator);
    validator
}

fn result_type(operation: MessageType) -> MessageType {
    if operation == MessageType::Enumerate {
        MessageType::EntryBatch
    } else {
        MessageType::DataChunk
    }
}

const OPERATIONS: [MessageType; 2] = [MessageType::Enumerate, MessageType::StreamEntry];

fn maximum_entry_batch_policy() -> EntryBatchPolicy {
    EntryBatchPolicy::new(
        MAXIMUM_ENUMERATED_ENTRIES,
        MAXIMUM_ENUMERATED_PATH_BYTES,
        MAXIMUM_ENUMERATED_ENTRY_BYTES,
        MAXIMUM_ENUMERATED_TOTAL_BYTES,
        None,
    )
    .expect("maximum policy is valid")
}

fn spelling(bytes: &[u8]) -> ArchiveSpelling<'_> {
    ArchiveSpelling::new(bytes).expect("test spellings are printable")
}

/// Builds an `entry_batch` payload from raw, possibly invalid, field values.
fn raw_batch(entries: &[(u64, u64, &[u8])]) -> Vec<u8> {
    let size = ENTRY_BATCH_PREFIX_BYTES
        + entries
            .iter()
            .map(|(_, _, path)| ENTRY_BATCH_ENTRY_PREFIX_BYTES + path.len())
            .sum::<usize>();
    let mut payload = vec![0_u8; size];
    let mut writer = PayloadWriter::new(&mut payload);
    writer
        .write_u16(u16::try_from(entries.len()).expect("test batches fit"))
        .expect("fits");
    for (token, size_bytes, path) in entries {
        writer.write_u64(*token).expect("fits");
        writer.write_u64(*size_bytes).expect("fits");
        writer
            .write_u16(u16::try_from(path.len()).expect("test paths fit"))
            .expect("fits");
        writer.write_bytes(path).expect("fits");
    }
    payload
}

// ------------------------------------------------------------- framing ----

#[test]
fn header_encoding_is_canonical_little_endian() {
    let source = frame(MessageType::Enumerate, 0x1112_1314_1516_1718, 0x0001_0203);
    let mut encoded = [0_u8; FRAME_HEADER_BYTES];
    assert_eq!(encode_frame_header(&source, &mut encoded), Ok(()));
    let expected: [u8; FRAME_HEADER_BYTES] = [
        0x4f, 0x48, 0x4c, 0x50, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x03, 0x02, 0x01,
        0x00, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13,
        0x12, 0x11,
    ];
    assert_eq!(encoded, expected);
    let decoded = decode_frame_header(&encoded).expect("valid header");
    assert_eq!(decoded.payload_length, source.payload_length);
    assert_eq!(decoded.session_id, source.session_id);
    assert_eq!(decoded.request_id, source.request_id);
}

#[test]
fn frame_round_trip_preserves_type_request_and_payload() {
    let payload = [0x00_u8, 0x7f, 0xff];
    let header = frame(MessageType::DataChunk, 7, 3);
    let mut encoded = [0_u8; FRAME_HEADER_BYTES + 3];
    assert_eq!(
        encode_frame(&header, &payload, &mut encoded),
        Ok(encoded.len())
    );
    let decoded = decode_frame(&encoded, SESSION).expect("valid frame");
    assert_eq!(decoded.header().message_type, MessageType::DataChunk);
    assert_eq!(decoded.header().request_id, 7);
    assert_eq!(decoded.payload(), payload);
}

#[test]
fn frame_encoding_rejects_mismatched_length_short_output_and_flags() {
    let payload = [0x01_u8, 0x02];
    let mut encoded = [0_u8; FRAME_HEADER_BYTES + 2];
    let mut header = frame(MessageType::DataChunk, 1, 1);
    assert_eq!(
        encode_frame(&header, &payload, &mut encoded),
        Err(ProtocolError::NoncanonicalValue)
    );
    header.payload_length = 2;
    let last = encoded.len() - 1;
    assert_eq!(
        encode_frame(&header, &payload, &mut encoded[..last]),
        Err(ProtocolError::OutputTooSmall)
    );
    header.flags = 1;
    assert_eq!(
        encode_frame(&header, &payload, &mut encoded),
        Err(ProtocolError::ReservedFlags)
    );
}

#[test]
fn frame_decoding_rejects_every_malformed_header_variant() {
    let mut encoded = [0_u8; FRAME_HEADER_BYTES];
    encode_frame_header(&frame(MessageType::Hello, 0, 0), &mut encoded).expect("valid header");

    assert_eq!(
        decode_frame(&encoded[..FRAME_HEADER_BYTES - 1], 0),
        Err(ProtocolError::TruncatedHeader)
    );

    let mut invalid = encoded;
    invalid[0] = 0;
    assert_eq!(decode_frame(&invalid, 0), Err(ProtocolError::InvalidMagic));

    invalid = encoded;
    invalid[4] = 2;
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::UnsupportedVersion)
    );

    invalid = encoded;
    invalid[8] = 0xff;
    invalid[9] = 0xff;
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::UnknownMessageType)
    );

    invalid = encoded;
    invalid[10] = 1;
    assert_eq!(decode_frame(&invalid, 0), Err(ProtocolError::ReservedFlags));

    invalid = encoded;
    let over = MAXIMUM_FRAME_PAYLOAD_BYTES + 1;
    invalid[12..16].copy_from_slice(&over.to_le_bytes());
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::PayloadTooLarge)
    );

    invalid = encoded;
    invalid[16..24].fill(0);
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::InvalidSessionId)
    );

    invalid = encoded;
    invalid[24] = 1;
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::InvalidRequestId)
    );

    invalid = encoded;
    invalid[8] = 0x10;
    invalid[9] = 0x00;
    assert_eq!(
        decode_frame(&invalid, 0),
        Err(ProtocolError::InvalidRequestId)
    );

    assert_eq!(
        decode_frame(&encoded, SESSION + 1),
        Err(ProtocolError::WrongSessionId)
    );

    let payload_header = frame(MessageType::EntryBatch, 1, 1);
    encode_frame_header(&payload_header, &mut encoded).expect("valid header");
    assert_eq!(
        decode_frame(&encoded, 0),
        Err(ProtocolError::TruncatedPayload)
    );

    let mut trailing = [0_u8; FRAME_HEADER_BYTES + 2];
    let header_span: &mut [u8; FRAME_HEADER_BYTES] = (&mut trailing[..FRAME_HEADER_BYTES])
        .try_into()
        .expect("exact");
    encode_frame_header(&payload_header, header_span).expect("valid header");
    assert_eq!(
        decode_frame(&trailing, 0),
        Err(ProtocolError::TrailingBytes)
    );
}

#[test]
fn exact_maximum_frame_round_trips() {
    let payload = vec![0x5a_u8; MAXIMUM_FRAME_PAYLOAD_BYTES as usize];
    let mut encoded = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
    let header = frame(MessageType::DataChunk, 1, MAXIMUM_FRAME_PAYLOAD_BYTES);
    assert_eq!(
        encode_frame(&header, &payload, &mut encoded),
        Ok(encoded.len())
    );
    let decoded = decode_frame(&encoded, 0).expect("valid frame");
    assert_eq!(decoded.payload().len(), payload.len());
}

// ------------------------------------------------------- payload codec ----

#[test]
fn payload_codec_is_canonical_little_endian() {
    let canonical: [u8; 22] = [
        0x12, 0x56, 0x34, 0xde, 0xbc, 0x9a, 0x78, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01,
        0x01, 0x06, 0x00, 0x03, 0x00, 0xaa, 0xbb,
    ];
    let tail = [0xaa_u8, 0xbb];
    let mut storage = [0_u8; 32];
    let mut writer = PayloadWriter::new(&mut storage);
    writer.write_u8(0x12).expect("fits");
    writer.write_u16(0x3456).expect("fits");
    writer.write_u32(0x789a_bcde).expect("fits");
    writer.write_u64(0x0102_0304_0506_0708).expect("fits");
    writer.write_bool(true).expect("fits");
    writer
        .write_status(ProtocolStatus::SourceChanged)
        .expect("fits");
    writer.write_phase(ProtocolPhase::SourceRead).expect("fits");
    writer.write_bytes(&tail).expect("fits");
    assert_eq!(writer.written(), canonical);

    let mut reader = PayloadReader::new(&canonical);
    assert_eq!(reader.read_u8(), Ok(0x12));
    assert_eq!(reader.read_u16(), Ok(0x3456));
    assert_eq!(reader.read_u32(), Ok(0x789a_bcde));
    assert_eq!(reader.read_u64(), Ok(0x0102_0304_0506_0708));
    assert_eq!(reader.read_bool(), Ok(true));
    assert_eq!(reader.read_status(), Ok(ProtocolStatus::SourceChanged));
    assert_eq!(reader.read_phase(), Ok(ProtocolPhase::SourceRead));
    assert_eq!(reader.read_bytes(tail.len()), Ok(&tail[..]));
    assert_eq!(reader.finish(), Ok(()));
}

#[test]
fn payload_codec_rejections_are_sticky() {
    let mut small = [0_u8; 1];
    let mut writer = PayloadWriter::new(&mut small);
    assert_eq!(writer.write_u16(1), Err(ProtocolError::OutputTooSmall));
    assert_eq!(writer.write_u8(1), Err(ProtocolError::OutputTooSmall));
    assert_eq!(writer.len(), 0);
    assert_eq!(writer.error(), Some(ProtocolError::OutputTooSmall));

    let mut bool_reader = PayloadReader::new(&[2_u8]);
    assert_eq!(
        bool_reader.read_bool(),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(bool_reader.read_u8(), Err(ProtocolError::NoncanonicalValue));
    assert_eq!(bool_reader.remaining(), 0);

    let invalid_status = [0xff_u8, 0xff];
    assert_eq!(
        PayloadReader::new(&invalid_status).read_status(),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        PayloadReader::new(&invalid_status).read_phase(),
        Err(ProtocolError::NoncanonicalValue)
    );

    assert_eq!(
        PayloadReader::new(&[0_u8]).read_u16(),
        Err(ProtocolError::PayloadUnderflow)
    );

    let mut trailing_reader = PayloadReader::new(&[0_u8, 1]);
    assert_eq!(trailing_reader.read_u8(), Ok(0));
    assert_eq!(
        trailing_reader.finish(),
        Err(ProtocolError::PayloadTrailingBytes)
    );

    let oversized = vec![0_u8; MAXIMUM_FRAME_PAYLOAD_BYTES as usize + 1];
    assert_eq!(
        PayloadReader::new(&oversized).error(),
        Some(ProtocolError::PayloadTooLarge)
    );
}

// ------------------------------------------------------ session states ----

#[test]
fn valid_state_sequence_accounts_for_messages_and_bytes() {
    let mut validator = validator();
    handshake(&mut validator);
    let steps: [(Direction, MessageType, u64, u32); 9] = [
        (Direction::ParentToWorker, MessageType::Enumerate, 1, 3),
        (Direction::WorkerToParent, MessageType::ReadRequest, 1, 2),
        (Direction::ParentToWorker, MessageType::ReadReply, 1, 4),
        (Direction::WorkerToParent, MessageType::EntryBatch, 1, 5),
        (Direction::WorkerToParent, MessageType::Complete, 1, 1),
        (Direction::ParentToWorker, MessageType::StreamEntry, 2, 2),
        (Direction::WorkerToParent, MessageType::DataChunk, 2, 8),
        (Direction::WorkerToParent, MessageType::Complete, 2, 1),
        (Direction::ParentToWorker, MessageType::Shutdown, 0, 0),
    ];
    for (direction, message_type, request_id, payload_length) in steps {
        assert_eq!(
            observe_sized(
                &mut validator,
                direction,
                message_type,
                request_id,
                payload_length
            ),
            Ok(())
        );
    }
    assert_eq!(validator.state(), SessionState::Closed);
    assert_eq!(validator.message_count(), 11);
    assert_eq!(validator.payload_bytes(), 26);
}

#[test]
fn state_rejections_match_the_contract() {
    assert_eq!(
        observe(
            &mut validator(),
            Direction::WorkerToParent,
            MessageType::Ready,
            0
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut wrong_session = handshaked();
    assert_eq!(
        wrong_session.observe(
            Direction::ParentToWorker,
            &FrameHeader::new(MessageType::Enumerate, SESSION + 1, 1, 0)
        ),
        Err(ProtocolError::WrongSessionId)
    );

    let mut no_read = handshaked();
    assert_eq!(
        observe(
            &mut no_read,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut no_read,
            Direction::ParentToWorker,
            MessageType::ReadReply,
            1
        ),
        Err(ProtocolError::NoReadInFlight)
    );

    let mut nonmonotonic = handshaked();
    assert_eq!(
        observe(
            &mut nonmonotonic,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            2
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut nonmonotonic,
            Direction::WorkerToParent,
            MessageType::Complete,
            2
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut nonmonotonic,
            Direction::ParentToWorker,
            MessageType::StreamEntry,
            2
        ),
        Err(ProtocolError::RequestIdNotMonotonic)
    );

    let mut second_read = handshaked();
    assert_eq!(
        observe(
            &mut second_read,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut second_read,
            Direction::WorkerToParent,
            MessageType::ReadRequest,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut second_read,
            Direction::WorkerToParent,
            MessageType::ReadRequest,
            1
        ),
        Err(ProtocolError::ReadAlreadyActive)
    );

    let mut result_in_flight = handshaked();
    assert_eq!(
        observe(
            &mut result_in_flight,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut result_in_flight,
            Direction::WorkerToParent,
            MessageType::ReadRequest,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut result_in_flight,
            Direction::WorkerToParent,
            MessageType::EntryBatch,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut wrong_request = handshaked();
    assert_eq!(
        observe(
            &mut wrong_request,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut wrong_request,
            Direction::WorkerToParent,
            MessageType::EntryBatch,
            2
        ),
        Err(ProtocolError::WrongRequestId)
    );

    let mut second_request = handshaked();
    assert_eq!(
        observe(
            &mut second_request,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut second_request,
            Direction::ParentToWorker,
            MessageType::StreamEntry,
            2
        ),
        Err(ProtocolError::RequestAlreadyActive)
    );
}

#[test]
fn completion_wins_both_orderings_of_the_cancellation_race() {
    for operation in OPERATIONS {
        let result = result_type(operation);

        // Parent-side ordering: cancellation is committed while a read is
        // outstanding. Its already-crossing reply resolves the read before
        // bounded worker traffic and completion arrive.
        let mut cancel_first = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::WorkerToParent, MessageType::ReadRequest),
            (Direction::ParentToWorker, MessageType::Cancel),
            (Direction::ParentToWorker, MessageType::ReadReply),
            (Direction::WorkerToParent, result),
            (Direction::WorkerToParent, MessageType::Complete),
        ] {
            assert_eq!(
                observe(&mut cancel_first, direction, message_type, 1),
                Ok(()),
                "cancel-first race step {message_type:?}"
            );
        }
        assert_eq!(cancel_first.state(), SessionState::Idle);

        // Worker-side ordering: completion is committed before the already
        // sent cancel arrives. The stale cancel is consumed once.
        let mut complete_first = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::WorkerToParent, MessageType::ReadRequest),
            (Direction::ParentToWorker, MessageType::ReadReply),
            (Direction::WorkerToParent, result),
            (Direction::WorkerToParent, MessageType::Complete),
            (Direction::ParentToWorker, MessageType::Cancel),
        ] {
            assert_eq!(
                observe(&mut complete_first, direction, message_type, 1),
                Ok(())
            );
        }
        assert_eq!(complete_first.state(), SessionState::Idle);
        assert_eq!(
            observe(&mut complete_first, Direction::ParentToWorker, operation, 2),
            Ok(())
        );

        // A read request may itself cross cancellation. It is bounded and
        // ignored; cancel_ack remains the cancellation terminal response.
        let mut crossed_read = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::ParentToWorker, MessageType::Cancel),
            (Direction::WorkerToParent, MessageType::ReadRequest),
            (Direction::WorkerToParent, MessageType::CancelAck),
        ] {
            assert_eq!(
                observe(&mut crossed_read, direction, message_type, 1),
                Ok(())
            );
        }
        assert_eq!(crossed_read.state(), SessionState::Cancelled);
    }
}

#[test]
fn unresolved_reads_block_crossed_completion() {
    for operation in OPERATIONS {
        let result = result_type(operation);

        let mut pre_cancel_read = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::WorkerToParent, MessageType::ReadRequest),
            (Direction::ParentToWorker, MessageType::Cancel),
        ] {
            assert_eq!(
                observe(&mut pre_cancel_read, direction, message_type, 1),
                Ok(())
            );
        }
        assert_eq!(
            observe(
                &mut pre_cancel_read,
                Direction::WorkerToParent,
                MessageType::Complete,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );

        let mut post_cancel_read = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::ParentToWorker, MessageType::Cancel),
            (Direction::WorkerToParent, MessageType::ReadRequest),
        ] {
            assert_eq!(
                observe(&mut post_cancel_read, direction, message_type, 1),
                Ok(())
            );
        }
        assert_eq!(
            observe(
                &mut post_cancel_read,
                Direction::WorkerToParent,
                MessageType::Complete,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );

        let mut result_before_reply = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::WorkerToParent, MessageType::ReadRequest),
            (Direction::ParentToWorker, MessageType::Cancel),
        ] {
            assert_eq!(
                observe(&mut result_before_reply, direction, message_type, 1),
                Ok(())
            );
        }
        assert_eq!(
            observe(
                &mut result_before_reply,
                Direction::WorkerToParent,
                result,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
    }
}

#[test]
fn late_read_reply_drains_exactly_once() {
    for operation in OPERATIONS {
        let acknowledged = |pre_cancel_read: bool, post_cancel_read: bool| {
            let mut validator = handshaked();
            assert_eq!(
                observe(&mut validator, Direction::ParentToWorker, operation, 1),
                Ok(())
            );
            if pre_cancel_read {
                assert_eq!(
                    observe(
                        &mut validator,
                        Direction::WorkerToParent,
                        MessageType::ReadRequest,
                        1
                    ),
                    Ok(())
                );
            }
            assert_eq!(
                observe(
                    &mut validator,
                    Direction::ParentToWorker,
                    MessageType::Cancel,
                    1
                ),
                Ok(())
            );
            if post_cancel_read {
                assert_eq!(
                    observe(
                        &mut validator,
                        Direction::WorkerToParent,
                        MessageType::ReadRequest,
                        1
                    ),
                    Ok(())
                );
            }
            assert_eq!(
                observe(
                    &mut validator,
                    Direction::WorkerToParent,
                    MessageType::CancelAck,
                    1
                ),
                Ok(())
            );
            validator
        };

        // cancel_ack may overtake the reply that was already queued in the
        // opposite direction. The cancelled session drains it once and stays
        // terminal until shutdown.
        let mut valid = acknowledged(true, false);
        assert_eq!(valid.state(), SessionState::Cancelled);
        assert_eq!(valid.active_request_id(), 0);
        assert_eq!(
            observe(
                &mut valid,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Ok(())
        );
        assert_eq!(valid.state(), SessionState::Cancelled);
        assert_eq!(
            observe(
                &mut valid,
                Direction::ParentToWorker,
                MessageType::Shutdown,
                0
            ),
            Ok(())
        );
        assert_eq!(valid.state(), SessionState::Closed);

        let mut wrong_id = acknowledged(true, false);
        assert_eq!(
            observe(
                &mut wrong_id,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                2
            ),
            Err(ProtocolError::WrongRequestId)
        );
        assert_eq!(
            observe(
                &mut wrong_id,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::WrongRequestId)
        );
        assert_eq!(wrong_id.state(), SessionState::Failed);

        let mut wrong_direction = acknowledged(true, false);
        assert_eq!(
            observe(
                &mut wrong_direction,
                Direction::WorkerToParent,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );
        assert_eq!(
            observe(
                &mut wrong_direction,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );
        assert_eq!(wrong_direction.state(), SessionState::Failed);

        let mut duplicate = acknowledged(true, false);
        assert_eq!(
            observe(
                &mut duplicate,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Ok(())
        );
        assert_eq!(
            observe(
                &mut duplicate,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );

        let mut no_outstanding = acknowledged(false, false);
        assert_eq!(
            observe(
                &mut no_outstanding,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );

        let mut post_cancel_read = acknowledged(false, true);
        assert_eq!(
            observe(
                &mut post_cancel_read,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );

        let mut shutdown = acknowledged(true, false);
        assert_eq!(
            observe(
                &mut shutdown,
                Direction::ParentToWorker,
                MessageType::Shutdown,
                0
            ),
            Ok(())
        );
        assert_eq!(
            observe(
                &mut shutdown,
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::TerminalState)
        );
    }
}

#[test]
fn wrong_direction_and_active_terminal_messages_are_rejected() {
    for operation in OPERATIONS {
        let result = result_type(operation);
        let rejects = |direction: Direction, message_type: MessageType, outstanding_read: bool| {
            let mut validator = handshaked();
            assert_eq!(
                observe(&mut validator, Direction::ParentToWorker, operation, 1),
                Ok(())
            );
            if outstanding_read {
                assert_eq!(
                    observe(
                        &mut validator,
                        Direction::WorkerToParent,
                        MessageType::ReadRequest,
                        1
                    ),
                    Ok(())
                );
            }
            let request_id = u64::from(message_type != MessageType::Shutdown);
            assert_eq!(
                observe(&mut validator, direction, message_type, request_id),
                Err(ProtocolError::UnexpectedMessage),
                "{message_type:?} in {direction:?}"
            );
        };
        rejects(Direction::ParentToWorker, MessageType::ReadRequest, false);
        rejects(Direction::WorkerToParent, MessageType::ReadReply, false);
        rejects(Direction::ParentToWorker, result, false);
        rejects(Direction::ParentToWorker, MessageType::Complete, false);
        rejects(Direction::ParentToWorker, MessageType::Shutdown, false);
        rejects(Direction::WorkerToParent, MessageType::Complete, true);
    }
}

#[test]
fn crossed_traffic_while_cancelling_is_rejected() {
    let cancelling = |operation: MessageType, outstanding_read: bool| {
        let mut validator = handshaked();
        assert_eq!(
            observe(&mut validator, Direction::ParentToWorker, operation, 1),
            Ok(())
        );
        if outstanding_read {
            assert_eq!(
                observe(
                    &mut validator,
                    Direction::WorkerToParent,
                    MessageType::ReadRequest,
                    1
                ),
                Ok(())
            );
        }
        assert_eq!(
            observe(
                &mut validator,
                Direction::ParentToWorker,
                MessageType::Cancel,
                1
            ),
            Ok(())
        );
        validator
    };

    for operation in OPERATIONS {
        let result = result_type(operation);
        let other_result = result_type(if operation == MessageType::Enumerate {
            MessageType::StreamEntry
        } else {
            MessageType::Enumerate
        });
        assert_eq!(
            observe(
                &mut cancelling(operation, false),
                Direction::ParentToWorker,
                result,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, false),
                Direction::WorkerToParent,
                other_result,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, false),
                Direction::ParentToWorker,
                MessageType::Complete,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, false),
                Direction::ParentToWorker,
                MessageType::ReadRequest,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, true),
                Direction::WorkerToParent,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::UnexpectedMessage)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, false),
                Direction::ParentToWorker,
                MessageType::ReadReply,
                1
            ),
            Err(ProtocolError::NoReadInFlight)
        );
        assert_eq!(
            observe(
                &mut cancelling(operation, true),
                Direction::WorkerToParent,
                MessageType::ReadRequest,
                1
            ),
            Err(ProtocolError::ReadAlreadyActive)
        );
    }
}

#[test]
fn terminal_and_cancel_rejections_are_sticky() {
    assert_eq!(
        observe(
            &mut validator(),
            Direction::WorkerToParent,
            MessageType::Hello,
            0
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut ready_from_parent = validator();
    assert_eq!(
        observe(
            &mut ready_from_parent,
            Direction::ParentToWorker,
            MessageType::Hello,
            0
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut ready_from_parent,
            Direction::ParentToWorker,
            MessageType::Ready,
            0
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut sticky = validator();
    assert_eq!(
        observe(
            &mut sticky,
            Direction::WorkerToParent,
            MessageType::Ready,
            0
        ),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(
        observe(
            &mut sticky,
            Direction::ParentToWorker,
            MessageType::Hello,
            0
        ),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(sticky.error(), Some(ProtocolError::UnexpectedMessage));
    assert_eq!(sticky.state(), SessionState::Failed);

    let mut closed = handshaked();
    assert_eq!(
        observe(
            &mut closed,
            Direction::ParentToWorker,
            MessageType::Shutdown,
            0
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut closed,
            Direction::ParentToWorker,
            MessageType::Shutdown,
            0
        ),
        Err(ProtocolError::TerminalState)
    );
    assert_eq!(
        observe(
            &mut closed,
            Direction::ParentToWorker,
            MessageType::Hello,
            0
        ),
        Err(ProtocolError::TerminalState)
    );
    assert_eq!(closed.state(), SessionState::Failed);

    let mut cancelled = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::Enumerate),
        (Direction::ParentToWorker, MessageType::Cancel),
        (Direction::WorkerToParent, MessageType::CancelAck),
    ] {
        assert_eq!(observe(&mut cancelled, direction, message_type, 1), Ok(()));
    }
    assert_eq!(cancelled.state(), SessionState::Cancelled);
    assert_eq!(
        observe(
            &mut cancelled,
            Direction::ParentToWorker,
            MessageType::StreamEntry,
            2
        ),
        Err(ProtocolError::TerminalState)
    );

    let mut idle_cancel = handshaked();
    assert_eq!(
        observe(
            &mut idle_cancel,
            Direction::ParentToWorker,
            MessageType::Cancel,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut worker_cancel = handshaked();
    assert_eq!(
        observe(
            &mut worker_cancel,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut worker_cancel,
            Direction::WorkerToParent,
            MessageType::Cancel,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut wrong_cancel_id = handshaked();
    assert_eq!(
        observe(
            &mut wrong_cancel_id,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut wrong_cancel_id,
            Direction::ParentToWorker,
            MessageType::Cancel,
            2
        ),
        Err(ProtocolError::WrongRequestId)
    );

    let mut early_ack = handshaked();
    assert_eq!(
        observe(
            &mut early_ack,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe(
            &mut early_ack,
            Direction::WorkerToParent,
            MessageType::CancelAck,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut ack_from_parent = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::Enumerate),
        (Direction::ParentToWorker, MessageType::Cancel),
    ] {
        assert_eq!(
            observe(&mut ack_from_parent, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut ack_from_parent,
            Direction::ParentToWorker,
            MessageType::CancelAck,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut ack_wrong_id = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::Enumerate),
        (Direction::ParentToWorker, MessageType::Cancel),
    ] {
        assert_eq!(
            observe(&mut ack_wrong_id, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut ack_wrong_id,
            Direction::WorkerToParent,
            MessageType::CancelAck,
            2
        ),
        Err(ProtocolError::WrongRequestId)
    );

    let mut repeated_stale_cancel = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::Enumerate),
        (Direction::WorkerToParent, MessageType::Complete),
        (Direction::ParentToWorker, MessageType::Cancel),
    ] {
        assert_eq!(
            observe(&mut repeated_stale_cancel, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut repeated_stale_cancel,
            Direction::ParentToWorker,
            MessageType::Cancel,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut wrong_stale_cancel = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::Enumerate),
        (Direction::WorkerToParent, MessageType::Complete),
    ] {
        assert_eq!(
            observe(&mut wrong_stale_cancel, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut wrong_stale_cancel,
            Direction::ParentToWorker,
            MessageType::Cancel,
            2
        ),
        Err(ProtocolError::WrongRequestId)
    );

    let mut ack_after_complete = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::StreamEntry),
        (Direction::WorkerToParent, MessageType::Complete),
    ] {
        assert_eq!(
            observe(&mut ack_after_complete, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut ack_after_complete,
            Direction::WorkerToParent,
            MessageType::CancelAck,
            1
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut repeated_ack = handshaked();
    for (direction, message_type) in [
        (Direction::ParentToWorker, MessageType::StreamEntry),
        (Direction::ParentToWorker, MessageType::Cancel),
        (Direction::WorkerToParent, MessageType::CancelAck),
    ] {
        assert_eq!(
            observe(&mut repeated_ack, direction, message_type, 1),
            Ok(())
        );
    }
    assert_eq!(
        observe(
            &mut repeated_ack,
            Direction::WorkerToParent,
            MessageType::CancelAck,
            1
        ),
        Err(ProtocolError::TerminalState)
    );
}

#[test]
fn cancellation_sequence_and_budget_ceilings() {
    let mut validator = handshaked();
    for (direction, message_type, request_id) in [
        (Direction::ParentToWorker, MessageType::Enumerate, 1),
        (Direction::ParentToWorker, MessageType::Cancel, 1),
        (Direction::WorkerToParent, MessageType::CancelAck, 1),
        (Direction::ParentToWorker, MessageType::Shutdown, 0),
    ] {
        assert_eq!(
            observe(&mut validator, direction, message_type, request_id),
            Ok(())
        );
    }
    assert_eq!(validator.state(), SessionState::Closed);

    let mut messages = SessionValidator::new(
        SessionId::new(SESSION).expect("non-zero"),
        ProtocolBudgets::new(3, 5).expect("valid"),
    );
    assert_eq!(
        observe_sized(
            &mut messages,
            Direction::ParentToWorker,
            MessageType::Hello,
            0,
            2
        ),
        Ok(())
    );
    assert_eq!(
        observe_sized(
            &mut messages,
            Direction::WorkerToParent,
            MessageType::Ready,
            0,
            1
        ),
        Ok(())
    );
    assert_eq!(
        observe_sized(
            &mut messages,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1,
            2
        ),
        Ok(())
    );
    assert_eq!(messages.message_count(), 3);
    assert_eq!(messages.payload_bytes(), 5);
    assert_eq!(
        observe(
            &mut messages,
            Direction::WorkerToParent,
            MessageType::Complete,
            1
        ),
        Err(ProtocolError::MessageBudgetExceeded)
    );

    let mut bytes = SessionValidator::new(
        SessionId::new(SESSION).expect("non-zero"),
        ProtocolBudgets::new(4, 4).expect("valid"),
    );
    assert_eq!(
        observe_sized(
            &mut bytes,
            Direction::ParentToWorker,
            MessageType::Hello,
            0,
            2
        ),
        Ok(())
    );
    assert_eq!(
        observe_sized(
            &mut bytes,
            Direction::WorkerToParent,
            MessageType::Ready,
            0,
            2
        ),
        Ok(())
    );
    assert_eq!(
        observe_sized(
            &mut bytes,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1,
            1
        ),
        Err(ProtocolError::ByteBudgetExceeded)
    );

    // Invalid budgets and sessions are rejected by their constructors, so no
    // validator can hold them (C++ rejected them inside the validator).
    assert_eq!(
        ProtocolBudgets::new(0, 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        ProtocolBudgets::new(MAXIMUM_PROTOCOL_MESSAGES + 1, 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        ProtocolBudgets::new(1, 0),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        ProtocolBudgets::new(1, MAXIMUM_CUMULATIVE_PAYLOAD_BYTES + 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert!(SessionId::new(0).is_none());
}

// -------------------------------------------------------- typed schemas ----

#[test]
fn typed_hello_and_ready() {
    let canonical = Hello {
        source_size: 0x0102_0304_0506_0708,
        maximum_read_bytes: 0x0001_0203,
    };
    let expected: [u8; HELLO_PAYLOAD_BYTES] = [
        0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x03, 0x02, 0x01, 0x00,
    ];
    let mut encoded = [0_u8; HELLO_PAYLOAD_BYTES];
    assert_eq!(
        encode_hello_payload(&canonical, &mut encoded),
        Ok(HELLO_PAYLOAD_BYTES)
    );
    assert_eq!(encoded, expected);
    assert_eq!(
        decode_hello_payload(&payload_frame(MessageType::Hello, &encoded, 0)),
        Ok(canonical)
    );

    for message in [
        Hello {
            source_size: 1,
            maximum_read_bytes: 1,
        },
        Hello {
            source_size: u64::MAX,
            maximum_read_bytes: MAXIMUM_READ_BYTES,
        },
    ] {
        assert!(encode_hello_payload(&message, &mut encoded).is_ok());
        assert_eq!(
            decode_hello_payload(&payload_frame(MessageType::Hello, &encoded, 0)),
            Ok(message)
        );
    }

    let malformed = [0_u8; HELLO_PAYLOAD_BYTES + 1];
    for size in 0..HELLO_PAYLOAD_BYTES {
        assert_eq!(
            decode_hello_payload(&payload_frame(MessageType::Hello, &malformed[..size], 0)),
            Err(ProtocolError::PayloadUnderflow)
        );
    }
    assert_eq!(
        decode_hello_payload(&payload_frame(MessageType::Hello, &malformed, 0)),
        Err(ProtocolError::PayloadTrailingBytes)
    );

    // Encoder value bounds.
    for (source_size, maximum_read_bytes, ok) in [
        (0_u64, 1_u32, false),
        (1, 0, false),
        (1, MAXIMUM_READ_BYTES, true),
        (1, MAXIMUM_READ_BYTES + 1, false),
    ] {
        let message = Hello {
            source_size,
            maximum_read_bytes,
        };
        assert_eq!(
            encode_hello_payload(&message, &mut encoded).is_ok(),
            ok,
            "hello encode {source_size}/{maximum_read_bytes}"
        );
        // Decoder value bounds, from raw canonical field writes.
        let mut raw = [0_u8; HELLO_PAYLOAD_BYTES];
        let mut writer = PayloadWriter::new(&mut raw);
        writer.write_u64(source_size).expect("fits");
        writer.write_u32(maximum_read_bytes).expect("fits");
        let decoded = decode_hello_payload(&payload_frame(MessageType::Hello, &raw, 0));
        if ok {
            assert!(decoded.is_ok());
        } else {
            assert_eq!(decoded, Err(ProtocolError::NoncanonicalValue));
        }
    }

    assert_eq!(
        decode_ready_payload(&payload_frame(MessageType::Ready, &[], 0)),
        Ok(())
    );
    assert_eq!(
        decode_ready_payload(&payload_frame(MessageType::Ready, &[0], 0)),
        Err(ProtocolError::PayloadTrailingBytes)
    );
    assert_eq!(
        decode_hello_payload(&payload_frame(MessageType::Ready, &expected, 0)),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(
        decode_ready_payload(&payload_frame(MessageType::Hello, &[], 0)),
        Err(ProtocolError::UnexpectedMessage)
    );
}

#[test]
fn typed_exact_empty_messages() {
    assert_eq!(
        decode_enumerate_payload(&payload_frame(MessageType::Enumerate, &[], 1)),
        Ok(())
    );
    assert_eq!(
        decode_cancel_payload(&payload_frame(MessageType::Cancel, &[], 1)),
        Ok(())
    );
    assert_eq!(
        decode_cancel_ack_payload(&payload_frame(MessageType::CancelAck, &[], 1)),
        Ok(())
    );
    assert_eq!(
        decode_shutdown_payload(&payload_frame(MessageType::Shutdown, &[], 0)),
        Ok(())
    );

    let nonempty = [0x5a_u8];
    assert_eq!(
        decode_enumerate_payload(&payload_frame(MessageType::Enumerate, &nonempty, 1)),
        Err(ProtocolError::PayloadTrailingBytes)
    );
    assert_eq!(
        decode_cancel_payload(&payload_frame(MessageType::Cancel, &nonempty, 1)),
        Err(ProtocolError::PayloadTrailingBytes)
    );
    assert_eq!(
        decode_cancel_ack_payload(&payload_frame(MessageType::CancelAck, &nonempty, 1)),
        Err(ProtocolError::PayloadTrailingBytes)
    );
    assert_eq!(
        decode_shutdown_payload(&payload_frame(MessageType::Shutdown, &nonempty, 0)),
        Err(ProtocolError::PayloadTrailingBytes)
    );

    assert_eq!(
        decode_enumerate_payload(&payload_frame(MessageType::Cancel, &[], 1)),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(
        decode_cancel_payload(&payload_frame(MessageType::CancelAck, &[], 1)),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(
        decode_cancel_ack_payload(&payload_frame(MessageType::Enumerate, &[], 1)),
        Err(ProtocolError::UnexpectedMessage)
    );
    assert_eq!(
        decode_shutdown_payload(&payload_frame(MessageType::Ready, &[], 0)),
        Err(ProtocolError::UnexpectedMessage)
    );

    // Declared-length mismatches, both directions.
    let mut truncated = frame(MessageType::Enumerate, 1, 1);
    assert_eq!(
        decode_enumerate_payload(&FrameView::new(truncated, &[])),
        Err(ProtocolError::TruncatedPayload)
    );
    truncated.message_type = MessageType::Cancel;
    assert_eq!(
        decode_cancel_payload(&FrameView::new(truncated, &[])),
        Err(ProtocolError::TruncatedPayload)
    );
    truncated.message_type = MessageType::CancelAck;
    assert_eq!(
        decode_cancel_ack_payload(&FrameView::new(truncated, &[])),
        Err(ProtocolError::TruncatedPayload)
    );
    assert_eq!(
        decode_shutdown_payload(&FrameView::new(frame(MessageType::Shutdown, 0, 1), &[])),
        Err(ProtocolError::TruncatedPayload)
    );

    let mut trailing = frame(MessageType::Enumerate, 1, 0);
    assert_eq!(
        decode_enumerate_payload(&FrameView::new(trailing, &nonempty)),
        Err(ProtocolError::TrailingBytes)
    );
    trailing.message_type = MessageType::Cancel;
    assert_eq!(
        decode_cancel_payload(&FrameView::new(trailing, &nonempty)),
        Err(ProtocolError::TrailingBytes)
    );
    trailing.message_type = MessageType::CancelAck;
    assert_eq!(
        decode_cancel_ack_payload(&FrameView::new(trailing, &nonempty)),
        Err(ProtocolError::TrailingBytes)
    );
    assert_eq!(
        decode_shutdown_payload(&FrameView::new(
            frame(MessageType::Shutdown, 0, 0),
            &nonempty
        )),
        Err(ProtocolError::TrailingBytes)
    );

    // Invalid header request ids and reserved flags.
    assert_eq!(
        decode_enumerate_payload(&payload_frame(MessageType::Enumerate, &[], 0)),
        Err(ProtocolError::InvalidRequestId)
    );
    assert_eq!(
        decode_cancel_payload(&payload_frame(MessageType::Cancel, &[], 0)),
        Err(ProtocolError::InvalidRequestId)
    );
    assert_eq!(
        decode_cancel_ack_payload(&payload_frame(MessageType::CancelAck, &[], 0)),
        Err(ProtocolError::InvalidRequestId)
    );
    assert_eq!(
        decode_shutdown_payload(&payload_frame(MessageType::Shutdown, &[], 1)),
        Err(ProtocolError::InvalidRequestId)
    );
    let mut bad_flags = frame(MessageType::Enumerate, 1, 0);
    bad_flags.flags = 1;
    assert_eq!(
        decode_enumerate_payload(&FrameView::new(bad_flags, &[])),
        Err(ProtocolError::ReservedFlags)
    );
}

#[test]
fn typed_stream_entry() {
    let canonical = StreamEntry {
        source_token: 0x0102_0304_0506_0708,
    };
    let expected: [u8; STREAM_ENTRY_PAYLOAD_BYTES] =
        [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
    let mut encoded = [0_u8; STREAM_ENTRY_PAYLOAD_BYTES];
    assert_eq!(
        encode_stream_entry_payload(&canonical, &mut encoded),
        Ok(STREAM_ENTRY_PAYLOAD_BYTES)
    );
    assert_eq!(encoded, expected);
    assert_eq!(
        decode_stream_entry_payload(&payload_frame(MessageType::StreamEntry, &encoded, 1)),
        Ok(canonical)
    );

    // The token is opaque: every value round trips.
    for token in [0, u64::MAX, 0x89ab_cdef_0123_4567] {
        let message = StreamEntry {
            source_token: token,
        };
        assert_eq!(
            encode_stream_entry_payload(&message, &mut encoded),
            Ok(STREAM_ENTRY_PAYLOAD_BYTES)
        );
        assert_eq!(
            decode_stream_entry_payload(&payload_frame(MessageType::StreamEntry, &encoded, 1)),
            Ok(message)
        );
    }

    let malformed = [0_u8; STREAM_ENTRY_PAYLOAD_BYTES + 1];
    for size in 0..STREAM_ENTRY_PAYLOAD_BYTES {
        assert_eq!(
            decode_stream_entry_payload(&payload_frame(
                MessageType::StreamEntry,
                &malformed[..size],
                1
            )),
            Err(ProtocolError::PayloadUnderflow)
        );
    }
    assert_eq!(
        decode_stream_entry_payload(&payload_frame(MessageType::StreamEntry, &malformed, 1)),
        Err(ProtocolError::PayloadTrailingBytes)
    );
    assert_eq!(
        decode_stream_entry_payload(&payload_frame(MessageType::ReadRequest, &expected, 1)),
        Err(ProtocolError::UnexpectedMessage)
    );

    let mut declared_longer = frame(MessageType::StreamEntry, 1, 8);
    declared_longer.payload_length += 1;
    assert_eq!(
        decode_stream_entry_payload(&FrameView::new(declared_longer, &expected)),
        Err(ProtocolError::TruncatedPayload)
    );
    let mut declared_shorter = frame(MessageType::StreamEntry, 1, 8);
    declared_shorter.payload_length -= 1;
    assert_eq!(
        decode_stream_entry_payload(&FrameView::new(declared_shorter, &expected)),
        Err(ProtocolError::TrailingBytes)
    );

    assert_eq!(
        decode_stream_entry_payload(&payload_frame(MessageType::StreamEntry, &expected, 0)),
        Err(ProtocolError::InvalidRequestId)
    );
    let mut bad_flags = frame(MessageType::StreamEntry, 1, 8);
    bad_flags.flags = 1;
    assert_eq!(
        decode_stream_entry_payload(&FrameView::new(bad_flags, &expected)),
        Err(ProtocolError::ReservedFlags)
    );

    let mut destination = [0xa5_u8; STREAM_ENTRY_PAYLOAD_BYTES];
    assert_eq!(
        encode_stream_entry_payload(
            &canonical,
            &mut destination[..STREAM_ENTRY_PAYLOAD_BYTES - 1]
        ),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(destination.iter().all(|byte| *byte == 0xa5));
}

#[test]
fn typed_read_request() {
    let policy = SourceReadPolicy::new(u64::MAX, MAXIMUM_READ_BYTES).expect("valid");
    let canonical = ReadRequest {
        read_sequence: 0x0102_0304,
        offset: 0x1112_1314_1516_1718,
        length: 0x0001_0203,
    };
    let expected: [u8; READ_REQUEST_PAYLOAD_BYTES] = [
        0x04, 0x03, 0x02, 0x01, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11, 0x03, 0x02, 0x01,
        0x00,
    ];
    let mut encoded = [0_u8; READ_REQUEST_PAYLOAD_BYTES];
    assert_eq!(
        encode_read_request_payload(&canonical, &policy, canonical.read_sequence, &mut encoded),
        Ok(READ_REQUEST_PAYLOAD_BYTES)
    );
    assert_eq!(encoded, expected);
    assert_eq!(
        decode_read_request_payload(
            &payload_frame(MessageType::ReadRequest, &encoded, 7),
            &policy,
            canonical.read_sequence
        ),
        Ok(canonical)
    );

    for message in [
        ReadRequest {
            read_sequence: 1,
            offset: 0,
            length: 1,
        },
        ReadRequest {
            read_sequence: u32::MAX,
            offset: u64::MAX - u64::from(MAXIMUM_READ_BYTES),
            length: MAXIMUM_READ_BYTES,
        },
    ] {
        let bounds = SourceReadPolicy::new(
            if message.length == 1 { 1 } else { u64::MAX },
            message.length,
        )
        .expect("valid");
        assert!(
            encode_read_request_payload(&message, &bounds, message.read_sequence, &mut encoded)
                .is_ok()
        );
        assert_eq!(
            decode_read_request_payload(
                &payload_frame(MessageType::ReadRequest, &encoded, 1),
                &bounds,
                message.read_sequence
            ),
            Ok(message)
        );
    }

    let tiny = SourceReadPolicy::new(1, 1).expect("valid");
    let malformed = [0_u8; READ_REQUEST_PAYLOAD_BYTES + 1];
    for size in 0..READ_REQUEST_PAYLOAD_BYTES {
        assert_eq!(
            decode_read_request_payload(
                &payload_frame(MessageType::ReadRequest, &malformed[..size], 1),
                &tiny,
                1
            ),
            Err(ProtocolError::PayloadUnderflow)
        );
    }
    assert_eq!(
        decode_read_request_payload(
            &payload_frame(MessageType::ReadRequest, &malformed, 1),
            &tiny,
            1
        ),
        Err(ProtocolError::PayloadTrailingBytes)
    );

    // Value and range bounds, encoder and decoder alike.
    let cases: [(ReadRequest, SourceReadPolicy, u32, Option<ProtocolError>); 8] = [
        (
            ReadRequest {
                read_sequence: 0,
                offset: 0,
                length: 1,
            },
            tiny,
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
        (
            ReadRequest {
                read_sequence: 2,
                offset: 0,
                length: 1,
            },
            tiny,
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: 0,
                length: 0,
            },
            tiny,
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: 0,
                length: 1,
            },
            tiny,
            0,
            Some(ProtocolError::InvalidBudget),
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: 0,
                length: MAXIMUM_READ_BYTES + 1,
            },
            SourceReadPolicy::new(u64::MAX, MAXIMUM_READ_BYTES).expect("valid"),
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: 4,
                length: 4,
            },
            SourceReadPolicy::new(8, 4).expect("valid"),
            1,
            None,
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: 4,
                length: 5,
            },
            SourceReadPolicy::new(8, 5).expect("valid"),
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
        (
            ReadRequest {
                read_sequence: 1,
                offset: u64::MAX - 1,
                length: 2,
            },
            SourceReadPolicy::new(u64::MAX, 2).expect("valid"),
            1,
            Some(ProtocolError::NoncanonicalValue),
        ),
    ];
    for (message, bounds, sequence, expected_error) in cases {
        let encode = encode_read_request_payload(&message, &bounds, sequence, &mut encoded);
        let mut raw = [0_u8; READ_REQUEST_PAYLOAD_BYTES];
        let mut writer = PayloadWriter::new(&mut raw);
        writer.write_u32(message.read_sequence).expect("fits");
        writer.write_u64(message.offset).expect("fits");
        writer.write_u32(message.length).expect("fits");
        let decode = decode_read_request_payload(
            &payload_frame(MessageType::ReadRequest, &raw, 1),
            &bounds,
            sequence,
        );
        match expected_error {
            None => {
                assert!(encode.is_ok());
                assert_eq!(decode, Ok(message));
            }
            Some(error) => {
                assert_eq!(encode, Err(error));
                assert_eq!(decode, Err(error));
            }
        }
    }

    // Invalid read policies are rejected by the constructor.
    assert_eq!(
        SourceReadPolicy::new(0, 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        SourceReadPolicy::new(1, 0),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        SourceReadPolicy::new(1, MAXIMUM_READ_BYTES + 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_read_request_payload(
            &payload_frame(MessageType::ReadRequest, &encoded, 1),
            &tiny,
            0
        ),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_read_request_payload(
            &payload_frame(MessageType::ReadReply, &encoded, 1),
            &policy,
            canonical.read_sequence
        ),
        Err(ProtocolError::UnexpectedMessage)
    );
}

#[test]
fn typed_read_reply() {
    let data = [0xaa_u8, 0xbb, 0xcc];
    let canonical = ReadReply {
        read_sequence: 0x0102_0304,
        status: ProtocolStatus::Ok,
        data: &data,
    };
    let expected: [u8; READ_REPLY_PREFIX_BYTES + 3] =
        [0x04, 0x03, 0x02, 0x01, 0x00, 0x00, 0xaa, 0xbb, 0xcc];
    let mut encoded = [0_u8; READ_REPLY_PREFIX_BYTES + 3];
    assert_eq!(
        encode_read_reply_payload(&canonical, canonical.read_sequence, 3, &mut encoded),
        Ok(encoded.len())
    );
    assert_eq!(encoded, expected);
    let frame_view = payload_frame(MessageType::ReadReply, &encoded, 7);
    let decoded = decode_read_reply_payload(&frame_view, canonical.read_sequence, 3)
        .expect("canonical reply");
    assert_eq!(decoded.read_sequence, canonical.read_sequence);
    assert_eq!(decoded.status, ProtocolStatus::Ok);
    assert_eq!(decoded.data, data);
    // The data view aliases the frame payload rather than copying it.
    assert!(std::ptr::eq(
        decoded.data.as_ptr(),
        frame_view.payload()[READ_REPLY_PREFIX_BYTES..].as_ptr()
    ));

    let one_byte = [0x5a_u8];
    let mut minimum = [0_u8; READ_REPLY_PREFIX_BYTES + 1];
    assert!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::Ok,
                data: &one_byte,
            },
            1,
            1,
            &mut minimum
        )
        .is_ok()
    );
    assert!(
        decode_read_reply_payload(&payload_frame(MessageType::ReadReply, &minimum, 1), 1, 1)
            .is_ok()
    );

    let maximum_data = vec![0x5a_u8; MAXIMUM_READ_BYTES as usize];
    let mut maximum_payload = vec![0_u8; MAXIMUM_FRAME_PAYLOAD_BYTES as usize];
    assert!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: u32::MAX,
                status: ProtocolStatus::Ok,
                data: &maximum_data,
            },
            u32::MAX,
            MAXIMUM_READ_BYTES,
            &mut maximum_payload
        )
        .is_ok()
    );
    let maximum_decoded = decode_read_reply_payload(
        &payload_frame(MessageType::ReadReply, &maximum_payload, 1),
        u32::MAX,
        MAXIMUM_READ_BYTES,
    )
    .expect("maximum reply");
    assert_eq!(maximum_decoded.data.len(), maximum_data.len());

    let prefix = [0_u8; READ_REPLY_PREFIX_BYTES];
    for size in 0..READ_REPLY_PREFIX_BYTES {
        assert_eq!(
            decode_read_reply_payload(
                &payload_frame(MessageType::ReadReply, &prefix[..size], 1),
                1,
                1
            ),
            Err(ProtocolError::PayloadUnderflow)
        );
    }

    let mut output = [0_u8; 32];
    let reply_rejects = |message: &ReadReply<'_>, sequence: u32, length: u32| {
        let mut buffer = [0_u8; 32];
        encode_read_reply_payload(message, sequence, length, &mut buffer).is_err()
    };
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 0,
            status: ProtocolStatus::Ok,
            data: &one_byte
        },
        1,
        1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 2,
            status: ProtocolStatus::Ok,
            data: &one_byte
        },
        1,
        1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &one_byte
        },
        0,
        1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &one_byte
        },
        1,
        0
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &one_byte
        },
        1,
        MAXIMUM_READ_BYTES + 1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &[]
        },
        1,
        1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::Ok,
            data: &data
        },
        1,
        2
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::SourceChanged,
            data: &one_byte
        },
        1,
        1
    ));
    assert!(reply_rejects(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::SourceReadFailed,
            data: &one_byte
        },
        1,
        1
    ));
    assert!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::SourceChanged,
                data: &[],
            },
            1,
            1,
            &mut output
        )
        .is_ok()
    );
    assert!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::SourceReadFailed,
                data: &[],
            },
            1,
            1,
            &mut output
        )
        .is_ok()
    );
    for status in [
        ProtocolStatus::Unsupported,
        ProtocolStatus::InvalidRequest,
        ProtocolStatus::ParserRejected,
        ProtocolStatus::BudgetExceeded,
        ProtocolStatus::Cancelled,
        ProtocolStatus::ResultValidationFailed,
        ProtocolStatus::InternalFailure,
    ] {
        assert!(reply_rejects(
            &ReadReply {
                read_sequence: 1,
                status,
                data: &[]
            },
            1,
            1
        ));
    }

    let decode_reply_values = |message_sequence: u32,
                               status: ProtocolStatus,
                               bytes: &[u8],
                               expected_sequence: u32,
                               requested_length: u32| {
        let mut payload = vec![0_u8; READ_REPLY_PREFIX_BYTES + bytes.len()];
        let mut writer = PayloadWriter::new(&mut payload);
        writer.write_u32(message_sequence).expect("fits");
        writer.write_status(status).expect("fits");
        writer.write_bytes(bytes).expect("fits");
        decode_read_reply_payload(
            &payload_frame(MessageType::ReadReply, &payload, 1),
            expected_sequence,
            requested_length,
        )
        .err()
    };
    assert_eq!(
        decode_reply_values(0, ProtocolStatus::Ok, &one_byte, 1, 1),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(2, ProtocolStatus::Ok, &one_byte, 1, 1),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::Ok, &[], 1, 1),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::Ok, &one_byte, 1, 2),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::Ok, &data, 1, 2),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::SourceChanged, &one_byte, 1, 1),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::SourceReadFailed, &one_byte, 1, 1),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::SourceChanged, &[], 1, 1),
        None
    );
    assert_eq!(
        decode_reply_values(1, ProtocolStatus::SourceReadFailed, &[], 1, 1),
        None
    );
    for status in [
        ProtocolStatus::Unsupported,
        ProtocolStatus::InvalidRequest,
        ProtocolStatus::ParserRejected,
        ProtocolStatus::BudgetExceeded,
        ProtocolStatus::Cancelled,
        ProtocolStatus::ResultValidationFailed,
        ProtocolStatus::InternalFailure,
    ] {
        assert_eq!(
            decode_reply_values(1, status, &[], 1, 1),
            Some(ProtocolError::NoncanonicalValue)
        );
    }

    // An unknown status word on the wire is rejected by the reader.
    let mut unknown_status = [0_u8; READ_REPLY_PREFIX_BYTES];
    unknown_status[0] = 1;
    unknown_status[4] = 0xff;
    unknown_status[5] = 0xff;
    assert_eq!(
        decode_read_reply_payload(
            &payload_frame(MessageType::ReadReply, &unknown_status, 1),
            1,
            1
        ),
        Err(ProtocolError::NoncanonicalValue)
    );

    assert_eq!(
        decode_read_reply_payload(&payload_frame(MessageType::ReadReply, &minimum, 1), 0, 1),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_read_reply_payload(&payload_frame(MessageType::ReadReply, &minimum, 1), 1, 0),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_read_reply_payload(
            &payload_frame(MessageType::ReadReply, &minimum, 1),
            1,
            MAXIMUM_READ_BYTES + 1
        ),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_read_reply_payload(&payload_frame(MessageType::ReadRequest, &minimum, 1), 1, 1),
        Err(ProtocolError::UnexpectedMessage)
    );
}

#[test]
fn typed_entry_batch_wire_and_bounds() {
    let policy = maximum_entry_batch_policy();
    let canonical_entries = [EntryBatchEntry {
        source_token: 0x0102_0304_0506_0708,
        size_bytes: 0x0000_0001_1516_1718,
        archive_path: spelling(b"A~"),
    }];
    let canonical_bytes: [u8; 22] = [
        0x01, 0x00, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x18, 0x17, 0x16, 0x15, 0x01,
        0x00, 0x00, 0x00, 0x02, 0x00, b'A', b'~',
    ];
    let mut canonical_output = [0_u8; 22];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &canonical_entries
            },
            &policy,
            &mut canonical_output
        ),
        Ok(canonical_output.len())
    );
    assert_eq!(canonical_output, canonical_bytes);

    let canonical_frame = payload_frame(MessageType::EntryBatch, &canonical_output, 1);
    let mut canonical_storage = [EntryBatchEntry::default(); 1];
    let decoded = decode_entry_batch_payload(&canonical_frame, &policy, &mut canonical_storage)
        .expect("canonical batch");
    assert_eq!(decoded.entries.len(), 1);
    assert_eq!(decoded.entries[0].source_token, 0x0102_0304_0506_0708);
    assert_eq!(decoded.entries[0].size_bytes, 0x0000_0001_1516_1718);
    assert_eq!(decoded.entries[0].archive_path.as_bytes(), b"A~");
    // Each spelling aliases the frame payload.
    assert!(std::ptr::eq(
        decoded.entries[0].archive_path.as_bytes().as_ptr(),
        canonical_frame.payload()[ENTRY_BATCH_PREFIX_BYTES + ENTRY_BATCH_ENTRY_PREFIX_BYTES..]
            .as_ptr()
    ));

    let extreme_entries = [
        EntryBatchEntry {
            source_token: 0,
            size_bytes: 0,
            archive_path: spelling(b" "),
        },
        EntryBatchEntry {
            source_token: u64::MAX,
            size_bytes: MAXIMUM_ENUMERATED_ENTRY_BYTES,
            archive_path: spelling(b"~"),
        },
    ];
    let mut extreme_output = [0_u8; 40];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &extreme_entries
            },
            &policy,
            &mut extreme_output
        ),
        Ok(extreme_output.len())
    );
    let mut extreme_storage = [EntryBatchEntry::default(); 2];
    let extreme = decode_entry_batch_payload(
        &payload_frame(MessageType::EntryBatch, &extreme_output, 1),
        &policy,
        &mut extreme_storage,
    )
    .expect("extreme batch");
    assert_eq!(extreme.entries[0].source_token, 0);
    assert_eq!(extreme.entries[0].archive_path.as_bytes(), b" ");
    assert_eq!(extreme.entries[1].source_token, u64::MAX);
    assert_eq!(
        extreme.entries[1].size_bytes,
        MAXIMUM_ENUMERATED_ENTRY_BYTES
    );
    assert_eq!(extreme.entries[1].archive_path.as_bytes(), b"~");

    // Exactly the maximum entry count.
    let maximum_count_entries: Vec<EntryBatchEntry<'_>> = (0..MAXIMUM_ENTRY_BATCH_ENTRIES)
        .map(|token| EntryBatchEntry {
            source_token: u64::from(token),
            size_bytes: 0,
            archive_path: spelling(b"a"),
        })
        .collect();
    let maximum_count_size = ENTRY_BATCH_PREFIX_BYTES
        + maximum_count_entries.len() * (ENTRY_BATCH_ENTRY_PREFIX_BYTES + 1);
    let mut maximum_count_output = vec![0_u8; maximum_count_size];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &maximum_count_entries
            },
            &policy,
            &mut maximum_count_output
        ),
        Ok(maximum_count_size)
    );
    let mut maximum_count_storage =
        vec![EntryBatchEntry::default(); MAXIMUM_ENTRY_BATCH_ENTRIES as usize];
    let maximum_count = decode_entry_batch_payload(
        &payload_frame(MessageType::EntryBatch, &maximum_count_output, 1),
        &policy,
        &mut maximum_count_storage,
    )
    .expect("maximum count batch");
    assert_eq!(
        maximum_count.entries.len(),
        MAXIMUM_ENTRY_BATCH_ENTRIES as usize
    );
    assert_eq!(maximum_count.entries[0].source_token, 0);
    assert_eq!(
        maximum_count.entries[MAXIMUM_ENTRY_BATCH_ENTRIES as usize - 1].source_token,
        u64::from(MAXIMUM_ENTRY_BATCH_ENTRIES) - 1
    );

    // Count bounds.
    let mut small_output = [0_u8; 2];
    let mut small_storage = [EntryBatchEntry::default(); 1];
    assert_eq!(
        encode_entry_batch_payload(&EntryBatch { entries: &[] }, &policy, &mut small_output),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &[0, 0], 1),
            &policy,
            &mut small_storage
        )
        .err(),
        Some(ProtocolError::NoncanonicalValue)
    );
    let over_count_entries: Vec<EntryBatchEntry<'_>> = (0..=MAXIMUM_ENTRY_BATCH_ENTRIES)
        .map(|token| EntryBatchEntry {
            source_token: u64::from(token),
            size_bytes: 0,
            archive_path: spelling(b"a"),
        })
        .collect();
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &over_count_entries
            },
            &policy,
            &mut []
        ),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &[0x01, 0x01], 1),
            &policy,
            &mut small_storage
        )
        .err(),
        Some(ProtocolError::NoncanonicalValue)
    );

    // Exactly the maximum path length.
    let maximum_path = vec![b'~'; MAXIMUM_ENTRY_BATCH_PATH_BYTES as usize];
    let maximum_path_entries = [EntryBatchEntry {
        source_token: 1,
        size_bytes: 0,
        archive_path: spelling(&maximum_path),
    }];
    let mut maximum_path_output =
        vec![0_u8; ENTRY_BATCH_PREFIX_BYTES + ENTRY_BATCH_ENTRY_PREFIX_BYTES + maximum_path.len()];
    assert!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &maximum_path_entries
            },
            &policy,
            &mut maximum_path_output
        )
        .is_ok()
    );
    let mut maximum_path_storage = [EntryBatchEntry::default(); 1];
    let maximum_path_batch = decode_entry_batch_payload(
        &payload_frame(MessageType::EntryBatch, &maximum_path_output, 1),
        &policy,
        &mut maximum_path_storage,
    )
    .expect("maximum path batch");
    assert_eq!(
        maximum_path_batch.entries[0].archive_path.len(),
        maximum_path.len()
    );

    // Path-length and printable-ASCII bounds on the wire.
    let overlong_path = vec![b'a'; MAXIMUM_ENTRY_BATCH_PATH_BYTES as usize + 1];
    for payload in [
        raw_batch(&[(1, 0, &[])]),
        raw_batch(&[(1, 0, &overlong_path)]),
    ] {
        let mut storage = [EntryBatchEntry::default(); 1];
        let error = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &payload, 1),
            &policy,
            &mut storage,
        )
        .err();
        assert_eq!(error, Some(ProtocolError::NoncanonicalValue));
    }
    for invalid_byte in [0x1f_u8, 0x7f, 0x80] {
        let payload = raw_batch(&[(1, 0, &[invalid_byte])]);
        let mut storage = [EntryBatchEntry::default(); 1];
        let error = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &payload, 1),
            &policy,
            &mut storage,
        )
        .err();
        assert_eq!(error, Some(ProtocolError::NoncanonicalValue));
        assert_eq!(
            ArchiveSpelling::new(&[invalid_byte]),
            Err(ProtocolError::NoncanonicalValue)
        );
    }

    // Printable but unsafe spellings stay verbatim; they carry no authority.
    let unsafe_entries = [
        EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: spelling(b"../x"),
        },
        EntryBatchEntry {
            source_token: 2,
            size_bytes: 0,
            archive_path: spelling(b"/abs"),
        },
        EntryBatchEntry {
            source_token: 3,
            size_bytes: 0,
            archive_path: spelling(b"C:\\x"),
        },
        EntryBatchEntry {
            source_token: 4,
            size_bytes: 0,
            archive_path: spelling(b"a//b"),
        },
    ];
    let mut unsafe_output = vec![0_u8; 128];
    let written = encode_entry_batch_payload(
        &EntryBatch {
            entries: &unsafe_entries,
        },
        &policy,
        &mut unsafe_output,
    )
    .expect("printable unsafe spellings encode");
    let mut unsafe_storage = [EntryBatchEntry::default(); 4];
    let unsafe_batch = decode_entry_batch_payload(
        &payload_frame(MessageType::EntryBatch, &unsafe_output[..written], 1),
        &policy,
        &mut unsafe_storage,
    )
    .expect("printable unsafe spellings decode");
    for (decoded, original) in unsafe_batch.entries.iter().zip(unsafe_entries.iter()) {
        assert_eq!(
            decoded.archive_path.as_bytes(),
            original.archive_path.as_bytes()
        );
    }
}

#[test]
fn typed_entry_batch_policy_and_atomicity() {
    let budget_entries = [
        EntryBatchEntry {
            source_token: 5,
            size_bytes: 2,
            archive_path: spelling(b"ab"),
        },
        EntryBatchEntry {
            source_token: 9,
            size_bytes: 3,
            archive_path: spelling(b"c"),
        },
    ];
    let budget_payload = raw_batch(&[(5, 2, b"ab"), (9, 3, b"c")]);
    let exact_policy = EntryBatchPolicy::new(2, 3, 3, 5, None).expect("valid");
    let mut budget_output = vec![0_u8; budget_payload.len()];
    let mut budget_storage = [EntryBatchEntry::default(); 2];
    assert!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &budget_entries
            },
            &exact_policy,
            &mut budget_output
        )
        .is_ok()
    );
    assert_eq!(budget_output, budget_payload);
    assert!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &budget_payload, 1),
            &exact_policy,
            &mut budget_storage
        )
        .is_ok()
    );

    for exceeded in [
        EntryBatchPolicy::new(1, 3, 3, 5, None).expect("valid"),
        EntryBatchPolicy::new(2, 2, 3, 5, None).expect("valid"),
        EntryBatchPolicy::new(2, 3, 2, 5, None).expect("valid"),
        EntryBatchPolicy::new(2, 3, 3, 4, None).expect("valid"),
    ] {
        assert_eq!(
            encode_entry_batch_payload(
                &EntryBatch {
                    entries: &budget_entries
                },
                &exceeded,
                &mut budget_output
            ),
            Err(ProtocolError::NoncanonicalValue)
        );
        assert_eq!(
            decode_entry_batch_payload(
                &payload_frame(MessageType::EntryBatch, &budget_payload, 1),
                &exceeded,
                &mut budget_storage
            )
            .err(),
            Some(ProtocolError::NoncanonicalValue)
        );
    }

    // Every invalid policy the C++ struct could express is rejected by the
    // constructor instead.
    for invalid in [
        EntryBatchPolicy::new(0, 1, 1, 0, None),
        EntryBatchPolicy::new(MAXIMUM_ENUMERATED_ENTRIES + 1, 1, 1, 0, None),
        EntryBatchPolicy::new(1, 0, 1, 0, None),
        EntryBatchPolicy::new(1, MAXIMUM_ENUMERATED_PATH_BYTES + 1, 1, 0, None),
        EntryBatchPolicy::new(1, 1, 0, 0, None),
        EntryBatchPolicy::new(1, 1, MAXIMUM_ENUMERATED_ENTRY_BYTES + 1, 0, None),
        EntryBatchPolicy::new(1, 1, 1, MAXIMUM_ENUMERATED_TOTAL_BYTES + 1, None),
    ] {
        assert_eq!(invalid, Err(ProtocolError::InvalidBudget));
    }

    // Within-batch ordering.
    let maximum_policy = maximum_entry_batch_policy();
    let increasing = [
        EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: spelling(b"a"),
        },
        EntryBatchEntry {
            source_token: 2,
            size_bytes: 0,
            archive_path: spelling(b"b"),
        },
    ];
    let duplicate = [
        EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: spelling(b"a"),
        },
        EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: spelling(b"b"),
        },
    ];
    let reordered = [
        EntryBatchEntry {
            source_token: 2,
            size_bytes: 0,
            archive_path: spelling(b"a"),
        },
        EntryBatchEntry {
            source_token: 1,
            size_bytes: 0,
            archive_path: spelling(b"b"),
        },
    ];
    assert!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &increasing
            },
            &maximum_policy,
            &mut budget_output
        )
        .is_ok()
    );
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &duplicate
            },
            &maximum_policy,
            &mut budget_output
        ),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &reordered
            },
            &maximum_policy,
            &mut budget_output
        ),
        Err(ProtocolError::NoncanonicalValue)
    );
    for payload in [
        raw_batch(&[(1, 0, b"a"), (1, 0, b"b")]),
        raw_batch(&[(2, 0, b"a"), (1, 0, b"b")]),
    ] {
        let mut storage = [EntryBatchEntry::default(); 2];
        let error = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &payload, 1),
            &maximum_policy,
            &mut storage,
        )
        .err();
        assert_eq!(error, Some(ProtocolError::NoncanonicalValue));
    }

    // Cross-batch replay ordering.
    let prior_policy = EntryBatchPolicy::new(
        MAXIMUM_ENUMERATED_ENTRIES,
        MAXIMUM_ENUMERATED_PATH_BYTES,
        MAXIMUM_ENUMERATED_ENTRY_BYTES,
        MAXIMUM_ENUMERATED_TOTAL_BYTES,
        Some(5),
    )
    .expect("valid");
    let mut cross_batch_storage = [EntryBatchEntry::default(); 1];
    let next = [EntryBatchEntry {
        source_token: 6,
        size_bytes: 0,
        archive_path: spelling(b"n"),
    }];
    assert!(
        encode_entry_batch_payload(
            &EntryBatch { entries: &next },
            &prior_policy,
            &mut budget_output
        )
        .is_ok()
    );
    let next_payload = raw_batch(&[(6, 0, b"n")]);
    assert!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &next_payload, 1),
            &prior_policy,
            &mut cross_batch_storage
        )
        .is_ok()
    );
    for token in [5_u64, 4] {
        let entries = [EntryBatchEntry {
            source_token: token,
            size_bytes: 0,
            archive_path: spelling(b"r"),
        }];
        assert_eq!(
            encode_entry_batch_payload(
                &EntryBatch { entries: &entries },
                &prior_policy,
                &mut budget_output
            ),
            Err(ProtocolError::NoncanonicalValue)
        );
        let replay_payload = raw_batch(&[(token, 0, b"r")]);
        let mut storage = [EntryBatchEntry::default(); 1];
        let error = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &replay_payload, 1),
            &prior_policy,
            &mut storage,
        )
        .err();
        assert_eq!(error, Some(ProtocolError::NoncanonicalValue));
    }
    let maximum_prior_policy = EntryBatchPolicy::new(
        MAXIMUM_ENUMERATED_ENTRIES,
        MAXIMUM_ENUMERATED_PATH_BYTES,
        MAXIMUM_ENUMERATED_ENTRY_BYTES,
        MAXIMUM_ENUMERATED_TOTAL_BYTES,
        Some(u64::MAX),
    )
    .expect("valid");
    let maximum_token_entry = [EntryBatchEntry {
        source_token: u64::MAX,
        size_bytes: 0,
        archive_path: spelling(b"m"),
    }];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &maximum_token_entry
            },
            &maximum_prior_policy,
            &mut budget_output
        ),
        Err(ProtocolError::NoncanonicalValue)
    );

    // Truncation and trailing bytes leave caller storage untouched.
    let single_payload = raw_batch(&[(0, 0, b"a")]);
    assert_eq!(single_payload.len(), 21);
    let sentinel = EntryBatchEntry {
        source_token: 0xa5a5_a5a5_a5a5_a5a5,
        size_bytes: 0xa5a5_a5a5_a5a5_a5a5,
        archive_path: spelling(b"sentinel"),
    };
    let mut atomic_storage;
    for size in 0..single_payload.len() {
        atomic_storage = [sentinel; 2];
        assert_eq!(
            decode_entry_batch_payload(
                &payload_frame(MessageType::EntryBatch, &single_payload[..size], 1),
                &maximum_policy,
                &mut atomic_storage
            )
            .err(),
            Some(ProtocolError::PayloadUnderflow)
        );
        assert!(atomic_storage.iter().all(|entry| *entry == sentinel));
    }
    let mut trailing_payload = single_payload.clone();
    trailing_payload.push(0xa5);
    atomic_storage = [sentinel; 2];
    assert_eq!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &trailing_payload, 1),
            &maximum_policy,
            &mut atomic_storage
        )
        .err(),
        Some(ProtocolError::PayloadTrailingBytes)
    );
    assert!(atomic_storage.iter().all(|entry| *entry == sentinel));

    // Storage capacity handling.
    let two_entry_payload = raw_batch(&[(1, 0, b"a"), (2, 0, b"b")]);
    let mut capacity_storage = [sentinel; 3];
    assert_eq!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &two_entry_payload, 1),
            &maximum_policy,
            &mut capacity_storage[..1]
        )
        .err(),
        Some(ProtocolError::OutputTooSmall)
    );
    assert!(capacity_storage.iter().all(|entry| *entry == sentinel));
    {
        let batch = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &two_entry_payload, 1),
            &maximum_policy,
            &mut capacity_storage[..2],
        )
        .expect("exact capacity");
        assert_eq!(batch.entries.len(), 2);
    }
    assert_eq!(capacity_storage[0].source_token, 1);
    assert_eq!(capacity_storage[1].source_token, 2);
    assert_eq!(capacity_storage[2], sentinel);

    capacity_storage = [sentinel; 3];
    {
        let batch = decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &two_entry_payload, 1),
            &maximum_policy,
            &mut capacity_storage,
        )
        .expect("larger capacity");
        assert_eq!(batch.entries.len(), 2);
    }
    assert_eq!(capacity_storage[2], sentinel);

    // Encoding failures never touch the destination.
    let mut sentinel_output = vec![0xa5_u8; two_entry_payload.len()];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &duplicate
            },
            &maximum_policy,
            &mut sentinel_output
        ),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert!(sentinel_output.iter().all(|byte| *byte == 0xa5));
    let short = two_entry_payload.len() - 1;
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &increasing
            },
            &maximum_policy,
            &mut sentinel_output[..short]
        ),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(sentinel_output.iter().all(|byte| *byte == 0xa5));

    // Exactly the frame ceiling, then one byte over.
    const FRAME_ENTRY_COUNT: usize = 255;
    let exact_path_bytes = MAXIMUM_FRAME_PAYLOAD_BYTES as usize
        - ENTRY_BATCH_PREFIX_BYTES
        - FRAME_ENTRY_COUNT * ENTRY_BATCH_ENTRY_PREFIX_BYTES;
    assert_eq!(exact_path_bytes, 1_043_984);
    let mut frame_paths: Vec<Vec<u8>> = (0..FRAME_ENTRY_COUNT - 1)
        .map(|_| vec![b'a'; MAXIMUM_ENTRY_BATCH_PATH_BYTES as usize])
        .collect();
    let final_path_bytes =
        exact_path_bytes - (FRAME_ENTRY_COUNT - 1) * MAXIMUM_ENTRY_BATCH_PATH_BYTES as usize;
    frame_paths.push(vec![b'b'; final_path_bytes]);
    let frame_entries: Vec<EntryBatchEntry<'_>> = frame_paths
        .iter()
        .enumerate()
        .map(|(index, path)| EntryBatchEntry {
            source_token: index as u64,
            size_bytes: 0,
            archive_path: spelling(path),
        })
        .collect();
    let mut frame_output = vec![0_u8; MAXIMUM_FRAME_PAYLOAD_BYTES as usize];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &frame_entries
            },
            &maximum_policy,
            &mut frame_output
        ),
        Ok(MAXIMUM_FRAME_PAYLOAD_BYTES as usize)
    );

    let mut over_paths = frame_paths;
    over_paths.last_mut().expect("non-empty").push(b'c');
    let over_entries: Vec<EntryBatchEntry<'_>> = over_paths
        .iter()
        .enumerate()
        .map(|(index, path)| EntryBatchEntry {
            source_token: index as u64,
            size_bytes: 0,
            archive_path: spelling(path),
        })
        .collect();
    let mut frame_sentinel = [0xa5_u8; 8];
    assert_eq!(
        encode_entry_batch_payload(
            &EntryBatch {
                entries: &over_entries
            },
            &maximum_policy,
            &mut frame_sentinel
        ),
        Err(ProtocolError::PayloadTooLarge)
    );
    assert!(frame_sentinel.iter().all(|byte| *byte == 0xa5));
}

#[test]
fn typed_entry_batch_state_and_batches() {
    let first_payload = raw_batch(&[(0, 1, b"a"), (2, 2, b"b")]);
    let second_payload = raw_batch(&[(3, 3, b"c"), (u64::MAX, 0, b"d")]);
    let first_policy = EntryBatchPolicy::new(4, 4, 3, 6, None).expect("valid");
    let second_policy = EntryBatchPolicy::new(2, 2, 3, 3, Some(2)).expect("valid");

    let mut validator = handshaked();
    assert_eq!(
        observe(
            &mut validator,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    let mut first_storage = [EntryBatchEntry::default(); 2];
    let first_frame = payload_frame(MessageType::EntryBatch, &first_payload, 1);
    assert!(decode_entry_batch_payload(&first_frame, &first_policy, &mut first_storage).is_ok());
    assert_eq!(
        validator.observe(Direction::WorkerToParent, first_frame.header()),
        Ok(())
    );
    assert_eq!(validator.state(), SessionState::Enumerating);

    let mut second_storage = [EntryBatchEntry::default(); 2];
    let second_frame = payload_frame(MessageType::EntryBatch, &second_payload, 1);
    assert!(decode_entry_batch_payload(&second_frame, &second_policy, &mut second_storage).is_ok());
    assert_eq!(
        validator.observe(Direction::WorkerToParent, second_frame.header()),
        Ok(())
    );
    assert_eq!(validator.state(), SessionState::Enumerating);
    assert_eq!(validator.message_count(), 5);

    let complete_payload = [0x00_u8, 0x00, 0x04, 0x00];
    let complete_frame = payload_frame(MessageType::Complete, &complete_payload, 1);
    assert!(decode_complete_payload(&complete_frame, OperationPhase::Enumerate).is_ok());
    assert_eq!(
        validator.observe(Direction::WorkerToParent, complete_frame.header()),
        Ok(())
    );
    assert_eq!(validator.state(), SessionState::Idle);
    assert_eq!(validator.message_count(), 6);

    // A rejected batch changes neither storage nor session state.
    let mut invalid_validator = handshaked();
    assert_eq!(
        observe(
            &mut invalid_validator,
            Direction::ParentToWorker,
            MessageType::Enumerate,
            1
        ),
        Ok(())
    );
    let replay_policy = EntryBatchPolicy::new(
        MAXIMUM_ENUMERATED_ENTRIES,
        MAXIMUM_ENUMERATED_PATH_BYTES,
        MAXIMUM_ENUMERATED_ENTRY_BYTES,
        MAXIMUM_ENUMERATED_TOTAL_BYTES,
        Some(0),
    )
    .expect("valid");
    let sentinel = EntryBatchEntry {
        source_token: 0xa5a5_a5a5_a5a5_a5a5,
        size_bytes: 0xa5a5_a5a5_a5a5_a5a5,
        archive_path: spelling(b"sentinel"),
    };
    let mut invalid_storage = [sentinel; 2];
    assert_eq!(
        decode_entry_batch_payload(
            &payload_frame(MessageType::EntryBatch, &first_payload, 1),
            &replay_policy,
            &mut invalid_storage
        )
        .err(),
        Some(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(invalid_validator.state(), SessionState::Enumerating);
    assert_eq!(invalid_validator.message_count(), 3);
    assert_eq!(invalid_validator.error(), None);
    assert!(invalid_storage.iter().all(|entry| *entry == sentinel));
}

#[test]
fn typed_data_chunk() {
    let one_byte = [0x5a_u8];
    let mut minimum_output = [0_u8; 1];
    assert_eq!(
        encode_data_chunk_payload(&DataChunk { data: &one_byte }, 1, &mut minimum_output),
        Ok(1)
    );
    assert_eq!(minimum_output, one_byte);
    let minimum_frame = payload_frame(MessageType::DataChunk, &minimum_output, 1);
    let minimum = decode_data_chunk_payload(&minimum_frame, 1).expect("minimum chunk");
    assert!(std::ptr::eq(
        minimum.data.as_ptr(),
        minimum_frame.payload().as_ptr()
    ));

    let maximum_data = vec![0xa6_u8; MAXIMUM_DATA_CHUNK_BYTES];
    let mut maximum_output = vec![0_u8; MAXIMUM_DATA_CHUNK_BYTES];
    assert_eq!(
        encode_data_chunk_payload(
            &DataChunk {
                data: &maximum_data
            },
            maximum_data.len() as u64,
            &mut maximum_output
        ),
        Ok(MAXIMUM_DATA_CHUNK_BYTES)
    );
    let maximum = decode_data_chunk_payload(
        &payload_frame(MessageType::DataChunk, &maximum_output, 1),
        MAXIMUM_DATA_CHUNK_BYTES as u64,
    )
    .expect("maximum chunk");
    assert_eq!(maximum.data.len(), MAXIMUM_DATA_CHUNK_BYTES);

    let two_bytes = [0x12_u8, 0x34];
    assert!(
        decode_data_chunk_payload(&payload_frame(MessageType::DataChunk, &two_bytes, 1), 2).is_ok()
    );
    assert_eq!(
        decode_data_chunk_payload(&payload_frame(MessageType::DataChunk, &two_bytes, 1), 1),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(
        decode_data_chunk_payload(&payload_frame(MessageType::DataChunk, &two_bytes, 1), 0),
        Err(ProtocolError::InvalidBudget)
    );
    assert_eq!(
        decode_data_chunk_payload(&payload_frame(MessageType::DataChunk, &[], 1), 1),
        Err(ProtocolError::NoncanonicalValue)
    );
    let oversized = vec![0x5c_u8; MAXIMUM_DATA_CHUNK_BYTES + 1];
    assert_eq!(
        decode_data_chunk_payload(
            &payload_frame(MessageType::DataChunk, &oversized, 1),
            oversized.len() as u64
        ),
        Err(ProtocolError::NoncanonicalValue)
    );

    let mut declared_longer = frame(MessageType::DataChunk, 1, 2);
    declared_longer.payload_length += 1;
    assert_eq!(
        decode_data_chunk_payload(&FrameView::new(declared_longer, &two_bytes), 0),
        Err(ProtocolError::TruncatedPayload)
    );
    let mut declared_shorter = frame(MessageType::DataChunk, 1, 2);
    declared_shorter.payload_length -= 1;
    assert_eq!(
        decode_data_chunk_payload(&FrameView::new(declared_shorter, &two_bytes), 0),
        Err(ProtocolError::TrailingBytes)
    );
    let mut invalid_header = frame(MessageType::DataChunk, 1, 2);
    invalid_header.flags = 1;
    assert_eq!(
        decode_data_chunk_payload(&FrameView::new(invalid_header, &two_bytes), 0),
        Err(ProtocolError::ReservedFlags)
    );
    assert_eq!(
        decode_data_chunk_payload(&payload_frame(MessageType::ReadReply, &two_bytes, 1), 0),
        Err(ProtocolError::UnexpectedMessage)
    );

    // Encoding failures never touch the destination.
    let mut destination = [0xa5_u8; 2];
    for (message, remainder, expected) in [
        (
            DataChunk { data: &[] },
            1_u64,
            ProtocolError::NoncanonicalValue,
        ),
        (
            DataChunk { data: &oversized },
            oversized.len() as u64,
            ProtocolError::NoncanonicalValue,
        ),
        (
            DataChunk { data: &two_bytes },
            1,
            ProtocolError::NoncanonicalValue,
        ),
        (
            DataChunk { data: &two_bytes },
            0,
            ProtocolError::InvalidBudget,
        ),
    ] {
        destination = [0xa5_u8; 2];
        assert_eq!(
            encode_data_chunk_payload(&message, remainder, &mut destination),
            Err(expected)
        );
        assert!(destination.iter().all(|byte| *byte == 0xa5));
    }
    assert_eq!(
        encode_data_chunk_payload(&DataChunk { data: &two_bytes }, 2, &mut destination[..1]),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(destination.iter().all(|byte| *byte == 0xa5));
}

#[test]
fn typed_complete() {
    let canonical = Complete {
        status: ProtocolStatus::Ok,
        phase: ProtocolPhase::Complete,
    };
    let canonical_bytes = [0x00_u8, 0x00, 0x04, 0x00];
    let statuses = [
        ProtocolStatus::Ok,
        ProtocolStatus::Unsupported,
        ProtocolStatus::InvalidRequest,
        ProtocolStatus::ParserRejected,
        ProtocolStatus::BudgetExceeded,
        ProtocolStatus::Cancelled,
        ProtocolStatus::SourceChanged,
        ProtocolStatus::SourceReadFailed,
        ProtocolStatus::ResultValidationFailed,
        ProtocolStatus::InternalFailure,
    ];
    let phases = [
        ProtocolPhase::Handshake,
        ProtocolPhase::Enumerate,
        ProtocolPhase::Stream,
        ProtocolPhase::SourceRead,
        ProtocolPhase::Complete,
    ];

    for context in [OperationPhase::Enumerate, OperationPhase::Stream] {
        let mut encoded = [0_u8; COMPLETE_PAYLOAD_BYTES];
        assert_eq!(
            encode_complete_payload(&canonical, context, &mut encoded),
            Ok(COMPLETE_PAYLOAD_BYTES)
        );
        assert_eq!(encoded, canonical_bytes);
        assert_eq!(
            decode_complete_payload(&payload_frame(MessageType::Complete, &encoded, 1), context),
            Ok(canonical)
        );

        for status in statuses {
            for phase in phases {
                let mut payload = [0_u8; COMPLETE_PAYLOAD_BYTES];
                let mut writer = PayloadWriter::new(&mut payload);
                writer.write_status(status).expect("fits");
                writer.write_phase(phase).expect("fits");

                let mut destination = [0xa5_u8; COMPLETE_PAYLOAD_BYTES];
                let message = Complete { status, phase };
                let encode = encode_complete_payload(&message, context, &mut destination);
                let decode = decode_complete_payload(
                    &payload_frame(MessageType::Complete, &payload, 1),
                    context,
                );
                if status == ProtocolStatus::Ok && phase == ProtocolPhase::Complete {
                    assert_eq!(encode, Ok(COMPLETE_PAYLOAD_BYTES));
                    assert_eq!(destination, canonical_bytes);
                    assert_eq!(decode, Ok(message));
                } else {
                    assert_eq!(encode, Err(ProtocolError::NoncanonicalValue));
                    assert!(destination.iter().all(|byte| *byte == 0xa5));
                    assert_eq!(decode, Err(ProtocolError::NoncanonicalValue));
                }
            }
        }

        // Unknown enum words on the wire.
        assert_eq!(
            decode_complete_payload(
                &payload_frame(MessageType::Complete, &[0xff, 0xff, 0x04, 0x00], 1),
                context
            ),
            Err(ProtocolError::NoncanonicalValue)
        );
        assert_eq!(
            decode_complete_payload(
                &payload_frame(MessageType::Complete, &[0x00, 0x00, 0xff, 0xff], 1),
                context
            ),
            Err(ProtocolError::NoncanonicalValue)
        );

        // Payload sizes around the exact schema size.
        let shaped = [0x00_u8, 0x00, 0x04, 0x00, 0xa5];
        for size in 0..=shaped.len() {
            let decode = decode_complete_payload(
                &payload_frame(MessageType::Complete, &shaped[..size], 1),
                context,
            );
            if size == COMPLETE_PAYLOAD_BYTES {
                assert_eq!(decode, Ok(canonical));
            } else if size < COMPLETE_PAYLOAD_BYTES {
                assert_eq!(decode, Err(ProtocolError::PayloadUnderflow));
            } else {
                assert_eq!(decode, Err(ProtocolError::PayloadTrailingBytes));
            }
        }

        // Output sizes around the exact schema size.
        for size in 0..=COMPLETE_PAYLOAD_BYTES + 1 {
            let mut destination = [0xa5_u8; COMPLETE_PAYLOAD_BYTES + 1];
            let encode = encode_complete_payload(&canonical, context, &mut destination[..size]);
            if size < COMPLETE_PAYLOAD_BYTES {
                assert_eq!(encode, Err(ProtocolError::OutputTooSmall));
                assert!(destination.iter().all(|byte| *byte == 0xa5));
            } else {
                assert_eq!(encode, Ok(COMPLETE_PAYLOAD_BYTES));
                assert_eq!(destination[..COMPLETE_PAYLOAD_BYTES], canonical_bytes);
                assert_eq!(destination[COMPLETE_PAYLOAD_BYTES], 0xa5);
            }
        }
    }

    // Frame-level rejections.
    let mut declared_longer = frame(MessageType::Complete, 1, 4);
    declared_longer.payload_length += 1;
    assert_eq!(
        decode_complete_payload(
            &FrameView::new(declared_longer, &canonical_bytes),
            OperationPhase::Enumerate
        ),
        Err(ProtocolError::TruncatedPayload)
    );
    let mut declared_shorter = frame(MessageType::Complete, 1, 4);
    declared_shorter.payload_length -= 1;
    assert_eq!(
        decode_complete_payload(
            &FrameView::new(declared_shorter, &canonical_bytes),
            OperationPhase::Enumerate
        ),
        Err(ProtocolError::TrailingBytes)
    );
    let mut invalid_flags = frame(MessageType::Complete, 1, 4);
    invalid_flags.flags = 1;
    assert_eq!(
        decode_complete_payload(
            &FrameView::new(invalid_flags, &canonical_bytes),
            OperationPhase::Enumerate
        ),
        Err(ProtocolError::ReservedFlags)
    );
    assert_eq!(
        decode_complete_payload(
            &payload_frame(MessageType::DataChunk, &canonical_bytes, 1),
            OperationPhase::Enumerate
        ),
        Err(ProtocolError::UnexpectedMessage)
    );

    // Dispatch: an invalid typed payload is never observed by the session.
    let invalid_payload = [0x01_u8, 0x00, 0x04, 0x00];
    for context in [OperationPhase::Enumerate, OperationPhase::Stream] {
        let operation = if context == OperationPhase::Enumerate {
            MessageType::Enumerate
        } else {
            MessageType::StreamEntry
        };
        let state = if context == OperationPhase::Enumerate {
            SessionState::Enumerating
        } else {
            SessionState::Streaming
        };
        let result = result_type(operation);

        let mut normal = handshaked();
        assert_eq!(
            observe(&mut normal, Direction::ParentToWorker, operation, 1),
            Ok(())
        );
        let invalid_frame = payload_frame(MessageType::Complete, &invalid_payload, 1);
        assert_eq!(
            decode_complete_payload(&invalid_frame, context),
            Err(ProtocolError::NoncanonicalValue)
        );
        assert_eq!(normal.state(), state);
        assert_eq!(normal.message_count(), 3);
        assert_eq!(normal.error(), None);
        let valid_frame = payload_frame(MessageType::Complete, &canonical_bytes, 1);
        assert!(decode_complete_payload(&valid_frame, context).is_ok());
        assert_eq!(
            normal.observe(Direction::WorkerToParent, valid_frame.header()),
            Ok(())
        );
        assert_eq!(normal.state(), SessionState::Idle);
        assert_eq!(normal.message_count(), 4);

        let mut cancel_first = handshaked();
        for (direction, message_type) in [
            (Direction::ParentToWorker, operation),
            (Direction::ParentToWorker, MessageType::Cancel),
            (Direction::WorkerToParent, result),
        ] {
            assert_eq!(
                observe(&mut cancel_first, direction, message_type, 1),
                Ok(())
            );
        }
        assert!(decode_complete_payload(&valid_frame, context).is_ok());
        assert_eq!(
            cancel_first.observe(Direction::WorkerToParent, valid_frame.header()),
            Ok(())
        );
        assert_eq!(cancel_first.state(), SessionState::Idle);

        let mut complete_first = handshaked();
        assert_eq!(
            observe(&mut complete_first, Direction::ParentToWorker, operation, 1),
            Ok(())
        );
        assert_eq!(
            complete_first.observe(Direction::WorkerToParent, valid_frame.header()),
            Ok(())
        );
        assert_eq!(
            observe(
                &mut complete_first,
                Direction::ParentToWorker,
                MessageType::Cancel,
                1
            ),
            Ok(())
        );
        assert_eq!(complete_first.state(), SessionState::Idle);
    }
}

#[test]
fn typed_failure_atomicity_and_header_ordering() {
    let mut output = [0xa5_u8; 32];
    assert!(
        encode_hello_payload(
            &Hello {
                source_size: 0,
                maximum_read_bytes: 1
            },
            &mut output
        )
        .is_err()
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));
    assert_eq!(
        encode_hello_payload(
            &Hello {
                source_size: 1,
                maximum_read_bytes: 1
            },
            &mut output[..HELLO_PAYLOAD_BYTES - 1]
        ),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));

    let tiny = SourceReadPolicy::new(1, 1).expect("valid");
    assert!(
        encode_read_request_payload(
            &ReadRequest {
                read_sequence: 1,
                offset: 0,
                length: 0
            },
            &tiny,
            1,
            &mut output
        )
        .is_err()
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));
    assert_eq!(
        encode_read_request_payload(
            &ReadRequest {
                read_sequence: 1,
                offset: 0,
                length: 1
            },
            &tiny,
            1,
            &mut output[..READ_REQUEST_PAYLOAD_BYTES - 1]
        ),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));

    let data = [1_u8];
    assert!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::Cancelled,
                data: &[]
            },
            1,
            1,
            &mut output
        )
        .is_err()
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));
    assert_eq!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::Ok,
                data: &data
            },
            1,
            1,
            &mut output[..READ_REPLY_PREFIX_BYTES]
        ),
        Err(ProtocolError::OutputTooSmall)
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));

    let oversized_reply = vec![1_u8; MAXIMUM_READ_BYTES as usize + 1];
    assert_eq!(
        encode_read_reply_payload(
            &ReadReply {
                read_sequence: 1,
                status: ProtocolStatus::SourceChanged,
                data: &oversized_reply
            },
            1,
            1,
            &mut output
        ),
        Err(ProtocolError::PayloadTooLarge)
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));

    // The global payload ceiling still applies to a typed decoder.
    let oversized = vec![0_u8; MAXIMUM_FRAME_PAYLOAD_BYTES as usize + 1];
    let mut oversized_header = frame(MessageType::Ready, 0, MAXIMUM_FRAME_PAYLOAD_BYTES);
    oversized_header.payload_length = MAXIMUM_FRAME_PAYLOAD_BYTES;
    assert_eq!(
        decode_ready_payload(&FrameView::new(oversized_header, &oversized)),
        Err(ProtocolError::PayloadTooLarge)
    );

    // Header validation precedes payload-length checks.
    let mut valid_hello = [0_u8; HELLO_PAYLOAD_BYTES];
    let mut writer = PayloadWriter::new(&mut valid_hello);
    writer.write_u64(1).expect("fits");
    writer.write_u32(1).expect("fits");

    let mut invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32);
    invalid.major_version += 1;
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::UnsupportedVersion)
    );
    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32);
    invalid.flags = 1;
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::ReservedFlags)
    );
    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32);
    invalid.session_id = 0;
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::InvalidSessionId)
    );
    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32);
    invalid.request_id = 1;
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::InvalidRequestId)
    );
    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32);
    invalid.payload_length = MAXIMUM_FRAME_PAYLOAD_BYTES + 1;
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::PayloadTooLarge)
    );

    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32 + 1);
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::TruncatedPayload)
    );
    invalid = frame(MessageType::Hello, 0, HELLO_PAYLOAD_BYTES as u32 - 1);
    assert_eq!(
        decode_hello_payload(&FrameView::new(invalid, &valid_hello)),
        Err(ProtocolError::TrailingBytes)
    );

    let request_payload = [0_u8; READ_REQUEST_PAYLOAD_BYTES];
    let mut request_header = frame(
        MessageType::ReadRequest,
        1,
        READ_REQUEST_PAYLOAD_BYTES as u32,
    );
    request_header.request_id = 0;
    assert_eq!(
        decode_read_request_payload(&FrameView::new(request_header, &request_payload), &tiny, 1),
        Err(ProtocolError::InvalidRequestId)
    );
    request_header = frame(
        MessageType::ReadRequest,
        1,
        READ_REQUEST_PAYLOAD_BYTES as u32 + 1,
    );
    request_header.flags = 1;
    assert_eq!(
        decode_read_request_payload(&FrameView::new(request_header, &request_payload), &tiny, 1),
        Err(ProtocolError::ReservedFlags)
    );
    request_header = frame(
        MessageType::ReadRequest,
        1,
        READ_REQUEST_PAYLOAD_BYTES as u32 + 1,
    );
    assert_eq!(
        decode_read_request_payload(&FrameView::new(request_header, &request_payload), &tiny, 1),
        Err(ProtocolError::TruncatedPayload)
    );

    let reply_payload = [0_u8; READ_REPLY_PREFIX_BYTES];
    let reply_header = frame(
        MessageType::ReadReply,
        1,
        READ_REPLY_PREFIX_BYTES as u32 - 1,
    );
    assert_eq!(
        decode_read_reply_payload(&FrameView::new(reply_header, &reply_payload), 1, 1),
        Err(ProtocolError::TrailingBytes)
    );

    let ready_header = frame(MessageType::Ready, 0, 1);
    assert_eq!(
        decode_ready_payload(&FrameView::new(ready_header, &[])),
        Err(ProtocolError::TruncatedPayload)
    );

    // Dispatch: the session never observes a message the decoder rejected.
    let mut invalid_hello = [0_u8; HELLO_PAYLOAD_BYTES];
    let mut invalid_writer = PayloadWriter::new(&mut invalid_hello);
    invalid_writer.write_u64(0).expect("fits");
    invalid_writer.write_u32(1).expect("fits");
    let mut validator = validator();
    let invalid_frame = payload_frame(MessageType::Hello, &invalid_hello, 0);
    assert_eq!(
        decode_hello_payload(&invalid_frame),
        Err(ProtocolError::NoncanonicalValue)
    );
    assert_eq!(validator.state(), SessionState::AwaitingHello);
    assert_eq!(validator.message_count(), 0);

    let valid_frame = payload_frame(MessageType::Hello, &valid_hello, 0);
    assert!(decode_hello_payload(&valid_frame).is_ok());
    assert_eq!(
        validator.observe(Direction::ParentToWorker, valid_frame.header()),
        Ok(())
    );
    assert_eq!(validator.state(), SessionState::AwaitingReady);
    assert_eq!(validator.message_count(), 1);
}
