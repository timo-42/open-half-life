//! Freestanding Linux x86-64 test worker image.
//!
//! This binary is the confined side of the `ohl-platform` isolated-worker
//! contract. It is built with `-nostdlib -static -no-pie` and issues raw
//! `syscall` instructions only, so that the host's seccomp allowlist
//! (`execveat`, `read`, `write`, `close`, `ppoll`, `restart_syscall`, `exit`,
//! `exit_group`) is
//! sufficient to run it and any additional syscall is a genuine policy
//! violation rather than libc noise.
//!
//! # `unsafe` inventory
//!
//! `unsafe_code` is allowed for this package only, and only for the four
//! kinds of site listed here. There is no allocator, no `static mut`, no
//! transmute, and no unsafe trait implementation.
//!
//! | # | Site | Why it is needed | Why it is sound |
//! |---|------|------------------|-----------------|
//! | 1 | `global_asm!` `_start` | A `-nostdlib` image has no C runtime, so the ELF entry point must be written by hand: clear the frame pointer, align the stack to 16 bytes, and tail-call into Rust. | The block only touches `rbp`/`rsp`/`rdi` before calling a diverging `extern "C"` function, exactly as the System V AMD64 process-entry ABI prescribes. |
//! | 2 | `syscall1` .. `syscall4`, `exit_group` | `core` exposes no syscall interface and no C library is linked. | Each wrapper passes only integers and pointers to live, correctly sized local buffers, declares `clobber_abi`-equivalent clobbers (`rcx`, `r11`, `memory`) and never lets the kernel write outside a slice the caller owns. |
//! | 3 | `rust_eh_personality` stub | The precompiled stable `core` was built with unwind tables, so a `DW.ref.rust_eh_personality` relocation survives into the image even though this package is compiled with `panic = "abort"`. | Nothing can ever unwind: the panic handler diverges into `exit_group`, so the symbol is never called. |
//! | 4 | `memcpy`/`memset`/`memmove`/`memcmp` | The prebuilt stable `compiler_builtins` does not ship its `mem` feature, so `core`'s slice codegen emits calls to these four symbols with nothing to resolve them once libc is gone. | Byte-at-a-time loops with the documented `memcpy`/`memmove` overlap rules; callers are `core` itself, which upholds the pointer/length preconditions. |
#![no_std]
#![no_main]
#![allow(unsafe_code)]

use core::arch::{asm, global_asm};
use core::ffi::c_void;
use core::panic::PanicInfo;

include!("../../src/protocol.rs");

const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_CLOSE: usize = 3;
const SYS_PPOLL: usize = 271;
const SYS_EXIT_GROUP: usize = 231;
const SYS_OPENAT: usize = 257;

const AT_FDCWD: i32 = -100;

/// `POLLNVAL`: the descriptor is not open. Reported regardless of `events`.
const POLLNVAL: i16 = 0x0020;

/// Largest `nfds` the kernel accepts, which is `RLIMIT_NOFILE` - 8 under the
/// host's resource limits - so the descriptor probe runs in chunks of eight.
const POLL_CHUNK: i32 = 8;

/// `struct pollfd`.
#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    descriptor: i32,
    events: i16,
    revents: i16,
}

/// `struct timespec`.
#[repr(C)]
struct Timespec {
    seconds: i64,
    nanoseconds: i64,
}

global_asm!(
    ".global _start",
    ".text",
    "_start:",
    "xor ebp, ebp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call ohl_test_worker_start",
    "ud2",
);

/// # Safety
///
/// Called exactly once by `_start` with the initial stack pointer and a
/// 16-byte aligned stack; never returns.
#[unsafe(no_mangle)]
extern "C" fn ohl_test_worker_start(_initial_stack: *const usize) -> ! {
    exit_group(run());
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit_group(WORKER_PROTOCOL_FAILURE_STATUS)
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
            options(noreturn, nostack)
        )
    }
}

// SAFETY: inventory #2 for all four wrappers.
unsafe fn syscall1(number: usize, a0: isize) -> isize {
    let result: isize;
    unsafe {
        asm!("syscall", inlateout("rax") number as isize => result, in("rdi") a0,
             lateout("rcx") _, lateout("r11") _, options(nostack));
    }
    result
}

unsafe fn syscall3(number: usize, a0: isize, a1: isize, a2: isize) -> isize {
    let result: isize;
    unsafe {
        asm!("syscall", inlateout("rax") number as isize => result, in("rdi") a0,
             in("rsi") a1, in("rdx") a2, lateout("rcx") _, lateout("r11") _,
             options(nostack));
    }
    result
}

unsafe fn syscall4(number: usize, a0: isize, a1: isize, a2: isize, a3: isize) -> isize {
    let result: isize;
    unsafe {
        asm!("syscall", inlateout("rax") number as isize => result, in("rdi") a0,
             in("rsi") a1, in("rdx") a2, in("r10") a3, lateout("rcx") _,
             lateout("r11") _, options(nostack));
    }
    result
}

fn write_all(descriptor: i32, bytes: &[u8]) -> bool {
    let mut offset = 0usize;
    while offset < bytes.len() {
        // SAFETY: inventory #2. The kernel reads `len - offset` bytes from a
        // pointer into a live slice this frame owns.
        let written = unsafe {
            syscall3(
                SYS_WRITE,
                descriptor as isize,
                bytes.as_ptr().add(offset) as isize,
                (bytes.len() - offset) as isize,
            )
        };
        if written <= 0 {
            return false;
        }
        offset += written as usize;
    }
    true
}

/// Reads exactly `buffer.len()` bytes. Returns `None` on end-of-file before
/// the first byte and `Some(false)` on a short or failed read.
fn read_exact(descriptor: i32, buffer: &mut [u8]) -> Option<bool> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        // SAFETY: inventory #2. The kernel writes at most `len - offset`
        // bytes into a pointer inside a live, exclusively borrowed slice.
        let read = unsafe {
            syscall3(
                SYS_READ,
                descriptor as isize,
                buffer.as_mut_ptr().add(offset) as isize,
                (buffer.len() - offset) as isize,
            )
        };
        if read == 0 {
            return if offset == 0 { None } else { Some(false) };
        }
        if read < 0 {
            return Some(false);
        }
        offset += read as usize;
    }
    Some(true)
}

fn hang_forever() -> ! {
    loop {
        // SAFETY: inventory #2. `ppoll(NULL, 0, NULL, NULL)` waits without a
        // descriptor set or a timeout and writes nothing.
        unsafe { syscall4(SYS_PPOLL, 0, 0, 0, 0) };
    }
}

/// Bitmask of the descriptors below [`FD_PROBE_CEILING`] that are still open,
/// probed with `ppoll` and a zero timeout: a closed descriptor comes back
/// with `POLLNVAL`, an open one with zero (nothing was requested).
///
/// Returns `None` if the probe itself fails, which the host reports as a
/// protocol failure rather than as an empty descriptor table.
fn fd_inventory() -> Option<u64> {
    let mut mask = 0u64;
    let mut base = 0i32;
    while base < FD_PROBE_CEILING {
        let mut probes = [PollFd {
            descriptor: 0,
            events: 0,
            revents: 0,
        }; POLL_CHUNK as usize];
        let mut index = 0i32;
        while index < POLL_CHUNK {
            probes[index as usize].descriptor = base + index;
            index += 1;
        }
        let timeout = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // SAFETY: inventory #2. The kernel reads `POLL_CHUNK` `pollfd`s and
        // one `timespec` from live locals of this frame and writes back only
        // into the `revents` fields of that same array.
        let ready = unsafe {
            syscall4(
                SYS_PPOLL,
                probes.as_mut_ptr() as isize,
                POLL_CHUNK as isize,
                core::ptr::from_ref(&timeout) as isize,
                0,
            )
        };
        if ready < 0 {
            return None;
        }
        let mut index = 0i32;
        while index < POLL_CHUNK {
            if probes[index as usize].revents & POLLNVAL == 0 {
                mask |= 1u64 << (base + index);
            }
            index += 1;
        }
        base += POLL_CHUNK;
    }
    Some(mask)
}

fn crash() -> ! {
    // SAFETY: inventory #1/#2 rationale: `ud2` raises SIGILL, which is the
    // whole point of this mode, and never returns.
    unsafe { asm!("ud2", options(noreturn, nostack)) }
}

fn forbidden_syscall() -> i32 {
    // SAFETY: inventory #2. `openat` only reads the NUL-terminated literal.
    // The seccomp policy kills the process before this ever returns.
    unsafe {
        syscall4(
            SYS_OPENAT,
            AT_FDCWD as isize,
            c"/".as_ptr() as isize,
            0,
            0,
        )
    };
    WORKER_PROTOCOL_FAILURE_STATUS
}

fn serve(buffer: &mut [u8; MAX_FRAME_BYTES]) -> i32 {
    let mut mode: Option<u8> = None;
    loop {
        let mut length = [0u8; 4];
        match read_exact(CHANNEL_FD, &mut length) {
            None => return 0,
            Some(false) => return WORKER_PROTOCOL_FAILURE_STATUS,
            Some(true) => {}
        }
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }
        if read_exact(CHANNEL_FD, &mut buffer[..length]) != Some(true) {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }

        let selected = *mode.get_or_insert(buffer[0]);
        match selected {
            MODE_HANG => hang_forever(),
            MODE_CRASH => crash(),
            MODE_FORBIDDEN_SYSCALL => return forbidden_syscall(),
            MODE_EXIT => {
                return if length >= 2 {
                    i32::from(buffer[1])
                } else {
                    WORKER_PROTOCOL_FAILURE_STATUS
                };
            }
            MODE_FD_INVENTORY => {
                let Some(mask) = fd_inventory() else {
                    return WORKER_PROTOCOL_FAILURE_STATUS;
                };
                let reply_length = 8u32;
                if !write_all(CHANNEL_FD, &reply_length.to_le_bytes())
                    || !write_all(CHANNEL_FD, &mask.to_le_bytes())
                {
                    return WORKER_PROTOCOL_FAILURE_STATUS;
                }
                continue;
            }
            MODE_ECHO_REVERSED => {}
            _ => return WORKER_PROTOCOL_FAILURE_STATUS,
        }

        let payload = &mut buffer[1..length];
        payload.reverse();
        let reply_length = (length - 1) as u32;
        if !write_all(CHANNEL_FD, &reply_length.to_le_bytes())
            || !write_all(CHANNEL_FD, &buffer[1..length])
        {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }
    }
}

fn run() -> i32 {
    if cfg!(feature = "never-ready") {
        hang_forever();
    }
    if !write_all(READY_FD, &READY_ATTESTATION) {
        return WORKER_PROTOCOL_FAILURE_STATUS;
    }
    // SAFETY: inventory #2. Closing the readiness descriptor is what signals
    // end-of-file to the host; it takes no pointer arguments.
    unsafe { syscall1(SYS_CLOSE, READY_FD as isize) };

    let mut buffer = [0u8; MAX_FRAME_BYTES];
    serve(&mut buffer)
}

// --- freestanding `mem*` shims (inventory #4) -------------------------------

/// # Safety
/// `dest` must be valid for writes of `count` bytes.
#[unsafe(no_mangle)]
unsafe extern "C" fn memset(dest: *mut c_void, value: i32, count: usize) -> *mut c_void {
    let dest = dest.cast::<u8>();
    let mut index = 0usize;
    while index < count {
        unsafe { *dest.add(index) = value as u8 };
        index += 1;
    }
    dest.cast::<c_void>()
}

/// # Safety
/// `dest` and `src` must be valid for `count` bytes and must not overlap.
#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    let (dest, src) = (dest.cast::<u8>(), src.cast::<u8>());
    let mut index = 0usize;
    while index < count {
        unsafe { *dest.add(index) = *src.add(index) };
        index += 1;
    }
    dest.cast::<c_void>()
}

/// # Safety
/// `dest` and `src` must be valid for `count` bytes; overlap is allowed.
#[unsafe(no_mangle)]
unsafe extern "C" fn memmove(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void {
    let (dest, src) = (dest.cast::<u8>(), src.cast::<u8>());
    if (dest as usize) < (src as usize) {
        let mut index = 0usize;
        while index < count {
            unsafe { *dest.add(index) = *src.add(index) };
            index += 1;
        }
    } else {
        let mut index = count;
        while index > 0 {
            index -= 1;
            unsafe { *dest.add(index) = *src.add(index) };
        }
    }
    dest.cast::<c_void>()
}

/// # Safety
/// `left` and `right` must be valid for reads of `count` bytes.
#[unsafe(no_mangle)]
unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> i32 {
    let (left, right) = (left.cast::<u8>(), right.cast::<u8>());
    let mut index = 0usize;
    while index < count {
        let (a, b) = unsafe { (*left.add(index), *right.add(index)) };
        if a != b {
            return i32::from(a) - i32::from(b);
        }
        index += 1;
    }
    0
}
