//! One confined child process and its private full-duplex byte channel.
//!
//! `IsolatedWorker` is the Rust port of the C++ `ohl::platform::IsolatedWorker`
//! contract (`src/platform/include/ohl/platform/isolated_worker.hpp`). The
//! shape of the API is deliberately Rust-idiomatic rather than a transcription:
//!
//! | C++ | Rust |
//! |-----|------|
//! | `IsolatedWorkerIoResult { bytes_transferred, error }` | `Result<(), IsolatedWorkerError>` — every successful transfer is exact, so a byte count carries no information |
//! | `IsolatedWorkerWaitResult { exit, error }` | `Result<IsolatedWorkerExitKind, IsolatedWorkerError>` with [`IsolatedWorkerError::Timeout`] for "still running" |
//! | one concurrent reader plus one concurrent writer | `&mut self` I/O; concurrency is expressed through [`IsolatedWorkerCancellationSource`], which is `Send + Sync` |
//! | `IsolatedWorkerExitKind::running` | never returned from a successful `wait`; it only exists so the vocabulary is total |
//!
//! # Guarantees
//!
//! - Callers choose a *service*, never an executable, argument vector,
//!   environment, working directory, or native handle.
//! - Launch is all-or-nothing: on any failure no child survives.
//! - Any partial transfer, timeout, cancellation, peer closure, or I/O
//!   failure permanently poisons the channel; the worker must be replaced.
//! - Caller deadlines earlier than the internal ceilings win; later ones are
//!   clamped to them (10 s startup, 30 s per I/O, 30 s observing wait, 5 s
//!   termination).
//! - [`IsolatedWorker`] never abandons a live child: its `Drop` requests
//!   termination and reaps.
//!
//! Only Linux x86-64 has a native backend. Everywhere else
//! [`launch_isolated_worker`] fails with [`IsolatedWorkerError::Unsupported`]
//! and no other item behaves differently.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "linux.rs"]
mod backend;

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
#[path = "unsupported.rs"]
mod backend;

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod linux_tests;

/// Largest transfer a single [`IsolatedWorker::read_exact`] or
/// [`IsolatedWorker::write_all`] call will accept.
pub const MAX_ISOLATED_WORKER_IO_BYTES: usize = 1 << 20;

/// Ceiling applied to [`launch_isolated_worker`]'s startup deadline.
const MAX_STARTUP_DURATION: Duration = Duration::from_secs(10);
/// Ceiling applied to each I/O deadline.
const MAX_IO_DURATION: Duration = Duration::from_secs(30);
/// Ceiling applied to an observing [`IsolatedWorker::wait`].
const MAX_WAIT_DURATION: Duration = Duration::from_secs(30);
/// Ceiling applied to [`IsolatedWorker::terminate_and_wait`] and used by
/// `Drop`.
const MAX_TERMINATION_DURATION: Duration = Duration::from_secs(5);

/// The fixed application capabilities a worker can be launched for.
///
/// This is intentionally a closed set: there is no way to ask for an
/// arbitrary program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IsolatedWorkerService {
    /// The media-import parser worker.
    MediaParser,
}

/// Fixed, payload-free failure codes.
///
/// No variant carries a path, an OS error string, or a media-derived byte, so
/// neither `Debug` nor `Display` can leak them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IsolatedWorkerError {
    /// A caller argument was rejected before anything native happened.
    InvalidArgument,
    /// The channel is already closed or poisoned.
    InvalidState,
    /// This platform has no isolated-worker backend.
    Unsupported,
    /// The service image is not installed.
    ServiceUnavailable,
    /// The service image failed ownership, permission, or ELF verification.
    ServiceIdentityMismatch,
    /// The kernel could not provide the required confinement.
    ConfinementUnavailable,
    /// The private channel or the readiness pipe could not be created.
    ChannelCreationFailed,
    /// The child process could not be created.
    ProcessCreationFailed,
    /// The child was created but did not reach a confined, ready state.
    BootstrapFailed,
    /// A descriptor, memory, or process resource was exhausted.
    ResourceExhausted,
    /// The transfer exceeds [`MAX_ISOLATED_WORKER_IO_BYTES`].
    TransferTooLarge,
    /// The deadline expired.
    Timeout,
    /// A cancellation token was signalled.
    Cancelled,
    /// The worker closed its end of the channel.
    PeerClosed,
    /// The channel failed for any other reason.
    IoFailure,
    /// Termination was requested but the signal could not be delivered.
    TerminationFailed,
    /// The child could not be reaped, so its status is unknowable.
    ReapFailed,
}

impl fmt::Display for IsolatedWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArgument => "isolated worker: invalid argument",
            Self::InvalidState => "isolated worker: channel is closed or poisoned",
            Self::Unsupported => "isolated worker: unsupported on this platform",
            Self::ServiceUnavailable => "isolated worker: service image is not installed",
            Self::ServiceIdentityMismatch => "isolated worker: service image failed verification",
            Self::ConfinementUnavailable => "isolated worker: containment is unavailable",
            Self::ChannelCreationFailed => "isolated worker: channel creation failed",
            Self::ProcessCreationFailed => "isolated worker: process creation failed",
            Self::BootstrapFailed => "isolated worker: bootstrap failed",
            Self::ResourceExhausted => "isolated worker: resources exhausted",
            Self::TransferTooLarge => "isolated worker: transfer too large",
            Self::Timeout => "isolated worker: deadline expired",
            Self::Cancelled => "isolated worker: cancelled",
            Self::PeerClosed => "isolated worker: peer closed the channel",
            Self::IoFailure => "isolated worker: channel I/O failed",
            Self::TerminationFailed => "isolated worker: termination failed",
            Self::ReapFailed => "isolated worker: the child could not be reaped",
        })
    }
}

impl std::error::Error for IsolatedWorkerError {}

/// How a worker process left the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum IsolatedWorkerExitKind {
    /// Still running; only produced by the internal vocabulary, never by a
    /// successful `wait`.
    Running,
    /// Exited with status 0.
    Clean,
    /// Exited with a non-zero status.
    Failed,
    /// Died on a signal. This covers both a genuine fault (`SIGSEGV`,
    /// `SIGILL`) and a containment kill (`SIGSYS` from
    /// `SECCOMP_RET_KILL_PROCESS`).
    Crashed,
    /// Died on `SIGXCPU` or `SIGXFSZ`, i.e. hit a resource limit.
    ResourceLimit,
    /// Died on the `SIGKILL` this process asked for.
    Terminated,
    /// Reaped, but the status could not be classified.
    Unknown,
}

/// Shared cancellation state. One monotonic request, never reset.
#[derive(Debug, Default)]
struct CancellationState {
    requested: AtomicBool,
}

/// The request authority paired with [`IsolatedWorkerCancellationToken`]s.
///
/// Requests are monotonic and idempotent: the first
/// [`request_cancellation`](Self::request_cancellation) returns `true`, later
/// ones return `false`.
#[derive(Debug, Clone, Default)]
pub struct IsolatedWorkerCancellationSource {
    state: Option<Arc<CancellationState>>,
}

impl IsolatedWorkerCancellationSource {
    /// Creates a source with fresh shared state.
    pub fn new() -> Self {
        Self {
            state: Some(Arc::new(CancellationState::default())),
        }
    }

    /// A copyable observation handle. Tokens keep the shared state alive
    /// independently of the source.
    pub fn token(&self) -> IsolatedWorkerCancellationToken {
        IsolatedWorkerCancellationToken {
            state: self.state.clone(),
        }
    }

    /// Requests cancellation. Returns `true` only for the first request.
    pub fn request_cancellation(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| !state.requested.swap(true, Ordering::SeqCst))
    }
}

/// A copyable observation handle for one monotonic cancellation request.
///
/// [`IsolatedWorkerCancellationToken::default`] has no source and is never
/// cancelled.
#[derive(Debug, Clone, Default)]
pub struct IsolatedWorkerCancellationToken {
    state: Option<Arc<CancellationState>>,
}

impl IsolatedWorkerCancellationToken {
    /// Whether cancellation has been requested.
    pub fn cancellation_requested(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.requested.load(Ordering::SeqCst))
    }
}

/// Clamps `caller` to `now + ceiling`, saturating instead of overflowing.
fn clamp_deadline(caller: Instant, ceiling: Duration) -> Instant {
    let internal = Instant::now()
        .checked_add(ceiling)
        .unwrap_or_else(Instant::now);
    caller.min(internal)
}

/// One confined child and its private full-duplex byte channel.
///
/// The value is not `Clone` and owns the child outright. It is `Send` (every
/// field is an owned descriptor or plain data) but every operation takes
/// `&mut self`, so a single worker is driven from one place at a time;
/// asynchronous *cancellation* is expressed with
/// [`IsolatedWorkerCancellationSource`] instead of with shared access to the
/// worker.
#[derive(Debug)]
pub struct IsolatedWorker {
    backend: backend::Backend,
    channel_closed: bool,
    poisoned: bool,
    cached_exit: Option<IsolatedWorkerExitKind>,
}

impl IsolatedWorker {
    /// Reads exactly `destination.len()` bytes.
    ///
    /// Any failure - including a partial transfer - poisons the channel.
    pub fn read_exact(
        &mut self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        let deadline = clamp_deadline(deadline, MAX_IO_DURATION);
        self.check_io_preconditions(destination.len())?;
        let outcome = self.backend.read_exact(destination, deadline, cancellation);
        self.finish_io(outcome, cancellation)
    }

    /// Writes all of `source`.
    ///
    /// Any failure - including a partial transfer - poisons the channel.
    pub fn write_all(
        &mut self,
        source: &[u8],
        deadline: Instant,
        cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        let deadline = clamp_deadline(deadline, MAX_IO_DURATION);
        self.check_io_preconditions(source.len())?;
        let outcome = self.backend.write_all(source, deadline, cancellation);
        self.finish_io(outcome, cancellation)
    }

    fn check_io_preconditions(&self, length: usize) -> Result<(), IsolatedWorkerError> {
        if length == 0 {
            return Err(IsolatedWorkerError::InvalidArgument);
        }
        if length > MAX_ISOLATED_WORKER_IO_BYTES {
            return Err(IsolatedWorkerError::TransferTooLarge);
        }
        if self.poisoned || self.channel_closed {
            return Err(IsolatedWorkerError::InvalidState);
        }
        Ok(())
    }

    fn finish_io(
        &mut self,
        outcome: Result<(), IsolatedWorkerError>,
        cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => {
                self.abort_io();
                Err(if cancellation.cancellation_requested() {
                    IsolatedWorkerError::Cancelled
                } else {
                    error
                })
            }
        }
    }

    /// Permanently poisons and closes the channel. Idempotent.
    pub fn abort_io(&mut self) {
        self.poisoned = true;
        self.channel_closed = true;
        self.backend.abort_io();
    }

    /// Performs an orderly local close when no further protocol traffic is
    /// required, so the worker observes end-of-file. Idempotent.
    pub fn close_channel(&mut self) {
        self.channel_closed = true;
        self.backend.close_channel();
    }

    /// Observes the child until `deadline`. A successful terminal result is
    /// cached and returned by every later call.
    pub fn wait(
        &mut self,
        deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        if let Some(exit) = self.cached_exit {
            return Ok(exit);
        }
        let deadline = clamp_deadline(deadline, MAX_WAIT_DURATION);
        let exit = self.backend.wait(deadline)?;
        self.cached_exit = Some(exit);
        Ok(exit)
    }

    /// Closes the channel, requests containment termination, and reaps.
    pub fn terminate_and_wait(
        &mut self,
        deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        self.abort_io();
        if let Some(exit) = self.cached_exit {
            return Ok(exit);
        }
        let deadline = clamp_deadline(deadline, MAX_TERMINATION_DURATION);
        let exit = self.backend.terminate_and_wait(deadline)?;
        self.cached_exit = Some(exit);
        Ok(exit)
    }
}

impl Drop for IsolatedWorker {
    /// The final ownership backstop: a live child is never abandoned.
    fn drop(&mut self) {
        if self.cached_exit.is_some() {
            return;
        }
        let deadline = Instant::now()
            .checked_add(MAX_TERMINATION_DURATION)
            .unwrap_or_else(Instant::now);
        let _ = self.terminate_and_wait(deadline);
    }
}

/// Launches one fully bootstrapped, confined worker for `service`.
///
/// Launch is all-or-nothing: on success the returned worker is confined,
/// running, and has attested its readiness; on any failure no child survives.
/// `startup_deadline` is clamped to 10 seconds.
pub fn launch_isolated_worker(
    service: IsolatedWorkerService,
    startup_deadline: Instant,
) -> Result<IsolatedWorker, IsolatedWorkerError> {
    let deadline = clamp_deadline(startup_deadline, MAX_STARTUP_DURATION);
    if deadline <= Instant::now() {
        return Err(IsolatedWorkerError::Timeout);
    }
    let backend = backend::Backend::launch(service, deadline)?;
    Ok(IsolatedWorker {
        backend,
        channel_closed: false,
        poisoned: false,
        cached_exit: None,
    })
}

/// Test-only launcher that names the image by path instead of resolving the
/// compile-fixed install location. Verification and confinement are exactly
/// the same; only the resolution step differs.
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(crate) fn launch_isolated_worker_from_image(
    image: &std::path::Path,
    startup_deadline: Instant,
) -> Result<IsolatedWorker, IsolatedWorkerError> {
    let deadline = clamp_deadline(startup_deadline, MAX_STARTUP_DURATION);
    if deadline <= Instant::now() {
        return Err(IsolatedWorkerError::Timeout);
    }
    let backend = backend::Backend::launch_verified_path(image, deadline)?;
    Ok(IsolatedWorker {
        backend,
        channel_closed: false,
        poisoned: false,
        cached_exit: None,
    })
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
impl IsolatedWorker {
    /// Terminating signal of the reaped child, so a seccomp kill (`SIGSYS`)
    /// can be told apart from another fatal signal in tests.
    pub(crate) fn terminating_signal(&self) -> Option<i32> {
        self.backend.terminating_signal()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IsolatedWorkerCancellationSource, IsolatedWorkerCancellationToken, IsolatedWorkerError,
        MAX_STARTUP_DURATION, clamp_deadline,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn a_default_token_is_never_cancelled() {
        assert!(!IsolatedWorkerCancellationToken::default().cancellation_requested());
    }

    #[test]
    fn cancellation_is_monotonic_and_idempotent() {
        let source = IsolatedWorkerCancellationSource::new();
        let token = source.token();
        assert!(!token.cancellation_requested());
        assert!(source.request_cancellation());
        assert!(!source.request_cancellation());
        assert!(token.cancellation_requested());
    }

    #[test]
    fn a_token_outlives_its_source() {
        let token = {
            let source = IsolatedWorkerCancellationSource::new();
            let token = source.token();
            assert!(source.request_cancellation());
            token
        };
        assert!(token.cancellation_requested());
    }

    #[test]
    fn a_default_source_has_no_shared_state() {
        let source = IsolatedWorkerCancellationSource::default();
        assert!(!source.request_cancellation());
        assert!(!source.token().cancellation_requested());
    }

    #[test]
    fn deadlines_are_clamped_to_the_internal_ceiling() {
        let far = Instant::now() + Duration::from_secs(3600);
        let clamped = clamp_deadline(far, MAX_STARTUP_DURATION);
        assert!(clamped < far);
        let near = Instant::now() + Duration::from_millis(5);
        assert_eq!(clamp_deadline(near, MAX_STARTUP_DURATION), near);
    }

    #[test]
    fn errors_render_without_any_payload() {
        let rendered = IsolatedWorkerError::ServiceIdentityMismatch.to_string();
        assert!(rendered.starts_with("isolated worker: "));
        assert!(!rendered.contains('/'));
    }
}
