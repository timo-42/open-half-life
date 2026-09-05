//! The freestanding Linux x86-64 media-parser worker image.
//!
//! This binary is the confined side of the OWP/1 contract. It writes the
//! readiness attestation on descriptor 4, hosts exactly one
//! `run_parser_worker_service` lifetime over descriptor 3 with the
//! compile-fixed `UnsupportedDispatcher`, and exits with a fixed status. It
//! parses no arguments, reads no environment, opens no file and allocates
//! nothing: there is no allocator at all, so a heap request cannot compile,
//! let alone run.
//!
//! It is built with `-nostdlib -static -no-pie` and issues raw `syscall`
//! instructions only, so `ohl-platform`'s seccomp allowlist (`execveat`,
//! `read`, `write`, `close`, `ppoll`, `exit`, `exit_group`) is sufficient and
//! any other syscall is a genuine policy violation rather than libc noise.
//!
//! # Exit statuses
//!
//! They mirror the C++ worker's codes (see `contract.rs`, included below):
//! `0` orderly shutdown or orderly peer close, `64` protocol failure, `65`
//! dispatcher `unsupported`, `66` transport failure, `70` anything else,
//! including a panic and a rejected configuration.
//!
//! # Deviations from the C++ worker
//!
//! - The C++ worker ignored `SIGPIPE` with `rt_sigaction` and wrote with
//!   `sendto(MSG_NOSIGNAL)`. Neither syscall is on the allowlist, so this
//!   image writes with `write(2)` and, if the parent vanishes mid-write, dies
//!   on `SIGPIPE`. The host reports that as a signalled exit, which is an
//!   observable teardown, not a silent one.
//! - Input is probed with a zero-timeout `ppoll(2)` instead of
//!   `recvfrom(MSG_PEEK | MSG_DONTWAIT)`, for the same reason. `ppoll` never
//!   consumes bytes, which is what the transport contract requires.
//!
//! # `unsafe` inventory
//!
//! `unsafe_code` is allowed for this package only, and only for the five
//! kinds of site listed here. There is no allocator, no `static mut`, no
//! transmute, and no unsafe trait implementation beyond the one `Sync` marker
//! in row 5.
//!
//! | # | Site | Why it is needed | Why it is sound |
//! |---|------|------------------|-----------------|
//! | 1 | `global_asm!` `_start` | A `-nostdlib` image has no C runtime, so the ELF entry point is written by hand: clear the frame pointer, align the stack to 16 bytes, tail-call into Rust. | The block touches only `rbp`/`rsp`/`rdi` before calling a diverging `extern "C"` function, exactly as the System V AMD64 process-entry ABI prescribes. |
//! | 2 | `syscall3`, `syscall4`, `exit_group` | `core` exposes no syscall interface and no C library is linked. | Each wrapper passes only integers and pointers into live, correctly sized local buffers, declares the `rcx`/`r11`/`memory` clobbers the `syscall` instruction requires, and never lets the kernel touch memory outside a slice the caller owns. |
//! | 3 | `rust_eh_personality` stub | The precompiled stable `core` was built with unwind tables, so a `DW.ref.rust_eh_personality` relocation survives into the image even though this package is compiled with `panic = "abort"`. | Nothing can ever unwind: the panic handler diverges into `exit_group`, so the symbol is never called. |
//! | 4 | `memcpy`/`memset`/`memmove`/`memcmp` | The prebuilt stable `compiler_builtins` does not ship its `mem` feature, so `core`'s slice codegen emits calls to these four symbols with nothing to resolve them once libc is gone. | Byte-at-a-time loops honouring the documented `memcpy`/`memmove` overlap rules; the callers are `core` itself, which upholds the pointer and length preconditions. |
//! | 5 | `PayloadBuffer` (`UnsafeCell` + `unsafe impl Sync`) and the two `&mut` it hands out | The service needs two 1 MiB scratch buffers and there is no allocator, so they must live in `.bss`. | The process is single-threaded by construction (`RLIMIT_NPROC` is 1 and neither `clone` nor `fork` is on the seccomp allowlist), and `PayloadBuffer::take` is called exactly once per buffer, on the only thread, before the service starts, so no second `&mut` can exist. |

#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::panic::PanicInfo;

use ohl_parser_protocol::MAXIMUM_FRAME_PAYLOAD_BYTES;
use ohl_parser_worker_service::{
    InputStatus, IoStatus, ServiceBuffers, ServiceError, ServiceFailure, ServiceLimits, Transport,
    UnsupportedDispatcher, run_parser_worker_service,
};

include!("../../src/contract.rs");

const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_CLOSE: usize = 3;
const SYS_PPOLL: usize = 271;
const SYS_EXIT_GROUP: usize = 231;

const POLLIN: i16 = 0x0001;
const POLLHUP: i16 = 0x0010;

const PAYLOAD_BYTES: usize = MAXIMUM_FRAME_PAYLOAD_BYTES as usize;

/// `EINTR`, the only error the I/O loops retry.
const INTERRUPTED: isize = -4;

// ------------------------------------------------------- entry and exit ----

global_asm!(
    ".global _start",
    ".text",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call ohl_media_parser_worker_start",
    "ud2",
);

/// The Rust side of the ELF entry point.
///
/// Called exactly once by `_start` with the initial stack pointer and a
/// 16-byte aligned stack; never returns.
#[unsafe(no_mangle)]
extern "C" fn ohl_media_parser_worker_start(_initial_stack: *const usize) -> ! {
    exit_group(serve());
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit_group(WORKER_INTERNAL_FAILURE_EXIT)
}

#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}

fn exit_group(status: i32) -> ! {
    // SAFETY: inventory #2. `exit_group` never returns and touches no memory.
    unsafe {
        asm!(
            "syscall",
            in("rax") SYS_EXIT_GROUP,
            in("rdi") status as isize,
            options(noreturn, nostack),
        )
    }
}

// ------------------------------------------------------------- syscalls ----

// SAFETY: inventory #2 for both wrappers.
unsafe fn syscall3(number: usize, a0: isize, a1: isize, a2: isize) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

unsafe fn syscall4(number: usize, a0: isize, a1: isize, a2: isize, a3: isize) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

fn raw_close(descriptor: i32) {
    // SAFETY: inventory #2. `close` takes an integer and writes no memory.
    unsafe { syscall3(SYS_CLOSE, descriptor as isize, 0, 0) };
}

fn raw_read(descriptor: i32, buffer: &mut [u8]) -> isize {
    // SAFETY: inventory #2. The kernel writes at most `buffer.len()` bytes
    // into a pointer inside a live, exclusively borrowed slice.
    unsafe {
        syscall3(
            SYS_READ,
            descriptor as isize,
            buffer.as_mut_ptr() as isize,
            buffer.len() as isize,
        )
    }
}

fn raw_write(descriptor: i32, bytes: &[u8]) -> isize {
    // SAFETY: inventory #2. The kernel reads at most `bytes.len()` bytes from
    // a pointer into a live shared slice.
    unsafe {
        syscall3(
            SYS_WRITE,
            descriptor as isize,
            bytes.as_ptr() as isize,
            bytes.len() as isize,
        )
    }
}

#[repr(C)]
struct PollFd {
    descriptor: i32,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct TimeSpec {
    seconds: i64,
    nanoseconds: i64,
}

/// A zero-timeout `ppoll` on `descriptor`, returning its `revents` or a
/// negative errno.
fn raw_poll_now(descriptor: i32, events: i16) -> isize {
    let mut item = PollFd {
        descriptor,
        events,
        revents: 0,
    };
    let timeout = TimeSpec {
        seconds: 0,
        nanoseconds: 0,
    };
    loop {
        // SAFETY: inventory #2. The kernel reads one `pollfd` and one
        // `timespec` this frame owns, writes back only `item.revents`, and is
        // given a null signal mask.
        let result = unsafe {
            syscall4(
                SYS_PPOLL,
                (&raw mut item) as isize,
                1,
                (&raw const timeout) as isize,
                0,
            )
        };
        if result == INTERRUPTED {
            continue;
        }
        if result < 0 {
            return result;
        }
        return isize::from(item.revents);
    }
}

// ------------------------------------------------------------ transport ----

/// Exact synchronous I/O over the pre-opened channel descriptor.
struct ChannelTransport {
    ended: bool,
}

impl ChannelTransport {
    const fn new() -> Self {
        Self { ended: false }
    }

    /// `abort_io` and `close_io` are the same operation here and must be
    /// idempotent, so the descriptor is closed at most once.
    fn end(&mut self) {
        if !self.ended {
            self.ended = true;
            raw_close(CHANNEL_FD);
        }
    }
}

impl Transport for ChannelTransport {
    fn read_exact(&mut self, destination: &mut [u8]) -> IoStatus {
        let mut offset = 0;
        while offset < destination.len() {
            let amount = raw_read(CHANNEL_FD, &mut destination[offset..]);
            if amount > 0 {
                offset += amount as usize;
                continue;
            }
            if amount == 0 {
                // End of file is an orderly close only on a frame boundary; a
                // truncated frame is a failure.
                return if offset == 0 {
                    IoStatus::PeerClosed
                } else {
                    IoStatus::Failed
                };
            }
            if amount == INTERRUPTED {
                continue;
            }
            return IoStatus::Failed;
        }
        IoStatus::Ok
    }

    fn write_all(&mut self, source: &[u8]) -> IoStatus {
        let mut offset = 0;
        while offset < source.len() {
            let amount = raw_write(CHANNEL_FD, &source[offset..]);
            if amount > 0 {
                offset += amount as usize;
                continue;
            }
            if amount == INTERRUPTED {
                continue;
            }
            return IoStatus::Failed;
        }
        IoStatus::Ok
    }

    fn probe_input(&mut self) -> InputStatus {
        let revents = raw_poll_now(CHANNEL_FD, POLLIN);
        if revents < 0 {
            return InputStatus::Failed;
        }
        let revents = revents as i16;
        if revents & POLLIN != 0 {
            InputStatus::Available
        } else if revents & POLLHUP != 0 {
            InputStatus::PeerClosed
        } else {
            InputStatus::Unavailable
        }
    }

    fn abort_io(&mut self) {
        self.end();
    }

    fn close_io(&mut self) {
        self.end();
    }
}

// -------------------------------------------------------------- buffers ----

/// One 1 MiB `.bss` scratch buffer, handed out exactly once.
///
/// See `unsafe` inventory row 5: the process is single-threaded by
/// construction, so the `Sync` marker only exists to allow a `static`, and
/// [`PayloadBuffer::take`] is called once per buffer before the service runs.
struct PayloadBuffer(UnsafeCell<[u8; PAYLOAD_BYTES]>);

// SAFETY: inventory #5.
unsafe impl Sync for PayloadBuffer {}

impl PayloadBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; PAYLOAD_BYTES]))
    }

    /// # Safety
    /// The caller must call this at most once per buffer, and only while no
    /// other borrow of it exists.
    unsafe fn take(&self) -> &mut [u8] {
        // SAFETY: inventory #5, plus this function's own contract.
        unsafe { &mut *self.0.get() }
    }
}

static RECEIVE_PAYLOAD: PayloadBuffer = PayloadBuffer::new();
static SEND_PAYLOAD: PayloadBuffer = PayloadBuffer::new();

// -------------------------------------------------------------- service ----

/// Writes the readiness attestation on [`READY_FD`] and closes it.
fn attest_readiness() -> bool {
    let mut offset = 0;
    while offset < READY_ATTESTATION.len() {
        let amount = raw_write(READY_FD, &READY_ATTESTATION[offset..]);
        if amount > 0 {
            offset += amount as usize;
            continue;
        }
        if amount == INTERRUPTED {
            continue;
        }
        return false;
    }
    raw_close(READY_FD);
    true
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
    // SAFETY: inventory #5. This is the only `take` of either buffer, on the
    // only thread, and both borrows live until the process exits.
    let (receive_payload, send_payload) = unsafe { (RECEIVE_PAYLOAD.take(), SEND_PAYLOAD.take()) };
    let outcome = run_parser_worker_service(
        ChannelTransport::new(),
        UnsupportedDispatcher::new(),
        ServiceBuffers {
            receive_payload,
            send_payload,
        },
        ServiceLimits::default(),
    );
    match outcome {
        Ok(_) => WORKER_CLEAN_EXIT,
        Err(failure) => failure_status(&failure),
    }
}

// ------------------------------------------------------------ mem shims ----

// SAFETY: inventory #4 for all four functions.

#[unsafe(no_mangle)]
unsafe extern "C" fn memset(destination: *mut c_void, value: i32, count: usize) -> *mut c_void {
    let bytes = destination.cast::<u8>();
    let mut index = 0;
    while index < count {
        unsafe { *bytes.add(index) = value as u8 };
        index += 1;
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    let (output, input) = (destination.cast::<u8>(), source.cast::<u8>());
    let mut index = 0;
    while index < count {
        unsafe { *output.add(index) = *input.add(index) };
        index += 1;
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memmove(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    let (output, input) = (destination.cast::<u8>(), source.cast::<u8>());
    if (output as usize) < (input as usize) {
        let mut index = 0;
        while index < count {
            unsafe { *output.add(index) = *input.add(index) };
            index += 1;
        }
    } else {
        let mut index = count;
        while index > 0 {
            index -= 1;
            unsafe { *output.add(index) = *input.add(index) };
        }
    }
    destination
}

#[unsafe(no_mangle)]
unsafe extern "C" fn memcmp(first: *const c_void, second: *const c_void, count: usize) -> i32 {
    let (left, right) = (first.cast::<u8>(), second.cast::<u8>());
    let mut index = 0;
    while index < count {
        let (a, b) = unsafe { (*left.add(index), *right.add(index)) };
        if a != b {
            return i32::from(a) - i32::from(b);
        }
        index += 1;
    }
    0
}
