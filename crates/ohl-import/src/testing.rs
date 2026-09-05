//! Deterministic in-crate implementations of the sealed capability traits.
//!
//! The capability traits ([`ExactIo`], [`SourceOps`], [`WorkerProcess`]) are
//! sealed, so their fault-injecting doubles must live in this crate. Nothing
//! here launches a process, opens a path, or reads real media: a
//! [`SyntheticTransport`] is an in-memory byte queue, a [`ScriptedSourceOps`]
//! replays a fixed list of outcomes against the pinned source it is given, and
//! a [`FakeWorker`] only records the lifecycle calls made on it.
//!
//! These types are deliberately public: they are how the tests, and later the
//! R4.7 `IsolatedWorker` adapter's own tests, drive every failure branch
//! without a real worker.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use ohl_parser_protocol::{FRAME_HEADER_BYTES, FrameHeader, encode_frame};
use ohl_platform::{MediaSource, MediaSourceError};

use crate::io::{CancellationToken, ExactIo, IoError, sealed};
use crate::process_session::{WaitOutcome, WorkerExit, WorkerProcess};
use crate::source_read_broker::SourceOps;

/// The longest a parked transfer sleeps before rechecking its predicates.
const PARK_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// One scripted transfer outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoStep {
    /// Transfer the complete slice and report it truthfully.
    Transfer,
    /// Transfer `min(count, len)` bytes but report `count` — the impossible
    /// I/O the channel must sanitize.
    Claim(usize),
    /// Fail with this error.
    Fail(IoError),
    /// Block until aborted, cancelled, or past the deadline.
    Block,
}

#[derive(Debug, Default)]
struct TransportState {
    inbound: VecDeque<u8>,
    written: Vec<u8>,
    reads: VecDeque<IoStep>,
    writes: VecDeque<IoStep>,
    read_calls: usize,
    write_calls: usize,
    abort_calls: usize,
    blocked: usize,
    aborted: bool,
    block_when_empty: bool,
}

/// An in-memory duplex byte channel with scripted failures.
#[derive(Debug, Default)]
pub struct SyntheticTransport {
    state: Mutex<TransportState>,
    signal: Condvar,
}

impl SyntheticTransport {
    /// An empty transport that fails reads once its inbound queue runs dry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, TransportState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes an exhausted read block instead of reporting a closed peer.
    pub fn block_when_empty(&self, block: bool) {
        self.state().block_when_empty = block;
        self.signal.notify_all();
    }

    /// Queues raw inbound bytes.
    pub fn push_bytes(&self, bytes: &[u8]) {
        self.state().inbound.extend(bytes.iter().copied());
        self.signal.notify_all();
    }

    /// Queues one complete inbound frame.
    ///
    /// # Panics
    /// If `header` and `payload` do not form an encodable frame; a test that
    /// wants a malformed frame uses [`SyntheticTransport::push_bytes`].
    pub fn push_frame(&self, header: &FrameHeader, payload: &[u8]) {
        let mut bytes = vec![0_u8; FRAME_HEADER_BYTES + payload.len()];
        let written = encode_frame(header, payload, &mut bytes).expect("encodable frame");
        self.push_bytes(&bytes[..written]);
    }

    /// Scripts the next read outcomes, in order.
    pub fn script_reads(&self, steps: impl IntoIterator<Item = IoStep>) {
        self.state().reads.extend(steps);
    }

    /// Scripts the next write outcomes, in order.
    pub fn script_writes(&self, steps: impl IntoIterator<Item = IoStep>) {
        self.state().writes.extend(steps);
    }

    /// Every byte the parent has written.
    #[must_use]
    pub fn written(&self) -> Vec<u8> {
        self.state().written.clone()
    }

    /// Discards the recorded outbound bytes.
    pub fn clear_written(&self) {
        self.state().written.clear();
    }

    /// The number of reads, writes and aborts made so far.
    #[must_use]
    pub fn call_counts(&self) -> (usize, usize, usize) {
        let state = self.state();
        (state.read_calls, state.write_calls, state.abort_calls)
    }

    /// Whether [`ExactIo::abort_io`] was called.
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.state().aborted
    }

    /// Blocks until at least `count` transfers are parked inside the
    /// transport, so a test can act while I/O is in flight.
    pub fn await_blocked(&self, count: usize) {
        let mut state = self.state();
        while state.blocked < count {
            state = self
                .signal
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Waits once, counting this transfer as parked while it sleeps.
    fn park_once<'transport>(
        &'transport self,
        mut state: MutexGuard<'transport, TransportState>,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<MutexGuard<'transport, TransportState>, IoError> {
        if state.aborted {
            return Err(IoError::Aborted);
        }
        if cancellation.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(IoError::TimedOut);
        };
        state.blocked += 1;
        self.signal.notify_all();
        let (mut state, _) = self
            .signal
            .wait_timeout(state, remaining.min(PARK_POLL_INTERVAL))
            .unwrap_or_else(PoisonError::into_inner);
        state.blocked -= 1;
        self.signal.notify_all();
        Ok(state)
    }

    /// Parks until abort, cancellation or the deadline.
    fn park<'transport>(
        &'transport self,
        mut state: MutexGuard<'transport, TransportState>,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        loop {
            state = self.park_once(state, deadline, cancellation)?;
        }
    }
}

impl sealed::Sealed for SyntheticTransport {}

impl ExactIo for SyntheticTransport {
    fn read_exact(
        &self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        let mut state = self.state();
        state.read_calls += 1;
        if state.aborted {
            return Err(IoError::Aborted);
        }
        if cancellation.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        let step = state.reads.pop_front().unwrap_or(IoStep::Transfer);
        let claimed = match step {
            IoStep::Transfer => destination.len(),
            IoStep::Claim(count) => count,
            IoStep::Fail(error) => return Err(error),
            IoStep::Block => return self.park(state, deadline, cancellation),
        };
        let transferred = claimed.min(destination.len());
        // An exhausted queue either blocks like a live peer or reports a
        // closed one, depending on the fixture.
        while state.inbound.len() < transferred {
            if !state.block_when_empty {
                return Err(IoError::PeerClosed);
            }
            state = self.park_once(state, deadline, cancellation)?;
        }
        for slot in destination.iter_mut().take(transferred) {
            *slot = state.inbound.pop_front().unwrap_or_default();
        }
        Ok(claimed)
    }

    fn write_all(
        &self,
        source: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        let mut state = self.state();
        state.write_calls += 1;
        if state.aborted {
            return Err(IoError::Aborted);
        }
        if cancellation.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        let step = state.writes.pop_front().unwrap_or(IoStep::Transfer);
        let claimed = match step {
            IoStep::Transfer => source.len(),
            IoStep::Claim(count) => count,
            IoStep::Fail(error) => return Err(error),
            IoStep::Block => return self.park(state, deadline, cancellation),
        };
        let transferred = claimed.min(source.len());
        state.written.extend_from_slice(&source[..transferred]);
        Ok(claimed)
    }

    fn abort_io(&self) {
        let mut state = self.state();
        state.abort_calls += 1;
        state.aborted = true;
        drop(state);
        self.signal.notify_all();
    }
}

/// A scripted [`SourceOps`] for deterministic source faults.
#[derive(Debug, Default)]
pub struct ScriptedSourceOps {
    state: Mutex<SourceOpsState>,
}

#[derive(Debug, Default)]
struct SourceOpsState {
    verify: VecDeque<Result<(), MediaSourceError>>,
    reads: VecDeque<Result<(), MediaSourceError>>,
    verify_calls: usize,
    read_calls: usize,
}

impl ScriptedSourceOps {
    /// Operations that succeed by delegating to the real pinned source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, SourceOpsState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Scripts the next `verify_unchanged` outcomes, in order.
    pub fn script_verify(&self, outcomes: impl IntoIterator<Item = Result<(), MediaSourceError>>) {
        self.state().verify.extend(outcomes);
    }

    /// Scripts the next `read_exact_at` outcomes, in order.
    pub fn script_reads(&self, outcomes: impl IntoIterator<Item = Result<(), MediaSourceError>>) {
        self.state().reads.extend(outcomes);
    }

    /// The number of verifies and reads made so far.
    #[must_use]
    pub fn call_counts(&self) -> (usize, usize) {
        let state = self.state();
        (state.verify_calls, state.read_calls)
    }
}

impl sealed::Sealed for ScriptedSourceOps {}

impl SourceOps for ScriptedSourceOps {
    fn verify_unchanged(&self, source: &MediaSource) -> Result<(), MediaSourceError> {
        let mut state = self.state();
        state.verify_calls += 1;
        match state.verify.pop_front() {
            Some(outcome) => outcome,
            None => source.verify_unchanged(),
        }
    }

    fn read_exact_at(
        &self,
        source: &MediaSource,
        offset: u64,
        destination: &mut [u8],
    ) -> Result<(), MediaSourceError> {
        let mut state = self.state();
        state.read_calls += 1;
        match state.reads.pop_front() {
            Some(Ok(())) | None => source.read_exact_at(offset, destination),
            Some(error) => error,
        }
    }
}

/// One lifecycle call made on a [`FakeWorker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCall {
    /// The parent closed its end of the channel.
    CloseChannel,
    /// The parent waited for an orderly exit.
    Wait,
    /// The parent escalated to termination.
    TerminateAndWait,
}

/// A worker that runs no process and only records what was asked of it.
///
/// Cloning shares one recorder, so a test can keep a handle to inspect while
/// [`crate::ProcessSession`] owns the worker itself.
#[derive(Debug, Clone)]
pub struct FakeWorker {
    io: Arc<SyntheticTransport>,
    state: Arc<Mutex<FakeWorkerState>>,
}

#[derive(Debug)]
struct FakeWorkerState {
    calls: Vec<WorkerCall>,
    wait: WaitOutcome,
    terminate: WaitOutcome,
}

impl FakeWorker {
    /// A worker whose wait and termination both succeed.
    #[must_use]
    pub fn new(io: Arc<SyntheticTransport>) -> Self {
        Self {
            io,
            state: Arc::new(Mutex::new(FakeWorkerState {
                calls: Vec::new(),
                wait: Ok(WorkerExit::Exited(0)),
                terminate: Ok(WorkerExit::Terminated),
            })),
        }
    }

    fn state(&self) -> MutexGuard<'_, FakeWorkerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets what an orderly wait reports.
    pub fn set_wait(&self, outcome: WaitOutcome) {
        self.state().wait = outcome;
    }

    /// Sets what a termination reports.
    pub fn set_terminate(&self, outcome: WaitOutcome) {
        self.state().terminate = outcome;
    }

    /// The lifecycle calls made so far, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<WorkerCall> {
        self.state().calls.clone()
    }

    /// How many times termination was requested.
    #[must_use]
    pub fn terminate_calls(&self) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|call| **call == WorkerCall::TerminateAndWait)
            .count()
    }

    /// The transport this worker's channel is framed over.
    #[must_use]
    pub fn transport(&self) -> &Arc<SyntheticTransport> {
        &self.io
    }
}

impl sealed::Sealed for FakeWorker {}

impl WorkerProcess for FakeWorker {
    type Io = Arc<SyntheticTransport>;

    fn io(&self) -> Self::Io {
        Arc::clone(&self.io)
    }

    fn close_channel(&self) {
        self.state().calls.push(WorkerCall::CloseChannel);
    }

    fn wait(&self, _deadline: Instant) -> WaitOutcome {
        let mut state = self.state();
        state.calls.push(WorkerCall::Wait);
        state.wait
    }

    fn terminate_and_wait(&self, _deadline: Instant) -> WaitOutcome {
        let mut state = self.state();
        state.calls.push(WorkerCall::TerminateAndWait);
        state.terminate
    }
}
