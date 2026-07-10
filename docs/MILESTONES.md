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
- a platform-independent payload streaming boundary exposes only opaque source
  tokens to readers, rejects writes beyond each declared size before forwarding
  them, requires exact final byte counts, and distinguishes source, destination,
  overflow, and underflow failures without mutating the filesystem
- a platform-independent staging orchestrator and injectable component-based
  atomic-directory-store contract enforce planned order, exact-file sealing,
  completion metadata last, explicit pre-publication cleanup, no-replace race
  revalidation, and precise published/parent-sync-complete versus
  published/parent-sync-uncertain reporting in synthetic fake-store and Linux
  native tests; cleanup failures are reported and may retain owned staging;
  sync completion does not claim universal filesystem durability
- a gated Linux atomic-directory store pins a validated existing root, uses
  private descriptor-relative staging, attempts descriptor-relative abort
  cleanup, structurally probes exact trees with same-device directories, and
  publishes once with `renameat2(RENAME_NOREPLACE)` on an explicit
  ext-family/XFS/Btrfs/tmpfs allowlist; failed cleanup may retain owned staging,
  same-size content tampering is outside the current structural probe guarantee,
  and the backend is not connected to startup; the shared `0xEF53` statfs value
  does not distinguish ext2/ext3/ext4, and qualification evidence currently
  covers ext4 and tmpfs while ext2, ext3, XFS, and Btrfs remain unqualified
- the Linux backend currently requires a trusted root namespace with no
  untrusted same-euid mutator; inode revalidation detects observed replacement
  but cannot make named cleanup conditional on inode identity
- a default-off experimental adapter proves the InstallShield 5+ cabinet can
  be parsed directly over VFS callbacks; its adapter output is entry-count
  bounded, invalid descriptors are reported rather than silently omitted, and
  a shared VFS handle prevents source lifetime bugs
- macOS and Windows atomic-directory stores and native adversarial gates,
  complete Linux qualification, production payload extraction, cache
  locking/recovery, and parser hardening or isolation remain

## Later milestones

- M3: BSP rendering
- M4: player movement
- M5: interactive entities
- M6: models and animation
- M7: combat
- M8: full campaign compatibility
- M9: release hardening
