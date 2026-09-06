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

## Running

These are the exact commands for each step; see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces underneath
them fit together and [docs/MEDIA_IMPORT.md](docs/MEDIA_IMPORT.md) /
[docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the import path's
design and current readiness.

**Import your own media** from an ISO, publishing the payload under an
explicit cache and payload root instead of the platform defaults:

```sh
cargo run --release -p ohl-app -- \
  --iso /path/to/owned-media.iso --cache /path/to/cache --payload-root /path/to/payload
```

**Play** an already-imported payload:

```sh
cargo run --release -p ohl-app -- --play --payload-root /path/to/payload
```

`--map NAME` picks a map by its bare name (default: the campaign's
documented start map); `--training` starts the Hazard Course instead;
`--load SLOT` resumes a save slot rather than starting a map fresh (mutually
exclusive with `--map`/`--training`); `--difficulty easy|medium|hard`
selects which `skill.cfg` values the game reads (default `medium`); and
`--overbright MULTIPLIER` scales the lightmap ramp; the app defaults it to
`1.7`, a project display default calibrated against public reference
screenshots (fidelity round 5), not a claimed engine fact — the engine's own
`LightRamp`/`GameConfig` defaults stay at the raw, unmultiplied `1.0`. Pass
`--overbright 1.0` to get that raw lighting back. See `--help` for the full
fidelity-investigation background, including GoldSrc's overbright convention
shipping disabled by default with no public source pinning a specific
non-default value.

**Headless screenshots**, for a machine with no display server (a GPU
adapter is still required):

```sh
cargo run --release -p ohl-app -- \
  --payload-root /path/to/payload \
  --headless-screenshot /path/to/shot.png --frames 30
```

`--frames N` advances the simulation a fixed step N times before the
1280x720 PNG is written; `--spawn-offset DX,DY,DZ,DPITCH,DYAW` captures from
a pose relative to the map's player start instead of standing exactly on it.
On a machine with no real GPU, `OHL_RENDER_GPU_TEST=1` opts into exercising
this path against a software Vulkan implementation (for example
`lavapipe`/`llvmpipe`) instead of skipping it; see
[docs/RENDER_DEPENDENCIES.md](docs/RENDER_DEPENDENCIES.md).

**Scripted input**, for deterministic automated runs (see `crate::script`'s
grammar in `crates/ohl-app/src/script.rs`):

```sh
cargo run --release -p ohl-app -- \
  --payload-root /path/to/payload --script /path/to/script.txt --script-log
```

Usable with or without `--headless-screenshot`; without one, the scripted
ticks still run headlessly with no GPU needed at all. `--script-log`
enables the fixed scripted-sequence milestone log lines.

By default, a `trigger_changelevel` fired during a headless or scripted
run is not followed: the run logs a fixed line and keeps rendering the
map it started on. Add `--follow-level-change` to instead call the same
level-change path the interactive window uses and keep ticking on the
destination map — useful for a capture or script that needs to land on
whatever map a level transition leads to.

Development-only builds (`--features dev-tools`) add
`--viewpoint-at-nearest-monster DISTANCE`, which places the headless
capture eye `DISTANCE` units from whichever spawned monster is nearest to
the map's player start, at its eye height, facing it, in noclip, instead
of at the player start or a caller-chosen `--viewpoint`/`--spawn-offset`:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c1a1 \
  --headless-screenshot /path/to/shot.png --frames 60 \
  --viewpoint-at-nearest-monster 160
```

**Smoke tests**, each of which builds (or accepts a prebuilt)
`open-half-life` and drives it against an already-imported payload:

```sh
cargo xtask campaign-smoke --payload-root /path/to/payload   # every campaign map, headless-screenshotted
cargo xtask combat-smoke --payload-root /path/to/payload     # every xtask/smoke-scenarios/*.txt scripted scenario
```

**Other `cargo xtask` subcommands:**

```sh
cargo xtask dist      # builds the release binary/worker image and packages a versioned archive
cargo xtask policy    # tracked-file policy check (private paths, extensions, size, magic bytes)
cargo xtask graph     # validates the crate dependency graph against xtask/src/graph.rs's ALLOWED_EDGES
```

### Platform notes

- **Audio**: on Linux, `ohl-audio` always uses a null output sink today —
  `cpal`'s only Linux backend links `libasound` through a build-time
  `pkg-config` lookup, which the project's "No FFI" rule forbids as
  written, so there is currently no real Linux audio backend (decision
  still open; see `docs/MILESTONES.md`, "Status as of 2026-09-07"). On
  macOS and Windows, `cpal` reaches the OS's own audio API (CoreAudio,
  WASAPI) with no such concern.
- **Linux import worker sandbox**: the isolated media-parser worker's
  native containment backend (resource limits, no-new-privileges,
  Landlock, seccomp, pidfd-backed lifecycle) is implemented and qualified
  only for Linux x86-64; every other platform/architecture tuple selects
  an unsupported backend, so import cannot begin there yet. See
  [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the exact gates.

## Release builds

`cargo xtask dist` builds the release binary (and, on a Linux x86-64 host
targeting Linux, the sandboxed media-parser worker image alongside it), then
assembles a versioned, self-contained release folder under
`target/dist/open-half-life-<version>-<target-triple>/`:

```text
bin/open-half-life[.exe]
libexec/open-half-life/ohl-media-parser-worker   (Linux only)
LICENSE
THIRD_PARTY_NOTICES.md
licenses/                                        (every dependency's declared license)
README-dist.md
SHA256SUMS
```

and archives it as a `.tar.gz` (Linux/macOS) or `.zip` (Windows) next to it,
by default under `target/dist/` (pass `--out-dir <DIR>` for another
location; it is created if missing and refused if it would resolve under
this workspace's own `assets/`, `cache/`, or `imported/` directories).
`OHL_VERSION` (falling back to `CARGO_PKG_VERSION`) selects the version, the
same as the binary's own `--version` output. Pass `--target <triple>` to
package for another target (`--print-target` prints the triple an
otherwise-identical invocation would resolve to, without building or
packaging anything); cross-compiling the binary itself is best effort (it
depends on your toolchain having that target installed) and the worker
image, being Linux x86-64-only, is only ever bundled for a Linux x86-64
host building for Linux. `cargo xtask dist --help` documents every flag.
No game data is ever included in the archive. Release binaries are built
with `[profile.release]` `strip = "symbols"`, `codegen-units = 1`, and thin
LTO (see the root `Cargo.toml`) for a meaningfully smaller download; this
does not change `panic` behaviour.

## Releases

Pushing an annotated tag matching `v*` (e.g. `git tag -a v0.1.0 -m "Open
Half-Life v0.1.0" && git push origin v0.1.0`) triggers CI's `release` job
on Linux, Windows and macOS runners, each running `cargo xtask dist`
natively for its own host triple, then a `publish-release` job that
downloads all three archives, writes a top-level `SHA256SUMS` covering
them, and creates (or updates, if the tag's release already exists) a
GitHub Release named after the tag with those three archives plus
`SHA256SUMS` attached as assets. Release notes are generated from this
file's own `docs/MILESTONES.md`, "Status as of" section — no text derived
from game media ever appears there. A manual `workflow_dispatch` run of
the same workflow still builds and uploads workflow artifacts for
inspection, but never touches GitHub Releases (there is no tag to attach
one to). See the `release` and `publish-release` jobs in
`.github/workflows/build.yml` for the exact mechanics.

## Status

Plainly, for a new reader: this project imports its own copy of a lawfully
owned game's payload and renders it, but nobody has yet sat down and played
it interactively on a real screen.

**What works:**

- **Import**, on Linux x86-64, from one real ISO layout: the medium is
  fingerprinted, mounted read-only, its Wise/MS-CAB/InstallShield-3-Z
  payload is parsed by a sandboxed worker process, and the result is
  published as a metadata-only provenance record plus an extracted payload
  tree. See [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the
  production-readiness gates that are *not* yet met on any platform, Linux
  included — only one ISO layout has been exercised, and no other platform
  tuple can extract a medium at all yet.
- **All 93 campaign maps** (18 story chapters plus the Hazard Course) load
  and render successfully headless (`cargo xtask campaign-smoke`), with
  monsters, props and sprites rendering alongside the world geometry.
- **Movement and collision**: the player collides against both worldspawn
  geometry and solid brush entities (doors, moving platforms, and the
  like), not only the static world.
- **Interactive map logic**: touch triggers (fired by the player's own
  movement, not only `use`), doors, buttons, `func_train`/`func_tracktrain`
  track trains, `trigger_camera` view sequences, scripted
  sequences/talk monsters, and level transitions by touch
  (`trigger_changelevel`) or by `use`.
- **Combat and AI**: weapons, pickups, damage, monster AI, and navigation
  are implemented and exercised by a scripted combat scenario that picks up
  and fires a weapon (`cargo xtask combat-smoke`).
- **Save/load** works over the project-owned `ohl-save` container (not the
  GoldSrc `.sav` format), with typed sections covering the engine header,
  entity registry, map-logic simulation, global state, light-style time,
  camera/player pose, and — as of M7.9 P4b — inventory, entity combat
  state, AI state, projectiles/deployables and the RNG stream.
- **Scripted-input smokes**: a project-owned deterministic script format
  drives headless runs for both the campaign and combat scenarios above,
  with fixed milestone log lines asserted in CI.
- **Release packaging**: `cargo xtask dist` builds a stripped, versioned
  release archive for Linux, Windows and macOS, and a pushed `v*` tag
  publishes it as a GitHub Release (see "Releases" below).

**What is not verified yet:**

- No interactive play-test end to end on a real display by a person —
  combat, AI, navigation, movers and scripted sequences are exercised only
  by automated unit/integration/property tests and the headless smokes
  above.
- Audio on Linux is a null sink: `cpal`'s only Linux backend links
  `libasound` through a build-time `pkg-config` lookup, which this
  project's "no FFI" rule forbids as written, so there is no real Linux
  audio output today (macOS and Windows are unaffected). See "Platform
  notes" above and `docs/MILESTONES.md`'s "Linux audio backend decision"
  follow-up.
- No real-display input test: keyboard/mouse input, the window loop, and
  rendering have been exercised offscreen (headless screenshots, scripted
  input) but not against a real display server and real hardware input.

**Known gaps** (see `docs/MILESTONES.md`'s "Status as of" sections for the
full list): `func_tracktrain` `altpath` branching is recorded but not
applied; a `monstermaker` cannot yet be toggled at runtime by its own
`targetname`; a `scripted_sequence` target's pre-trigger idle animation
(`m_iszIdle`) is not yet modelled; and weapon inventory still rides inside
`SECTION_PLAYER_CARRY`'s ad hoc encoding rather than its own save section.

**Fidelity note:** scene lighting is calibrated against public reference
screenshots via the app's own `--overbright 1.7` default (pass
`--overbright 1.0` for the engine's raw, unmultiplied lighting; see
`--help` and `docs/FORMAT_SOURCES.md`, "Rendering conventions"); this is a
project display default, not a claimed engine fact. The Hazard Course
training spawn view has not been verified against a reference.

See [docs/MILESTONES.md](docs/MILESTONES.md) for the full package-by-package
history and its closing "Status as of" summaries, and
[docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) before contributing compatibility
work.

M0-M1 (build/logging foundation, media preflight/mount/provenance cache)
are complete in Rust; the earlier C++ implementation has been removed. M2
(import pipeline) is functionally complete on Linux x86-64 and still
tracked for the remaining platform tuples and release-evidence gates. M3-M9
(rendering, movement, entities, models/animation, combat/AI, campaign
save/load, UI shell, packaging, fuzz targets) are each in progress or done
per crate; see the milestones file for exactly which package covers which
slice.

Run with `cargo run -p ohl-app -- --iso /path/to/owned-media.iso`; no
installer or media binary is executed.

Half-Life is a trademark of Valve Corporation. This independent project is
not affiliated with or endorsed by Valve Corporation.

Repository-authored code is MIT licensed; adopted Rust dependencies use
permissive licenses (MIT, Apache-2.0, BSD, Zlib, Unicode-3.0). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
