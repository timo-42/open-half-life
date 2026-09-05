//! Ownership of exactly one confined parser worker's process lifetime.
//!
//! Port of the C++ `media::ParserSessionIdAllocator` and
//! `media::ParserProcessSession`. This type owns the worker, the frame channel
//! adapted over its exact I/O, and the handshake that produces the
//! [`ParserSession`]. It has no raw-path, destination, cache, staging,
//! publication or component-selection authority: callers drive
//! enumerate/stream/cancel/shutdown through the returned session, and this
//! type only keeps the channel alive across the handshake and the session
//! lifetime, then closes and reaps the worker in the right order.
//!
//! `Drop` never abandons a live worker: it escalates to
//! [`WorkerProcess::terminate_and_wait`] with a short bounded deadline. That
//! escalation is cached and happens at most once, however many times shutdown
//! is attempted or the object is dropped.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ohl_media::ValidatedMedia;
use ohl_parser_protocol::{ProtocolBudgets, SessionId};
use thiserror::Error;

use crate::catalog::{ImportLimits, WorkerEpoch};
use crate::frame_channel::{FrameBuffer, FrameChannel};
use crate::handshake::{HandshakeError, perform_parent_handshake_over_window};
use crate::io::{CancellationToken, ExactIo, IoError, sealed};
use crate::parent_session::{
    Cancelled, Closed, Idle, ParserSession, SessionError, TerminalSession,
    create_parser_session_with_ops,
};
use crate::source_read_broker::{NativeSourceOps, SourceOps, SourceReadLimits};

/// The bounded deadline `Drop` escalates with.
const DROP_TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// The allocator handed out its last identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[error("parser session identities exhausted")]
pub struct AllocatorExhausted;

/// One fresh session identity and its worker epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionAllocation {
    /// The protocol session id.
    pub session_id: SessionId,
    /// The catalog-generation epoch for the same worker lifetime.
    pub worker_epoch: WorkerEpoch,
}

/// Hands out fresh, non-zero, monotonically increasing session ids and worker
/// epochs.
///
/// Values are unique for the allocator's lifetime and never reused, including
/// after exhaustion. Deterministic and free of randomness: production
/// composition starts at 1; [`SessionIdAllocator::starting_at`] exists so
/// tests can drive exhaustion without iterating the whole 64-bit space.
#[derive(Debug)]
pub struct SessionIdAllocator {
    next_session_id: u64,
    next_worker_epoch: u64,
    exhausted: bool,
}

impl Default for SessionIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionIdAllocator {
    /// An allocator starting at 1.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_session_id: 1,
            next_worker_epoch: 1,
            exhausted: false,
        }
    }

    /// An allocator starting at explicit values.
    ///
    /// A zero start is treated as already exhausted, so a zero session id or
    /// epoch can never be handed out.
    #[must_use]
    pub const fn starting_at(first_session_id: u64, first_worker_epoch: u64) -> Self {
        Self {
            next_session_id: first_session_id,
            next_worker_epoch: first_worker_epoch,
            exhausted: first_session_id == 0 || first_worker_epoch == 0,
        }
    }

    /// Allocates the next identity.
    ///
    /// Fails closed rather than wrapping once either counter has issued its
    /// maximum representable value.
    ///
    /// # Errors
    /// [`AllocatorExhausted`].
    pub fn allocate(&mut self) -> Result<SessionAllocation, AllocatorExhausted> {
        let allocation = (!self.exhausted)
            .then(|| {
                Some(SessionAllocation {
                    session_id: SessionId::new(self.next_session_id)?,
                    worker_epoch: WorkerEpoch::new(self.next_worker_epoch)?,
                })
            })
            .flatten();
        let Some(allocation) = allocation else {
            self.exhausted = true;
            return Err(AllocatorExhausted);
        };
        if self.next_session_id == u64::MAX || self.next_worker_epoch == u64::MAX {
            // Fail closed instead of wrapping to a previously issued or zero
            // value on the very next call.
            self.exhausted = true;
        } else {
            self.next_session_id += 1;
            self.next_worker_epoch += 1;
        }
        Ok(allocation)
    }
}

/// How a worker process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerExit {
    /// The worker exited on its own with this status code.
    Exited(i32),
    /// The worker was terminated by the parent.
    Terminated,
}

/// The outcome of waiting for, or terminating, a worker.
pub type WaitOutcome = Result<WorkerExit, IoError>;

/// A launched, confined worker process.
///
/// Sealed: only this crate provides implementations — the
/// [`FakeWorker`](crate::testing::FakeWorker) used by the tests today, and the
/// `ohl-platform` `IsolatedWorker` adapter in R4.7.
pub trait WorkerProcess: sealed::Sealed {
    /// The exact-I/O capability over this worker's channel.
    type Io: ExactIo;

    /// A handle to the worker's byte channel. Grants no process authority.
    fn io(&self) -> Self::Io;

    /// Closes the parent's end of the channel.
    fn close_channel(&self);

    /// Waits for the worker to exit and reaps it.
    ///
    /// # Errors
    /// [`IoError::TimedOut`] past `deadline`, or [`IoError::IoFailure`].
    fn wait(&self, deadline: Instant) -> WaitOutcome;

    /// Terminates the worker, then waits for and reaps it.
    ///
    /// # Errors
    /// As [`WorkerProcess::wait`].
    fn terminate_and_wait(&self, deadline: Instant) -> WaitOutcome;
}

/// Everything one session needs beyond the media and the deadline.
///
/// Bundling the three quota sets keeps [`ProcessSession::open`] readable and
/// makes it impossible to pass the read limits and the import limits in the
/// wrong order.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionConfig {
    /// The parent-serviced read quotas, identical to the advertised hello.
    pub source_read_limits: SourceReadLimits,
    /// The OWP/1 message and payload budgets.
    pub protocol_budgets: ProtocolBudgets,
    /// The enumeration quotas the catalog is promoted under.
    pub import_limits: ImportLimits,
}

/// The process-lifetime phase of a [`ProcessSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessState {
    /// Launched but not handshaken.
    Idle,
    /// A session is open over the channel.
    Open,
    /// The worker was shut down and reaped cleanly.
    Closed,
    /// The worker was terminated.
    Terminated,
}

/// Why opening a session failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum OpenError {
    /// `open` was already called on this worker.
    #[error("process session already opened")]
    InvalidState,
    /// The handshake failed.
    #[error("worker handshake failed")]
    Handshake(#[source] HandshakeError),
    /// The session could not be composed from the proof.
    #[error("worker session creation failed")]
    Session(#[source] SessionError),
}

/// An `open` failure together with the single escalation it triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFailure {
    error: OpenError,
    termination: Option<WaitOutcome>,
}

impl OpenFailure {
    /// Why the open failed.
    #[must_use]
    pub const fn error(&self) -> OpenError {
        self.error
    }

    /// The cached termination outcome, if the worker was escalated.
    #[must_use]
    pub const fn termination(&self) -> Option<WaitOutcome> {
        self.termination
    }
}

/// Why an orderly shutdown failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum ShutdownError {
    /// No session was ever opened, or the session was already consumed.
    #[error("process session not open")]
    InvalidState,
    /// The protocol shutdown failed.
    #[error("protocol shutdown failed")]
    Protocol(#[source] SessionError),
    /// The worker did not exit before the deadline.
    #[error("worker reap failed")]
    Reap(#[source] IoError),
}

/// A shutdown failure and the single escalation it triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownFailure {
    error: ShutdownError,
    termination: WaitOutcome,
}

impl ShutdownFailure {
    /// Why the shutdown failed.
    #[must_use]
    pub const fn error(&self) -> ShutdownError {
        self.error
    }

    /// The cached termination outcome.
    pub const fn termination(&self) -> WaitOutcome {
        self.termination
    }
}

/// A session phase from which an orderly protocol shutdown is legal.
pub trait ShutdownReady<T: ExactIo, O: SourceOps>: sealed::Sealed + Sized {
    /// Sends `shutdown` and returns the closed session.
    ///
    /// # Errors
    /// The retired session.
    fn shutdown_protocol(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>>;
}

impl<T: ExactIo, O: SourceOps> sealed::Sealed for ParserSession<Idle, T, O> {}

impl<T: ExactIo, O: SourceOps> ShutdownReady<T, O> for ParserSession<Idle, T, O> {
    fn shutdown_protocol(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>> {
        self.shutdown(deadline, cancellation)
    }
}

impl<T: ExactIo, O: SourceOps> sealed::Sealed for ParserSession<Cancelled, T, O> {}

impl<T: ExactIo, O: SourceOps> ShutdownReady<T, O> for ParserSession<Cancelled, T, O> {
    fn shutdown_protocol(
        self,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Closed, T, O>, TerminalSession<T, O>> {
        self.shutdown(deadline, cancellation)
    }
}

/// Owns one worker for one session lifetime.
#[derive(Debug)]
pub struct ProcessSession<W: WorkerProcess> {
    worker: W,
    channel: Option<Arc<FrameChannel<W::Io>>>,
    session_id: SessionId,
    worker_epoch: WorkerEpoch,
    state: ProcessState,
    termination: Option<WaitOutcome>,
}

impl<W: WorkerProcess> ProcessSession<W> {
    /// Takes ownership of an already-launched worker for one session.
    pub const fn new(worker: W, allocation: SessionAllocation) -> Self {
        Self {
            worker,
            channel: None,
            session_id: allocation.session_id,
            worker_epoch: allocation.worker_epoch,
            state: ProcessState::Idle,
            termination: None,
        }
    }

    /// The process-lifetime phase.
    #[must_use]
    pub const fn state(&self) -> ProcessState {
        self.state
    }

    /// Whether the worker has been reaped, cleanly or not.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self.state, ProcessState::Closed | ProcessState::Terminated)
    }

    /// The pinned session id.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// The catalog-generation epoch.
    #[must_use]
    pub const fn worker_epoch(&self) -> WorkerEpoch {
        self.worker_epoch
    }

    /// The cached wait or termination outcome, once the worker was reaped.
    #[must_use]
    pub const fn outcome(&self) -> Option<WaitOutcome> {
        self.termination
    }

    /// The frame channel, once `open` created it.
    #[must_use]
    pub fn channel(&self) -> Option<&Arc<FrameChannel<W::Io>>> {
        self.channel.as_ref()
    }

    fn terminate_once(&mut self, deadline: Instant) -> WaitOutcome {
        // The outcome is recorded exactly once, by whichever of a clean reap
        // or an escalation happened first.
        if let Some(cached) = self.termination {
            return cached;
        }
        self.state = ProcessState::Terminated;
        let outcome = self.worker.terminate_and_wait(deadline);
        self.termination = Some(outcome);
        outcome
    }

    /// Terminates the worker, at most once for this object's lifetime.
    pub fn terminate(&mut self, deadline: Instant) -> WaitOutcome {
        self.terminate_once(deadline)
    }

    /// Performs the parent handshake and composes the session.
    ///
    /// On any failure the worker is escalated to
    /// [`WorkerProcess::terminate_and_wait`] exactly once and this object
    /// becomes terminal; `open` may not be retried.
    ///
    /// # Errors
    /// [`OpenFailure`] carrying [`OpenError`] and the escalation outcome.
    pub fn open(
        &mut self,
        media: &ValidatedMedia,
        config: SessionConfig,
        buffer: &mut FrameBuffer,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ParserSession<Idle, W::Io, NativeSourceOps>, OpenFailure> {
        self.open_with_ops(
            media,
            config,
            buffer,
            deadline,
            cancellation,
            NativeSourceOps,
        )
    }

    /// [`ProcessSession::open`] over an explicit source-operation seam.
    ///
    /// The seam is how a caller narrows the worker's read authority — see
    /// [`SourceWindow`](crate::SourceWindow), which confines every serviced
    /// read to one container file. It does not replace the broker's quotas:
    /// `config`'s read limits still bound the size, count, and cumulative
    /// bytes of what the worker may ask for.
    ///
    /// # Errors
    /// As [`ProcessSession::open`].
    pub fn open_with_ops<O: SourceOps>(
        &mut self,
        media: &ValidatedMedia,
        config: SessionConfig,
        buffer: &mut FrameBuffer,
        deadline: Instant,
        cancellation: &CancellationToken,
        ops: O,
    ) -> Result<ParserSession<Idle, W::Io, O>, OpenFailure> {
        if self.state != ProcessState::Idle || self.channel.is_some() {
            return Err(OpenFailure {
                error: OpenError::InvalidState,
                termination: None,
            });
        }
        let channel = Arc::new(FrameChannel::new(self.session_id, self.worker.io()));
        self.channel = Some(Arc::clone(&channel));

        let proof = match perform_parent_handshake_over_window(
            &channel,
            media,
            ops.window_length(media.source()),
            config.source_read_limits,
            config.protocol_budgets,
            buffer,
            deadline,
            cancellation,
        ) {
            Ok(proof) => proof,
            Err(error) => {
                return Err(OpenFailure {
                    error: OpenError::Handshake(error),
                    termination: Some(self.terminate_once(deadline)),
                });
            }
        };

        match create_parser_session_with_ops(
            proof,
            channel,
            media,
            self.worker_epoch,
            config.import_limits,
            ops,
        ) {
            Ok(session) => {
                self.state = ProcessState::Open;
                Ok(session)
            }
            Err(error) => Err(OpenFailure {
                error: OpenError::Session(error),
                termination: Some(self.terminate_once(deadline)),
            }),
        }
    }

    /// Protocol shutdown, then `close_channel`, then wait and reap.
    ///
    /// Any shutdown failure or reap timeout escalates to a single
    /// [`WorkerProcess::terminate_and_wait`] instead.
    ///
    /// # Errors
    /// [`ShutdownFailure`] carrying the reason and the escalation outcome.
    pub fn orderly_shutdown<O: SourceOps, S: ShutdownReady<W::Io, O>>(
        &mut self,
        session: S,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<WorkerExit, ShutdownFailure> {
        if self.is_terminal() {
            return self.cached_result();
        }
        if self.state != ProcessState::Open {
            return Err(ShutdownFailure {
                error: ShutdownError::InvalidState,
                termination: self.terminate_once(deadline),
            });
        }

        match session.shutdown_protocol(deadline, cancellation) {
            Ok(closed) => drop(closed),
            Err(terminal) => {
                return Err(ShutdownFailure {
                    error: ShutdownError::Protocol(terminal.error()),
                    termination: self.terminate_once(deadline),
                });
            }
        }

        self.worker.close_channel();
        match self.worker.wait(deadline) {
            Ok(exit) => {
                self.termination = Some(Ok(exit));
                self.state = ProcessState::Closed;
                Ok(exit)
            }
            Err(error) => Err(ShutdownFailure {
                error: ShutdownError::Reap(error),
                termination: self.terminate_once(deadline),
            }),
        }
    }

    /// Escalation path for a session that already failed terminally.
    pub fn abandon<O: SourceOps>(
        &mut self,
        session: TerminalSession<W::Io, O>,
        deadline: Instant,
    ) -> WaitOutcome {
        drop(session);
        self.terminate_once(deadline)
    }

    /// The idempotent repeat of [`ProcessSession::orderly_shutdown`].
    ///
    /// Once closed or terminated this returns the cached outcome without any
    /// further worker interaction; while still open — which only happens if
    /// the session was lost rather than shut down — it escalates once.
    ///
    /// # Errors
    /// [`ShutdownFailure`] as for [`ProcessSession::orderly_shutdown`].
    pub fn finish(&mut self, deadline: Instant) -> Result<WorkerExit, ShutdownFailure> {
        if self.is_terminal() {
            return self.cached_result();
        }
        Err(ShutdownFailure {
            error: ShutdownError::InvalidState,
            termination: self.terminate_once(deadline),
        })
    }

    fn cached_result(&self) -> Result<WorkerExit, ShutdownFailure> {
        match self.termination {
            Some(Ok(exit)) => Ok(exit),
            Some(Err(error)) => Err(ShutdownFailure {
                error: ShutdownError::Reap(error),
                termination: Err(error),
            }),
            None => Err(ShutdownFailure {
                error: ShutdownError::InvalidState,
                termination: Err(IoError::IoFailure),
            }),
        }
    }
}

impl<W: WorkerProcess> Drop for ProcessSession<W> {
    fn drop(&mut self) {
        if !self.is_terminal() {
            // Never abandon a live worker.
            let _ = self.terminate_once(Instant::now() + DROP_TERMINATION_TIMEOUT);
        }
    }
}
