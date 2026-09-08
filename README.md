# Open Half-Life

Open Half-Life is a clean-room, cross-platform reimplementation of the
original Half-Life single-player runtime. It is at an early development
stage and cannot run the game yet.

The project does not include game data. You must own compatible Half-Life
media and provide it separately. Do not submit game assets, extracted files,
installer binaries, or information obtained by decompiling the original
engine.

## Build

The project is implemented in Rust (Rust 2024 edition), as a Cargo workspace
under `crates/`. The Linux parser worker uses Rust's standard library with
a statically linked musl runtime. There is no CMake
or C++ build; the earlier C++ tree was removed once the Rust port reached M1
parity (see [docs/MILESTONES.md](docs/MILESTONES.md)).

Requirements: a stable Rust toolchain matching `rust-toolchain.toml` (installed
automatically by `rustup` on first use) plus the `clippy` and `rustfmt`
components.

On Linux x86-64, install the worker's static-runtime build prerequisite
before running the tests or building an import worker:

```sh
rustup target add x86_64-unknown-linux-musl
```

`musl-tools` (for a system `musl-gcc`) is only needed if your Rust toolchain
lacks the self-contained musl target; recent `rustup`-installed toolchains
bundle it and link the worker without any extra package.

```sh
cargo build --workspace
cargo test --workspace
cargo xtask policy
cargo xtask graph
```

Payload import needs the parser worker installed beside the application.
On Linux x86-64 and macOS, build, audit and install it for the debug profile,
then run the composition-root binary:

```sh
cargo xtask worker-image
cargo run -p ohl-app -- --iso /path/to/owned-media.iso
```

For a release application, install beside the release binary instead:

```sh
cargo run --release -p xtask -- worker-image
```

Repeat the matching worker-image command after updating the parser code.
`cargo build --workspace` does not install the standalone worker image.
Packaged releases produced by `cargo xtask dist` include it on supported
hosts. Other platforms currently have no native import worker.

For Linux-hosted builds targeting Windows x86-64 or Apple Silicon macOS,
see [cross-build setup and commands](docs/CROSS_BUILD.md). The macOS target
is `aarch64-apple-darwin`; no Intel macOS cross-build is produced.

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
`--viewpoint X,Y,Z,PITCH,YAW` captures from an explicit, fixed world-space
position and `--spawn-offset DX,DY,DZ,DPITCH,DYAW` rides along with the
player instead: the render eye is the player's own current position plus
this offset, recomputed every frame, so it keeps following the player
(including riding a mover) rather than freezing in place.

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
cargo run --release -p xtask -- worker-image
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
a pose relative to the player instead of standing exactly on it.
Unlike `--viewpoint` (an explicit, fixed world-space pose applied once,
under noclip), `--spawn-offset` never enables noclip: the player spawns
and moves normally — falling, colliding, riding a mover — and every frame
the render eye is recomputed as the player's *current* position plus this
offset, so a capture on a map with a moving `func_train`/`func_tracktrain`/
`func_plat`/lift `func_door` keeps riding along with the player instead of
being left behind in whatever geometry the mover has since vacated. Either
flag's placement is checked for landing inside solid collision geometry
both at the first frame and, since a rider offset can end up somewhere
different than where it started, at the last frame too; each check logs
its own fixed warning line at most once. On a machine with no real GPU,
`OHL_RENDER_GPU_TEST=1` opts into exercising this path against a software
Vulkan implementation (for example `lavapipe`/`llvmpipe`) instead of
skipping it; see
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

**Chained scripted routes**, for walking the campaign the way it is
actually played. Every scenario under `xtask/smoke-scenarios/` starts at
its own map's player start with an empty inventory; a real campaign
instead arrives through a level change, at an offset from the
destination's landmark, carrying whatever the earlier maps gave the
player. `--chain-script` runs a *sequence* of route files in one process:
the first from the start map's player start, and each later one from
wherever the previous route's level change put the player down, with
health, armor, weapons and ammo carried across by the engine's own
transition machinery. Give the flag once per route, in chain order:

```sh
cargo run --release -p ohl-app -- \
  --payload-root /path/to/payload --map c0a0 --script-log \
  --chain-script xtask/chain-routes/c0a0.txt \
  --chain-script xtask/chain-routes/c0a0-hop1.txt
```

A route ends at the first level change it reaches (the next route takes
over there, and the run logs the same fixed "A level change was followed."
line a `--follow-level-change` run does); when a route's ticks run out
first, the walk logs the fixed line "The chain walk stopped." and ends. A
walk that ran every route it was given logs "The chain walk has no further
route." instead. If a route's level change lands back in a map the chain
has already entered — most often a route walking straight back into the
boundary it arrived through — the walk logs "The chain walk re-entered a
map it had already visited." and ends as a failure: the reported depth
counts *distinct* maps, so a chain cannot manufacture progress by
ping-ponging across one boundary. Level changes are always followed during
a chain, so `--follow-level-change` is neither needed nor consulted;
`--chain-script` is mutually exclusive with `--script` and with the
capture-pose flags (`--headless-screenshot`, `--viewpoint`,
`--spawn-offset`), which a headless, multi-map walk has no use for.

On a `dev-tools` build, adding `--reachability-report` to a chain run
walks the reachability report *after* the chain, from wherever its last
route left the player standing — route triage from a level-change arrival
point, which a cold `--map <name>` load cannot reproduce.

Development-only builds (`--features dev-tools`) add
`--viewpoint-at-nearest-monster DISTANCE`, which places the headless
capture eye `DISTANCE` units from whichever spawned monster is nearest to
the map's player start, at its eye height, facing it, in noclip, instead
of at the player start or a caller-chosen `--viewpoint`/`--spawn-offset`.
It also combines with `--script`: the placement is applied once, right
after the map loads and before the scripted input runs, so a motion
capture can start already facing the nearest monster instead of the
ordinary player start:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c1a1 \
  --headless-screenshot /path/to/shot.png --frames 60 \
  --viewpoint-at-nearest-monster 160
```

The same `dev-tools` build also adds `--reachability-report`, a
route-triage command for level authors and regression investigations: it
loads a map headlessly (no window, no GPU) and runs a bounded,
deterministic breadth-first walk over the live collision model from the
player start, using the same standing hull and 18-unit step-up the walking
player does. From every reached cell, in every direction, the walk first
tries a plain step; a one-way fall of any height (not just the small,
conservative bound the walk originally shipped with) is a legal landing
for it, so a route that only works by dropping off a ledge is now found,
not just one that descends a stair. When the plain step fails (blocked
ascending, blocked moving across, or no floor found at all), the walk
tries a jump instead: an ascent up to the walking player's own jump apex
plus the step-up height, and a horizontal reach up to the distance that
player's run speed covers over one jump's full airtime, both computed from
this build's own `ohl_physics::MoveConfig` rather than restated — a
deliberately coarse, documented approximation of a running jump, not a
simulated arc. It prints, per round: how many 16-unit grid cells were
reached (and, of those, how many were reached only by a one-way fall
deeper than the walk's old bound, called out separately since that kind of
route needs a real fall rather than a stair step), which brush-entity
classnames sit on the unreached frontier (a count of distinct entities and
whether the engine's own use-proximity path could open one from a reached
cell), and whether a `trigger_changelevel` was reached (and its
straight-line distance from spawn, rounded to the nearest ten units).
Every closed door the walk found and could open is then simulated open for
the next round, so a route needing several doors opened in sequence is
triaged one round at a time, for up to six rounds by default (see
`--reachability-round-cap` below). A `func_pushable` on the
frontier is reported push-openable and its brush is detached for the next
round the same way a door's is — approximate (this walk does not simulate
the real push distance or direction, only that the crate is out of the way
afterward), and needing no assumed weapon, since shoving a crate needs
nothing but the player's own body. A `func_breakable` with `health > 0`
(not the documented "Only Trigger" flag, and not already broken) is
reported damage-openable, and broken the same way, but **only** when the
run is given `--reachability-assume-armed`: without it a breakable stays
on the frontier forever, because a cold map load starts with no weapon at
all, not even the crowbar (`ohl_combat::Inventory::new`), and this walk
never fabricates one — see `--start-inventory` below for giving it one for
real instead of merely assuming one. Output is deliberately sparse:
classnames, aggregate counts and rounded distances only — never a map
name, coordinate, or targetname:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c1a0 --reachability-report \
  --reachability-assume-armed
```

`--reachability-assume-longjump` adds a third, longer-reaching edge
attempt (tried only when both the plain step and the ordinary running-jump
edge fail): a long jump (`item_longjump`), bounded the same way the
ordinary jump is — read live from this build's own
`ohl_physics::MoveConfig::long_jump_forward_speed`/`long_jump_up_speed`/
`gravity`, the same constants the engine's real long-jump impulse uses,
never a restated literal. A cell reached only this way is counted
separately in the printed report. Like `--reachability-assume-armed`, this
never checks or grants actual ownership of the long jump module — a cold
map load owns none — it only assumes one for the walk's own triage:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c4a1 --reachability-report \
  --reachability-assume-longjump
```

`--reachability-assume-pendulum-wait` adds a fourth round-advance edge, for
a `func_pendulum`: a brush that swings continuously through a corridor
rather than sitting statically closed or broken. The walk has no notion of
time or of a swing's current phase, so it cannot tell "blocked only while
swinging through this cell" from "permanently blocking" — without this
flag a `func_pendulum` on the frontier stays there forever, exactly like an
unarmed breakable does without `--reachability-assume-armed`. Setting the
flag treats it as passable between rounds, the same "detach the brush"
treatment a broken breakable or a shoved pushable gets, on the documented
assumption that a player can time the swing and walk through during a
clear moment — not a claim that the corridor is actually, permanently
open. The round that follows one being treated this way is marked
"(pendulum wait assumed)" in the printed report, so a route depending on
timing a swing is distinguishable from one that is not:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c1a2 --reachability-report \
  --reachability-assume-pendulum-wait
```

`--reachability-cell-cap N` and `--reachability-round-cap N` raise the
walk's own bounds past their defaults (40,000 cells per round, 6 rounds).
A map whose reachable area is itself larger than the default cell cap
stops the walk before a single round-advance edge
(door/breakable/pushable/pendulum) ever runs at all, hiding whatever those
edges would otherwise reveal; a map needing more than 6 rounds of doors
opened in sequence stops early the same way. Both are bounded by a hard
sanity maximum (`ohl_engine::reachability::MAX_CELL_CAP`/`MAX_ROUND_CAP`)
so even the most permissive override cannot turn a single report into
unbounded work:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map c4a2 --reachability-report \
  --reachability-assume-armed --reachability-cell-cap 300000
```

Also `dev-tools` only: `--start-inventory LIST` gives the player named
weapons and ammo right after the map loads, so a single-map probe or
scripted scenario can model the inventory a real campaign run would have
carried in from an earlier map via `changelevel`, instead of always
starting from the empty inventory a cold load otherwise gets. `LIST` is a
comma-separated list of `weapon_*`/`ammo_*` classnames — the same pickup
vocabulary a `weapon_*`/`ammo_*` entity in a map already uses
(`ohl_combat::classify_classname`), applied through the same grant path a
touch pickup uses: a weapon entry unlocks it and grants its bundled ammo,
an ammo entry tops up one pickup's worth, and repeating a classname stacks
it. An unrecognised classname, or one that names something other than a
weapon or ammo (`item_suit`, `func_healthcharger`, ...), is a usage error.
This never changes the save format — inventory is save tag 23, and giving
items at load uses the ordinary runtime inventory API, not a new one. It
combines with `--script`/`--headless-screenshot`/`--play` the same way
`--load` does, for an armed scripted scenario or capture; it is
independent of `--reachability-report --reachability-assume-armed` above
— that flag only ever *assumes* a weapon for the walk's own triage, it
never reads the player's actual inventory, so `--start-inventory` does not
change what the walk reports:

```sh
cargo run --release -p ohl-app --features dev-tools -- \
  --payload-root /path/to/payload --map t0a0b1 \
  --start-inventory weapon_shotgun,ammo_buckshot --play
```

**Smoke tests**, each of which builds (or accepts a prebuilt)
`open-half-life` and drives it against an already-imported payload:

```sh
cargo xtask campaign-smoke --payload-root /path/to/payload   # every campaign map, headless-screenshotted
cargo xtask combat-smoke --payload-root /path/to/payload     # every xtask/smoke-scenarios/*.txt scripted scenario
cargo xtask chain-walk --payload-root /path/to/payload       # the chained campaign walk (xtask/chain-routes/)
```

`cargo xtask chain-walk` assembles a chain from `xtask/chain-routes/`
(`<start>.txt` for the start map's own route, then `<start>-hop1.txt`,
`-hop2.txt`, ... for the route from each successive arrival point; a
missing hop ends the chain), runs it in one process through
`--chain-script`, and prints an aggregate-only report: how many *distinct*
maps deep the chain got, how many simulated seconds that took, and which
fixed terminal line ended it. It exits non-zero when the chain reaches
fewer distinct maps than `--min-depth` (default 2), or when it re-entered
a map it had already visited at any depth. The shipped chain reaches four
distinct maps and ends on "The chain walk has no further route." — it
runs out of authored routes, not out of map. `--start NAME` walks a different
chain, and must name a map from `ohl-campaign`'s own cited table.

Route files are named by their position in the chain rather than by the
map they run on, past the first: which map a level change lands in is a
fact about the user's own payload, and only lawfully public name literals
belong in this repository (see [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md)
rule 7). Their contents follow the same rule the smoke scenarios do —
script commands, campaign-table names and route words only.

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
- **Import worker sandbox**: the isolated media-parser worker has a
  native containment backend on two targets. On Linux x86-64 it is
  resource limits, no-new-privileges, Landlock, seccomp and a
  pidfd-backed lifecycle around a static musl Rust std image executed by
  descriptor. On macOS (both architectures) it is resource limits, a
  descriptor sweep, and the system sandbox (Seatbelt, via
  `/usr/bin/sandbox-exec`) around a hosted image that links nothing but
  libSystem, with a `kqueue` `NOTE_EXIT` lifecycle. Every other
  platform/architecture tuple still selects the unsupported backend, so
  import cannot begin there. Neither backend is release-qualified, and
  only Linux x86-64 has been exercised against a real medium. See
  [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the exact gates.

## Release builds

`cargo xtask dist` builds the release binary (and, on a Linux x86-64 or
macOS host building for itself, the sandboxed media-parser worker image
alongside it), then
assembles a versioned, self-contained release folder under
`target/dist/open-half-life-<version>-<target-triple>/`:

```text
bin/open-half-life[.exe]
libexec/open-half-life/ohl-media-parser-worker   (Linux x86-64 and macOS)
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
image, which exists only for the two targets with a native containment
backend, is only ever bundled for a host building for itself.
`cargo xtask dist --help` documents every flag.
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
  tree. macOS now has the second native worker sandbox, so the same
  pipeline composes there rather than refusing at launch, but no medium
  has been imported on macOS yet: that path has hosted CI evidence only,
  not a real-disc run. See
  [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the
  production-readiness gates that are *not* yet met on any platform, Linux
  included — only one ISO layout has been exercised anywhere.
- **All 93 campaign maps** (18 story chapters plus the Hazard Course) load
  and render successfully headless (`cargo xtask campaign-smoke`), with
  monsters, props and sprites rendering alongside the world geometry.
- **Movement and collision**: the player collides against both worldspawn
  geometry and solid brush entities (doors, moving platforms, and the
  like), not only the static world; a player standing on a moving brush
  entity (a `func_train`/`func_tracktrain`, `func_plat`, or lift
  `func_door`) rides along with it instead of being left behind or sinking
  through it; and `func_ladder`/`func_water` submodels attach their own
  ladder/water/slime/lava contents so climbing and swimming work against a
  brush entity, not only world-baked geometry — verified on a real map
  (the Hazard Course's "t0a0a" sub-map) with a scripted walk that reaches
  and climbs a `func_ladder`.
- **The campaign start map's opening tram ride now plays**: a
  `func_tracktrain` is placed on the first node of its own path at spawn
  (not wherever its brushes happened to be compiled), `game_playerspawn`
  entities fire on load, and the mover-riding fix above carries the player
  with the tram, so a scripted idle run rides out of the start area,
  through the hazard-striped tunnel portal and down the rock tunnel, with
  no movement key pressed.
- **Monsters that move**: a monster's rendered model and hitbox now track
  its AI-driven walking/chasing/fleeing motion every step (previously the
  model stayed pinned at its last position while the AI kept moving
  underneath), and a monster's position resumes correctly from a save
  rather than snapping back to its map spawn point on the next AI think.
- **Interactive map logic**: touch triggers (fired by the player's own
  movement, not only `use`, including `trigger_changelevel`), doors,
  buttons, `func_train`/`func_tracktrain` track trains,
  `trigger_camera` view sequences, scripted sequences/talk monsters,
  `monstermaker`s (including toggling one by name via `use`/`trigger`),
  and level transitions by touch (`trigger_changelevel`) or by `use`.
- **Combat and AI**: weapons, pickups, damage, monster AI, and navigation
  are implemented and exercised by a scripted combat scenario that picks up
  and fires a weapon (`cargo xtask combat-smoke`), which also runs a
  moving-player walk scenario through each of the 18 story chapters plus
  the Hazard Course, plus a ladder-climb scenario, as its own regression
  guard against the player falling through the world or a touch trigger
  never firing from movement — 24 scripted scenarios in total, all passing
  against a real imported payload.
- **Save/load** works over the project-owned `ohl-save` container (not the
  GoldSrc `.sav` format), with typed sections covering the engine header,
  entity registry, map-logic simulation, global state, light-style time,
  camera/player pose, inventory, entity combat state, AI state,
  projectiles/deployables, the RNG stream (M7.9 P4b), and — as of M7.13 —
  `func_train`/`func_tracktrain` position, a running `trigger_camera`
  sequence, a running scripted sequence's phase, a `monstermaker`'s spawn
  counters, and a `trigger_auto`'s one-shot fired flag.
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
applied; a brush entity's own `angles` keyvalue (a rotated door or
platform) is not yet applied to its collision shape; a `scripted_sequence`
target's pre-trigger idle animation (`m_iszIdle`) is not yet modelled; a
`monstermaker`'s already-spawned children are not themselves part of any
save section (only the maker's own spawn counters round-trip); and weapon
inventory still rides inside `SECTION_PLAYER_CARRY`'s ad hoc encoding
rather than its own save section.

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
(import pipeline) is functionally complete on Linux x86-64, has its
containment backend but no real-medium evidence on macOS, and is still
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
