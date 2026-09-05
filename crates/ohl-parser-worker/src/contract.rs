// The launch contract shared by the host side of this crate, the integration
// test, and the freestanding worker image itself.
//
// This file is compiled twice: once as a module of the `ohl-parser-worker`
// library (`std`), and once via `include!` from the `#![no_std]` image. It
// therefore contains nothing but `const` items.
//
// The descriptor numbers and the readiness attestation must stay identical to
// `ohl-platform`'s isolated-worker backend; the integration test asserts that
// by launching a real confined worker.

/// Descriptor the private full-duplex OWP/1 channel is bound to in the child.
pub const CHANNEL_FD: i32 = 3;

/// Descriptor the one-shot readiness pipe is bound to in the child.
pub const READY_FD: i32 = 4;

/// The exact byte sequence the worker writes to [`READY_FD`] before closing
/// it. The host accepts a worker only after reading this in full and then
/// observing end-of-file.
pub const READY_ATTESTATION: [u8; 16] = [
    b'O', b'H', b'L', b'I', b'S', b'O', b'L', b'A', b'T', b'E', b'D', 0, 1, 0, 0, 0,
];

/// Orderly shutdown, or an orderly peer close. Mirrors the C++ worker's
/// `kCleanExit`.
pub const WORKER_CLEAN_EXIT: i32 = 0;

/// A frame, payload or ordering rule was violated. Mirrors
/// `kProtocolErrorExit`.
pub const WORKER_PROTOCOL_FAILURE_EXIT: i32 = 64;

/// The compile-fixed dispatcher refused the request. Mirrors
/// `kUnsupportedExit`.
pub const WORKER_UNSUPPORTED_EXIT: i32 = 65;

/// The channel failed. Mirrors `kTransportErrorExit`.
pub const WORKER_TRANSPORT_FAILURE_EXIT: i32 = 66;

/// Any other fail-closed outcome, including a panic. Mirrors
/// `kInternalErrorExit`.
pub const WORKER_INTERNAL_FAILURE_EXIT: i32 = 70;
