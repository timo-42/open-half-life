//! Worker ownership and identity allocation, ported from
//! `tests/media/parser_process_session_test.cpp`.

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use ohl_import::io::IoError;
use ohl_import::process_session::SessionConfig;
use ohl_import::testing::{FakeWorker, IoStep, SyntheticTransport, WorkerCall};
use ohl_import::{
    AllocatorExhausted, FrameBuffer, OpenError, ProcessSession, ProcessState, SessionIdAllocator,
    WorkerExit,
};
use ohl_parser_protocol::{FrameHeader, MessageType, SessionId};
use support::{Fixture, budgets, deadline, import_limits, no_cancellation, read_limits};

fn config() -> SessionConfig {
    SessionConfig {
        source_read_limits: read_limits(),
        protocol_budgets: budgets(),
        import_limits: import_limits(),
    }
}

fn push_ready(transport: &SyntheticTransport, session_id: SessionId) {
    transport.push_frame(
        &FrameHeader::new(MessageType::Ready, session_id.get(), 0, 0),
        &[],
    );
}

fn worker() -> (Arc<SyntheticTransport>, FakeWorker) {
    let transport = Arc::new(SyntheticTransport::new());
    let worker = FakeWorker::new(Arc::clone(&transport));
    (transport, worker)
}

#[test]
fn the_allocator_issues_unique_non_zero_identities() {
    let mut allocator = SessionIdAllocator::new();
    let mut sessions = BTreeSet::new();
    let mut epochs = BTreeSet::new();
    for expected in 1..=64_u64 {
        let allocation = allocator.allocate().expect("a fresh identity");
        assert_eq!(allocation.session_id.get(), expected);
        assert_eq!(allocation.worker_epoch.get(), expected);
        assert!(sessions.insert(allocation.session_id.get()));
        assert!(epochs.insert(allocation.worker_epoch.get()));
    }
}

#[test]
fn the_allocator_fails_closed_instead_of_wrapping() {
    let mut allocator = SessionIdAllocator::starting_at(u64::MAX, u64::MAX);
    let last = allocator.allocate().expect("the last identity");
    assert_eq!(last.session_id.get(), u64::MAX);
    for _ in 0..4 {
        assert_eq!(allocator.allocate(), Err(AllocatorExhausted));
    }

    // A zero start is already exhausted, so zero is never handed out.
    let mut zeroed = SessionIdAllocator::starting_at(0, 1);
    assert_eq!(zeroed.allocate(), Err(AllocatorExhausted));
    let mut zero_epoch = SessionIdAllocator::starting_at(1, 0);
    assert_eq!(zero_epoch.allocate(), Err(AllocatorExhausted));
}

#[test]
fn an_open_then_orderly_shutdown_closes_and_reaps_in_order() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    let mut allocator = SessionIdAllocator::new();
    let allocation = allocator.allocate().expect("identity");
    let mut process = ProcessSession::new(worker.clone(), allocation);
    assert_eq!(process.state(), ProcessState::Idle);

    let session_id = process.session_id();
    push_ready(&transport, session_id);
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("a canonical open");
    assert_eq!(process.state(), ProcessState::Open);
    assert_eq!(session.session_id(), session_id);

    let exit = process
        .orderly_shutdown(session, deadline(), &no_cancellation())
        .expect("an orderly shutdown");
    assert_eq!(exit, WorkerExit::Exited(0));
    assert_eq!(process.state(), ProcessState::Closed);
    assert_eq!(
        worker.calls(),
        vec![WorkerCall::CloseChannel, WorkerCall::Wait],
        "protocol shutdown, then close, then wait"
    );
    assert_eq!(worker.terminate_calls(), 0);
    drop(process);
    assert_eq!(
        worker.terminate_calls(),
        0,
        "a cleanly reaped worker is never terminated"
    );
}

#[test]
fn a_repeated_shutdown_returns_the_cached_outcome() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    push_ready(&transport, process.session_id());
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("open");
    let first = process
        .orderly_shutdown(session, deadline(), &no_cancellation())
        .expect("shutdown");
    let calls = worker.calls();

    for _ in 0..3 {
        assert_eq!(process.finish(deadline()), Ok(first));
    }
    assert_eq!(
        worker.calls(),
        calls,
        "a repeated shutdown never touches the worker again"
    );
    assert_eq!(worker.terminate_calls(), 0);
}

#[test]
fn a_failed_handshake_terminates_the_worker_exactly_once() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    // The worker never answers the hello.
    transport.script_reads([IoStep::Fail(IoError::PeerClosed)]);
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    let mut buffer = FrameBuffer::new();
    let failure = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("a silent worker fails the handshake");
    assert!(matches!(failure.error(), OpenError::Handshake(_)));
    assert_eq!(failure.termination(), Some(Ok(WorkerExit::Terminated)));
    assert_eq!(process.state(), ProcessState::Terminated);
    assert_eq!(worker.terminate_calls(), 1);

    // Neither a retry nor the destructor terminates a second time.
    let retried = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect_err("open is not retryable");
    assert_eq!(retried.error(), OpenError::InvalidState);
    assert_eq!(process.terminate(deadline()), Ok(WorkerExit::Terminated));
    drop(process);
    assert_eq!(worker.terminate_calls(), 1);
}

#[test]
fn a_shutdown_send_failure_escalates_to_a_single_termination() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    push_ready(&transport, process.session_id());
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("open");

    transport.script_writes([IoStep::Fail(IoError::PeerClosed)]);
    let failure = process
        .orderly_shutdown(session, deadline(), &no_cancellation())
        .expect_err("the shutdown frame never leaves");
    assert_eq!(failure.termination(), Ok(WorkerExit::Terminated));
    assert_eq!(process.state(), ProcessState::Terminated);
    assert_eq!(worker.terminate_calls(), 1);
    assert!(
        !worker.calls().contains(&WorkerCall::Wait),
        "a failed shutdown never waits for an orderly exit"
    );
    drop(process);
    assert_eq!(worker.terminate_calls(), 1);
}

#[test]
fn a_reap_timeout_escalates_to_a_single_termination() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    worker.set_wait(Err(IoError::TimedOut));
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    push_ready(&transport, process.session_id());
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("open");

    let failure = process
        .orderly_shutdown(session, deadline(), &no_cancellation())
        .expect_err("the worker never exits");
    assert_eq!(failure.termination(), Ok(WorkerExit::Terminated));
    assert_eq!(worker.terminate_calls(), 1);
    assert_eq!(
        worker.calls(),
        vec![
            WorkerCall::CloseChannel,
            WorkerCall::Wait,
            WorkerCall::TerminateAndWait
        ]
    );
    drop(process);
    assert_eq!(worker.terminate_calls(), 1);
}

#[test]
fn dropping_a_live_process_session_terminates_the_worker() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    push_ready(&transport, process.session_id());
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("open");
    drop(session);
    assert_eq!(worker.terminate_calls(), 0);
    drop(process);
    assert_eq!(
        worker.terminate_calls(),
        1,
        "the destructor never abandons a live worker"
    );
}

#[test]
fn a_never_opened_process_session_is_still_reaped() {
    let (_transport, worker) = worker();
    {
        let process = ProcessSession::new(worker.clone(), support::allocation());
        assert_eq!(process.state(), ProcessState::Idle);
    }
    assert_eq!(worker.terminate_calls(), 1);
}

#[test]
fn a_failed_session_is_abandoned_with_one_termination() {
    let fixture = Fixture::new(4_096);
    let (transport, worker) = worker();
    let mut process = ProcessSession::new(worker.clone(), support::allocation());
    push_ready(&transport, process.session_id());
    let mut buffer = FrameBuffer::new();
    let session = process
        .open(
            fixture.media(),
            config(),
            &mut buffer,
            deadline(),
            &no_cancellation(),
        )
        .expect("open");
    let terminal = session.notify_worker_failed();
    assert_eq!(
        process.abandon(terminal, deadline()),
        Ok(WorkerExit::Terminated)
    );
    assert_eq!(worker.terminate_calls(), 1);
    drop(process);
    assert_eq!(worker.terminate_calls(), 1);
}
