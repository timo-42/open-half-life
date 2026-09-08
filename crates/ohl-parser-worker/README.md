# ohl-parser-worker

Host-side support for the media-parser worker image, plus the image itself in
[`image/`](image). The image has two shapes, selected by target: the
freestanding Linux x86-64 one (`image/src/freestanding.rs`) and the hosted
macOS one (`image/src/hosted.rs`).

Both shapes host the compile-fixed `ohl_parser_backends::ContainerDispatcher`,
which decodes Wise, MS-CAB and IS3 Z containers for real. Media cannot select
or configure it. Production import remains unqualified on every platform, and
on macOS no medium has been imported at all; see
[docs/IMPORT_READINESS.md](../../docs/IMPORT_READINESS.md).

## Install location

`ohl-platform`'s isolated-worker backends resolve the media-parser image at

```
<directory of the executable that launches it>/libexec/open-half-life/ohl-media-parser-worker
```

walking one `O_NOFOLLOW` component at a time and refusing any group- or
world-writable directory, any symlink, and any file that is not a read-only,
non-set-id executable. The format check is the backend's own: on Linux a
statically linked `ET_EXEC` x86-64 ELF without `PT_INTERP` or `PT_DYNAMIC`;
on macOS a thin 64-bit `MH_EXECUTE` Mach-O for the host CPU that names
`/usr/lib/dyld` as its only dynamic linker, `/usr/lib/libSystem.B.dylib` as
its only dynamic library, and no `LC_RPATH`.

`cargo xtask worker-image` builds, audits and installs exactly that layout
next to the xtask binary's own build profile
(`target/<profile>/libexec/open-half-life/`).
`install_parser_worker_image` writes it for any other executable directory
and forces the two created directories to mode `0o755`.

## Build configuration

`image/` is a standalone (non-member) Cargo package. It is not a workspace
member because the Linux `#![no_std] #![no_main]` shape needs
`panic = "abort"`, which Cargo will not scope to one package inside a
workspace, and because it needs a package-local `unsafe_code = "allow"`
against the workspace-wide `forbid`.

`main.rs` selects the shape by target and carries the conditional crate
attributes, so one package and one `Cargo.lock` serve both. Its `build.rs`
emits `cargo::rustc-link-arg-bins` for
`-nostdlib -static -no-pie -Wl,-e,_start -Wl,--build-id=none` using the
default `cc` linker driver, **for the Linux x86-64 target only**; the hosted
macOS shape links the ordinary way. No global `RUSTFLAGS` and no
`.cargo/config.toml` is involved, so `cargo build --workspace` on Linux,
macOS and Windows never touches this package; only
`build_parser_worker_image` and `cargo xtask worker-image` do.

`strip = "debuginfo"` keeps `.symtab`, which is what lets
`cargo xtask worker-image` prove the Linux image has no undefined symbol and
names none of `open`, `openat`, `ioctl`, `socket`, `mmap`, `brk`. Those
symbol audits are Linux-only: the hosted macOS image links libSystem by
design, and its Mach-O identity check already proves it links nothing
else.

## Exit statuses

They mirror the C++ worker (`src/platform/src/media_parser_worker_linux.cpp`)
and are defined once in [`src/contract.rs`](src/contract.rs), which is also
`include!`d by the freestanding image:

| status | meaning |
| --- | --- |
| `0` | orderly `shutdown`, or an orderly peer close |
| `64` | protocol failure |
| `65` | the dispatcher refused the request (`unsupported`) |
| `66` | transport failure |
| `70` | anything else, including a panic and a rejected configuration |

`ohl-platform` deliberately reduces a child's status to `Clean` / `Failed` /
`Crashed` / ..., so the integration test asserts that vocabulary; the numeric
values above are the contract for any future consumer that observes the raw
status.

## Syscalls

The freestanding Linux image issues `read`, `write`, `close`, `ppoll` and
`exit_group` only, which is a subset of the backend's seccomp allowlist. It
parses no arguments, reads no environment, and opens no file.

Two deviations from the C++ worker follow from that allowlist: `SIGPIPE` is
not ignored (`rt_sigaction` is not allowed), so a parent that vanishes
mid-write ends the worker with a signal rather than an exit status; and input
is probed with a zero-timeout `ppoll` instead of
`recvfrom(MSG_PEEK | MSG_DONTWAIT)`.

The hosted macOS image has no syscall allowlist to satisfy (Seatbelt confines
resources, not syscall numbers), and it also parses no arguments, reads no
environment and opens no file. Its two deviations run the other way:
`SIGPIPE` *is* ignored, as in every Rust `std` binary, so a vanished parent
produces a transport failure rather than a signalled exit; and input is
probed with a non-blocking one-byte read into a private pushback slot that
the next `read_exact` drains first, because `std` exposes no stable
`MSG_PEEK`. Instead of a fixed `.bss` arena it bounds its live heap with a
counting global allocator, because XNU does not enforce
`RLIMIT_AS`/`RLIMIT_DATA` against `mmap`-backed allocations.
