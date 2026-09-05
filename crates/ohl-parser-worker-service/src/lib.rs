//! The worker side of one complete OWP/1 session lifetime.
//!
//! This crate is the Rust port of the C++ contract in
//! `src/parser/src/parser_worker_service_internal.hpp` and
//! `parser_worker_service.cpp`. It owns no capability of its own: the caller
//! supplies a [`Transport`] (exact synchronous I/O over the already-open
//! channel), a [`Dispatcher`] (the trusted parser back end) and two scratch
//! buffers, and [`run_parser_worker_service`] drives exactly one
//! `hello`/`ready` ... `shutdown` lifetime over them.
//!
//! Like [`ohl_parser_protocol`], it is `#![no_std]`, allocation-free and
//! `#![forbid(unsafe_code)]`, because the identical code is linked into the
//! freestanding worker binary (`ohl-parser-worker`), which has neither an
//! allocator nor a libc.
//!
//! # What the type system replaces
//!
//! The C++ contract carried several invariants as runtime checks that Rust
//! makes unrepresentable. They are listed here so a reader comparing the two
//! implementations can see nothing was dropped:
//!
//! - **Function-pointer tables with `void* context`** become the
//!   [`Transport`] and [`Dispatcher`] traits. `valid()` (all members
//!   non-null) disappears: a trait object or generic parameter always has a
//!   complete method table and a live receiver.
//! - **Buffer overlap checks** (`ranges_overlap`) disappear:
//!   [`ServiceBuffers`] holds two `&mut [u8]`, which the borrow checker
//!   already proves disjoint, and a dispatcher cannot name them at all.
//! - **"Views are dead after the next callback"** becomes a lifetime: a
//!   [`DispatchAction`] borrows the dispatcher, so the service must finish
//!   encoding it before it can touch the dispatcher again, and it can never
//!   read a stale view.
//! - **"Unused step fields must be zero/empty"** disappears:
//!   [`DispatchAction`] is a sum type, so a `need_read` action has no entry
//!   or data fields to leave dirty.
//! - **Out-of-range budget structs** are rejected by
//!   [`ohl_parser_protocol::ProtocolBudgets`] at construction, so
//!   [`ServiceLimits`] can only carry a valid protocol budget.
//! - **Out-of-table enum values** (for example a `probe_input` result of
//!   `0xff`, which the C++ test maps to `internal_failure`) are
//!   unrepresentable; [`ServiceError::InternalFailure`] remains for the
//!   invariant violations that are still reachable.
//!
//! # Failure discipline
//!
//! Every non-clean return aborts the transport exactly once and, if a
//! dispatch is still active, cancels it exactly once. Invalid configuration
//! is rejected before any read, write or probe. `unsupported` from the
//! dispatcher is terminal and never puts a frame on the wire. Errors are
//! fixed, project-defined codes ([`ServiceError`] and
//! [`ohl_parser_protocol::ProtocolError`]); no variant carries media-derived
//! bytes, so neither `Debug` nor `Display` can leak them.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod capability;
mod service;

pub use capability::{
    DispatchAction, DispatchError, Dispatcher, InputStatus, IoStatus, Operation, ServiceBuffers,
    Transport, UnsupportedDispatcher,
};
pub use service::{
    MAXIMUM_DISPATCH_STEPS, ServiceError, ServiceFailure, ServiceLimits, ServiceSummary,
    run_parser_worker_service,
};
