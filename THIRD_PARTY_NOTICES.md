# Third-party software

The project is implemented entirely in Rust. The earlier C++ tree and its
fetched C/C++ dependencies (libudfread, Unshield, zlib, and the P4 graphics
dependency facade's SDL/Vulkan-Headers/volk/MoltenVK acquisitions) were
removed along with that tree once the Rust port reached M1 parity; none of
those licenses apply to the current build.

## Rust crates

The Rust workspace (`Cargo.toml`, `crates/`, `xtask/`; see
`.plan/rust-architecture-r1.md`) depends only on crates.io packages; it links
no C libraries and defines no FFI boundary. `Cargo.lock` is the exact,
reproducible inventory of every dependency (direct and transitive) actually
built; `cargo deny check licenses` (configured in `deny.toml`) enforces the
license allow list in CI and is the authoritative gate against copyleft
licenses (GPL/LGPL/AGPL are denied by omission).

Direct dependencies at R2 (M0-rs), each MIT or MIT/Apache-2.0 dual-licensed:

- `thiserror` (MIT OR Apache-2.0) -- sanitized diagnostic derive, `ohl-core`
- `sha2` (MIT OR Apache-2.0) -- SHA-256, `ohl-core`
- `clap` (MIT OR Apache-2.0) -- CLI argument parsing, `ohl-app`
- `tracing` / `tracing-subscriber` (MIT) -- structured logging, `ohl-app`
- `toml` (MIT OR Apache-2.0) -- `Cargo.toml` parsing, `xtask`
- `tempfile` (MIT OR Apache-2.0) -- temporary directories in `xtask` tests

Added at R3.2 by the media readers, each MIT licensed and pinned at exact
`2.3.0`:

- `hadris-iso` (MIT) -- ECMA-119 / Joliet directory decoding, `ohl-iso9660`
- `hadris-udf` (MIT) -- ECMA-167 descriptor and directory decoding, `ohl-udf`

Both are built with `default-features = false` and only `alloc`, `sync` and
(for `hadris-iso`) `joliet`, so neither pulls in `std`. They bring the
transitive MIT-licensed `hadris-common`, `hadris-fixed`, `hadris-io`,
`hadris-macros`, `hadris-part` and `hadris-path` crates plus `bitflags`,
`bytemuck`, `embedded-io`, `endian-num`, `spin` and `zerocopy`
(MIT OR Apache-2.0). `proptest` (MIT OR Apache-2.0) is a dev dependency only.

Added at R3.3 by the fingerprint/`ValidatedMedia`/provenance-cache crate
(`ohl-media`), each permissively licensed:

- `directories` (MIT OR Apache-2.0) -- per-user cache directory resolution
- `fs4` (MIT OR Apache-2.0) -- advisory file locking during cache publication
- `tempfile` (MIT OR Apache-2.0) -- staging files during atomic publication
- `serde` / `serde_json` (MIT OR Apache-2.0) -- the metadata-only manifest
  codec

Added by the M3 "first light" renderer, each pinned exactly in the depending
crate's `Cargo.toml` (see `docs/RENDER_DEPENDENCIES.md` for the full table and
the backend policy):

- `wgpu` `=30.0.1` (MIT OR Apache-2.0) -- graphics API, `ohl-render`
- `pollster` `=1.0.1` (Apache-2.0 OR MIT) -- blocking executor for wgpu's
  futures, `ohl-render`
- `winit` `=0.30.13` (**Apache-2.0 only**, not dual-licensed) -- windowing and
  input, `ohl-app`, and only under the non-default `dev-tools` feature

Added by the M5 audio package (`crates/ohl-audio`), pinned exactly in its
`Cargo.toml` (see `docs/RENDER_DEPENDENCIES.md`, "Audio backend", for the full
write-up and the Linux linking decision):

- `hound` `=3.5.1` (Apache-2.0) -- writes synthetic WAV fixtures for this
  crate's own tests only; WAV *decoding* is this crate's own bounded,
  zero-copy chunk walker, `ohl-audio`, dev-dependency only
- `cpal` `=0.18.2` (Apache-2.0) -- output device backend, `ohl-audio`, and
  only ever a dependency on macOS/Windows (`[target.'cfg(...)']`): its only
  Linux backend links `libasound` through `alsa-sys`'s build-time
  `pkg-config` lookup, a genuine link-time C-library dependency the "No FFI"
  rule forbids, so it is never even resolved for a Linux build. On
  macOS/Windows it pulls in the transitive MIT/Apache-2.0-family crates
  `coreaudio-rs`, `objc2` (and its `objc2-*` framework-binding crates),
  `dispatch2`, `block2`, `mach2`, and the official `windows` crate family
  (all MIT OR Apache-2.0), reaching CoreAudio and WASAPI directly rather than
  vendoring or linking a third-party C library

As later packages add the freestanding parser worker's remaining pieces and
the input stack, this section will be extended; `cargo deny check` remains
the enforced, up-to-date source of truth between updates to this file.

Added by the M4 "movement" physics crate, pinned exactly in
`crates/ohl-physics/Cargo.toml`:

- `glam` `=0.33.6` (MIT OR Apache-2.0) -- vector math, `ohl-physics`
- `libm` `=0.2.16` (MIT) -- the scalar trigonometry `core` does not provide,
  and the same implementation `glam`'s `libm` feature uses, `ohl-physics`

Both are built with `default-features = false`, so neither pulls in `std`.

As later packages add the freestanding parser worker (`rustix`, `seccompiler`,
`landlock`) and the audio/input stack (`cpal`/`rodio`), this section will be
extended; `cargo deny check` remains the enforced, up-to-date source of truth
between updates to this file.

## `ohl-cabinet-format` and `ohl-cabinet` (Rust translation of Unshield)

The two Rust crates `crates/ohl-cabinet-format` and `crates/ohl-cabinet` are a
**licensed derivative of Unshield 1.6.2** (commit
`51de441ba6893f11026d4671ccef9e8e2a4634fa`), not clean-room work. Their
knowledge of the InstallShield 5/6/2003 cabinet container -- the common header
and version encodings, the cabinet descriptor and its offset tables, file,
component, file-group and directory descriptor layouts, the volume headers, the
length-prefixed raw DEFLATE chunk framing, the obfuscation keystream, and the
split-volume rules -- was obtained by translating Unshield's C implementation
into Rust. Unshield is copyright David Eriksson and contributors and is
licensed under the MIT license; its full text, together with the required
"portions translated from Unshield (c) David Eriksson" attribution, is kept in
`crates/ohl-cabinet-format/LICENSE-UNSHIELD` and
`crates/ohl-cabinet/LICENSE-UNSHIELD`, and is restated in each crate's
crate-level documentation.

Constraints on these two crates, enforced by the `xtask graph` edge table and
required by `docs/CLEAN_ROOM.md`:

- They are leaves of the dependency graph. Only the sandboxed parser worker
  may link them; no other crate may depend on them.
- Their format knowledge must never be used as a source for, copied into,
  restated in, or cited by project-owned parsing code, documentation, tests or
  fixtures. The finding recorded in `docs/FORMAT_SOURCES.md` -- that
  project-owned cabinet or component-selection parsing **may not begin**
  without an independently authored public specification -- is unchanged.
- They link no C library. The translation is `#![no_std]` plus `alloc` and
  `#![forbid(unsafe_code)]`, replaces Unshield's unchecked pointer arithmetic
  and fixed-size buffers with validated offsets and caller-supplied limits,
  never opens a path or builds a filename, and uses the Rust crates
  `miniz_oxide` (raw DEFLATE, MIT) and optionally `md-5` (RustCrypto,
  MIT OR Apache-2.0) instead of zlib and the RSA MD5 reference code.

## `ohl-isz` (PKWARE DCL code tables from `blast`)

`crates/ohl-isz` is a clean-room decoder for InstallShield 3 "Z" archives and
for the PKWARE Data Compression Library "implode" streams they contain,
written from the public documents recorded in
[`docs/FORMAT_SOURCES.md`](docs/FORMAT_SOURCES.md). It is **not** a
translation of any implementation and it links no third-party crate other
than the workspace's own `ohl-core`.

One artefact is derived from a licensed source and is attributed here for
transparency: the format's three fixed Huffman codebooks (the bit lengths of
the 256 literal, 16 length and 64 distance codes) together with the length
base and extra-bit tables. Those are constants defined by the format itself
rather than authored code. They were read from the format description
distributed with Mark Adler's `blast` (`contrib/blast/blast.c`, version 1.3,
24 August 2013, zlib licence, <https://github.com/madler/zlib>), which in turn
credits Ben Rudiak-Gould's 2001 `comp.compression` description as the primary
public specification. The tables are restated in a different form and their
completeness is verified by a unit test. The zlib licence text is reproduced
in `crates/ohl-isz/LICENSE-BLAST`.

There is no C zlib dependency in the Rust build: `miniz_oxide` (MIT OR
Apache-2.0 OR Zlib), a pure-Rust reimplementation, replaces it for
decompression. The C++ tree's separately fetched libudfread (LGPL-2.1-or-later),
its default-off experimental Unshield-linked adapter, zlib, and the P4
graphics dependency facade's SDL/Vulkan-Headers/volk/MoltenVK acquisitions
were removed along with that tree; none of those licenses apply to the
current build.
