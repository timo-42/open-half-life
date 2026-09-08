//! Shared standard-library Linux musl and macOS test worker image.
//!
//! The confined side of the `ohl-platform` isolated-worker contract: an
//! ordinary Rust binary that the host runs under its native sandbox
//! with descriptors 3 (channel) and 4 (readiness) already in place. It
//! parses no arguments, reads no environment and, outside
//! [`MODE_CONFINEMENT_PROBE`], opens no file.
//!
//! # `unsafe` inventory
//!
//! `unsafe_code` is allowed for this package only. This file has exactly one
//! kind of unsafe site: adopting the two inherited descriptors (and, for the
//! inventory probe, borrowing descriptor numbers) as `std` objects with
//! `from_raw_fd`. Each is sound because the host contract guarantees 3 and 4
//! are open and owned by nobody else in this process, and the inventory
//! probe never closes what it borrows (`ManuallyDrop`).

use std::fs::File;
use std::io::{Read as _, Write as _};
#[cfg(target_os = "macos")]
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd as _;
use std::os::unix::net::UnixStream;

/// The wire contract, shared with the host side. Not every constant is used
/// by every image shape (the Linux-only and macOS-only modes each leave the
/// other's constants unread).
#[allow(dead_code)]
mod protocol {
    include!("../../src/protocol.rs");
}
use protocol::*;

/// Adopts the channel. Called once.
fn channel() -> UnixStream {
    // SAFETY: inventory #1. Descriptor 3 is the channel the host placed
    // there, open, and owned by this function's single caller.
    unsafe { UnixStream::from_raw_fd(CHANNEL_FD) }
}

/// Writes the readiness attestation on [`READY_FD`] and closes it.
fn attest_readiness() -> bool {
    // SAFETY: inventory #1, for descriptor 4; dropping the `File` is what
    // closes it and signals end-of-file to the host.
    let mut ready = unsafe { File::from_raw_fd(READY_FD) };
    ready.write_all(&READY_ATTESTATION).is_ok() && ready.flush().is_ok()
}

/// Reads exactly `buffer.len()` bytes. Returns `None` on end-of-file before
/// the first byte and `Some(false)` on a short or failed read.
fn read_exact(stream: &mut UnixStream, buffer: &mut [u8]) -> Option<bool> {
    let mut offset = 0usize;
    while offset < buffer.len() {
        match stream.read(&mut buffer[offset..]) {
            Ok(0) => return if offset == 0 { None } else { Some(false) },
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Some(false),
        }
    }
    Some(true)
}

fn write_all(stream: &mut UnixStream, bytes: &[u8]) -> bool {
    stream.write_all(bytes).is_ok()
}

fn hang_forever() -> ! {
    loop {
        std::thread::park();
    }
}

/// Bitmask of the descriptors below [`FD_PROBE_CEILING`] that are open,
/// probed with `fstat`: it succeeds on any open descriptor and fails with
/// `EBADF` on a closed number.
#[cfg(target_os = "macos")]
fn fd_inventory() -> u64 {
    let mut mask = 0u64;
    for descriptor in 0..FD_PROBE_CEILING {
        // SAFETY: inventory #1. The `File` only borrows the number for one
        // `fstat`; `ManuallyDrop` guarantees it never closes it, whether or
        // not the number is open.
        let probe = ManuallyDrop::new(unsafe { File::from_raw_fd(descriptor) });
        if probe.metadata().is_ok() {
            mask |= 1u64 << descriptor;
        }
    }
    mask
}

/// Attempts each operation the sandbox must deny; bit `n` set means probe
/// `n` succeeded. See [`CONFINEMENT_PROBE_COUNT`] for the order.
#[cfg(target_os = "macos")]
fn confinement_probe() -> u8 {
    let mut succeeded = 0u8;
    if File::open("/private/etc/hosts").is_ok() {
        succeeded |= 1 << 0;
    }
    let scratch = format!("/private/tmp/ohl-confinement-probe-{}", std::process::id());
    if File::create(&scratch).is_ok() {
        succeeded |= 1 << 1;
        let _ = std::fs::remove_file(&scratch);
    }
    if std::net::TcpListener::bind(("127.0.0.1", 0)).is_ok() {
        succeeded |= 1 << 2;
    }
    if std::process::Command::new("/usr/bin/true")
        .status()
        .is_ok_and(|status| status.success())
    {
        succeeded |= 1 << 3;
    }
    const { assert!(CONFINEMENT_PROBE_COUNT == 4) };
    succeeded
}

#[cfg(target_os = "linux")]
use crate::linux_probes::fd_inventory;

fn standard_runtime_probe() -> bool {
    let start = std::time::Instant::now();
    let lock = std::sync::Mutex::new(Vec::new());
    let mut bytes = lock.lock().unwrap();
    // Large grow/shrink operations exercise malloc/realloc/free and touch
    // every page, keeping actual allocation observable through the checksum.
    bytes.resize(2 * 1024 * 1024, 37u8);
    bytes.resize(4 * 1024 * 1024, 19);
    let sum: u64 = bytes.iter().map(|byte| u64::from(*byte)).sum();
    bytes.truncate(128);
    bytes.shrink_to_fit();
    let mut seeded = std::collections::HashMap::new();
    seeded.insert("sum", sum);
    std::thread::park_timeout(std::time::Duration::from_millis(1));
    start.elapsed() > std::time::Duration::ZERO
        && seeded["sum"] == 2 * 1024 * 1024 * (37 + 19)
        && bytes.iter().all(|byte| *byte == 37)
}

fn reply(stream: &mut UnixStream, payload: &[u8]) -> bool {
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    write_all(stream, &length.to_le_bytes()) && write_all(stream, payload)
}

fn serve(stream: &mut UnixStream, buffer: &mut [u8]) -> i32 {
    let mut mode: Option<u8> = None;
    loop {
        let mut length = [0u8; 4];
        match read_exact(stream, &mut length) {
            None => return 0,
            Some(false) => return WORKER_PROTOCOL_FAILURE_STATUS,
            Some(true) => {}
        }
        let length = u32::from_le_bytes(length) as usize;
        if length == 0 || length > MAX_FRAME_BYTES {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }
        if read_exact(stream, &mut buffer[..length]) != Some(true) {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }

        let selected = *mode.get_or_insert(buffer[0]);
        match selected {
            MODE_HANG => hang_forever(),
            #[cfg(target_os = "macos")]
            MODE_CRASH => std::process::abort(),
            #[cfg(target_os = "linux")]
            MODE_CRASH => crate::linux_probes::crash(),
            #[cfg(target_os = "macos")]
            MODE_FORBIDDEN_SYSCALL => return WORKER_PROTOCOL_FAILURE_STATUS,
            #[cfg(target_os = "linux")]
            MODE_FORBIDDEN_SYSCALL => return crate::linux_probes::denied(0),
            #[cfg(target_os = "linux")]
            MODE_LINUX_DENIAL => {
                return crate::linux_probes::denied(*buffer.get(1).unwrap_or(&255));
            }
            MODE_STD_RUNTIME => {
                if !reply(stream, &[u8::from(standard_runtime_probe())]) {
                    return WORKER_PROTOCOL_FAILURE_STATUS;
                }
                continue;
            }
            MODE_EXIT => {
                return if length >= 2 {
                    i32::from(buffer[1])
                } else {
                    WORKER_PROTOCOL_FAILURE_STATUS
                };
            }
            MODE_FD_INVENTORY => {
                if !reply(stream, &fd_inventory().to_le_bytes()) {
                    return WORKER_PROTOCOL_FAILURE_STATUS;
                }
                continue;
            }
            #[cfg(target_os = "macos")]
            MODE_CONFINEMENT_PROBE => {
                if !reply(stream, &[confinement_probe()]) {
                    return WORKER_PROTOCOL_FAILURE_STATUS;
                }
                continue;
            }
            MODE_ECHO_REVERSED => {}
            _ => return WORKER_PROTOCOL_FAILURE_STATUS,
        }

        buffer[1..length].reverse();
        if !reply(stream, &buffer[1..length]) {
            return WORKER_PROTOCOL_FAILURE_STATUS;
        }
    }
}

pub(crate) fn main() -> ! {
    if cfg!(feature = "never-ready") {
        hang_forever();
    }
    if !attest_readiness() {
        std::process::exit(WORKER_PROTOCOL_FAILURE_STATUS);
    }
    let mut stream = channel();
    if stream.set_nonblocking(true).is_err() || stream.set_nonblocking(false).is_err() {
        std::process::exit(WORKER_PROTOCOL_FAILURE_STATUS);
    }
    let mut buffer = vec![0u8; MAX_FRAME_BYTES];
    let status = serve(&mut stream, &mut buffer);
    std::process::exit(status)
}
