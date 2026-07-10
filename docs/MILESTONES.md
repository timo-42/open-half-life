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
- the owned image mounts; an integration check reads one byte without logging,
  persisting, or committing its content
- a default-off experimental adapter proves the InstallShield 5+ cabinet can
  be parsed directly over VFS callbacks; the owned cabinet has 302 valid
  entries, but the unaudited parser is not reachable during normal startup
- cache layout, destination containment, symlink defenses, atomic extraction,
  a provenance manifest, and parser hardening or isolation remain

## Later milestones

- M3: BSP rendering
- M4: player movement
- M5: interactive entities
- M6: models and animation
- M7: combat
- M8: full campaign compatibility
- M9: release hardening
