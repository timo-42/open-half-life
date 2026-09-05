//! Source-read brokering, ported from
//! `tests/media/parser_source_read_broker_test.cpp`.

mod support;

use ohl_import::testing::ScriptedSourceOps;
use ohl_import::{
    PrepareOutcome, ResultSession, ResultSessionError, SourceReadBroker, SourceReadError,
    SourceReadLimits,
};
use ohl_parser_protocol::messages::READ_REPLY_PREFIX_BYTES;
use ohl_parser_protocol::{MessageType, ProtocolStatus, SessionState};
use ohl_platform::MediaSourceError;
use support::{
    Fixture, READ_BYTES, frame, header, read_limits, read_request_payload, result_session,
};

const REQUEST: u64 = 1;

struct Buffers {
    scratch: Vec<u8>,
    reply: Vec<u8>,
}

impl Buffers {
    fn new(limits: SourceReadLimits) -> Self {
        Self {
            scratch: vec![0; limits.maximum_read_bytes() as usize],
            reply: vec![0; limits.reply_storage_bytes()],
        }
    }
}

fn enumerating(session: &mut ResultSession) {
    let enumerate = header(MessageType::Enumerate, REQUEST, 0);
    session
        .begin_enumeration(&frame(&enumerate, &[]))
        .expect("enumerate is legal while idle");
}

fn request_frame(fixture: &Fixture, sequence: u32, offset: u64, length: u32) -> (Vec<u8>, u32) {
    let payload = read_request_payload(sequence, offset, length, &fixture.policy());
    let size = u32::try_from(payload.len()).expect("fixed request payload");
    (payload, size)
}

#[test]
fn a_serviced_read_returns_the_exact_source_bytes_and_then_advances_the_sequence() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let mut broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 16, 64);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let prepared = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("a canonical read is serviced");
    let PrepareOutcome::ReplyReady(prepared) = prepared else {
        panic!("an uncancelled read must produce a reply");
    };
    assert_eq!(prepared.status(), ProtocolStatus::Ok);
    assert_eq!(prepared.header().message_type, MessageType::ReadReply);
    assert_eq!(
        &prepared.payload()[READ_REPLY_PREFIX_BYTES..],
        &fixture.content()[16..80],
        "the reply carries the exact pinned bytes"
    );
    // The scratch prefix is scrubbed before the reply is handed over.
    assert!(buffers.scratch[..64].iter().all(|byte| *byte == 0));

    broker
        .commit_reply_sent(prepared, &mut session)
        .expect("the transport delivered the reply");
    assert_eq!(broker.requests_charged(), 1);
    assert!(!broker.reply_is_pending());

    // The next read of the same request must use sequence 2.
    let (payload, size) = request_frame(&fixture, 2, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    assert!(matches!(
        broker.prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply
        ),
        Ok(PrepareOutcome::ReplyReady(_))
    ));
    assert_eq!(broker.requests_charged(), 2);
}

#[test]
fn a_wrong_sequence_or_out_of_range_window_retires_the_broker() {
    let fixture = Fixture::new(1_024);
    for (sequence, offset, length) in [(2, 0, 8), (1, 1_024, 8), (1, 1_020, 8)] {
        let mut session = result_session();
        let mut broker =
            SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
        let mut buffers = Buffers::new(read_limits());
        enumerating(&mut session);

        // Build the frame against a policy that admits it, then present it to
        // a broker that expects sequence 1 over the real source size.
        let policy = ohl_parser_protocol::SourceReadPolicy::new(1 << 20, READ_BYTES)
            .expect("permissive fixture policy");
        let payload = {
            let mut payload = vec![0_u8; 16];
            let written = ohl_parser_protocol::messages::encode_read_request_payload(
                &ohl_parser_protocol::ReadRequest {
                    read_sequence: sequence,
                    offset,
                    length,
                },
                &policy,
                sequence,
                &mut payload,
            )
            .expect("encodable hostile request");
            payload.truncate(written);
            payload
        };
        let request = header(
            MessageType::ReadRequest,
            REQUEST,
            u32::try_from(payload.len()).expect("fixed request payload"),
        );
        let failure = broker
            .prepare(
                &mut session,
                &frame(&request, &payload),
                &mut buffers.scratch,
                &mut buffers.reply,
            )
            .expect_err("an out-of-policy request is refused");
        assert!(matches!(failure, SourceReadError::Protocol(_)));
        assert!(broker.is_terminal());
        assert!(session.is_terminal());
    }
}

#[test]
fn the_request_and_byte_quotas_bound_one_session() {
    let fixture = Fixture::new(4_096);
    let limits = SourceReadLimits::new(READ_BYTES, 1, READ_REPLY_PREFIX_BYTES as u64 + 4_096)
        .expect("one-read quota");
    let mut session = result_session();
    let mut broker = SourceReadBroker::new(fixture.media(), &mut session, limits).expect("broker");
    let mut buffers = Buffers::new(limits);
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let prepared = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("the first read fits the quota");
    let PrepareOutcome::ReplyReady(prepared) = prepared else {
        panic!("reply expected");
    };
    broker
        .commit_reply_sent(prepared, &mut session)
        .expect("delivered");

    let (payload, size) = request_frame(&fixture, 2, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    assert_eq!(
        broker
            .prepare(
                &mut session,
                &frame(&request, &payload),
                &mut buffers.scratch,
                &mut buffers.reply
            )
            .expect_err("the second read exceeds the quota"),
        SourceReadError::RequestBudgetExceeded
    );
    assert!(broker.is_terminal());
    assert!(session.is_terminal());
}

#[test]
fn undersized_buffers_are_refused_without_retiring_anything() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let mut broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 64);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let mut scratch = vec![0_u8; 8];
    let mut reply = vec![0_u8; 8];
    assert_eq!(
        broker
            .prepare(
                &mut session,
                &frame(&request, &payload),
                &mut scratch,
                &mut reply
            )
            .expect_err("too small"),
        SourceReadError::OutputTooSmall
    );
    assert!(!broker.is_terminal());
    assert!(!session.is_terminal());
    assert_eq!(broker.requests_charged(), 0);

    // With adequate buffers the same request now succeeds.
    let mut buffers = Buffers::new(read_limits());
    assert!(matches!(
        broker.prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply
        ),
        Ok(PrepareOutcome::ReplyReady(_))
    ));
}

#[test]
fn a_second_prepare_while_a_reply_is_pending_is_refused() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let mut broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    let mut buffers = Buffers::new(read_limits());
    let mut second = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let prepared = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("first prepare");
    assert!(broker.reply_is_pending());
    assert_eq!(
        broker
            .prepare(
                &mut session,
                &frame(&request, &payload),
                &mut second.scratch,
                &mut second.reply
            )
            .expect_err("a pending reply blocks the next prepare"),
        SourceReadError::ReplyPending
    );
    assert!(!broker.is_terminal());
    let PrepareOutcome::ReplyReady(prepared) = prepared else {
        panic!("reply expected");
    };
    broker
        .commit_reply_sent(prepared, &mut session)
        .expect("delivered");
    assert!(!broker.reply_is_pending());
}

#[test]
fn an_abandoned_reply_retires_the_broker_and_the_session() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let mut broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let PrepareOutcome::ReplyReady(prepared) = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("prepare")
    else {
        panic!("reply expected");
    };
    assert_eq!(
        broker.abandon_reply(prepared, &mut session),
        SourceReadError::TransportAbandoned
    );
    assert!(broker.is_terminal());
    assert_eq!(session.failure(), Some(ResultSessionError::WorkerFailure));
}

#[test]
fn a_changed_source_answers_with_a_status_and_then_retires_the_session() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let ops = ScriptedSourceOps::new();
    ops.script_verify([Err(MediaSourceError::Changed)]);
    let mut broker = SourceReadBroker::with_ops(fixture.media(), &mut session, read_limits(), ops)
        .expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let PrepareOutcome::ReplyReady(prepared) = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("a source change is answered, not hidden")
    else {
        panic!("reply expected");
    };
    assert_eq!(prepared.status(), ProtocolStatus::SourceChanged);
    assert!(prepared.payload()[READ_REPLY_PREFIX_BYTES..].is_empty());
    assert_eq!(
        broker
            .commit_reply_sent(prepared, &mut session)
            .expect_err("the session ends once the worker knows"),
        SourceReadError::SourceChanged
    );
    assert_eq!(
        session.failure(),
        Some(ResultSessionError::SourceInvalidated)
    );
}

#[test]
fn a_stable_source_that_refuses_a_read_reports_a_read_failure() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let ops = ScriptedSourceOps::new();
    ops.script_reads([Err(MediaSourceError::ReadFailed)]);
    let mut broker = SourceReadBroker::with_ops(fixture.media(), &mut session, read_limits(), ops)
        .expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let PrepareOutcome::ReplyReady(prepared) = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("prepared")
    else {
        panic!("reply expected");
    };
    assert_eq!(prepared.status(), ProtocolStatus::SourceReadFailed);
    assert_eq!(
        broker
            .commit_reply_sent(prepared, &mut session)
            .expect_err("a read failure ends the session"),
        SourceReadError::SourceReadFailure
    );
    assert_eq!(
        session.failure(),
        Some(ResultSessionError::SourceReadFailure)
    );
}

#[test]
fn a_read_that_races_a_truncation_is_reported_as_a_change() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let ops = ScriptedSourceOps::new();
    // The read fails and the following verification also fails: the change
    // wins over the read error.
    ops.script_reads([Err(MediaSourceError::UnexpectedEof)]);
    ops.script_verify([Ok(()), Err(MediaSourceError::Changed)]);
    let mut broker = SourceReadBroker::with_ops(fixture.media(), &mut session, read_limits(), ops)
        .expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let PrepareOutcome::ReplyReady(prepared) = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("prepared")
    else {
        panic!("reply expected");
    };
    assert_eq!(prepared.status(), ProtocolStatus::SourceChanged);
    let _ = broker.commit_reply_sent(prepared, &mut session);
}

#[test]
fn a_request_crossing_a_cancel_is_consumed_without_touching_the_source() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let ops = ScriptedSourceOps::new();
    let mut broker = SourceReadBroker::with_ops(fixture.media(), &mut session, read_limits(), ops)
        .expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);
    let cancel = header(MessageType::Cancel, REQUEST, 0);
    session
        .accept_cancel(&frame(&cancel, &[]))
        .expect("cancel is legal");
    assert_eq!(session.protocol_state(), SessionState::Cancelling);

    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    assert!(matches!(
        broker
            .prepare(
                &mut session,
                &frame(&request, &payload),
                &mut buffers.scratch,
                &mut buffers.reply
            )
            .expect("a crossed request is consumed"),
        PrepareOutcome::IgnoredAfterCancel
    ));
    assert_eq!(broker.requests_charged(), 0);
    assert!(!broker.reply_is_pending());
    assert!(!broker.is_terminal());
}

#[test]
fn a_broker_cannot_be_built_over_a_retired_session() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    session.worker_failed();
    assert_eq!(
        SourceReadBroker::new(fixture.media(), &mut session, read_limits())
            .expect_err("a retired session refuses a broker"),
        SourceReadError::InvalidConfiguration
    );

    // A session that already started a request is not idle either.
    let mut session = result_session();
    enumerating(&mut session);
    assert_eq!(
        SourceReadBroker::new(fixture.media(), &mut session, read_limits())
            .expect_err("a busy session refuses a broker"),
        SourceReadError::InvalidConfiguration
    );
    assert!(session.is_terminal());
}

#[test]
fn invalid_limits_are_rejected_before_any_broker_exists() {
    assert!(SourceReadLimits::new(0, 8, 1 << 20).is_err());
    assert!(SourceReadLimits::new(READ_BYTES, 0, 1 << 20).is_err());
    // The reply quota must hold at least one maximum-size reply.
    assert!(SourceReadLimits::new(READ_BYTES, 8, 8).is_err());
    assert!(SourceReadLimits::new(READ_BYTES, 8, 1 << 20).is_ok());
}

#[test]
fn retiring_a_broker_retires_a_live_session() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    broker.retire(&mut session);
    assert_eq!(session.failure(), Some(ResultSessionError::WorkerFailure));

    // A closed session is left alone.
    let mut session = result_session();
    let broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    let shutdown = header(MessageType::Shutdown, 0, 0);
    session
        .accept_shutdown(&frame(&shutdown, &[]))
        .expect("shutdown is legal while idle");
    broker.retire(&mut session);
    assert!(!session.is_terminal());
}

#[test]
fn a_prepared_reply_never_exposes_source_bytes_in_its_debug_output() {
    let fixture = Fixture::new(4_096);
    let mut session = result_session();
    let mut broker =
        SourceReadBroker::new(fixture.media(), &mut session, read_limits()).expect("broker");
    let mut buffers = Buffers::new(read_limits());
    enumerating(&mut session);
    let (payload, size) = request_frame(&fixture, 1, 0, 8);
    let request = header(MessageType::ReadRequest, REQUEST, size);
    let PrepareOutcome::ReplyReady(prepared) = broker
        .prepare(
            &mut session,
            &frame(&request, &payload),
            &mut buffers.scratch,
            &mut buffers.reply,
        )
        .expect("prepared")
    else {
        panic!("reply expected");
    };
    assert_eq!(
        prepared.payload().len(),
        READ_REPLY_PREFIX_BYTES + 8,
        "the reply is exactly prefix plus payload"
    );
    let rendered = format!("{:?}", prepared.status());
    assert!(!rendered.contains("byte"));
}
