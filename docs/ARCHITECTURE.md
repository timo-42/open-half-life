# Architecture

Open Half-Life is split into narrow C++ libraries. Dependencies point from
high-level orchestration toward low-level services; cyclic module dependencies
are not allowed.

The intended module set is:

- `core`: logging, diagnostics, common utilities
- `platform`: operating-system and architecture abstraction
- `formats`: independent parsers for documented binary formats
- `vfs`: path normalization, mounts, and read-only virtual files
- `media`: validation and import of user-provided media
- `render`: Vulkan rendering and MoltenVK integration
- `audio`, `input`: runtime device services
- `world`, `physics`: world representation and simulation
- `game`: single-player rules and entity behavior
- `ui`: menus and in-game user interface
- `tools`: developer and media-inspection programs
- `app`: composition root and executable lifecycle

The `core`, `platform`, `media`, `vfs`, and `app` modules now exist. New modules
must expose public
headers beneath `include/ohl/<module>` and keep implementation details in
`src`. Third-party API types should not leak across module interfaces.

The current dependency direction is:

```text
app -> core + platform + media + vfs
media -> standard library
vfs -> libudfread + standard library
core/platform -> standard library

experimental media cabinet adapter -> vfs + Unshield + zlib
```

The `media` target performs a bounded, project-owned ECMA-167 NSR02 preflight
and contains a default-off experimental Unshield adapter for read-only
InstallShield cabinet metadata.
The `vfs` target wraps libudfread behind C++ pImpl types, so third-party API
types do not leak into the engine. It exposes normalized read-only paths,
directory listings, and seekable files. Unshield receives callbacks backed by
that interface, allowing nested cabinets to be investigated without copying
them out of the image. Because Unshield is not hardened for malicious cabinet
metadata, the adapter is excluded from default builds and normal startup. The
app is the only composition root.

The intended gameplay/rendering graph remains under design. Each new edge must
be expressed explicitly with `target_link_libraries` so CMake remains the
source of truth and cycles can be rejected in review.
