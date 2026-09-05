//! The `ohl-platform` adapters behind the sealed capability traits.
//!
//! [`ParserWorkerProcess`] owns one confined
//! [`IsolatedWorker`](ohl_platform::IsolatedWorker) launched for
//! [`IsolatedWorkerService::MediaParser`], and [`WorkerChannel`] is the
//! [`ExactIo`] view of that worker's private full-duplex byte channel. Both
//! are the production counterparts of
//! [`FakeWorker`](crate::testing::FakeWorker) and
//! [`SyntheticTransport`](crate::testing::SyntheticTransport).
//!
//! # Platform support
//!
//! `ohl-platform` selects a native containment backend only on Linux x86-64
//! and the unsupported backend everywhere else, so this adapter compiles on
//! every supported tuple and [`ParserWorkerProcess::launch`] simply reports
//! the sanitized [`IsolatedWorkerError::Unsupported`] where no backend
//! exists. Callers do not branch on the target: they map the launch error.
//!
//! # Concurrency
//!
//! [`ExactIo`] permits one read and one write at the same time.
//! `IsolatedWorker` takes `&mut self` for every transfer, so this adapter
//! serialises transfers behind one mutex. That is exact for the way
//! [`ParserSession`](crate::ParserSession) drives the channel — strictly one
//! synchronous send or receive at a time — and every transfer is bounded by
//! the caller's deadline, so a serialised peer cannot block another
//! indefinitely. [`WorkerChannel::abort_io`] never waits for the mutex: it
//! records the terminal abort and requests the worker's own cancellation, so
//! a transfer parked in the kernel is woken and the deferred native abort
//! runs at the next uncontended call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use ohl_platform::{
    IsolatedWorker, IsolatedWorkerCancellationSource, IsolatedWorkerError, IsolatedWorkerExitKind,
    IsolatedWorkerService, launch_isolated_worker,
};

use crate::io::{CancellationToken, ExactIo, IoError, sealed};
use crate::process_session::{WaitOutcome, WorkerExit, WorkerProcess};

/// Maps a native worker failure onto the transport's fixed vocabulary.
///
/// The mapping is total and lossy on purpose: everything that is not a
/// deadline, a cancellation, a peer close, or a terminal channel state is one
/// generic I/O failure, so no native cause can widen the parent's error
/// surface.
const fn map_io_error(error: IsolatedWorkerError) -> IoError {
    match error {
        IsolatedWorkerError::Timeout => IoError::TimedOut,
        IsolatedWorkerError::Cancelled => IoError::Cancelled,
        IsolatedWorkerError::PeerClosed => IoError::PeerClosed,
        IsolatedWorkerError::InvalidState => IoError::Aborted,
        _ => IoError::IoFailure,
    }
}

/// Maps a reaped child's classification onto the session's exit vocabulary.
///
/// `Clean` is the only exit that reports success. A non-zero exit becomes
/// `Exited(1)` because the public platform vocabulary does not carry the
/// status code, and every containment kill, fault, or resource-limit death
/// becomes `Terminated`. `Running` and `Unknown` are not terminal answers, so
/// they are reported as an I/O failure rather than invented exits.
const fn map_exit(kind: IsolatedWorkerExitKind) -> WaitOutcome {
    match kind {
        IsolatedWorkerExitKind::Clean => Ok(WorkerExit::Exited(0)),
        IsolatedWorkerExitKind::Failed => Ok(WorkerExit::Exited(1)),
        IsolatedWorkerExitKind::Crashed
        | IsolatedWorkerExitKind::ResourceLimit
        | IsolatedWorkerExitKind::Terminated => Ok(WorkerExit::Terminated),
        // `IsolatedWorkerExitKind` is `non_exhaustive`: an exit kind this
        // build does not know is not a terminal answer either.
        _ => Err(IoError::IoFailure),
    }
}

/// The shared state a [`WorkerChannel`] and its [`ParserWorkerProcess`] hold.
#[derive(Debug)]
struct WorkerHandle {
    worker: Mutex<IsolatedWorker>,
    cancellation: IsolatedWorkerCancellationSource,
    /// Set by [`WorkerChannel::abort_io`]; makes every later transfer
    /// terminal even if the native abort had to be deferred.
    aborted: AtomicBool,
    /// Set while an abort could not take the mutex, so the next uncontended
    /// caller performs the native abort.
    abort_pending: AtomicBool,
}

impl WorkerHandle {
    fn lock(&self) -> MutexGuard<'_, IsolatedWorker> {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if self.abort_pending.swap(false, Ordering::AcqRel) {
            worker.abort_io();
        }
        worker
    }

    /// Records the terminal abort without ever waiting for the mutex.
    fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
        // Wakes a transfer parked in the kernel so the mutex is released.
        self.cancellation.request_cancellation();
        match self.worker.try_lock() {
            Ok(mut worker) => worker.abort_io(),
            Err(_) => self.abort_pending.store(true, Ordering::Release),
        }
    }

    /// The pre-transfer gate: an aborted channel and an already-signalled
    /// caller token both fail before any native call is made.
    fn gate(&self, cancellation: &CancellationToken) -> Result<(), IoError> {
        if self.aborted.load(Ordering::Acquire) {
            return Err(IoError::Aborted);
        }
        if cancellation.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        Ok(())
    }
}

/// The [`ExactIo`] view of one confined worker's byte channel.
///
/// Holding it grants no launch, termination, reap, or executable-selection
/// authority: those live on [`ParserWorkerProcess`] alone.
#[derive(Debug, Clone)]
pub struct WorkerChannel {
    handle: Arc<WorkerHandle>,
}

impl sealed::Sealed for WorkerChannel {}

impl ExactIo for WorkerChannel {
    fn read_exact(
        &self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        self.handle.gate(cancellation)?;
        let length = destination.len();
        self.handle
            .lock()
            .read_exact(destination, deadline, &self.handle.cancellation.token())
            .map(|()| length)
            .map_err(map_io_error)
    }

    fn write_all(
        &self,
        source: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        self.handle.gate(cancellation)?;
        self.handle
            .lock()
            .write_all(source, deadline, &self.handle.cancellation.token())
            .map(|()| source.len())
            .map_err(map_io_error)
    }

    fn abort_io(&self) {
        self.handle.abort();
    }
}

/// One launched, confined media-parser worker process.
///
/// Dropping it drops the owned `IsolatedWorker`, whose own `Drop` is the
/// final backstop against abandoning a live child;
/// [`ProcessSession`](crate::ProcessSession) escalates first.
#[derive(Debug)]
pub struct ParserWorkerProcess {
    handle: Arc<WorkerHandle>,
}

impl ParserWorkerProcess {
    /// Launches the compile-fixed media-parser service image.
    ///
    /// There is no way to name another program: the service is a closed
    /// enum and the image location is fixed by `ohl-platform`.
    ///
    /// # Errors
    /// The sanitized [`IsolatedWorkerError`], including
    /// [`IsolatedWorkerError::Unsupported`] on a target with no containment
    /// backend and [`IsolatedWorkerError::ServiceUnavailable`] when the
    /// image is not installed.
    pub fn launch(startup_deadline: Instant) -> Result<Self, IsolatedWorkerError> {
        let worker = launch_isolated_worker(IsolatedWorkerService::MediaParser, startup_deadline)?;
        Ok(Self {
            handle: Arc::new(WorkerHandle {
                worker: Mutex::new(worker),
                cancellation: IsolatedWorkerCancellationSource::new(),
                aborted: AtomicBool::new(false),
                abort_pending: AtomicBool::new(false),
            }),
        })
    }
}

impl sealed::Sealed for ParserWorkerProcess {}

impl WorkerProcess for ParserWorkerProcess {
    type Io = WorkerChannel;

    fn io(&self) -> Self::Io {
        WorkerChannel {
            handle: Arc::clone(&self.handle),
        }
    }

    fn close_channel(&self) {
        self.handle.lock().close_channel();
    }

    fn wait(&self, deadline: Instant) -> WaitOutcome {
        match self.handle.lock().wait(deadline) {
            Ok(kind) => map_exit(kind),
            Err(error) => Err(map_io_error(error)),
        }
    }

    fn terminate_and_wait(&self, deadline: Instant) -> WaitOutcome {
        // The native call aborts the channel itself; recording it here keeps
        // a later transfer terminal rather than merely failing.
        self.handle.aborted.store(true, Ordering::Release);
        self.handle.cancellation.request_cancellation();
        match self.handle.lock().terminate_and_wait(deadline) {
            Ok(kind) => map_exit(kind),
            Err(error) => Err(map_io_error(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use ohl_platform::{IsolatedWorkerError, IsolatedWorkerExitKind};

    use super::{IoError, WorkerExit, map_exit, map_io_error};

    #[test]
    fn every_native_error_maps_to_a_fixed_transport_code() {
        assert_eq!(
            map_io_error(IsolatedWorkerError::Timeout),
            IoError::TimedOut
        );
        assert_eq!(
            map_io_error(IsolatedWorkerError::Cancelled),
            IoError::Cancelled
        );
        assert_eq!(
            map_io_error(IsolatedWorkerError::PeerClosed),
            IoError::PeerClosed
        );
        assert_eq!(
            map_io_error(IsolatedWorkerError::InvalidState),
            IoError::Aborted
        );
        for error in [
            IsolatedWorkerError::Unsupported,
            IsolatedWorkerError::ServiceUnavailable,
            IsolatedWorkerError::ServiceIdentityMismatch,
            IsolatedWorkerError::ConfinementUnavailable,
            IsolatedWorkerError::BootstrapFailed,
            IsolatedWorkerError::TransferTooLarge,
            IsolatedWorkerError::IoFailure,
            IsolatedWorkerError::ReapFailed,
        ] {
            assert_eq!(map_io_error(error), IoError::IoFailure);
        }
    }

    #[test]
    fn only_a_clean_exit_reports_success() {
        assert_eq!(
            map_exit(IsolatedWorkerExitKind::Clean),
            Ok(WorkerExit::Exited(0))
        );
        assert_eq!(
            map_exit(IsolatedWorkerExitKind::Failed),
            Ok(WorkerExit::Exited(1))
        );
        for kind in [
            IsolatedWorkerExitKind::Crashed,
            IsolatedWorkerExitKind::ResourceLimit,
            IsolatedWorkerExitKind::Terminated,
        ] {
            assert_eq!(map_exit(kind), Ok(WorkerExit::Terminated));
        }
        for kind in [
            IsolatedWorkerExitKind::Running,
            IsolatedWorkerExitKind::Unknown,
        ] {
            assert_eq!(map_exit(kind), Err(IoError::IoFailure));
        }
    }

    #[test]
    fn an_unsupported_target_reports_a_sanitized_launch_failure() {
        // On Linux x86-64 the image may or may not be installed, so only the
        // targets with no backend at all have a fixed expectation.
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            assert_eq!(
                super::ParserWorkerProcess::launch(deadline).err(),
                Some(IsolatedWorkerError::Unsupported)
            );
        }
    }
}
