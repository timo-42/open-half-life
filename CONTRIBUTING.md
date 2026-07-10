# Contributing

Read [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) before contributing. By
submitting a change, you attest that it is your original work, that it was not
derived from leaked source, decompilation, disassembly, or proprietary game
content, and that you have the right to license it under the repository's MIT
license.

Compatibility changes must identify their lawful sources in the pull request
and, when they implement a format, in [docs/FORMAT_SOURCES.md](docs/FORMAT_SOURCES.md).
Acceptable sources include public standards, official SDK documentation,
independently authored public documentation, and minimal black-box behavioral
observations from legally obtained software. Do not submit copyrighted media,
extracted assets, raw file listings, keys, or fixtures derived from game data.
Use project-authored synthetic fixtures.

Build and test before submitting:

```sh
cmake --preset dev
cmake --build --preset dev
ctest --preset dev
git add <intended-files>
cmake -P cmake/CheckRepository.cmake
```

The policy check examines tracked and staged files, so run it after staging.
It is a backstop, not permission to add game-derived data.

Document every new dependency, its version, source, and license in
`THIRD_PARTY_NOTICES.md`.
