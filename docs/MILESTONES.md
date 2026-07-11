# Milestones

## M0: bootstrap

Status: accepted; feature baseline at
`df5ea6d51037671ef0165dacac9fe26df1bf4d2b`; current hosted CI at
`3fd0375c7a759b0fcd269fa73d6bdc8a36123134`

- C++20 CMake/Ninja build
- Linux x64, Windows x64, and macOS Apple Silicon CI matrix
- warning-clean `core`, `platform`, and `app` targets
- basic logging and host-platform detection
- unit and command-line smoke tests
- clean-room and architecture documentation

Hosted evidence includes Linux x64, Linux x64 with
address/undefined-behavior sanitizers, Windows x64, macOS Apple Silicon, and a
Linux x64 experimental-adapter build. All five required jobs pass for the
historical package-1 through package-4 feature baseline. The same five jobs
also pass at the current exact hosted-CI SHA above.

## M1: ISO detection

Status: accepted; feature baseline at
`df5ea6d51037671ef0165dacac9fe26df1bf4d2b`; current hosted CI at
`3fd0375c7a759b0fcd269fa73d6bdc8a36123134`

Implemented acceptance criteria:

- accepts an ISO path via `--iso`, a positional argument, or a prompt
- acquires the path once as a pinned, read-only `MediaSource` and does not
  retain or reopen the selected path
- rejects missing, non-regular, truncated, and structurally invalid files
- performs bounded ECMA-167 NSR02 preflight through its recognition sequence,
  anchor, exact descriptor CRC lengths, bounded extents, and volume records
- computes a full project-owned SHA-256 fingerprint with source-stability
  checks at validation boundaries
- returns a move-only `ValidatedMedia` proof that binds the same pinned source,
  structural inspection, size, and validation digest
- confirms the filesystem and reads its root through pinned libudfread
- logs sanitized validation failures and a generic mount result without
  exposing media-derived names, counts, paths, or content
- covers valid and malformed project-authored synthetic images with tests

The hosted cross-platform and sanitizer evidence listed under M0 exercises the
accepted M1 path as part of the same commit.

## M2: media import and virtual filesystem

Status: in progress; accepted packages 2–4 establish the current capability,
cache, planning/staging, VFS, and application-composition baseline at
`df5ea6d51037671ef0165dacac9fe26df1bf4d2b`

Current functionality:

- `MediaSource` pins native identity and supports positional reads plus
  explicit `verify_unchanged()` phase checks; identity pinning prevents path
  retargeting but does not claim that an external writer cannot mutate content
- cache preparation accepts `ValidatedMedia`, rechecks the same pinned source,
  rehashes all content, requires equality with the validation digest, and only
  then publishes a metadata-only provenance manifest; source paths and media
  bytes are not persisted
- platform user-cache discovery and an explicit `--cache` override are wired
  into startup; content-addressed source directories apply current
  standard-library symbolic-link and type checks, write a same-directory
  temporary manifest, and publish it by atomic hard-link insertion without
  replacing an existing destination; a raced existing regular manifest is
  opened through the pinned no-follow boundary and reused only when its complete
  contents match exactly, while a mismatch is a manifest conflict and
  unsupported hard-link publication fails safely. Directory-component checks
  are not yet a fully pinned native traversal
- the application is an acquire-once composition root: it discards the input
  path after `platform` acquisition, validates the capability through `media`,
  mounts `validated.source()` through `vfs`, reads the bounded root listing,
  and gives the same `ValidatedMedia` to cache preparation without reopening
  the original path
- the dependency graph has `media -> platform` and `vfs -> platform`, with no
  `vfs -> media` edge; the default-off experimental adapter adds only the
  one-way `media -> vfs` edge
- the read-only UDF VFS provides path normalization, mounted-state sharing,
  seekable streaming files, entry-at access restricted to one separator-free
  component, and serialized third-party access over the retained source
- bounded directory enumeration returns provider-ordered pages and a move-only,
  opaque `DirectoryCursor`; continuation consumes the cursor and rejects
  default, moved-from, reused, stale, and foreign cursors without partial output
- directory errors and source changes return empty, tokenless pages; the legacy
  `list()` API aggregates the same pages but succeeds only with the complete
  result, otherwise returning the error with an empty listing
- package-4 hard ceilings are 64 normalized path components; 256 entries,
  64 KiB of names, 96 KiB of logical result data, and 1,024 provider-work units
  per page; and 64 pages plus 65,536 provider-work units per cursor. Callers may
  lower, but not remove or raise, these limits
- archive-controlled payload paths have a strict printable-ASCII policy;
  traversal, reserved device names, ambiguous separators, excessive depth,
  and non-portable components are rejected before filesystem mutation
- deterministic payload layout applies entry, metadata, per-file, and
  aggregate-size limits; preserves opaque source tokens; rejects duplicate,
  case-only, and file/directory conflicts; and produces deterministic order
- the platform-independent payload streaming boundary exposes only opaque
  source tokens, rejects writes beyond the declared size before forwarding,
  requires exact final byte counts, and distinguishes source, destination,
  overflow, and underflow failures without filesystem mutation
- the platform-independent staging orchestrator validates a complete plan
  before touching an injected store, streams and seals one file at a time,
  seals completion metadata last, and models cache hits, conflicts, no-replace
  publication races, cleanup, and parent-sync completion versus uncertainty
- the component-based store is covered by a deterministic in-memory fake and a
  gated Linux backend that uses a validated existing root, descriptor-relative
  private staging and cleanup, exact-tree structural probes with same-device
  directories, and `renameat2(RENAME_NOREPLACE)` publication
- the Linux backend reports cleanup failures, may retain owned staging after a
  failed cleanup, does not authenticate same-size content in its structural
  probe, and requires a trusted root namespace without an untrusted same-euid
  mutator; current native qualification covers ext4 and tmpfs
- the default-off experimental parser adapter can read bounded metadata through
  shared VFS callbacks without copying the source or borrowing the caller's
  lifetime; invalid descriptors are reported rather than silently omitted

Remaining M2 work:

- deterministic component selection must precede final layout planning;
  edition-specific selection data may be supplied only through a runtime-only
  local recipe, and any project-owned selection parser requires recorded public
  format provenance
- the constrained parser worker protocol in `MEDIA_IMPORT.md` remains
  mandatory before any third-party parser may feed production extraction; the
  worker must have bounded read-only input authority, bounded IPC, sanitized
  errors, and no destination/cache authority
- production payload extraction remains absent and must not execute installer
  binaries or media-provided code
- macOS and Windows atomic-directory stores and native adversarial gates,
  complete Linux filesystem qualification, cache locking/recovery, component
  selection, and the parser worker boundary remain required before M2 can be
  completed

The package-4 run at `df5ea6d` remains the historical hosted evidence for that
feature baseline. Current exact-SHA evidence at
`3fd0375c7a759b0fcd269fa73d6bdc8a36123134` passes all five required jobs: Linux
x64, Linux sanitizers, the Linux experimental configuration, Windows x64, and
macOS Apple Silicon. This evidence validates implemented M2 functionality only;
it is not evidence for the remaining production extraction path.

The accepted isolated parser protocol sequence starts with the bounded OWP/1
codec at `3bc135c`, adds completion/cancellation race handling at `f17a40a`,
closes its late-reply drain gap at `3fd0375`, adds typed `hello`, `ready`,
`read_request`, and `read_reply` schemas at `16f15cb`, and adds deterministic
parser fuzz validation at `81a7ee9`. Commit `d59b6c5` then adds exact-empty
typed schemas for `enumerate`, `cancel`, `cancel_ack`, and `shutdown` and
extends typed fuzz dispatch. The accepted result includes canonical framing,
generic bounded payload primitives and budgets, fail-closed session ordering,
and complete-payload typed validation for eight message types. The typed
decoders enforce the applicable source/read bounds, request sequencing,
permitted reply status/data shapes, exact-empty payload shapes, and exact
payload consumption. Typed schemas remain absent for `stream_entry`,
`entry_batch`, `data_chunk`, and `complete`.

The ordering contract permits exactly one same-request late reply to drain
after `cancel_ack` only when a read was already outstanding before cancellation.
The deterministic fuzz target exercises frame decoding, generic payload
reading, session ordering, and all eight accepted typed decoders with bounded
matching and deliberately mismatching read contexts. Its deterministic
self-check establishes canonical read-request/read-reply decode reachability
and both context branches. The fixed corpus remains project-authored and
synthetic. This accepted protocol layer supports active M2 work but is not a
production import path: no runtime target depends on it, it has no source,
destination, extraction, or cache authority, and the typed decoders are not
wired to production state transitions. The protocol work authorizes no
proprietary extraction, and no worker implementation, transport, or native
sandbox backend has been accepted or integrated.

## Later milestones

- M3: BSP rendering
- M4: player movement
- M5: interactive entities
- M6: models and animation
- M7: combat
- M8: full campaign compatibility
- M9: release hardening
