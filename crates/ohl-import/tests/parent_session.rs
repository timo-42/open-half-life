//! The typestate parent session, ported from
//! `tests/media/parser_parent_session_test.cpp`.

mod support;

use std::sync::Arc;

use ohl_import::io::IoError;
use ohl_import::testing::{IoStep, SyntheticTransport};
use ohl_import::{
    CancelStep, CatalogGeneration, ChannelError, FrameBuffer, ImportLimits, RequestStep,
    SessionBuffers, SessionError, SessionPhaseKind, SourceToken, create_parser_session,
    perform_parent_handshake,
};
use ohl_parser_protocol::messages::READ_REPLY_PREFIX_BYTES;
use ohl_parser_protocol::{
    FRAME_HEADER_BYTES, MessageType, OperationPhase, ProtocolError, decode_frame,
};
use support::{
    BatchEntry, Fixture, OpenSession, RecordingSink, budgets, complete_payload, data_chunk_payload,
    deadline, entry_batch_payload, header, import_limits, no_cancellation, open_session,
    read_limits, read_request_payload, ready_frame, session_id, synthetic_bytes, worker_epoch,
};

fn buffers() -> SessionBuffers {
    SessionBuffers::new(read_limits())
}

fn batch_frame(
    request_id: u64,
    entries: &[BatchEntry<'_>],
) -> (ohl_parser_protocol::FrameHeader, Vec<u8>) {
    let payload = entry_batch_payload(entries, import_limits());
    let header = header(
        MessageType::EntryBatch,
        request_id,
        u32::try_from(payload.len()).expect("bounded batch"),
    );
    (header, payload)
}

fn complete_frame(
    request_id: u64,
    phase: OperationPhase,
) -> (ohl_parser_protocol::FrameHeader, Vec<u8>) {
    let payload = complete_payload(phase);
    let header = header(
        MessageType::Complete,
        request_id,
        u32::try_from(payload.len()).expect("fixed complete payload"),
    );
    (header, payload)
}

/// Enumerates one entry and returns the promoted generation.
fn enumerate_one(open: &mut Option<OpenSession>, fixture: &Fixture) -> CatalogGeneration {
    let OpenSession {
        transport,
        channel,
        session,
    } = open.take().expect("session");
    let mut buffers = buffers();
    let session = session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate is legal while idle");
    let request_id = session.active_request_id();

    let (batch, payload) = batch_frame(
        request_id,
        &[BatchEntry {
            source_token: 5,
            archive_path: "valve/pak0.pak",
            size_bytes: 4,
        }],
    );
    transport.push_frame(&batch, &payload);
    let (done, done_payload) = complete_frame(request_id, OperationPhase::Enumerate);
    transport.push_frame(&done, &done_payload);

    let RequestStep::Progress(session) = session
        .receive_one(&mut buffers, None, deadline(), &no_cancellation())
        .expect("a canonical batch")
    else {
        panic!("an entry batch is progress");
    };
    let RequestStep::Complete(session) = session
        .receive_one(&mut buffers, None, deadline(), &no_cancellation())
        .expect("completion")
    else {
        panic!("a complete frame ends the request");
    };
    let generation = session.catalog().expect("a promoted catalog").generation();
    assert_eq!(session.phase(), SessionPhaseKind::Idle);
    let _ = fixture;
    *open = Some(OpenSession {
        transport,
        channel,
        session,
    });
    generation
}

#[test]
fn a_full_enumeration_and_stream_round_trip() {
    let fixture = Fixture::new(4_096);
    let mut open = Some(open_session(&fixture));
    let generation = enumerate_one(&mut open, &fixture);
    let OpenSession {
        transport,
        channel,
        session,
    } = open.take().expect("session");
    let mut buffers = buffers();

    // The enumerate frame really went out.
    let written = transport.written();
    let frame = decode_frame(&written[..FRAME_HEADER_BYTES], session_id().get())
        .expect("a decodable enumerate");
    assert_eq!(frame.header().message_type, MessageType::Enumerate);
    transport.clear_written();

    let session = session
        .begin_stream(generation, SourceToken(5), deadline(), &no_cancellation())
        .expect("a promoted token streams");
    let request_id = session.active_request_id();
    let bytes = synthetic_bytes(4);
    let chunk_payload = data_chunk_payload(&bytes, 4);
    transport.push_frame(
        &header(MessageType::DataChunk, request_id, 4),
        &chunk_payload,
    );
    let (done, done_payload) = complete_frame(request_id, OperationPhase::Stream);
    transport.push_frame(&done, &done_payload);

    let mut sink = RecordingSink::new();
    let RequestStep::Progress(session) = session
        .receive_one(
            &mut buffers,
            Some(&mut sink),
            deadline(),
            &no_cancellation(),
        )
        .expect("a bounded chunk")
    else {
        panic!("a data chunk is progress");
    };
    let RequestStep::Complete(session) = session
        .receive_one(&mut buffers, None, deadline(), &no_cancellation())
        .expect("completion")
    else {
        panic!("a complete frame ends the stream");
    };
    assert_eq!(sink.accepted(), bytes.as_slice());

    let closed = session
        .shutdown(deadline(), &no_cancellation())
        .expect("shutdown is legal while idle");
    assert_eq!(closed.phase(), SessionPhaseKind::Closed);
    drop(closed);
    assert!(
        !channel.is_terminal(),
        "a closed session leaves the channel to its owner"
    );
}

#[test]
fn a_source_read_is_serviced_and_answered_on_the_same_channel() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let mut buffers = buffers();
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    let request_id = session.active_request_id();
    open.transport.clear_written();

    let payload = read_request_payload(1, 32, 16, &fixture.policy());
    open.transport.push_frame(
        &header(
            MessageType::ReadRequest,
            request_id,
            u32::try_from(payload.len()).expect("fixed request payload"),
        ),
        &payload,
    );
    let RequestStep::ReadReplied(session) = session
        .receive_one(&mut buffers, None, deadline(), &no_cancellation())
        .expect("a serviced read")
    else {
        panic!("a read request is answered");
    };
    assert_eq!(session.requests_charged(), 1);

    let written = open.transport.written();
    let reply = decode_frame(&written, session_id().get()).expect("a decodable reply");
    assert_eq!(reply.header().message_type, MessageType::ReadReply);
    assert_eq!(
        &reply.payload()[READ_REPLY_PREFIX_BYTES..],
        &fixture.content()[32..48],
        "the worker is answered with the exact pinned bytes"
    );
}

#[test]
fn a_cancellation_race_is_drained_to_an_acknowledgement() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let mut buffers = buffers();
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    let request_id = session.active_request_id();
    let session = session
        .request_cancel(deadline(), &no_cancellation())
        .expect("cancel is legal while enumerating");
    assert_eq!(session.phase(), SessionPhaseKind::Cancelling);

    // A batch already in flight, a crossed read request, then the ack.
    let (batch, payload) = batch_frame(
        request_id,
        &[BatchEntry {
            source_token: 5,
            archive_path: "valve/late.pak",
            size_bytes: 4,
        }],
    );
    open.transport.push_frame(&batch, &payload);
    let read = read_request_payload(1, 0, 8, &fixture.policy());
    open.transport.push_frame(
        &header(
            MessageType::ReadRequest,
            request_id,
            u32::try_from(read.len()).expect("fixed request payload"),
        ),
        &read,
    );
    open.transport
        .push_frame(&header(MessageType::CancelAck, request_id, 0), &[]);

    let CancelStep::Progress(session) = session
        .receive_one(&mut buffers, None, false, deadline(), &no_cancellation())
        .expect("a crossing batch")
    else {
        panic!("a batch already in flight is progress");
    };
    let CancelStep::ReadIgnored(session) = session
        .receive_one(&mut buffers, None, false, deadline(), &no_cancellation())
        .expect("a crossed read request")
    else {
        panic!("a crossed read is consumed without service");
    };
    assert_eq!(session.requests_charged(), 0, "no source read was serviced");
    let CancelStep::Acknowledged(session) = session
        .receive_one(&mut buffers, None, false, deadline(), &no_cancellation())
        .expect("the acknowledgement")
    else {
        panic!("cancel_ack terminates the request");
    };
    assert_eq!(session.phase(), SessionPhaseKind::Cancelled);
    assert!(session.catalog().is_none());

    let closed = session
        .shutdown(deadline(), &no_cancellation())
        .expect("shutdown follows an acknowledgement");
    assert_eq!(closed.phase(), SessionPhaseKind::Closed);
}

#[test]
fn completion_may_win_the_race_with_cancellation() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let mut buffers = buffers();
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    let request_id = session.active_request_id();
    let session = session
        .request_cancel(deadline(), &no_cancellation())
        .expect("cancel");

    let (done, payload) = complete_frame(request_id, OperationPhase::Enumerate);
    open.transport.push_frame(&done, &payload);
    let CancelStep::Complete(session) = session
        .receive_one(&mut buffers, None, false, deadline(), &no_cancellation())
        .expect("completion wins")
    else {
        panic!("a completion that crossed the cancel ends the request");
    };
    assert_eq!(session.phase(), SessionPhaseKind::Idle);
    assert!(
        session.catalog().is_none(),
        "a cancelled enumeration never promotes"
    );
}

#[test]
fn a_stale_generation_never_streams() {
    let fixture = Fixture::new(4_096);
    let mut open = Some(open_session(&fixture));
    let first = enumerate_one(&mut open, &fixture);
    let second = enumerate_one(&mut open, &fixture);
    assert_ne!(first, second);
    let OpenSession {
        channel, session, ..
    } = open.take().expect("session");

    let terminal = session
        .begin_stream(first, SourceToken(5), deadline(), &no_cancellation())
        .expect_err("a stale generation is refused");
    assert!(matches!(terminal.error(), SessionError::Result(_)));
    assert!(
        channel.is_terminal(),
        "the channel is aborted with the session"
    );
}

#[test]
fn a_refusing_sink_retires_the_session() {
    let fixture = Fixture::new(4_096);
    let mut open = Some(open_session(&fixture));
    let generation = enumerate_one(&mut open, &fixture);
    let OpenSession {
        transport, session, ..
    } = open.take().expect("session");
    let mut buffers = buffers();

    let session = session
        .begin_stream(generation, SourceToken(5), deadline(), &no_cancellation())
        .expect("stream begins");
    let request_id = session.active_request_id();
    let bytes = synthetic_bytes(4);
    transport.push_frame(
        &header(MessageType::DataChunk, request_id, 4),
        &data_chunk_payload(&bytes, 4),
    );
    let mut sink = RecordingSink::refusing();
    let terminal = session
        .receive_one(
            &mut buffers,
            Some(&mut sink),
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a refusing sink retires the session");
    assert!(matches!(terminal.error(), SessionError::Result(_)));
}

#[test]
fn a_send_failure_is_sticky_and_retires_the_session() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    open.transport
        .script_writes([IoStep::Fail(IoError::PeerClosed)]);
    let terminal = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect_err("a failed enumerate");
    assert_eq!(
        terminal.error(),
        SessionError::Channel(ChannelError::Transport(IoError::PeerClosed))
    );
    assert_eq!(
        terminal.channel_failure(),
        Some(ChannelError::Transport(IoError::PeerClosed))
    );
    assert!(terminal.result_failure().is_some());
    assert!(open.channel.is_terminal());
}

#[test]
fn a_worker_frame_out_of_order_is_a_protocol_failure() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let mut buffers = buffers();
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    let request_id = session.active_request_id();
    // A `ready` frame has no place inside an enumeration.
    open.transport.push_frame(&ready_frame(), &[]);
    let terminal = session
        .receive_one(&mut buffers, None, deadline(), &no_cancellation())
        .expect_err("an out-of-order frame");
    assert_eq!(
        terminal.error(),
        SessionError::Protocol(ProtocolError::UnexpectedMessage)
    );
    let _ = request_id;
}

#[test]
fn undersized_buffers_are_refused() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    let mut small = SessionBuffers::new(
        ohl_import::SourceReadLimits::new(64, 8, 1 << 20).expect("smaller quota"),
    );
    let terminal = session
        .receive_one(&mut small, None, deadline(), &no_cancellation())
        .expect_err("buffers below the session's own quota");
    assert_eq!(terminal.error(), SessionError::BuffersTooSmall);
}

#[test]
fn dropping_a_live_session_retires_the_worker() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let channel = Arc::clone(&open.channel);
    let session = open
        .session
        .begin_enumeration(deadline(), &no_cancellation())
        .expect("enumerate");
    drop(session);
    assert!(
        channel.is_terminal(),
        "a dropped live session never abandons the channel"
    );
    assert_eq!(channel.failure(), Some(ChannelError::Aborted));
}

#[test]
fn out_of_band_notifications_retire_the_session() {
    let fixture = Fixture::new(4_096);
    let open = open_session(&fixture);
    let terminal = open.session.notify_worker_failed();
    assert_eq!(terminal.error(), SessionError::WorkerFailure);
    assert!(open.channel.is_terminal());

    let open = open_session(&fixture);
    let terminal = open.session.invalidate_source();
    assert_eq!(terminal.error(), SessionError::SourceInvalidated);
    assert!(open.channel.is_terminal());
}

#[test]
fn a_proof_from_another_channel_never_composes_a_session() {
    let fixture = Fixture::new(4_096);
    let transport = Arc::new(SyntheticTransport::new());
    let channel = Arc::new(ohl_import::FrameChannel::new(
        session_id(),
        Arc::clone(&transport),
    ));
    transport.push_frame(&ready_frame(), &[]);
    let mut buffer = FrameBuffer::new();
    let proof = perform_parent_handshake(
        &channel,
        fixture.media(),
        read_limits(),
        budgets(),
        &mut buffer,
        deadline(),
        &no_cancellation(),
    )
    .expect("handshake");

    let other_transport = Arc::new(SyntheticTransport::new());
    let other = Arc::new(ohl_import::FrameChannel::new(
        session_id(),
        Arc::clone(&other_transport),
    ));
    assert_eq!(
        create_parser_session(
            proof,
            Arc::clone(&other),
            fixture.media(),
            worker_epoch(),
            ImportLimits::default(),
        )
        .expect_err("a foreign proof is refused"),
        SessionError::InvalidConfiguration
    );
    assert!(other.is_terminal(), "the mis-bound channel is aborted");
}
