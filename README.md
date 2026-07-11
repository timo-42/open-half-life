# Open Half-Life

Open Half-Life is a clean-room, cross-platform reimplementation of the
original Half-Life single-player runtime. It is at an early development
stage and cannot run the game yet.

The project does not include game data. You must own compatible Half-Life
media and provide it separately. Do not submit game assets, extracted files,
installer binaries, or information obtained by decompiling the original
engine.

## Build

Requirements:

- A C++20 compiler
- CMake 3.25 or newer
- Ninja
- Internet access for the first configure, to fetch pinned dependencies

Configure, build, and test a developer build:

```sh
cmake --preset dev
cmake --build --preset dev
ctest --preset dev
```

To use an installed libudfread instead, configure with
`-DOHL_USE_SYSTEM_UDFREAD=ON`; pkg-config must be able to find libudfread
1.1.2 or newer.

The default-off InstallShield feasibility adapter additionally requires Git
and can be enabled with `-DOHL_ENABLE_EXPERIMENTAL_INSTALLSHIELD=ON`. It is not
hardened for malicious cabinets and is not used by the application.

Run it with:

```sh
./build/dev/src/app/open-half-life
```

The path ends in `.exe` on Windows.
Supply `--cache /absolute/path` to override the platform user-cache location.
The current M2 implementation fingerprints the validated ISO and publishes a
metadata-only provenance record there; it does not extract game data yet.
Payload metadata can be checked against bounded, cross-platform path and
layout policies, but those checks are not yet connected to a production
extractor. Production payload import remains unavailable on every platform;
see [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md) for the current
readiness matrix and release-evidence gates.

## Status

The current implementation provides the M0 build and logging foundation, M1
media preflight, and the read-only UDF and provenance-cache portions of M2.
Run with `open-half-life --iso /path/to/owned-media.iso`; no installer or media
binary is executed. See [docs/MILESTONES.md](docs/MILESTONES.md)
for current progress, [docs/IMPORT_READINESS.md](docs/IMPORT_READINESS.md)
for production import readiness, and [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md)
before contributing compatibility work.

Half-Life is a trademark of Valve Corporation. This independent project is
not affiliated with or endorsed by Valve Corporation.

Repository-authored code is MIT licensed. The fetched libudfread dependency
is LGPL-2.1-or-later; Unshield and zlib use permissive licenses. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
