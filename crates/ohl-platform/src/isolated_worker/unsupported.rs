//! Fallback backend for every target without native containment.
//!
//! It is uninhabited on purpose: `launch` is the only constructor and it
//! always fails, so no other method can ever be reached. That keeps the
//! public API identical everywhere without a single stubbed-out behaviour
//! that could be mistaken for real confinement.

use super::{
    IsolatedWorkerCancellationToken, IsolatedWorkerError, IsolatedWorkerExitKind,
    IsolatedWorkerService,
};
use std::time::Instant;

#[derive(Debug)]
pub(super) enum Backend {}

impl Backend {
    pub(super) fn launch(
        _service: IsolatedWorkerService,
        _startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        Err(IsolatedWorkerError::Unsupported)
    }

    pub(super) fn read_exact(
        &mut self,
        _destination: &mut [u8],
        _deadline: Instant,
        _cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        match *self {}
    }

    pub(super) fn write_all(
        &mut self,
        _source: &[u8],
        _deadline: Instant,
        _cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        match *self {}
    }

    pub(super) fn abort_io(&mut self) {
        match *self {}
    }

    pub(super) fn close_channel(&mut self) {
        match *self {}
    }

    pub(super) fn wait(
        &mut self,
        _deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        match *self {}
    }

    pub(super) fn terminate_and_wait(
        &mut self,
        _deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        match *self {}
    }
}
