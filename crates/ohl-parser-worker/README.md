# ohl-parser-worker

Host-side support for the media-parser worker image, plus the image itself in
[`image/`](image). Linux x86-64 and macOS share the hosted `std`
implementation in [`image/src/hosted.rs`](image/src/hosted.rs).

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
member because it needs `panic = "abort"`, which Cargo will not scope to one
package inside a workspace, and because it needs a package-local
`unsafe_code = "allow"` to adopt its two inherited descriptors and install
its bounded allocator against the workspace-wide `forbid`.

`main.rs` selects the hosted implementation for both supported targets, so
one package and one `Cargo.lock` serve both. On Linux the builder explicitly
uses `x86_64-unknown-linux-musl`, scopes `-C relocation-model=static` to its
nested Cargo invocation, and `build.rs` emits
`cargo::rustc-link-arg-bins` for `-static -no-pie -Wl,--build-id=none`.
Those settings produce the required static non-PIE `ET_EXEC` without an
interpreter or dynamic segment. macOS links the ordinary way. No global
`RUSTFLAGS` and no `.cargo/config.toml` is involved, so `cargo build
--workspace` on Linux, macOS and Windows never touches this package; only
`build_parser_worker_image` and `cargo xtask worker-image` do.

`strip = "debuginfo"` keeps `.symtab`, which lets `cargo xtask worker-image`
audit the Linux image's linked runtime. The hosted musl image defines libc
and allocator symbols by design; required unresolved symbols are rejected,
while optional weak hooks are permitted. Those symbol audits are Linux-only:
the hosted macOS image links libSystem by design, and its Mach-O identity
check already proves it links nothing else.

## Exit statuses

They mirror the C++ worker (`src/platform/src/media_parser_worker_linux.cpp`)
and are defined once in [`src/contract.rs`](src/contract.rs), which is also
`include!`d by the hosted image:

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

## Hosted transport

The hosted image parses no arguments, reads no environment and opens no file.
It adopts the pre-opened channel and readiness descriptors, and probes input
with a non-blocking one-byte read into a private pushback slot that the next
`read_exact` drains first, because `std` exposes no stable `MSG_PEEK`.
`SIGPIPE` is ignored as in Rust `std` binaries, so a parent that vanishes
mid-write becomes a transport failure. A counting system allocator bounds
the live heap at 128 MiB; this supplies the ceiling that XNU cannot enforce
with `RLIMIT_AS` or `RLIMIT_DATA`.
