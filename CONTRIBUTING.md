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

The project is implemented entirely in Rust under `crates/`, with repository
tooling in `xtask/`. There is no CMake or C++ tree. Build and test before
submitting:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace   # or: cargo test --workspace
cargo deny check
git add <intended-files>
cargo xtask policy
cargo xtask graph
```

`cargo xtask policy` checks tracked and staged files against the repository's
prohibited-content rules (asset/cache/imported prefixes, prohibited
extensions, a size ceiling, and known installer/media magic signatures), so
run it after staging; it is a backstop, not permission to add game-derived
data. `cargo xtask graph` enforces the acyclic crate dependency edges from
`.plan/rust-architecture-r1.md` section 1.

Document every new dependency, its version, source, and license in
`THIRD_PARTY_NOTICES.md`.
