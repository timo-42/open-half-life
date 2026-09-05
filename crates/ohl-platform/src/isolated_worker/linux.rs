//! Native Linux x86-64 isolated-worker backend.
//!
//! The child is confined before it ever runs service code, in this fixed
//! order (each step failing closed):
//!
//! 1. `dup3` the pre-verified descriptors onto 3 (channel), 4 (readiness),
//!    5 (image, `O_CLOEXEC`) and 6 (Landlock ruleset, `O_CLOEXEC`); 0/1/2 are
//!    `/dev/null` courtesy of [`std::process::Stdio::null`];
//! 2. `close_range(7, ~0)` so nothing else is inherited;
//! 3. `prlimit64` for the eight resource limits in [`RESOURCE_LIMITS`];
//! 4. `prctl(PR_SET_PDEATHSIG, SIGKILL)` plus a `getppid` re-check that
//!    closes the "parent already died" race;
//! 5. `prctl(PR_SET_NO_NEW_PRIVS, 1)`;
//! 6. `landlock_restrict_self` on a ruleset built (and ABI-probed) in the
//!    parent that handles every filesystem access the running kernel knows
//!    about and grants exactly one rule: execute+read on the worker image;
//! 7. the seccomp-BPF allowlist - compiled *and* wrapped in its
//!    `struct sock_fprog` by the parent, installed by the child with a raw
//!    `seccomp(SECCOMP_SET_MODE_FILTER, ...)` - with
//!    `SECCOMP_RET_KILL_PROCESS` as the mismatch action;
//! 8. `execveat(5, "", ["ohl-media-parser-worker"], [], AT_EMPTY_PATH)`.
//!
//! # Descriptor inventory
//!
//! Every descriptor the parent creates for a launch is `O_CLOEXEC` from the
//! syscall that creates it, so an unexpected `exec` anywhere in the parent
//! can never leak one: the channel socketpair (`SOCK_CLOEXEC`), the readiness
//! pipe (`O_CLOEXEC`), the verified image (`O_CLOEXEC`), the collision-safe
//! duplicates (`F_DUPFD_CLOEXEC`), the Landlock ruleset and its `O_PATH`
//! handle (the kernel and the `landlock` crate both force `O_CLOEXEC`), and
//! the pidfd (`pidfd_open` always sets `O_CLOEXEC`). The child then rebuilds
//! its own table from scratch: `dup3` puts the four inherited descriptors on
//! 3..=6 - 3 and 4 without `O_CLOEXEC` because the worker must keep them
//! across `execveat`, 5 and 6 with it because it must not - and
//! `close_range(7, ~0)` removes everything else, including the descriptors
//! `std`'s own spawn machinery holds. The worker therefore starts with
//! exactly `{0, 1, 2, 3, 4}` and, once it has attested readiness and closed
//! 4, with exactly `{0, 1, 2, 3}`; `RLIMIT_NOFILE` of 8 caps what it can ever
//! add. `fd_inventory_after_exec_is_exactly_the_contract` asserts this from
//! inside the confined child.
//!
//! Steps 3-8 are unreachable for a caller: the image is opened and verified
//! (regular file, owned by root or by this user, no write bits, no set-id
//! bits, executable, `ET_EXEC` x86-64 ELF64 with no `PT_INTERP` and no
//! `PT_DYNAMIC`) *before* the fork, and the descriptor - not the path - is
//! what gets executed, so there is no TOCTOU window.
//!
//! Everything the child does after `fork` is async-signal-safe: raw syscalls
//! and reads of buffers allocated by the parent, no allocation, no locks, no
//! libc. In particular the seccomp filter is installed through the same raw
//! `syscall` wrapper as every other step rather than through
//! `seccompiler::apply_filter`, which would route the last confinement step
//! through libc's PLT and its errno TLS slot.

use super::{
    IsolatedWorkerCancellationToken, IsolatedWorkerError, IsolatedWorkerExitKind,
    IsolatedWorkerService,
};

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::fs::FileExt as _;
use std::os::unix::process::CommandExt as _;
#[cfg(test)]
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr as _, RulesetCreatedAttr as _,
};
use rustix::event::{PollFd, PollFlags};
use rustix::fs::{Mode, OFlags};
use rustix::net::{AddressFamily, SendFlags, Shutdown, SocketFlags, SocketType};
use rustix::process::{Pid, Signal};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch, sock_filter,
};

/// Descriptor the private channel is bound to in the child.
const CHANNEL_FD: RawFd = 3;
/// Descriptor the one-shot readiness pipe is bound to in the child.
const READY_FD: RawFd = 4;
/// Descriptor the verified image is bound to in the child (`O_CLOEXEC`).
const IMAGE_FD: RawFd = 5;
/// Descriptor the Landlock ruleset is bound to in the child (`O_CLOEXEC`).
const RULESET_FD: RawFd = 6;
/// First descriptor number `close_range` sweeps away.
const FIRST_UNUSED_FD: isize = 7;
/// Lowest descriptor number the parent's collision-safe duplicates may take.
const FIRST_TEMPORARY_FD: RawFd = 10;

/// Exactly what a bootstrapped worker must write to [`READY_FD`] before
/// closing it. Anything else - including a prefix, a superset, or a held-open
/// descriptor - fails the launch.
pub(super) const READY_ATTESTATION: [u8; 16] = [
    b'O', b'H', b'L', b'I', b'S', b'O', b'L', b'A', b'T', b'E', b'D', 0, 1, 0, 0, 0,
];

/// `argv[0]` handed to the worker. The image is executed by descriptor, so
/// this is a label, not a lookup key.
const WORKER_ARGV0: &[u8] = b"ohl-media-parser-worker\0";

/// Install location of a service image, relative to the directory holding the
/// running executable. Compile-fixed: no caller and no environment variable
/// can influence it in a shipping build.
const SERVICE_IMAGE_RELATIVE_DIRECTORIES: [&str; 2] = ["libexec", "open-half-life"];

/// File name of the media-parser service image.
const MEDIA_PARSER_IMAGE_NAME: &str = "ohl-media-parser-worker";

/// The eight `prlimit64` limits, `(resource, soft, hard)`.
///
/// `RLIMIT_CPU` is the one limit with a soft/hard split, so the kernel raises
/// `SIGXCPU` (reported as [`IsolatedWorkerExitKind::ResourceLimit`]) 30
/// seconds before the unappealable `SIGKILL`.
const RESOURCE_LIMITS: [(isize, u64, u64); 8] = [
    (RLIMIT_AS, 512 * 1024 * 1024, 512 * 1024 * 1024),
    (RLIMIT_DATA, 256 * 1024 * 1024, 256 * 1024 * 1024),
    (RLIMIT_STACK, 8 * 1024 * 1024, 8 * 1024 * 1024),
    (RLIMIT_CPU, 300, 330),
    (RLIMIT_FSIZE, 0, 0),
    (RLIMIT_CORE, 0, 0),
    (RLIMIT_NOFILE, 8, 8),
    (RLIMIT_NPROC, 1, 1),
];

const RLIMIT_CPU: isize = 0;
const RLIMIT_FSIZE: isize = 1;
const RLIMIT_DATA: isize = 2;
const RLIMIT_STACK: isize = 3;
const RLIMIT_CORE: isize = 4;
const RLIMIT_NPROC: isize = 6;
const RLIMIT_NOFILE: isize = 7;
const RLIMIT_AS: isize = 9;

/// The complete seccomp allowlist. Everything else, on any architecture other
/// than x86-64, is `SECCOMP_RET_KILL_PROCESS`.
///
/// | syscall | why it is allowed |
/// |---------|-------------------|
/// | `execveat` | the final bootstrap step, and constrained to `dirfd == 5`, `flags == AT_EMPTY_PATH` |
/// | `read`, `write` | the protocol channel and the readiness attestation |
/// | `close` | closing [`READY_FD`] is what signals "ready" to the host |
/// | `ppoll` | lets a worker block for input instead of spinning against `RLIMIT_CPU` |
/// | `restart_syscall` | the kernel's own resumption of an interrupted blocking call (see below) |
/// | `exit`, `exit_group` | orderly shutdown |
///
/// `restart_syscall` is never issued by the worker: the kernel inserts it
/// when a process is stopped and continued (`SIGSTOP`/`SIGCONT`, a debugger
/// attach, cgroup freezing) in the middle of a blocking call that carries
/// resumption state in a `restart_block` (`ERESTART_RESTARTBLOCK`; a plain
/// socket `read` instead rewinds into the original syscall and needs
/// nothing). Denying it would turn such a stop into
/// `SECCOMP_RET_KILL_PROCESS` - a self-inflicted denial of service triggered
/// by something entirely outside the worker's control. It carries no
/// arguments of its own and can only resume a call the process already made
/// and the policy already allowed, so allowing it grants no new authority.
///
/// Notably absent: `openat`, `mmap`, `mprotect`, `brk`, `socket`, `clone`,
/// `fork`, `prctl` and every `*stat`. A statically linked, freestanding
/// worker needs none of them, so any appearance is a genuine escape attempt.
const ALLOWED_SYSCALLS: [isize; 7] = [
    SYS_READ,
    SYS_WRITE,
    SYS_CLOSE,
    SYS_PPOLL,
    SYS_RESTART_SYSCALL,
    SYS_EXIT,
    SYS_EXIT_GROUP,
];

const SYS_READ: isize = 0;
const SYS_WRITE: isize = 1;
const SYS_CLOSE: isize = 3;
const SYS_PPOLL: isize = 271;
const SYS_RESTART_SYSCALL: isize = 219;
const SYS_EXIT: isize = 60;
const SYS_EXIT_GROUP: isize = 231;
const SYS_EXECVEAT: isize = 322;

const AT_EMPTY_PATH: isize = 0x1000;

/// Bootstrap steps, reported as a single byte on [`READY_FD`] when they fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BootstrapFailure {
    DescriptorSetup = 1,
    ResourceLimits = 2,
    NoNewPrivileges = 3,
    Landlock = 4,
    Seccomp = 5,
    Execute = 6,
    ParentDied = 7,
}

impl BootstrapFailure {
    const fn error(self) -> IsolatedWorkerError {
        match self {
            Self::ResourceLimits | Self::NoNewPrivileges | Self::Landlock | Self::Seccomp => {
                IsolatedWorkerError::ConfinementUnavailable
            }
            Self::DescriptorSetup | Self::Execute | Self::ParentDied => {
                IsolatedWorkerError::BootstrapFailed
            }
        }
    }

    const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::DescriptorSetup,
            2 => Self::ResourceLimits,
            3 => Self::NoNewPrivileges,
            4 => Self::Landlock,
            5 => Self::Seccomp,
            6 => Self::Execute,
            7 => Self::ParentDied,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Raw syscalls used after `fork`
// ---------------------------------------------------------------------------

/// Raw `syscall` instruction wrappers, used only between `fork` and `execveat`
/// where allocation, locking, and most library code are forbidden.
///
/// # Clobbers and options
///
/// Both blocks follow the x86-64 Linux syscall ABI exactly:
///
/// - `rax` carries the syscall number in and the result out; `rdi`, `rsi`,
///   `rdx`, `r10`, `r8` (and `r9`, unused here) carry the arguments;
/// - the `syscall` instruction destroys `rcx` (return address) and `r11`
///   (saved `rflags`), so both are declared `lateout(_)`;
/// - the **`memory` clobber is implicit**: `asm!` assumes a block reads and
///   writes arbitrary memory unless `options(nomem)` or `options(readonly)`
///   is given, and neither is, because the kernel does read and write through
///   the pointer arguments. This is what keeps the compiler from reordering
///   or eliding stores into buffers that a syscall then reads;
/// - `options(nostack)` is valid: `syscall` pushes nothing, the kernel
///   switches to its own stack, and neither block touches the red zone or
///   emits a call;
/// - `options(preserves_flags)` is deliberately *not* given, because the
///   kernel entry sequence clobbers `rflags` through `r11`;
/// - `options(pure)` / `readonly` are likewise absent - a syscall is a side
///   effect and must never be cached or reordered.
///
/// See the `unsafe` inventory in `crate` docs, entries 3-5.
mod raw {
    use std::arch::asm;

    /// # Safety
    ///
    /// The caller must supply a syscall number and arguments that form a
    /// well-defined call: any pointer argument must be valid for the access
    /// the kernel performs, for the whole call.
    pub(super) unsafe fn syscall(
        number: isize,
        a0: isize,
        a1: isize,
        a2: isize,
        a3: isize,
        a4: isize,
    ) -> isize {
        let result: isize;
        // SAFETY: guaranteed by this function's own contract. The clobbers
        // (`rcx`, `r11`, and the implicit `memory` clobber) are the ones the
        // x86-64 kernel entry sequence requires; see the module docs.
        unsafe {
            asm!(
                "syscall",
                inlateout("rax") number => result,
                in("rdi") a0,
                in("rsi") a1,
                in("rdx") a2,
                in("r10") a3,
                in("r8") a4,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack)
            );
        }
        result
    }

    /// Terminates the whole process immediately, without running any
    /// destructor, atexit handler, or buffered flush.
    pub(super) fn exit_group(status: i32) -> ! {
        // SAFETY: `exit_group` never returns and dereferences nothing, so
        // `noreturn` is correct by construction and the implicit `memory`
        // clobber is irrelevant. `nostack` holds for the same reason as in
        // `syscall`.
        unsafe {
            asm!(
                "syscall",
                in("rax") 231usize,
                in("rdi") status as isize,
                options(noreturn, nostack)
            )
        }
    }
}

const SYS_DUP3: isize = 292;
const SYS_CLOSE_RANGE: isize = 436;
const SYS_PRLIMIT64: isize = 302;
const SYS_PRCTL: isize = 157;
const SYS_GETPPID: isize = 110;
const SYS_LANDLOCK_RESTRICT_SELF: isize = 446;
const SYS_SECCOMP: isize = 317;

const PR_SET_PDEATHSIG: isize = 1;
const PR_SET_NO_NEW_PRIVS: isize = 38;
const SIGKILL: isize = 9;
const O_CLOEXEC: isize = 0o2_000_000;
const SECCOMP_SET_MODE_FILTER: isize = 1;
/// `BPF_MAXINSNS`: the kernel's own ceiling on a filter program, and the
/// reason `struct sock_fprog::len` is only 16 bits wide.
const BPF_MAX_INSTRUCTIONS: usize = 4096;

/// The `struct rlimit64` layout `prlimit64` expects.
#[repr(C)]
struct Rlimit64 {
    soft: u64,
    hard: u64,
}

/// The `struct sock_fprog` layout `seccomp(SECCOMP_SET_MODE_FILTER, ...)`
/// expects: an instruction count and a pointer to that many `sock_filter`s.
///
/// Only ever built from [`ChildBootstrap::filter_pointer`] and
/// [`ChildBootstrap::filter_length`], both computed in the parent.
#[repr(C)]
struct SockFprog {
    length: u16,
    filter: *const sock_filter,
}

/// Everything the child needs, captured before `fork` so that the post-fork
/// path only reads plain data.
///
/// `filter` owns the compiled BPF program; it is kept alive here for the
/// whole lifetime of the bootstrap so that the address in `filter_pointer`
/// stays valid across `fork` (moving the `Vec` moves the handle, never the
/// heap buffer it points at). The pointer is held as a `usize` rather than a
/// raw pointer so the struct stays `Send + Sync`, which `pre_exec` requires,
/// without any `unsafe impl`.
struct ChildBootstrap {
    channel: RawFd,
    ready: RawFd,
    image: RawFd,
    ruleset: RawFd,
    parent: u32,
    filter: BpfProgram,
    filter_pointer: usize,
    filter_length: u16,
}

impl ChildBootstrap {
    /// Builds the bootstrap in the parent, including the `sock_fprog` fields
    /// the child installs verbatim.
    fn new(
        channel: RawFd,
        ready: RawFd,
        image: RawFd,
        ruleset: RawFd,
        filter: BpfProgram,
    ) -> Result<Self, IsolatedWorkerError> {
        if filter.is_empty() || filter.len() > BPF_MAX_INSTRUCTIONS {
            return Err(IsolatedWorkerError::ConfinementUnavailable);
        }
        let filter_length =
            u16::try_from(filter.len()).map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
        let filter_pointer = filter.as_ptr() as usize;
        Ok(Self {
            channel,
            ready,
            image,
            ruleset,
            parent: std::process::id(),
            filter,
            filter_pointer,
            filter_length,
        })
    }

    /// Reports `failure` on `descriptor` and leaves immediately.
    fn fail(descriptor: RawFd, failure: BootstrapFailure) -> ! {
        let byte = failure as u8;
        // SAFETY: `write` reads one byte from a live local. A short or failed
        // write cannot be handled any better than by exiting anyway.
        unsafe {
            raw::syscall(
                SYS_WRITE,
                descriptor as isize,
                std::ptr::from_ref(&byte) as isize,
                1,
                0,
                0,
            );
        }
        raw::exit_group(127)
    }

    /// Runs entirely in the forked child. Never returns.
    fn run(&self) -> ! {
        // SAFETY (this whole function): every call is a raw syscall taking
        // integers or pointers to live locals of this frame; nothing
        // allocates, locks, or touches parent memory that could be in an
        // inconsistent post-fork state.
        let moved = unsafe {
            raw::syscall(
                SYS_DUP3,
                self.channel as isize,
                CHANNEL_FD as isize,
                0,
                0,
                0,
            ) >= 0
                && raw::syscall(SYS_DUP3, self.ready as isize, READY_FD as isize, 0, 0, 0) >= 0
                && raw::syscall(
                    SYS_DUP3,
                    self.image as isize,
                    IMAGE_FD as isize,
                    O_CLOEXEC,
                    0,
                    0,
                ) >= 0
                && raw::syscall(
                    SYS_DUP3,
                    self.ruleset as isize,
                    RULESET_FD as isize,
                    O_CLOEXEC,
                    0,
                    0,
                ) >= 0
        };
        if !moved {
            Self::fail(self.ready, BootstrapFailure::DescriptorSetup);
        }

        // SAFETY: as above.
        if unsafe { raw::syscall(SYS_CLOSE_RANGE, FIRST_UNUSED_FD, 0xffff_ffff, 0, 0, 0) } != 0 {
            Self::fail(READY_FD, BootstrapFailure::DescriptorSetup);
        }

        for (resource, soft, hard) in RESOURCE_LIMITS {
            let limit = Rlimit64 { soft, hard };
            // SAFETY: as above; `prlimit64` reads 16 bytes from `limit`.
            if unsafe {
                raw::syscall(
                    SYS_PRLIMIT64,
                    0,
                    resource,
                    std::ptr::from_ref(&limit) as isize,
                    0,
                    0,
                )
            } != 0
            {
                Self::fail(READY_FD, BootstrapFailure::ResourceLimits);
            }
        }

        // SAFETY: as above.
        let parented = unsafe {
            raw::syscall(SYS_PRCTL, PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) == 0
                && usize::try_from(raw::syscall(SYS_GETPPID, 0, 0, 0, 0, 0))
                    == Ok(self.parent as usize)
        };
        if !parented {
            Self::fail(READY_FD, BootstrapFailure::ParentDied);
        }

        // SAFETY: as above.
        if unsafe { raw::syscall(SYS_PRCTL, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            Self::fail(READY_FD, BootstrapFailure::NoNewPrivileges);
        }

        // SAFETY: as above.
        if unsafe { raw::syscall(SYS_LANDLOCK_RESTRICT_SELF, RULESET_FD as isize, 0, 0, 0, 0) } != 0
        {
            Self::fail(READY_FD, BootstrapFailure::Landlock);
        }

        // The filter is installed by hand instead of through
        // `seccompiler::apply_filter`, which would call `libc::prctl` and
        // `libc::syscall` - a PLT indirection plus an errno TLS write - in the
        // one place where only raw syscalls are allowed. `program` is a plain
        // stack local; the instruction buffer it points at was allocated,
        // measured, and address-captured by the parent before `fork`, so
        // nothing here allocates.
        let program = SockFprog {
            length: self.filter_length,
            filter: self.filter_pointer as *const sock_filter,
        };
        let installed = self.filter_pointer == self.filter.as_ptr() as usize
            && usize::from(self.filter_length) == self.filter.len()
        // SAFETY: as above; `seccomp` reads `length` instructions from the
        // program the parent compiled, which outlives this call. Flags are
        // zero, so the syscall returns instead of taking a listener path.
            && unsafe {
                raw::syscall(
                    SYS_SECCOMP,
                    SECCOMP_SET_MODE_FILTER,
                    0,
                    std::ptr::from_ref(&program) as isize,
                    0,
                    0,
                )
            } == 0;
        if !installed {
            Self::fail(READY_FD, BootstrapFailure::Seccomp);
        }

        let argv: [*const u8; 2] = [WORKER_ARGV0.as_ptr(), std::ptr::null()];
        let envp: [*const u8; 1] = [std::ptr::null()];
        let empty: [u8; 1] = [0];
        // SAFETY: as above; `execveat` reads the NUL-terminated `empty`
        // path and the two NULL-terminated pointer vectors, all live here.
        unsafe {
            raw::syscall(
                SYS_EXECVEAT,
                IMAGE_FD as isize,
                empty.as_ptr() as isize,
                argv.as_ptr() as isize,
                envp.as_ptr() as isize,
                AT_EMPTY_PATH,
            );
        }
        Self::fail(READY_FD, BootstrapFailure::Execute)
    }
}

// ---------------------------------------------------------------------------
// Image resolution and verification (parent, before fork)
// ---------------------------------------------------------------------------

fn open_error(error: rustix::io::Errno) -> IsolatedWorkerError {
    if error == rustix::io::Errno::NOENT {
        IsolatedWorkerError::ServiceUnavailable
    } else {
        IsolatedWorkerError::ServiceIdentityMismatch
    }
}

/// A directory on the install path must be a directory owned by root or by
/// this user and writable by nobody else.
fn trusted_directory(descriptor: BorrowedFd<'_>) -> bool {
    let Ok(status) = rustix::fs::fstat(descriptor) else {
        return false;
    };
    let mode = status.st_mode;
    mode & FILE_TYPE_MASK == DIRECTORY_TYPE
        && (status.st_uid == 0 || status.st_uid == rustix::process::geteuid().as_raw())
        && mode & (0o020 | 0o002 | 0o4000 | 0o2000) == 0
}

const FILE_TYPE_MASK: u32 = 0o170_000;
const DIRECTORY_TYPE: u32 = 0o040_000;
const REGULAR_TYPE: u32 = 0o100_000;

/// A service image must be a regular file owned by root or by this user, with
/// no write bit, no set-id bit, and at least one execute bit.
fn trusted_image_metadata(status: &rustix::fs::Stat) -> bool {
    let mode = status.st_mode;
    mode & FILE_TYPE_MASK == REGULAR_TYPE
        && (status.st_uid == 0 || status.st_uid == rustix::process::geteuid().as_raw())
        && mode & 0o222 == 0
        && mode & (0o4000 | 0o2000) == 0
        && mode & 0o111 != 0
}

/// Verifies the ELF identity by reading through the already-open descriptor,
/// so what is inspected is exactly what will be executed.
fn verify_static_elf(file: &File, size: u64) -> bool {
    const ELF64_HEADER_BYTES: usize = 64;
    const PROGRAM_HEADER_BYTES: u64 = 56;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;

    let mut header = [0u8; ELF64_HEADER_BYTES];
    if size < ELF64_HEADER_BYTES as u64 || file.read_exact_at(&mut header, 0).is_err() {
        return false;
    }
    let read_u16 = |offset: usize| u16::from_le_bytes([header[offset], header[offset + 1]]);
    let read_u64 = |offset: usize| {
        u64::from_le_bytes(
            header[offset..offset + 8]
                .try_into()
                .expect("eight bytes inside a 64-byte header"),
        )
    };

    if header[..4] != [0x7f, b'E', b'L', b'F']
        || header[4] != 2
        || header[5] != 1
        || read_u16(0x10) != 2
        || read_u16(0x12) != 62
        || u64::from(read_u16(0x36)) != PROGRAM_HEADER_BYTES
    {
        return false;
    }

    let count = u64::from(read_u16(0x38));
    if count == 0 || count > 1024 {
        return false;
    }
    let table_offset = read_u64(0x20);
    let Some(table_bytes) = count.checked_mul(PROGRAM_HEADER_BYTES) else {
        return false;
    };
    if table_offset > size || table_bytes > size - table_offset {
        return false;
    }

    for index in 0..count {
        let mut kind = [0u8; 4];
        let offset = table_offset + index * PROGRAM_HEADER_BYTES;
        if file.read_exact_at(&mut kind, offset).is_err() {
            return false;
        }
        let kind = u32::from_le_bytes(kind);
        if kind == PT_INTERP || kind == PT_DYNAMIC {
            return false;
        }
    }
    true
}

/// Opens `path` with `O_NOFOLLOW` and applies the full metadata plus ELF
/// policy. The returned file is the descriptor that will be executed.
///
/// Only the tests need to name an image by path. Shipping code reaches an
/// image exclusively through [`resolve_service_image`], which is compile-fixed
/// and honours no environment variable, so a deployed binary cannot be
/// redirected at a different program.
#[cfg(test)]
fn open_verified_image(path: &Path) -> Result<File, IsolatedWorkerError> {
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    verify_open_image(descriptor)
}

fn verify_open_image(descriptor: OwnedFd) -> Result<File, IsolatedWorkerError> {
    let status =
        rustix::fs::fstat(&descriptor).map_err(|_| IsolatedWorkerError::ServiceIdentityMismatch)?;
    if !trusted_image_metadata(&status) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }
    let size =
        u64::try_from(status.st_size).map_err(|_| IsolatedWorkerError::ServiceIdentityMismatch)?;
    let file = File::from(descriptor);
    if !verify_static_elf(&file, size) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }
    Ok(file)
}

/// Walks `<dir of current executable>/libexec/open-half-life/<image>` one
/// `O_NOFOLLOW` component at a time, verifying each directory on the way.
fn resolve_service_image(service: IsolatedWorkerService) -> Result<File, IsolatedWorkerError> {
    let name = match service {
        IsolatedWorkerService::MediaParser => MEDIA_PARSER_IMAGE_NAME,
    };

    let executable =
        std::env::current_exe().map_err(|_| IsolatedWorkerError::ServiceUnavailable)?;
    let base = executable
        .parent()
        .ok_or(IsolatedWorkerError::ServiceUnavailable)?;

    let mut directory = rustix::fs::open(
        base,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    for component in SERVICE_IMAGE_RELATIVE_DIRECTORIES {
        if !trusted_directory(directory.as_fd()) {
            return Err(IsolatedWorkerError::ServiceIdentityMismatch);
        }
        directory = rustix::fs::openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(open_error)?;
    }
    if !trusted_directory(directory.as_fd()) {
        return Err(IsolatedWorkerError::ServiceIdentityMismatch);
    }

    let image = rustix::fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_error)?;
    verify_open_image(image)
}

// ---------------------------------------------------------------------------
// Confinement policy built in the parent
// ---------------------------------------------------------------------------

/// Highest Landlock ABI the pinned `landlock` crate can express. Combined
/// with the crate's default best-effort compatibility level, asking for every
/// access of this ABI means "handle everything this kernel knows about".
const HIGHEST_KNOWN_LANDLOCK_ABI: ABI = ABI::V9;

/// Builds the Landlock ruleset and hands back its descriptor.
///
/// The ruleset *handles* every filesystem access the running kernel's ABI
/// knows about, and grants exactly one rule - execute and read on the worker
/// image - because `execveat` still has to work after `restrict_self`. Any
/// other path is unreachable for the worker.
///
/// # `/proc/self/fd` dependency
///
/// Landlock rules are attached to a *path*, not to a descriptor, so the one
/// rule this ruleset grants has to name the already-open, already-verified
/// image. `/proc/self/fd/<n>` is the only way to do that without reopening
/// the file by its real name and reintroducing the TOCTOU window the
/// descriptor-based verification exists to close. That makes a mounted
/// `/proc` (with the running process able to see its own `self/fd`) a hard
/// requirement of this backend: in a mount namespace without `/proc`, or
/// under a `hidepid`/`subset=pid` configuration that hides it, `PathFd::new`
/// fails and this function returns
/// [`IsolatedWorkerError::ConfinementUnavailable`]. That is the intended
/// behaviour - **it fails closed**: no ruleset means no launch, never a
/// launch with a weaker sandbox. The same is true of a kernel without
/// Landlock, or one whose ABI the pinned `landlock` crate cannot express.
fn build_landlock_ruleset(image: &File) -> Result<OwnedFd, IsolatedWorkerError> {
    let path = PathFd::new(format!("/proc/self/fd/{}", image.as_raw_fd()))
        .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    let created = Ruleset::default()
        .handle_access(AccessFs::from_all(HIGHEST_KNOWN_LANDLOCK_ABI))
        .and_then(Ruleset::create)
        .and_then(|ruleset| {
            ruleset.add_rule(PathBeneath::new(
                path,
                AccessFs::Execute | AccessFs::ReadFile,
            ))
        })
        .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    Option::<OwnedFd>::from(created).ok_or(IsolatedWorkerError::ConfinementUnavailable)
}

/// Compiles [`ALLOWED_SYSCALLS`] plus the argument-constrained `execveat`
/// into BPF, in the parent, so the child only has to install it.
fn build_seccomp_filter() -> Result<BpfProgram, IsolatedWorkerError> {
    let number = |value: isize| {
        i64::try_from(value).map_err(|_| IsolatedWorkerError::ConfinementUnavailable)
    };
    let mut rules: std::collections::BTreeMap<i64, Vec<SeccompRule>> = ALLOWED_SYSCALLS
        .into_iter()
        .map(|syscall| number(syscall).map(|syscall| (syscall, Vec::new())))
        .collect::<Result<_, _>>()?;

    let execveat = SeccompRule::new(vec![
        SeccompCondition::new(
            0,
            SeccompCmpArgLen::Qword,
            SeccompCmpOp::Eq,
            u64::try_from(IMAGE_FD).map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?,
        )
        .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?,
        SeccompCondition::new(
            4,
            SeccompCmpArgLen::Qword,
            SeccompCmpOp::Eq,
            u64::try_from(AT_EMPTY_PATH)
                .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?,
        )
        .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?,
    ])
    .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    rules.insert(number(SYS_EXECVEAT)?, vec![execveat]);

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::KillProcess,
        SeccompAction::Allow,
        TargetArch::x86_64,
    )
    .map_err(|_| IsolatedWorkerError::ConfinementUnavailable)?;
    BpfProgram::try_from(filter).map_err(|_| IsolatedWorkerError::ConfinementUnavailable)
}

// ---------------------------------------------------------------------------
// Deadline-aware polling
// ---------------------------------------------------------------------------

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn to_timespec(duration: Duration) -> rustix::fs::Timespec {
    rustix::fs::Timespec {
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

/// One confined child, its pidfd, and its half of the channel.
#[derive(Debug)]
pub(super) struct Backend {
    child: Child,
    pidfd: OwnedFd,
    channel: OwnedFd,
    aborted: bool,
    channel_shutdown: bool,
    termination: TerminationState,
    reaped: Option<IsolatedWorkerExitKind>,
    /// Terminating signal number of the reaped child, kept for tests that
    /// need to distinguish a seccomp kill (`SIGSYS`) from another fault.
    terminating_signal: Option<i32>,
}

impl Backend {
    pub(super) fn launch(
        service: IsolatedWorkerService,
        startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        let image = resolve_service_image(service)?;
        Self::launch_image(&image, startup_deadline)
    }

    /// Test-only entry point that skips install-location resolution but
    /// applies exactly the same verification and confinement.
    #[cfg(test)]
    pub(super) fn launch_verified_path(
        path: &Path,
        startup_deadline: Instant,
    ) -> Result<Self, IsolatedWorkerError> {
        let image = open_verified_image(path)?;
        Self::launch_image(&image, startup_deadline)
    }

    fn launch_image(image: &File, startup_deadline: Instant) -> Result<Self, IsolatedWorkerError> {
        let ruleset = build_landlock_ruleset(image)?;
        let filter = build_seccomp_filter()?;

        let (parent_channel, child_channel) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        // Only the host side is non-blocking: the worker gets an ordinary
        // blocking descriptor so it never needs a syscall outside the
        // allowlist to wait for work.
        rustix::io::ioctl_fionbio(&parent_channel, true)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;

        let (ready_read, ready_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;
        rustix::io::ioctl_fionbio(&ready_read, true)
            .map_err(|_| IsolatedWorkerError::ChannelCreationFailed)?;

        // Collision-safe duplicates: the child's bootstrap moves descriptors
        // onto 3..=6, so the originals must not already live there.
        let temporary = |descriptor: BorrowedFd<'_>| {
            rustix::io::fcntl_dupfd_cloexec(descriptor, FIRST_TEMPORARY_FD)
                .map_err(|_| IsolatedWorkerError::ResourceExhausted)
        };
        let channel_copy = temporary(child_channel.as_fd())?;
        let ready_copy = temporary(ready_write.as_fd())?;
        let image_copy = temporary(image.as_fd())?;
        let ruleset_copy = temporary(ruleset.as_fd())?;

        let bootstrap = ChildBootstrap::new(
            channel_copy.as_raw_fd(),
            ready_copy.as_raw_fd(),
            image_copy.as_raw_fd(),
            ruleset_copy.as_raw_fd(),
            filter,
        )?;

        // The program path is never used: `pre_exec` diverges into
        // `execveat` on the verified descriptor. `Stdio::null` is what puts
        // `/dev/null` on 0/1/2.
        let mut command = Command::new("/nonexistent/ohl-isolated-worker");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: `unsafe` inventory entry 3. `ChildBootstrap::run` performs
        // raw syscalls only - no allocation, no locks, no library state - so
        // it is async-signal-safe as `pre_exec` requires, and it diverges, so
        // the standard library's own post-fork path is never reached.
        unsafe {
            command.pre_exec(move || bootstrap.run());
        }

        let child = command
            .spawn()
            .map_err(|_| IsolatedWorkerError::ProcessCreationFailed)?;

        drop((child_channel, ready_write, channel_copy, ready_copy));
        drop((image_copy, ruleset_copy, ruleset));

        let pid = Pid::from_child(&child);
        let Ok(pidfd) = rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty())
        else {
            let mut child = child;
            kill_and_reap(&mut child, None);
            return Err(IsolatedWorkerError::ConfinementUnavailable);
        };

        let mut backend = Self {
            child,
            pidfd,
            channel: parent_channel,
            aborted: false,
            channel_shutdown: false,
            termination: TerminationState::default(),
            reaped: None,
            terminating_signal: None,
        };

        match backend.await_ready(ready_read.as_fd(), startup_deadline) {
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

    /// Consumes the readiness pipe: exactly [`READY_ATTESTATION`] followed by
    /// end-of-file, before `deadline`, and only while the child still lives.
    fn await_ready(
        &mut self,
        ready: BorrowedFd<'_>,
        deadline: Instant,
    ) -> Result<(), IsolatedWorkerError> {
        let mut received = 0usize;
        loop {
            let mut items = [
                PollFd::new(&ready, PollFlags::IN),
                PollFd::new(&self.pidfd, PollFlags::IN),
            ];
            let timeout = to_timespec(remaining(deadline));
            match rustix::event::poll(&mut items, Some(&timeout)) {
                Ok(0) => return Err(IsolatedWorkerError::Timeout),
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => {
                    if remaining(deadline).is_zero() {
                        return Err(IsolatedWorkerError::Timeout);
                    }
                    continue;
                }
                Err(_) => return Err(IsolatedWorkerError::BootstrapFailed),
            }
            if items[0].revents().intersects(PollFlags::NVAL)
                || items[1].revents().intersects(PollFlags::NVAL)
            {
                return Err(IsolatedWorkerError::BootstrapFailed);
            }

            if items[0]
                .revents()
                .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
            {
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

            if items[1]
                .revents()
                .intersects(PollFlags::IN | PollFlags::HUP | PollFlags::ERR)
                && received != READY_ATTESTATION.len()
            {
                return Err(IsolatedWorkerError::BootstrapFailed);
            }
            if remaining(deadline).is_zero() {
                return Err(IsolatedWorkerError::Timeout);
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
            match rustix::net::send(&self.channel, &source[transferred..], SendFlags::NOSIGNAL) {
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
        match rustix::process::pidfd_send_signal(&self.pidfd, Signal::KILL) {
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
        match poll_until(self.pidfd.as_fd(), PollFlags::IN, deadline) {
            Err(()) => {
                self.request_termination();
                Err(IsolatedWorkerError::ReapFailed)
            }
            Ok(None) => Err(IsolatedWorkerError::Timeout),
            Ok(Some(_)) => self.reap(),
        }
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

    /// Terminating signal of the reaped child, for tests that must tell a
    /// seccomp kill (`SIGSYS`) apart from another fatal signal.
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
        BootstrapFailure, IsolatedWorkerError, IsolatedWorkerExitKind, READY_ATTESTATION, classify,
    };

    #[test]
    fn bootstrap_failure_bytes_never_collide_with_the_attestation() {
        for byte in 1u8..=7 {
            assert!(BootstrapFailure::from_byte(byte).is_some());
            assert_ne!(byte, READY_ATTESTATION[0]);
        }
        assert!(BootstrapFailure::from_byte(0).is_none());
        assert!(BootstrapFailure::from_byte(8).is_none());
    }

    #[test]
    fn confinement_failures_are_reported_as_confinement_failures() {
        for failure in [
            BootstrapFailure::ResourceLimits,
            BootstrapFailure::NoNewPrivileges,
            BootstrapFailure::Landlock,
            BootstrapFailure::Seccomp,
        ] {
            assert_eq!(failure.error(), IsolatedWorkerError::ConfinementUnavailable);
        }
        for failure in [
            BootstrapFailure::DescriptorSetup,
            BootstrapFailure::Execute,
            BootstrapFailure::ParentDied,
        ] {
            assert_eq!(failure.error(), IsolatedWorkerError::BootstrapFailed);
        }
    }

    #[test]
    fn wait_statuses_are_classified_like_the_reference_backend() {
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
            classify(None, Some(31), false),
            IsolatedWorkerExitKind::Crashed
        );
        assert_eq!(classify(None, None, false), IsolatedWorkerExitKind::Unknown);
    }
}
