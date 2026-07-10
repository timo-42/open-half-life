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
media -> core + platform + standard library
vfs -> libudfread + standard library
core/platform -> standard library

experimental media cabinet adapter -> vfs + Unshield + zlib
```

The `media` target performs a bounded, project-owned ECMA-167 NSR02 preflight,
streams the validated source through the project-owned SHA-256 implementation,
and publishes a metadata-only provenance manifest under a content-addressed
user cache. Source paths are not persisted. Standard-library checks require an
absolute cache path and reject observed symbolic links or non-directories; the
manifest is committed with a same-directory rename. Race-proof no-follow and
Windows reparse-point handling remain importer hardening work. The target also
contains a default-off experimental Unshield adapter for read-only
InstallShield cabinet metadata.
The `vfs` target wraps libudfread behind C++ pImpl types, so third-party API
types do not leak into the engine. It exposes normalized read-only paths,
directory listings, seekable files, and explicitly shared read-only mount
handles. Unshield receives callbacks backed by a shared handle, allowing
nested cabinets to be investigated without copying them out of the image or
borrowing the caller's lifetime. Its entry-count-bounded adapter output reports
invalid descriptors; the unaudited parser must still be isolated before
production use.

The always-built `media` path and layout policy is independent of Unshield. It
normalizes archive-controlled names to a strict printable-ASCII subset,
creates deterministic case-folded keys, rejects portable path conflicts, and
applies metadata and declared-size quotas before any destination is opened.
Those path and layout checks are lexical and planning checks only. A
platform-independent streaming boundary now gives a source only a planned
entry's opaque token, wraps the caller's byte sink, rejects chunks that exceed
the declared size before they reach that sink, and requires an exact final byte
count. It reports source, destination, overflow, and underflow failures
separately and performs no filesystem mutation. Production extraction remains
absent: it still needs atomic destination publication, native
no-follow/create-new operations, and Windows reparse-point enforcement.
The media module now also has a platform-independent staging orchestrator. It
validates a complete deterministic plan before touching an injected store,
streams and seals one file at a time, seals completion metadata last, and
models cache hits, conflicts, no-replace publication races, explicit cleanup,
and whether the backend's post-publication parent-sync operation completed or
failed. A completed sync is not presented as a universal durability guarantee.
The component-based store is only an interface, and its behavior is tested with
a deterministic in-memory fake; there is no native filesystem implementation
or production-safety claim yet.
Because Unshield is not hardened for malicious cabinet metadata, the adapter
is excluded from default builds and normal startup. The app is the only
composition root.

The intended gameplay/rendering graph remains under design. Each new edge must
be expressed explicitly with `target_link_libraries` so CMake remains the
source of truth and cycles can be rejected in review.
