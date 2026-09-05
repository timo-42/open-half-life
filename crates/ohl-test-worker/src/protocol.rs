// The wire contract shared by the host backend, the test-image builder, and
// the freestanding worker image itself.
//
// This file is compiled twice: once as a module of the `ohl-test-worker`
// library (`std`), and once via `include!` from the freestanding
// `#![no_std]` image, which cannot depend on any other crate. It therefore
// contains nothing but `const` items and one plain data enum.

/// Descriptor the private full-duplex byte channel is bound to in the child.
pub const CHANNEL_FD: i32 = 3;

/// Descriptor the one-shot readiness pipe is bound to in the child.
pub const READY_FD: i32 = 4;

/// Descriptor the verified worker image is bound to in the child. It is
/// `O_CLOEXEC`, so a successfully bootstrapped worker never sees it.
pub const IMAGE_FD: i32 = 5;

/// The exact byte sequence a bootstrapped worker writes to [`READY_FD`]
/// before closing it. A worker is only accepted when the host reads this
/// sequence in full and then observes end-of-file.
pub const READY_ATTESTATION: [u8; 16] = [
    b'O', b'H', b'L', b'I', b'S', b'O', b'L', b'A', b'T', b'E', b'D', 0, 1, 0, 0, 0,
];

/// Largest frame payload either side will send or accept.
pub const MAX_FRAME_BYTES: usize = 32 * 1024;

/// Mode byte: reverse each frame payload and send it back until the channel
/// reaches end-of-file, then exit 0.
pub const MODE_ECHO_REVERSED: u8 = 0x00;

/// Mode byte: block forever without ever reading or writing again.
pub const MODE_HANG: u8 = 0x01;

/// Mode byte: execute an undefined instruction (`ud2`), i.e. die on `SIGILL`.
pub const MODE_CRASH: u8 = 0x02;

/// Mode byte: attempt `openat(2)`, which the seccomp policy does not allow,
/// i.e. die on `SIGSYS` from `SECCOMP_RET_KILL_PROCESS`.
pub const MODE_FORBIDDEN_SYSCALL: u8 = 0x03;

/// Mode byte: exit immediately with the status in the next payload byte.
pub const MODE_EXIT: u8 = 0x04;

/// Exit status the worker uses for any protocol or I/O failure of its own.
pub const WORKER_PROTOCOL_FAILURE_STATUS: i32 = 90;
