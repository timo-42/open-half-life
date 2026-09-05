//! Result validation and catalog promotion, ported from
//! `tests/media/parser_result_session_test.cpp`.

mod support;

use ohl_import::{
    CatalogGeneration, ImportLimits, LayoutError, ReadRequestOutcome, ResultSession,
    ResultSessionError, SourceToken,
};
use ohl_parser_protocol::messages::{READ_REPLY_PREFIX_BYTES, encode_read_reply_payload};
use ohl_parser_protocol::{
    MessageType, OperationPhase, ProtocolBudgets, ProtocolError, ProtocolStatus, ReadReply,
    SessionId, SessionState, SessionValidator, SourceReadPolicy,
};
use support::{
    BatchEntry, RecordingSink, complete_payload, data_chunk_payload, entry_batch_payload, frame,
    header, idle_validator, import_limits, raw_entry_batch_payload, read_request_payload,
    result_session, session_id, synthetic_bytes, worker_epoch,
};

const REQUEST: u64 = 1;

fn begin(session: &mut ResultSession, request_id: u64) {
    let enumerate = header(MessageType::Enumerate, request_id, 0);
    session
        .begin_enumeration(&frame(&enumerate, &[]))
        .expect("enumerate is legal while idle");
}

fn push_batch(session: &mut ResultSession, request_id: u64, entries: &[BatchEntry<'_>]) {
    let payload = entry_batch_payload(entries, import_limits());
    let batch = header(
        MessageType::EntryBatch,
        request_id,
        u32::try_from(payload.len()).expect("bounded batch"),
    );
    session
        .accept_entry_batch(&frame(&batch, &payload))
        .expect("a canonical batch is accepted");
}

fn complete(
    session: &mut ResultSession,
    request_id: u64,
    phase: OperationPhase,
) -> Result<(), ResultSessionError> {
    let payload = complete_payload(phase);
    let done = header(
        MessageType::Complete,
        request_id,
        u32::try_from(payload.len()).expect("fixed complete payload"),
    );
    match phase {
        OperationPhase::Enumerate => session.complete_enumeration(&frame(&done, &payload)),
        OperationPhase::Stream => session.complete_stream(&frame(&done, &payload)),
    }
}

fn promoted(session: &mut ResultSession, request_id: u64) -> CatalogGeneration {
    begin(session, request_id);
    push_batch(
        session,
        request_id,
        &[
            BatchEntry {
                source_token: 3,
                archive_path: "valve/pak0.pak",
                size_bytes: 8,
            },
            BatchEntry {
                source_token: 9,
                archive_path: "valve/halflife.wad",
                size_bytes: 16,
            },
        ],
    );
    complete(session, request_id, OperationPhase::Enumerate).expect("promotion");
    session.catalog().expect("a promoted catalog").generation()
}

#[test]
fn a_validator_that_has_not_handshaken_cannot_open_a_session() {
    let fresh = SessionValidator::new(session_id(), ProtocolBudgets::default());
    assert_eq!(
        ResultSession::new(fresh, worker_epoch(), ImportLimits::default())
            .expect_err("an unfinished handshake is refused"),
        ResultSessionError::InvalidConfiguration
    );
    assert!(ResultSession::new(idle_validator(), worker_epoch(), ImportLimits::default()).is_ok());
}

#[test]
fn an_enumeration_promotes_a_catalog_that_a_stream_then_binds_to() {
    let mut session = result_session();
    let generation = promoted(&mut session, REQUEST);
    let catalog = session.catalog().expect("catalog");
    assert_eq!(catalog.total_bytes(), 24);
    assert_eq!(catalog.entries().len(), 2);
    assert_eq!(generation.epoch(), worker_epoch());
    assert_eq!(generation.enumeration(), 1);
    assert_eq!(
        catalog
            .find(SourceToken(9))
            .map(|entry| entry.relative_path().as_str().to_owned()),
        Some("/valve/halflife.wad".to_owned())
    );

    let stream_payload = 9_u64.to_le_bytes();
    let stream = header(MessageType::StreamEntry, REQUEST + 1, 8);
    session
        .begin_stream_entry(&frame(&stream, &stream_payload), generation)
        .expect("a promoted token streams");
    assert_eq!(session.remaining_stream_bytes(), 16);

    let mut sink = RecordingSink::new();
    let bytes = synthetic_bytes(16);
    let payload = data_chunk_payload(&bytes, 16);
    let chunk = header(MessageType::DataChunk, REQUEST + 1, 16);
    session
        .accept_data_chunk(&frame(&chunk, &payload), &mut sink)
        .expect("a bounded chunk is accepted");
    assert_eq!(sink.accepted(), bytes.as_slice());
    assert_eq!(session.remaining_stream_bytes(), 0);
    complete(&mut session, REQUEST + 1, OperationPhase::Stream).expect("stream completes");
    assert!(!session.is_terminal());
}

#[test]
fn an_empty_enumeration_promotes_an_empty_catalog() {
    let mut session = result_session();
    begin(&mut session, REQUEST);
    complete(&mut session, REQUEST, OperationPhase::Enumerate).expect("empty promotion");
    let catalog = session.catalog().expect("catalog");
    assert!(catalog.entries().is_empty());
    assert_eq!(catalog.total_bytes(), 0);
}

#[test]
fn a_second_enumeration_retires_the_first_generation() {
    let mut session = result_session();
    let first = promoted(&mut session, REQUEST);
    begin(&mut session, REQUEST + 1);
    // The candidate enumeration retires the promoted catalog immediately.
    assert!(session.catalog().is_none());
    push_batch(
        &mut session,
        REQUEST + 1,
        &[BatchEntry {
            source_token: 3,
            archive_path: "valve/pak0.pak",
            size_bytes: 8,
        }],
    );
    complete(&mut session, REQUEST + 1, OperationPhase::Enumerate).expect("second promotion");
    let second = session.catalog().expect("catalog").generation();
    assert_ne!(first, second);
    assert_eq!(second.enumeration(), 2);

    // A stale generation is refused even though the token still exists.
    let stream_payload = 3_u64.to_le_bytes();
    let stream = header(MessageType::StreamEntry, REQUEST + 2, 8);
    assert_eq!(
        session.begin_stream_entry(&frame(&stream, &stream_payload), first),
        Err(ResultSessionError::UnknownSourceToken)
    );
    assert!(session.is_terminal());
}

#[test]
fn an_unknown_token_is_refused() {
    let mut session = result_session();
    let generation = promoted(&mut session, REQUEST);
    let stream_payload = 4_u64.to_le_bytes();
    let stream = header(MessageType::StreamEntry, REQUEST + 1, 8);
    assert_eq!(
        session.begin_stream_entry(&frame(&stream, &stream_payload), generation),
        Err(ResultSessionError::UnknownSourceToken)
    );
    assert!(session.catalog().is_none());
}

#[test]
fn aliasing_paths_fail_promotion_before_any_destination_exists() {
    let mut session = result_session();
    begin(&mut session, REQUEST);
    push_batch(
        &mut session,
        REQUEST,
        &[
            BatchEntry {
                source_token: 1,
                archive_path: "valve/pak0.pak",
                size_bytes: 1,
            },
            BatchEntry {
                source_token: 2,
                archive_path: "VALVE/PAK0.PAK",
                size_bytes: 1,
            },
        ],
    );
    assert_eq!(
        complete(&mut session, REQUEST, OperationPhase::Enumerate),
        Err(ResultSessionError::Layout(LayoutError::PathConflict))
    );
    assert!(session.is_terminal());
    assert!(session.catalog().is_none());
}

#[test]
fn stream_accounting_rejects_short_and_overlong_streams() {
    let mut session = result_session();
    let generation = promoted(&mut session, REQUEST);
    let stream = header(MessageType::StreamEntry, REQUEST + 1, 8);
    session
        .begin_stream_entry(&frame(&stream, &9_u64.to_le_bytes()), generation)
        .expect("stream begins");

    // Completing before the declared size is delivered is a failure.
    assert_eq!(
        complete(&mut session, REQUEST + 1, OperationPhase::Stream),
        Err(ResultSessionError::IncompleteStream)
    );

    let mut session = result_session();
    let generation = promoted(&mut session, REQUEST);
    session
        .begin_stream_entry(&frame(&stream, &9_u64.to_le_bytes()), generation)
        .expect("stream begins");
    let too_long = synthetic_bytes(17);
    let chunk = header(MessageType::DataChunk, REQUEST + 1, 17);
    assert_eq!(
        session.accept_data_chunk(&frame(&chunk, &too_long), &mut RecordingSink::new()),
        Err(ResultSessionError::Protocol(
            ProtocolError::NoncanonicalValue
        ))
    );
}

#[test]
fn a_refusing_sink_retires_the_session() {
    let mut session = result_session();
    let generation = promoted(&mut session, REQUEST);
    let stream = header(MessageType::StreamEntry, REQUEST + 1, 8);
    session
        .begin_stream_entry(&frame(&stream, &9_u64.to_le_bytes()), generation)
        .expect("stream begins");
    let bytes = synthetic_bytes(16);
    let payload = data_chunk_payload(&bytes, 16);
    let chunk = header(MessageType::DataChunk, REQUEST + 1, 16);
    assert_eq!(
        session.accept_data_chunk(&frame(&chunk, &payload), &mut RecordingSink::refusing()),
        Err(ResultSessionError::DownstreamFailure)
    );
    assert!(session.is_terminal());
}

#[test]
fn a_data_chunk_without_a_stream_is_refused() {
    let mut session = result_session();
    begin(&mut session, REQUEST);
    let bytes = synthetic_bytes(4);
    let payload = data_chunk_payload(&bytes, 4);
    let chunk = header(MessageType::DataChunk, REQUEST, 4);
    assert_eq!(
        session.accept_data_chunk(&frame(&chunk, &payload), &mut RecordingSink::new()),
        Err(ResultSessionError::InvalidState)
    );
}

#[test]
fn source_reads_are_ordered_and_their_status_is_authoritative() {
    let policy = SourceReadPolicy::new(4_096, 1_024).expect("valid policy");
    let mut session = result_session();
    begin(&mut session, REQUEST);

    let payload = read_request_payload(1, 0, 64, &policy);
    let request = header(
        MessageType::ReadRequest,
        REQUEST,
        u32::try_from(payload.len()).expect("fixed request payload"),
    );
    let outcome = session
        .accept_read_request(&frame(&request, &payload), &policy, 1)
        .expect("a canonical read request");
    let ReadRequestOutcome::Serviceable(message) = outcome else {
        panic!("an uncancelled request must be serviceable");
    };
    assert_eq!(message.offset, 0);
    assert_eq!(message.length, 64);

    // A reply announcing a changed source retires the session.
    let mut reply = vec![0_u8; READ_REPLY_PREFIX_BYTES];
    let written = encode_read_reply_payload(
        &ReadReply {
            read_sequence: 1,
            status: ProtocolStatus::SourceChanged,
            data: &[],
        },
        1,
        64,
        &mut reply,
    )
    .expect("encodable reply");
    let reply_header = header(
        MessageType::ReadReply,
        REQUEST,
        u32::try_from(written).expect("bounded reply"),
    );
    assert_eq!(
        session.accept_read_reply(&frame(&reply_header, &reply[..written]), 1, 64),
        Err(ResultSessionError::SourceInvalidated)
    );
    assert!(session.is_terminal());
}

#[test]
fn a_cancel_retires_the_catalog_but_still_validates_crossing_frames() {
    let mut session = result_session();
    promoted(&mut session, REQUEST);
    begin(&mut session, REQUEST + 1);
    let cancel = header(MessageType::Cancel, REQUEST + 1, 0);
    session
        .accept_cancel(&frame(&cancel, &[]))
        .expect("cancel is legal while enumerating");
    assert!(session.catalog().is_none());
    assert_eq!(session.protocol_state(), SessionState::Cancelling);

    // A batch already in flight is still validated, but never promoted.
    push_batch(
        &mut session,
        REQUEST + 1,
        &[BatchEntry {
            source_token: 5,
            archive_path: "valve/late.wad",
            size_bytes: 2,
        }],
    );
    complete(&mut session, REQUEST + 1, OperationPhase::Enumerate)
        .expect("completion wins the race");
    assert!(
        session.catalog().is_none(),
        "a cancelled enumeration never promotes"
    );
}

#[test]
fn a_read_request_crossing_a_cancel_is_consumed_without_service() {
    let policy = SourceReadPolicy::new(4_096, 1_024).expect("valid policy");
    let mut session = result_session();
    begin(&mut session, REQUEST);
    let cancel = header(MessageType::Cancel, REQUEST, 0);
    session
        .accept_cancel(&frame(&cancel, &[]))
        .expect("cancel is legal");

    let payload = read_request_payload(1, 0, 64, &policy);
    let request = header(
        MessageType::ReadRequest,
        REQUEST,
        u32::try_from(payload.len()).expect("fixed request payload"),
    );
    assert_eq!(
        session
            .accept_read_request(&frame(&request, &payload), &policy, 1)
            .expect("a crossed request is consumed"),
        ReadRequestOutcome::IgnoredAfterCancel
    );

    let ack = header(MessageType::CancelAck, REQUEST, 0);
    session
        .accept_cancel_ack(&frame(&ack, &[]))
        .expect("cancellation is acknowledged");
    assert_eq!(session.protocol_state(), SessionState::Cancelled);
    assert!(session.catalog().is_none());
}

#[test]
fn shutdown_and_out_of_band_notifications_retire_everything() {
    let mut session = result_session();
    promoted(&mut session, REQUEST);
    let shutdown = header(MessageType::Shutdown, 0, 0);
    session
        .accept_shutdown(&frame(&shutdown, &[]))
        .expect("shutdown is legal while idle");
    assert!(session.catalog().is_none());
    assert_eq!(session.protocol_state(), SessionState::Closed);

    let mut session = result_session();
    promoted(&mut session, REQUEST);
    session.worker_failed();
    assert_eq!(session.failure(), Some(ResultSessionError::WorkerFailure));
    assert!(session.catalog().is_none());
    // The first failure is retained.
    session.invalidate_source();
    assert_eq!(session.failure(), Some(ResultSessionError::WorkerFailure));

    let mut session = result_session();
    promoted(&mut session, REQUEST);
    session.invalidate_source();
    assert_eq!(
        session.failure(),
        Some(ResultSessionError::SourceInvalidated)
    );
}

#[test]
fn an_out_of_order_batch_is_a_protocol_failure() {
    let mut session = result_session();
    begin(&mut session, REQUEST);
    // Tokens must strictly increase inside one batch.
    let payload = raw_entry_batch_payload(&[
        BatchEntry {
            source_token: 5,
            archive_path: "b",
            size_bytes: 1,
        },
        BatchEntry {
            source_token: 5,
            archive_path: "c",
            size_bytes: 1,
        },
    ]);
    let batch = header(
        MessageType::EntryBatch,
        REQUEST,
        u32::try_from(payload.len()).expect("bounded batch"),
    );
    assert!(matches!(
        session.accept_entry_batch(&frame(&batch, &payload)),
        Err(ResultSessionError::Protocol(_))
    ));
    assert!(session.is_terminal());
}

#[test]
fn a_batch_without_an_enumeration_is_refused() {
    let mut session = result_session();
    let payload = entry_batch_payload(
        &[BatchEntry {
            source_token: 1,
            archive_path: "a",
            size_bytes: 1,
        }],
        import_limits(),
    );
    let batch = header(
        MessageType::EntryBatch,
        REQUEST,
        u32::try_from(payload.len()).expect("bounded batch"),
    );
    assert_eq!(
        session.accept_entry_batch(&frame(&batch, &payload)),
        Err(ResultSessionError::InvalidState)
    );
}

#[test]
fn entry_quotas_bound_one_enumeration() {
    let limits = ImportLimits::new(1, 4_096, 1_024, 2_048).expect("valid limits");
    let mut session = ResultSession::new(idle_validator(), worker_epoch(), limits)
        .expect("an idle validator opens a session");
    begin(&mut session, REQUEST);
    let payload = raw_entry_batch_payload(&[
        BatchEntry {
            source_token: 1,
            archive_path: "a",
            size_bytes: 1,
        },
        BatchEntry {
            source_token: 2,
            archive_path: "b",
            size_bytes: 1,
        },
    ]);
    let batch = header(
        MessageType::EntryBatch,
        REQUEST,
        u32::try_from(payload.len()).expect("bounded batch"),
    );
    assert!(matches!(
        session.accept_entry_batch(&frame(&batch, &payload)),
        Err(ResultSessionError::Protocol(_))
    ));
}

#[test]
fn every_call_on_a_retired_session_reports_the_first_failure() {
    let mut session = result_session();
    session.worker_failed();
    let enumerate = header(MessageType::Enumerate, REQUEST, 0);
    assert_eq!(
        session.begin_enumeration(&frame(&enumerate, &[])),
        Err(ResultSessionError::WorkerFailure)
    );
    assert_eq!(
        complete(&mut session, REQUEST, OperationPhase::Enumerate),
        Err(ResultSessionError::WorkerFailure)
    );
    assert_eq!(session.catalog().map(|_| ()), None);
    assert_eq!(SessionId::new(0), None);
}
