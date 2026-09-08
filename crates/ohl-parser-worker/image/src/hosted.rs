//! The hosted (`std`) media-parser worker image.
//!
//! This is the confined side of the OWP/1 contract on Linux x86-64 and macOS.
//! It is an ordinary Rust binary. Linux builds use the static
//! `x86_64-unknown-linux-musl` target and are audited as non-PIE `ET_EXEC`
//! files with no interpreter or dynamic segment. macOS builds are thin
//! `MH_EXECUTE` Mach-O files linking only `/usr/lib/libSystem.B.dylib`.
//! Their respective backends confine the process with descriptors 3
//! (channel) and 4 (readiness) already in place and the resource limits
//! already applied.
//! It writes the readiness attestation on descriptor 4, hosts exactly one
//! `run_parser_worker_service` lifetime over descriptor 3 with
//! `ohl_parser_backends::ContainerDispatcher`, and exits with a fixed status.
//! It parses no arguments, reads no environment and opens no file.
//!
//! # The heap
//!
//! This image has a real allocator. On macOS, the host cannot put a ceiling
//! under it: Darwin rejects `setrlimit` for
//! both `RLIMIT_AS` and `RLIMIT_DATA` outright (`EINVAL`), so those limits
//! are absent from the macOS backend's table and no kernel-enforced memory
//! bound exists on this platform.
//! [`BoundedSystem`] is: a counting wrapper around the system allocator that
//! refuses any allocation which would take the live total past
//! [`HEAP_CEILING_BYTES`]. Refusal is the standard allocation-failure abort,
//! which the host reports as a crashed worker.
//!
//! # Exit statuses
//!
//! `contract.rs` defines: `0` orderly shutdown or orderly peer close, `64`
//! protocol failure, `65` dispatcher
//! `unsupported`, `66` transport failure, `70` anything else, including a
//! panic (a panic hook turns the `panic = "abort"` into that exit status).
//!
//! # Hosted transport behavior
//!
//! - `SIGPIPE` is ignored (every Rust `std` binary starts that way), so a
//!   parent that vanishes mid-write produces `EPIPE`, a transport failure,
//!   rather than a signalled exit.
//! - Input is probed with a non-blocking one-byte `read` into a private
//!   pushback slot that the next `read_exact` drains first (`std` has no
//!   stable `MSG_PEEK`), so no byte is ever lost or reordered and the probe
//!   is non-consuming as far as the service can observe, which is what the
//!   transport contract requires.
//!
//! # `unsafe` inventory
//!
//! `unsafe_code` is allowed for this package only. This file has two kinds
//! of unsafe site:
//!
//! | # | Site | Why it is needed | Why it is sound |
//! |---|------|------------------|-----------------|
//! | 1 | `channel`, `attest_readiness` (`from_raw_fd`) | The two inherited descriptors have to become `std` objects. | The host contract guarantees 3 and 4 are open and owned by nobody else in this process; each is adopted exactly once, on the only thread, before the service starts. |
//! | 2 | `BoundedSystem` (`unsafe impl GlobalAlloc`) | `GlobalAlloc` is an unsafe trait. | Every call forwards to `std::alloc::System` with the caller's own `Layout` unchanged, so the pointer contract is `System`'s; the only addition is an atomic byte count, whose failure path returns null, which callers of `GlobalAlloc` must already handle. |

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::FromRawFd as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};

use ohl_parser_backends::{BackendLimits, ContainerDispatcher};
use ohl_parser_protocol::MAXIMUM_FRAME_PAYLOAD_BYTES;
use ohl_parser_worker_service::{
    InputStatus, IoStatus, ServiceBuffers, ServiceError, ServiceFailure, ServiceLimits, Transport,
    run_parser_worker_service,
};

include!("../../src/contract.rs");

const PAYLOAD_BYTES: usize = MAXIMUM_FRAME_PAYLOAD_BYTES as usize;

/// The live-heap ceiling, in bytes. It is far above what the back ends need
/// and gives macOS the memory ceiling its kernel cannot impose.
const HEAP_CEILING_BYTES: usize = 128 * 1024 * 1024;

/// The spelling storage the dispatcher copies offered names into.
const SPELLING_ARENA_BYTES: usize = 8 * 1024 * 1024;

// ------------------------------------------------------------- the heap ----

/// The system allocator behind a live-byte ceiling. See inventory #2.
struct BoundedSystem {
    live: AtomicUsize,
}

impl BoundedSystem {
    /// Reserves `size` bytes of the ceiling, or reports that it is exhausted.
    fn reserve(&self, size: usize) -> bool {
        let mut current = self.live.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(size) else {
                return false;
            };
            if next > HEAP_CEILING_BYTES {
                return false;
            }
            match self.live.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self, size: usize) {
        self.live.fetch_sub(size, Ordering::Relaxed);
    }
}

// SAFETY: inventory #2.
unsafe impl GlobalAlloc for BoundedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: the layout is the caller's, unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            self.release(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout are the ones `alloc` handed out.
        unsafe { System.dealloc(pointer, layout) };
        self.release(layout.size());
    }
}

#[global_allocator]
static HEAP: BoundedSystem = BoundedSystem {
    live: AtomicUsize::new(0),
};

#[cfg(test)]
mod heap_tests {
    use super::{AtomicUsize, BoundedSystem, HEAP_CEILING_BYTES, Ordering};

    #[test]
    fn the_live_heap_ceiling_is_released_for_later_allocations() {
        let heap = BoundedSystem {
            live: AtomicUsize::new(0),
        };
        assert!(heap.reserve(HEAP_CEILING_BYTES));
        assert!(!heap.reserve(1));

        heap.release(HEAP_CEILING_BYTES);
        assert!(heap.reserve(1));
        assert_eq!(heap.live.load(Ordering::Relaxed), 1);
    }
}

// ------------------------------------------------------------ transport ----

/// Exact synchronous I/O over the pre-opened channel descriptor.
struct ChannelTransport {
    stream: Option<UnixStream>,
    /// One byte `probe_input` had to take off the socket to learn that input
    /// was available; `read_exact` hands it back first.
    pushback: Option<u8>,
}

impl ChannelTransport {
    fn new() -> Self {
        // SAFETY: inventory #1. Descriptor 3 is the channel the host placed
        // there, open, and adopted exactly once, here.
        let stream = unsafe { UnixStream::from_raw_fd(CHANNEL_FD) };
        Self {
            stream: Some(stream),
            pushback: None,
        }
    }

    /// `abort_io` and `close_io` are the same operation here and must be
    /// idempotent, so the descriptor is closed at most once.
    fn end(&mut self) {
        self.stream = None;
    }
}

impl Transport for ChannelTransport {
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus {
        let Some(stream) = self.stream.as_mut() else {
            return IoStatus::Failed;
        };
        let mut offset = 0;
        if let (Some(byte), Some(first)) = (self.pushback.take(), destination.first_mut()) {
            *first = byte;
            offset = 1;
        }
        while offset < destination.len() {
            match stream.read(&mut destination[offset..]) {
                // End of file is an orderly close only on a frame boundary; a
                // truncated frame is a failure.
                Ok(0) => {
                    return if offset == 0 {
                        IoStatus::PeerClosed
                    } else {
                        IoStatus::Failed
                    };
                }
                Ok(amount) => offset += amount,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return IoStatus::Failed,
            }
        }
        IoStatus::Ok
    }

    fn write_all(&mut self, source: &[u8]) -> IoStatus {
        let Some(stream) = self.stream.as_mut() else {
            return IoStatus::Failed;
        };
        let mut offset = 0;
        while offset < source.len() {
            match stream.write(&source[offset..]) {
                Ok(0) => return IoStatus::Failed,
                Ok(amount) => offset += amount,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return IoStatus::Failed,
            }
        }
        IoStatus::Ok
    }

    fn probe_input(&mut self) -> InputStatus {
        if self.pushback.is_some() {
            return InputStatus::Available;
        }
        let Some(stream) = self.stream.as_mut() else {
            return InputStatus::Failed;
        };
        if stream.set_nonblocking(true).is_err() {
            return InputStatus::Failed;
        }
        let mut byte = [0u8; 1];
        let probed = loop {
            match stream.read(&mut byte) {
                Ok(0) => break InputStatus::PeerClosed,
                Ok(_) => {
                    self.pushback = Some(byte[0]);
                    break InputStatus::Available;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    break InputStatus::Unavailable;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break InputStatus::Failed,
            }
        };
        if stream.set_nonblocking(false).is_err() {
            return InputStatus::Failed;
        }
        probed
    }

    fn abort_io(&mut self) {
        self.end();
    }

    fn close_io(&mut self) {
        self.end();
    }
}

// -------------------------------------------------------------- service ----

/// Writes the readiness attestation on [`READY_FD`] and closes it.
fn attest_readiness() -> bool {
    // SAFETY: inventory #1, for descriptor 4; dropping the `File` at the end
    // of this function is what closes it and signals end-of-file to the host.
    let mut ready = unsafe { File::from_raw_fd(READY_FD) };
    ready.write_all(&READY_ATTESTATION).is_ok() && ready.flush().is_ok()
}

/// The exit status for one fail-closed lifetime.
const fn failure_status(failure: &ServiceFailure) -> i32 {
    match failure.error {
        // A parent that closes the channel instead of sending `shutdown` is
        // an orderly end of the worker's job, not a failure of its own.
        ServiceError::TransportFailure => match failure.io_status {
            IoStatus::PeerClosed => WORKER_CLEAN_EXIT,
            _ => WORKER_TRANSPORT_FAILURE_EXIT,
        },
        ServiceError::ProtocolFailure => WORKER_PROTOCOL_FAILURE_EXIT,
        ServiceError::DispatchUnsupported => WORKER_UNSUPPORTED_EXIT,
        _ => WORKER_INTERNAL_FAILURE_EXIT,
    }
}

fn serve() -> i32 {
    if !attest_readiness() {
        return WORKER_TRANSPORT_FAILURE_EXIT;
    }
    let mut receive_payload = vec![0u8; PAYLOAD_BYTES];
    let mut send_payload = vec![0u8; PAYLOAD_BYTES];
    let mut spellings = vec![0u8; SPELLING_ARENA_BYTES];
    let outcome = run_parser_worker_service(
        ChannelTransport::new(),
        ContainerDispatcher::new(&mut spellings, BackendLimits::default()),
        ServiceBuffers {
            receive_payload: &mut receive_payload,
            send_payload: &mut send_payload,
        },
        ServiceLimits::default(),
    );
    match outcome {
        Ok(_) => WORKER_CLEAN_EXIT,
        Err(failure) => failure_status(&failure),
    }
}

pub(crate) fn main() -> ! {
    // A panic must be the contract's internal-failure status, not `SIGABRT`,
    // and must print nothing (there is nowhere to print to anyway: 0/1/2 are
    // `/dev/null`).
    std::panic::set_hook(Box::new(|_| {
        std::process::exit(WORKER_INTERNAL_FAILURE_EXIT)
    }));
    std::process::exit(serve())
}
