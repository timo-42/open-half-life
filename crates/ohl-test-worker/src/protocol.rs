// The wire contract shared by the host backend, the test-image builder, and
// the worker image itself.
//
// This file is compiled twice: once as a module of the `ohl-test-worker`
// library and once via `include!` from the standalone std image.
// It therefore contains nothing but `const` items.

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
///
/// Linux only. The hosted (macOS) image answers this mode with a protocol
/// failure exit; its confinement is probed with [`MODE_CONFINEMENT_PROBE`]
/// instead, because a Seatbelt denial is an error return, not a kill.
pub const MODE_FORBIDDEN_SYSCALL: u8 = 0x03;

/// Mode byte: exit immediately with the status in the next payload byte.
pub const MODE_EXIT: u8 = 0x04;

/// Mode byte: report which of the first [`FD_PROBE_CEILING`] descriptors are
/// open as a little-endian `u64` bitmask, then keep serving.
///
/// The probe uses `ppoll`, because it is the only allowlisted syscall that
/// distinguishes an open descriptor from a closed one (`POLLNVAL`); `fcntl`
/// would be the obvious tool and is deliberately not in the policy.
pub const MODE_FD_INVENTORY: u8 = 0x05;

/// How many descriptor numbers [`MODE_FD_INVENTORY`] probes.
pub const FD_PROBE_CEILING: i32 = 64;

/// Mode byte: attempt each of the [`CONFINEMENT_PROBE_COUNT`] operations the
/// sandbox must deny, then reply with one byte whose bit `n` is set when
/// probe `n` *succeeded*, and keep serving. A confined worker replies `0`.
///
/// Hosted (macOS) image only; the freestanding Linux image cannot attempt
/// any of these without being killed by seccomp, which
/// [`MODE_FORBIDDEN_SYSCALL`] already proves.
pub const MODE_CONFINEMENT_PROBE: u8 = 0x06;

/// The probes behind [`MODE_CONFINEMENT_PROBE`], in bit order:
///
/// 0. open a world-readable system file for reading (`/private/etc/hosts`);
/// 1. create a file in the system temporary directory (`/private/tmp`);
/// 2. bind a TCP listener on the loopback interface;
/// 3. spawn a child process (`/usr/bin/true`).
pub const CONFINEMENT_PROBE_COUNT: u32 = 4;

/// The descriptors a bootstrapped worker must still have once it has attested
/// readiness and closed [`READY_FD`]: `/dev/null` on 0/1/2 and the channel on
/// 3. On Linux, [`IMAGE_FD`] and the Landlock ruleset descriptor are
/// `O_CLOEXEC`, so `execveat` closes them, and `close_range` removed
/// everything else; on macOS the bootstrap's close sweep did the same.
pub const EXPECTED_FD_MASK: u64 = 0b1111;

/// Exit status the worker uses for any protocol or I/O failure of its own.
pub const WORKER_PROTOCOL_FAILURE_STATUS: i32 = 90;

/// Linux probe selector: each operation must independently die with SIGSYS.
pub const MODE_LINUX_DENIAL: u8 = 0x07;
/// Exercise standard allocation, reallocation, time and synchronization.
pub const MODE_STD_RUNTIME: u8 = 0x08;
/// Denied operations: file open/create, network, clone/fork/exec, executable
/// mmap/mprotect, file-backed mmap, descriptor duplication, TLS operation,
/// socket ioctl and non-private futex.
pub const LINUX_DENIAL_PROBE_COUNT: u8 = 14;
