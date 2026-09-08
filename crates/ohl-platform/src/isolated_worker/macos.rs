//! Native macOS isolated-worker backend.
//!
//! macOS has no `execveat`, no `pidfd`, no Landlock and no seccomp, and a
//! process can only ever be linked against the system's own libSystem, so
//! the Linux design (a freestanding static image executed by descriptor under
//! a syscall allowlist) cannot be transplanted. This backend reaches the same
//! contract - a confined child, a private channel, fail-closed bootstrap -
//! with what macOS does provide:
//!
//! 1. the image is resolved and verified in the parent exactly as on Linux
//!    (`unix_image`: a compile-fixed install location walked one
//!    `O_NOFOLLOW` component at a time, root- or self-owned and
//!    non-writable directories, a read-only, non-set-id, executable regular
//!    file), and its format is checked as a thin `MH_EXECUTE` Mach-O for the
//!    running CPU type that loads no dynamic library other than
//!    `/usr/lib/libSystem.B.dylib` and no dynamic linker other than
//!    `/usr/lib/dyld`;
//! 2. the child is confined by the system sandbox (Seatbelt), applied by the
//!    root-owned `/usr/bin/sandbox-exec` from a profile rendered by this
//!    backend (`macos_profile.sb`): `(deny default)`, execute-and-read on the
//!    verified image only, read-only access to the system library cache,
//!    and no network, no writes, no `fork`, no Mach lookups;
//! 3. before that, still in the forked child, the private channel and the
//!    readiness pipe are moved onto descriptors 3 and 4, every other
//!    inherited descriptor above 4 is closed, and the six resource limits
//!    in [`RESOURCE_LIMITS`] are applied.
//!
//! `sandbox-exec` compiles the profile, applies it to itself with
//! `sandbox_init`, and then `exec`s the image in place (no `fork`), so the
//! process the parent spawned *is* the worker, and its exit status is the
//! worker's. Termination is a plain `kill(SIGKILL)` on that pid, which
//! cannot have been reused because the child is never reaped until
//! [`Backend::wait`] does so; exit is observed through a `kqueue`
//! `EVFILT_PROC`/`NOTE_EXIT` registration, the macOS equivalent of polling a
//! pidfd.
//!
//! # What is weaker than Linux, and why it is still fail-closed
//!
//! - The image is executed by its (canonical) path, not by descriptor. The
//!   window between verification and `exec` is guarded by the same directory
//!   trust policy that guards the executable itself: only root or the user
//!   can replace anything on that path.
//! - `RLIMIT_AS` and `RLIMIT_DATA` cannot be set at all: Darwin fails both
//!   with `EINVAL`. There is no kernel-enforced memory ceiling on this
//!   platform, so the hosted image bounds its own heap with a counting
//!   allocator instead (see `crates/ohl-parser-worker/image/src/hosted.rs`),
//!   which binds its allocator rather than its address space.
//! - There is no parent-death signal. A worker whose parent vanished sees
//!   end-of-file on the channel and exits on its own.
//! - `sandbox-exec` is documented by Apple as deprecated but is present and
//!   functional on every supported macOS. If it is ever missing, not
//!   root-owned, or refuses the profile, the launch fails with
//!   [`IsolatedWorkerError::ConfinementUnavailable`] or
//!   [`IsolatedWorkerError::BootstrapFailed`] - never with a weaker sandbox.
//!
//! # Descriptor inventory
//!
//! Every descriptor the parent creates for a launch is close-on-exec: the
//! socketpair and the pipe get `FD_CLOEXEC` immediately after creation
//! (macOS has neither `SOCK_CLOEXEC` nor `pipe2`), the collision-safe
//! duplicates are made with `F_DUPFD_CLOEXEC`, and `kqueue` descriptors are
//! always close-on-exec, so an `exec` anywhere else in the parent cannot
//! leak one. The child's `dup2` onto 3 and 4 clears the flag for
//! exactly those two, then the close loop removes everything from 5 up to
//! the descriptor ceiling captured in the parent, and `RLIMIT_NOFILE` caps
//! what the worker can ever add. `fd_inventory_after_exec_is_exactly_the_contract`
//! asserts from inside the confined child that it starts with exactly
//! `{0, 1, 2, 3, 4}` and, once it has attested readiness and closed 4, with
//! exactly `{0, 1, 2, 3}`.
//!
//! # Post-fork discipline
//!
//! Everything the child does between `fork` and `exec` is async-signal-safe:
//! `dup2`, `close`, `setrlimit` and `write`, each a thin libc syscall wrapper
//! through `rustix`, on integers and locals captured before the fork. No
//! allocation, no locks, no `std` I/O objects.

use super::unix_image;
use super::{
    IsolatedWorkerCancellationToken, IsolatedWorkerError, IsolatedWorkerExitKind,
    IsolatedWorkerService,
};

use std::fs::File;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::fs::FileExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustix::event::kqueue::{Event, EventFilter, EventFlags, ProcessEvents, kevent, kqueue};
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::io::FdFlags;
use rustix::net::{AddressFamily, Shutdown, SocketFlags, SocketType};
use rustix::process::{Pid, Resource, Rlimit, Signal};

/// Descriptor the private channel is bound to in the child.
const CHANNEL_FD: RawFd = 3;
/// Descriptor the one-shot readiness pipe is bound to in the child.
const READY_FD: RawFd = 4;
/// First descriptor number the child's close loop sweeps away.
const FIRST_UNUSED_FD: RawFd = 5;
/// Lowest descriptor number the parent's collision-safe duplicates may take.
const FIRST_TEMPORARY_FD: RawFd = 10;
/// Highest descriptor number the child's close loop will ever visit, should
/// the parent's own `RLIMIT_NOFILE` be larger (or unlimited). Everything the
/// parent holds is close-on-exec anyway; the loop exists so the worker's
/// table is *exactly* the contract, not merely leak-free.
const CLOSE_SWEEP_CEILING: u64 = 65_536;
/// Fallback sweep ceiling when the parent's `RLIMIT_NOFILE` is unlimited.
const DEFAULT_CLOSE_SWEEP: u64 = 4096;

/// Exactly what a bootstrapped worker must write to [`READY_FD`] before
/// closing it. Anything else - including a prefix, a superset, or a held-open
/// descriptor - fails the launch.
pub(super) const READY_ATTESTATION: [u8; 16] = [
    b'O', b'H', b'L', b'I', b'S', b'O', b'L', b'A', b'T', b'E', b'D', 0, 1, 0, 0, 0,
];

/// The system sandbox tool that applies the profile and executes the image.
const SANDBOX_TOOL: &str = "/usr/bin/sandbox-exec";

/// The Seatbelt profile, with one `@IMAGE@` placeholder for the verified
/// image path.
const PROFILE_TEMPLATE: &str = include_str!("macos_profile.sb");
const PROFILE_PLACEHOLDER: &str = "@IMAGE@";

/// The six `setrlimit` limits, `(resource, soft, hard)`.
///
/// `RLIMIT_AS` and `RLIMIT_DATA` are deliberately absent, and their absence
/// is measured rather than assumed: Darwin rejects setting either at all
/// (`EINVAL` — `ulimit -d` and `ulimit -v` both report "cannot modify
/// limit"), so including them did not merely fail to bound memory, it failed
/// the whole bootstrap and took the launch down with it. The Linux backend's
/// table has both, which is exactly how they got here.
///
/// Memory is therefore bounded inside the image instead of around it, by the
/// counting global allocator in
/// `crates/ohl-parser-worker/image/src/hosted.rs`. That is a weaker
/// guarantee — it binds the image's own allocator rather than the kernel's
/// view of the address space — and it is the only one this platform offers.
///
/// `RLIMIT_CPU` is the one remaining limit with a soft/hard split, so the
/// kernel raises `SIGXCPU` (reported as
/// [`IsolatedWorkerExitKind::ResourceLimit`]) 30 seconds before the
/// unappealable `SIGKILL`. `RLIMIT_NOFILE` is 16 rather
/// than Linux's 8 because `sandbox-exec` and `dyld` open a handful of files
/// transiently while bringing the image up. `RLIMIT_NPROC` of 1 makes a
/// `fork` fail for a user who already has a process (which is every user);
/// the sandbox denies `process-fork` as well.
const RESOURCE_LIMITS: [(Resource, u64, u64); 6] = [
    (Resource::Stack, 8 * 1024 * 1024, 8 * 1024 * 1024),
    (Resource::Cpu, 300, 330),
    (Resource::Fsize, 0, 0),
    (Resource::Core, 0, 0),
    (Resource::Nofile, 16, 16),
    (Resource::Nproc, 1, 1),
];

/// Bootstrap steps, reported as a single byte on [`READY_FD`] when they fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BootstrapFailure {
    DescriptorSetup = 1,
    ResourceLimits = 2,
}

impl BootstrapFailure {
    const fn error(self) -> IsolatedWorkerError {
        match self {
            Self::ResourceLimits => IsolatedWorkerError::ConfinementUnavailable,
            Self::DescriptorSetup => IsolatedWorkerError::BootstrapFailed,
        }
    }

    const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::DescriptorSetup,
            2 => Self::ResourceLimits,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// The forked child, before `exec`
// ---------------------------------------------------------------------------

/// Everything the child needs, captured before `fork` so that the post-fork
/// path only reads plain data.
struct ChildBootstrap {
    channel: RawFd,
    ready: RawFd,
    /// One past the highest descriptor number the close loop visits.
    sweep_end: RawFd,
}

impl ChildBootstrap {
    /// Reports `failure` on `descriptor` and hands `std` an error so it
    /// leaves the child immediately (`_exit`, no destructors).
    ///
    /// The caller names the descriptor because which one carries the
    /// readiness pipe depends on how far the bootstrap got: before the moves
    /// it is the parent's duplicate (at or above [`FIRST_TEMPORARY_FD`]),
    /// after them it is [`READY_FD`]. Writing to a fixed number would, on a
    /// failure of the very first move, put the byte into whatever the
    /// spawning machinery happened to leave at 4.
    fn fail(descriptor: RawFd, failure: BootstrapFailure) -> std::io::Error {
        // SAFETY: `unsafe` inventory entry 7. `descriptor` is one the caller
        // has just established is open in this child. A short or failed
        // write cannot be handled any better than by leaving anyway, in
        // which case the parent reports the bare spawn failure.
        let ready = unsafe { BorrowedFd::borrow_raw(descriptor) };
        let _ = rustix::io::write(ready, &[failure as u8]);
        std::io::Error::from_raw_os_error(rustix::io::Errno::PERM.raw_os_error())
    }

    /// Moves `source` onto `target`, clearing close-on-exec on the result.
    fn place(source: RawFd, target: RawFd) -> bool {
        // SAFETY: `unsafe` inventory entry 7. `source` is a descriptor the
        // parent duplicated at or above `FIRST_TEMPORARY_FD` specifically for
        // this child and never closed; `target` is treated as a slot
        // (`dup2` closes whatever is there), which is why the `OwnedFd` for
        // it is never dropped - `into_raw_fd` releases it without a `close`.
        let source = unsafe { BorrowedFd::borrow_raw(source) };
        let mut slot = unsafe { OwnedFd::from_raw_fd(target) };
        let placed = rustix::io::dup2(source, &mut slot).is_ok();
        let _ = std::os::fd::IntoRawFd::into_raw_fd(slot);
        placed
    }

    /// Runs in the forked child. Returning `Ok` lets `std` `exec`
    /// `sandbox-exec`; returning `Err` makes it leave.
    fn run(&self) -> std::io::Result<()> {
        // Until both moves are done the only descriptor known to carry the
        // readiness pipe is the parent's own duplicate, so that is where a
        // failure of either move is reported.
        if !Self::place(self.channel, CHANNEL_FD) || !Self::place(self.ready, READY_FD) {
            return Err(Self::fail(self.ready, BootstrapFailure::DescriptorSetup));
        }

        for (resource, soft, hard) in RESOURCE_LIMITS {
            let limit = Rlimit {
                current: Some(soft),
                maximum: Some(hard),
            };
            if rustix::process::setrlimit(resource, limit).is_err() {
                return Err(Self::fail(READY_FD, BootstrapFailure::ResourceLimits));
            }
        }

        for descriptor in FIRST_UNUSED_FD..self.sweep_end {
            // SAFETY: `unsafe` inventory entry 7. Nothing in this child owns
            // any of these descriptors any more - the parent's `OwnedFd`s
            // live in the parent - and closing a number that is not open is
            // a harmless `EBADF`, which `rustix::io::close` ignores.
            unsafe { rustix::io::close(descriptor) };
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Image verification (parent, before fork)
// ---------------------------------------------------------------------------

const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_EXECUTE: u32 = 2;
const CPU_ARCH_ABI64: u32 = 0x0100_0000;
const CPU_TYPE_X86_64: u32 = CPU_ARCH_ABI64 | 0x07;
const CPU_TYPE_ARM64: u32 = CPU_ARCH_ABI64 | 0x0c;
const MACH_HEADER_64_BYTES: usize = 32;
const LOAD_COMMAND_HEADER_BYTES: usize = 8;
const MAXIMUM_LOAD_COMMANDS: u32 = 1024;
const MAXIMUM_LOAD_COMMAND_BYTES: u32 = 1 << 20;

const LC_LOAD_DYLIB: u32 = 0xc;
const LC_ID_DYLIB: u32 = 0xd;
const LC_LOAD_DYLINKER: u32 = 0xe;
const LC_ID_DYLINKER: u32 = 0xf;
const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
const LC_RPATH: u32 = 0x8000_001c;
const LC_REEXPORT_DYLIB: u32 = 0x8000_001f;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;

/// The only dynamic library a hosted Rust image may load: every macOS
/// process links it, and nothing else is needed for `std`.
const PERMITTED_DYLIB: &[u8] = b"/usr/lib/libSystem.B.dylib";
/// The only dynamic linker.
const PERMITTED_DYLINKER: &[u8] = b"/usr/lib/dyld";

/// The Mach-O CPU type of the running process, or `None` on an architecture
/// this backend has never been built for.
const fn host_cpu_type() -> Option<u32> {
    if cfg!(target_arch = "aarch64") {
        Some(CPU_TYPE_ARM64)
    } else if cfg!(target_arch = "x86_64") {
        Some(CPU_TYPE_X86_64)
    } else {
        None
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The NUL-terminated string at `offset` inside one load command.
fn command_string(command: &[u8], offset: usize) -> Option<&[u8]> {
    let rest = command.get(offset..)?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    Some(&rest[..end])
}

/// Checks a Mach-O header plus its load commands against the policy: a thin
/// 64-bit `MH_EXECUTE` for `cpu_type`, whose only dynamic library is
/// libSystem and whose only dynamic linker is dyld, with no `LC_RPATH` and
/// no dylib identity.
fn verify_macho_bytes(bytes: &[u8], cpu_type: u32) -> bool {
    if read_u32(bytes, 0) != Some(MH_MAGIC_64)
        || read_u32(bytes, 4) != Some(cpu_type)
        || read_u32(bytes, 12) != Some(MH_EXECUTE)
    {
        return false;
    }
    let Some(count) = read_u32(bytes, 16) else {
        return false;
    };
    let Some(size) = read_u32(bytes, 20) else {
        return false;
    };
    if count == 0 || count > MAXIMUM_LOAD_COMMANDS || size > MAXIMUM_LOAD_COMMAND_BYTES {
        return false;
    }
    let Some(commands) = bytes.get(MACH_HEADER_64_BYTES..MACH_HEADER_64_BYTES + size as usize)
    else {
        return false;
    };

    let mut offset = 0usize;
    let mut saw_dylinker = false;
    for _ in 0..count {
        let Some(kind) = read_u32(commands, offset) else {
            return false;
        };
        let Some(length) =
            read_u32(commands, offset + 4).and_then(|length| usize::try_from(length).ok())
        else {
            return false;
        };
        if length < LOAD_COMMAND_HEADER_BYTES || length % 8 != 0 {
            return false;
        }
        let Some(end) = offset.checked_add(length) else {
            return false;
        };
        let Some(command) = commands.get(offset..end) else {
            return false;
        };
        match kind {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB
            | LC_LAZY_LOAD_DYLIB => {
                let Some(name_offset) = read_u32(command, 8) else {
                    return false;
                };
                if command_string(command, name_offset as usize) != Some(PERMITTED_DYLIB) {
                    return false;
                }
            }
            LC_LOAD_DYLINKER => {
                let Some(name_offset) = read_u32(command, 8) else {
                    return false;
                };
                if command_string(command, name_offset as usize) != Some(PERMITTED_DYLINKER) {
                    return false;
                }
                saw_dylinker = true;
            }
            LC_ID_DYLIB | LC_ID_DYLINKER | LC_RPATH => return false,
            _ => {}
        }
        offset += length;
    }
    saw_dylinker && offset == commands.len()
}

/// The macOS [`unix_image::FormatCheck`]: reads the header and load commands
/// through the already-open descriptor and applies [`verify_macho_bytes`]
/// for the running CPU type.
fn verify_macho(file: &File, size: u64) -> bool {
    let Some(cpu_type) = host_cpu_type() else {
        return false;
    };
    let mut header = [0u8; MACH_HEADER_64_BYTES];
    if size < MACH_HEADER_64_BYTES as u64 || file.read_exact_at(&mut header, 0).is_err() {
        return false;
    }
    let Some(command_bytes) = read_u32(&header, 20) else {
        return false;
    };
    if command_bytes > MAXIMUM_LOAD_COMMAND_BYTES
        || u64::from(command_bytes) > size - MACH_HEADER_64_BYTES as u64
    {
        return false;
    }
    let mut bytes = vec![0u8; MACH_HEADER_64_BYTES + command_bytes as usize];
    bytes[..MACH_HEADER_64_BYTES].copy_from_slice(&header);
    if file
        .read_exact_at(
            &mut bytes[MACH_HEADER_64_BYTES..],
            MACH_HEADER_64_BYTES as u64,
        )
        .is_err()
    {
        return false;
    }
    verify_macho_bytes(&bytes, cpu_type)
}

// ---------------------------------------------------------------------------
// Confinement policy built in the parent
// ---------------------------------------------------------------------------

/// The sandbox tool must be a root-owned regular file that nobody but root
/// can write, with an execute bit. Anything else means the system is not one
/// this backend understands, and the launch fails closed.
fn verify_sandbox_tool() -> Result<(), IsolatedWorkerError> {
    let descriptor = rustix::fs::open(
        SANDBOX_TOOL,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    let status =
        rustix::fs::fstat(&descriptor).map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    let mode = u32::from(status.st_mode);
    let trusted = mode & 0o170_000 == 0o100_000
        && status.st_uid == 0
        && mode & 0o022 == 0
        && mode & 0o111 != 0;
    if trusted {
        Ok(())
    } else {
        Err(IsolatedWorkerError::ConfinementUnavailable)
    }
}

/// Renders the profile for `image`.
///
/// The path is embedded as a Seatbelt string literal, so it must be absolute
/// UTF-8 with no character that could end or escape the literal; a path that
/// cannot be spelled safely fails the launch rather than being escaped, which
/// no public grammar for the profile language promises to honour.
fn render_profile(image: &Path) -> Result<String, IsolatedWorkerError> {
    let text = image
        .to_str()
        .ok_or(IsolatedWorkerError::ConfinementUnavailable)?;
    if !image.is_absolute()
        || text
            .chars()
            .any(|character| character == '"' || character == '\\' || character.is_control())
    {
        return Err(IsolatedWorkerError::ConfinementUnavailable);
    }
    Ok(PROFILE_TEMPLATE.replace(PROFILE_PLACEHOLDER, text))
}

// ---------------------------------------------------------------------------
// Deadline-aware polling
// ---------------------------------------------------------------------------

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn to_timespec(duration: Duration) -> Timespec {
    Timespec {
        tv_sec: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(duration.subsec_nanos()),
    }
}

/// Polls `descriptor` for `events` until `deadline`.
///
/// `Ok(Some(revents))` means the descriptor is ready, `Ok(None)` that the
/// deadline expired, `Err(())` that polling itself failed.
fn poll_until(
    descriptor: BorrowedFd<'_>,
    events: PollFlags,
    deadline: Instant,
) -> Result<Option<PollFlags>, ()> {
    loop {
        let timeout = to_timespec(remaining(deadline));
        let mut items = [PollFd::new(&descriptor, events)];
        match rustix::event::poll(&mut items, Some(&timeout)) {
            Ok(0) => return Ok(None),
            Ok(_) => {
                let revents = items[0].revents();
                if revents.contains(PollFlags::NVAL) {
                    return Err(());
                }
                return Ok(Some(revents));
            }
            Err(rustix::io::Errno::INTR) => {
                if remaining(deadline).is_zero() {
                    return Ok(None);
                }
            }
            Err(_) => return Err(()),
        }
    }
}

/// The cancellation-aware variant: wakes at least every 10 ms so a
/// cancellation request observes a bounded latency even though the token is
/// not a pollable object.
fn poll_io_until(
    descriptor: BorrowedFd<'_>,
    events: PollFlags,
    deadline: Instant,
    cancellation: &IsolatedWorkerCancellationToken,
) -> Result<Option<PollFlags>, ()> {
    const SLICE: Duration = Duration::from_millis(10);
    while !cancellation.cancellation_requested() {
        let now = Instant::now();
        if deadline <= now {
            return Ok(None);
        }
        let slice = deadline.min(now.checked_add(SLICE).unwrap_or(deadline));
        if let Some(revents) = poll_until(descriptor, events, slice)? {
            return Ok(Some(revents));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Process-exit observation
// ---------------------------------------------------------------------------

/// A `kqueue` with one `EVFILT_PROC`/`NOTE_EXIT` registration for the child.
#[derive(Debug)]
struct ExitWatch {
    queue: OwnedFd,
}

impl ExitWatch {
    /// Registers interest in `pid`'s exit. Fails if the process is already
    /// gone (`ESRCH`), which for a just-spawned child means it died before
    /// it could be watched.
    fn new(pid: Pid) -> Result<Self, ()> {
        let queue = kqueue().map_err(|_| ())?;
        let change = Event::new(
            EventFilter::Proc {
                pid,
                flags: ProcessEvents::EXIT,
            },
            EventFlags::ADD,
            std::ptr::null_mut(),
        );
        let mut none: [Event; 0] = [];
        // SAFETY: `unsafe` inventory entry 8. The registration names a pid,
        // not a descriptor, so the "descriptors must outlive the queue"
        // condition is vacuous; the change and event lists are live locals.
        unsafe { kevent(&queue, &[change], &mut none, Some(Duration::ZERO)) }.map_err(|_| ())?;
        Ok(Self { queue })
    }

    /// Waits until the child has exited or `deadline` passes.
    ///
    /// `Ok(true)` means the exit was observed, `Ok(false)` that the deadline
    /// expired, `Err(())` that the queue itself failed.
    fn wait_exit(&self, deadline: Instant) -> Result<bool, ()> {
        loop {
            let mut events = [const { MaybeUninit::<Event>::uninit() }; 1];
            // SAFETY: `unsafe` inventory entry 8, as above.
            match unsafe { kevent(&self.queue, &[], &mut events, Some(remaining(deadline))) } {
                Ok(([], _)) => return Ok(false),
                Ok((ready, _)) => {
                    return if ready[0].flags().contains(EventFlags::ERROR) {
                        Err(())
                    } else {
                        Ok(true)
                    };
                }
                Err(rustix::io::Errno::INTR) => {
                    if remaining(deadline).is_zero() {
                        return Ok(false);
                    }
                }
                Err(_) => return Err(()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Monotonic termination bookkeeping: whether `SIGKILL` was asked for, whether
/// it was actually delivered, and whether delivery failed for a reason other
/// than "the child is already gone".
#[derive(Debug, Default)]
struct TerminationState {
    requested: bool,
    signal_sent: bool,
    signal_failed: bool,
}

/// One confined child, its exit watch, and its half of the channel.
#[derive(Debug)]
pub(super) struct Backend {
    child: Child,
    pid: Pid,
    watch: ExitWatch,
    /// Set once the exit watch has fired, so a later `wait` reaps without
    /// waiting for an event that will never come again.
    exit_observed: bool,
    channel: OwnedFd,
    aborted: bool,
    channel_shutdown: bool,
    termination: TerminationState,
    reaped: Option<IsolatedWorkerExitKind>,
    /// Terminating signal number of the reaped child, kept for tests that
    /// need to distinguish an abort (`SIGABRT`) from another fault.
    terminating_signal: Option<i32>,
}

impl Backend {
    pub(super) fn launch(
        service: IsolatedWorkerService,
        startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        // The sandbox profile names the image by path, and Seatbelt matches
        // literals against real paths, so the install location is resolved
        // from the canonical executable directory (`current_exe` is already
        // canonical on macOS; this makes it explicit).
        let executable =
            std::env::current_exe().map_err(|_| IsolatedWorkerError::ServiceUnavailable)?;
        let base = executable
            .parent()
            .and_then(|base| std::fs::canonicalize(base).ok())
            .ok_or(IsolatedWorkerError::ServiceUnavailable)?;
        let image = unix_image::resolve_service_image_under(&base, service, verify_macho)?;
        Self::launch_image(&image.file, &image.path, startup_deadline)
    }

    /// Test-only entry point that skips install-location resolution but
    /// applies exactly the same verification and confinement.
    #[cfg(test)]
    pub(super) fn launch_verified_path(
        path: &Path,
        startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        // Only the directory is canonicalised (a temporary directory on
        // macOS is reached through a symbolic link, which a Seatbelt literal
        // would not match); the final component is opened `O_NOFOLLOW` as in
        // production, so a symlinked image is still refused.
        let canonical = match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => std::fs::canonicalize(parent)
                .map_or_else(|_| path.to_path_buf(), |parent| parent.join(name)),
            _ => path.to_path_buf(),
        };
        let image = unix_image::open_verified_image(&canonical, verify_macho)?;
        Self::launch_image(&image, &canonical, startup_deadline)
    }

    /// `_image` is the verified descriptor. macOS has no `fexecve`, so it
    /// cannot be what gets executed; it has done its job (the format check
    /// ran on it) and the path-versus-descriptor gap is documented in the
    /// module docs. The caller keeps it open across the launch.
    fn launch_image(
        _image: &File,
        path: &Path,
        startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        verify_sandbox_tool()?;
        let profile = render_profile(path)?;

        // macOS has no `SOCK_CLOEXEC` and no `pipe2`, so close-on-exec is
        // set immediately after creation. The only thing that could `exec`
        // in between is another thread of this process spawning a child at
        // that exact instant; such a child would inherit a socket it cannot
        // name, and the confined worker is never that child.
        let (parent_channel, child_channel) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::empty(),
            None,
        )
        .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        let (ready_read, ready_write) =
            rustix::pipe::pipe().map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        for descriptor in [
            parent_channel.as_fd(),
            child_channel.as_fd(),
            ready_read.as_fd(),
            ready_write.as_fd(),
        ] {
            rustix::io::fcntl_setfd(descriptor, FdFlags::CLOEXEC)
                .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        }
        // Only the host side is non-blocking, and only the host side needs
        // `SO_NOSIGPIPE` (the hosted worker ignores `SIGPIPE` itself, as
        // every Rust `std` binary does).
        rustix::io::ioctl_fionbio(&parent_channel, true)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        rustix::net::sockopt::set_socket_nosigpipe(&parent_channel, true)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        rustix::io::ioctl_fionbio(&ready_read, true)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;

        // Collision-safe duplicates: the child's bootstrap moves descriptors
        // onto 3 and 4, so the originals must not already live there.
        let temporary = |descriptor: BorrowedFd<'_>| {
            rustix::io::fcntl_dupfd_cloexec(descriptor, FIRST_TEMPORARY_FD)
                .map_err(|_| IsolatedWorkerError::ResourceExhausted)
        };
        let channel_copy = temporary(child_channel.as_fd())?;
        let ready_copy = temporary(ready_write.as_fd())?;

        let sweep = rustix::process::getrlimit(Resource::Nofile)
            .current
            .map_or(DEFAULT_CLOSE_SWEEP, |limit| limit.min(CLOSE_SWEEP_CEILING));
        let bootstrap = ChildBootstrap {
            channel: channel_copy.as_raw_fd(),
            ready: ready_copy.as_raw_fd(),
            sweep_end: RawFd::try_from(sweep).unwrap_or(RawFd::MAX),
        };

        let mut command = Command::new(SANDBOX_TOOL);
        command
            .arg("-p")
            .arg(&profile)
            .arg(path)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: `unsafe` inventory entry 6. `ChildBootstrap::run` performs
        // only `dup2`, `setrlimit`, `close` and one `write`, each a direct
        // syscall wrapper on integers captured before the fork - no
        // allocation, no locking, no library state - so it is
        // async-signal-safe as `pre_exec` requires.
        unsafe {
            command.pre_exec(move || bootstrap.run());
        }

        let Ok(child) = command.spawn() else {
            drop((child_channel, ready_write, channel_copy, ready_copy));
            return Err(spawn_failure(ready_read.as_fd()));
        };
        drop((child_channel, ready_write, channel_copy, ready_copy));

        let pid = Pid::from_child(&child);
        let Ok(watch) = ExitWatch::new(pid) else {
            let mut child = child;
            kill_and_reap(&mut child, None);
            return Err(IsolatedWorkerError::BootstrapFailed);
        };

        let mut backend = Self {
            child,
            pid,
            watch,
            exit_observed: false,
            channel: parent_channel,
            aborted: false,
            channel_shutdown: false,
            termination: TerminationState::default(),
            reaped: None,
            terminating_signal: None,
        };

        match await_ready(ready_read.as_fd(), startup_deadline) {
            Ok(()) => Ok(backend),
            Err(error) => {
                backend.request_termination();
                let deadline = Instant::now()
                    .checked_add(Duration::from_secs(5))
                    .unwrap_or_else(Instant::now);
                let _ = backend.wait(deadline);
                Err(error)
            }
        }
    }

    pub(super) fn read_exact(
        &mut self,
        destination: &mut [u8],
        deadline: Instant,
        cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        let mut transferred = 0usize;
        while transferred < destination.len() {
            match poll_io_until(self.channel.as_fd(), PollFlags::IN, deadline, cancellation) {
                Err(()) => return Err(IsolatedWorkerError::IoFailure),
                Ok(None) => return Err(self.stalled_error(cancellation)),
                Ok(Some(_)) => {}
            }
            match rustix::io::read(&self.channel, &mut destination[transferred..]) {
                Ok(0) => {
                    return Err(if self.aborted {
                        IsolatedWorkerError::Cancelled
                    } else {
                        IsolatedWorkerError::PeerClosed
                    });
                }
                Ok(amount) => transferred += amount,
                Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
                Err(_) => {
                    return Err(if self.aborted {
                        IsolatedWorkerError::Cancelled
                    } else {
                        IsolatedWorkerError::IoFailure
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn write_all(
        &mut self,
        source: &[u8],
        deadline: Instant,
        cancellation: &IsolatedWorkerCancellationToken,
    ) -> Result<(), IsolatedWorkerError> {
        let mut transferred = 0usize;
        while transferred < source.len() {
            match poll_io_until(self.channel.as_fd(), PollFlags::OUT, deadline, cancellation) {
                Err(()) => return Err(IsolatedWorkerError::IoFailure),
                Ok(None) => return Err(self.stalled_error(cancellation)),
                Ok(Some(_)) => {}
            }
            match rustix::io::write(&self.channel, &source[transferred..]) {
                Ok(0) => return Err(IsolatedWorkerError::PeerClosed),
                Ok(amount) => transferred += amount,
                Err(rustix::io::Errno::INTR | rustix::io::Errno::AGAIN) => {}
                Err(_) => {
                    return Err(if self.aborted {
                        IsolatedWorkerError::Cancelled
                    } else {
                        IsolatedWorkerError::PeerClosed
                    });
                }
            }
        }
        Ok(())
    }

    fn stalled_error(&self, cancellation: &IsolatedWorkerCancellationToken) -> IsolatedWorkerError {
        if cancellation.cancellation_requested() || self.aborted {
            IsolatedWorkerError::Cancelled
        } else {
            IsolatedWorkerError::Timeout
        }
    }

    pub(super) fn abort_io(&mut self) {
        self.aborted = true;
        self.close_channel();
    }

    pub(super) fn close_channel(&mut self) {
        if !self.channel_shutdown {
            self.channel_shutdown = true;
            let _ = rustix::net::shutdown(&self.channel, Shutdown::Both);
        }
    }

    /// Idempotent, never blocks waiting for exit.
    fn request_termination(&mut self) {
        self.aborted = true;
        self.close_channel();
        if self.termination.requested {
            return;
        }
        self.termination.requested = true;
        match rustix::process::kill_process(self.pid, Signal::KILL) {
            Ok(()) => self.termination.signal_sent = true,
            Err(rustix::io::Errno::SRCH) => {}
            Err(_) => self.termination.signal_failed = true,
        }
    }

    pub(super) fn wait(
        &mut self,
        deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        if let Some(exit) = self.reaped {
            return Ok(exit);
        }
        if !self.exit_observed {
            match self.watch.wait_exit(deadline) {
                Ok(true) => self.exit_observed = true,
                Ok(false) => return Err(IsolatedWorkerError::Timeout),
                Err(()) => {
                    self.request_termination();
                    return Err(IsolatedWorkerError::ReapFailed);
                }
            }
        }
        self.reap()
    }

    pub(super) fn terminate_and_wait(
        &mut self,
        deadline: Instant,
    ) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        self.request_termination();
        match self.wait(deadline) {
            Err(IsolatedWorkerError::Timeout) if self.termination.signal_failed => {
                Err(IsolatedWorkerError::TerminationFailed)
            }
            other => other,
        }
    }

    fn reap(&mut self) -> Result<IsolatedWorkerExitKind, IsolatedWorkerError> {
        use std::os::unix::process::ExitStatusExt as _;

        let Ok(Some(status)) = self.child.try_wait() else {
            self.request_termination();
            return Err(IsolatedWorkerError::ReapFailed);
        };
        self.terminating_signal = status.signal();
        let exit = classify(status.code(), status.signal(), self.termination.signal_sent);
        self.reaped = Some(exit);
        Ok(exit)
    }

    /// Terminating signal of the reaped child, for tests that must tell an
    /// abort (`SIGABRT`) apart from another fatal signal.
    #[cfg(test)]
    pub(super) fn terminating_signal(&self) -> Option<i32> {
        self.terminating_signal
    }

    /// Process ID of the confined child, for the test that stops and
    /// continues it. The child is never reaped while a [`Backend`] is alive,
    /// so the ID cannot have been reused.
    #[cfg(test)]
    pub(super) fn child_process_id(&self) -> u32 {
        self.child.id()
    }
}

/// Classifies a `spawn` that failed: a bootstrap step reported itself on the
/// readiness pipe before `std` left the child, or nothing did and the process
/// could not be created at all.
fn spawn_failure(ready: BorrowedFd<'_>) -> IsolatedWorkerError {
    let mut byte = [0u8; 1];
    match rustix::io::read(ready, &mut byte) {
        Ok(1) => BootstrapFailure::from_byte(byte[0])
            .map_or(IsolatedWorkerError::ProcessCreationFailed, |failure| {
                failure.error()
            }),
        _ => IsolatedWorkerError::ProcessCreationFailed,
    }
}

/// Consumes the readiness pipe: exactly [`READY_ATTESTATION`] followed by
/// end-of-file, before `deadline`.
///
/// The child's copy of the pipe is the only writer left, so a child that dies
/// before attesting produces end-of-file with a short (or empty) record,
/// which is a bootstrap failure; there is no separate exit signal to poll
/// here.
fn await_ready(ready: BorrowedFd<'_>, deadline: Instant) -> Result<(), IsolatedWorkerError> {
    let mut received = 0usize;
    loop {
        let revents = match poll_until(ready, PollFlags::IN, deadline) {
            Ok(Some(revents)) => revents,
            Ok(None) => return Err(IsolatedWorkerError::Timeout),
            Err(()) => return Err(IsolatedWorkerError::BootstrapFailed),
        };
        if revents.intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR) {
            let mut byte = [0u8; 1];
            loop {
                match rustix::io::read(ready, &mut byte) {
                    Ok(1) => {
                        if received >= READY_ATTESTATION.len()
                            || byte[0] != READY_ATTESTATION[received]
                        {
                            return Err(if received == 0 {
                                BootstrapFailure::from_byte(byte[0])
                                    .map_or(IsolatedWorkerError::BootstrapFailed, |failure| {
                                        failure.error()
                                    })
                            } else {
                                IsolatedWorkerError::BootstrapFailed
                            });
                        }
                        received += 1;
                    }
                    Ok(_) => {
                        return if received == READY_ATTESTATION.len() {
                            Ok(())
                        } else {
                            Err(IsolatedWorkerError::BootstrapFailed)
                        };
                    }
                    Err(rustix::io::Errno::INTR) => {}
                    Err(rustix::io::Errno::AGAIN) => break,
                    Err(_) => return Err(IsolatedWorkerError::BootstrapFailed),
                }
            }
        }
        if remaining(deadline).is_zero() {
            return Err(IsolatedWorkerError::Timeout);
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if self.reaped.is_some() {
            return;
        }
        self.request_termination();
        kill_and_reap(&mut self.child, Some(self.termination.signal_failed));
    }
}

/// Last-resort reaper used when there is no live [`Backend`] to drive.
fn kill_and_reap(child: &mut Child, signal_already_failed: Option<bool>) {
    if signal_already_failed.unwrap_or(true) {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Maps a reaped status into the public vocabulary.
fn classify(
    code: Option<i32>,
    signal: Option<i32>,
    termination_requested: bool,
) -> IsolatedWorkerExitKind {
    const SIGKILL_NUMBER: i32 = 9;
    const SIGXCPU: i32 = 24;
    const SIGXFSZ: i32 = 25;
    match (code, signal) {
        (Some(0), _) => IsolatedWorkerExitKind::Clean,
        (Some(_), _) => IsolatedWorkerExitKind::Failed,
        (None, Some(SIGKILL_NUMBER)) if termination_requested => IsolatedWorkerExitKind::Terminated,
        (None, Some(SIGXCPU | SIGXFSZ)) => IsolatedWorkerExitKind::ResourceLimit,
        (None, Some(_)) => IsolatedWorkerExitKind::Crashed,
        (None, None) => IsolatedWorkerExitKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BootstrapFailure, CPU_TYPE_ARM64, CPU_TYPE_X86_64, IsolatedWorkerError,
        IsolatedWorkerExitKind, LC_LOAD_DYLIB, LC_LOAD_DYLINKER, LC_RPATH, MH_EXECUTE, MH_MAGIC_64,
        PROFILE_PLACEHOLDER, READY_ATTESTATION, classify, render_profile, verify_macho_bytes,
    };
    use std::path::Path;

    /// A load command of `kind` carrying one NUL-terminated string at offset
    /// 8 (a `dylib_command` has a 16-byte `dylib` struct there; only the
    /// name offset in its first word matters to the verifier).
    fn string_command(kind: u32, string_offset: u32, text: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&string_offset.to_le_bytes());
        // Pad the fixed part out to the declared string offset.
        while body.len() + 8 < string_offset as usize {
            body.push(0);
        }
        body.extend_from_slice(text);
        body.push(0);
        while (body.len() + 8) % 8 != 0 {
            body.push(0);
        }
        let mut command = Vec::new();
        command.extend_from_slice(&kind.to_le_bytes());
        command.extend_from_slice(&u32::try_from(body.len() + 8).expect("small").to_le_bytes());
        command.extend_from_slice(&body);
        command
    }

    fn segment_command() -> Vec<u8> {
        // `LC_SEGMENT_64` with an empty body beyond the header: the verifier
        // ignores it, which is what this fixture proves.
        let mut command = vec![0u8; 72];
        command[..4].copy_from_slice(&0x19_u32.to_le_bytes());
        command[4..8].copy_from_slice(&72_u32.to_le_bytes());
        command
    }

    fn macho(cpu_type: u32, file_type: u32, commands: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = commands.iter().flatten().copied().collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
        bytes.extend_from_slice(&cpu_type.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&file_type.to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(commands.len()).expect("small").to_le_bytes());
        bytes.extend_from_slice(&u32::try_from(body.len()).expect("small").to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&body);
        bytes
    }

    fn dyld() -> Vec<u8> {
        string_command(LC_LOAD_DYLINKER, 12, b"/usr/lib/dyld")
    }

    fn libsystem() -> Vec<u8> {
        string_command(LC_LOAD_DYLIB, 24, b"/usr/lib/libSystem.B.dylib")
    }

    #[test]
    fn a_hosted_rust_executable_shape_is_accepted() {
        let bytes = macho(
            CPU_TYPE_ARM64,
            MH_EXECUTE,
            &[segment_command(), dyld(), libsystem()],
        );
        assert!(verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
        assert!(
            !verify_macho_bytes(&bytes, CPU_TYPE_X86_64),
            "the CPU type must match the running process"
        );
    }

    #[test]
    fn an_extra_dynamic_library_is_rejected() {
        let extra = string_command(LC_LOAD_DYLIB, 24, b"/usr/lib/libz.1.dylib");
        let bytes = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[dyld(), libsystem(), extra]);
        assert!(!verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
    }

    #[test]
    fn a_run_path_search_is_rejected() {
        let rpath = string_command(LC_RPATH, 12, b"@loader_path");
        let bytes = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[dyld(), libsystem(), rpath]);
        assert!(!verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
    }

    #[test]
    fn a_foreign_dynamic_linker_is_rejected() {
        let linker = string_command(LC_LOAD_DYLINKER, 12, b"/tmp/dyld");
        let bytes = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[linker, libsystem()]);
        assert!(!verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
    }

    #[test]
    fn an_image_without_a_dynamic_linker_is_rejected() {
        let bytes = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[libsystem()]);
        assert!(!verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
    }

    #[test]
    fn a_dynamic_library_or_fat_file_is_rejected() {
        const MH_DYLIB: u32 = 6;
        let dylib = macho(CPU_TYPE_ARM64, MH_DYLIB, &[dyld(), libsystem()]);
        assert!(!verify_macho_bytes(&dylib, CPU_TYPE_ARM64));
        let mut fat = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[dyld(), libsystem()]);
        fat[..4].copy_from_slice(&0xcafe_babe_u32.to_be_bytes());
        assert!(!verify_macho_bytes(&fat, CPU_TYPE_ARM64));
    }

    #[test]
    fn truncated_or_inconsistent_load_commands_are_rejected() {
        let mut bytes = macho(CPU_TYPE_ARM64, MH_EXECUTE, &[dyld(), libsystem()]);
        // Declare more command bytes than are present.
        let oversized = u32::try_from(bytes.len()).expect("a small fixture");
        bytes[20..24].copy_from_slice(&oversized.to_le_bytes());
        assert!(!verify_macho_bytes(&bytes, CPU_TYPE_ARM64));
        assert!(!verify_macho_bytes(&bytes[..16], CPU_TYPE_ARM64));
    }

    #[test]
    fn the_profile_names_the_image_and_denies_by_default() {
        let profile =
            render_profile(Path::new("/opt/ohl/libexec/open-half-life/worker")).expect("renders");
        assert!(profile.contains("(deny default)"));
        assert!(
            profile.contains(
                "(allow process-exec (literal \"/opt/ohl/libexec/open-half-life/worker\"))"
            )
        );
        assert!(!profile.contains(PROFILE_PLACEHOLDER));
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn an_unspellable_image_path_fails_closed() {
        for path in [
            "relative/worker",
            "/opt/oh\"l/worker",
            "/opt/oh\\l/worker",
            "/opt/oh\nl",
        ] {
            assert_eq!(
                render_profile(Path::new(path)).err(),
                Some(IsolatedWorkerError::ConfinementUnavailable),
                "{path:?}"
            );
        }
    }

    #[test]
    fn bootstrap_failure_bytes_never_collide_with_the_attestation() {
        for byte in 1u8..=2 {
            assert!(BootstrapFailure::from_byte(byte).is_some());
            assert_ne!(byte, READY_ATTESTATION[0]);
        }
        assert!(BootstrapFailure::from_byte(0).is_none());
        assert!(BootstrapFailure::from_byte(3).is_none());
        assert_eq!(
            BootstrapFailure::ResourceLimits.error(),
            IsolatedWorkerError::ConfinementUnavailable
        );
        assert_eq!(
            BootstrapFailure::DescriptorSetup.error(),
            IsolatedWorkerError::BootstrapFailed
        );
    }

    #[test]
    fn wait_statuses_are_classified_like_the_linux_backend() {
        assert_eq!(
            classify(Some(0), None, false),
            IsolatedWorkerExitKind::Clean
        );
        assert_eq!(
            classify(Some(3), None, false),
            IsolatedWorkerExitKind::Failed
        );
        assert_eq!(
            classify(None, Some(9), true),
            IsolatedWorkerExitKind::Terminated
        );
        assert_eq!(
            classify(None, Some(9), false),
            IsolatedWorkerExitKind::Crashed
        );
        assert_eq!(
            classify(None, Some(24), false),
            IsolatedWorkerExitKind::ResourceLimit
        );
        assert_eq!(
            classify(None, Some(6), false),
            IsolatedWorkerExitKind::Crashed
        );
        assert_eq!(classify(None, None, false), IsolatedWorkerExitKind::Unknown);
    }
}
