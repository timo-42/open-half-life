//! Property tests: encode/decode identity and total (panic-free) decoding.

#![allow(
    clippy::cast_possible_truncation,
    clippy::comparison_chain,
    clippy::elidable_lifetime_names,
    clippy::items_after_statements,
    clippy::too_many_lines
)]

use proptest::prelude::*;

use ohl_parser_protocol::messages::{
    Complete, DataChunk, EntryBatch, EntryBatchEntry, EntryBatchPolicy, Hello,
    MAXIMUM_DATA_CHUNK_BYTES, MAXIMUM_ENTRY_BATCH_ENTRIES, MAXIMUM_ENUMERATED_ENTRIES,
    MAXIMUM_ENUMERATED_ENTRY_BYTES, MAXIMUM_ENUMERATED_PATH_BYTES, MAXIMUM_ENUMERATED_TOTAL_BYTES,
    MAXIMUM_READ_BYTES, OperationPhase, ReadReply, ReadRequest, SourceReadPolicy, StreamEntry,
    decode_complete_payload, decode_data_chunk_payload, decode_entry_batch_payload,
    decode_hello_payload, decode_read_reply_payload, decode_read_request_payload,
    decode_stream_entry_payload, encode_complete_payload, encode_data_chunk_payload,
    encode_entry_batch_payload, encode_hello_payload, encode_read_reply_payload,
    encode_read_request_payload, encode_stream_entry_payload,
};
use ohl_parser_protocol::{
    ArchiveSpelling, Direction, FRAME_HEADER_BYTES, FrameHeader, FrameView,
    MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType, PayloadReader, ProtocolBudgets, ProtocolStatus,
    SessionId, SessionValidator, decode_frame, encode_frame,
};

const SESSION: u64 = 0x0102_0304_0506_0708;

fn payload_frame<'a>(
    message_type: MessageType,
    payload: &'a [u8],
    request_id: u64,
) -> FrameView<'a> {
    FrameView::new(
        FrameHeader::new(
            message_type,
            SESSION,
            request_id,
            u32::try_from(payload.len()).expect("bounded"),
        ),
        payload,
    )
}

fn any_message_type() -> impl Strategy<Value = MessageType> {
    prop::sample::select(vec![
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
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Frames round trip byte for byte.
    #[test]
    fn frame_round_trip(
        request_id in 1_u64..=u64::MAX,
        session_id in 1_u64..=u64::MAX,
        payload in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let header = FrameHeader::new(
            MessageType::DataChunk,
            session_id,
            request_id,
            u32::try_from(payload.len()).expect("bounded"),
        );
        let mut encoded = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
        let written = encode_frame(&header, &payload, &mut encoded).expect("valid frame");
        prop_assert_eq!(written, encoded.len());
        let decoded = decode_frame(&encoded, session_id).expect("valid frame");
        prop_assert_eq!(*decoded.header(), header);
        prop_assert_eq!(decoded.payload(), &payload[..]);
    }

    /// `hello` round trips for every in-policy value pair.
    #[test]
    fn hello_round_trip(
        source_size in 1_u64..=u64::MAX,
        maximum_read_bytes in 1_u32..=MAXIMUM_READ_BYTES,
    ) {
        let message = Hello { source_size, maximum_read_bytes };
        let mut encoded = [0_u8; 12];
        encode_hello_payload(&message, &mut encoded).expect("valid hello");
        let decoded = decode_hello_payload(&payload_frame(MessageType::Hello, &encoded, 0));
        prop_assert_eq!(decoded, Ok(message));
    }

    /// `stream_entry` round trips for every opaque token.
    #[test]
    fn stream_entry_round_trip(source_token in any::<u64>()) {
        let message = StreamEntry { source_token };
        let mut encoded = [0_u8; 8];
        encode_stream_entry_payload(&message, &mut encoded).expect("valid entry");
        let decoded =
            decode_stream_entry_payload(&payload_frame(MessageType::StreamEntry, &encoded, 1));
        prop_assert_eq!(decoded, Ok(message));
    }

    /// `read_request` round trips for every in-policy read.
    #[test]
    fn read_request_round_trip(
        read_sequence in 1_u32..=u32::MAX,
        offset in 0_u64..1_000_000,
        length in 1_u32..=4096,
    ) {
        let policy = SourceReadPolicy::new(offset + u64::from(length), length).expect("valid");
        let message = ReadRequest { read_sequence, offset, length };
        let mut encoded = [0_u8; 16];
        encode_read_request_payload(&message, &policy, read_sequence, &mut encoded)
            .expect("in-policy read");
        let decoded = decode_read_request_payload(
            &payload_frame(MessageType::ReadRequest, &encoded, 1),
            &policy,
            read_sequence,
        );
        prop_assert_eq!(decoded, Ok(message));
    }

    /// `read_reply` round trips and its data aliases the frame.
    #[test]
    fn read_reply_round_trip(
        read_sequence in 1_u32..=u32::MAX,
        data in prop::collection::vec(any::<u8>(), 1..1024),
    ) {
        let requested_length = u32::try_from(data.len()).expect("bounded");
        let message = ReadReply {
            read_sequence,
            status: ProtocolStatus::Ok,
            data: &data,
        };
        let mut encoded = vec![0_u8; 6 + data.len()];
        encode_read_reply_payload(&message, read_sequence, requested_length, &mut encoded)
            .expect("valid reply");
        let frame = payload_frame(MessageType::ReadReply, &encoded, 1);
        let decoded = decode_read_reply_payload(&frame, read_sequence, requested_length)
            .expect("valid reply");
        prop_assert_eq!(decoded.read_sequence, read_sequence);
        prop_assert_eq!(decoded.data, &data[..]);
        prop_assert!(std::ptr::eq(decoded.data.as_ptr(), frame.payload()[6..].as_ptr()));
    }

    /// `data_chunk` round trips for every bounded, non-empty chunk.
    #[test]
    fn data_chunk_round_trip(data in prop::collection::vec(any::<u8>(), 1..4096)) {
        let remainder = data.len() as u64;
        let mut encoded = vec![0_u8; data.len()];
        encode_data_chunk_payload(&DataChunk { data: &data }, remainder, &mut encoded)
            .expect("valid chunk");
        let decoded = decode_data_chunk_payload(
            &payload_frame(MessageType::DataChunk, &encoded, 1),
            remainder,
        )
        .expect("valid chunk");
        prop_assert_eq!(decoded.data, &data[..]);
    }

    /// `entry_batch` round trips for strictly increasing, in-policy batches.
    #[test]
    fn entry_batch_round_trip(
        tokens in prop::collection::btree_set(0_u64..1_000_000, 1..16),
        sizes in prop::collection::vec(0_u64..1_000_000, 16),
        path_lengths in prop::collection::vec(1_usize..32, 16),
    ) {
        let paths: Vec<Vec<u8>> = path_lengths
            .iter()
            .take(tokens.len())
            .map(|length| vec![b'a'; *length])
            .collect();
        let entries: Vec<EntryBatchEntry<'_>> = tokens
            .iter()
            .zip(sizes.iter())
            .zip(paths.iter())
            .map(|((token, size), path)| EntryBatchEntry {
                source_token: *token,
                size_bytes: *size,
                archive_path: ArchiveSpelling::new(path).expect("printable"),
            })
            .collect();
        let policy = EntryBatchPolicy::new(
            MAXIMUM_ENUMERATED_ENTRIES,
            MAXIMUM_ENUMERATED_PATH_BYTES,
            MAXIMUM_ENUMERATED_ENTRY_BYTES,
            MAXIMUM_ENUMERATED_TOTAL_BYTES,
            None,
        )
        .expect("valid");
        let mut encoded = vec![0_u8; 2 + entries.len() * (18 + 32)];
        let written = encode_entry_batch_payload(
            &EntryBatch { entries: &entries },
            &policy,
            &mut encoded,
        )
        .expect("in-policy batch");
        let frame = payload_frame(MessageType::EntryBatch, &encoded[..written], 1);
        let mut storage =
            vec![EntryBatchEntry::default(); MAXIMUM_ENTRY_BATCH_ENTRIES as usize];
        let decoded = decode_entry_batch_payload(&frame, &policy, &mut storage)
            .expect("in-policy batch");
        prop_assert_eq!(decoded.entries.len(), entries.len());
        for (decoded, original) in decoded.entries.iter().zip(entries.iter()) {
            prop_assert_eq!(decoded.source_token, original.source_token);
            prop_assert_eq!(decoded.size_bytes, original.size_bytes);
            prop_assert_eq!(
                decoded.archive_path.as_bytes(),
                original.archive_path.as_bytes()
            );
        }
    }

    /// `complete` round trips; only the canonical pair is accepted.
    #[test]
    fn complete_round_trip(enumerate in any::<bool>()) {
        let context = if enumerate { OperationPhase::Enumerate } else { OperationPhase::Stream };
        let message = Complete {
            status: ohl_parser_protocol::ProtocolStatus::Ok,
            phase: ohl_parser_protocol::ProtocolPhase::Complete,
        };
        let mut encoded = [0_u8; 4];
        encode_complete_payload(&message, context, &mut encoded).expect("canonical");
        let decoded =
            decode_complete_payload(&payload_frame(MessageType::Complete, &encoded, 1), context);
        prop_assert_eq!(decoded, Ok(message));
    }

    /// Frame decoding never panics on arbitrary bytes, and never yields a
    /// frame whose payload disagrees with its header.
    #[test]
    fn frame_decoding_is_total(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(frame) = decode_frame(&bytes, 0) {
            prop_assert_eq!(frame.payload().len(), frame.header().payload_length as usize);
            prop_assert!(frame.header().validate().is_ok());
            prop_assert!(bytes.len() <= FRAME_HEADER_BYTES + MAXIMUM_FRAME_PAYLOAD_BYTES as usize);
        }
    }

    /// Every typed decoder is total on arbitrary payload bytes.
    #[test]
    fn typed_decoding_is_total(
        message_type in any_message_type(),
        request_id in any::<u64>(),
        declared in any::<u32>(),
        payload in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut header = FrameHeader::new(message_type, SESSION, request_id, declared);
        header.payload_length = declared;
        let frame = FrameView::new(header, &payload);
        let policy = SourceReadPolicy::new(4096, 4096).expect("valid");
        let batch_policy = EntryBatchPolicy::new(
            MAXIMUM_ENUMERATED_ENTRIES,
            MAXIMUM_ENUMERATED_PATH_BYTES,
            MAXIMUM_ENUMERATED_ENTRY_BYTES,
            MAXIMUM_ENUMERATED_TOTAL_BYTES,
            None,
        )
        .expect("valid");
        let mut storage =
            vec![EntryBatchEntry::default(); MAXIMUM_ENTRY_BATCH_ENTRIES as usize];

        let _ = decode_hello_payload(&frame);
        let _ = decode_stream_entry_payload(&frame);
        let _ = decode_read_request_payload(&frame, &policy, 1);
        let _ = decode_read_reply_payload(&frame, 1, 1);
        let _ = decode_entry_batch_payload(&frame, &batch_policy, &mut storage);
        let _ = decode_data_chunk_payload(&frame, MAXIMUM_DATA_CHUNK_BYTES as u64);
        let _ = decode_complete_payload(&frame, OperationPhase::Enumerate);

        // The bounded reader never reads past the payload either.
        let mut reader = PayloadReader::new(&payload);
        while reader.remaining() >= 8 {
            if reader.read_u64().is_err() {
                break;
            }
        }
        let _ = reader.finish();
    }

    /// The session validator is total on arbitrary header sequences and
    /// stays sticky once it fails.
    #[test]
    fn session_validation_is_total(
        steps in prop::collection::vec(
            (any::<bool>(), any_message_type(), any::<u8>(), any::<u16>()),
            0..64,
        ),
    ) {
        let mut validator = SessionValidator::new(
            SessionId::new(SESSION).expect("non-zero"),
            ProtocolBudgets::new(64, 64 * 1024).expect("valid"),
        );
        let mut failed = false;
        for (from_parent, message_type, request_id, payload_length) in steps {
            let direction = if from_parent {
                Direction::ParentToWorker
            } else {
                Direction::WorkerToParent
            };
            let header = FrameHeader::new(
                message_type,
                SESSION,
                u64::from(request_id),
                u32::from(payload_length),
            );
            let observed = validator.observe(direction, &header);
            if failed {
                prop_assert!(observed.is_err());
            }
            failed |= observed.is_err();
            prop_assert_eq!(failed, validator.error().is_some() || failed);
        }
    }
}
