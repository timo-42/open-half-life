# Milestones

## M0: bootstrap

Status: implemented locally; hosted CI pending

- C++20 CMake/Ninja build
- Linux x64, Windows x64, and macOS Apple Silicon CI matrix
- warning-clean `core`, `platform`, and `app` targets
- basic logging and host-platform detection
- unit and command-line smoke tests
- clean-room and architecture documentation

## M1: ISO detection

Status: implemented locally; hosted CI pending

Implemented acceptance criteria:

- accepts an ISO path via `--iso`, a positional argument, or a prompt
- rejects missing, non-regular, truncated, and structurally invalid files
- performs bounded ECMA-167 NSR02 preflight through its recognition sequence,
  anchor, exact descriptor CRC lengths, bounded extents, and volume records
- confirms the filesystem and reads its root with pinned libudfread
- reports only sanitized filesystem metadata
- covers valid and malformed project-authored synthetic images with tests

## M2: media import and virtual filesystem

Status: in progress

- read-only UDF VFS mount, path normalization, directory listing, streaming
  reads, and seek are implemented
- VFS entry-at access now accepts exactly one separator-free path component
- validated media is streamed through a project-owned, known-answer-tested
  SHA-256 implementation; source paths and media bytes are not persisted
- platform user-cache discovery and an explicit `--cache` override are wired
  into startup
- content-addressed source directories apply standard-library symbolic-link
  and type checks, and publish a metadata-only manifest by same-directory rename
- the owned image mounts; an integration check reads one byte without logging,
  persisting, or committing its content
- archive-controlled payload paths now have a strict printable-ASCII policy;
  traversal, reserved device names, ambiguous separators, excessive depth,
  and non-portable components are rejected before filesystem mutation
- payload layout planning applies entry, metadata, per-file, and aggregate-size
  limits; preserves opaque source tokens; rejects duplicate, case-only, and
  file/directory conflicts; and produces deterministic extraction order
- a default-off experimental adapter proves the InstallShield 5+ cabinet can
  be parsed directly over VFS callbacks; its adapter output is entry-count
  bounded, invalid descriptors are reported rather than silently omitted, and
  a shared VFS handle prevents source lifetime bugs. The owned cabinet has 302
  valid entries whose paths pass the portability policy; a manual owned-media
  observation found duplicate destinations, showing that installer component
  selection is required before extraction
- safe payload extraction, destination filesystem enforcement, Windows
  reparse-point checks, race-proof OS-level no-follow operations, streaming
  enforcement of declared sizes, cache locking/recovery, component selection,
  and parser hardening or isolation remain

## Later milestones

- M3: BSP rendering
- M4: player movement
- M5: interactive entities
- M6: models and animation
- M7: combat
- M8: full campaign compatibility
- M9: release hardening
