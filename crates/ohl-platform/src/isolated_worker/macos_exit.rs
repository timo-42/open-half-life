//! The macOS exit-observation and owned-child reaping state machine.
//!
//! Kept independent of kqueue so deterministic tests execute this same loop
//! with a fake clock. The production source polls only its owned `Child`.

use super::{IsolatedWorkerError, IsolatedWorkerExitKind};
use std::io;
use std::time::{Duration, Instant};

/// Project-chosen backoff after an exit notification precedes waitable status.
/// Every sleep is also capped by the caller's remaining absolute deadline.
const REAP_BACKOFF: Duration = Duration::from_millis(1);

#[derive(Debug, Default)]
pub(super) struct ExitState {
    /// Monotonic: NOTE_EXIT need only be observed once, even across timeouts.
    pub(super) observed: bool,
    pub(super) reaped: Option<IsolatedWorkerExitKind>,
    pub(super) terminating_signal: Option<i32>,
}

/// Only a successful owned-child status poll may produce this value.
#[derive(Debug, Clone, Copy)]
pub(super) struct Status {
    pub(super) code: Option<i32>,
    pub(super) signal: Option<i32>,
}

/// Private operations seam; no wait-any, blocking reap or process-id API.
pub(super) trait ExitSource {
    fn exit_state(&mut self) -> &mut ExitState;
    fn observe_exit(&mut self, deadline: Instant) -> Result<bool, ()>;
    fn poll_status(&mut self) -> io::Result<Option<Status>>;
    fn termination_sent(&self) -> bool;
    fn request_termination(&mut self);
    fn now(&self) -> Instant;
    fn backoff(&mut self, duration: Duration);
}

pub(super) fn wait_until(
    source: &mut impl ExitSource,
    deadline: Instant,
) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
    if let Some(exit) = source.exit_state().reaped {
        return Ok(exit);
    }
    if !source.exit_state().observed {
        match source.observe_exit(deadline) {
            Ok(true) => source.exit_state().observed = true,
            Ok(false) => return Err(IsolatedWorkerError::Timeout),
            Err(()) => {
                source.request_termination();
                return Err(IsolatedWorkerError::ReapFailed);
            }
        }
    }

    loop {
        // Poll once even at an expired deadline: already-ready status must
        // remain collectable without a second exit notification or any sleep.
        match source.poll_status() {
            Ok(Some(status)) => {
                let exit = classify(status.code, status.signal, source.termination_sent());
                let state = source.exit_state();
                state.terminating_signal = status.signal;
                state.reaped = Some(exit);
                return Ok(exit);
            }
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                source.request_termination();
                return Err(IsolatedWorkerError::ReapFailed);
            }
        }
        let remaining = deadline.saturating_duration_since(source.now());
        if remaining.is_zero() {
            return Err(IsolatedWorkerError::Timeout);
        }
        source.backoff(REAP_BACKOFF.min(remaining));
    }
}

/// Maps the actual macOS wait status into the public vocabulary.
pub(super) fn classify(
    code: Option<i32>,
    signal: Option<i32>,
    termination_requested: bool,
) -> IsolatedWorkerExitKind {
    const SIGKILL_NUMBER: i32 = 9;
    const SIGXCPU: i32 = 24;
    const SIGXFSZ: i32 = 25;
    match (code, signal) {
        (Some(0), _) => IsolatedWorkerExitKind::Clean,
        (Some(_), _) => IsolatedWorkerExitKind::Failed,
        (None, Some(SIGKILL_NUMBER)) if termination_requested => IsolatedWorkerExitKind::Terminated,
        (None, Some(SIGXCPU | SIGXFSZ)) => IsolatedWorkerExitKind::ResourceLimit,
        (None, Some(_)) => IsolatedWorkerExitKind::Crashed,
        (None, None) => IsolatedWorkerExitKind::Unknown,
    }
}
