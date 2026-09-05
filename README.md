# Open Half-Life

Open Half-Life is a clean-room, cross-platform reimplementation of the
original Half-Life single-player runtime. It is at an early development
stage and cannot run the game yet.

The project does not include game data. You must own compatible Half-Life
media and provide it separately. Do not submit game assets, extracted files,
installer binaries, or information obtained by decompiling the original
engine.

## Build

The project is implemented entirely in Rust (Rust 2024 edition, no FFI, no
linked C libraries), as a Cargo workspace under `crates/`. There is no CMake
or C++ build; the earlier C++ tree was removed once the Rust port reached M1
parity (see [docs/MILESTONES.md](docs/MILESTONES.md)).

Requirements: a stable Rust toolchain matching `rust-toolchain.toml` (installed
automatically by `rustup` on first use) plus the `clippy` and `rustfmt`
components.

```sh
cargo build --workspace
cargo test --workspace
cargo xtask policy
cargo xtask graph
```

Run the composition-root binary:

```sh
cargo run -p ohl-app -- --iso /path/to/owned-media.iso
```

or `cargo run -p ohl-app -- /path/to/owned-media.iso` (positional form), or
with no path at all, which prompts for one on stdin, as the previous C++
build did. `--cache /absolute/path` overrides the platform per-user cache
location the metadata-only provenance record is published under; the default
is the platform's standard cache directory. `--version` reports the binary's
version and exits; `cargo run -p ohl-app -- --version` or the built binary's
`--version` both work.

That form imports the medium's payload and exits. To play it afterwards, add
`--play` (or any of the flags below, which imply it):

```sh
cargo run --release -p ohl-app -- --play --payload-root /path/to/payload
```

The engine locates the published payload (through the medium's provenance
entry when `--iso` is given, importing first if nothing is published yet, and
otherwise resolving the single published tree under `--payload-root`), mounts
its assets, and starts on the campaign's documented start map. `--map NAME`
picks a different map and `--training` starts the hazard course instead.
In the window, WASD moves, the mouse looks, `E` uses the nearest door or
button, the backquote key opens the console, and Escape quits.

On a machine with no display server, render offscreen instead:

```sh
cargo run --release -p ohl-app -- \
  --payload-root /path/to/payload \
  --headless-screenshot /path/to/shot.png --frames 30
```

That advances `--frames` frames and writes one 1280x720 PNG, then exits 0.
`--viewpoint X,Y,Z,PITCH,YAW` captures from an explicit position and
`--spawn-offset DX,DY,DZ,DPITCH,DYAW` from one relative to the map's player
start.

The import path fingerprints the validated ISO 9660/Joliet or UDF image,
mounts it read-only, and publishes (or reuses) a metadata-only provenance
record alongside the extracted payload. See
[docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the current
production-readiness matrix and release-evidence gates.

## Status

M0 (build and logging foundation) and M1 (media preflight, mount, and
provenance cache) parity has been achieved in Rust; the earlier C++
implementation has been removed. M2 (parser worker, cabinet/staging pipeline)
is in progress: the OWP/1 protocol, media archive traits, the ISO 9660/UDF
readers, the VFS mount facade, and the fingerprint/cache crates have already
landed. M3 (wgpu/winit first light) is in progress, and its playable loop now runs a
map out of an imported payload (see M3.3 in the milestones). See
[docs/MILESTONES.md](docs/MILESTONES.md) for current progress,
[docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for production import
readiness, and [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) before contributing
compatibility work.

Run with `cargo run -p ohl-app -- --iso /path/to/owned-media.iso`; no
installer or media binary is executed.

Half-Life is a trademark of Valve Corporation. This independent project is
not affiliated with or endorsed by Valve Corporation.

Repository-authored code is MIT licensed; adopted Rust dependencies use
permissive licenses (MIT, Apache-2.0, BSD, Zlib, Unicode-3.0). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
