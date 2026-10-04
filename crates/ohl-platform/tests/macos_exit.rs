//! Cross-platform deterministic execution of the private macOS wait algorithm.
//! This does not claim macOS kernel or sandbox execution on other hosts.

use ohl_platform::{IsolatedWorkerError, IsolatedWorkerExitKind};
use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

#[path = "../src/isolated_worker/macos_exit.rs"]
mod macos_exit;

use macos_exit::{ExitSource, ExitState, Status, wait_until};

const MS: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation {
    Observe(Instant),
    Poll,
    Terminate,
    Backoff(Duration),
}

struct FakeChild {
    state: ExitState,
    now: Instant,
    watch_time: Duration,
    poll_time: Duration,
    oversleep: Duration,
    observations: VecDeque<Result<bool, ()>>,
    statuses: VecDeque<io::Result<Option<Status>>>,
    operations: Vec<Operation>,
    signal_sent: bool,
}

impl FakeChild {
    fn new(statuses: impl IntoIterator<Item = io::Result<Option<Status>>>) -> Self {
        Self {
            state: ExitState::default(),
            now: Instant::now(),
            watch_time: Duration::ZERO,
            poll_time: Duration::ZERO,
            oversleep: Duration::ZERO,
            observations: [Ok(true)].into(),
            statuses: statuses.into_iter().collect(),
            operations: Vec::new(),
            signal_sent: false,
        }
    }

    fn wait_for(
        &mut self,
        duration: Duration,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        let deadline = self.now + duration;
        wait_until(self, deadline)
    }
}

impl ExitSource for FakeChild {
    fn exit_state(&mut self) -> &mut ExitState {
        &mut self.state
    }

    fn observe_exit(&mut self, deadline: Instant) -> Result<bool, ()> {
        self.operations.push(Operation::Observe(deadline));
        self.now += self.watch_time;
        self.observations
            .pop_front()
            .expect("no second notification after the exit was observed")
    }

    fn poll_status(&mut self) -> io::Result<Option<Status>> {
        self.operations.push(Operation::Poll);
        self.now += self.poll_time;
        self.statuses
            .pop_front()
            .expect("bounded status polls; no poll after the terminal cache")
    }

    fn termination_sent(&self) -> bool {
        self.signal_sent
    }

    fn request_termination(&mut self) {
        self.operations.push(Operation::Terminate);
        self.signal_sent = true;
    }

    fn now(&self) -> Instant {
        self.now
    }

    fn backoff(&mut self, duration: Duration) {
        assert!(!duration.is_zero(), "pending status must not spin");
        self.operations.push(Operation::Backoff(duration));
        self.now += duration + self.oversleep;
    }
}

fn status(code: Option<i32>, signal: Option<i32>) -> Status {
    Status { code, signal }
}

#[test]
fn pending_status_is_retried_then_clean_status_is_cached() {
    let mut child = FakeChild::new([Ok(None), Ok(None), Ok(Some(status(Some(0), None)))]);
    let start = child.now;
    let deadline = start + 10 * MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(child.now, start + 2 * MS);
    assert!(child.state.observed);
    assert_eq!(child.state.reaped, Some(IsolatedWorkerExitKind::Clean));
    assert_eq!(child.state.terminating_signal, None);
    assert!(!child.signal_sent);
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
        ]
    );
    let before = child.operations.clone();
    assert_eq!(
        wait_until(&mut child, start),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(
        child.operations, before,
        "cached wait performs no OS operation"
    );
}

#[test]
fn timeout_retains_observation_and_allows_later_reap_without_killing() {
    let mut child = FakeChild::new([
        Ok(None),
        Ok(None),
        Ok(None),
        Ok(Some(status(Some(0), None))),
    ]);
    let deadline = child.now + 2 * MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert_eq!(child.now, deadline);
    assert!(child.state.observed);
    assert_eq!(child.state.reaped, None);
    assert_eq!(child.state.terminating_signal, None);
    assert!(!child.signal_sent);
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
        ]
    );
    assert_eq!(child.wait_for(10 * MS), Ok(IsolatedWorkerExitKind::Clean));
    assert_eq!(child.operations.last(), Some(&Operation::Poll));
    assert_eq!(child.operations.len(), 7);
    let before = child.operations.clone();
    assert_eq!(
        child.wait_for(Duration::ZERO),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(child.operations, before);
}

#[test]
fn observation_consumes_the_original_deadline_and_backoff_is_clamped() {
    let mut child = FakeChild::new([Ok(None), Ok(None), Ok(None)]);
    let start = child.now;
    child.watch_time = Duration::from_micros(8_500);
    let deadline = start + 10 * MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert_eq!(child.now, deadline);
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
            Operation::Backoff(Duration::from_micros(500)),
            Operation::Poll,
        ]
    );
    assert_eq!(child.state.reaped, None);
    assert!(!child.signal_sent);
}

#[test]
fn polling_and_scheduler_delay_do_not_extend_the_deadline() {
    let mut child = FakeChild::new([Ok(None), Ok(None)]);
    let start = child.now;
    child.poll_time = Duration::from_micros(500);
    child.oversleep = 4 * MS;
    let deadline = start + 2 * MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
        ]
    );
    assert_eq!(child.now, start + 6 * MS);
    assert!(!child.signal_sent);
}

#[test]
fn poll_time_is_deducted_before_clamping_the_backoff() {
    let mut child = FakeChild::new([Ok(None), Ok(None)]);
    child.poll_time = Duration::from_micros(750);
    let deadline = child.now + MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(Duration::from_micros(250)),
            Operation::Poll,
        ]
    );
    assert!(!child.signal_sent);
}

#[test]
fn already_ready_status_can_be_reaped_at_an_expired_deadline() {
    let mut child = FakeChild::new([Ok(Some(status(Some(0), None)))]);
    let deadline = child
        .now
        .checked_sub(MS)
        .expect("a representable expired fixture deadline");
    assert_eq!(
        wait_until(&mut child, deadline),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(
        child.operations,
        [Operation::Observe(deadline), Operation::Poll]
    );
}

#[test]
fn pending_status_at_an_expired_deadline_returns_timeout_without_backoff() {
    let mut child = FakeChild::new([Ok(None)]);
    let deadline = child
        .now
        .checked_sub(MS)
        .expect("a representable expired fixture deadline");
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert_eq!(
        child.operations,
        [Operation::Observe(deadline), Operation::Poll]
    );
    assert!(child.state.observed);
    assert_eq!(child.state.reaped, None);
    assert!(!child.signal_sent);
}

#[test]
fn actual_status_controls_classification_and_retains_terminating_signal() {
    for (code, signal, signal_sent, expected) in [
        (Some(0), None, false, IsolatedWorkerExitKind::Clean),
        (Some(0), None, true, IsolatedWorkerExitKind::Clean),
        (Some(7), None, false, IsolatedWorkerExitKind::Failed),
        (None, Some(6), false, IsolatedWorkerExitKind::Crashed),
        (None, Some(24), false, IsolatedWorkerExitKind::ResourceLimit),
        (None, Some(25), false, IsolatedWorkerExitKind::ResourceLimit),
        (None, Some(9), false, IsolatedWorkerExitKind::Crashed),
        (None, Some(9), true, IsolatedWorkerExitKind::Terminated),
        (None, None, false, IsolatedWorkerExitKind::Unknown),
    ] {
        let mut child = FakeChild::new([Ok(None), Ok(Some(status(code, signal)))]);
        child.signal_sent = signal_sent;
        assert_eq!(child.wait_for(5 * MS), Ok(expected));
        assert_eq!(child.state.reaped, Some(expected));
        assert_eq!(child.state.terminating_signal, signal);
        assert!(!child.operations.contains(&Operation::Terminate));
        let before = child.operations.clone();
        assert_eq!(child.wait_for(Duration::ZERO), Ok(expected));
        assert_eq!(child.operations, before);
    }
}

#[test]
fn interrupted_poll_retries_under_the_same_deadline() {
    let mut child = FakeChild::new([
        Err(io::ErrorKind::Interrupted.into()),
        Ok(Some(status(Some(0), None))),
    ]);
    let deadline = child.now + MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Poll,
            Operation::Backoff(MS),
            Operation::Poll,
        ]
    );
}

#[test]
fn persistent_interruption_expires_without_killing_or_caching() {
    let mut child = FakeChild::new([
        Err(io::ErrorKind::Interrupted.into()),
        Err(io::ErrorKind::Interrupted.into()),
    ]);
    assert_eq!(child.wait_for(MS), Err(IsolatedWorkerError::Timeout));
    assert!(child.state.observed);
    assert_eq!(child.state.reaped, None);
    assert!(!child.signal_sent);
}

#[test]
fn genuine_status_errors_fail_closed_without_fabricating_a_terminal_cache() {
    for error in [
        io::ErrorKind::PermissionDenied.into(),
        io::Error::from_raw_os_error(10),
    ] {
        // Darwin ECHILD is 10. This injected real error must never become Clean.
        let mut child = FakeChild::new([Err(error)]);
        let deadline = child.now + 5 * MS;
        assert_eq!(
            wait_until(&mut child, deadline),
            Err(IsolatedWorkerError::ReapFailed)
        );
        assert!(child.state.observed);
        assert_eq!(child.state.reaped, None);
        assert_eq!(child.state.terminating_signal, None);
        assert_eq!(
            child.operations,
            [
                Operation::Observe(deadline),
                Operation::Poll,
                Operation::Terminate,
            ]
        );
    }
}

#[test]
fn watch_error_is_distinct_from_pending_status_timeout() {
    let mut child = FakeChild::new([]);
    child.observations = [Err(())].into();
    let deadline = child.now + MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::ReapFailed)
    );
    assert!(!child.state.observed);
    assert_eq!(child.state.reaped, None);
    assert_eq!(
        child.operations,
        [Operation::Observe(deadline), Operation::Terminate]
    );
}

#[test]
fn watch_timeout_remains_unobserved_and_retry_still_watches() {
    let mut child = FakeChild::new([Ok(Some(status(Some(0), None)))]);
    child.observations = [Ok(false), Ok(true)].into();
    let deadline = child.now + MS;
    assert_eq!(
        wait_until(&mut child, deadline),
        Err(IsolatedWorkerError::Timeout)
    );
    assert!(!child.state.observed);
    assert_eq!(child.operations, [Operation::Observe(deadline)]);
    assert_eq!(
        wait_until(&mut child, deadline),
        Ok(IsolatedWorkerExitKind::Clean)
    );
    assert_eq!(
        child.operations,
        [
            Operation::Observe(deadline),
            Operation::Observe(deadline),
            Operation::Poll,
        ]
    );
}

#[test]
fn independent_child_keeps_its_status_and_is_never_polled_or_signalled() {
    let mut pending = FakeChild::new([Ok(None), Ok(None), Ok(Some(status(Some(0), None)))]);
    let mut independent = FakeChild::new([Ok(Some(status(Some(19), None)))]);
    assert_eq!(pending.wait_for(MS), Err(IsolatedWorkerError::Timeout));
    assert!(independent.operations.is_empty());
    assert!(!independent.state.observed);
    assert_eq!(independent.state.reaped, None);
    assert_eq!(independent.statuses.len(), 1);
    assert_eq!(independent.wait_for(MS), Ok(IsolatedWorkerExitKind::Failed));
    assert_eq!(pending.wait_for(MS), Ok(IsolatedWorkerExitKind::Clean));
    assert!(!pending.signal_sent);
    assert!(!independent.signal_sent);
}
