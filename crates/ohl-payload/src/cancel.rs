//! Cooperative, phase-checked cancellation.
//!
//! A port of the C++ `ohl::media::CancellationToken` / `CancellationSource`
//! pair. Cancellation here is *cooperative*: nothing is interrupted, killed,
//! or unwound. A [`CancellationSource`] flips one atomic flag, and the phases
//! of staging poll [`CancellationToken::stop_requested`] at the boundaries
//! where stopping is safe — before a transaction exists, between chunks,
//! after source reverification, and immediately before publication. That is
//! why a cancelled stage never leaves a published tree behind: the poll points
//! are chosen, not arbitrary.
//!
//! Clones of a source and its tokens share one state and one requested flag,
//! and [`CancellationSource::request_stop`] succeeds only for the first
//! request made through any of them.
//!
//! # Token lifetime after the last source
//!
//! Tokens keep their state alive after every source referring to it is
//! dropped. An *unstopped* token then reports
//! [`CancellationToken::stop_possible`] as `false` — nothing can ever request
//! a stop through it again, so a consumer can stop polling — while an
//! already-stopped token keeps reporting both possible and requested, because
//! the request it observed remains true forever.
//!
//! A default [`CancellationToken`] has no state: it can never be stopped and
//! never reports cancellation, so `CancellationToken::default()` is the right
//! argument for a caller that has nothing to cancel.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The state a source and its tokens share.
#[derive(Debug)]
struct CancellationState {
    /// Set once, by the first successful request, and never cleared.
    stop_requested: AtomicBool,
    /// How many live sources could still request a stop.
    source_count: AtomicUsize,
}

/// A cheap, cloneable cancellation observation handle.
///
/// See the [module documentation](self) for the lifetime and default-token
/// contract. Two tokens compare equal exactly when they observe the same
/// state, which is what lets a callee assert it was handed the caller's token
/// rather than a fresh one.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    /// `None` for a default token, which can never be stopped.
    state: Option<Arc<CancellationState>>,
}

impl CancellationToken {
    /// Whether a stop could still be requested, or already has been.
    ///
    /// `false` means the answer can never change again: either this is a
    /// default token, or every source is gone and none of them requested a
    /// stop.
    pub fn stop_possible(&self) -> bool {
        self.state.as_ref().is_some_and(|state| {
            // Once the source count reaches zero no new request can begin, so
            // reading the flag afterwards closes the last-source race: a
            // request that was in flight has already set the flag.
            state.source_count.load(Ordering::Acquire) != 0
                || state.stop_requested.load(Ordering::Acquire)
        })
    }

    /// Whether a stop has been requested.
    ///
    /// This is the predicate every cooperative phase check polls.
    pub fn stop_requested(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.stop_requested.load(Ordering::Acquire))
    }
}

impl PartialEq for CancellationToken {
    /// Compares observed state identity, not the requested flag's value.
    fn eq(&self, other: &Self) -> bool {
        match (self.state.as_ref(), other.state.as_ref()) {
            (None, None) => true,
            (Some(first), Some(second)) => Arc::ptr_eq(first, second),
            _ => false,
        }
    }
}

impl Eq for CancellationToken {}

/// An owner of cancellation state.
///
/// Clones refer to the same state; see the [module
/// documentation](self). Dropping the last source does not clear a request
/// that was already made.
#[derive(Debug)]
pub struct CancellationSource {
    /// The shared state. Always present for a live source.
    state: Arc<CancellationState>,
}

impl CancellationSource {
    /// Creates a source with fresh, unrequested state.
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancellationState {
                stop_requested: AtomicBool::new(false),
                source_count: AtomicUsize::new(1),
            }),
        }
    }

    /// A token observing this source's state.
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            state: Some(Arc::clone(&self.state)),
        }
    }

    /// Whether a stop has been requested through any source sharing the state.
    pub fn stop_requested(&self) -> bool {
        self.state.stop_requested.load(Ordering::Acquire)
    }

    /// Requests a stop.
    ///
    /// Returns `true` only for the first request made through any source
    /// sharing this state, so exactly one caller can own the "I cancelled it"
    /// decision even under concurrency.
    pub fn request_stop(&self) -> bool {
        self.state
            .stop_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

impl Default for CancellationSource {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CancellationSource {
    fn clone(&self) -> Self {
        self.state.source_count.fetch_add(1, Ordering::Relaxed);
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl Drop for CancellationSource {
    fn drop(&mut self) {
        self.state.source_count.fetch_sub(1, Ordering::Release);
    }
}

impl PartialEq for CancellationSource {
    /// Compares owned state identity.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for CancellationSource {}

#[cfg(test)]
mod tests {
    use super::{CancellationSource, CancellationToken};
    use alloc::vec::Vec;

    #[test]
    fn a_default_token_can_never_be_stopped() {
        let first = CancellationToken::default();
        let second = CancellationToken::default();
        assert!(!first.stop_possible());
        assert!(!first.stop_requested());
        assert_eq!(first, second);
    }

    #[test]
    fn fresh_sources_have_distinct_identities() {
        let source = CancellationSource::new();
        let distinct = CancellationSource::default();
        let token = source.token();

        assert!(!source.stop_requested());
        assert!(token.stop_possible());
        assert!(!token.stop_requested());
        assert_ne!(source, distinct);
        assert_ne!(token, distinct.token());
        assert_ne!(token, CancellationToken::default());
    }

    #[test]
    fn clones_share_one_identity_and_one_idempotent_request() {
        let source = CancellationSource::new();
        let token = source.token();
        let cloned_token = token.clone();
        let cloned_source = source.clone();
        assert_eq!(cloned_token, token);
        assert_eq!(cloned_source, source);

        assert!(cloned_source.request_stop());
        assert!(!cloned_source.request_stop());
        assert!(!source.request_stop());
        assert!(source.stop_requested());
        assert!(token.stop_requested());
        assert!(cloned_token.stop_requested());
        assert!(token.stop_possible());
    }

    #[test]
    fn an_unstopped_token_retires_when_its_last_source_goes() {
        let unstopped = {
            let source = CancellationSource::new();
            let copy = source.clone();
            let token = source.token();
            assert!(token.stop_possible());
            drop(copy);
            assert!(token.stop_possible(), "a live copy keeps stopping possible");
            token
        };
        assert!(!unstopped.stop_possible());
        assert!(!unstopped.stop_requested());

        let requested = {
            let source = CancellationSource::new();
            let token = source.token();
            assert!(source.request_stop());
            token
        };
        assert!(requested.stop_possible());
        assert!(requested.stop_requested());
    }

    #[test]
    fn a_stop_request_is_observed_across_threads() {
        let source = CancellationSource::new();
        let token = source.token();
        let observer = std::thread::spawn(move || {
            while !token.stop_requested() {
                std::thread::yield_now();
            }
            token.stop_possible() && token.stop_requested()
        });
        assert!(source.request_stop());
        assert!(observer.join().expect("observer thread"));
    }

    #[test]
    fn concurrent_requests_have_exactly_one_winner() {
        let contested = CancellationSource::new();
        let workers = (0..8)
            .map(|_| {
                let copy = contested.clone();
                std::thread::spawn(move || copy.request_stop())
            })
            .collect::<Vec<_>>();
        let winners = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker thread"))
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
        assert!(contested.stop_requested());
        assert!(contested.token().stop_requested());
    }
}
