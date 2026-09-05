# Milestones

## M0: bootstrap

Status: accepted (Rust); evidence: PR #<n> ("Reach M1 parity in Rust and
remove the C++ implementation"). This milestone was originally accepted
against the C++ tree at feature baseline `df5ea6d51037671ef0165dacac9fe26df1bf4d2b`
(hosted CI `3fd0375c7a759b0fcd269fa73d6bdc8a36123134`); that C++ tree has now
been removed, and Rust reproduces the same acceptance criteria. The
historical C++ evidence remains reachable in git history immediately before
the "Remove C++ implementation superseded by the Rust workspace" commit.

- Cargo workspace (Rust 2024 edition, `resolver = "3"`), `cargo xtask policy`
  and `cargo xtask graph` in CI
- Linux x64, Windows x64, and macOS Apple Silicon CI matrix (`rust-clippy`,
  `rust-test`)
- warning-clean `ohl-core`, `ohl-platform`, and `ohl-app` crates
  (`unsafe_code = "forbid"` outside the two documented exceptions)
- basic logging and host-platform detection (`Platform: <OS> <arch>`)
- unit and command-line smoke tests (`crates/ohl-app/tests/cli.rs`)
- clean-room and architecture documentation

## M1: ISO detection

Status: accepted (Rust); evidence: PR #<n> ("Reach M1 parity in Rust and
remove the C++ implementation"). Originally accepted against the same C++
feature baseline as M0; see that note above.

Implemented acceptance criteria:

- accepts an ISO path via `--iso`, a positional argument, or a prompt
- acquires the path once as a pinned, read-only `ohl_platform::MediaSource`
  and does not retain or reopen the selected path
- rejects missing, non-regular, truncated, and structurally invalid files
- runs the bounded ECMA-119 preflight (`ohl_iso9660::preflight`) and then the
  bounded ECMA-167 NSR02 preflight (`ohl_udf::preflight`) — recognition
  sequence, anchor, exact descriptor CRC lengths, bounded extents, and volume
  records — over a shared block reader
- computes a full project-owned SHA-256 fingerprint (`ohl_media::fingerprint`)
  with source-stability checks at validation boundaries
- returns a move-only `ohl_media::ValidatedMedia` proof that binds the same
  pinned source, structural inspection, size, and validation digest
- confirms the filesystem and reads its root through `ohl_vfs::Mount`
  (backed by the pinned `ohl-iso9660`/`ohl-udf` readers)
- logs sanitized validation failures and a generic mount result without
  exposing media-derived names, counts, paths, or content
- covers valid and malformed project-authored synthetic images with tests,
  plus a manual run against a real Half-Life GOTY ISO reporting only
  sanitized aggregates

The three-OS `rust-clippy`/`rust-test` CI evidence listed under M0 exercises
the accepted M1 path as part of the same PR.

## M2: media import and virtual filesystem

Status: in progress (Rust). The import path itself is implemented end to end
on Linux x86-64 as of R4.7b; the milestone stays open for the other platform
tuples and for the production release-evidence gates in
`docs/IMPORT_READINESS.md`. The narrative below this note describes the C++
implementation that was accepted against this milestone before the C++ tree
was removed at Rust M1 parity; it is retained as historical acceptance
evidence and, for the parts not yet ported, as the specification the Rust
crates below must still reproduce (see the "Media import planning and
staging" note in `docs/ARCHITECTURE.md`).

The following crates have landed in Rust so far, each covering part of this
milestone's surface:

- `ohl-parser-protocol` — OWP/1 framing and all twelve typed message schemas,
  budgets, and fail-closed session ordering (isolated; not yet wired to a
  worker)
- `ohl-media-archive` — the block-source trait, bounded directory-listing
  model, path normalization, and the fixed classification vocabulary shared
  by both media readers and `ohl-vfs`
- `ohl-iso9660`, `ohl-udf` — bounded ECMA-119/Joliet and ECMA-167 NSR02
  preflight and read-only archive wrappers over pinned `hadris-iso`/
  `hadris-udf` 2.3.0
- `ohl-vfs` — the uniform read-only `Mount` facade over both readers, with
  bounded paged enumeration and move-only cursors
- `ohl-media` — fingerprinting, the move-only `ValidatedMedia` proof, and the
  metadata-only provenance cache
- `ohl-formats` — early BSP30/WAD3/MDL10/SPR decoder work (M3 groundwork)
- `ohl-parser-worker-service` and `ohl-parser-worker` — the bounded
  worker-side OWP/1 lifetime and the freestanding sandboxed worker binary
- `ohl-cabinet-format` and `ohl-cabinet` — the licensed Unshield-derived
  cabinet translation (see `THIRD_PARTY_NOTICES.md`), isolated behind the
  sandboxed worker
- `ohl-import` and `ohl-payload` — parser import sessions (channel, broker,
  catalog, handshake, process/result sessions) and payload path/layout/
  selection/staging

See `.plan/rust-architecture-r1.md` section 5, packages R4.1-R4.7, for the
package plan those crates were built against; this note does not re-audit
each crate's exact completeness against that plan.

Historical C++ status (pre-removal): in progress; packages 2–4 establish the
capability, cache, planning/staging, VFS, and application-composition feature
baseline at `df5ea6d51037671ef0165dacac9fe26df1bf4d2b`. Disconnected parser-result
validation was accepted at `909edcc`, followed by portable media cancellation
accepted at `0f2c78d`, trusted parser source reads at `c90f2d1`, and the
disconnected frame channel at `e4b819a`. The trusted parent handshake was
accepted at `13f0fb0`, and the disconnected trusted parent session was accepted
at `7bd9d38`. Accepted P1 work now also provides a private, non-installed,
disconnected parser-worker service with synthetic boundary tests and accepted
hosted cross-platform evidence from PR #7. B1 now hosts that service in the
installed, contained Linux x86-64 worker with a compile-fixed unsupported
dispatcher and local real-launcher evidence. The higher parent process-session
owner and its session-ID/worker-epoch allocation policy were accepted at
`537c11b` ([PR #12](https://github.com/timo-42/open-half-life/pull/12)); see
"Remaining M2 work" below for what it still lacks.

Production payload import remains unavailable on every platform. The current
readiness matrix and release-evidence gates are tracked in
[IMPORT_READINESS.md](IMPORT_READINESS.md).

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
- the platform-independent payload streaming boundary gives every
  `PayloadSource` the exact pinned `MediaSource` from `ValidatedMedia`, the
  planned opaque source token, and the same staging `CancellationToken`; it observes
  cancellation around source dispatch and sink writes, rejects writes beyond
  the declared size before forwarding, requires exact final byte counts, and
  distinguishes source, destination, overflow, underflow, and cancellation
  failures
- `stage_payload` requires `ValidatedMedia` and no longer accepts caller source
  identity; its local `ohl-payload-v2-sha256` identity binds the accepted
  source size and SHA-256, a non-empty trusted recipe identity bounded to 4,096
  bytes, and normalized paths and declared sizes plus entry count and declared
  total, while excluding transport-local source tokens
- the platform-independent staging orchestrator validates a complete plan
  before touching an injected store, streams and seals each payload file, seals
  completion metadata, reverifies the complete pinned source, and performs a
  final cancellation check whose next store operation is
  `publish_no_replace()`; every verification or cancellation failure before
  publication either precedes transaction creation or aborts the owned
  transaction and publishes nothing
- the orchestrator also models cache hits, conflicts, no-replace publication
  races, cleanup, and parent-sync completion versus uncertainty
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
- media-owned `CancellationToken` and `CancellationSource` provide copyable
  shared-identity, atomic, standard-like polling semantics across streaming,
  staging, and full-source verification. A default token cannot be stopped;
  `request_stop()` succeeds once; requested state survives source destruction;
  and an unstopped token becomes impossible after its final source disappears.
  This removes media's dependency on AppleClang 17 libc++ experimental
  `std::stop_token` support without enabling an experimental ABI
- the disconnected `OpenHalfLife::media_parser_results` target owns a
  caller-supplied worker epoch plus enumeration sequence, copies accepted paths,
  validates aggregate layout before catalog promotion, indexes token
  membership, requires the exact generation for streams, decrements trusted
  remainders only after accepted sink writes, and retires authority on
  replacement, cancellation, shutdown, failure, source invalidation, or worker
  failure. It has no runtime dependency edge
- the disconnected `OpenHalfLife::media_parser_reads` target depends only on
  the trusted result/session stack. It retains the exact pinned source from
  `ValidatedMedia`; source size comes from the validated fingerprint, while
  `maximum_read_bytes` is trusted constructor configuration that must exactly
  match the accepted typed `hello`. The broker alone cannot verify that binding;
  the accepted handshake proof records the values, not media identity, for
  trusted same-media, exact-limit composition. It validates typed read requests,
  owns
  sequence and request/reply-byte budgets, verifies stability before and after
  bounded reads, emits canonical success or prefix-only failure replies, scrubs
  scratch storage, and advances only when a unique prepared ticket is committed
  after full delivery. Pre-cancel replies may cross; post-cancel reads are
  ignored without source or output access; terminal and destructor paths retire
  the associated session and catalog authority
- the disconnected `OpenHalfLife::media_parser_transport` target depends only
  on `OpenHalfLife::parser`, `OpenHalfLife::platform`, and `Threads::Threads`.
  A nonzero session is bound to a trusted non-owning exact-I/O table, including
  an adapter that constructs callbacks forwarding directly to an already-created
  `IsolatedWorker`. Each frame uses a separate exact 32-byte canonical header
  transfer and bounded payload transfer with the same deadline and cancellation
  token. Receive views alias caller storage; failed payload reads invalidate the
  whole supplied buffer as a frame. One send may overlap one receive, while
  duplicate directions are rejected. Protocol or transport failure terminally
  retains the first sanitized cause, calls byte-channel `abort_io()` once, and
  prevents later I/O. A future process owner, not the channel, must close plus
  `wait()`/reap on orderly shutdown and reserve `terminate_and_wait()` for
  failure or orderly-close timeout
- the disconnected `OpenHalfLife::media_parser_handshake` target depends only
  on the source-read broker and frame transport. It validates the media proof,
  exact source-read limits, minimum protocol budgets, maximum receive storage,
  and channel state before I/O; sends the canonical 12-byte `hello` before
  observing it; then decodes an exact-empty `ready` before observing that
  header. The same deadline and cancellation token reach both channel calls.
  Success returns a move-only, single-consumption proof containing an idle
  validator charged for exactly two messages and 12 payload bytes plus the
  exact limits and derived source policy. Failed receive storage is unsanitized
  and wholly invalid as a frame; failures after interaction return no proof,
  terminally abort and permit no later I/O; impossible exact-I/O reports are
  sanitized and the first channel cause is retained. The proof borrows the
  exact channel identity, which must outlive the proof through consumption or
  disposal and then outlive a successfully created parent session and its calls
- the disconnected `OpenHalfLife::media_parser_parent_session` target depends
  only on the parent handshake. Its factory consumes the move-only proof only
  after exact frame-channel object binding, same-media size/policy checks,
  nonterminal state, valid import limits, and nonzero epoch succeed. The proof
  binds channel identity and read-policy values, but trusted composition remains
  responsible for supplying the same `ValidatedMedia`. The session owns result,
  broker, request, active-operation, reply-ticket, cancellation, outbound
  transaction, and sticky-failure state while borrowing the channel, retained
  sink, and per-call buffers. Sends and abort callbacks run outside the session
  mutex; read-only callback re-entry sees committed state with catalog hidden
  during a transaction. A staged read reply excludes cancel with
  `concurrent_operation`; if cancel stages first, later read consumption is
  ignored. Prompt worker/source notifications, prepared-reply commit/abandon,
  synchronous sink delivery, buffer privacy, shutdown, and destruction follow
  the accepted lifecycle contracts
- the disconnected `OpenHalfLife::parser_worker_service` static target depends
  only on `OpenHalfLife::parser`. Its private callback-and-buffer contract
  drives one bounded worker-side protocol lifetime for enumeration, streaming,
  parent-owned reads, cancellation, and shutdown. It is not installed or
  exported; the Linux x86-64 worker links a separate private freestanding copy
  of the same implementation. It chooses no real payload dispatcher/parser and
  owns no source path, destination, selection, staging, publication, cache, or
  application authority. Focused project-authored synthetic validation passed
  1/1, the development suite passed 39/39, and ASan plus UBSan passed 40/40;
  the accepted PR #7 hosted matrix also runs the service test across Linux x64,
  sanitizers, the experimental Linux configuration, Windows x64, and macOS
  Apple Silicon. This is cross-platform evidence for the disconnected boundary,
  not production qualification
- [PR #10](https://github.com/timo-42/open-half-life/pull/10) (`fb9a2df`) fixed
  GCC 15 hardened-libstdc++ linking of the freestanding worker: GCC 15 enables
  `_GLIBCXX_ASSERTIONS` by default and its `<span>`/`<bits/*>` precondition
  checks pull in hosted `memcmp` and `std::__glibcxx_assert_fail` symbols that
  the `-nostdlib`-linked freestanding worker and its isolated-worker test
  helper cannot resolve; the fix adds private, terminating shims for both
  instead of disabling the hardening checks
- the installed Linux x86-64 static worker now emits and closes its exact
  readiness record on fd 4, then hosts one bounded OWP/1 lifetime on fd 3. It
  accepts canonical `hello`, emits exact-empty `ready`, and supports shutdown
  and orderly peer close. Its compile-fixed project dispatcher rejects
  enumeration and streaming as unsupported, and sanitized terminal outcomes
  surface through the existing native lifecycle categories. It has no real
  parser, source-read, selection, destination, staging, publication, cache, or
  runtime authority
- `platform.isolated_worker.linux` stages byte-identical production-target
  worker bytes and launches them through the public native backend, covering
  static identity, resource limits, no-new-privileges, Landlock, seccomp,
  readiness EOF, pidfd lifecycle, fragmented hello/ready/shutdown, malformed
  protocol failure, fixed unsupported enumeration, IPC close, cached wait, and
  owned termination/reap. Direct service and install-smoke tests add truncated
  I/O, peer-close, non-writable/non-set-id static image, payload arenas,
  exact fd-3 shutdown,
  and clean-exit evidence. The focused bootstrap set passed 4/4, the full
  development suite passed 40/40, and the real-launcher test passed 50/50
  consecutive runs. Owned termination may resolve as `clean` or `terminated`
  when orderly EOF wins; both outcomes are terminal, cached, and reaped. This
  is local Linux x86-64 bootstrap evidence only

Remaining M2 work:

- deterministic component selection must precede final layout planning;
  edition-specific selection data may be supplied only through a runtime-only
  local recipe, and any project-owned selection parser requires recorded public
  format provenance
- **Done at R4.7a/R4.7b on Linux x86-64.** The real dispatcher
  (`ohl-parser-backends`: Wise overlay, MS-CAB, InstallShield 3 Z over the
  OWP/1 pull model), the parent-side composition (`ohl_import::pipeline`:
  locate, deterministic container choice, bounded source window, one worker
  per session, enumerate, select, plan, stage, reverify, publish once, record
  provenance) and the application composition (`ohl-app`: real import on first
  run, `--payload-root`, runtime rediscovery of a published tree) are
  implemented and exercised against a real medium. The worker still has no
  raw-path, destination, cache, recipe-selection or publication authority
- the abstract `IsolatedWorker` lifecycle facade exists, but a native
  containment backend is source-selected only for Linux x86-64; every other
  platform and Linux architecture selects the unsupported backend, so import
  cannot begin there. Those backends remain required
- production *qualification* is still blocked on the objective
  release-evidence gates in `IMPORT_READINESS.md`: installed-package inventory
  and identity, an installed-prefix hosted end-to-end run, crash/restart and
  publication-recovery evidence, sanitizer/fuzz/stress campaigns over the new
  container back ends, and independent architecture, security, reliability,
  release, and product review. No tuple meets them, Linux x86-64 included
- payload extraction must continue to never execute installer binaries or
  media-provided code; the back ends decode container bytes only, inside a
  confined worker with a fixed `.bss` arena and no `brk`/`mmap`
- macOS and Windows atomic-directory stores and native adversarial gates,
  complete Linux filesystem qualification, and cache locking/recovery remain
  required before M2 can be completed. Component selection and parser-worker
  runtime composition are no longer on that list: both landed at R4.7a/R4.7b

The package-4 run at `df5ea6d` remains the historical hosted evidence for that
feature baseline. [PR #7](https://github.com/timo-42/open-half-life/pull/7)
accepted the current P1 disconnected worker-service tree at reviewed head
[`3ec70b34f461ec7dddb1ca26770544df6debfe0f`](https://github.com/timo-42/open-half-life/commit/3ec70b34f461ec7dddb1ca26770544df6debfe0f),
then rebased that tree onto `main` as
[`6b3df8f1cf6660eed46246790bff382c6c4001b6`](https://github.com/timo-42/open-half-life/commit/6b3df8f1cf6660eed46246790bff382c6c4001b6).
Both commits have exact tree
`888bee1be57b45c7583fe05bcf22698725f5f651`. All 12 hosted jobs passed: Build
runs
[`29195613360`](https://github.com/timo-42/open-half-life/actions/runs/29195613360)
and
[`29195614365`](https://github.com/timo-42/open-half-life/actions/runs/29195614365)
each passed Linux x64, sanitizers, the Linux experimental configuration,
Windows x64, and macOS Apple Silicon; Parser fuzz smoke runs
[`29195613343`](https://github.com/timo-42/open-half-life/actions/runs/29195613343)
and
[`29195614314`](https://github.com/timo-42/open-half-life/actions/runs/29195614314)
each passed Linux Clang 18/libFuzzer. The final PR change replaced an oversized
fixed stack buffer in the service test with payload-sized dynamic test storage
so Windows x64 could run it; no production code changed. This evidence validates
the private disconnected service and the implemented M2 stack only; it is not
evidence for a real parser, worker/runtime composition, or the remaining
production extraction path.

The later `909edcc` bridge and `0f2c78d` cancellation migration have exact-SHA
hosted evidence from build run `29147060407` at `ca576e9`. GNU 13 Linux passed
32/32 tests including the bridge; the experimental, sanitizer, and Windows jobs
passed; and AppleClang 17 macOS passed 22/22 tests including
`media.cancellation` and the bridge. This confirms the common macOS portability
fix and disconnected result validation, not any remaining native worker,
source, staging, atomic-store, or runtime-import prerequisite.

Trusted parser source reads were accepted and pushed at
`c90f2d1a7cbabdb90b688197d2d34ceb48526aeb`. The full local CTest suite passed
33/33, including comprehensive synthetic broker coverage. Exact-commit hosted
build run `29148133002` passed Linux x64, sanitizers, the experimental cabinet
adapter, Windows x64, and macOS Apple Silicon; this is the cross-platform broker
evidence. Fuzz run `29148132997` separately passed its typed-protocol-only Clang
18/libFuzzer job and did not build or fuzz the broker. The build evidence
qualifies the disconnected broker on those hosts, not any absent native worker,
transport, runtime import, staging, or publication path.

The disconnected frame channel was accepted and pushed at
`e4b819a9efa37d5e401d111c4ac591365ce669ae`. Local validation completed a clean
warnings-as-errors build with 83/83 steps, the full CTest suite at 34/34, 50
consecutive passes each for the frame-channel and repository-policy tests, and
the platform common-worker test at 1/1. These exact-commit local results cover
the trusted operation table, exact header/payload transfers, validation
ordering, session binding, caller buffer/view lifetimes, concurrency, terminal
poisoning, sanitized transfer errors, and abort wakeup. The later PR #7 hosted
evidence above covers the same frame-channel implementation in the accepted
merged tree.

The trusted parent handshake was accepted and pushed at
`13f0fb08e7d00159000f3721ebe0b0e1b1481188`. Its local clean
warnings-as-errors build passed 87/87 steps and the full local CTest suite
passed 35/35. Synthetic evidence covers independent hello/header bytes,
exact-empty ready, observation order, deadline/token identity, exact policy and
limits, proof moves and single extraction, downstream session/broker
construction, every public pre-I/O rejection class, caller-buffer invalidation,
sanitized terminal failure, one abort, and no escaped proof or view. These
exact-commit results are local; the later PR #7 hosted evidence above covers the
same handshake implementation in the accepted merged tree.

The trusted parent session was accepted and pushed at
`7bd9d38213c7df160e0e84fcb50a9cacb0095558`. Hash, index, manifest, and diff
guards showed that its seven-file parent-session package matched that commit.
Independent pristine local verification archived exact tree
`f28715ef827044928a0c9cc1ce45464d5c8d9519` with SHA-256
`6361378e63c5de330784836106851fd4b0afb4d4b239d10b495912fe585a8123`.
The archive compiled committed `isolated_worker_unsupported.cpp`, excluded the
shared worktree's dirty native files, and passed a clean Linux GCC 14 Debug
warnings-as-errors build at 91/91, full CTest at 36/36, and explicit policy at
1/1. Synthetic behavioral tests cover proof/channel binding,
owned result/broker/request/transaction state, borrowed sink and buffers,
callbacks outside the lock, read-only in-flight visibility, receive/cancel and
notification crossings, reply tickets and privacy, sink/lifecycle/destruction
contracts, budgets, errors, and no escaped frame or payload result. Separate
API/source/CMake review establishes the disconnected edge and absence of path,
launch, staging, publication, and runtime authority. Request-ID exhaustion is
not practically exercised without a counter seam, and stable injected
source-read failure is covered at the broker layer because the parent factory
does not expose the broker's operation-table seam. These are accepted coverage
limitations. These exact-commit results are local; the later PR #7 hosted
evidence above covers the same parent-session implementation in the accepted
merged tree.

The accepted isolated parser protocol sequence starts with the bounded OWP/1
codec at `3bc135c`, adds completion/cancellation race handling at `f17a40a`,
closes its late-reply drain gap at `3fd0375`, adds typed `hello`, `ready`,
`read_request`, and `read_reply` schemas at `16f15cb`, and adds deterministic
parser fuzz validation at `81a7ee9`. Commit `d59b6c5` then adds exact-empty
typed schemas for `enumerate`, `cancel`, `cancel_ack`, and `shutdown` and
extends typed fuzz dispatch. Commit `f4d908a` adds the typed `stream_entry`
schema and its fuzz dispatch. Its payload is exactly one canonical 8-byte
little-endian opaque `source_token`; zero and every other `uint64_t` value,
including the all-ones value, are valid at this codec boundary. Token
membership is not established by this codec and the token conveys no source
authority; the later disconnected bridge establishes membership only after
catalog promotion. Commit `c28ea9f` adds the typed
`data_chunk` schema and fuzz dispatch. A data chunk is its opaque whole payload
with no prefix, offset, token, or status field; zero bytes are forbidden and
the accepted range is 1 byte through 256 KiB. The codec requires a trusted
nonzero remaining-entry context and rejects a chunk larger than that bound.
Its decoded span aliases the frame payload, so that storage must stay alive and
unchanged while the span is used. The caller owns remainder accounting and may
decrement it only after the accepted bytes are written downstream. Commit
`2d71079` adds the success-only typed `complete` schema and fuzz dispatch. Its
exact four-byte canonical little-endian payload is `u16 ProtocolStatus` then
`u16 ProtocolPhase`. The trusted expected-operation context must be
`enumerate` or `stream`, and only `(ok, complete)` is accepted in either. All
other known or unknown pairs are rejected; failure-result representation and
worker failure/publication authority remain deferred. A receiver must decode
the payload before state observation and separately establish every read,
result, remainder, and downstream-write prerequisite. Commit `ba84cfc` adds the
final typed `entry_batch` schema and fuzz dispatch. Its canonical
little-endian wire layout is:

```text
u16 entry_count (1..256)
repeat entry_count:
  u64 source_token
  u64 size_bytes
  u16 archive_path_length (1..4096)
  printable ASCII archive_path bytes
```

The generic 1 MiB frame ceiling applies. The trusted cumulative policy caps
remaining entries at 50,000, remaining path bytes at 64 MiB, an entry at 8 GiB,
and remaining declared bytes at 32 GiB; callers may tighten those bounds.
Tokens increase strictly within and across batches, with zero valid as the
first candidate. The allocation-free two-pass decoder validates the whole
payload and policy before populating caller storage. Its entry span aliases
that storage and its path views alias the frame, so both must stay alive and
unchanged while in use. Printable archive spellings are not normalized paths,
and an empty batch is rejected; an empty enumeration completes without a
batch.

The accepted result includes canonical framing, generic bounded payload
primitives and budgets, fail-closed session ordering, and complete-payload
typed validation for all twelve message families. The typed decoders enforce
the applicable source/read bounds, request sequencing, entry-batch count, path,
size, cumulative-policy and token-ordering bounds, permitted reply status/data
shapes, exact-empty, exact-token, bounded opaque-chunk, or success-only
completion payload shapes, and exact payload consumption. No untyped message
family remains.

Commit `909edcc` adds the disconnected trusted parser-result bridge. The caller
assigns a nonzero epoch unique to the worker lifetime, and each enumeration
adds a local sequence so matching token values cannot revive a catalog across
worker restarts. Candidate batches advance quotas and token ordering only after
typed and protocol acceptance, and their path views are copied into owned
strings. Successful enumeration completion validates the complete candidate
through deterministic payload-layout planning, rejects aggregate or normalized
path conflicts, and atomically promotes an empty or populated catalog with a
sorted token membership index. Streaming requires the exact catalog generation
and a member token, derives its remainder from the promoted size, and decrements
only after the sink accepts each complete chunk; completion requires zero.
Cancellation removes catalog authority and prevents candidate promotion while
preserving only bounded crossing-frame validation. Other replacements,
terminal failures, shutdown, trusted source invalidation, and worker failure
retire the relevant state.

Commit `0f2c78d` then replaces media's standard stop types with project-owned
`CancellationToken` and `CancellationSource`. They retain standard-like
copyable shared-state identity, polling, first-request idempotence, and
cross-thread observation without callbacks or experimental libc++ ABI flags.
The change preserves existing cancellation points and removes the known
AppleClang 17 libc++ compile dependency; the exact hosted result above confirms
that correction.

Commit `c90f2d1` adds the disconnected trusted parser source-read broker. It
depends only through `OpenHalfLife::media_parser_results`, retains the exact
pinned capability from `ValidatedMedia`, and obtains source size from that
proof's fingerprint. Its `maximum_read_bytes` is trusted constructor input that
must exactly match the accepted typed `hello`; the broker alone cannot verify
that binding. The accepted parent-handshake proof records the values, not media
identity; trusted later composition must use the same media and exact limits.
It owns canonical request
sequencing and independent request/reply-byte budgets. For a serviceable request
it verifies stability, performs one bounded read, verifies again, encodes an
exact success or prefix-only `source_changed`/`source_read_failed` reply, and
scrubs all temporary scratch. Reply storage remains caller-owned under a prepare
ticket; sequence advances only when full delivery is reported through
`commit_reply_sent()`.
Abandonment, invalid tickets, committed source failure, terminal errors, and
active destruction retire the broker and result session. A pre-cancel reply may
cross, including the one acknowledged drain, while a post-cancel request is
ignored without reading, charging, or touching output. Its complete optional
operation table is trusted test injection and partial tables are invalid. The
broker passes the retained capability as the callback source argument but does
not constrain callback code's ambient authority; only trusted project/test code
may supply it, and worker/media input cannot configure it.

Commit `e4b819a` adds the disconnected trusted parser frame channel. Its only
dependencies are the parser protocol, platform worker interface, and Threads.
A caller supplies a nonzero session plus a complete non-owning exact-I/O table;
the existing adapter constructs callbacks that directly forward to an
already-created `IsolatedWorker`.
After configuration checks, send validates header, session, payload ceiling,
and exact declared length before I/O. Receive checks maximum caller capacity
before header consumption, then validates the exact 32-byte header and session
before a separate bounded payload read. The deadline and cancellation token are
forwarded unchanged at each stage. Successful views borrow caller storage; a
failed payload read can leave a partial untrusted prefix and stale suffix, so
the whole buffer is invalid as a frame. One send and one receive may overlap;
same-direction overlap is rejected. Protocol or transport failure retains the
first terminal cause, sanitizes impossible exact-I/O reports, calls idempotent
`abort_io()` once to wake active byte-channel operations, and suppresses later
I/O. This abort is not process termination or reap authority. Trusted custom
callbacks retain ambient process authority; limiting suppliers is composition
policy, not mechanical confinement.

Commit `13f0fb0` adds the disconnected trusted parent handshake, depending
directly only on the source-read broker and frame channel. It first validates
the borrowed fresh channel and `ValidatedMedia`, captured source-size
agreement, exact copied source-read limits, copied protocol budgets of at least
two messages and 12 payload bytes, protocol-maximum capacity of the borrowed
receive storage, derived source policy, and validator configuration. The
deadline is copied, and the copied cancellation token shares its source's
state. Nonterminal pre-I/O rejection performs no channel I/O or abort and
returns no proof. Through the borrowed media, the handshake temporarily
receives the pinned source capability only to query captured size; it reads no
source bytes, and neither the handshake nor proof retains or grants that
capability.

The parent then sends one canonical `hello` for the channel session and request
zero. Its exact 12-byte payload binds the validated fingerprint size and trusted
maximum read. Only after complete transport acceptance does the validator
observe the outgoing header. The received frame must decode as exact-empty
typed `ready` before its header is observed. Both operations receive the same
deadline and cancellation token. Success proves an idle validator with exactly
two messages and 12 payload bytes charged, alongside the exact limits and
derived source policy. The proof is move-only and transfers its validator once;
taking it invalidates the proof and result.

Later trusted composition must move that validator into the result session and
construct the source-read broker from the same media proof and limits. The
proof retains copies of the limits and policy but no media identity, so it does
not mechanically prove same-media use; that is a trusted composition
requirement. The handshake's receive buffer is not scrubbed: after payload I/O
or typed-ready
failure it may hold an attacker-controlled prefix and stale suffix, and the
whole buffer remains invalid as a frame until reinitialized. Interaction
failures return no proof, terminally abort the channel, retain a sanitized first
cause, and allow no later I/O or escaped frame/payload view.
The exact channel must outlive the proof through consumption or disposal; after
successful consumption it must outlive the created parent session and calls.

Commit `7bd9d38` composes that proof into the disconnected trusted parent
session. Factory validation binds the proof to the exact borrowed channel
object and its nonterminal state, checks the media's captured size against both
its fingerprint and the proof's source policy, checks the proof's exact read
limits, import ceilings, and nonzero worker epoch, and consumes the proof only
after success. A same-session-ID substitute channel is rejected. The proof
still contains no media identity, so same-media use remains a trusted caller
contract.

The session owns the result bridge, source broker, monotonic nonzero request
allocation, active operation, reply tickets, cancellation and outbound
transactions. It borrows the channel for its lifetime, a synchronous
nonthrowing sink for the active stream, and disjoint receive, scratch, and reply
storage for each receive. The channel must not be used directly while owned by
this composition, and all borrowed objects must outlive their documented
interval. Neither active calls nor callbacks may race destruction. Open
destruction retires authority and aborts; closed destruction does not.

Enumerate, stream, read reply, cancel, and shutdown use explicit in-flight
transactions. Provisional lower-layer changes are made under the session mutex,
then sends and abort callbacks run outside it. Competing mutations fail without
I/O. Read-only frame callback re-entry is limited to terminal/state/result and
catalog inspection; it sees last-committed wrapper state and no catalog while a
transaction is staged. Worker-failure and source-invalidation notifications
retain the first cause, retire result authority, and then abort outside the
lock, promptly waking blocked channel operations.

Receive runs channel I/O outside the transaction mutex, allowing cancellation
to cross a blocked read. The established ordering lets valid completion win a
completion/cancel crossing. Entry batches and chunks report progress,
enumeration completion promotes a catalog, exact stream completion requires no
remainder, and cancel acknowledgement clears the operation. Read requests are
prepared through the broker under unique tickets and committed only after exact
send; failed delivery abandons the ticket. Post-cancel reads are ignored without
source/reply access. ParentSession serializes read reply against cancel: once a
reply transaction is staged, cancel returns `concurrent_operation` without I/O;
if cancel stages first, receive waits and the broker ignores the newly observed
read. The lower result/broker drain allowance remains valid in isolation, but
this parent arbitration cannot produce a cancel acknowledgement overtaking its
staged reply.

Receive, scratch, and reply buffers must respectively hold the protocol maximum,
the accepted maximum read, and the fixed reply prefix plus maximum read, and
must be nonnull and pairwise disjoint before I/O. Receive storage may retain an
attacker prefix and stale suffix; used scratch is scrubbed but unused scratch
can remain stale; reply storage can retain private source bytes and requires
caller scrubbing. No view escapes. Sink rejection is terminal but cannot undo
accepted caller-side effects. Shutdown closes only protocol state and does not
close, terminate, wait for, or reap the worker/channel.

The parent-session result preserves bounded project errors for configuration,
state/concurrency, buffers, request exhaustion, allocation, protocol, channel,
result, source, worker, source invalidation, and internal failure. It accepts no
path, replacement source, executable/service or component selection and owns no
worker launch/lifecycle, destination, staging, publication, cache, application,
or runtime-import authority. It does accept `ValidatedMedia`, and its broker
retains that proof's pinned source capability; the exclusion is raw-path and
replacement-source authority, not all source capability.

The abstract `IsolatedWorker` facade already supplies lifecycle operations, and
committed HEAD source-selects a native containment backend for Linux x86-64;
other platforms and Linux architectures select the unsupported backend.
Remaining gates are qualification of a native backend for each supported tuple;
the real media dispatcher/parser; a higher owner for session-ID and
worker-epoch uniqueness, channel/session
lifetime, orderly close plus `wait()`/reap, and failure/timeout
`terminate_and_wait()`; then handshake/session composition, deterministic
selection, staging, and publication, in that order.

The ordering contract permits exactly one same-request late reply to drain
after `cancel_ack` only when a read was already outstanding before cancellation.
The deterministic fuzz target exercises frame decoding, generic payload
reading, session ordering, and all twelve accepted typed decoders. Entry-batch
dispatch uses fixed 256-entry storage plus bounded broad, matching-token,
replay-token, and reduced-budget contexts. Its deterministic self-check covers
canonical and matching-token acceptance plus replay, non-printable, and budget
rejection. Exhaustive unit validation covers wire order; count, path, ASCII,
token, size, cumulative and frame ceilings; truncation; storage capacity and
alias lifetimes; cross-batch ordering; multi-batch completion; and
decode-before-observe atomicity. The prior bounded read, data-chunk, and
completion contexts remain covered. The fixed corpus remains project-authored
and synthetic.

The tests-only `ca576e9` change did not trigger the parser-fuzz workflow.
Its hosted fuzz evidence therefore remained the earlier `ba84cfc` result and
was separate from run `29147060407`. The later exact `c90f2d1` fuzz run
`29148132997` now passes for the typed protocol only; it did not build or fuzz
the source-read broker. Cross-platform broker evidence comes from build run
`29148133002` above.

This accepted protocol, result-validation, source-read, disconnected frame
transport, parent-handshake, parent-session, and private worker-service stack
supports active M2 work but is not a production import path.
The result bridge owns
catalog generation, promotion, membership, layout, stream remainder, and
retirement; the read broker owns bounded reads from the retained pinned
capability and their prepare/commit ordering; the frame channel owns only
bounded framing over a caller-supplied byte capability; the handshake proves
only the typed transition and exact broker policy binding; the parent session
owns the guarded composition and transaction state above those pieces. No
application or trusted parent-composition target links these libraries. The
Linux installed worker hosts only its private service-runtime copy. The frame
channel and handshake do not launch or own a worker or sandbox, accept a source
path, read source bytes, select a component,
stage or publish data, or grant runtime/application authority. They have no
process termination or reap authority. The frame channel also accepts no
executable, path, source, component selection, catalog, staging, destination,
publication, cache, or application authority; the result and read bridges also
create no worker or runtime import path and own no staging or publication.
Linux x86-64 native isolated-worker containment exists as a source-selected
backend, and its installed worker now hosts the private service implementation
over fd 3. The compile-fixed dispatcher rejects payload operations as
unsupported, and the worker is not composed with the trusted parent session or
the runtime. A real parser, higher process-session management, deterministic
selection, staging/publication, and runtime composition remain later
dependencies. This work authorizes no proprietary extraction.

## R2: Rust bootstrap

Status: accepted. Superseded by M0/M1 above: R2 added the Rust workspace
beside the C++ tree; R3 (packages R3.1-R3.5) then brought Rust to M1 parity
and removed the C++ tree in PR #<n>.

Adds the Rust migration workspace (`.plan/rust-architecture-r1.md`) beside the
still-authoritative C++ tree, without touching or removing any C++ code:

- root virtual Cargo workspace (`resolver = "3"`, `edition = "2024"`,
  `rust-version = "1.98"`), workspace-wide lints forbidding `unsafe_code` and
  enabling `clippy::all`/`clippy::pedantic`, and pinned exact
  `[workspace.dependencies]` versions for the crates named in the migration
  architecture
- `rust-toolchain.toml` (stable 1.98.1 with `clippy`/`rustfmt`), `rustfmt.toml`,
  `deny.toml` (MIT/Apache-2.0/BSD/Zlib/Unicode-3.0/etc. allow list, GPL/LGPL/
  AGPL denied by omission), and `.cargo/config.toml` (`cargo xtask` alias)
- `ohl-core`: `no_std` + optional `std` feature crate providing a
  `SanitizedError` diagnostic type whose `Display` output is always a fixed,
  project-defined string, bounded checked-arithmetic helpers, and a streaming
  SHA-256 wrapper over the pinned `sha2` crate, covered by the FIPS 180-4 test
  vectors (empty string, `"abc"`, and the two-block message)
- `ohl-app`: the `open-half-life` Rust binary (clap-based CLI, `--version`
  stamped from `OHL_VERSION` at build time the same way the C++ binary reads
  `OHL_VERSION_OVERRIDE`, `--iso PATH` accepted but reporting that media
  import is not implemented in the Rust build yet with exit code 2,
  `tracing`-based `[level] message` logging to stderr mirroring
  `ohl::core::log`, and a `Platform: <OS> <arch>` line mirroring
  `ohl::platform::to_string`)
- `xtask`: `cargo xtask policy` reimplementing `cmake/CheckRepository.cmake`'s
  tracked-file rules (private-path prefixes, prohibited extensions, the 50 MiB
  ceiling, and the `MZ`/`MSCF`/`IWAD`/`PWAD`/`PACK` magic-byte check) with unit
  tests against temporary Git repositories, and `cargo xtask graph` validating
  the crate dependency graph against the full allowed-edge table from the
  migration architecture (most edges reference crates that do not exist yet,
  so the check currently validates `ohl-core` and `ohl-app` and will start
  covering later crates automatically as they are added)
- CI: new `rust-fmt`, `rust-policy`, `rust-clippy` (three-OS), `rust-test`
  (three-OS, `cargo-nextest`), and `rust-deny` jobs alongside the unchanged C++
  jobs, with the shared `vergit` version passed through as `OHL_VERSION`
- docs: `PROMPT.md` (Rust 2024/Cargo, winit, wgpu with Vulkan on Linux/Windows
  and Metal on macOS, a "No FFI" clean-room rule), `README.md` (Rust build
  section), `CONTRIBUTING.md` (Rust validation steps), `THIRD_PARTY_NOTICES.md`
  (Rust crate inventory pointer)

At the time this package landed, the C++ tree, its CMake/Ninja build, and
`cmake/CheckRepository.cmake` were unchanged and remained the accepted,
authoritative M0/M1 implementation; this package added a parallel,
not-yet-feature-complete Rust workspace per the two-step transition plan in
`.plan/rust-architecture-r1.md` section 4. That C++ tree, and
`cmake/CheckRepository.cmake` itself, were removed once R3 reached the same
milestone (`cargo xtask policy` reimplements its rules).

## R3: Rust M1 parity and C++ removal

Status: accepted; evidence: PR #<n> ("Reach M1 parity in Rust and remove the
C++ implementation"), plus the `ohl-vfs` and `ohl-media` crates merged
immediately before it.

- R3.1-R3.3: `ohl_platform::MediaSource` pinning on all three tuples,
  `ohl-iso9660`/`ohl-udf` preflight wrappers over pinned `hadris-iso`/
  `hadris-udf` 2.3.0, and the fingerprint/`ValidatedMedia`/provenance-cache
  crate (`ohl-media`)
- R3.4: `ohl-vfs` mounts, path normalization, bounded paged enumeration, and
  the move-only directory cursor
- R3.5: `ohl-app`'s M1 CLI (`--iso`/positional/prompt, `--cache`, sanitized
  logging identical to the removed C++ binary's) and the C++/CMake removal
  described under M0-M2 above

See M0 and M1 above for the reproduced acceptance criteria and M2 for the
Rust crates this package's dependencies (`ohl-vfs`, `ohl-media`) contribute
toward that milestone.

## M3 (Rust): first light

Status: in progress. A BSP v30 map with its baked lightmaps renders in a
window on a machine with a GPU; only the offscreen path has been verified in
this environment (see "Verified" below). Package M3.4 adds sky, liquids,
brush/studio render modes, light styles and SPR sprites (below); submodels
1.., texture animation and backface culling remain open (see "Not yet done").

Adds two crates and one development-only flag:

- `ohl-world`: turns an `ohl_formats::bsp30::Bsp` into an owned, GPU-ready
  `WorldModel` — triangle-fan face geometry following surfedge winding,
  texture coordinates from the texinfo axes, per-face lightmap extents
  computed at the documented 16-unit luxel spacing and packed into one
  RGBA8 atlas by a shelf packer, embedded miptexes decoded to RGBA (index 255
  keyed transparent for `{`-prefixed names), external textures resolved from
  caller-supplied WAD3 packages and otherwise replaced by a checkerboard
  placeholder, a decompressed potentially-visible set, a frustum test, and
  the `info_player_start` origin and facing. `WorldModel::build_submodel`
  builds any other submodel (a brush entity's `"*N"` `model` key) the same
  way, as its own standalone `WorldModel`; this crate does not parse entity
  keys itself (that is a future milestone's job), so a caller supplies both
  the submodel index and the entity's placement transform.
- `ohl-render`: a wgpu 30 renderer — Vulkan on Linux/Windows and Metal on
  macOS with a fallback to `wgpu::Backends::PRIMARY`, WGSL shaders
  multiplying the diffuse texture by the lightmap in GoldSrc's overbright-free
  gamma space, one vertex buffer plus a per-frame index buffer grouped into
  per-texture batches, a per-batch bind group with the lightmap atlas bound
  globally, a `Depth32Float` buffer, a WASD/mouse free-fly camera in GoldSrc
  units (Z-up world, right-handed view, `0..1` clip depth), resize handling,
  and an offscreen render-to-texture path with CPU readback.
- `ohl-app`: `--dev-bsp PATH [--dev-wad PATH]...` behind the non-default
  `dev-tools` cargo feature. It opens a winit 0.30 window on the renderer,
  quits on Escape, and logs a frame-rate line every two seconds. It loads a
  map straight off disk and therefore **bypasses the media pipeline** (no ISO
  validation, import, cache or VFS); it is a development aid only and is
  absent from release builds, which is why the feature is off by default.
  Neither the supplied paths nor any map-derived count appears in a log line:
  the project's sanitized-logging policy is applied uniformly here too.

Package M3.4 adds renderer polish on top of the above, all clean-room from
the public sources recorded in `docs/FORMAT_SOURCES.md`, "Rendering
conventions":

- `ohl-world`: `sky::SkyboxAsset` decodes the six documented
  `<skyname><suffix>.tga` faces (`image` `=0.25.10`, `tga`/`bmp` features
  only) into one owned RGBA8 image per face; `sky::is_sky_texture` excludes
  `sky`-prefixed faces from the opaque world batches instead of drawing
  them as ordinary textures. `water::is_liquid_texture` classifies
  `!`-prefixed and `laser`/`water`-family surfaces as liquids, routed to a
  translucent batch list kept separate from the opaque one. `sprite::
  SpriteAsset` decodes SPR frames to RGBA8 applying the documented
  per-format alpha convention and exposes the documented billboard `type`
  and sync mode; sprite frame timing advances at the declared `framerate`
  capped at the documented 10 Hz engine tick, defaulting to 10 fps.
  `WorldModel::blend_lightmap` now blends up to four per-face light styles
  (`Face::styles`) against the baked atlas using a caller-supplied
  intensity table instead of always sampling style 0.
  `WorldModel::build_draw_list_for_model` fills a `DrawList` with every face
  of a submodel unconditionally (no PVS or frustum culling): GoldSrc draws
  a submodel entity whenever the *entity* itself is visible rather than
  culling its faces leaf-by-leaf the way worldspawn is, and this crate does
  not parse entity visibility yet, so the conservative default is to draw
  the whole submodel.
- `ohl-render`: a sky pass draws the cubemap at infinite depth behind all
  other geometry; a liquid pass draws the translucent batches after opaque
  geometry with depth testing but no depth write, perturbing UVs by
  `water::turbulence_offset` (mirrored in `world_water.wgsl`). `RenderProps
  { mode, amount, color, fx }` reproduces the documented `rendermode` enum
  (`Normal`/`Color`/`Texture`/`Glow`/`Solid`/`Additive`) and maps each
  variant to one of three precompiled blend pipelines (opaque, alpha-blend,
  additive). `WorldRenderer::draw_world_submodel` draws one placed
  `SubmodelInstance` (a submodel `WorldModel` plus its entity transform)
  with `RenderProps`' blend state, into the same colour/depth target the
  opaque world pass already rendered into that frame: it builds the
  submodel's vertex/index buffers and its own texture/lightmap bind groups
  fresh on every call rather than caching them (brush entities are
  typically small, and this keeps the first-light implementation simple),
  and pre-multiplies the camera's view-projection by the entity transform
  on the CPU rather than uploading the transform separately. It draws only
  a submodel's opaque batches; a submodel's own liquid faces are not yet
  drawn by this call. `light_styles::LightStyles` evaluates `a`..`z`
  pattern strings at a fixed 10 Hz, seeded with the documented default
  patterns for styles `0..=11` (see `docs/FORMAT_SOURCES.md`, "Rendering
  conventions").

Verified:

- headless: the offscreen path renders the project-authored synthetic room
  (six lit faces, one embedded and one WAD3 texture, two leaves with a real
  compressed visibility lump) to an RGBA buffer and asserts the frame is lit,
  using whatever adapter the host offers; on a machine with none, the test
  skips instead of failing. It is `#[ignore]`d by default, with an
  `OHL_RENDER_GPU_TEST=1` opt-in, so CI runners without GPUs stay green.
  Three further gated offscreen tests cover M3.4: the sky pass fills the
  frame when nothing else is drawn, the liquid pass blends visibly over the
  cleared background, and `draw_world_submodel` blends a translucent
  (`RenderMode::Texture`) submodel visibly over the cleared background.
- on screen: **not verified** in the development environment used for this
  package, which has no display server. `--dev-bsp` was exercised there only
  as far as loading the map and reporting, through the sanitized error path,
  that no window system is available.

Not yet done: parsing entity keys (`ohl-app`'s `--dev-bsp` viewer does not yet
place any submodel or call `draw_world_submodel`; the entity's transform and
render properties must come from a future milestone's entity parsing), a
submodel's own liquid faces, texture animation (`+0`/`-0` frames), backface
culling (winding is not yet normalised across `plane_side`, so both sides are
drawn), mipmaps and anisotropy, sprite billboard *transforms* (the documented
`type` is exposed but `ohl-render` does not yet build a per-frame billboard
matrix from it), and any map source other than a path on disk.

## M3.3 (Rust): playable loop on the imported payload

Status: in progress. The engine loads a map out of an imported payload and
runs it: geometry, entities, map logic, collision, a walking player and a
composed frame. Verified headless in this environment against real imported
media; the windowed loop is unverified here (no display server).

Adds one crate and the production `ohl-app` path:

- `ohl-engine`: the `Game` state struct. It owns the worldspawn
  `ohl_world::WorldModel`, the brush-entity submodels an entity actually
  references, the `ohl_game::Registry` and `Simulation`, the
  `ohl_physics::CollisionModel` and `PlayerController`, the light-style
  table, the decoded skybox, the studio models the map's monster/prop
  entities reference, and (once a GPU context is attached) the renderer
  handles. It exposes exactly two verbs: `tick(dt, input)` advances the
  frame from a host-independent `Input` snapshot (clamped to a maximum
  step so a stalled frame cannot tunnel the player) and returns the
  `GameEvent`s the host must act on, and `render(target)` composes one
  frame: opaque world, studio models over the world's depth buffer, sky,
  brush-entity submodels through `draw_world_submodel` with each entity's
  `rendermode`/`renderamt`/`rendercolor`, then liquids. Light styles are
  re-blended every frame at the documented 10 Hz. A door's visual offset is
  derived from the `ohl-game` state machine's timer, because that crate
  models a door as timed state rather than a moving transform.
  `ohl_game::Event::LevelChange` surfaces as
  `GameEvent::LevelChange { map, landmark }`, and `Game::change_level`
  reloads the destination with the player placed at its landmark plus the
  offset it had from the same landmark in the map it left.
  Every asset arrives through the `AssetSource` trait, implemented by
  `AssetFsSource` over `ohl_assets::AssetFs` and by `MemoryAssets` for
  hosts and tests that already hold the bytes; the crate itself performs no
  I/O and logs nothing.
- `ohl-app`: the production playable path — `--map NAME`, `--training`,
  `--play`, `--headless-screenshot PATH`, `--frames N`, `--viewpoint
  X,Y,Z,PITCH,YAW` and `--spawn-offset DX,DY,DZ,DPITCH,DYAW`. It locates
  the published payload (through the medium's own provenance entry when an
  ISO is given, running the import first when nothing is published yet, and
  otherwise resolving the single published tree under the payload root),
  finds the directory inside it that holds the mod directories, mounts
  `AssetFs` with the default search path, and picks the start map from
  `ohl-campaign`'s sourced `STARTMAP`/`TRAINMAP` unless `--map` overrides
  it. With a display it opens a winit window (WASD, mouse look, `E` to use,
  backquote for the `ohl-ui` console, Escape to quit) with the HUD drawn
  over the frame; without one, `--headless-screenshot` renders `--frames`
  frames to an offscreen target and writes a 1280x720 PNG (the `image`
  crate with only its `png` feature enabled) before exiting 0.

Verified:

- headless, on real imported media: the campaign start map and the hazard
  course start map both load out of an imported payload and render to a
  non-empty PNG, as do several `--spawn-offset` viewpoints on the start
  map. Every frame carries thousands of distinct colours over more than
  90% of its pixels, i.e. real lit geometry rather than the clear colour.
- headless, on synthetic fixtures: `ohl-engine`'s own offscreen test
  renders a whole composed frame of the project-authored synthetic map, and
  `ohl-app`'s CLI test drives the built binary against a synthetic payload
  tree and asserts the PNG it writes is lit. Both follow the project's
  `OHL_RENDER_GPU_TEST=1` opt-in convention so GPU-less CI stays green.
- unit: ticking advances the simulation clock and clamps an overlong frame,
  `use` opens the door in front of the player, a `trigger_changelevel`
  reached through a button surfaces `GameEvent::LevelChange`, a level
  change preserves the player's offset from the shared landmark, and a
  missing destination leaves the current level running.
- on screen: **not verified** in the development environment used for this
  package, which has no display server.

Not yet done: sprites for `env_sprite`/`env_glow` entities (`ohl-world`
decodes `SpriteAsset` but `ohl-render` has no sprite pass yet, so nothing
places one), studio-model sequence selection (every placed model plays
sequence 0), full level-transition state (only the player's position
carries across a `changelevel`; inventory, entity state and global
variables do not), HUD values (health and armour are placeholders, not
driven by gameplay), audio, and any console command bound to the game.

## M4 (Rust): movement

Status: in progress. Package M4.1 adds collision hulls and a walking player;
the remaining M4 packages (entity-driven brush models, ladders, trains and
the rest of the movement environment) are not started.

Adds one crate and extends the development-only viewer:

- `ohl-physics`: clean-room clip-hull tracing and player movement, `no_std`
  plus `alloc`. `CollisionModel::from_bsp` validates every plane, clip-node
  child, leaf and head-node index once, then `trace` sweeps a segment through
  any of the four documented hulls (point, standing 32x32x72, large 64x64x64,
  crouched 32x32x36) with the classic recursive plane-clipping walk, a 1/32
  unit epsilon, and a traversal depth limit so a cyclic hull tree costs a
  bounded amount of work instead of overflowing the stack. On top of it,
  `player_move` runs one fixed movement tick: ground categorization with a
  0.7 slope limit, friction with the near-edge multiplier, ground and air
  acceleration with the 30 unit/s air cap, jumping, an 18-unit step move,
  a 4-bump slide, ducking, a basic swimming mode and noclip. Every tunable
  lives in `MoveConfig`; the values are community-documented defaults that
  still have to be verified against the real game (see
  `docs/FORMAT_SOURCES.md`, "Collision hulls and player movement").
- `ohl-app`: `--dev-bsp` now starts in a walking mode driven by
  `PlayerController` at a fixed 100 Hz tick, spawning at the map's
  `info_player_start` with a 28-unit standing (12-unit ducked) eye height.
  `N` toggles noclip, `V` switches back to the free-fly camera, which stays
  fully available. Both modes share the mouse look and WASD keys; nothing
  outside the `dev-tools` feature changed.

Verified: 32 tests in `ohl-physics` covering floor, wall and open-space
traces against analytic expectations, start-solid and all-solid detection,
hull selection, a rejected malformed map, a cyclic tree, gravity settling,
a 45-unit jump apex, the 18-unit step succeeding where a 19-unit ledge
fails, walkable versus too-steep slopes, friction decay, the air speed cap,
ducking, swimming, noclip, and four `proptest` properties (traces always
report a fraction in `0..=1` whose end position lies on the segment, and
movement never produces a non-finite state or ends inside solid).

Not yet done: brush-entity (submodel) collision, ladders, conveyors and
push volumes, water currents, the duck transition delay, and any verification
of the movement constants against the real game.

## M5 (Rust): entities

Status: in progress (M5.1). Adds `ohl-game`, the entity registry and a
minimal, deterministic map logic simulation, and wires both into the
`ohl-app` development viewer.

- `ohl-game::keyvalues`: lenient, bounded, never-panicking conversion of
  `ohl_formats::bsp30::entities::parse`'s raw key/value maps into typed
  `EntityDef`s (classname, origin, angles, targetname/target, spawnflags,
  `model` as a brush index or asset path, `rendermode`/`renderamt`/
  `rendercolor`), plus a `worldspawn` `wad` list parser. Unknown keys are
  preserved; malformed or oversized fields fall back to a default or a
  bounded truncation rather than rejecting the entity. Covered by
  `proptest` (never panics on arbitrary input; keyvalue strings and `wad`
  lists round-trip).
- `ohl-game::registry`: a `hecs::World` populated from the parsed entities,
  with components for classname, transform, brush-model index (and its
  precomputed bounding-box centre), targetname/target, spawnflags, render
  properties, and `Door`/`Button`/`Platform` (`func_door`, `func_button`,
  `func_plat`), `Light` (`light`/`light_spot`/`light_environment`),
  `PlayerStart`, `Landmark`/`ChangeLevel` (`info_landmark`,
  `trigger_changelevel`), `Path` (`path_corner`/`path_track`),
  `MultiManager` and a generic `Trigger` for other `trigger_*` classnames,
  falling back to `Unknown`. A bounded `targetname -> entities` index
  supports name-based lookups; `worldspawn`'s `skyname` and `wad` list are
  parsed into their own component.
- `ohl-game::brush`: gathers one `ModelInstance` (model index, transform,
  render properties) per brush entity for a renderer to draw. `ohl-world`
  gained a matching, additive `build_draw_list_for_model` (submodel 1..
  geometry, fullbright since no per-submodel lightmap atlas exists yet);
  `ohl-app`'s viewer does not yet call it (see "Not yet done").
- `ohl-game::logic`: a `Simulation` with a bounded `Fire { target, delay }`
  event queue, name-index dispatch, door/button/platform
  closed/opening/open/closing state machines driven by `speed`/`wait`/`lip`
  and a movement direction derived from `angles`/`angle` (including the
  `-1`/`-2` "straight up"/"straight down" sentinel), `multi_manager` fan-out,
  `trigger_once`/`trigger_multiple` dispatch (including the `wait` cooldown),
  and `trigger_changelevel` emitting a `LevelChange { map, landmark }` event
  for the caller to act on. No rendering, physics, AI or combat.
- `ohl-app` (`dev-tools`): after loading a map, builds the registry and
  simulation from its entities lump and submodel bounding boxes, ticks the
  simulation every frame, and presses of `E` `use` the nearest door/button
  within 64 units of the active eye position — the walking player's eye
  when `V` has walking mode engaged (see M4 above), otherwise the free-fly
  camera — preferring a brush entity's bounding-box centre over its
  usually-zero `origin` keyvalue.

Verified: `ohl-game`'s unit and property tests (parsing, registry
construction and name-index lookups, door open/wait/close timing, a button
firing a door target after a delay, `multi_manager` fan-out ordering,
`trigger_multiple`'s `wait` cooldown, `trigger_changelevel`'s event) and
`ohl-world`'s new `build_draw_list_for_model` tests, against the project's
existing synthetic BSP fixture. `E` was not exercised on screen (same
no-display-server limitation as M3).

Not yet done: `ohl-render` has no draw path for `ModelInstance`s yet, so
brush entities (doors, buttons, platforms) are not visually drawn or
animated in the viewer even though their state machines run; `func_door_rotating`
is dispatched through the same `Door` component as a translating door rather
than a rotation; damage-triggered doors/buttons (`health`), sounds, and
`momentary_door`/`func_train`/monster-driven `path_corner` following are not
implemented.

## M6 (Rust): models and animation

Status: in progress. Studio models (MDL v10) load, skin, animate and render
alongside world geometry; only the offscreen path has been verified in this
environment (see "Verified" below).

Adds one module to each existing renderer crate plus one development-only
flag; the BSP pipeline is untouched.

- `ohl-world`: `StudioModel` turns an `ohl_formats::mdl10::Mdl` into owned,
  indexed triangle geometry. Each body part's sub-models and meshes are
  triangulated from the documented strip (`N > 0`) / fan (`N < 0`) trivert
  command stream, with identical `(vertex, normal, s, t)` tuples collapsed
  into shared vertices; every vertex carries its single bone index, its
  normal, and texture coordinates normalised by the referenced texture's
  own width and height. Textures are decoded from 8-bit indexed pixels plus
  the trailing palette to RGBA, with palette index 255 keyed transparent for
  `STUDIO_NF_MASKED` textures, and their `STUDIO_NF_*` flags
  (chrome/additive/masked/fullbright and the rest) are published per
  texture. Skin families remap each mesh's texture slot. `StudioPose`
  samples a sequence at a wall-clock time: the frame index advances at the
  sequence's own `fps`, the fractional part interpolates linearly between
  two adjacent frames (positions lerped, rotations normalised-lerped along
  the shorter arc), a `STUDIO_LOOPING` sequence wraps and any other holds
  its last frame, and per-bone local transforms are composed along the
  parent chain into model-space matrices. Hitbox and attachment transforms
  are exposed for later packages. Bone controllers and multi-animation
  blending stay at their defaults (blend 0 only), and sequences stored in an
  external sequence-group file fall back to the bind pose.
  `WorldModel::ambient_at` returns an approximate ambient colour for a point
  by averaging the mean lightmap colour of the faces in its BSP leaf.
- `ohl-render`: a studio pipeline separate from the world one. WGSL does the
  skinning (one bone per vertex, up to 128 bone matrices in a per-instance
  uniform buffer), lighting is a per-vertex Lambert term against one
  directional light plus the caller-supplied ambient, chrome textures ignore
  their stored coordinates and use a view-space sphere-map approximation
  instead, masked textures alpha-test in the fragment stage, and additive
  textures go through a second, depth-write-disabled pipeline drawn after
  the opaque meshes. A `ModelInstance` list is drawn after world geometry
  into the same colour and depth target (the world renderer now exposes its
  depth view for exactly this), or into its own cleared depth buffer when
  there is no map.
- `ohl-app`: `--dev-mdl PATH` behind the same non-default `dev-tools` cargo
  feature. It opens a window on the model, plays the current sequence at its
  own frame rate, and cycles sequences with `[` and `]`. Combined with
  `--dev-bsp` it loads the map too and places the model at the map's player
  start; on its own the model orbits in front of the camera. Like
  `--dev-bsp` it loads files straight off disk and therefore **bypasses the
  media pipeline**, and neither the supplied paths nor any model-derived
  count or index appears in a log line.

Approximations, documented here because they are deliberate rather than
incidental:

- entity lighting is the leaf-average lightmap colour at the model's origin,
  not GoldSrc's downward trace onto a specific surface;
- shading is per vertex, not per pixel, and uses one directional light;
- chrome is a view-space sphere map with this project's own scale and bias,
  matching the reviewed community description of the mode rather than any
  published formula (see `docs/FORMAT_SOURCES.md`).

Verified:

- headless: the offscreen path renders the project-authored synthetic model
  (two triangles, one 16x16 texture, a two-bone chain and a two-frame
  compressed animation sampled halfway between its frames) into an RGBA
  buffer and asserts that a meaningful part of the frame is not the cleared
  background, using whatever adapter the host offers; on a machine with
  none, the test skips instead of failing. Like the world render test it is
  `#[ignore]`d by default with an `OHL_RENDER_GPU_TEST=1` opt-in.
- unit and property tests: triangulation counts, texture-coordinate ranges,
  the bone parent chain, pose interpolation continuity and midpoint values,
  looping versus held playback, and a `proptest` that model building and
  pose sampling never panic for any sequence description, frame or playback
  time the synthetic fixture can be rewritten to hold.
- on screen: **not verified** in the development environment used for this
  package, which has no display server. `--dev-mdl` was exercised there only
  as far as loading the model and reporting, through the sanitized error
  path, that no window system is available.

Not yet done: bone controllers and mouth control, multi-animation sequence
blending, sequence transition graphs and events, external sequence-group and
external texture files, per-pixel lighting, backface culling, mipmaps, and
any model source other than a path on disk.

## M7 (Rust): combat

Status: in progress. Package M7.1 adds `ohl-combat`, the combat skeleton the
remaining M7 packages (weapons, projectiles, pickups, monsters, player
systems) build on. See `.plan/m7-design.md` for the package breakdown and
`docs/FORMAT_SOURCES.md`, "Combat and damage", for the sources.

- `damage`: `DamageType`, a bitmask over Half-Life's published damage-type
  vocabulary (generic, crush, bullet, slash, burn, freeze, fall, blast, club,
  shock, sonic, energy beam, drown, paralyze, nerve gas, poison, radiation,
  acid, slow burn, slow freeze); `DamageInfo` (attacker, inflictor, amount,
  type, origin, direction); `Health` and `Armor` components; and
  `apply_damage`, which splits a hit between armour and health according to a
  caller-supplied `ArmorRule { ratio, bonus }`, reports `health_lost`,
  `armor_lost` and a `killed` flag that is set only on the transition into
  death, and rejects zero, negative and non-finite amounts through
  `SanitizedError::InvalidInput`. A local `Difficulty` enum plus
  `DifficultyScale` provide the per-skill-level scaling hook; `Difficulty`
  mirrors `ohl_campaign::Difficulty` rather than depending on it, so
  `ohl-combat` keeps the crate edges the M7 design gives it.
- `trace`: `trace_attack` resolves a shot by tracing `ohl-physics`' point
  hull through the world and then refining against a `HitboxIndex` — a
  bounded, flat list of entities and their posed hitboxes that the caller
  rebuilds each tick, so hit resolution never touches an ECS world. Each
  entity's hitboxes come from `StudioPose::hitbox_bounds` and are treated as
  oriented boxes in the entity's own space; the nearest impact, world or
  entity, wins, and the result carries the entity, the hitbox index, its
  published `HitGroup` and the surface normal.
- `events`: a bounded `CombatEventQueue` of `DamageDealt`, `Killed` and
  `Impact` events, drained by the composition root later. `ohl-combat` has no
  dependency on `ohl-render`, `ohl-audio` or `ohl-ui`.

Every behavioural constant the real game has but no usable public source
documents — the HEV absorption split, per-hit-group multipliers,
per-difficulty scaling — is a field of a caller-supplied parameter struct
whose default is neutral and is marked "to be black-box observed" in
`docs/FORMAT_SOURCES.md`; no unpublished number is shipped.

Verified: `ohl-combat`'s unit and property tests against the project's
synthetic collision-room BSP fixture and a hand-built two-bone hitbox pose —
a shot at head height reports the head hitbox and one at chest height the
chest hitbox, a ledge in front of the target takes the shot instead, a shot
aimed past the target misses it, the nearer of two targets wins, a rotated
box is hit through its orientation, and `proptest` shows a trace never panics
and always reports a fraction in `0..=1` with its impact point on the traced
segment, while damage application never restores health or armour and never
removes more than the target had.

Not yet done: everything else in M7 — the weapon table and firing state
machines, projectiles and radius damage, ammo and inventory, `ohl-ai`,
per-monster definitions, and the player systems (fall damage, drowning,
flashlight, long jump) with their save sections.

## M8 (Rust): save container

Status: in progress. Adds `ohl-save`, a project-owned, versioned save-file
container: a fixed magic, a format version, a bounded header (game version,
creation time, map identity, chapter/title, and a reserved thumbnail slot), a
tagged section table with a per-section SHA-256 digest, the section
payloads, and a whole-file SHA-256 trailer. **This is not the id
Tech/GoldSrc `.sav`/`.hl1` save format**; it is a from-scratch binary
container designed for this project (see `crates/ohl-save/README.md` and
`docs/ARCHITECTURE.md`'s "Save files" paragraph for the exact layout).

`SaveWriter::begin(header)` → `add_section`/`add_section_serde` (the latter
encoding with `postcard`) → `finish(&limits)` produces the bytes.
`SaveReader::open(bytes, &limits)` validates every offset, length, and
digest against the file size and the caller-supplied `Limits` before
trusting them, then exposes `header()`, `sections()`, `section(tag)`, and
`deserialize::<T>(tag)`. A major format-version mismatch is always rejected;
a minor-version mismatch and section-table entries whose tag is reserved for
this crate's own future use are tolerated and counted rather than causing a
failure. `SaveSlot` layers a directory of `<slot>.ohlsave` files on top, with
`AUTOSAVE_SLOT_NAME`/`QUICKSAVE_SLOT_NAME`, atomic write-to-temp-then-rename
publication, bounded `list()`, and `delete()`.

Verified: unit tests cover round-tripping, every-field tamper (header,
table, section digest, trailer), truncation at every byte length including a
dedicated 64-byte-boundary sweep, limits enforcement, unknown-section
skipping, and minor/major version rules; `proptest` checks that opening
never panics on arbitrary bytes and that arbitrary headers/sections
round-trip exactly; `tests/integration.rs` exercises the same guarantees
through the public API, including `SaveSlot` listing over a temporary
directory. A standalone `fuzz/` package (`open_fuzz`, `roundtrip_fuzz`) ran
60 seconds each with no crashes during development.

Not yet done: wiring `ohl-save` into `ohl-game`/`ohl-app` to actually
serialize and restore world/entity state; no section tags are defined by any
other crate yet, so this milestone is the container format only.
## M8 (Rust): campaign data and text formats

Status: in progress. M8.1: bounded, never-panicking parsers for the plain-text
game/data files GoldSrc/Half-Life loads alongside its binary assets, plus a new
crate carrying the sourced single-player chapter/map sequence and a
`skill.cfg`-backed difficulty table.

- `ohl-formats` gains five new modules (`titles`, `sentences`, `skill_cfg`,
  `liblist`, `hud_sprites`), each a bounded, `no_std` + `alloc` decoder with
  its own `Limits` struct, sharing a small internal bounded line-splitter
  (`text_lines`). Every module is covered by hand-written unit tests plus a
  `proptest` "never panics on arbitrary bytes" property
  (`crates/ohl-formats/tests/text_formats.rs`) and a matching `cargo fuzz`
  target under `crates/ohl-formats/fuzz/`. See `docs/FORMAT_SOURCES.md`
  ("Game text formats") for the public documentation each parser was
  implemented from.
- `ohl-campaign` (new crate, `no_std` + `alloc`, depends only on
  `ohl-core` per `xtask/src/graph.rs`'s dependency policy): the sourced
  Half-Life chapter/map sequence (`CHAPTERS`, `chapter_of`, `next_chapter`),
  `STARTMAP`/`TRAINMAP` defaults, a `Difficulty` enum
  (Easy/Medium/Hard → `skill.cfg` suffix 1/2/3), and `SkillTable`, a
  difficulty-aware lookup built from already-parsed `(cvar, value)` pairs
  (deliberately not from `ohl-formats` types directly, to keep the
  dependency edge to `ohl-core` only). See `docs/FORMAT_SOURCES.md`
  ("Campaign map sequence") for per-row citations.
- Three items from the M8 research pass are flagged **to verify** rather
  than encoded as confirmed facts: Interloper's starting map prefix
  (sources disagreed; left as an empty map list), `env_global`/save-file
  global-state-variable semantics (not modeled), and player-inventory
  persistence across `changelevel` (not confirmed from a public page). See
  `crates/ohl-campaign/src/lib.rs`'s module documentation.

Not yet done: wiring either crate into `ohl-game`/`ohl-import`'s campaign
bootstrap, save/restore semantics, and the two open citation items above.

## M9 (Rust): UI shell

Status: in progress. `ohl-ui` adds an egui-based overlay: a Quake-style
developer console, a data-driven HUD and a menu skeleton, plus the
`UiLayer` adapter that turns winit window events and a wgpu frame into
rendered egui output. It depends only on `ohl-core` per the crate-graph
policy (`xtask/src/graph.rs`); `ohl-app` and `ohl-render` are not modified by
this package and will wire it up in a later change.

- `UiLayer` (`crates/ohl-ui/src/layer.rs`) owns the `egui::Context`, an
  `egui-wgpu::Renderer` and, in windowed mode, the `egui-winit::State` input
  bridge. `handle_window_event` reports whether egui consumed a winit event;
  `begin_frame`/`begin_frame_headless` start a pass, and
  `end_frame_and_render` tessellates, uploads textures/buffers and records
  the draw calls into a caller-supplied `wgpu::CommandEncoder` and
  `wgpu::TextureView`, applying egui's platform output (cursor icon,
  clipboard) back to the window when one exists. The headless variant needs
  no window at all, which is what the offscreen render test uses.
- `console`: a bounded (4,096-line) sanitized scrollback buffer, a
  `CommandRegistry` (`register`/`execute`/prefix-based tab completion), a
  typed and bounded `Variables` ("cvar") table with change callbacks, and a
  `Console` that ties them together with an input line, history navigation
  and the built-in `help`, `echo`, `set`, `quit` and `map <name>` commands.
  `quit` and `map` raise `ConsoleEvent`s rather than acting directly, so the
  host validates a map name against the asset index before switching levels.
- `hud`: `HudState` (health, armor, clip/reserve ammo, a decaying damage
  flash, a timed message/title, crosshair visibility) plus an egui-painter
  draw function scaled to the window; layout only, updated by the game.
- `menu`: `Screen` (`InGame`/`MainMenu`/`Pause`/`Console`) with the input
  capture rule each screen implies (`InGame` releases the keyboard, mouse
  and cursor to gameplay; every other screen captures them), a main/pause
  menu skeleton (New game / Load / Save / Options / Quit), an options pane
  (mouse sensitivity, volume, field of view, each bounded) and a bindings
  placeholder screen, all reporting intent through `MenuAction` rather than
  acting directly.

Verified:

- unit tests: the command registry (tokenizing, dispatch, unknown-command
  and empty-line errors, tab completion), variables (typed parsing, bounds
  rejection, change callbacks, unknown-name errors), the scrollback buffer's
  4,096-line bound and control-character sanitization, the console's history
  and tab-completion behaviour, HUD state clamping/decay/message countdown,
  and the menu screen's input-capture state machine.
- headless: an offscreen test renders one frame of the HUD plus an open
  console with scrollback text into an RGBA target and asserts a meaningful
  part of the frame differs from the cleared background. Like the renderer's
  own offscreen tests it is `#[ignore]`d by default with an
  `OHL_RENDER_GPU_TEST=1` opt-in, using `ohl-render` as a dev-only dependency
  for the GPU device and readback helpers.
- on screen: **not verified**; wiring `UiLayer` into `ohl-app`'s window and
  render loop is left to a later change.

Not yet done: wiring into `ohl-app` (console toggle key handling, HUD/menu
composition with the world renderer, cursor grab/release on screen
transitions), persisting variables and options across sessions, load/save
screens, and an editable bindings screen.

## M9 (Rust): packaging

Status: in progress. `cargo xtask dist` builds the release `open-half-life`
binary and, on a Linux x86-64 host targeting Linux, the sandboxed
media-parser worker image (via the existing `worker-image` build path),
then assembles a versioned `target/dist/open-half-life-<version>-<target-
triple>/` folder and archives it as a `.tar.gz` (Linux/macOS, gzip via
`flate2`'s pure-Rust `rust_backend`) or `.zip` (Windows, deflate via the
`zip` crate). See [README.md](../README.md#release-builds) for the exact
layout.

- `licenses/` is generated by walking `cargo metadata`'s `license` field for
  every non-workspace dependency and bundling each crate's own
  `LICENSE*`/`COPYING*`/`NOTICE*` files from the local Cargo registry
  source cache when present.
- `SHA256SUMS` lists a SHA-256 digest for every file in the archive, in the
  conventional `sha256sum -c`-compatible format.
- No dependency in the packaging path links a C library: `flate2` is
  restricted to `rust_backend` and `zip` to its `deflate` method (its
  defaults pull in zstd/bzip2/lzma, several of them C or C-adjacent).
- CI's `release` job (`.github/workflows/build.yml`) runs `cargo xtask dist`
  on Linux, Windows and macOS runners for a `v*` tag push or a manual
  `workflow_dispatch`, and uploads each archive as a workflow artifact.
  Nothing is published to GitHub Releases yet.

Verified: unit tests cover the license walker (a hand-built `cargo
metadata` fixture, including dedup and excluding this workspace's own path
dependencies), the `SHA256SUMS` writer (against a real `sha256sum -c` run
during manual verification), and the release layout and archive writers
(assembled into a temp directory from dummy binary/license files, including
Unix permission bits and a round-trip read-back of both archive formats).
`cargo xtask dist` has been run end-to-end on Linux x86-64 and its output
verified with `sha256sum -c` and `tar`/`sha256sum` extraction.

Not yet done: publishing archives to GitHub Releases; verified
cross-compilation (the `--target` flag is wired but only exercised as a
best-effort passthrough, not proven against an actual cross toolchain in
CI).

## Later milestones

- M3: BSP rendering (Rust first light in progress, see above)
- M4: player movement (M4.1 hulls and walking in progress, see above)
- M5: interactive entities (Rust `ohl-game`, M5.1, in progress, see above)
- M6: models and animation (Rust studio models in progress, see above)
- M7: combat (Rust combat skeleton in progress, see above)
- M8: full campaign compatibility (Rust save container in progress, see above)
- M9: release hardening
