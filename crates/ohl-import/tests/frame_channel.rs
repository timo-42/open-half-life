//! Frame-channel behaviour, ported from `tests/media/parser_frame_channel_test.cpp`.

mod support;

use std::sync::Arc;
use std::thread;

use ohl_import::io::{CancellationSource, IoError};
use ohl_import::testing::{IoStep, SyntheticTransport};
use ohl_import::{ChannelError, FrameBuffer, FrameChannel};
use ohl_parser_protocol::messages::MAXIMUM_READ_BYTES;
use ohl_parser_protocol::{
    FRAME_HEADER_BYTES, FrameHeader, MAXIMUM_FRAME_PAYLOAD_BYTES, MessageType, ProtocolError,
    SessionId, encode_frame,
};
use support::{
    SESSION, deadline, header, new_channel, no_cancellation, session_id, synthetic_bytes,
};

fn encoded(header: &FrameHeader, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
    let written = encode_frame(header, payload, &mut bytes).expect("encodable fixture frame");
    bytes.truncate(written);
    bytes
}

#[test]
fn a_canonical_frame_is_sent_header_first_and_received_back() {
    let (transport, channel) = new_channel();
    let payload = synthetic_bytes(64);
    let outgoing = header(MessageType::EntryBatch, 7, 64);
    channel
        .send(&outgoing, &payload, deadline(), &no_cancellation())
        .expect("canonical send");
    assert_eq!(transport.written(), encoded(&outgoing, &payload));

    let incoming = header(MessageType::DataChunk, 7, 32);
    let chunk = synthetic_bytes(32);
    transport.push_frame(&incoming, &chunk);
    let mut buffer = FrameBuffer::new();
    let frame = channel
        .receive(&mut buffer, deadline(), &no_cancellation())
        .expect("canonical receive");
    assert_eq!(*frame.header(), incoming);
    assert_eq!(frame.payload(), chunk.as_slice());
    assert!(!channel.is_terminal());
}

#[test]
fn a_maximum_payload_round_trips() {
    let (transport, channel) = new_channel();
    let payload = synthetic_bytes(MAXIMUM_FRAME_PAYLOAD_BYTES as usize);
    let outgoing = header(MessageType::EntryBatch, 1, MAXIMUM_FRAME_PAYLOAD_BYTES);
    channel
        .send(&outgoing, &payload, deadline(), &no_cancellation())
        .expect("maximum payload send");

    transport.push_frame(&outgoing, &payload);
    let mut buffer = FrameBuffer::new();
    let frame = channel
        .receive(&mut buffer, deadline(), &no_cancellation())
        .expect("maximum payload receive");
    assert_eq!(frame.payload().len(), MAXIMUM_FRAME_PAYLOAD_BYTES as usize);
    assert_eq!(frame.payload(), payload.as_slice());
}

#[test]
fn outgoing_validation_rejects_before_any_io() {
    let payload = synthetic_bytes(8);
    let cases = [
        (
            FrameHeader::new(MessageType::EntryBatch, SESSION ^ 1, 1, 8),
            ProtocolError::WrongSessionId,
        ),
        (
            header(MessageType::EntryBatch, 1, 9),
            ProtocolError::NoncanonicalValue,
        ),
        (
            header(MessageType::EntryBatch, 0, 8),
            ProtocolError::InvalidRequestId,
        ),
        (
            header(MessageType::Hello, 3, 8),
            ProtocolError::InvalidRequestId,
        ),
    ];
    for (outgoing, expected) in cases {
        let (transport, channel) = new_channel();
        let failure = channel
            .send(&outgoing, &payload, deadline(), &no_cancellation())
            .expect_err("invalid header");
        assert_eq!(failure, ChannelError::Protocol(expected));
        // Nothing was written, and the channel is terminally poisoned.
        assert_eq!(transport.call_counts(), (0, 0, 1));
        assert_eq!(channel.failure(), Some(failure));
    }
}

#[test]
fn incoming_headers_are_rejected_before_payload_io() {
    let good = encoded(&header(MessageType::EntryBatch, 1, 0), &[]);
    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    let mut wrong_version = good.clone();
    wrong_version[4] = 2;
    let mut reserved_flags = good.clone();
    reserved_flags[10] = 1;
    let mut unknown_type = good.clone();
    unknown_type[8] = 0xff;
    let mut oversized = good.clone();
    oversized[12..16].copy_from_slice(&(MAXIMUM_FRAME_PAYLOAD_BYTES + 1).to_le_bytes());
    let mut zero_session = good.clone();
    zero_session[16..24].copy_from_slice(&0_u64.to_le_bytes());
    let mut other_session = good.clone();
    other_session[16..24].copy_from_slice(&(SESSION ^ 1).to_le_bytes());

    for (bytes, expected) in [
        (wrong_magic, ProtocolError::InvalidMagic),
        (wrong_version, ProtocolError::UnsupportedVersion),
        (reserved_flags, ProtocolError::ReservedFlags),
        (unknown_type, ProtocolError::UnknownMessageType),
        (oversized, ProtocolError::PayloadTooLarge),
        (zero_session, ProtocolError::InvalidSessionId),
        (other_session, ProtocolError::WrongSessionId),
    ] {
        let (transport, channel) = new_channel();
        transport.push_bytes(&bytes);
        let mut buffer = FrameBuffer::new();
        let failure = channel
            .receive(&mut buffer, deadline(), &no_cancellation())
            .expect_err("rejected header");
        assert_eq!(failure, ChannelError::Protocol(expected));
        assert!(channel.is_terminal());
        // A rejected header never begins payload mutation.
        assert!(buffer.is_usable());
        assert_eq!(transport.call_counts().0, 1);
    }
}

#[test]
fn a_zero_session_id_cannot_build_a_channel() {
    // The C++ `invalid_configuration` result is unrepresentable here: the
    // constructor takes a validated `SessionId` and an `ExactIo` value.
    assert!(SessionId::new(0).is_none());
}

#[test]
fn impossible_and_failed_transfers_are_terminal() {
    let payload = synthetic_bytes(16);
    let outgoing = header(MessageType::EntryBatch, 1, 16);
    for (step, expected) in [
        (IoStep::Claim(FRAME_HEADER_BYTES - 1), IoError::IoFailure),
        (IoStep::Claim(0), IoError::IoFailure),
        (IoStep::Claim(FRAME_HEADER_BYTES + 1), IoError::IoFailure),
        (IoStep::Fail(IoError::TimedOut), IoError::TimedOut),
        (IoStep::Fail(IoError::Cancelled), IoError::Cancelled),
        (IoStep::Fail(IoError::PeerClosed), IoError::PeerClosed),
        (IoStep::Fail(IoError::IoFailure), IoError::IoFailure),
    ] {
        let (transport, channel) = new_channel();
        transport.script_writes([step]);
        let failure = channel
            .send(&outgoing, &payload, deadline(), &no_cancellation())
            .expect_err("failed transfer");
        assert_eq!(failure, ChannelError::Transport(expected));
        assert_eq!(channel.failure(), Some(failure));
        // The transport is aborted exactly once, however often we retry.
        let retried = channel
            .send(&outgoing, &payload, deadline(), &no_cancellation())
            .expect_err("poisoned channel");
        assert_eq!(retried, failure);
        assert_eq!(transport.call_counts().2, 1);
    }
}

#[test]
fn a_partial_payload_invalidates_the_entire_buffer() {
    let (transport, channel) = new_channel();
    transport.push_frame(&header(MessageType::DataChunk, 1, 32), &synthetic_bytes(32));
    transport.script_reads([IoStep::Transfer, IoStep::Claim(8)]);
    let mut buffer = FrameBuffer::new();
    let failure = channel
        .receive(&mut buffer, deadline(), &no_cancellation())
        .expect_err("short payload");
    assert_eq!(failure, ChannelError::Transport(IoError::IoFailure));
    assert!(!buffer.is_usable());

    // Even on a fresh channel the buffer stays refused until reinitialized.
    let (fresh_transport, fresh) = new_channel();
    fresh_transport.push_frame(&header(MessageType::DataChunk, 1, 4), &synthetic_bytes(4));
    assert_eq!(
        fresh.receive(&mut buffer, deadline(), &no_cancellation()),
        Err(ChannelError::BufferInvalidated)
    );
    assert!(!fresh.is_terminal());
    buffer.reinit();
    assert!(
        fresh
            .receive(&mut buffer, deadline(), &no_cancellation())
            .is_ok()
    );
}

#[test]
fn one_direction_admits_one_operation_at_a_time() {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = FrameChannel::new(session_id(), Arc::clone(&transport));
    transport.script_writes([IoStep::Block]);
    let outgoing = header(MessageType::EntryBatch, 1, 0);

    thread::scope(|scope| {
        let blocked = scope.spawn(|| {
            channel
                .send(&outgoing, &[], deadline(), &no_cancellation())
                .expect_err("aborted send")
        });
        transport.await_blocked(1);
        assert_eq!(
            channel.send(&outgoing, &[], deadline(), &no_cancellation()),
            Err(ChannelError::ConcurrentOperation)
        );
        channel.abort();
        assert_eq!(
            blocked.join().expect("joined send"),
            ChannelError::Aborted,
            "the retained abort wins over the interrupted transfer"
        );
    });
}

#[test]
fn a_send_and_a_receive_may_overlap() {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = FrameChannel::new(session_id(), Arc::clone(&transport));
    transport.block_when_empty(true);
    let outgoing = header(MessageType::EntryBatch, 1, 0);
    let incoming = header(MessageType::DataChunk, 1, 4);

    thread::scope(|scope| {
        let receiver = scope.spawn(|| {
            let mut buffer = FrameBuffer::new();
            let frame = channel
                .receive(&mut buffer, deadline(), &no_cancellation())
                .expect("overlapping receive");
            (*frame.header(), frame.payload().to_vec())
        });
        transport.await_blocked(1);
        channel
            .send(&outgoing, &[], deadline(), &no_cancellation())
            .expect("overlapping send");
        transport.push_frame(&incoming, &synthetic_bytes(4));
        let (frame_header, frame_payload) = receiver.join().expect("joined receive");
        assert_eq!(frame_header, incoming);
        assert_eq!(frame_payload, synthetic_bytes(4));
    });
    assert!(!channel.is_terminal());
}

#[test]
fn a_failure_in_one_direction_wakes_the_other_and_the_first_failure_is_retained() {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = FrameChannel::new(session_id(), Arc::clone(&transport));
    transport.block_when_empty(true);
    transport.script_writes([IoStep::Fail(IoError::PeerClosed)]);
    let outgoing = header(MessageType::EntryBatch, 1, 0);

    thread::scope(|scope| {
        let receiver = scope.spawn(|| {
            let mut buffer = FrameBuffer::new();
            channel
                .receive(&mut buffer, deadline(), &no_cancellation())
                .expect_err("woken receive")
        });
        transport.await_blocked(1);
        assert_eq!(
            channel.send(&outgoing, &[], deadline(), &no_cancellation()),
            Err(ChannelError::Transport(IoError::PeerClosed))
        );
        assert_eq!(
            receiver.join().expect("joined receive"),
            ChannelError::Transport(IoError::PeerClosed),
            "the retained first failure is reported to both directions"
        );
    });
    assert_eq!(
        channel.failure(),
        Some(ChannelError::Transport(IoError::PeerClosed))
    );
    assert_eq!(transport.call_counts().2, 1);
}

#[test]
fn abort_wakes_both_active_operations_exactly_once() {
    let transport = Arc::new(SyntheticTransport::new());
    let channel = FrameChannel::new(session_id(), Arc::clone(&transport));
    transport.block_when_empty(true);
    transport.script_writes([IoStep::Block]);
    let outgoing = header(MessageType::EntryBatch, 1, 0);

    thread::scope(|scope| {
        let receiver = scope.spawn(|| {
            let mut buffer = FrameBuffer::new();
            channel
                .receive(&mut buffer, deadline(), &no_cancellation())
                .expect_err("aborted receive")
        });
        let sender = scope.spawn(|| {
            channel
                .send(&outgoing, &[], deadline(), &no_cancellation())
                .expect_err("aborted send")
        });
        transport.await_blocked(2);
        channel.abort();
        channel.abort();
        assert_eq!(
            receiver.join().expect("joined receive"),
            ChannelError::Aborted
        );
        assert_eq!(sender.join().expect("joined send"), ChannelError::Aborted);
    });
    assert_eq!(channel.failure(), Some(ChannelError::Aborted));
    assert_eq!(transport.call_counts().2, 1);
}

#[test]
fn a_cancelled_or_expired_transfer_is_terminal() {
    let (transport, channel) = new_channel();
    transport.block_when_empty(true);
    let mut buffer = FrameBuffer::new();
    assert_eq!(
        channel.receive(&mut buffer, support::expired_deadline(), &no_cancellation()),
        Err(ChannelError::Transport(IoError::TimedOut))
    );

    let (transport, channel) = new_channel();
    transport.block_when_empty(true);
    let source = CancellationSource::new();
    source.cancel();
    let mut buffer = FrameBuffer::new();
    assert_eq!(
        channel.receive(&mut buffer, deadline(), &source.token()),
        Err(ChannelError::Transport(IoError::Cancelled))
    );
}

#[test]
fn the_read_ceiling_stays_inside_one_frame() {
    // The broker's largest reply must still fit one frame.
    const { assert!(MAXIMUM_READ_BYTES < MAXIMUM_FRAME_PAYLOAD_BYTES) }
}
