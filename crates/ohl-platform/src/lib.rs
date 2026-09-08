//! Native platform capabilities for the Open Half-Life Rust port.
//!
//! This crate owns the two native primitives the rest of the port is allowed
//! to rely on:
//!
//! - [`MediaSource`], a move-only, read-only capability for exactly one
//!   natively opened file object (see [`media_source`]);
//! - the atomic-directory publication primitives in [`atomic_directory`];
//! - [`IsolatedWorker`], one confined child process and its private
//!   full-duplex byte channel (see [`isolated_worker`]).
//!
//! It depends only on `ohl-core` inside the workspace, and reports every
//! failure through fixed, payload-free codes that convert into
//! [`ohl_core::SanitizedError`]. No error, `Debug`, or `Display` output
//! produced here ever contains a path, a media-derived byte, or an OS error
//! string.
//!
//! # `unsafe` inventory
//!
//! `ohl-platform` is the **only** library crate in the workspace where
//! `unsafe_code` is `allow` rather than the workspace-wide `forbid`; the
//! allowance lives in this crate's `Cargo.toml` because a `forbid` level
//! cannot be relaxed by a crate-level attribute. Every unsafe site in the
//! crate is listed here. The `MediaSource` Unix backend, the
//! atomic-directory primitives, the stability checker, and every test are
//! entirely safe code.
//!
//! | # | Site | Call | Why it is needed | Why it is sound |
//! |---|------|------|------------------|-----------------|
//! | 1 | `sys::windows::file_type` | `GetFileType` | Rejecting pipes, character devices, and unknown handle types before pinning. `std` exposes no equivalent. | The handle is borrowed from a live, owned [`std::fs::File`] for the duration of the call, so it is a valid open handle. The call takes no out-parameters and cannot fail in a way that writes memory. |
//! | 2 | `sys::windows::file_information` | `GetFileInformationByHandle` | Reading the volume serial number and the 64-bit file index, which together are the pinned native identity. The `std` accessors for these (`MetadataExt::volume_serial_number`, `MetadataExt::file_index`) are permanently unstable behind `windows_by_handle`. | Same borrowed-handle argument as above. The single out-parameter is a stack-allocated, fully zero-initialised `BY_HANDLE_FILE_INFORMATION` passed by exclusive reference; it is only read after the call reports success. |
//! | 3 | `isolated_worker::linux::Backend::launch_image` | `Command::pre_exec` | Nothing safe can run code between `fork` and `exec`, and the entire confinement policy (descriptors, rlimits, `PR_SET_PDEATHSIG`, `PR_SET_NO_NEW_PRIVS`, Landlock, seccomp, `execveat`) has to be applied there. | The closure is `ChildBootstrap::run`, which performs raw syscalls only: no allocation, no locking, no library state, not even libc (the seccomp filter is installed with a raw `seccomp(SECCOMP_SET_MODE_FILTER, ...)` rather than `seccompiler::apply_filter`, whose `libc::prctl`/`libc::syscall` would add a PLT indirection and an errno TLS write), so it is async-signal-safe. It diverges into `execveat` or `exit_group`, so the standard library's own post-fork path is never reached and the child can never return into parent code. |
//! | 4 | `isolated_worker::linux::raw::syscall` | `asm!("syscall")` | The post-fork steps (`dup3`, `close_range`, `prlimit64`, `prctl`, `landlock_restrict_self`, `seccomp`, `execveat`, `write`) must be raw syscalls to stay async-signal-safe; several of them have no `rustix` equivalent at the pinned version. | Every call site passes integers or pointers to live locals of the calling frame, valid for the whole call. The only heap buffer the kernel is pointed at is the compiled BPF program, which the parent allocates before `fork` and the `ChildBootstrap` keeps alive (its address and instruction count are captured in the parent and re-checked against the live `Vec` before use). Clobbers: `rax` is `inlateout`, `rcx` and `r11` are declared `lateout(_)` because the `syscall` instruction destroys them, and the `memory` clobber is **implicit** - `asm!` assumes arbitrary reads and writes unless `options(nomem)` or `options(readonly)` is given, and neither is, which is exactly what the kernel's access through the pointer arguments requires. `options(nostack)` is given and valid (`syscall` pushes nothing, the kernel switches stacks, no red-zone use); `preserves_flags` is deliberately omitted because the entry sequence clobbers `rflags` via `r11`. |
//! | 5 | `isolated_worker::linux::raw::exit_group` | `asm!("syscall")` | Leaving a failed child immediately without running a destructor, atexit handler, or buffered flush inherited from the parent. | `exit_group` cannot return and dereferences nothing, so `options(noreturn)` is correct by construction and the implicit `memory` clobber is immaterial; `options(nostack)` holds for the same reason as at site 4. |
//! | 6 | `isolated_worker::macos::Backend::launch_image` | `Command::pre_exec` | As at site 3: the descriptor moves, the resource limits and the descriptor sweep have to happen between `fork` and the `exec` of `sandbox-exec`. | The closure is `ChildBootstrap::run`, which performs only `dup2`, `setrlimit`, `close` and one `write` - each a direct libc syscall wrapper through `rustix` on integers captured before the fork - so it allocates nothing, takes no lock and touches no library state; it is async-signal-safe as `pre_exec` requires. It returns instead of diverging, so `std` performs the `exec` (or `_exit`s on error) exactly as it would without the closure. |
//! | 7 | `isolated_worker::macos::ChildBootstrap::{place, fail, run}` | `BorrowedFd::borrow_raw`, `OwnedFd::from_raw_fd`, `rustix::io::close` | `dup2` needs an `OwnedFd` for the *target* slot and a borrow of the source; the sweep closes raw numbers that no Rust value owns in the child. | The source numbers are duplicates the parent created for this child at or above `FIRST_TEMPORARY_FD` and keeps open until after the spawn. The target `OwnedFd` is released with `into_raw_fd` and never dropped, so it never closes the slot it merely names. The sweep runs in a child that owns none of the swept descriptors (every `OwnedFd` lives in the parent) and `rustix::io::close` ignores `EBADF`. |
//! | 8 | `isolated_worker::macos::ExitWatch::{new, wait_exit}` | `rustix::event::kqueue::kevent` | Observing the confined child's exit with a deadline; macOS has no pidfd. | `kevent` is `unsafe` only because a registered *descriptor* must outlive the queue; the one registration here is an `EVFILT_PROC` on a pid, so that condition is vacuous. The change and event buffers are stack locals of the calling frame. |
//!
//! There is no `unsafe impl`, no transmute, and no unsafe trait method
//! anywhere in the crate. The only raw pointers are the ones handed to the
//! kernel at sites 3-5, all pointing at locals of the frame that makes the
//! call; the macOS sites 6-8 pass descriptors and pids only.

pub mod atomic_directory;
pub mod isolated_worker;
pub mod media_source;
pub mod stability;

mod sys;

pub use atomic_directory::{AtomicDirectoryError, PublishOutcome, StagingDirectory};
pub use isolated_worker::{
    IsolatedWorker, IsolatedWorkerCancellationSource, IsolatedWorkerCancellationToken,
    IsolatedWorkerError, IsolatedWorkerExitKind, IsolatedWorkerService, launch_isolated_worker,
};
pub use media_source::{MediaSource, MediaSourceError};
pub use stability::{SourceFingerprint, SourceStabilityError, verify_complete_source_stability};
