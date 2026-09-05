//! The parent hello/ready exchange, ported from
//! `tests/media/parser_parent_handshake_test.cpp`.

mod support;

use ohl_import::io::IoError;
use ohl_import::testing::IoStep;
use ohl_import::{
    ChannelError, FrameBuffer, HandshakeError, SourceReadLimits, perform_parent_handshake,
};
use ohl_parser_protocol::messages::HELLO_PAYLOAD_BYTES;
use ohl_parser_protocol::{
    FRAME_HEADER_BYTES, MessageType, ProtocolBudgets, ProtocolError, SessionState, decode_frame,
};
use support::{
    Fixture, READ_BYTES, budgets, deadline, header, new_channel, no_cancellation, read_limits,
    ready_frame, synthetic_bytes,
};

#[test]
fn a_canonical_exchange_produces_a_proof_bound_to_its_channel() {
    let fixture = Fixture::new(4_096);
    let (transport, channel) = new_channel();
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
    .expect("a canonical handshake");

    assert!(proof.matches_channel(&channel));
    assert_eq!(proof.session_id(), channel.session_id());
    assert_eq!(proof.source_read_limits(), read_limits());
    assert_eq!(proof.source_read_policy().source_size(), fixture.size());
    assert_eq!(proof.source_read_policy().maximum_read_bytes(), READ_BYTES);

    // Exactly one hello frame was written, and it advertises the policy.
    let written = transport.written();
    assert_eq!(written.len(), FRAME_HEADER_BYTES + HELLO_PAYLOAD_BYTES);
    let frame = decode_frame(&written, channel.session_id().get()).expect("a decodable hello");
    assert_eq!(frame.header().message_type, MessageType::Hello);
    let hello = ohl_parser_protocol::messages::decode_hello_payload(&frame).expect("typed hello");
    assert_eq!(hello.source_size, fixture.size());
    assert_eq!(hello.maximum_read_bytes, READ_BYTES);

    // The validator the proof carries is idle and already charged.
    let protocol = proof.take_protocol();
    assert_eq!(protocol.state(), SessionState::Idle);
    assert_eq!(protocol.message_count(), 2);
    assert_eq!(protocol.payload_bytes(), HELLO_PAYLOAD_BYTES as u64);
    assert!(!channel.is_terminal());
}

#[test]
fn an_invalid_configuration_is_refused_before_any_io() {
    let fixture = Fixture::new(4_096);
    // Budgets that cannot carry hello and ready.
    let (transport, channel) = new_channel();
    let mut buffer = FrameBuffer::new();
    assert_eq!(
        perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            ProtocolBudgets::new(1, 1 << 20).expect("one-message budget"),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a one-message budget cannot handshake"),
        HandshakeError::InvalidConfiguration
    );
    assert_eq!(transport.call_counts(), (0, 0, 0));
    assert!(!channel.is_terminal());

    // A payload budget below one hello is refused too.
    let (transport, channel) = new_channel();
    assert_eq!(
        perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            ProtocolBudgets::new(64, 4).expect("tiny payload budget"),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a hello must fit the payload budget"),
        HandshakeError::InvalidConfiguration
    );
    assert_eq!(transport.call_counts(), (0, 0, 0));
}

#[test]
fn an_already_poisoned_channel_is_reported_without_io() {
    let fixture = Fixture::new(4_096);
    let (transport, channel) = new_channel();
    channel.abort();
    let mut buffer = FrameBuffer::new();
    assert_eq!(
        perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            budgets(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a poisoned channel cannot handshake"),
        HandshakeError::Channel(ChannelError::Aborted)
    );
    assert_eq!(transport.call_counts().0, 0);
}

#[test]
fn a_transport_failure_aborts_the_channel() {
    let fixture = Fixture::new(4_096);
    for step in [
        IoStep::Fail(IoError::PeerClosed),
        IoStep::Fail(IoError::TimedOut),
        IoStep::Claim(4),
    ] {
        let (transport, channel) = new_channel();
        transport.script_writes([step]);
        let mut buffer = FrameBuffer::new();
        let failure = perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            budgets(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a failed hello");
        assert!(matches!(failure, HandshakeError::Channel(_)));
        assert!(channel.is_terminal());
        assert_eq!(transport.call_counts().2, 1);
    }
}

#[test]
fn a_worker_that_never_answers_fails_the_handshake() {
    let fixture = Fixture::new(4_096);
    let (transport, channel) = new_channel();
    let mut buffer = FrameBuffer::new();
    let failure = perform_parent_handshake(
        &channel,
        fixture.media(),
        read_limits(),
        budgets(),
        &mut buffer,
        deadline(),
        &no_cancellation(),
    )
    .expect_err("a silent worker");
    assert_eq!(
        failure,
        HandshakeError::Channel(ChannelError::Transport(IoError::PeerClosed))
    );
    assert!(channel.is_terminal());
    assert_eq!(transport.call_counts().2, 1);
}

#[test]
fn an_unexpected_answer_is_a_protocol_failure() {
    let fixture = Fixture::new(4_096);
    // A `complete` where `ready` belongs, and a `ready` with trailing bytes.
    for (frame_header, payload, expected) in [
        (
            header(MessageType::Complete, 1, 4),
            vec![0_u8; 4],
            ProtocolError::UnexpectedMessage,
        ),
        (
            header(MessageType::Ready, 0, 4),
            vec![0_u8; 4],
            ProtocolError::PayloadTrailingBytes,
        ),
    ] {
        let (transport, channel) = new_channel();
        transport.push_frame(&frame_header, &payload);
        let mut buffer = FrameBuffer::new();
        let failure = perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            budgets(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a malformed answer");
        assert_eq!(failure, HandshakeError::Protocol(expected));
        assert!(channel.is_terminal());
    }
}

#[test]
fn a_partial_ready_payload_leaves_no_usable_buffer() {
    let fixture = Fixture::new(4_096);
    let (transport, channel) = new_channel();
    // A `ready` header that claims a payload, followed by a short transfer.
    transport.push_frame(&header(MessageType::Ready, 0, 8), &synthetic_bytes(8));
    transport.script_reads([IoStep::Transfer, IoStep::Claim(2)]);
    let mut buffer = FrameBuffer::new();
    let failure = perform_parent_handshake(
        &channel,
        fixture.media(),
        read_limits(),
        budgets(),
        &mut buffer,
        deadline(),
        &no_cancellation(),
    )
    .expect_err("a short ready payload");
    assert!(matches!(failure, HandshakeError::Channel(_)));
    assert!(!buffer.is_usable(), "the receive buffer stays untrusted");
    assert!(channel.is_terminal());
}

#[test]
fn a_proof_only_matches_the_channel_that_made_it() {
    let fixture = Fixture::new(4_096);
    let (transport, channel) = new_channel();
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

    let (_other_transport, other) = new_channel();
    assert!(proof.matches_channel(&channel));
    assert!(
        !proof.matches_channel(&other),
        "a proof never transfers to another channel"
    );
}

#[test]
fn a_read_quota_outside_the_protocol_ceiling_is_refused() {
    assert!(SourceReadLimits::new(0, 8, 1 << 20).is_err());
    let fixture = Fixture::new(0);
    let (_, channel) = new_channel();
    let mut buffer = FrameBuffer::new();
    // A zero-length source cannot support any read policy.
    assert_eq!(
        perform_parent_handshake(
            &channel,
            fixture.media(),
            read_limits(),
            budgets(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("an empty source has no read policy"),
        HandshakeError::InvalidConfiguration
    );
}
