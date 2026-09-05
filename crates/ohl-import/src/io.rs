//! The exact-I/O capability the parent frames OWP/1 over.
//!
//! This is the Rust port of the C++ `ParserFrameChannelOperations` function
//! table. Where C++ passed four raw pointers that a caller had to validate at
//! runtime (`operations.valid()`), Rust passes one value implementing the
//! [`ExactIo`] trait: an invalid operation table is unrepresentable.
//!
//! The trait is **sealed**: only this crate can implement it, so media-derived
//! code can never become the parent's transport. Two implementations exist
//! today — the deterministic
//! [`SyntheticTransport`](crate::testing::SyntheticTransport) used by the
//! tests, and the blanket [`Arc`] forwarder that lets one capability back both
//! directions of a channel. The `ohl-platform` `IsolatedWorker` adapter is
//! added in R4.7.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use thiserror::Error;

pub(crate) mod sealed {
    /// Prevents downstream implementations of the capability traits.
    pub trait Sealed {}
}

/// Every way one exact transfer can fail.
///
/// Each variant is a fixed, project-defined code: no variant carries data, so
/// neither `Display` nor `Debug` can interpolate a media-derived byte, a path,
/// or an OS error string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Error)]
#[non_exhaustive]
pub enum IoError {
    /// The transfer failed, or the implementation reported an impossible
    /// byte count (short, zero or over-long "success").
    #[error("transport io failure")]
    IoFailure,
    /// The deadline passed before the transfer completed.
    #[error("transport deadline exceeded")]
    TimedOut,
    /// The caller's cancellation token was signalled.
    #[error("transport cancelled")]
    Cancelled,
    /// The peer closed its end of the channel.
    #[error("transport peer closed")]
    PeerClosed,
    /// [`ExactIo::abort_io`] terminally interrupted the channel.
    #[error("transport aborted")]
    Aborted,
}

/// A cooperative cancellation token handed to a blocking transfer.
///
/// A default token is never signalled; cloning shares one signal.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    flag: Option<Arc<AtomicBool>>,
}

impl CancellationToken {
    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }
}

/// The owning half of a [`CancellationToken`].
#[derive(Debug, Default)]
pub struct CancellationSource {
    flag: Arc<AtomicBool>,
}

impl CancellationSource {
    /// Creates an unsignalled source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A token observing this source.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            flag: Some(Arc::clone(&self.flag)),
        }
    }

    /// Requests cancellation. Idempotent.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }
}

/// A trusted, non-owning exact byte channel for exactly one worker.
///
/// Implementations must:
///
/// - transfer the **complete** non-empty slice on success and report the
///   transferred byte count; every other outcome is terminal for the channel;
/// - support one read and one write concurrently (hence `Send + Sync`);
/// - make [`ExactIo::abort_io`] idempotent, concurrency-safe, callable while
///   either or both transfers are active, and promptly return both of them.
///
/// Holding this capability grants no launch, termination, reap,
/// executable-selection or process-ownership authority.
pub trait ExactIo: sealed::Sealed + Send + Sync {
    /// Fills `destination` completely, returning the bytes transferred.
    ///
    /// # Errors
    /// Any [`IoError`]; the channel treats every failure as terminal.
    fn read_exact(
        &self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError>;

    /// Writes all of `source`, returning the bytes transferred.
    ///
    /// # Errors
    /// Any [`IoError`]; the channel treats every failure as terminal.
    fn write_all(
        &self,
        source: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError>;

    /// Terminally interrupts both directions. Idempotent.
    fn abort_io(&self);
}

impl<T: ExactIo> sealed::Sealed for Arc<T> {}

impl<T: ExactIo> ExactIo for Arc<T> {
    fn read_exact(
        &self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        self.as_ref()
            .read_exact(destination, deadline, cancellation)
    }

    fn write_all(
        &self,
        source: &[u8],
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<usize, IoError> {
        self.as_ref().write_all(source, deadline, cancellation)
    }

    fn abort_io(&self) {
        self.as_ref().abort_io();
    }
}

#[cfg(test)]
mod tests {
    use super::{CancellationSource, CancellationToken};

    #[test]
    fn a_default_token_is_never_cancelled() {
        assert!(!CancellationToken::default().is_cancelled());
    }

    #[test]
    fn a_source_signals_every_token_it_issued() {
        let source = CancellationSource::new();
        let first = source.token();
        let second = first.clone();
        assert!(!first.is_cancelled());
        source.cancel();
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        assert!(source.token().is_cancelled());
    }
}
