# ohl-parser-worker

Host-side support for the freestanding Linux x86-64 media-parser worker
image, plus the image itself in [`image/`](image).

Production media import is still unavailable: the shipped image hosts the
compile-fixed `UnsupportedDispatcher`, so every `enumerate` and
`stream_entry` is answered with `unsupported` and no parser runs.

## Install location

`ohl-platform`'s isolated-worker backend resolves the media-parser image at

```
<directory of the executable that launches it>/libexec/open-half-life/ohl-media-parser-worker
```

walking one `O_NOFOLLOW` component at a time and refusing any group- or
world-writable directory, any symlink, and any file that is not a read-only,
non-set-id, statically linked `ET_EXEC` x86-64 ELF without `PT_INTERP` or
`PT_DYNAMIC`.

`cargo xtask worker-image` builds, audits and installs exactly that layout
next to the xtask binary's own build profile
(`target/<profile>/libexec/open-half-life/`).
`install_parser_worker_image` writes it for any other executable directory
and forces the two created directories to mode `0o755`.

## Build configuration

`image/` is a standalone (non-member) Cargo package. It is not a workspace
member because a `#![no_std] #![no_main]` binary needs `panic = "abort"`,
which Cargo will not scope to one package inside a workspace, and because it
needs a package-local `unsafe_code = "allow"` against the workspace-wide
`forbid`.

Its `build.rs` emits `cargo::rustc-link-arg-bins` for
`-nostdlib -static -no-pie -Wl,-e,_start -Wl,--build-id=none` using the
default `cc` linker driver. No global `RUSTFLAGS` and no `.cargo/config.toml`
is involved, so `cargo build --workspace` on Linux, macOS and Windows never
touches this package; only `build_parser_worker_image` and
`cargo xtask worker-image` do.

`strip = "debuginfo"` keeps `.symtab`, which is what lets
`cargo xtask worker-image` prove the image has no undefined symbol and names
none of `open`, `openat`, `ioctl`, `socket`, `mmap`, `brk`.

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

The image issues `read`, `write`, `close`, `ppoll` and `exit_group` only,
which is a subset of the backend's seccomp allowlist. It parses no arguments,
reads no environment, and opens no file.

Two deviations from the C++ worker follow from that allowlist: `SIGPIPE` is
not ignored (`rt_sigaction` is not allowed), so a parent that vanishes
mid-write ends the worker with a signal rather than an exit status; and input
is probed with a zero-timeout `ppoll` instead of
`recvfrom(MSG_PEEK | MSG_DONTWAIT)`.
