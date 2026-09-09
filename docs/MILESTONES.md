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
  conventions"). `WorldRenderer::draw_sprites` draws a batch of
  `SpriteInstance { asset, origin, scale, render_props, frame_time }`
  billboards into the same colour/depth target, depth-tested but never
  depth-written (per instance's own opaque/alpha/additive pipeline,
  selected the same way `RenderProps::blend_kind` selects a submodel
  pipeline): `SpriteType::ParallelUpright`/`FacingUpright` stay upright
  (world +Z) and only rotate their facing axis (the latter facing the
  camera's position, the former its view direction);
  `Parallel`/`ParallelOriented`/unknown types fully align to the camera's
  own right/up axes; `Oriented` lies flat in the world XY plane, since this
  milestone's `SpriteInstance` carries no per-instance rotation to orient a
  fixed-angle sprite by. Each instance picks its frame with
  `SpriteAsset::frame_at` at the documented default 10 Hz (no per-instance
  framerate override yet). Opaque instances draw first; translucent ones
  draw back-to-front by camera distance, batching consecutive same-frame
  instances into one texture upload.

M3 fidelity, round 2 (`ohl-world`, `ohl-render`) addresses the first two
findings of the round-1 headless-capture review:

- Lighting ramp. Compiled lightmap samples were copied into the RGBA8 atlas
  verbatim and multiplied raw in the shader, which left every frame 2.5-3x
  too dark and spent only ~20-30 of the 256 available code values.
  `ohl_world::LightRamp` (`{ texgamma, lightgamma, brightness, overbright }`,
  defaulting to the documented cvar defaults) is now applied to every luxel
  as its tile is packed, per light-style layer, so style blending is a
  weighted sum in the ramped space and `world.wgsl` stays a plain multiply.
  Exposed as `WorldBuildOptions::ramp`; `LightRamp::identity()` keeps the raw
  samples. Sources and the exact composition: `docs/FORMAT_SOURCES.md`,
  "Rendering conventions".
- Invisible brush entities. `RenderProps::from_entity` builds render
  properties from raw `rendermode`/`renderamt`/`rendercolor`/`renderfx`
  keyvalues and ignores `renderamt` for the two documented opaque modes
  (`Normal`, `Solid`), so the common mapper habit of leaving `renderamt` at
  its `0` default on a mode-0 brush entity no longer renders it invisible.
  `WorldModel::build_submodels` returns a `SubmodelSet` that pairs each
  built submodel with its `"*N"` index and reports every failure as data
  (`failure_count`), so a caller can no longer drop a submodel silently, and
  `WorldError::SubmodelOutOfRange` distinguishes "this map declares no
  geometry" from "this entity references a submodel the map does not have".
  `ohl-engine`'s `Level` counts the submodels it could not build and
  publishes the count as `Game::unbuildable_submodel_count`, alongside the
  existing missing-model count.

  Still open after this package: a `func_tracktrain`-class brush entity is
  drawn at its raw `origin` keyvalue rather than at the `path_track` it is
  targeted at, so the first chapter's tram car is built and drawn but ends
  up outside the visible area. That is entity/mover logic in `ohl-game`, not
  a renderer defect, and is left to the milestone that implements track
  movers.

Verified:

- headless: the offscreen path renders the project-authored synthetic room
  (six lit faces, one embedded and one WAD3 texture, two leaves with a real
  compressed visibility lump) to an RGBA buffer and asserts the frame is lit,
  using whatever adapter the host offers; on a machine with none, the test
  skips instead of failing. It is `#[ignore]`d by default, with an
  `OHL_RENDER_GPU_TEST=1` opt-in, so CI runners without GPUs stay green.
  Further gated offscreen tests cover M3.4: the sky pass fills the frame
  when nothing else is drawn, the liquid pass blends visibly over the
  cleared background, `draw_world_submodel` blends a translucent
  (`RenderMode::Texture`) submodel visibly over the cleared background, and
  `draw_sprites` brightens the centre of the frame with a synthetic
  `RenderMode::Additive` sprite. A further gated test
  (`headless_opaque_submodel_render.rs`) renders a `func_train`-like
  submodel (model index 1) built from `renderamt`-less mode-0 and mode-4
  keyvalues and asserts it occludes the darker worldspawn surface behind
  it, with an explicitly translucent (mode 2, `renderamt` 0) entity as the
  negative control.
- on screen: **not verified** in the development environment used for this
  package, which has no display server. `--dev-bsp` was exercised there only
  as far as loading the map and reporting, through the sanitized error path,
  that no window system is available.

Not yet done: parsing entity keys (`ohl-app`'s `--dev-bsp` viewer does not yet
place any submodel or call `draw_world_submodel`; the entity's transform and
render properties must come from a future milestone's entity parsing), a
submodel's own liquid faces, texture animation (`+0`/`-0` frames), backface
culling (winding is not yet normalised across `plane_side`, so both sides are
drawn), mipmaps and anisotropy, a per-instance rotation for
`SpriteType::Oriented` sprites (drawn flat in the world XY plane instead;
see `ohl-render`'s M3.4 entry above), and any map source other than a path
on disk.

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

Package M7.2 adds `ammo`, `weapons` and `firing` to `ohl-combat`, on top of
the M7.1 skeleton above; see `docs/FORMAT_SOURCES.md`, "Weapons and firing
(M7.2)" for its sources.

- `ammo`: `AmmoType`, Half-Life's twelve published ammunition classes, each
  with its published carry cap where a usable source states one (`Snarks`'
  cap does not and is an explicit, marked black-box placeholder); a bounded
  `AmmoPool` that never exceeds capacity and never goes negative.
- `weapons`: `WeaponId`'s fourteen weapons and a `const fn spec` table of
  `WeaponSpec`s (kind, damage, clip size, ammo type, cycle time, reload time,
  an optional secondary fire), one cited comment per entry; every value this
  project could not confirm on a usable source is wrapped in `BlackBox<T>`
  with a `// TODO(black-box)` marker instead of being invented.
- `firing`: `FiringState`, a deterministic per-weapon state machine
  (`Idle`, `Firing`, `Reloading`, `Charging`, `Beam`, `Holstered`) driven by
  `tick(dt, WeaponInput, &mut AmmoPool)`, producing `WeaponAction`s
  (`Hitscan`, `Melee`, `SpawnProjectile`, `BeamTick`, `PlaySequence`,
  `Sound`, `Empty`) and consuming clip or pool ammo as it goes; the gauss
  gun's published charge/overcharge rule (a release before 10 seconds scales
  damage 25..=200, a hold past 10 seconds deals 50 self-damage instead) is
  implemented directly. `resolve_hitscan` turns a hitscan action and
  `trace_attack` results into `DamageInfo` records at the spec's damage and
  damage type.

Verified: the weapon table's published numbers are asserted against the
design table; firing cycles (fire, then cannot fire again until the cycle
time elapses, then can), reloads (only when the clip is short and the pool
has ammo) and dry fire (no ammo anywhere) are covered by unit tests; the
gauss charge/overcharge behaviour and the hornet gun's regenerating-clip
placeholder each have a dedicated test; a `.357` shot and a shotgun blast
against a target in the project's synthetic collision room deposit their
documented per-hit damage through the same `trace_attack` /
`resolve_hitscan` / `apply_damage` pipeline the composition root will drive;
and `proptest` shows the state machine never panics over arbitrary weapons,
starting ammo and input sequences, that its ammo pool never exceeds capacity,
and that a gauss charge/release always yields a charge damage in `25..=200`,
a self-damage of exactly 50, or neither, never both.

Package M7.3 adds `projectile`, `explosion` and `deployables` to
`ohl-combat`; see `docs/FORMAT_SOURCES.md`, "Projectiles, explosions and
deployables (M7.3)" for its sources.

- `projectile`: a bounded `ProjectileSet` of crossbow bolts, RPG rockets
  (optionally steered toward a laser-designated point), MP5 and hand
  grenades (which arc and bounce), hornets (homing on primary fire, straight
  on secondary) and snarks (which hop toward the nearest entity in the hitbox
  index and bite it). Every projectile advances at the fixed tick under
  `MoveConfig`'s gravity by a *swept* hull-0 trace against the world refined
  against the same `HitboxIndex` hitscan uses, so nothing tunnels at any
  speed; bouncers keep sweeping with the time left over after each impact,
  with a documented placeholder restitution, and park once they settle.
  Reports `ProjectileEvent::{Impact, Detonate, Expired}`; damage stays the
  caller's job. The published hand-grenade five second fuse and snark
  ~20 second self-destruct are named constants.
- `explosion`: `radius_damage`, linear (and therefore monotonic) falloff from
  a blast centre, measured to the nearest face of a target's hitbox, with an
  all-or-nothing world line-of-sight check, a self-damage scaling hook and a
  blast pushback vector alongside each `DamageInfo`.
- `deployables`: `DeployableSet`, satchel charges the owner sets off together
  and tripmines placed from a world trace that arm after the published three
  seconds and then watch a beam cast along their own normal, both bounded by
  the published maximum of five each.

Verified: fuse and lifetime timings fire at the published times and not
before; rocket guidance converges where flying straight does not; hornet
homing never turns away from its target; a snark settles, closes and bites; a
tripmine arms exactly once and trips on the first hitbox to cross its beam;
satchels cap at five and detonate together; radius damage is monotonic,
spares occluded targets and honours the self-damage hook; `proptest` shows a
grenade launched at any speed up to 12000 units/s and any angle never ends a
tick inside solid, and that ticking is total; and replaying one seed with the
same inputs reproduces the event sequence exactly.

Package M7.4 adds `inventory`, `pickups` and a new `ohl-gameplay` crate on
top of the M7.1/M7.2 skeleton above; see `docs/FORMAT_SOURCES.md`, "Pickups
and chargers", for its sources.

- `inventory`: `Inventory`, the player's owned-weapon bitset (over
  `WeaponId`'s fourteen variants), a bounded `AmmoPool` per `AmmoType`
  (reusing `ammo::AmmoPool`'s published carry caps), per-weapon loaded
  clips, the current selection, and `hud_slot`, a project-authored (not
  published) HUD slot/position layout. `give_weapon`/`give_ammo`/`drop`/
  `holster`/`select_next`/`select_prev`/`select_slot` are all deterministic;
  `give_weapon` only unlocks a weapon, leaving ammo to the same
  `give_ammo` path an ammo-box pickup uses, so there is exactly one place
  that enforces a carry cap.
- `pickups`: `classify_classname`, mapping Half-Life's published
  `weapon_*`/`ammo_*`/`item_healthkit`/`item_battery`/`item_suit`/
  `item_longjump`/`func_healthcharger`/`func_recharge` classnames (TWHL
  wiki) to a `PickupKind`; `try_pickup`, which resolves a touch pickup
  against an `Inventory` and the target's `Health`/`Armor`, reporting
  `PickupOutcome { taken, remaining }` (a full pool or an already-owned flag
  item is not taken); and `ChargerState`, a use-and-hold reservoir model for
  `func_healthcharger`/`func_recharge` that drains to Combine OverWiki's
  published 50 HP / 75-50-35 (easy/medium/hard) totals. Every pickup
  *amount* this source does not publish (ammo box contents, healthkit/
  battery amounts, a weapon pickup's bundled ammo, the charger drain rate)
  is a `weapons::BlackBox` placeholder with a `// TODO(black-box)` marker,
  never invented as a plain number.
- `ohl-gameplay` (new crate; `ohl-core, ohl-combat, ohl-game, ohl-ui,
  ohl-audio`): `GameplayBridge`, which turns `ohl-combat`'s `CombatEvent`s,
  `WeaponAction`s and `PickupOutcome`s into `ohl_ui::HudState` updates
  (health/armor synced from the current `Health`/`Armor` rather than
  accumulated by subtraction, clip/reserve ammo from the current
  `Inventory`, the damage flash, and a pickup message), `SoundCue`s (a
  lightweight entity/channel-class/optional-asset-path record, not itself
  an `ohl_audio::PlayRequest`, since this crate never decodes a sound file
  and so has no `Arc<SoundBuffer>` to embed in one) and `ViewModelAction`s
  (`Draw`/`Idle`/`Fire`/`Reload`/`Holster`) for the later viewmodel
  animation work; both output queues are bounded, on
  `ohl_combat::CombatEventQueue`'s "drop and count the overflow" model.
  Every sound asset path this package ships is `None`: no source this
  project may use publishes Half-Life's sound file layout as reusable data,
  and `docs/CLEAN_ROOM.md` rule 7 requires a clean-room provenance review
  before any such literal enters source.

Verified: each published carry cap, the classname-to-`PickupKind` mapping,
each published charger total (50 HP; 75/50/35 suit power), a full ammo pool
correctly reporting a pickup as not taken, and a fixed weapon-pickup then
weapon-fire then damage-event sequence producing the exact expected
`HudState` (clip/reserve ammo, health, armor, damage flash, pickup message)
are covered by unit tests; `proptest` shows an arbitrary sequence of
give-weapon/give-ammo/select/drop operations never lets any `Inventory` ammo
pool exceed its capacity or go negative and never lets a clip exceed its
weapon's clip size.

Not yet done: per-monster definitions and the player systems (fall damage,
drowning, flashlight, long jump) with their save sections.

## M7 (Rust): monster AI core

Status: in progress (package 7.5). Adds `ohl-ai`, the decision-making half
of the monsters: what they perceive, what state that puts them in, which
schedule of tasks they run, how a squad shares an enemy, and the movement
glue that turns a task into clip-hull-traced motion. It renders nothing,
plays nothing and resolves no damage; `AiWorld::tick` returns an `AiEvent`
list for `ohl-app` and, later, `ohl-combat` to act on. Per the crate-graph
policy (`xtask/src/graph.rs`) it depends only on `ohl-core`, `ohl-physics`,
`ohl-world` and `ohl-game`.

- `state`: `MonsterState` (none/idle/alert/combat/hunt/prone/script/
  playdead/dead), the published `Classification` vocabulary, `Relationship`
  ordered so that `Ord` *is* the enemy-selection priority, the data-driven
  `RelationshipTable` with a small **provisional** default matrix, and a
  29-bit `Conditions` set (`SEE_ENEMY`, `SEE_HATE`/`FEAR`/`DISLIKE`/
  `NEMESIS`, `SEE_CLIENT`, `ENEMY_OCCLUDED`/`TOOFAR`/`FACING_ME`/`DEAD`,
  `NEW_ENEMY`, `HEAR_SOUND`/`DANGER`/`COMBAT`, `SMELL`, `LIGHT_DAMAGE`,
  `HEAVY_DAMAGE`, `CAN_MELEE_ATTACK1/2`, `CAN_RANGE_ATTACK1/2`,
  `TASK_FAILED`, `SCHEDULE_DONE`, `PROVOKED`, `NO_AMMO_LOADED`, `BLOCKED`,
  `IN_DANGER`, `SPECIAL1/2`).
- `senses`: `look()` filters candidates by look distance, the `ohl-world`
  potentially-visible set, the view cone and finally an `ohl-physics`
  point-hull trace from `origin + view_ofs` to the target's eye, then picks
  an enemy by relationship then distance; `listen()` scans a bounded
  `SoundList` scaled by `Senses::hearing_sensitivity`; `EnemyMemory` keeps
  the last known position and applies the published "occluded within 256
  units is still known" rule.
- `schedule`: a `Task` enum, a `Schedule` of `&'static [Task]` plus an
  interrupt mask, a `ScheduleRunner` that advances one task per tick after
  checking the mask, and the `Brain` trait (`select_schedule(state,
  conditions)`). `brain` supplies the project's own default schedule set and
  state machine.
- `squad`: `SquadRoster` groups monsters by `netname`, lets a `SquadLeader`
  recruit at most three members, and shares an enemy across the squad.
- `movement`: `Route` (already the waypoint-plus-cursor shape the node graph
  of package 7.6 needs, with a straight-line fallback today), `move_toward`
  with a step-up over 18-unit obstructions, and a `StuckDetector`.
- `spawn`: attaches `Actor`/`MonsterAi`/`SquadTag` to the entities
  `ohl_game::Registry` already spawned, so there is one entity world.
- `damage`: a deliberately minimal `DamageEvent`/`DamageSink` pair, to be
  replaced by `ohl-combat`'s richer `DamageInfo` when package 7.1 lands.
- `rng`: a project-owned `PCG-XSH-RR 64/32` generator written from the
  public PCG paper, so the AI has a save-restorable random stream with no
  third-party dependency.

Verified: unit tests per module; integration tests over a project-authored
synthetic BSP room divided by a wall show line of sight flipping idle to
combat in a single tick, the wall blocking a sight line the open room
allows, occlusion dropping the monster to alert while retaining the last
known position, an enemy occluded beyond 256 units being forgotten, a danger
sound interrupting the running schedule, a squad leader recruiting exactly
three members, and 1,000 ticks replaying to identical `state_hash` digests;
`proptest` checks that sight, hearing, the movement step, the route cursor,
the stuck detector and a full tick never panic and never produce non-finite
state for arbitrary positions.

Package 7.7 (`crates/ohl-ai/src/monsters`, `crates/ohl-ai/src/spawner.rs`)
adds the sixteen defined `MonsterKind`s (headcrab, zombie, houndeye,
bullsquid, alien slave, alien grunt, human grunt, barney, scientist, turret,
miniturret, sentry, ichthyosaur, leech, gargantua, tentacle, plus
`Unknown(classname)`), each with a `MonsterSpec` (health, melee/ranged
attack, hull, classification, blood, size class, door-opening, flags) behind
a `sk_<subject>_<property><N>`-style `SkillLookup` override hook; one
data-driven `MonsterBrain` covering all sixteen, with the new schedules
package 7.5's default set did not need (houndeye pack blast, bullsquid spit,
alien slave zap, grunt suppress/flank/grenade, barney/scientist follow,
scientist heal, turret deploy/retract/track, tentacle sound-driven strikes,
gargantua flame/stomp as a scripted-set-piece placeholder) and the
squad-blast-bonus/heal-cooldown math they need; `lifecycle::apply_damage`,
the sole place `Actor::health` is decremented today, guaranteeing exactly one
`Died` per kill, a corpse/gib decision on overkill, `Fade Corpse`, and the
eleven-value `TriggerCondition`/`TriggerTarget` pair; and `spawner::Spawner`,
`monstermaker`'s `monstercount`/`delay`/`m_imaxlivechildren`/`Start On`/
`Cyclic` semantics. **Every per-monster health and primary melee/ranged
attack-damage number is cited to that monster's own TWHL wiki page**
(`https://twhl.info/wiki/page/<entity>`); see `docs/FORMAT_SOURCES.md`,
"Monster definitions", for the per-row citation table, including the
houndeye's squad-blast-bonus numbers and the scientist's heal
amount/cooldown/range/threshold. Attack reach/range for most monsters, view
cones, look distances, movement speeds and every schedule's own timing
remain `TODO(black-box)`.

`monsters::integration` forward-declares the two minimal seams the concurrent
node-graph (7.6) and projectile/explosion (7.3) packages will fill:
`Navigator::next_move` (today: `StraightLineNavigator`) and
`RangedAttackSink::spawn` (today: `NoOpRangedAttackSink`), so a real
implementation drops in at the composition root later without this
package's data changing.

`monsters::nav_bridge::NavBridge` is that real 7.6 implementation, now wired
in: built once per map from `ohl-nav`'s `NodeGraph` (seeded from
`info_node`/`info_node_air`, via `node_seeds_from_defs` over `ohl-game`'s
typed `EntityDef`s) and attached with `AiWorld::attach_navigator`, it drives
`advance_route` in place of the straight-line stepper whenever one is
present. Per actor it caches an `ohl_nav::Path` plus its own `Steer` cursor,
rebuilt when the goal drifts past the cited 80-unit
`Route::needs_refresh` threshold, tries `straight_path_if_clear` before
spending one of a bounded per-tick `find_path` search budget (default 8,
shared across every actor so a tick stays bounded), and falls back to
`StraightLineNavigator` when the graph is empty, no path exists, or the
budget is spent this tick. Hull comes from the moving `Actor` (already keyed
off `MonsterSpec::hull`/`SizeClass`); node kind (ground/air/water) needs no
extra per-monster logic since `ohl-nav` already keeps ground and air/water
links in disjoint, hull-validated subgraphs. `Route` itself is unchanged —
it still only ever carries the path task's single ultimate goal, so
`WaitForMovement`/`StopMoving`/the determinism hash are unaffected by
whether a navigator is attached.

Not yet done: scripted sequences and save sections (7.8); unifying
`ohl-ai::damage`/`monsters::lifecycle` with `ohl-combat`'s `DamageInfo`
(7.1); substituting a real `RangedAttackSink` implementation once 7.3 lands;
observing every per-monster health/damage/range/timing number against
legally obtained retail software. Every movement speed, view-cone angle,
look distance, attack range, turn rate and damage threshold in the crate is
a placeholder still to be black-box observed; see
`docs/FORMAT_SOURCES.md`, "Monster AI behaviour" and "Monster definitions".

- **M7.10: track trains.** `ohl-game::track_train` resolves a
  `func_train`/`func_tracktrain`'s `target` into a bounded `PathChain` of
  `path_corner`/`path_track` nodes, places the train on its first node
  (height-adjusted, and — for a `func_tracktrain` only — facing the second
  node), and advances it along the chain at a fixed timestep, honouring each
  node's `wait` pause, `path_track`'s own `speed` override and "Wait for
  retrigger" stop flag, and a closed loop's wrap instead of a dead end;
  `toggle`/`turn_on`/`turn_off`/`reverse` are wired through the existing
  `Simulation::activate` "use" path shared with doors, buttons and
  platforms, and `ohl-engine`'s `render.rs` reads the resolved position/yaw
  each frame via a new `track_train_transform`, the same way it already
  reads a door's timer. Covered by unit tests (spawn placement, chain
  movement with node stop/wait/speed-override handling, reverse, loop) and a
  `proptest` showing the reported position always lies on the chain's
  polyline and is never `NaN`. See `docs/FORMAT_SOURCES.md`, "Track trains
  and paths", for sources and `TODO(black-box)` items (`bank`, `dmg` and
  `wheels` are recorded but not applied; `path_track`'s `altpath` branching
  is not implemented).
Not yet done: everything else in M7 — the weapon table and firing state
machines, projectiles and radius damage, ammo and inventory, `ohl-ai`,
per-monster definitions, and the rest of the player systems (item and
charger entities, `scripted_sequence` integration) — see "M7.8 (Rust):
player systems" below for the part that is done.

## M7.8 (Rust): player systems

Status: in progress. Package 7.8a of the M7 plan (`.plan/m7-design.md`
section 3): everything about the player that is not motion, plus the
movement modes M4 left out.

`ohl-physics` (additive):

- ladders. A player whose origin is inside a `CONTENTS_LADDER` volume
  attaches to it; the ladder's outward normal is found by probing the four
  horizontal directions for the nearest open face. Pressing into the face
  climbs up, pulling away climbs down, and the rest of the wish slides
  sideways along the ladder plane. Gravity does not apply while attached,
  catching a ladder in mid-air cancels the fall, jumping pushes off along
  the normal with a short re-attach lockout, and leaving the volume
  detaches.
- liquids. `categorize_liquid` reports both the documented `waterlevel`
  0..3 and *which* liquid it is (`LiquidKind::Water/Slime/Lava`), sampled
  from the leaf contents at the feet, the origin and the eye.
- riding movers. `MoveInput::base_velocity` is added to the player's
  velocity for the duration of one move and removed afterwards, so standing
  on a `func_plat`/`func_train` carries the player without the ride
  accumulating into their own velocity.
- the long jump. With the module owned, a jump pressed within
  `MoveConfig::long_jump_duck_window` of the duck key going down produces a
  flat forward+up impulse instead of a crouch jump.
- reporting. `player_move_events` returns a `MoveEvents` with the landing
  impact speed (suppressed when landing in a liquid or catching a ladder),
  the long jump, ladder attach/detach and water level changes.
  `player_move` is unchanged and now delegates to it.

`ohl-player` (new crate): health, HEV armor, suit/long-jump ownership, the
flashlight, `waterlevel`, the air timer and the damage-type flags; systems
for fall damage, drowning, `trigger_hurt` intake, slime/lava contact damage,
HEV suit voice events with per-occasion cooldowns and staggered delays, and
the flashlight's drain/recharge. `PlayerSystems::tick(dt, input,
physics_output, contents_query) -> Vec<PlayerEvent>` is the hook the game
loop package will call; nothing here draws or plays anything. Save/load goes
through one `ohl-save` section (tag `0x20`) with a `snapshot()`/`restore()`
pair that clamps every restored value.

`ohl-game` (additive): a `TriggerHurt` component (`dmg`, `damagetype`) and a
`Ladder` marker for `func_ladder`.

Every published constant is cited in `docs/FORMAT_SOURCES.md` under "Player
systems"; everything else is a neutral `TODO(black-box)` placeholder, listed
in the same section, that must be measured against legally obtained retail
software before parity is claimed.

Verified: ladder climb/descend/detach and the ladder normal in a synthetic
ladder room, the four water levels and slime/lava categorisation in a
synthetic pool, a platform ride carrying the player exactly one
platform-second, the long-jump window (module present, module absent, window
expired), a landing impact reported for a fall but not for a step or a
splash, the published fall-damage curve and armor split, the drowning timer
and its recovery, the half-second `trigger_hurt` cadence and its healing
case, suit events firing once per cooldown and never without the suit, the
flashlight's toggle/drain/auto-off/recharge, a save section that round-trips
byte-identically, and proptests that no input makes the systems panic or
drives health, armor, air or charge out of range.

Not yet done: wiring into the game loop (`ohl-app`/`ohl-engine` are owned by
other packages), item and charger entities, `scripted_sequence` integration,
and the per-entity health/AI save sections (`0x21`/`0x22`).

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

Not yet done: the two open citation items above.

## M8 (Rust): campaign flow

Status: in progress. M8.2 wires the M8.1 data and text formats into the
running game: level transitions that carry state, save/load over the
`ohl-save` container, chapter titles and HUD messages, and a
difficulty-selected `skill.cfg` table. See `docs/FORMAT_SOURCES.md`
("Campaign flow") for the public documentation these semantics were
implemented from.

- `ohl-game` gains (additively) a `globalname` component, brush bounds, a
  `trigger_transition` marker, `env_global` and `env_message`/`game_text`
  components, a worldspawn `newunit` flag, an `Event::Message` variant, a
  serializable `SimulationState` snapshot, and an optional `serde` feature
  that derives `Serialize`/`Deserialize` on every entity component.
- `ohl-engine` gains `transition` (a `TransitionState` carrying the
  player's landmark-relative pose, a `PlayerCarry` hook `ohl-player` will
  implement, entities inside the landmark's `trigger_transition` volumes or
  within `DEFAULT_CARRY_RADIUS`, the `globalname` state table and the
  previous map's modified mover states; `newunit` drops all of it, and a
  landmark missing from either map leaves the player at the destination's
  own `info_player_start`), `save`
  (a `GameSave` laid into `ohl-save`'s tagged sections, with
  `save_slot`/`load_slot`) and `text` (a `titles.txt` library, a
  `sentences.txt` `SentenceLookup`, and the `skill.cfg` reader feeding
  `ohl_campaign::SkillTable`). `GameEvent` grows `ChapterTitle` and
  `Message` variants.
- `ohl-app` gains `--load <slot>` and `--difficulty easy|medium|hard`, an
  autosave on level change, F6/F7 quicksave/quickload in the windowed loop,
  and shows chapter titles and `env_message` text in the HUD message area.
- Save section tags: engine header `16`, player carry `17`, entity registry
  `18`, simulation `19`, global state `20`, light-style time `21`, view
  `22`. A save read back and written again is byte identical.

Not yet done: real player/inventory carry (the `PlayerCarry` hook still
returns health/armor placeholders until `ohl-player` implements it) and the
three "to verify" citation items.

### M8.3 (Rust): all-chapters smoke

Status: done. `cargo xtask campaign-smoke --payload-root <dir>` builds (or
accepts a prebuilt) `open-half-life` release binary and headless-screenshots
every map in every `ohl-campaign` chapter, plus the Hazard Course training
maps, against an already-imported payload, with a per-map timeout and bounded
parallelism. Each run is classified (loaded/rendered, missing-map,
load-error, timeout, crash, or blank-capture) from the app's own exit code
and sanitized fixed log lines, never from payload-derived strings. A
capture's decoded size and non-background pixel fraction are checked only
against a fixed expected size and threshold to decide the blank-capture
classification; neither figure is ever written out. The markdown summary
written under `--out` reports chapter-level aggregate counts per category
only (no per-map row, and no pixel count, dimension, or per-map timing of
any kind); the command exits non-zero if any map failed to load.

Run once against a legally imported retail payload: all 93 maps across the
18 story chapters and the Hazard Course training maps loaded and rendered
successfully (93/93 pass, 0 fail), no missing-map/load-error/timeout/crash/
blank-capture results, in 347.1s total.

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

## M7 (Rust): monster navigation

`crates/ohl-nav` (package 7.6), the navigation layer the AI crate routes
with. It is a leaf over `ohl-core`, `ohl-physics` and `ohl-formats` and knows
nothing about entities, so `ohl-ai` can compose it without a cycle.

- `graph`: `NodeGraph::build` takes node positions and kinds (ground, air,
  water — a host extracts them from `info_node`/`info_node_air` entities, or
  uses `node_seeds_from_entities` on an already-parsed entities lump) plus a
  `CollisionModel`. Ground nodes are dropped onto the floor; air nodes are
  left where they are; ground and air nodes never link to each other
  (published behaviour, see `docs/FORMAT_SOURCES.md`, "Navigation"). Every
  candidate pair inside the link radius is validated once per hull — step up,
  sweep across, sample the floor, drop at the far end — so one graph serves
  the point, 32x32x72, 64x64x64 and 32x32x36 hulls with different answers.
  Construction is bounded (max nodes, links per node, candidate pairs),
  deterministic, and serialisable behind the `serde` feature so a host can
  cache it.
- `path`: A* per hull with a Euclidean heuristic, a bounded expansion
  counter, endpoint attachment by hull trace to the nearest reachable node,
  and a `straight_path_if_clear` shortcut for the open-room case.
- `steer`: `Steer::next_move` turns a path into a `MoveIntent`
  (`dir`, `speed_scale`, `reached`, `blocked`) with waypoint advancement and
  skip-ahead, plane sliding, two rotated side probes, a documented creep
  fallback, and a progress-window stuck flag telling the caller to re-path.

Verified: per-hull linking through a 40-unit doorway (the humanoid hull
passes, the large hull does not) and a 160-unit one; rejection of links
across an unsupported floor gap and through walls; ground snapping; the
construction bounds and determinism; A* agreeing with a brute-force Dijkstra
for every pair of a 5x5 lattice and on a hand-computed optimum; endpoint
attachment and its radius; the exploration bound; steering around a corner,
sliding along a wall and detecting a wedged mover in a fixed-tick
simulation; and proptests that building, pathing and steering never panic
for arbitrary positions and bounded limits, that graphs rebuild identically,
and that returned paths are contiguous.

Not yet done: `ohl-ai` still has to call this crate instead of its
straight-line route placeholder, and nothing caches a built graph to disk
yet.

## M7.9 (Rust): engine integration

Status: in progress. Composes the M7/M8 crates into `ohl-engine`'s existing
`Game::tick`/`Game::render` pair, so `ohl-app` stays a thin composition root.
Everything is additive: no existing public API changes, and save/load and
level transitions keep their behaviour.

- **M7.9 P0: engine spine (fixed tick, one world, entity-driven models).**
  `ohl-engine::tick` lifts the fixed timestep out of
  `ohl_physics::PlayerController` and into the engine: `TickClock` turns a
  variable frame time into whole `TICK_SECONDS` steps (re-exported from
  `ohl_physics::controller`, so the two cannot drift), banking the remainder
  and dropping the backlog at `MAX_TICKS_PER_FRAME`. `Game::tick` is now a
  frame loop — clamp the frame, turn the view once (aiming is a frame-rate
  concern), then run whole steps — and `Game::elapsed` advances one step at
  a time, so a saved clock is a whole multiple of the step.
  `ohl-engine::systems` holds the normative thirteen-phase step list as
  private functions, with the phases M7.9 P1–P3 fill present as empty hooks;
  damage resolves after AI thinks, and movers run last.
  `ohl-engine::components` adds the components no other crate owns
  (`StudioAnim`, `PlayerTag`, `Owner`, `Pickup`, `Charger`, `Corpse`),
  reusing `ohl_combat::{Health, Armor}` rather than duplicating them, and
  `ohl-engine::ids` maps a `hecs::Entity` to and from
  `ohl_combat::EntityId`. `Level` keeps its parsed entity lump
  (`Level::defs`, published as `Game::entity_defs`) and spawns the client
  entity, which stays out of the definition-aligned `Registry::entities` so
  saves keep referencing entities by spawn index. Every studio placement is
  now an entity carrying a `StudioAnim`, and `render.rs` sources its studio
  instances from the registry (sampled at each entity's own cursor) instead
  of a static list, keeping M3's `sequence`/`body`/`skin` and sprite paths
  intact. `Input` gains `attack`, `attack2`, `reload`, `select_slot` and
  `flashlight_pressed`; `Game` gains `hud()`, `player_entity()`,
  `prop_count()`, `entity_defs()` and the `SystemsConfig` accessors.
  Verified: `TickClock` unit tests (whole multiples, banking, the clamp
  dropping rather than banking, non-finite input); `entity_id`/`entity_of`
  round-trips including a despawned entity and a recycled slot; an
  integration test that the same simulated second lands in the same place
  however it is cut into frames; and a proptest that an arbitrary frame time
  and an arbitrary `Input` never panic and never advance the clock by more
  than one clamped burst.

- **M7.9 P2: AI and navigation wiring.** `ohl-engine::ai` owns one
  `ohl_ai::AiWorld` over the same entity world everything else uses: one
  brain per `monster_*` classname the map declares (and per `monstermaker`
  `monstertype`), registered in sorted classname order so brain ids are a
  function of the map alone; `ohl_ai::attach_monsters` turns the entity lump
  into monsters, with health from the species table overridden by
  `skill.cfg`; and a classname with no table row is left inert rather than
  guessed at. Phase 8 builds one `ohl_ai::SightContext` per step (PVS
  pre-filter plus a hull-0 trace) and consumes the resulting `AiEvent`s:
  activity and sequence events pick a `StudioAnim::sequence` by looking the
  intent's name up in the loaded model's own sequence table (falling back to
  sequence 0, with no sequence name written into the engine), and attacks
  become either a filtered trace and a queued `ohl_combat::DamageInfo` or a
  `ProjectileRequest` handed to a `ProjectileSpawner` seam M7.9 P3 fills.
  Phase 10 applies queued damage through `ohl_ai::apply_monster_damage` —
  the one place a monster's health moves, and so the one thing that can
  report a death — then decides corpse or gib, fades the corpses whose
  species fades, evaluates each monster's declared
  `TriggerCondition`/`TriggerTarget` into `Simulation::fire`, and ticks every
  `monstermaker`. `ohl-engine::nav` builds the map's `info_node`/
  `info_node_air` lattice into an `ohl_ai::NavBridge` and attaches it; a map
  with no nodes leaves the navigator detached and `ohl-ai`'s straight-line
  fallback in charge, which is a supported state. The client is a real
  entity with an `ohl_ai::Actor`, so monsters see, target and shoot the
  player through the components they use for each other. `Systems` owns the
  one `ohl_ai::Pcg32` seeded from `SystemsConfig::rng_seed` and the engine's
  damage queue; `Game` gains `monster_count()`, `ai_state_hash()` and
  `monster_death_count()`, all data and never logged. Verified: a monster
  acquires the player across an open room and does not across a wall; a
  `monstermaker` respects `monstercount` and `m_imaxlivechildren`; a killed
  monster is reported dead exactly once and leaves a corpse that fades,
  while an overkill gibs; two games from the same bytes, seed and inputs
  produce the same `ai_state_hash` after 600 steps; two mutually hostile
  monsters hurt each other end to end; a map with no `info_node` still moves
  monsters; and a proptest over arbitrary node lattices and inputs never
  panics.

  Not yet done: P4 (save sections and the scripted-input smoke).

- **M7.9 P3: projectiles, deployables, view model and transient sprites.**
  `ohl-engine::projectiles` fills phase 7: `ProjectileSystem` owns an
  `ohl_combat::ProjectileSet` and `DeployableSet`, rebuilds a `HitboxIndex`
  each step from every entity carrying a `StudioAnim` (sampling its posed
  hitboxes with `ohl_world::StudioPose::hitbox_bounds`), and sweeps both
  through the level's collision so a grenade or a bolt never tunnels.
  `ProjectileEvent`/`DeployableEvent` map to `ohl_combat::radius_damage`
  calls (queued into `Systems::damage_queue`, created here since P1 has not
  landed in this tree yet) and to bounded transient sprites; model-backed
  kinds get a real `hecs` entity carrying `StudioAnim`, tracked in a
  `BTreeMap<ProjectileId, Entity>` so the existing entity-driven render path
  draws them for free. `Systems::spawn_projectile` is the seam a later
  package's monster ranged attacks use without this crate depending on
  `ohl-ai`. `ohl-engine::sprites` is the bounded (64, oldest-dropped-and-
  counted) transient-sprite list; `ohl-engine::viewmodel` builds the view
  model's placement from the camera's basis (forward/left/up, pitch
  included) plus the new `SystemsConfig::{view_model_fov, view_model_offset}`,
  and reproduces `ohl_gameplay::ViewModelAction`'s five variants locally
  (that crate is not yet a dependency of `ohl-engine`) so unifying the two
  is a type substitution once P1 lands. `render.rs` gains two additive
  calls: the view model, drawn last with its depth manually reset to the
  far plane (so it is never clipped by world geometry) but the frame's
  colour kept; and the transient sprites, appended to the existing sprite
  instance list. `Game` gains `projectile_count()` and `viewmodel_visible()`.
  Verified: a grenade fired at a wall inside a synthetic room never tunnels
  through it under a swept trace; `ohl_combat::radius_damage` falls off
  monotonically with distance and a target behind a wall takes nothing; a
  placed tripmine arms only after the published three seconds; a satchel
  set off by its own owner is among the blast's own hits; a proptest that
  arbitrary spawn velocities and tick lengths never panic and keep a
  projectile finite and within the room's bounds or resolved; a GPU-gated
  (`#[ignore]`, `OHL_RENDER_GPU_TEST=1`) test that a frame with a view model
  and a transient sprite differs from the same frame without them.

  Not yet done: no package in this tree yet selects a weapon or a view
  model (that lands with the inventory/`ohl-gameplay` wiring); per-kind
  blast radius, direct-impact damage, transient-sprite duration/scale and
  viewmodel FOV/offset are `TODO(black-box)` placeholders (see
  `docs/FORMAT_SOURCES.md`, "Projectiles, explosions and deployables
  (M7.3)").

- **M7.9 P4a: deterministic scripted input and the combat smoke harness.**
  `crates/ohl-app/src/script.rs` parses a tiny, bounded, project-owned
  scripted-input format (`<ticks> <token> [args]`, a closed token set,
  4,096 lines/100,000 ticks/8 tokens-per-line caps, fixed parse-error
  messages, never panics on arbitrary bytes) into a `Vec<ohl_engine::Input>`;
  `--script <PATH>` and `--script-log` (`crates/ohl-app/src/main.rs`'s
  `Cli`, `game_run.rs`'s `GameArgs`) run it for `script.len()` ticks at the
  existing `CAPTURE_STEP`, with no GPU context created unless
  `--headless-screenshot` is also given, so the scripted loop runs
  headlessly. `crates/ohl-app/src/script_log.rs` emits "Scripted input
  loaded."/"Scripted input finished." around the loop and observes two new
  `ohl-engine` data counters — `Game::monster_damage_event_count` (new;
  `ai::AiState::damage_events`) and the existing `Game::monster_death_count`
  — to emit "A monster took damage."/"A monster died." at most once each;
  per this milestone's logging policy, `ohl-engine` itself still never
  logs. "The player fired a weapon.", "A shot hit an entity.", "A pickup
  was collected." and "The player took damage." are documented TODO(P1)
  hooks, not wired: their sources (weapon firing, hitscan, pickups, and
  phase 9 damage resolution) do not exist on this branch (M7.9 P1 is not
  merged), and damage aimed at a non-monster target — the player included
  — is currently discarded rather than applied
  (`ai::AiState::drain_engine_damage`). `ohl_engine::test_support::run_script`
  drives a `Game` from a fixed `Input` slice for a determinism test outside
  any CLI or GPU. `xtask/src/combat_smoke.rs` (`cargo xtask combat-smoke`)
  runs the release binary once per project-authored scenario under the new
  `xtask/smoke-scenarios/*.txt` (map names only from `ohl_campaign`'s
  cited table: `TRAINMAP`, `STARTMAP`, and `"c1a1"`), asserts the exact
  fixed lines are present/absent per scenario, and reports scenario names
  and pass/fail buckets only, reusing `campaign_smoke.rs`'s
  `Category`/`sanitize_error_code` shape. Verified: parser unit tests for
  the documented grammar and every bounded-rejection path, plus a proptest
  that it never panics on arbitrary bytes; an integration test that two
  fresh `Game`s built from the same bytes and default seed, ticked with the
  same scripted sequence, produce identical `ai_state_hash` after every
  tick; a CLI test that `--script`/`--script-log` runs headlessly (no
  graphics adapter) and logs exactly the two milestone lines this branch
  can produce; and a unit test that `combat-smoke`'s summary names no
  payload path or pixel statistic.

  Not yet done: nothing — M7.9 P4b (below) adds the five additive save
  sections and wires the four TODO(P1) milestone lines above now that M7.9
  P1 has landed.

- **M7.9 P1: weapons, pickups, damage routing and HUD/audio.**
  `ohl-engine::combat` rebuilds `ohl_combat::HitboxIndex` each step from
  every entity carrying a `StudioAnim` (phase 5); drives the selected
  weapon's `ohl_combat::FiringState` and resolves its hitscan/melee/beam
  actions through `trace_attack_filtered`, always ignoring the player
  entity (phase 6). Firing consumes the engine's own `AmmoBank` rather than
  `ohl_combat::Inventory`'s (grow-only) ammo pools directly — `Inventory`
  never gains a subtracting mutator, so this crate keeps the bank as the
  one authoritative reserve and hands out a freshly stamped `Inventory`
  (`Game::inventory`) whenever a caller needs to read ammo from one.
  `Systems` owns one shared `damage_queue: Vec<QueuedDamage>` (keyed by
  `hecs::Entity`, not `ohl_combat::EntityId`), which phase 9 drains once:
  damage aimed at the player routes to `ohl_player::Player` (armor, suit,
  death), damage aimed at anything else to that entity's
  `ohl_combat::Health`/`Armor` components. `ohl-engine::damage_map` is the
  `ohl_player::DamageKind` <-> `ohl_combat::DamageType` mapping neither
  player-systems crate needs to know about the other for (§3 of the design
  note); the reduction order is fixed so a combined mask never depends on
  iteration order. `ohl-engine::pickups` classifies `Level::defs` once per
  level (lazily, so `level.rs` stays untouched) into `Pickup`/`Charger`
  components, touch-tests them against the player's origin (phase 11), and
  drains a charger's reservoir while `use` is held. `ohl-engine::presentation`
  owns the one `ohl_gameplay::GameplayBridge` and turns `ohl_player::
  PlayerEvent`s the bridge cannot see (the player's own health/armor) into
  `HudState` updates and the four additive `GameEvent` variants: `Sound`,
  `Suit`, `ViewModel`, `PlayerDied`. `ohl_player::Player::tick` is wired into
  phase 3 from the `PhysicsOutput` phase 2's `PlayerController` reports, with
  `trigger_hurt` volumes gathered by a radius test against the player's
  origin (a documented `TODO(black-box)` stand-in for a real brush-overlap
  test, matching the same simplification `pickups.rs` uses for its own touch
  radius). Every `ohl_gameplay::SoundCue::path` this package emits is `None`
  (§5): the plumbing is complete, the path table stays empty pending a
  clean-room provenance review. `Game` gains `inventory()`, `player_health()`
  and `player_armor()`.

  Post-review fixes: `Input` gains `use_held` (a held axis) alongside the
  pre-existing `use_pressed` edge; `PickupsState`'s charger drain and
  `Player::tick`'s `PlayerInput::use_held` both key off the hold, not the
  edge (the edge fires once per press regardless of how long the key stays
  down, which cannot drive a use-and-hold charger). `Systems::{capture_carry,
  restore_carry}` bind `#62`'s `PlayerCarry` seam (`transition.rs`) to this
  package's real `ohl_player`/`CombatState`/`AmmoBank` state: a
  `Game::change_level` now carries health, armor, owned weapons, per-weapon
  clips, reserve ammo, the HEV suit and the long jump module across, via an
  ad hoc byte encoding in `PlayerCarryState::extra`. A save/load round trip
  already preserves the same state today, for free: `GameSave` (`extra`
  blob included) is serialized whole into the `ohl-save` container, so
  nothing about `to_save`/`from_save` needed to change. `TODO(P4)`: fold
  this ad hoc encoding into its own `SECTION_INVENTORY` (§6) instead, so a
  save's inventory section is self-describing independent of
  `PlayerCarryState`'s shape. Weapon
  *selection* is not carried — `Inventory`'s selection API is cycle-only,
  with no way to force an exact weapon back into place — so a transition
  holsters. `ohl-app` copies `Game::hud()`'s health/armor/ammo/damage-flash
  fields into its own drawn `HudState` each frame (without clobbering that
  struct's own title/message state, which arrives as `GameEvent`s, not
  through `HudState`); `PickupsState` now calls
  `GameplayBridge::on_pickup`, giving `GameEvent::Sound` its first real
  producer. Known remaining gaps: hitscan `spread` is discarded (every
  pellet of a multi-pellet shot traces the identical ray; `TODO(black-box)`
  at the call site, since no usable source publishes Half-Life's spread
  cones and sampling one needs a random source this package does not yet
  own), and `CombatEvent::DamageDealt`'s `health_lost`/`armor_lost` fields
  report the pre-armour amount and `0.0` rather than the true split (only
  `target` is read by anything today).

  Verified: `damage_kind_of` is total and order-independent for every
  single-bit `DamageType` mask and for `BURN|FREEZE`; `damage_type_of`
  composed with `damage_kind_of` is the identity on the eleven mapped
  `DamageKind`s; firing at a synthetic target deposits exactly the weapon's
  published per-shot damage, and never hits the player even when the player
  is in the hitbox index; walking over a `weapon_*` entity adds it once and
  respects its published ammo carry cap; a `func_healthcharger` drains while
  `use_held` stays true and stops the instant it goes false, and a single
  held tick restores exactly one tick's worth (never more, never less); a
  proptest (now over a fixture that actually owns a weapon) that an
  arbitrary sequence of `Input`s never drives health, armor, clip or reserve
  ammo out of range and never panics; firing some ammo into a weapon's clip
  and changing level leaves the reserve, the clip and the (already-damaged)
  health exactly as they were; the existing headless capture still renders.

  Not yet done: P4 (save sections and the scripted-input smoke).

- **M7.9 P4b: save sections for combat, AI and projectiles; remaining
  scripted log lines.** Five additive save-file sections, allocated next
  to #62's in one place (`ohl-engine::save`): `SECTION_INVENTORY` (23,
  `save_state::InventorySnapshot` — owned weapons/clips, `AmmoBank`
  reserves, selection, and the drawn weapon's firing-state-machine summary
  via new `ohl_combat::FiringState::{state_tag_and_timer, restore}`),
  `SECTION_ENTITY_COMBAT` (24, one `Option<(health, armor)>`-shaped entry
  per registry entity in spawn order, reading a monster's live health from
  `ohl_ai::Actor` — the value `apply_monster_damage` actually moves —
  rather than its spawn-time `Health` component), `SECTION_AI` (25, one
  `save_state::AiSnapshot` per entity: `MonsterState`/`Activity` tags with
  new `from_tag` reverse mappings, the running schedule's stable name
  restored through new `ohl_ai::ScheduleRunner::{started, restore}`, task
  index, enemy and route waypoints by spawn index, `Conditions` raw bits,
  squad name/leader), `SECTION_PROJECTILES` (26, every live
  `ohl_combat::Projectile`/`Satchel`/`Tripmine` plus new
  `ProjectileSet::{next_id_and_rng_state, restore_from_parts}` and
  `DeployableSet::{next_id, restore_from_parts}` so ids and the snark-hop
  random stream continue exactly where the save left them) and
  `SECTION_RNG` (27, `Systems`'s shared `ohl_ai::Pcg32` state plus a new
  substep counter). Entities are referenced only by their position in
  `Registry::entities`, never `hecs::Entity` bits
  (`save_state::{spawn_index_of, entity_at_spawn_index}`); a `monstermaker`
  child is not in that list at all (it is spawned directly through
  `registry.world.spawn`), so it is not saved — a documented, bounded
  `TODO(P4b-followup)`, not silent corruption. A new
  `save::optional_section` helper tells `ohl_save::SaveError::
  SectionNotFound` (a pre-P4b save, missing tags 23-27 entirely — loads as
  `None`/a default) apart from a section present but failing to decode
  (fails closed with `EngineError::SaveUnreadable`, same as every other
  section). The additive API this needed in `ohl-ai`/`ohl-combat` stays
  serde-free (`ohl-engine`'s own `save_state` DTOs are the only new
  `serde` surface); `MonsterAi`/`Inventory`/`ProjectileSet`/`DeployableSet`
  themselves are untouched beyond the new methods.

  `crates/ohl-app/src/script_log.rs` now wires the four milestone lines
  M7.9 P4a left as TODO(P1): "The player fired a weapon."/"A shot hit an
  entity." from new `Game::{weapon_fired_count, shot_hit_count}` (backed by
  new `CombatState` counters incremented in `Systems`'s phase 6), "A
  pickup was collected." from new `Game::pickup_count` (a new
  `PickupsState::taken_count`), and "The player took damage." from new
  `Game::player_damage_event_count` (a new `Systems::player_damage_events`
  counter, incremented in phase 9's player-damage branch, which
  `crate::combat::resolve_damage` now takes as an added parameter).
  `xtask/src/combat_smoke.rs`'s `Scenario` now carries its own
  present/absent milestone-line set (rather than one fixed pair for every
  scenario), so a later scenario that does reach one of the six can move
  it from `absent` to `present` without touching the others. A fourth
  scenario, `xtask/smoke-scenarios/pick_up_and_fire_a_weapon.txt` (run
  against `"t0a0b1"`, one of `ohl_campaign::HAZARD_COURSE_MAPS`'s own
  cited maps), now does reach two of the six: a scripted walk to that
  map's own first weapon pickup, a HUD slot select and a swing reaches
  both "A pickup was collected." and "The player fired a weapon." — a
  crowbar swing routes through the same melee branch a hitscan shot does
  (`crates/ohl-engine/src/combat.rs`'s `CombatState::weapons`), so it
  counts as firing too; see that scenario file's own header.

  Verified: unit round trips for the AI/inventory/projectile/RNG snapshot
  conversions and the new `ohl-ai`/`ohl-combat` additive methods;
  integration tests (`crates/ohl-engine/tests/save_sections.rs`) that a
  save -> load -> save chain after firing a weapon, killing a monster,
  picking up ammo and leaving a live projectile in flight is byte-identical
  (which also exercises the fired/hit counters end to end, against this
  package's own synthetic fixture); a save missing tags 23-27 (a pre-P4b
  file) still loads with defaults; a save with a corrupted `SECTION_AI`
  fails closed with `SaveUnreadable`; a fixed-seed scripted run continued
  after a load produces the same `ai_state_hash` as the uninterrupted run
  (which also required fixing a pre-existing bug: `Game::to_save`/
  `Game::restore` stored/restored the player's *eye* position as the
  `PlayerController`'s spawn *origin*, silently displacing the reloaded
  player upward by the stance's eye offset every save/load); a proptest
  that decoding an arbitrary `postcard` byte string into any new snapshot
  type never panics.

  Clean-room: the save format is this project's own (`ohl-save`), not
  GoldSrc's `.sav` layout; the script format and every log line remain
  project-authored fixed strings, per `docs/m79-design.md` §10.

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

Not yet done (at the time this package landed; see M9.3 below for
publishing to GitHub Releases): verified cross-compilation (the `--target`
flag is wired but only exercised as a best-effort passthrough, not proven
against an actual cross toolchain in CI).

## M9.1 (Rust): fuzz targets for untrusted inputs

Status: done. Four more `cargo fuzz` packages/targets cover the remaining
untrusted-input decoders, all run by `.github/workflows/fuzz.yml`'s
dynamic discovery (30s each, four at a time; the job timeout was raised
from 15 to 25 minutes because the total is now 19 targets and the
`ohl-engine`/`ohl-app` packages each build the full renderer chain first).

- `crates/ohl-save/fuzz`: `open_fuzz` now also reads every table entry
  back through `section` after a successful `open`; `roundtrip_fuzz` is
  rebuilt on an `Arbitrary`-derived input that writes random sections
  through `SaveWriter`, asserts the untouched output reopens with every
  section intact, then flips random bytes and asserts the mutated bytes
  never panic `SaveReader::open`.
- `crates/ohl-engine/fuzz`: `decode_fuzz` (`GameSave::from_bytes` over raw
  bytes) and `sections_fuzz` (an `Arbitrary` list of `(tag, bytes)` pairs
  written into a correctly framed container so the per-section `postcard`
  decoders for tags 16 through 22 see adversarial payloads).
- `crates/ohl-app/fuzz`: `script_parse` (`Script::parse` over raw bytes,
  asserting the parsed input count never exceeds `MAX_TOTAL_TICKS`). The
  parser is reached through a new minimal `[lib]` target in `ohl-app`
  (`src/lib.rs`, which only re-exposes `script`); the binary is unchanged.
- `crates/ohl-ai/fuzz`: `entity_defs_spawn` (an `Arbitrary` list of
  entities with random keyvalues, built into a `Registry`, spawned through
  `attach_monsters` with an always-spawn rule, and ticked ten times with no
  collision model or navigator).

Seed corpora are hand-authored synthetic bytes only (empty input, a
truncated prefix, a documented-grammar example, a bad magic), force-added
under each package's otherwise git-ignored `corpus/` so fuzzer-grown
entries never land in the tree. Every target was run locally for 60s on
top of the CI smoke; no panics were found in the code under test.

## M9.3 (Rust): release packaging polish

Status: done. Closes the packaging-mechanics gaps a release-readiness dry
run of M9 flagged (`.plan/release-readiness.md`), short of anything that
actually requires cutting a tag.

- `cargo xtask dist` is now a proper `clap`-derived subcommand, matching
  `campaign-smoke`/`combat-smoke`: `--help`/`-h` prints usage and exits 0
  (previously any unrecognised argument, `--help` included, hit the
  catch-all `unrecognised argument` error path and exited 1).
- `--out-dir <DIR>` selects where the release folder and archive are
  written (default `target/dist`, unchanged from before this package),
  created if missing. It is lexically resolved and rejected if it would
  fall under this workspace's own `assets/`, `cache/`, or `imported/`
  directories — the same untracked payload/cache locations `cargo xtask
  policy` (`PROHIBITED_PREFIXES`) already keeps tracked files out of —
  including via a `..`-traversal that would otherwise land there.
- `--print-target` prints the resolved target triple (the same one
  `--target` would build for, or the host triple otherwise) and exits
  without building or packaging anything; CI's `release` job uses this to
  name its workflow artifact after the real target triple
  (`open-half-life-<version>-x86_64-unknown-linux-gnu`, say) instead of the
  GitHub-runner label (`ubuntu-latest`) it used before, matching the
  archive/folder naming already inside it.
- The workspace root `Cargo.toml` now sets `[profile.release]` `strip =
  "symbols"`, `codegen-units = 1`, and `lto = "thin"` (`panic` is
  unchanged). Measured locally on Linux x86-64: the shipped binary shrank
  from 22.4 MB (unstripped, M9's original dry run) to about 14.6 MiB, and
  the packaged `.tar.gz` from 7.9 MB to about 6.6 MiB; the release build
  itself took about 4m46s end to end (cold `target/`), versus that same dry
  run's 3m15s without any profile tuning — comfortably inside the
  `release` job's 30-minute timeout. The packaged, stripped binary was
  re-verified end to end: `--help`, `--version`, and a full headless
  capture (`OHL_RENDER_GPU_TEST=1 ... --training --headless-screenshot ...
  --frames 30`) against a real imported payload all still pass.
- CI's `release` job (`.github/workflows/build.yml`) now has a
  `publish-release` job after it, gated strictly on a `v*` tag push (never
  on `workflow_dispatch`, which has no tag to attach a release to). It
  downloads all three platforms' archives, writes a top-level `SHA256SUMS`
  over them, and runs `gh release create` (or `gh release upload --clobber`
  if the tag's release already exists) to attach the three archives plus
  `SHA256SUMS` to a GitHub Release named after the tag — using only the
  GitHub CLI already on every runner and the automatic `GITHUB_TOKEN`, with
  `permissions: contents: write` scoped to that one job (the workflow's
  top-level permissions stay `contents: read`). No third-party Action is
  used anywhere in this path. Release notes are generated by a small `awk`
  step that extracts the "Status as of" section straight out of this file
  (see above) — never anything derived from game media.

Verified: `cargo test -p xtask` covers the new `--help`/`-h`/unknown-flag
exit codes, `--out-dir` default and parsing, `--print-target` parsing, the
lexical path normalizer, and `--out-dir` rejection for all three prohibited
prefixes (including a `..`-traversal case). `cargo xtask dist --out-dir
<dir>` was run end-to-end on Linux x86-64 with the new stripped/LTO release
profile and its archive re-verified with `sha256sum -c` (all 1074 bundled
license files plus the binary, worker image, `LICENSE`,
`THIRD_PARTY_NOTICES.md`, and `README-dist.md`). The workflow YAML was
parsed with `yaml.safe_load` to catch syntax errors; the `publish-release`
job's actual `gh release` behavior was reviewed but, per this package's own
scope, not exercised against a real tag (no tag or release was created).

Not yet done: exercising `publish-release` against a real `v*` tag (the
next actual tag push will be the first real test); macOS/Windows codesigning
and notarization (M9's original "Not yet done" list); verified
cross-compilation.

## Later milestones

- M3: BSP rendering (Rust first light in progress, see above)
- M4: player movement (M4.1 hulls and walking in progress, see above)
- M5: interactive entities (Rust `ohl-game`, M5.1, in progress, see above)
- M6: models and animation (Rust studio models in progress, see above)
- M7: combat (Rust combat skeleton, `ohl-ai` monster AI core, package 7.5,
  and player systems in progress, see above)
- M8: full campaign compatibility (Rust save container in progress, see above)
- M9: release hardening

- **M7.11: scripted sequences and talk monsters.** `ohl-game::scripts`
  reads the published `scripted_sequence`/`aiscripted_sequence`/
  `scripted_sentence` keyvalues, choice values and spawnflag bits, and adds
  the one `ScriptActivation` component that puts those entities on the
  existing `Simulation::activate` "use"/target-firing path shared with
  doors, buttons and `multi_manager` (plus `trigger_auto`, so a map can
  start a script unaided). `ohl-ai::scripts` runs the pure state machine —
  every `m_fMoveTo` mode (no move, walk, run, instantaneous, turn to face),
  `No Interruptions`, `Repeatable` and `m_flRepeat` — and its `ScriptHold`
  marker suspends a possessed monster's brain inside `AiWorld::tick` while
  leaving its senses, its damage intake and its route intact.
  `ohl-ai::follow` adds the `monster_scientist`/`monster_barney` follow
  layer: a player `use` toggles a talk monster into the player's group, the
  published two-ally limit evicts the longest-serving member, and a
  `Pre-Disaster` talk monster refuses; following itself reuses the
  `FOLLOW_PLAYER` schedule and the `Navigator`/`NavBridge` movement seam
  that already existed. `ohl-engine::ai` wires all of it additively into
  phases 8 and 10 — target selection by `targetname` or by classname inside
  `m_flRadius`, movement through the same navigator, sequence *names*
  resolved at runtime against the monster's own loaded studio model (no
  sequence-name literal is shipped), `target` fired through the map-logic
  simulation, and `scripted_sentence` resolved through the existing
  `SentenceLookup` into a `GameEvent::Sound` whose path is always `None` —
  and exposes `Game::{active_script_count, script_start_count,
  script_completion_count, followers}`. Behind `--script-log`, `ohl-app`
  emits the two fixed lines "A scripted sequence started." and "A scripted
  sequence finished."; nothing map-derived is ever interpolated. Covered by
  `ohl-ai` unit tests over the whole state machine (each `m_fMoveTo` mode,
  interruption with and without the flag, repeat behaviour), `ohl-engine`
  integration tests over an extended synthetic room (a `trigger_auto`-driven
  walking script that fires its target exactly once, a `No Interruptions`
  script surviving mid-script damage, an instantaneous warp, a radius-bounded
  classname search, a scientist following and unfollowing, a
  `Pre-Disaster` scientist refusing, a `scripted_sentence` cue with no asset
  path, and a determinism replay), and `proptest`s showing arbitrary
  keyvalues on all three entities never panic and never produce non-finite
  state. See `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk
  monsters", for sources and the nineteen `TODO(black-box)` items.

- **M7.12: trigger_camera.** `ohl-game::registry::TriggerCamera` reads the
  published `trigger_camera` keyvalues (`wait`, `moveto`, `speed`,
  `acceleration`, `deceleration`, `target`) and its three documented
  spawnflags (`Start At Player`, `Follow Player`, `Freeze Player`);
  `ohl-game::camera::TriggerCameraState` resolves `moveto` into the same
  `PathChain` type `func_train`/`func_tracktrain` already builds (M7.10) and
  rides the same `Simulation::activate` "use"/target-firing path every
  other entity here shares, including `trigger_auto` unchanged from #75 —
  re-triggering an active sequence stops it (per the published behaviour)
  without firing its completion `target`; only a natural completion (the
  `wait` hold elapsing, or a non-looped path's dead end, whichever comes
  first) does. `ohl-engine::camera` reads the resolved state each frame the
  same way `render.rs` already reads a track train's position: it overrides
  the world camera `Game::eye_position`/`Game::render` use while a sequence
  is active (position from the path or the entity's own placed origin,
  aimed at `target`'s entity, at the player under `Follow Player`, or along
  the path's own heading otherwise), and gates player movement/attack input
  (never mouse look) while `Freeze Player` is set — landing one tick behind
  the view override itself, the same lag `ScriptActivation` already has
  between a trigger firing and its being drained. Behind `--script-log`,
  `ohl-app` emits the two fixed lines "A camera sequence started." and "A
  camera sequence finished."; nothing map-derived is ever interpolated.
  `TriggerCameraState` is not part of any save section: `SECTION_SIMULATION`
  is `postcard`-encoded, which is not self-describing, so an additive field
  (even with `#[serde(default)]`) fails to decode any save file written
  before it existed — confirmed with a standalone reproduction — so a save
  taken mid-sequence resumes with it dormant instead, the same known gap
  `TrackTrainState` already has for a `func_train` mid-route. Covered by
  `ohl-game` unit tests over the state machine (hold duration, path
  traversal and its own node `wait`, the dead-end-completes-early reading,
  re-trigger semantics, completion firing exactly once, and a `proptest`
  over arbitrary chains/speeds/possibly-non-finite `dt`) and `ohl-engine`
  integration tests over a `trigger_auto`-driven synthetic room (the eye
  position matching the camera's own origin during the hold and reverting
  afterward, `Freeze Player` ignoring forward input for the whole hold and
  releasing it afterward, plain input still moving the player without that
  flag, and a delayed second `trigger_auto` stopping an active sequence
  without firing its target). See `docs/FORMAT_SOURCES.md`, "Camera
  sequences", for sources and every `TODO(black-box)` item.

- **M7.13: trigger_changelevel fires on touch.** Fixes a gap where the
  dedicated `trigger_changelevel` arm in `Registry::build` ran ahead of the
  generic `trigger_*` arm and never attached the touch-capable `Trigger`
  component the way every other `trigger_*` does, so only `use`/being
  targeted could ever reach a level-transition volume, not the player's own
  movement (unlike `trigger_once`/`trigger_multiple`, fixed for touch
  earlier by `Simulation::touch_triggers`). `Registry::build` now also
  attaches `Trigger` to a `trigger_changelevel`, unless its published "USE
  Only" spawnflag (`SPAWNFLAG_CHANGELEVEL_USE_ONLY`, bit `2`) is set, in
  which case it stays `use`-only exactly as before.
  `Simulation::touch_changelevel_triggers` fires the `Event::LevelChange` on
  a rising overlap edge rather than every overlapping frame a continuously
  touched volume produces, and (conservatively, since no public source
  states the destination-map-arrival case either way) never fires on a
  volume's first observation even if already overlapping, so a freshly
  loaded map's own return trigger requires leaving and re-entering before
  it can fire. Covered by `ohl-game` unit tests (touch fires once per
  entrance and not again while still overlapping; "USE Only" ignores touch
  but still responds to `use`) and an `ohl-engine` integration test driving
  a scripted walk into a synthetic `trigger_changelevel` volume, asserting
  exactly one `GameEvent::LevelChange` and a real `Game::change_level`
  transition. See `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
  logic", for the source and the exact `TODO(black-box)` wording.

## Status as of 2026-09-06

This section is a snapshot, not a replacement for the package-by-package
history above; each "Not yet done" note under a given package describes
that package's own gaps at the time it landed, and most of those gaps have
since been closed by a later package documented further down this file.

What works today, end to end: real payload import on Linux x86-64
(`ohl-parser-backends` over the Wise/MS-CAB/InstallShield-3-Z back ends,
composed by `ohl_import::pipeline`); all 93 campaign maps (18 story
chapters plus the Hazard Course) loading and rendering headless
(`cargo xtask campaign-smoke`, M8.3); combat, monster AI, navigation,
player systems, projectiles, `func_train`/`func_tracktrain` movers, and
scripted sequences/talk monsters, all implemented and covered by unit,
integration and property tests (M7.x, M7.9 P0-P4a); campaign flow (level
transitions, chapter titles, difficulty) and save/load over the
project-owned `ohl-save` container (M8); and 19 `cargo fuzz` targets across
every crate that parses untrusted bytes (M9.1). None of the combat/AI/
scripted-sequence work has been play-tested end to end on a real display
by a person; it is exercised only by the automated test suites and the
headless smokes.

Open follow-ups, in no particular priority order:

- **Per-trace hitbox exclusion seam (M7.9 P1/P3).** `trace_attack_filtered`
  currently has one hardcoded exclusion (the player entity is always
  ignored so nothing can shoot itself with a hitscan weapon); there is no
  general per-trace exclusion list yet, so a monster's own attack cannot
  yet exclude itself the same way. Filling this in is a small seam, not a
  design change.
- **`trigger_camera`/`trigger_auto`-driven intro views (fidelity finding
  F2) — implemented (M7.12), training-start view unconfirmed.**
  `trigger_camera` now takes over the view the same way `trigger_auto`
  already fired other targets (M7.11); see M7.12 above. Whether this
  actually explains fidelity round 4/5's `t0a0` spawn-framing finding (F2/E4)
  is still open: that specific map's own entity data has not yet been
  black-box confirmed to contain a `trigger_camera` at all, only that no
  other candidate cause survived rounds 4 and 5's investigation.
- **`func_tracktrain` `altpath` branching.** The `path_track` chain builder
  (M7.10, "track trains") resolves a `func_train`/`func_tracktrain`'s
  `target` into a bounded `PathChain`, but `path_track`'s documented
  `altpath` (branch path) keyvalue is recorded, not yet applied; a
  tracktrain always follows its primary chain.
- **`monstermaker` `targetname` activation.** `monstercount`,
  `m_imaxlivechildren` and `Start On` are respected (M7.9 P2), but a
  `monstermaker` cannot yet be turned on or off at runtime by a `use` or a
  `trigger` aimed at its own `targetname`.
- **`m_iszIdle` pre-trigger idle animation.** A `scripted_sequence` only
  animates its target monster while actually holding it; the published
  looping idle animation a dormant, waiting script's target is supposed to
  play before the script triggers is not yet modelled (see
  `docs/FORMAT_SOURCES.md`, "Scripted sequences and talk monsters", item 21,
  for the exact ambiguity this is deferred behind).
- **Save sections P4b (in progress).** Weapon inventory (owned weapons,
  clips, reserve ammo) currently round-trips inside `SECTION_PLAYER_CARRY`'s
  ad hoc byte encoding rather than its own `SECTION_INVENTORY`; the data
  survives save/load correctly today, but is not yet self-describing
  independent of that carry-state's shape.
- **Linux audio backend decision.** `ohl-audio` always uses a `NullSink` on
  Linux today (see `crates/ohl-audio/src/device.rs`): `cpal`'s only Linux
  backend links `libasound` through `alsa-sys`'s build-time `pkg-config`
  lookup, which the project's "No FFI" rule forbids as written. Whether to
  relax that rule for a system audio library specifically, adopt a
  pure-Rust ALSA/PipeWire backend if one becomes available, or leave Linux
  silent by design remains an open decision; `cpal` already covers
  macOS (CoreAudio) and Windows (WASAPI) with no FFI concern.
- **`cargo xtask campaign-smoke`'s default `--jobs` lowered; `monster_
  bullchicken` accepted.** The smoke's default parallelism (previously the
  host's full `available_parallelism`) made lavapipe's software renderer
  self-contend for CPU cores badly enough to time out 26 of 93 maps that
  all pass serially, so `default_job_count` now defaults to a quarter of
  the host's parallelism (clamped to `[1, 4]`), still overridable via
  `--jobs`. Separately, `ohl_ai::MonsterKind::from_classname` now also
  accepts `monster_bullchicken` (GoldSrc's own internal classname for the
  bullsquid asset, per TWHL's "Reference: Entities and their models"; see
  `docs/FORMAT_SOURCES.md`, "Monster definitions") as an alias for
  `monster_bullsquid`.
- **`ohl-app`'s `--overbright` default calibrated to `1.7`.** A follow-up
  fidelity investigation (round 5) measured `--overbright 1.7` bringing this
  project's captures to roughly 1.01x the public-reference mean luma with no
  added clipping, against roughly 1.72x under at `1.0`, so the application
  now defaults `--overbright` to `1.7` as its own calibrated display
  default — a project display choice, not a claimed engine fact.
  `ohl_world::lightmap::LightRamp::default()` and
  `ohl_engine::GameConfig::default()` stay unchanged at the documented raw
  `1.0`; see `docs/FORMAT_SOURCES.md`, "Rendering conventions".
- **Per-trace hitbox exclusion for projectiles; deployables stay shootable
  (M7.9 P1/P3 follow-up).** `ProjectileSystem::set_model_for` is now wired
  (`ProjectileSystem::configure_models`, called on level attach) and a
  model-backed projectile or placed deployable stays *in* the shared
  `Systems::hitboxes` index rather than being excluded from it — a placed
  tripmine or satchel is damageable by the player's hitscan and by another
  explosive's blast, per the published behaviour cited in
  `docs/FORMAT_SOURCES.md`, "Deployable damageability and per-trace hitbox
  exclusion". What a projectile's own movement trace must not hit — its own
  drawn model, its owner — is ignored per trace instead
  (`ohl_combat::Projectile::self_id`/`owner`, `TraceFilter::ignoring`), never
  by narrowing the index everyone else traces against.
- **Deployable stand-ins and a projectile's `self_id` are re-created on
  load.** `SECTION_PROJECTILES` restores the plain `DeployableSet`/
  `ProjectileSet` data but never carried a model-backed stand-in entity or
  a `hecs::Entity` handle (never serialisable across a save to begin
  with); `ProjectileSystem::restore_snapshot` now re-spawns a fresh
  stand-in for every restored satchel/tripmine and every model-backed
  in-flight projectile, so a restored deployable draws and stays
  damageable again and a restored projectile's own movement trace ignores
  its own drawn model again. The save wire format is unchanged; a save
  from before this fix still loads and simply gets its stand-ins back
  where it would previously have come back undrawn and undamageable.
- **Headless capture can follow a level change; a dev viewpoint near the
  nearest monster (fidelity round 6/7 follow-up).** `--follow-level-change`
  makes a headless (`--headless-screenshot`) or scripted (`--script`) run
  call the same `Game::change_level` the windowed loop uses when a
  `trigger_changelevel` fires, instead of only logging that it was not
  followed (the unchanged default); `crate::game_run`'s two capture paths
  share one `handle_level_change` helper for this. Separately, an additive
  `Game::nearest_monster_position(from: Vec3) -> Option<Vec3>` (data only,
  never logged) backs a new `--viewpoint-at-nearest-monster DISTANCE`
  (`dev-tools` feature only, like `--dev-mdl`), which places a headless
  capture's eye at a spawned monster's eye height, facing it, in noclip.
  Round 7 (closing round 6's finding I2) used the flag to confirm a level
  transition end to end against a synthetic two-map fixture (an `ohl-app`
  integration test); a real-payload Hazard Course transition (`t0a0` →
  `t0a0a`) was attempted but not reached within a bounded
  scripted-navigation budget, and stayed undone rather than added as a
  flaky smoke scenario (see `.plan/fidelity-round-7.md`, which is
  git-ignored and local-only, for what blocked it).
## Status as of 2026-09-07

This section is a snapshot, not a replacement for the package-by-package
history above or the previous "Status as of 2026-09-06" snapshot; it lists
what merged since that snapshot and does not repeat unaffected detail from
it.

Merged since 2026-09-06, by milestone:

- **M6 (fidelity F3).** [PR #78](https://github.com/timo-42/open-half-life/pull/78)
  makes monster models load from the payload: most `monster_*` entities
  carry no `model` keyvalue in the map's own entity lump (GoldSrc hardcodes
  it in `Spawn`/`Precache`), so `ohl_ai::MonsterKind::default_model_path`
  now supplies each defined kind's default `models/*.mdl` path and
  `ohl_engine::level::load_studio_models` falls back to it.
- **M7.9 P4b (save sections).** [PR #80](https://github.com/timo-42/open-half-life/pull/80)
  adds save sections 23-27 (inventory, entity combat state, AI state,
  projectiles/deployables, RNG stream), closing the "Save sections P4b (in
  progress)" follow-up above.
- **M7.9 P1/P3 follow-up.** [PR #84](https://github.com/timo-42/open-half-life/pull/84)
  wires per-trace hitbox exclusion for projectiles so a model-backed
  projectile or placed deployable (tripmine, satchel) stays shootable
  instead of being excluded from the shared hitbox index; what a
  projectile's own movement trace must not hit (its own model, its owner)
  is now excluded per trace instead.
- **M7.9 P4b follow-up (save restore).** [PR #88](https://github.com/timo-42/open-half-life/pull/88)
  re-creates deployable stand-in entities and a restored projectile's
  `self_id` on load, so a save taken mid-fight draws and re-damages those
  entities correctly after a reload instead of coming back undrawn and
  undamageable.
- **M7.11 follow-up.** [PR #82](https://github.com/timo-42/open-half-life/pull/82)
  stops a scripted sequence from holding its target monster forever.
- **M7.12 (trigger_camera).** [PR #86](https://github.com/timo-42/open-half-life/pull/86)
  adds `trigger_camera` view sequences (see the M7.12 entry above).
- **Touch triggers.** [PR #90](https://github.com/timo-42/open-half-life/pull/90)
  fires touch triggers from the player's own movement, not only from
  `use` — the last gap between a mapper-authored trigger volume and this
  project's own trigger-firing path.
- **Brush-entity collision.** [PR #91](https://github.com/timo-42/open-half-life/pull/91)
  collides the player against solid brush entities (doors, moving
  platforms, and similar), not only worldspawn geometry.
- **Combat smoke.** [PR #87](https://github.com/timo-42/open-half-life/pull/87)
  adds a scripted scenario that picks up and fires a weapon
  (`xtask/smoke-scenarios/pick_up_and_fire_a_weapon.txt`), reaching two of
  the six scripted milestone lines M7.9 P4a introduced.
- **Headless capture.** [PR #89](https://github.com/timo-42/open-half-life/pull/89)
  adds `--follow-level-change` (calls the same `Game::change_level` path
  the interactive window uses when a `trigger_changelevel` fires, instead
  of only logging that it was not followed) and, behind `dev-tools`,
  `--viewpoint-at-nearest-monster DISTANCE`.
- **M9.3 (release packaging polish).** [PR #83](https://github.com/timo-42/open-half-life/pull/83)
  makes `cargo xtask dist` a proper `clap` subcommand with `--help`/
  `--out-dir`/`--print-target`, strips and LTOs the release profile, and
  wires `publish-release` to attach archives to a GitHub Release on a `v*`
  tag push.
- **Overbright default.** [PR #85](https://github.com/timo-42/open-half-life/pull/85)
  defaults the app's `--overbright` multiplier to `1.7` (a calibrated
  display default, not a claimed engine fact); `LightRamp`/`GameConfig`
  keep their raw, unmultiplied `1.0` default.
- **Docs.** [PR #78](https://github.com/timo-42/open-half-life/pull/78)
  through [PR #91](https://github.com/timo-42/open-half-life/pull/91) are
  covered above; a further docs-only refresh is this section itself.

As of this snapshot, [PR #92](https://github.com/timo-42/open-half-life/pull/92)
("Fire trigger_changelevel on player touch") is open and merging: it makes a
`trigger_changelevel` fire from the player's own touch, the same way
[PR #90](https://github.com/timo-42/open-half-life/pull/90) already did for
ordinary touch triggers, rather than requiring a `use`. Treat it as
in-flight, not yet landed, until it shows as merged.

Remaining follow-ups, in no particular priority order (superseding the
equivalent bullets in the 2026-09-06 snapshot above where they overlap):

- **Brush collision follow-ups (in flight).** A PR building on
  [PR #91](https://github.com/timo-42/open-half-life/pull/91)'s brush-entity
  collision is in progress; the current landed behavior collides the
  player against a brush entity's solid shape, but does not yet cover
  every mover interaction (see the mover-riders and submodel-contents
  bullets below).
- **Per-chapter walk scenarios.** Closed; see "M9: chapter-walk combat
  smoke" below.
- **`func_water`/`func_ladder` submodel contents.** Brush-entity collision
  (PR #91) treats every solid brush entity as ordinary solid geometry; it
  does not yet special-case a submodel's `CONTENTS_WATER`/`CONTENTS_LADDER`
  volume, so swimming and ladder-climbing brush entities do not yet behave
  distinctly from a solid brush.
- **Brush `angles`.** A brush entity's own `angles` keyvalue (a rotated
  door or platform) is not yet applied to its collision shape.
- **Mover riders.** An entity standing on a moving brush entity (a lift, a
  moving platform) does not yet ride along with it; only the mover's own
  motion and the player's direct collision against its current shape are
  modelled.
- **`trigger_camera`/track-train persistence.** Neither `TriggerCameraState`
  nor `TrackTrainState` is part of any save section yet (`SECTION_SIMULATION`
  is `postcard`-encoded and not self-describing, so an additive field
  cannot be added without breaking old saves); a save taken mid-sequence or
  mid-route resumes with that state dormant instead.
- **`monstermaker` `targetname` activation** (unchanged from the
  2026-09-06 snapshot): still not toggleable at runtime by `use` or
  `trigger`.
- **`m_iszIdle` pre-trigger idle animation** (unchanged from the
  2026-09-06 snapshot): a dormant `scripted_sequence` target's pre-trigger
  idle loop is still not modelled.
- **Linux audio backend decision** (unchanged from the 2026-09-06
  snapshot): `ohl-audio` still always uses a `NullSink` on Linux; whether
  to relax the "no FFI" rule for a system audio library, adopt a pure-Rust
  ALSA/PipeWire backend if one appears, or leave Linux silent by design
  remains open.
- **First tagged release.** No `v*` tag has been pushed yet, so
  `publish-release` (PR #83) has never run against a real tag; the next
  actual tag push will be its first real exercise, and the first entry in
  this project's GitHub Releases.

- **Brush collision follow-ups: detach on despawn, broad-phase, caps
  (PR #91 review).** A despawned solid brush entity (a `func_wall` floor
  removed by a scripted `killtarget`, for example) used to stay attached to
  `ohl_physics::hull::CollisionModel` forever, since nothing ever told the
  collision model the entity was gone; `Level::sync_brush_collision` now
  detects a despawned or component-stripped entity in one pass over its own
  `brush_collision` list (no per-step allocation, no O(n²) name/entity
  search — each entry already names its own `BrushId`) and calls the new
  `CollisionModel::detach_brush`, which reduces that brush to the same
  bare-contents no-op state a submodel with no real geometry already was.
  `CollisionModel::brush_count` now excludes both cases. Separately,
  `trace`/`contents_at` gained a broad-phase check (each attached brush's
  own compiled bounding box, expanded by the traced hull's size) that skips
  a brush's tree walk entirely once its segment or point cannot reach it,
  attaching is capped at a documented `MAX_ATTACHED_BRUSHES`, and
  `Trace::in_open` is now documented and computed as world-only (an
  attached brush's own hull tree reports "empty" almost everywhere outside
  its own small footprint, so folding it into `in_open` previously
  overstated how much of a trace was genuinely open once brushes were
  attached).

## M9: chapter-walk combat smoke

`cargo xtask combat-smoke` (`xtask/src/combat_smoke.rs`) gains nineteen new
scripted-input scenarios (`xtask/smoke-scenarios/walk_*.txt`): one moving-
player walk for each of the 18 story chapters whose starting map is
confirmed in `ohl_campaign::CHAPTERS` (every chapter except Interloper,
whose starting map prefix is deliberately left unverified), plus one for
the Hazard Course — 23 scenarios in total alongside the four pre-existing
ones. Each walks forward with short steps and periodic turns for roughly
20-40 simulated seconds, the technique that had exposed the PR #90 (touch
triggers never firing from movement) and PR #91 (the player falling
through a brush entity's floor) regressions that static captures and load
smokes had missed for days.

Two new fixed `--script-log` lines back these scenarios
(`crates/ohl-app/src/script_log.rs`): "The player moved from the spawn
point." (the player's eye position has moved more than a project constant,
64 units, from where it stood at the scenario's start — asserted present
in every walk scenario as evidence the scripted walk actually moved the
player) and "The player is inside solid geometry." (`Game::eye_is_in_solid`
has read `true` for a cumulative total of more than one second during the
run — asserted absent in every scenario, including the four pre-existing
ones, as this scenario set's own regression guard for the PR #91 class of
bug). Three of the nineteen walks (Power Up, "Forget About Freeman!", Xen)
turn before or instead of advancing straight ahead, tuned only against
that one map's own player-start-relative geometry (a dead-end player
start, a solid corner in the walk's path, and — for Xen — a player start
at the edge of a small platform where advancing much past half a second
sends the player over the edge into a long fall); each scenario file's own
header explains why. All 23 scenarios pass against a real imported
payload.

- **Follow-up: the Xen walk's fall, unstuck.** [PR #96](https://github.com/timo-42/open-half-life/pull/96)
  found that extending the Xen scenario's own cautious ~0.6-second forward
  window to a longer walk let the player go over the platform edge after
  all, and land embedded in solid geometry rather than on top of it. The
  fall itself was already correctly lethal (`ohl_player::fall_damage`
  brings health to zero on that landing, per the documented fall-damage
  rules), but two gaps hid it: a fast enough landing's own ground-probe
  trace could resolve to an origin that still read as embedded (see
  `docs/FORMAT_SOURCES.md`'s "Collision hulls and player movement" for the
  new `unstick_from_ground` nudge this adds), and neither
  `ohl_engine::Systems::step` nor `ohl-app`'s scripted/headless run path
  (`run_scripted`, what `cargo xtask combat-smoke` drives) did anything
  with the player's own death: `step` kept simulating a dead player's
  movement indefinitely, and `run_scripted` dropped `GameEvent::PlayerDied`
  entirely instead of logging the same "The player died." line the
  windowed loop already does. All 23 `combat-smoke` scenarios, including
  the Xen one, still pass unchanged — the standard scenario's own cautious
  window still never reaches the fall.

- **Mover riders: carry the player on moving brush entities.** A player
  standing on a `func_train`/`func_tracktrain`/`func_plat`/lift `func_door`
  is now carried with it instead of being left behind or sinking through a
  rising platform. `ohl_physics::hull::Trace` gained `brush_index`, so the
  ground trace reports *which* attached brush (if any) it hit;
  `ohl_physics::movement::PlayerState` gained `ground_brush` from that;
  `ohl-engine`'s `Level::brush_velocity` records each attached brush's
  per-step displacement divided by `dt`; and `Systems::player_move` feeds
  a standing player's ground brush's velocity back in as
  `PlayerController::base_velocity` — a mechanism that already existed
  (PR #59) but was never fed from anywhere until now. A vertical mover is
  additionally tracked by moving the player's origin directly, bounded by
  the same hull trace, since a ground probe that only looks 2 units down
  cannot reliably catch a platform moving faster than that in one tick. A
  mover that moves into the player instead pushes them clear, bounded by
  the same trace; a push that cannot fully clear the player is published
  on `Level::movers_blocked` (`TODO(black-box)`: nothing yet halts,
  reverses, or applies a blocking mover's `dmg` from that signal — the
  same crush-detection gap already recorded for `TrackTrain::dmg`).
  `func_plat`/`func_platform` also gained a `platform_offset` render/
  collision offset mirroring `door_offset`'s: previously `Platform`
  advanced its own `MoverState`/timer but nothing translated that into a
  visible or collidable brush move at all. Tested with synthetic
  `ohl-physics` fixtures (a platform brush ridden up, down and sideways
  with zero player input) and a synthetic `ohl-engine` test riding a real
  `func_train`/`path_corner` chain through `Simulation`. Verified against
  the real payload: the campaign start map's opening tram sequence does
  *not* trip the new "The player is riding a mover." script-log line —
  not because of any camera override (this map has no `trigger_camera`
  entity at all), but for two independent, open reasons: no
  `func_tracktrain` there is ever started (every one has `start_speed`
  `0`, and nothing this project's map logic does within a 40-second
  scripted run fires one another way), and the player is not standing on
  the tram's collision brush at spawn either way (`ground_brush` reads
  `None` for the whole run; the player instead falls several hundred
  units onto ordinary world geometry). `cargo xtask combat-smoke`'s
  `BASE_ABSENT` set asserts the line absent on 22 of the 23 scenarios as a
  regression guard; the start-map scenario is carved out into its own
  `FIRST_CHAPTER_START_ABSENT` (documenting both gaps) rather than also
  asserting absent there, since the correct expectation once both gaps
  are fixed is *present*, not absent.
  [PR #97](https://github.com/timo-42/open-half-life/pull/97)'s review
  caught the original (incorrect) camera-override attribution via a
  real-payload instrumentation pass; also from that review:
  `Game::restore` now syncs brush collision with a zero `dt` at the end of
  restore, so a mover saved mid-move does not have its whole restore
  displacement counted as one step's velocity by the next tick's mover
  push; `hull::combine` now records `Trace::brush_index` when a brush's
  segment starts embedded even on a fraction tie against a
  likewise-embedded world trace, so the push path is not silently skipped
  for a player standing in both; and `door_offset`/`platform_offset` now
  share one `mover_offset` helper instead of two independently
  hand-copied implementations.
- **Follow-up: monster `Transform` was never synced from `Actor`.** A
  read-only investigation on main found that `ohl_ai::AiWorld::tick`
  writes a monster's walking/chasing/fleeing move to `Actor::origin`/
  `yaw` alone: nothing in `ohl-engine` copied that back onto the
  monster's `Transform`, so a moving monster's rendered model — and the
  hitbox index `crate::combat::rebuild_hitbox_index` rebuilds from
  `Transform` each step — stayed pinned at wherever it last stood while
  the AI itself kept walking, sensing and attacking correctly underneath.
  A second, related gap: `Game::restore` (save/load) already restored
  every monster's `Transform` from the save, but never touched `Actor`,
  which stayed at `ohl_ai::attach_monsters`'s spawn-time default — so a
  reloaded monster's very next think step sensed, navigated and attacked
  from the map's spawn point rather than from where the save actually
  left it. Fixed with two small, additive syncs: a new phase 8b
  (`crate::systems::Systems::sync_monster_transforms`, right after phase
  8's AI think) copies `Actor` onto `Transform` for every monster not
  currently possessed by a `scripted_sequence` (`ohl_ai::ScriptHold`
  absent — a possessed monster's `Transform` stays whatever `crate::ai`'s
  own `place` last set), and `Systems::sync_actor_from_transforms`, run
  once at the end of `Game::restore` after tags 18/24/25 apply, copies
  the reverse direction. `crates/ohl-engine/tests/
  monster_transform_sync.rs` asserts both: a walking monster's `Transform`
  now tracks its `Actor` (it visibly diverged before this fix), and a
  monster's `Actor::origin` matches its `Transform::origin` immediately
  after a save/load boundary rather than silently reverting to spawn.
- **`func_ladder`/`func_water` submodels now attach their own contents.**
  Closes the gap `docs/FORMAT_SOURCES.md`'s "brush-collision follow-ups"
  item 23 recorded: `ohl_physics::hull::CollisionModel::
  attach_contents_brush` attaches a `func_ladder`/`func_water` submodel as
  a non-solid *contents volume* (`ContentsKind::Ladder`/`Water`/`Slime`/
  `Lava`, the latter three read from `func_water`'s documented `skin`
  keyvalue) alongside the existing solid-brush attachment, so
  `contents_at`/`point_contents` report the right contents inside the
  volume while a trace still passes through it — implemented as a
  query-time remap of the submodel's own compiled "inside the brush"
  leaf, since nothing in the BSP format lets a brush entity declare
  special leaf contents at compile time (see `ContentsKind`'s doc
  comment). `ohl_game::brush::contents_model_instances` (plus a new
  `Water`/`Liquid` registry component) classifies which entities qualify;
  `ohl-engine`'s `attach_brush_collision` (renamed from
  `attach_solid_brushes`) attaches both kinds into the same
  `Vec<(Entity, BrushId)>` `Level::sync_brush_collision` already moves
  every step, so a `func_water` mover (documented as sharing `func_door`'s
  move/trigger behaviour) is kept at its current origin with no new code
  path. `ohl-player`/`ohl-physics`'s existing ladder-climb and
  water-swim/no-fall-damage logic already reads probed contents, so
  neither needed a change. `Game::player_on_ladder`/`player_in_water` and
  two new fixed `--script-log` lines, "The player is on a ladder."/"The
  player is in water." (a 0.5-second hold, like the existing in-solid
  guard), back a scripted-walk assertion the same way the PR #91 guard
  does. Tested with synthetic entity-attached ladder/water fixtures
  (`ohl_physics::test_support::build_ladder_entity_room_bsp`/
  `build_water_entity_room_bsp`, compiled the way a real brush entity
  actually compiles — an ordinary solid-shaped submodel, not
  world-baked special contents) across `ohl-physics`'s unit, player-
  systems and proptest suites, plus `ohl-game` classification tests and
  an `ohl-engine` wiring test exercising `attach_brush_collision`
  directly. All 23 pre-existing `combat-smoke` scenarios still pass.
  **Real-payload verdict:** every one of the Hazard Course's seven maps
  was probed for `func_ladder`/`func_water` entities directly from the
  imported payload's own compiled BSP data (no `func_water` entity exists
  on any of them — the course's water section is world-baked liquid,
  already working before this change); reaching one of the several
  `func_ladder`s within a bounded scripted walk was attempted extensively
  (entity/trigger-level analysis, a purpose-built collision-based
  waypoint search against this crate's own `CollisionModel::trace`, and
  dozens of scripted-walk iterations) but not completed — the course's
  early rooms are gated by sequential doors, trigger strips and a static
  hologram guide that the project's existing forward/turn scripted-walk
  technique could not reliably clear within a bounded budget. This is
  left as a follow-up rather than recorded as a passing scenario.
- **M7.13: `monstermaker` activation by `targetname`, and `SECTION_MOVER_STATE`
  (tag 28).** A `monstermaker` without `Start On` now begins spawning only
  once triggered by name, riding the same `target`-firing path
  `scripted_sequence`'s `ScriptActivation` already uses
  (`ohl_game::registry::MakerActivation`, drained by `ohl_engine::ai`'s
  `Spawner::trigger`); a second trigger on an active maker toggles it off
  (a product decision, applied uniformly to `Cyclic` and non-`Cyclic`
  makers alike, recorded beside `ohl_ai::Spawner::trigger`'s own doc
  comment and in `docs/FORMAT_SOURCES.md`'s "Monster definitions", since no
  public source states the exact retrigger rule), and one trigger (or
  `Start On`) starts a continuous, `delay`-paced spawn loop, still bounded
  by `monstercount`/`m_imaxlivechildren`. A new save section,
  `SECTION_MOVER_STATE` (tag 28, additive, following tags 23-27's own
  `optional_section`/spawn-index/float-sanitizing conventions exactly),
  closes three previously documented mid-sequence save/load gaps at once:
  `ohl_game::track_train::TrackTrainState` and
  `ohl_game::camera::TriggerCameraState` (neither serializable before this)
  now carry their position/hold/active state across a save, a running
  `scripted_sequence`'s phase/timers/bound-monster now resume instead of
  resetting to dormant, and a `monstermaker`'s spawn counters (not its
  already-spawned children — still a separate, documented gap) survive a
  reload. The same section also carries `AutoTrigger::fired`, on that
  field's own long-standing "Save/load note" doc comment inviting exactly
  this fix: without it, every `trigger_auto` on a map replays on load and
  silently re-toggles (stopping) any train/camera the rest of this section
  had just restored to an active state.

- **Review follow-up: `monstermaker`'s `Cyclic` flag reconciled with its
  own cited source, `MakerActivation::pending` also carried by tag 28, and
  a decode-time size cap.** [PR #98](https://github.com/timo-42/open-half-life/pull/98)'s
  review found the `Cyclic` implementation above had drifted from
  `docs/FORMAT_SOURCES.md`'s own recorded citation for it ("keep spawning
  rather than stopping after one quota"): an earlier draft made one trigger
  worth exactly one child, an untimed, discrete step that ignored `delay`
  and stopped a `Cyclic` maker's production far short of `monstercount`
  from a single trigger — the opposite of the cited phrase. `Spawner`'s
  `Cyclic` path now runs the same continuous, `delay`-paced loop a
  non-`Cyclic` maker's own trigger/`Start On` already runs (so `Start On` +
  `Cyclic` together also start that loop immediately, with no trigger
  needed), still hard-capped by `monstercount` either way; see the new
  `docs/FORMAT_SOURCES.md` addendum (appended, not rewriting the original
  citation) and `spawner.rs`'s own module doc for the exact, honestly
  `TODO(black-box)`-flagged reading. The review also found that
  `MakerActivation::pending` (bumped by phase 12's `Simulation::activate`,
  drained by phase 10 the *next* tick) was not itself part of tag 28: a
  save taken in that one-tick window lost a trigger that had fired but not
  yet reached the `Spawner`; it is now part of the maker's own
  `SECTION_MOVER_STATE` entry and restored before phase 10 next runs. The
  now-unused `MakerActivation::take()` (superseded by draining the whole
  counter at once) was removed. Tag 28's own decode
  (`crates/ohl-engine/src/save.rs`) now goes through a bounded
  `Vec<Option<T>>` visitor (`crate::save_state::MAX_SNAPSHOT_MOVERS`) that
  caps the element count — and so the allocation — *before* trusting a
  section's own claimed length, rather than letting `postcard` pre-allocate
  from an untrusted length prefix first; tags 24/25 share the same
  large-allocation-before-validation shape and were left unchanged here (a
  pre-existing, non-regressed pattern, not part of this fix). A redundant
  second `Vec` allocation in `Systems::restore_mover_state` was also
  collapsed into one, and the review's finding that a `Remove On fire`
  `trigger_auto` already fired before a save has no live `AutoTrigger` for
  tag 28 to read was checked end to end: that entity is despawned again on
  load anyway, by `SECTION_ENTITY_COMBAT` (tag 24)'s own pre-existing
  despawn-if-not-live rule, so it does not refire — confirmed with a
  dedicated test, not merely asserted in a doc comment.
- **Follow-up: investigating why the campaign start map's tram sequence
  drops the player.** PR #97's review reported every `func_tracktrain` on
  the start map parked at `startspeed 0` for a 40-second idle run, with
  the player falling from spawn onto world geometry rather than landing on
  the tram. Re-investigating against the real payload (classname/aggregate
  facts only, per `docs/CLEAN_ROOM.md`) found two of the three suspected
  causes were already fine and one real, independent gap:
  - The activation chain (`trigger_auto` → `multi_manager` →
    `func_tracktrain`) is present on the real map and already works:
    instrumented locally, the train's own `moving` flag turns on a few
    seconds after load, matching a deliberate pre-departure pause — this
    was not the blocker.
  - `func_tracktrain`/`func_train` spawn placement and its render/collision
    offset (`crates/ohl-game/src/track_train.rs`, `ohl-engine`'s
    `track_train_transform`/`brush_offset`) **was** the blocker, and this
    entry's original claim that it was "confirmed already correct" was
    wrong. The offset was measured from the chain's first node, which
    cancels to exactly zero at spawn — so a train whose brushes are
    authored away from its own track (which the start map's tram is, tied
    to the world only through the origin-brush position the compiler
    writes into its `origin` keyvalue) never moved onto that track at all,
    and there was nothing under the player's spawn to stand on. Only a
    train a map happens to author sitting on its first node placed
    correctly, which is why reading the code without instrumenting it
    looked right. Fixed in the "Track-train spawn placement" entry below
    — reverting only that fix makes both start-map smoke scenarios fail
    again, which is the check this bullet originally lacked.
  - A real, separate gap: GoldSrc's documented `game_playerspawn` special
    `targetname` convention (an entity named this way is activated once
    per player spawn, independent of `trigger_auto`; see
    `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic") was never
    implemented at all — confirmed by grep, zero prior references in this
    crate. `ohl_game::logic::Simulation::fire_player_spawn` now activates
    every `game_playerspawn`-named entity once, the first tick after load.
    This is a genuine, independently useful engine capability, but the
    start map's own tram does not use this convention (confirmed absent
    from its entity lump), so this fix alone does not change its
    behaviour.

  The start map's actual fall was root-caused by direct instrumentation:
  the player is in unbroken free-fall from the very first tick (never
  touching anything, health steady at 100 until lethal impact well under
  a second later), which happens regardless of whether the tram has
  started moving yet. This traces back to the same gap the "Mover riders"
  entry above closed on `main` while this investigation was in progress
  (PR #97, "Carry the player on moving brush entities," merged after
  branching): the start map's intro depends on the player being
  physically carried by/resting on the tram over what is otherwise open
  space. That mechanic landing was necessary but not sufficient — with it
  in, the fall persisted, because the tram itself was still parked where
  its brushes were compiled rather than on its track; see the corrected
  placement bullet above and the entry below.

- **Track-train spawn placement (start-map tram).** A `func_train`/
  `func_tracktrain` is now placed *on the first node of its own path* at
  spawn, rather than left wherever its brushes were compiled.
  `ohl-engine`'s `track_train_transform` previously measured the train's
  travel from the chain's first node (the since-removed
  `TrackTrainState::built_origin`), which makes the spawn offset
  identically zero — so a map that authors the
  train's brushes away from its track, tied to the world only through the
  origin-brush position the compiler writes into the entity's `origin`
  keyvalue, never moved its train onto the track at all. The offset is
  now measured from that `origin` keyvalue, so the sum every caller
  already forms (`origin` + offset, in both the renderer and
  `Level::sync_brush_collision`) cancels to the absolute path position
  exactly once — which keeps `.plan/fidelity-round-2.md` finding E1
  (returning the raw polyline coordinate, which the caller then
  double-applies the keyvalue to) fixed. This follows the public
  documentation's description of a train riding its path on its origin
  brush: `height` is "the height above the path_track that the train will
  ride, based on the location of the train's origin brush"
  (`docs/FORMAT_SOURCES.md`, "Track trains and paths").
  `Level::attach_brush_collision` now attaches every brush hull — solid
  and `func_ladder`/`func_water` contents alike — at that same
  already-placed position too, so a train is on its track for the level's
  very first tick rather than one tick later — which is the tick a player
  the map spawned standing inside it is first traced against.
  Verified against the real payload with local, since-reverted
  instrumentation (aggregates and spawn-relative values only, per
  `docs/CLEAN_ROOM.md`): before the fix a standing-hull trace 256 units
  straight down from the campaign start map's player spawn hit nothing at
  all; after it, that trace stops on an attached brush a few tens of units
  down. This closes both gaps recorded on the M7 mover-riders entry above:
  the map's trains *are* started by its own trigger chain a few seconds
  in, and the player *is* standing on one at spawn. A 40-second scripted
  idle run on that map now logs "The player is riding a mover." (and "The
  player moved from the spawn point.", from the ride alone), with no
  movement key pressed and no "inside solid geometry" line; a headless
  capture over the same window shows the view riding out of the start
  area, through the hazard-striped tunnel portal and on down the rock
  tunnel past a lamp-lit side ledge.
  Both `cargo xtask combat-smoke` scenarios that run on that map
  (`first_chapter_start.txt` and `walk_black_mesa_inbound.txt`) assert the
  riding line present accordingly, and its tick counts were corrected: a script line's
  leading number is a count of ticks (`CAPTURE_STEP`, 1/60 s), not
  seconds, so the previous counts covered under two seconds of simulation
  and never reached the ride. Regression tests:
  `crates/ohl-engine/tests/train_spawn_placement.rs` (a synthetic train
  compiled far from its own track, with the player start authored inside
  the car at the first node) and a new
  `crates/ohl-physics/tests/hull_trace.rs` case pinning that the
  bare-contents head-link skip is per hull, and so was never what removed
  the tram's floor.

- **Probe ladders with the player hull, not a point.** PR #100 review
  follow-up: `ohl_physics::movement::in_ladder_volume`/`ladder_normal`
  tested only `PlayerState::origin`, so a player whose standing/crouched
  hull box overlapped a `func_ladder` volume (world-compiled or, since PR
  #100, an attached brush-entity contents volume) but whose origin point
  sat just outside it never attached. Both functions now probe a bounded,
  project-owned sample set (`ladder_hull_samples`: the hull box's eight
  corners, six face centres, and the origin) against
  `CollisionModel::contents_at`; `ladder_normal` derives its escape
  direction from whichever sample actually touched the volume (the origin
  when it did, otherwise the first hull sample that did, in a fixed
  order), radiating outward the same four-direction/twelve-step probe this
  crate already used, nudged `DIST_EPSILON` past each grid step so a probe
  landing exactly on a compiled brush face is not misread as an opening
  next to a wall built flush against the ladder. PR #59's attach/detach
  hysteresis (`ladder_lockout`) is unchanged. New synthetic fixtures/tests
  (`ohl_physics::test_support::build_thin_ladder_room_bsp`, a free-standing
  8-unit-thick volume) prove a hull-overlap-only attach, a climb from that
  position, and detaching once the whole hull clears the volume's top; a
  512-case proptest across both the entity-attached and thin-ladder
  fixtures asserts the probe never panics for an arbitrary hull placement
  and that its normal is always finite, at most unit length, and zero
  exactly when not attached. **Real-payload verdict:** unlike PR #100's
  attempt to reach a `func_ladder` by progressing through the Hazard
  Course's hub map (`t0a0`) — which did not complete within a bounded
  scripted-walk budget — this change instead loads one of the course's
  own sub-maps, "t0a0a", directly (`--map`, the same technique the M9
  chapter-walk scenarios already use to start mid-campaign), whose own
  player start faces close enough to a `func_ladder` that a short,
  turn-then-walk script (well under 60 simulated seconds) reaches it: "The
  player is on a ladder." fires. Added as a 24th `combat-smoke` scenario
  (`xtask/smoke-scenarios/ladder_t0a0a.txt`, "reach and climb a ladder in
  the Hazard Course"); all 24 scenarios pass against the real imported
  payload.

- **M7.11 follow-up: the pre-trigger idle.** `docs/FORMAT_SOURCES.md`'s
  `TODO(black-box)` item 21 previously left `m_iszIdle`'s published
  "on a loop until the scripted_sequence is triggered" behaviour only
  partially modelled: a dormant script never touched its monster at all,
  so the idle animation never played before the first trigger.
  `ohl_ai::scripts::ScriptRunner::pretrigger_idle_sequence` now reports
  that idle by name whenever the script is `Dormant` and named a working
  `m_iszIdle`, without becoming a `ScriptAction` and without the script
  holding the monster — `ScriptHold` is still only ever inserted for a
  `Moving`/`Playing` script, so the "a script only owns an actor while it
  is holding it" contract from PR #75/#82 is unchanged. `ohl_engine::ai`'s
  `AiState::apply_pretrigger_idles` applies it last in `think`, after the
  monster's own `AiWorld::tick` has already resolved its own activity
  into a sequence, and only when that monster's own `MonsterAi` reports
  `MonsterState::Idle` and `Activity::Idle` — the project-authored
  precedence rule item 21 now records: the script's idle wins the tie
  only while the monster's own AI is idle, and loses to anything else the
  monster's own brain is doing (walking, alert, fighting, following). A
  monster with no `MonsterAi` at all is treated as not eligible, since
  there is no AI state to confirm as idle. Covered by `ohl-ai` unit tests
  over the eligibility rule itself (dormant with/without a name, excluded
  on `aiscripted_sequence`, absent while moving/playing/repeating/done,
  and reachable again once interruption or `Repeatable` returns the
  script to `Dormant`) and by an `ohl-engine` integration test
  (`a_dormant_scripts_idle_animation_plays_while_its_monster_is_idle_and_yields_otherwise`)
  over a two-sequence synthetic model that tells the AI's own idle and
  the script's `m_iszIdle` apart by sequence index: the script's idle
  plays while the guard is idle and untouched in position, steps aside
  the moment the guard acquires an enemy, and the ordinary triggered path
  still takes over and hands the guard back to its own idle once the
  script completes.

## Status as of 2026-09-07 (wave 3)

This section is a rollup of the detailed entries directly above (all landed
between the "Status as of 2026-09-07" snapshot's own initial writing and
this addendum); it groups them by theme for a reader who does not want the
full narrative, and does not repeat their detail.

Merged, by theme:

- **Xen landing unstuck, dead-player simulation gated.**
  [PR #96](https://github.com/timo-42/open-half-life/pull/96) fixes a hard
  Xen-map landing that could resolve to an origin still read as embedded in
  solid geometry, adds `unstick_from_ground`, and stops `Systems::step`/
  `run_scripted` from continuing to simulate and move a player who has
  already died.
- **Mover riders and `func_plat` motion.**
  [PR #97](https://github.com/timo-42/open-half-life/pull/97) carries the
  player along on a moving brush entity (`func_train`/`func_tracktrain`/
  `func_plat`/lift `func_door`) instead of leaving them behind or letting
  them sink through it, and gives `func_plat`/`func_platform` the render/
  collision offset (`platform_offset`) it was missing, so a platform's own
  `MoverState` timer actually translates into a visible, collidable move.
- **`monstermaker` triggers, and tag-28 mover/camera/script/maker/
  auto-trigger persistence.**
  [PR #98](https://github.com/timo-42/open-half-life/pull/98) lets a
  `monstermaker` without `Start On` be started (and toggled off again) by
  its own `targetname`, and adds `SECTION_MOVER_STATE` (save tag 28),
  which closes three previously open save/load gaps at once: a
  `func_train`/`func_tracktrain`'s mid-route position, an active
  `trigger_camera` sequence, and a running `scripted_sequence`'s
  phase/timers/bound monster now all resume from a save instead of
  resetting to dormant; a `monstermaker`'s spawn counters (not its already-
  spawned children) survive a reload; and a `trigger_auto`'s one-shot
  `fired` flag is carried too, so replaying it on load no longer silently
  re-toggles a train/camera the rest of the same section had just restored
  to an active state.
- **Monster `Transform`/`Actor` sync.**
  [PR #99](https://github.com/timo-42/open-half-life/pull/99) adds phase 8b
  (`Systems::sync_monster_transforms`), copying a walking/chasing/fleeing
  monster's `Actor` position onto its `Transform` every step so its
  rendered model and hitbox actually track its AI-driven motion instead of
  staying pinned at its last position, and `Systems::sync_actor_from_
  transforms`, run once after a `Game::restore`, so a reloaded monster
  senses, navigates and attacks from where the save left it rather than
  snapping back to its map spawn point.
- **`func_ladder`/`func_water` contents volumes.**
  [PR #100](https://github.com/timo-42/open-half-life/pull/100) attaches a
  `func_ladder`/`func_water` submodel as a non-solid contents volume
  (ladder, water, slime, or lava, the latter three from `func_water`'s
  `skin` keyvalue) alongside its existing solid-brush attachment, so a
  brush-entity ladder or body of water behaves distinctly from ordinary
  solid geometry, not only world-baked contents.
- **`game_playerspawn`.**
  [PR #101](https://github.com/timo-42/open-half-life/pull/101) implements
  GoldSrc's `game_playerspawn` special-`targetname` convention (fired once
  per player spawn, independent of `trigger_auto`), which had never been
  implemented at all.
- **Track-train spawn placement — the intro tram ride works.**
  [PR #102](https://github.com/timo-42/open-half-life/pull/102) places a
  `func_train`/`func_tracktrain` on the first node of its own path at
  spawn (measuring its offset from the entity's own `origin` keyvalue
  rather than from the path chain's first node, which had cancelled to
  zero), instead of leaving it wherever its brushes were compiled. This
  was the real root cause of the campaign start map's tram sequence
  dropping the player into free-fall; combined with the mover-riders fix
  above, the intro tram ride now plays end to end on the real payload — a
  scripted idle run rides the tram out of the start area, through the
  hazard-striped tunnel portal, and down the rock tunnel.
- **Hull-box ladder probe, and a 24th combat-smoke scenario.**
  [PR #103](https://github.com/timo-42/open-half-life/pull/103) probes a
  `func_ladder` volume with the player's own standing/crouched hull box
  (eight corners, six face centres, and the origin) instead of a single
  origin point, so a player whose hull overlaps a ladder volume but whose
  origin sits just outside it still attaches; verified against the real
  payload by reaching and climbing a `func_ladder` on the Hazard Course's
  "t0a0a" sub-map, added as the 24th `combat-smoke` scenario.

As of this snapshot, [PR #104](https://github.com/timo-42/open-half-life/pull/104)
("Play a scripted_sequence's idle animation before it is triggered") is
open, implementing the `m_iszIdle` pre-trigger idle animation this
document's follow-up lists have long recorded as missing. Treat it as
in-flight, not yet landed, until it shows as merged.

Remaining follow-ups, in no particular priority order (superseding the
equivalent bullets in the 2026-09-06 and initial 2026-09-07 snapshots above
where they overlap):

- **`m_iszIdle` pre-trigger idle animation.** In flight as
  [PR #104](https://github.com/timo-42/open-half-life/pull/104) (see
  above); not yet landed on `main`.
- **Brush `angles`.** A brush entity's own `angles` keyvalue (a rotated
  door or platform) is still not applied to its collision shape; no PR
  addressing this was open at the time of this snapshot.
- **`func_pendulum`/`momentary_rot_button`.** Neither entity is
  implemented yet; both depend on the same brush-`angles` gap above for a
  collidable rotating shape, not only a rendered one.
- **Monster children of a `monstermaker` are not persisted.** *Fixed at
  M9.5 (below): tag 28 (`SECTION_MOVER_STATE`) still carries only a
  maker's own spawn counters, but the new `SECTION_MAKER_CHILDREN` (tag
  29) now separately restores the already-spawned children themselves,
  which are indexed by `Registry::entities` as of this package (see
  `ohl_engine::save_state`'s module doc, "Monstermaker children are now
  saved").* `func_tracktrain`/`func_train` altpath branching is likewise
  still recorded but not applied.

## M9.5 (Rust): `monstermaker` children survive a save/load

Status: accepted; evidence: PR #<n> ("Persist a `monstermaker`'s runtime-
spawned children across a save/load").

The gap the bullet just above used to describe in full — a live
`monstermaker` child (a monster the map itself never declared, spawned at
runtime by `ohl_ai::AiState::spawn_child`) was lost across a save/load,
because it was never indexed by `Registry::entities` at all — is closed.
`AiState::spawn_child` now pushes every child it creates onto
`Registry::entities` the instant it spawns (both the runtime path,
`AiState::tick_makers`, and the new restore path below use the same
function), so a maker child gets a spawn index exactly like a
map-declared monster and is covered automatically by every existing
index-keyed save section: `SECTION_ENTITY_REGISTRY` (18, transform),
`SECTION_ENTITY_COMBAT` (24, health/armor) and `SECTION_AI` (25,
schedule/enemy memory/route state) all already restore a maker child
correctly with no changes of their own.

What those sections cannot do on their own is recreate the child *entity*
after a load: a fresh `attach_level` only ever spawns a map's own declared
entities, never a maker's dynamically-created children, so
`Registry::entities` is shorter than the save recorded until something
fills the gap back in first. The new `SECTION_MAKER_CHILDREN` (tag 29,
`ohl_engine::save_state::MonsterMakerChildSnapshot`) records just enough
per registry slot — which maker spawned it (as a spawn index) and its
spawn classname — for `AiState::restore_maker_children` to recreate a
placeholder monster of the right kind at the right index, run first in
`Game::restore`, ahead of every other index-keyed section's own restore.
`AiState::finalize_maker_children` then runs last (after
`SECTION_ENTITY_COMBAT`/`SECTION_AI` and `Systems::
sync_actor_from_transforms` have all applied the save's actual health/AI
state), linking each recreated child back onto its maker's own
`ohl_ai::Spawner::children` list (`Spawner::restore_child`, new) and
pruning any that are already dead — the same `is_alive` check
`AiState::tick_makers` uses mid-session — so `Spawner::live_children`/
`has_room` (and so a reloaded maker's remaining-spawn accounting) read
correctly immediately after a load, not just its `spawned_total`/`active`/
`timer` counters (which already round-tripped through tag 28 before this
package).

A child already gone by save time — gibbed outright, or a faded corpse
`AiState::age_corpses` had already despawned — has no classname left to
record (its components are gone with the entity), so its
`SECTION_MAKER_CHILDREN` slot is `None`; restoring a `None` tail slot
spawns an inert, component-less placeholder purely to keep every later
child's own index aligned, mirroring the pre-existing rule
`SECTION_ENTITY_COMBAT` already applies to any other entity gone by save
time. Recreation is capped by the same `MAX_MAKER_CHILDREN_PER_LEVEL`
guard `AiState::tick_makers` already enforces at runtime, so a corrupt or
adversarial save cannot use this restore path to spawn unbounded
monsters. Tag 29 is read as absent (defaulting to no maker children
recreated, the documented pre-M9.5 behaviour) when missing, so a save
written before this package still loads.

**Level transitions do not carry a live `monstermaker` child across a
`trigger_changelevel`, and this package leaves that scope unchanged.**
`crate::transition::TransitionState::capture` only carries an entity that
has a `targetname` or a `globalname` of its own; `AiState::spawn_child`
gives a maker child neither (it is built from the maker's
`monstertype`/keyvalues alone), so it was already excluded by that same
name test before this package existed and still is now that it carries a
spawn index. Extending transitions to carry a maker child by, for
example, its owning maker's own `targetname` plus an ordinal is left for
a later package; this one is bounded to the save/load round trip alone.

Tests (`crates/ohl-engine/tests/save_sections.rs`): a `monstermaker` with
two spawned children, one of them killed before the save, round-trips
both — the live one keeps thinking from its saved position/health and
counts toward `Spawner::live_children`, the dead one stays a corpse and
does not; and a save written before tag 29 existed (no
`SECTION_MAKER_CHILDREN` section at all) still loads, with the
pre-existing "children not restored, only counters" behaviour, exactly as
`a_monstermakers_counters_survive_a_save_load_round_trip` already
documented before this package updated it to assert the fix instead.
- **Linux audio backend decision** (unchanged from the 2026-09-06 and
  initial 2026-09-07 snapshots): `ohl-audio` still always uses a
  `NullSink` on Linux, since `cpal`'s only Linux backend links `libasound`
  through a build-time `pkg-config` lookup, which this project's "no FFI"
  rule forbids as written; whether to relax that rule, adopt a pure-Rust
  ALSA/PipeWire backend if one appears, or leave Linux silent by design
  remains open.
- **First tagged release.** No `v*` tag has been pushed yet; the next
  actual tag push will be `publish-release`'s (PR #83) first real exercise
  and the first entry in this project's GitHub Releases.
- **Headless capture: spawn offsets ride with the player; dev viewpoint
  works with scripts.** Fidelity round 8 (`.plan/fidelity-round-8.md`, J1)
  found `--spawn-offset` broken by the tram fix above: it was applied once
  via `Game::set_viewpoint` (noclip) right after the map loaded and never
  touched again, so on a mover map the player kept riding away while the
  frozen camera stayed behind, ending the capture embedded in geometry the
  mover had vacated. `--spawn-offset` no longer calls `set_viewpoint` or
  enables noclip at all: the player spawns and moves normally (falling,
  colliding, riding a mover), and each rendered frame recomputes the eye
  as the player's *current* position plus the offset
  (`ohl_engine::Game::render_from`, new — swaps the tracked camera in for
  one draw call and restores it, so the next tick's physics is untouched).
  `--viewpoint` keeps the old frozen-in-world-space/noclip behaviour,
  unchanged, since it is documented as an absolute pose. The
  "starts inside solid geometry" warning is now also checked at the final
  frame ("ends inside solid geometry", printed once) for both the frozen
  and rider modes. Separately (J4), `--viewpoint-at-nearest-monster` was a
  silent no-op under `--script` (`run_scripted` never called the
  placement helper `capture` did); it is now applied once, right after the
  map loads and before the script's own ticks run, in both paths.
  Regression tests: `crates/ohl-app/src/game_run.rs` unit tests against a
  new `ohl_engine::test_support::mover_train_bsp` fixture (a rider pose
  tracks the player's live eye position across a full mover ride, never
  lands in solid geometry while riding, and is unchanged frame-to-frame on
  a static map once the player has settled); `crates/ohl-app/tests/
  spawn_offset_follows_the_rider.rs` and `crates/ohl-app/tests/
  viewpoint_at_nearest_monster_under_script.rs` (dev-tools only) exercise
  the same properties through the built binary. **Real-payload verdict:**
  re-ran round 8's own broken `c0a0` "forward"/"left" `--spawn-offset`
  captures at 60 and 900 frames against the real imported payload — no
  "inside solid geometry" warning at either frame count on either
  viewpoint (previously frame 900 landed on an indistinct, blown-out,
  heavily-clipped close surface); the 900-frame "forward" capture instead
  shows the camera riding well down the tunnel, past the rail bed, a
  curving wall and a strip ceiling light, matching round 8's own
  `--script`-based tram-ride description at the same simulated time.


- **M9.4 (rotating brush entities).** `func_door_rotating`/`func_rotating`
  now actually rotate, in both collision and render, instead of sitting
  frozen at their compiled orientation. `ohl_physics::hull::
  CollisionModel::set_brush_pose` extends the brush-attachment mechanism
  ("Collision hulls and player movement" in `docs/FORMAT_SOURCES.md`) with
  an optional rotation about a caller-given pivot — a query is
  inverse-rotated into the brush's compiled frame before the existing
  hull-tree walk, and the hit position/normal are rotated back, with the
  broad-phase AABB re-derived from the brush's rotated corners so it stays
  conservative (a proptest caught and fixed a real bug in this path: the
  test-only "widen the broad-phase bounds to infinity" helper produced NaN
  once its `±INFINITY` corners were rotated, which made the broad phase
  wrongly reject every segment instead of accepting all of them).
  `ohl_game::registry::Door` gained `rotation_axis: Option<Vec3>` for
  `func_door_rotating`, reusing `Door`'s existing timer/state machine with
  `travel_distance` read as degrees instead of units; `func_rotating` is a
  new `Rotator` component/state advanced every fixed step. `ohl-engine`
  feeds the identical `(pivot, axis, angle_degrees)` triple into both the
  submodel's draw transform (`render::rotated_placement`) and the attached
  collision brush's pose (`level.rs`), so a rotating door blocks and pushes
  the player at the pose it is drawn at. See `docs/FORMAT_SOURCES.md`
  `TODO(black-box)` item 24 for what remains open (an activator-relative
  opening direction, `func_pendulum`/`func_rot_button`/
  `momentary_rot_button`, and a rotating mover carrying a standing rider).
  A local classname survey of the campaign's own entity lumps (91 of 96
  `ohl_campaign::CHAPTERS` map names loaded from the payload's `pak0.pak`,
  including the Hazard Course; 5 map names were absent) found rotating
  brush entities are common: 179 `func_door_rotating` across 43 maps
  (including the Hazard Course's `t0a0`/`t0a0b`/`t0a0d`), 119
  `func_rotating` across 34 maps, 48 `func_pendulum` across 10 maps, 31
  `func_rot_button` across 18 maps, and 12 `momentary_rot_button` across 9
  maps (including the Hazard Course's `t0a0a`). Inspecting a real
  `func_door_rotating`'s own compiled submodel bounds against the payload
  directly (its `BSPMODEL::mins/maxs` sit in a small box near local `(0, 0,
  0)`, nowhere near the entity's `origin` keyvalue) caught a real bug this
  milestone's first synthetic fixture had masked: `attach_brush_collision`/
  `Level::sync_brush_collision`/`render::rotated_placement` originally
  treated a rotating brush's compiled geometry as already sitting at its
  world-space pivot (correct only for geometry authored with no origin
  brush at all), when a real `func_door_rotating`'s geometry — like a
  `func_train`'s (`render::track_train_transform`'s own doc comment) — is
  compiled *relative to* its origin brush instead. Both are now fixed to
  translate by the `origin` keyvalue and rotate about the submodel's own
  local `(0, 0, 0)`, and `crates/ohl-engine/src/test_support.rs`'s
  synthetic fixture was rebuilt to compile the same way (geometry relative
  to a `ROTATING_DOOR_PIVOT` constant, matching the real convention) so it
  cannot mask the same class of bug again. A scripted walk toward a real
  `t0a0b` `func_door_rotating` reached within `use` range
  of its pivot (about 25 units) but did not reach a point where opening it
  demonstrably freed the player's path within this milestone's budget — a
  blind turn-then-walk script cannot navigate a real map's corridors and
  doorframes the way a player would; `docs/FORMAT_SOURCES.md`'s
  `TODO(black-box)` item 25 also records a related, separately-scoped gap
  this same probe surfaced (`ohl_game::registry::BrushCenter`, what a
  proximity-based `use` press targets, is computed from a submodel's raw
  compiled bounds with no `origin`-keyvalue offset added, so it is wrong
  for any origin-brush entity, not only a rotating one). The engine-level
  fixture tests (`crates/ohl-engine/tests/rotating_door.rs`) built on the
  corrected convention do demonstrate a rotating door blocking while closed
  and letting the player through once open.
- **M9.5 (a brush entity's `use` proximity point follows its placed
  pose).** `docs/FORMAT_SOURCES.md`'s `TODO(black-box)` item 25 is fixed: a
  `use` press now measures against where a brush entity's geometry actually
  is, so a `func_door_rotating` on a real map can be opened by a player for
  the first time. The placement rule every consumer shares — compiled
  geometry, rotated about the submodel's own local `(0, 0, 0)` for a
  rotating mover, translated by the `origin` keyvalue, plus the mover's
  current displacement — is now stated once in the new `ohl_game::pose`
  module; `ohl-engine`'s renderer and brush-collision attachment re-export
  those helpers instead of keeping their own copy of the arithmetic, and
  `ohl_game::logic::find_usable_within` and the level transition's
  `entity_position` read the same function. `BrushCenter`/`BrushBounds` are
  now the entity's *placed* centre and box (compiled values plus `origin`,
  unconditionally); a survey of the campaign map set (93 map names, 92
  parsed) justified making it unconditional rather than classname-gated:
  527 of 8,622 brush entities carry a non-zero `origin` across 78 maps, and
  two of them are ordinary translating `func_door`s, which the collision
  model and renderer already placed by `origin` regardless. PR #107's two
  review follow-ups landed alongside: `func_rotating`'s state now also
  travels through a level transition (`transition::EntitySnapshot::
  rotator`, with `is_modified_mover` recognising a spinning rotator), and
  item 24's two misstatements about which save section carries a `Door` are
  struck and corrected. `crates/ohl-engine/tests/rotating_door.rs` now
  opens its door through the real `use`-proximity path rather than forcing
  the state, and a new combat-smoke scenario
  (`xtask/smoke-scenarios/use_rotating_door_anomalous_materials.txt`, run
  against `"c1a0"` from `ohl_campaign::CHAPTERS`'s own cited table) walks
  up to a real `func_door_rotating` and opens it, asserting the new fixed
  milestone line "The player opened a door."; the other 24 scenarios assert
  that line absent.

- **M9.6 (rotating movers: activator-relative doors and riders).** Closes
  two of the three gaps M9.4 left open (`docs/FORMAT_SOURCES.md`
  `TODO(black-box)` item 26). A `func_door_rotating` now swings *away* from
  whoever opened it unless its documented "One Way" spawnflag is set: a new
  spawn-time `ohl_game::registry::RotatingDoorSwing` component carries the
  spawnflag-chosen axis, the "One Way" bit, and the direction the door
  leaf's centre first moves in under a positive rotation, and
  `Simulation::activate` picks the sign of `Door::rotation_axis` from which
  side of that hinge plane the activator stands on — on the closed ->
  opening edge only, so a part-open leaf can never be mirrored mid-swing.
  Because the choice lives in `rotation_axis`'s own sign, it persists
  through the per-entity `Door` snapshot both a save (tag 18) and a level
  transition already carry, with no new save state. The player is not a
  `hecs` entity in this project, so `Simulation::set_activator_origin` (a
  per-tick scratch value, not persisted) hands the simulation the player's
  position for `use` presses and touch triggers.
  Riders: `ohl_physics::rotational_ride_velocity` computes the rigid-body
  `v = omega x r`, `Level::brush_rotation` records each attached rotating
  brush's pivot and angular velocity per step (shortest signed arc, so a
  `func_rotating`'s 360-degree wrap does not read as a backwards
  revolution), and `Level::brush_ride_velocity` sums it with the existing
  translation velocity at the player's own origin — feeding the same
  `PlayerController::base_velocity` and `push_from_mover` paths a
  translating mover already used, so a spinning platform carries a standing
  player and a swinging brush pushes rather than traps one.
  `Game::ground_mover_speed` (the dev-tools "riding a mover" line) measures
  the same combined ride. A property test caught a real bug on the way: a
  player resting on a rotating brush sits exactly on its pre-expanded hull
  plane, and the pose's inverse rotation put that point a rounding step
  inside solid on some steps, so the ground probe intermittently reported
  them embedded and dropped them off a floor they had not left;
  `ohl_physics::movement`'s `ground_probe` now retries the trace from one
  unit up and uses the retry only when it comes back clean.
  Tests: `crates/ohl-game/src/logic.rs` unit tests (both activator sides,
  "One Way", and no mirroring mid-cycle); `crates/ohl-physics/tests/
  rotating_riders.rs` property tests (the tangential-velocity relation, a
  point on the axis, and a hull resting on a slowly rotating disc staying on
  it and out of solid for every tick of a ride);
  `crates/ohl-engine/tests/rotating_riders.rs` (a `func_rotating` turntable
  in a void carries a standing player around its axis, and stops carrying
  when stopped) and `crates/ohl-engine/tests/rotating_door.rs` (a door
  opened by the player through a touch trigger flips its swing away from
  them and never leaves them in solid); `crates/ohl-engine/tests/
  save_sections.rs` (the chosen swing side survives a save/load).
  **Still open:** the player's *view* is not yawed with a rotating platform
  they ride (a view change no public source describes; deliberately not
  approximated), and rotating movers' `dmg`/blocking is still not wired
  into `Level::movers_blocked`. No combat-smoke scenario was added for the
  *rider* path: a local probe found no rotating brush entity near the
  player start of any campaign map the existing scenarios visit, so a
  scenario would have to navigate a real map's corridors to reach one.

- **M9.7 (`func_rot_button`, `momentary_rot_button`, `func_pendulum`).**
  The three rotating-mover classnames item 24's own M9.4 entry left
  unimplemented now round-trip through the map logic simulation, render,
  and collision the same way `func_door_rotating`/`func_rotating` already
  do. `ohl_game::registry::RotButton` is a new component reusing `Door`'s
  rotating `speed`/`distance`/`state`/`timer` shape and the `ohl_game::pose`
  render/collision pose mechanism M9.5 just centralised, but — unlike
  `Door` — fires its `target` on reaching the pressed pose and supports the
  documented "Toggle" spawnflag (`ohl_game::logic::Simulation::
  advance_rot_buttons`); its documented "Touch activates" spawnflag is
  enforced both by excluding a touch-only button from proximity `use`
  search and by a new edge-triggered `Simulation::touch_rot_buttons`,
  called from the same `Simulation::touch_triggers` phase every other
  touch-driven entity already uses. `ohl_game::registry::MomentaryRotButton`
  is driven every fixed step by *held* `use` (not a discrete activation,
  unlike every other entity in this crate) through a new
  `ohl_game::logic::find_momentary_rot_button_within`/
  `Simulation::drive_momentary_rot_button` pair, wired into
  `ohl-engine`'s `Systems::triggers_and_movers` (phase 12) behind
  `Input::use_held`; its `target` is documented as normally naming a
  `momentary_door`, which this crate does not implement, so its `0..1`
  `fraction` is exposed on the component for a future consumer but not
  wired to anything today. `ohl_game::registry::Pendulum` swings a plain
  damped sinusoid between `-distance` and `+distance` — this project's own
  choice, recorded at its point of use, since no public source states
  GoldSrc's exact per-step motion law — settling at rest once its damped
  amplitude falls under a small threshold (matching the documented
  "narrows... until it stops... in the middle of its swing" behaviour),
  toggled by `use`/trigger like `func_rotating`, with the documented "Auto
  Return" spawnflag animating linearly back to rest instead of freezing in
  place. All three now round-trip through a **new** optional save section,
  `SECTION_ROTATING_MOVER_STATE` (tag 30: `RotButtonSnapshot`/
  `MomentaryRotButtonSnapshot`/`PendulumSnapshot`, plus the
  `func_rot_button` touch-edge bookkeeping), each with a dedicated,
  discriminating round-trip test and a dedicated pre-existing-save
  compatibility regression. A review round found the first version of
  this milestone had instead added these same fields directly to three
  *existing, required* sections (`SECTION_ENTITY_REGISTRY`/tag 18 via
  `ohl_engine::transition::EntitySnapshot`, `SECTION_SIMULATION`/tag 19,
  and `SECTION_MOVER_STATE`/tag 28) — since `postcard`'s wire shape is not
  self-describing, that made every save written before this milestone
  existed fail to load outright (`EngineError::SaveUnreadable`), confirmed
  directly against a save built from `rotating_door_bsp` at `origin/main`.
  Moving all three entities' state to the new tag 30 fixed that, at the
  documented cost that none of the three now carries across a
  `trigger_changelevel` transition (only across a save/load) — the
  `EntitySnapshot` type that would carry it is exactly the required tag
  the fields had to come back out of. A new integration test,
  `crates/ohl-engine/tests/rot_button.rs`, drives a
  `func_rot_button` through the real proximity `use_pressed` input path
  end to end — the same real-proximity path M9.5/item 25 fixed, landed
  concurrently with this package and rebased onto directly rather than
  worked around. See `docs/FORMAT_SOURCES.md` item 27 for the full
  keyvalue/spawnflag citations and this milestone's documented
  approximations (the pendulum's motion law, the angular-frequency
  mapping, and the damping-rate constant). A bounded, aggregate-only probe
  of the real payload (parsing entities lumps directly; booleans/counts
  only, nothing media-derived committed) found `c1a4` — a map the existing
  `combat-smoke` suite already visits ("walk from spawn in Blast Pit") —
  has both a `func_rot_button` and a `func_pendulum` within 256 units of a
  spawn point, and no `momentary_rot_button` within that radius of a
  spawn on any visited map; no combat-smoke scenario was added for any of
  the three regardless. This is a budget call rather than a technical
  limit: M9.5's own `use_rotating_door_anomalous_materials.txt` scenario
  proves a tuned scripted-input file *can* walk up to and `use` a specific
  rotating entity on a real map (superseding item 24's own earlier,
  more pessimistic finding), but authoring and validating an equivalent
  file for `c1a4`'s own corridors was not done within this package's
  budget. Two pre-existing gaps remain, both already recorded elsewhere:
  `dmg`/blocking behaviour is not wired into `Level::movers_blocked` for
  any of the three (the same gap item 9, under "Mover riders", already
  records for a translating mover), and none of the three carries a
  standing rider (item 24's own reasoning for why a rotating mover cannot
  yet).

- **M9.8 (`momentary_door`).** Closes the gap M9.7's own entry above left
  open: `momentary_rot_button`'s documented `target` — a `momentary_door` —
  is now implemented. `ohl_game::registry::MomentaryDoor` is a new
  component sharing `func_door`'s translating `speed`/`lip`/`movedir`/
  `travel_distance` shape (the same `movedir_from_angles`/
  `brush_travel_distance` helpers `func_door` already uses), but with no
  `wait`/`state`/`timer` open-close cycle of its own: only
  `Simulation::drive_momentary_rot_button` ever moves its `0.0..=1.0`
  `fraction`, and `ohl_game::pose::momentary_door_offset` (chained into
  `pose::brush_offset`, the same pipeline `func_door`/`func_plat`/
  track-train offsets already share) turns that fraction into a world
  displacement the renderer, collision model and `use`-proximity search
  all agree on by construction. Each tick a `momentary_rot_button`'s own
  `fraction` changes (held, or mid "Auto return"), it pushes that value as
  a commanded fraction to every `momentary_door` sharing its `target`
  keyvalue; a second pass then moves each such door's own `fraction`
  toward the commanded value at the door's own `speed` — this project's
  own reading of the cited "synchronized... a `0` to `1` fraction" text,
  which does not say at what rate the door follows the button (`docs/
  FORMAT_SOURCES.md` item 29). A door with no button currently pushing to
  it (nothing active this tick) simply holds wherever it last stopped,
  which is what gives the documented "stays where you left it" behaviour
  for a button with no "Auto return" and the documented return-to-`0.0`
  behaviour for one that has it, without the door needing any return logic
  of its own. `MomentaryDoor::fraction` round-trips through a **new**
  optional save section, `SECTION_MOMENTARY_DOOR_STATE` (tag 31), kept
  separate from tag 30 (`func_rot_button`/`momentary_rot_button`/
  `func_pendulum`'s own state) even though the two are closely related,
  because tag 30 is already shipped and frozen at its own shape — the same
  "new state gets a new tag" rule tag 30 itself was created to follow,
  restated in `ohl_engine::save`'s module doc. A dedicated, discriminating
  round-trip test (`crates/ohl-engine/tests/save_sections.rs`), a
  pre-existing-save compatibility regression proving a save missing tag 31
  entirely still loads, and a **new** golden-bytes test for tag 31
  (`crates/ohl-engine/tests/save_format_frozen.rs`, added without touching
  any existing golden) all landed with this package. A new integration
  test, `crates/ohl-engine/tests/momentary_door.rs`, drives a
  `momentary_rot_button` through the real `use_held` proximity path
  (`ohl_game::logic::find_momentary_rot_button_within`) from the player's
  own spawn point, opening its target `momentary_door` in step and closing
  it again on release. A bounded, aggregate-only probe of the real payload
  (this project's own clean-room `ohl-formats` BSP entities-lump parser;
  a single integer count, nothing media-derived committed) found **zero**
  `momentary_door` entities anywhere in the payload, so no combat-smoke
  scenario was added for this milestone — there is no cited-table map to
  point one at, not a budget call like M9.7's own `func_rot_button`/
  `func_pendulum` gap.

## Status as of 2026-09-08

- **First spawn-to-exit progression scenario, and `combat-smoke`'s
  `--follow-level-change` support.** A 26th `combat-smoke` scenario
  (`xtask/smoke-scenarios/progress_c1a1_reach_changelevel.txt`) walks from
  spawn on "c1a1" (Unforeseen Consequences, `ohl_campaign::CHAPTERS`'s
  second chapter's first map) to a level change and follows it end to end
  (`crates/ohl-app/src/game_run.rs`'s `handle_level_change`), the first
  scenario in this suite whose script rides a chapter's own route from its
  start all the way to the next map rather than only partway. Reaching
  the level change from spawn takes under a second of simulated time on
  this map: a short turn followed by a walk in `use`-pressed steps crosses
  it almost immediately. `xtask/src/combat_smoke.rs`'s `Scenario` gained a
  `follow_level_change` field so only a scenario that opts in is run with
  `--follow-level-change`; every other scenario now also asserts "A level
  change was followed." absent, verified by running the full scenario
  suite against the real payload.
- **`c1a0` progression reached; its "blocker" was a wall, not a bug.** A
  27th `combat-smoke` scenario
  (`xtask/smoke-scenarios/reach_level_change_anomalous_materials.txt`)
  walks from spawn on "c1a0" (Anomalous Materials, the second chapter in
  `ohl_campaign::CHAPTERS`'s own cited table), opens the door on the way
  with a `use` press through the engine's own proximity search, and crosses
  that
  map's own level-change trigger in about fourteen simulated seconds,
  following it end to end. The forward-movement stop previously reported
  on this map was traced at the player's own origin: a hull trace in every
  direction returns zero fraction across the whole forward half and full
  fraction across the rear half, every blocking trace comes from the world
  tree rather than an attached brush entity, and the nearest brush entity
  of any classname is more than four times `ohl_engine::USE_RADIUS` away —
  i.e. a corner of ordinary map geometry the earlier scripted walk drove
  into, not a door that never opened, a mistreated non-solid brush, or a
  blocking NPC. A breadth-first walk over the map's own collision hulls
  confirms the rest of the route is walkable once that one door is opened,
  with no further gate needing anything this engine does not implement.
- **`c0a0` progression is still blocked, pending investigation.** On the
  campaign's very first map, forward movement stops for good just after
  the map's opening scripted ride ends, at a point that turns out to sit
  almost directly beneath the map's only found level-change trigger; that
  map's several scripted sequences also never all finish over the run
  lengths tried, and the player is not embedded in solid geometry, which
  argues against a repeat of the PR #91 class of regression and toward a
  closed door, an invisible clip brush, or an unresolved scripted gate
  facing the walk direction. No scenario for that map was added this
  round; this is left as a follow-up, and the reachability probe used to
  clear "c1a0" is the obvious next tool to point at it.
- **`--reachability-report` now models one-way falls and jumps.**
  `.plan/progress-probe-3.md`'s own follow-up investigation found the
  walk's original 72-unit drop bound (not any real geometry) was the
  reason two maps read as sealed. The walk's plain step now accepts a
  one-way fall of any height (a landing deeper than the old 72-unit bound
  is still called out separately, as `RoundReport::long_drop_cells`), and
  a jump edge — ascent up to the walking player's own jump apex plus
  step-up, horizontal reach up to run speed times one jump's airtime, both
  read live from `ohl_physics::MoveConfig` rather than restated — is tried
  whenever the plain step fails. Re-run against the three maps
  `progress-probe-3.md` reported unreachable: "c4a1" (Xen) now reaches
  40,000 cells (the walk's own cap) via 67 long-drop landings instead of
  388 cells sealed by plain wall, but still does not reach its
  `trigger_changelevel` within that cap — consistent with the probe's own
  "island-hopping, longer than even this bound" read, not yet resolved.
  "c2a4" (Residue Processing) similarly grows from 416 to 16,948 cells (89
  of them long-drop landings) and its frontier is no longer wall-only;
  what remains is `func_breakable` and one `func_wall`, not a fall/jump
  bound. "c3a2" (Lambda Core) is unchanged (1,467 cells, `func_breakable`
  and `func_door` on the frontier) — its blocker was never a drop or a
  jump, only `func_breakable` having no destructible behaviour anywhere
  in the engine, which this change does not touch. `func_breakable` is
  not modeled as openable by this walk; see the "known limitation" note
  in M9.9's own entry below. New synthetic fixtures back two new
  regression tests: `ohl_engine::test_support::reachability_ledge_bsp`/
  `reachability_ledge_entities` (a ledge only reachable by a >72-unit
  drop) and `reachability_gap_bsp`/`reachability_gap_entities` (a gap
  crossable by a jump within the computed horizontal bound, and the same
  map widened past it, unreachable).

## M9.9 (Rust): reachability/route-triage dev tool

Status: accepted (Rust); evidence: PR #<n> ("Add a reachability/route-
triage dev tool"). Promotes a technique two throwaway, uncommitted
investigations (`.plan/progress-probe-1.md`,
`.plan/c1a0-progress-investigation.md`) each rebuilt from scratch — a
breadth-first walk over the live collision model to find what blocks the
player from reaching a map's `trigger_changelevel` — into a reusable,
tested engine module and a `dev-tools`-only CLI flag.

- **`ohl_engine::reachability`** (`crates/ohl-engine/src/reachability.rs`):
  a bounded, deterministic breadth-first walk over a 16-unit grid from the
  player's own hull-space origin, using
  `ohl_physics::CollisionModel::trace` with `Hull::Standing` and the
  documented 18-unit step-up (`ohl_physics::movement`'s own `sv_stepsize`),
  in all eight compass directions, with a floor-drop bound so a pit or
  ledge is treated as leaving the walkable graph rather than a new cell. A
  closed `func_door`/`func_door_rotating` blocks the walk exactly as it
  blocks the walking player, since it traces against the same attached
  brush. `compute_reachability_report` runs that walk for up to six
  rounds, reporting per round: how many cells were reached; each
  frontier-adjacent brush-entity classname's instance count and whether
  the engine's own use-proximity path (`ohl_game::find_usable_within`'s
  radius) could open it from a reached cell; and whether any
  `trigger_changelevel` volume was reached (and its straight-line distance
  from spawn, rounded to the nearest ten units). Between rounds, every
  closed door found both on the frontier and use-openable is detached
  from the collision model (`CollisionModel::detach_brush`) to simulate it
  having been opened, so a route needing several doors opened in sequence
  is triaged one round at a time.
- Two new `Game` accessors this needed and did not already have:
  `collision`/`collision_mut` (the live `CollisionModel`, for tracing
  directly rather than through the player-move step) and
  `brush_collision` (which attached brush hull belongs to which entity)
  and `player_origin` (the walking player's own hull-space origin, as
  distinct from `eye_position`).
- **`--reachability-report`** (`crates/ohl-app/src/game_run.rs`,
  `dev-tools` only): loads a map through the normal `--map`/payload path
  headlessly (no window, no GPU) and prints the report as fixed lines —
  classnames, aggregate counts, and rounded distances only, never a map
  name, coordinate, or targetname (`docs/CLEAN_ROOM.md`).
- A **new** synthetic fixture,
  `ohl_engine::test_support::reachability_door_bsp`/
  `reachability_changelevel_entities` (a closed corridor door gating a
  `trigger_changelevel` beyond it, in a bounded room so the walk's own
  cell cap is never approached), backs a **new** regression test proving
  the changelevel trigger is reachable only after the one closed,
  use-openable door on the frontier is opened.
- Run against the real payload's start map and the first maps of the next
  two campaign chapters (`ohl_campaign::CHAPTERS` order), the tool
  reproduces both throwaway investigations' own findings without rebuilding
  either one: the start map's reachable area stops at a `func_tracktrain`
  (the intro tram) roughly 7,240 units short of its `trigger_changelevel`,
  and `c1a0` reports a `func_door` neither investigation could get `use`'s
  proximity search to reach on its own frontier after the first door
  opens — the same "if a second door blocks the spot, `use`'s proximity
  search is not finding it" finding
  `.plan/c1a0-progress-investigation.md` recorded manually.

- **Five more spawn-to-exit progression scenarios, from a second
  reachability triage pass.** With the `--reachability-report` dev tool in
  hand, five more `combat-smoke` scenarios were added:
  `xtask/smoke-scenarios/progress_c1a3_reach_changelevel.txt` ("We've Got
  Hostiles!") and `progress_c2a2_reach_changelevel.txt` (On A Rail), each
  opening a door on the way with a `use` press, the latter also passing a
  monster; `progress_c2a1_reach_changelevel.txt` (Power Up), which also
  passes a monster but needs no door; and
  `progress_c1a4_reach_changelevel.txt` (Blast Pit) and
  `progress_c2a3_reach_changelevel.txt` (Apprehension), whose routes need
  neither, all four reaching their own map's level-change trigger and
  following it end to end. Progression scenarios now cover `c1a0`, `c1a1`,
  `c1a3`, `c1a4`, `c2a1`, `c2a2` and `c2a3`.
  The same pass found "c1a2" (Office Complex) **not** reachable by that
  walk: every use-openable door on the map's frontier does get opened
  round over round, but the frontier left standing afterward is a
  `func_pendulum`, a `func_pushable` and further `func_button`/
  `func_wall`/`func_breakable` entities — none of them a closed door, so
  none of them is something the reachability walk's round-advance (which
  only ever opens a closed `Door`-component brush) can act on. A targeted
  probe confirmed buttons on this map are live (pressing one reachable
  button did change two real door states elsewhere), but a fresh walk from
  the player's new position still left the same non-door frontier and the
  level-change trigger still unreached, so the proximate blocker is most
  plausibly the mover/pushable obstacle rather than a missed trigger
  chain. `c1a2` is therefore blocked on `func_pendulum`/`func_pushable`
  support (timed obstacle dodging / pushing an object to open a new step),
  not on anything this round's scenario work changes; no scenario was
  added for it.

- **Two more spawn-to-exit progression scenarios, from a third
  reachability triage pass.** `.plan/progress-probe-3.md` pointed PR
  #121's `--reachability-report` dev tool at the six campaign maps
  following `c2a3` in `ohl_campaign::CHAPTERS`'s own cited table and found
  two more reachable at round 0: `progress_c2a5_reach_changelevel.txt`
  (Surface Tension), whose route opens a door along the way with a `use`
  press, and `progress_c3a1_reach_changelevel.txt` ("Forget About
  Freeman!"), whose route needs no door. Both were authored by an
  autopilot flown over the map's own collision hulls the same way as the
  second pass's five scenarios, merged into a scripted-input file, and
  verified end to end against the real binary with
  `--script-log --follow-level-change`; both worked on the first try.
  Progression scenarios now cover `c1a0`, `c1a1`, `c1a3`, `c1a4`, `c2a1`,
  `c2a2`, `c2a3`, `c2a5` and `c3a1`.
  The same pass found three of the remaining four maps **not** reachable
  by this walk. `c2a4` (Residue Processing) and `c4a1` (Xen) are both
  sealed by static, non-entity world geometry or a fall/jump longer than
  the walk's own conservative 72-unit drop bound (most confidently for
  `c4a1`, whose `down_no_floor` frontier share is roughly ten times
  `c2a4`'s and whose `trigger_changelevel` sits an order of magnitude
  further from spawn than any other map probed so far) — a duck-only
  vent/crawlspace was ruled out for both by re-running the identical walk
  with the crouched hull instead of standing. `c3a2` (Lambda Core) is
  blocked because `func_breakable` has no shoot-to-destroy behaviour
  anywhere in this engine at all today: it is spawned as permanent solid
  geometry with no health, damage, or destruction handling in any crate, a
  finding confirmed by a direct source search rather than only inferred
  from the walk's own inability to shoot through it, so this map is
  expected to unblock once `func_breakable` support lands (tracked
  separately). The fourth, `c2a4d` (Questionable Ethics), **is** reachable
  by `--reachability-report` itself (round 4, after four rounds of
  door-opening) but no route was authored for it: every steering strategy
  this probe's own from-scratch autopilot tried left the player wedged at
  the same reproducible point roughly a fifth of the way along the route,
  a limitation of that one probe's tooling rather than a confirmed engine
  defect or a reachability finding in question. No scenario was added for
  any of these four maps.
- **Follow-up (one-way drops and jumps).** The original 72-unit drop bound
  was later found to be the reason two of `.plan/progress-probe-3.md`'s six
  probed maps read as sealed; the walk now also tries a one-way fall of any
  height and a jump edge. **Known limitation, not addressed by that
  follow-up:** although `ohl_game::registry::Breakable` now exists (M9.10,
  landed separately), this walk still does not read it — a `func_breakable`
  blocking a route still reports on the frontier exactly like a
  `func_wall`, never as openable-by-damage. Teaching the walk to treat a
  damageable `func_breakable` as an opening path the same way it already
  treats a use-openable door is left for whoever extends it next.

## M9.9 (Rust): touch-activated doors

Status: done. Closes a gap public Half-Life mapping documentation exposed:
every door-opening path in this crate before this package needed either a
proximity `use` press or a *separate* touch-trigger brush a mapper placed
around the door, but a real `func_door`/`func_door_rotating` also opens the
moment a player walks into it — with three documented exceptions, all read
from one cited sentence and its neighbouring spawnflags
(`docs/FORMAT_SOURCES.md` item 30, cited from the Sven Co-op wiki's
`Func_door` page, fetched directly): the "Use Only" spawnflag (256), the
"Passable" spawnflag (8), and carrying a `targetname` at all ("Func_doors
are triggered on touch, unless they have a name, in which's case they
require to be triggered manually").

A PR #122 review round caught that the version first merged here read only
the "Use Only" half of that sentence and explicitly declined the
`targetname` half, reasoning (incorrectly) that it was a separate,
project-invented concern. Instrumenting the real payload during review
found every door either of the two chapter-walk scenarios this milestone
had flipped to expect "The player opened a door." actually opened carried
a `targetname` — zero unnamed doors touched anywhere — so the shipped
reading had no in-spec activation to show for itself. Item 30 records the
wrong paragraph struck, in place, alongside the corrected one, rather than
silently rewritten.

Two new marker components, `ohl_game::registry::DoorUseOnly` and
`DoorPassable`, are attached at `Registry::build` time from
`SPAWNFLAG_DOOR_USE_ONLY`/`SPAWNFLAG_DOOR_PASSABLE` — plain markers rather
than `bool` fields added to `Door` itself, since `Door` is reached
transitively by the save file's *required* entity-registry section and
item 28's frozen-section rule forbids a new field there; like
`RotatingDoorSwing` before them, both markers are rebuilt from the map
every load and never touch the save/transition path. `ohl_game::logic::
Simulation::touch_doors` — mirroring `Simulation::touch_rot_buttons`'s PR
#111 touch-edge pattern — opens every closed door with neither marker and
no `TargetName` component, whose placed brush bounds, inflated by a new,
bounded `DOOR_TOUCH_MARGIN` (4 units; large enough to still register where
ordinary collision has already stopped the player flush against the
door's own solid, per `ohl_physics::hull::DIST_EPSILON` — and, per a
PR #122 review measurement now recorded on the constant's own doc
comment, reaching through a wall thinner than 4 units, which is
considered acceptable since real door frames are not built into walls
that thin), overlap the player's hull, through the same
`Simulation::activate` a `use` press already goes through — so a
`func_door_rotating` opened this way still swings away from the player
(item 26/PR #110's rule). `ohl-engine`'s `Systems::triggers_and_movers`
(phase 12) calls it with the same standing-hull box `touch_triggers`
already builds. `Game::doors_opened_by_use_count` is renamed
`doors_opened_count` (every call site updated, including the "The player
opened a door." milestone line in `crates/ohl-app/src/script_log.rs`)
since it now counts either opening path.

Tests: `crates/ohl-game/src/logic.rs` adds `touch_opens_a_plain_unnamed_door`,
`touch_does_nothing_for_a_named_door` (a direct `Simulation::activate`/
`use_entity` call still opens it, isolating the `targetname` exclusion),
`touch_does_nothing_for_a_use_only_door`, and
`touch_does_nothing_for_a_passable_door` — the latter two built unnamed so
each isolates its own flag's effect from the `targetname` exclusion.
`crates/ohl-game/src/registry.rs` adds marker-attachment tests for both
new spawnflags, each with a `func_door_rotating` counterpart.
`crates/ohl-engine/tests/rotating_door.rs`'s pre-existing
`a_closed_rotating_door_blocks_the_corridor` is renamed
`a_use_only_rotating_door_blocks_the_corridor_forever` and rebuilt on a
`rotating_door_use_only_entities` fixture (named, "Use Only" set, so it
proves that flag's own exclusion specifically). A **new**
`rotating_door_unnamed_entities` fixture (the same corridor and door,
`targetname` omitted) backs `a_closed_door_opens_when_the_player_walks_
into_it`, the only test in that file whose door has no name to look it up
by, so it queries the registry for its one `Door` component directly
instead of through `Registry::find`.

`cargo xtask combat-smoke --payload-root <dir>` (26/26) required
*reverting* the two pre-existing chapter-walk scenarios this milestone had
flipped — "walk from spawn in Anomalous Materials" and "walk from spawn
in Surface Tension" — back to their original `WALK_PRESENT`/`BASE_ABSENT`
sets, since the doors either walk touches are all named and this
correction excludes them; confirmed directly with `--script-log` against
the real payload, neither logging "The player opened a door." any more.
The pre-existing `use`-press scenario is unaffected and unchanged. `cargo
xtask campaign-smoke --payload-root <dir>` stays 93/93 throughout (it only
loads maps, never scripts movement). This leaves the feature with no
demonstrated, in-spec touch-open anywhere in the current real payload's
chapter-walk routes — real doors along those routes are all named — a gap
recorded rather than hidden.

**`TODO(black-box)`**: whether a monster (as opposed to the player) opens a
touch-eligible door by walking into it is not implemented — `touch_doors`
is called only with the player's own hull box, the same scope
`touch_triggers`/`touch_rot_buttons` already have. Whether a real touch
check re-triggers every tick a mover keeps overlapping an already-open door
is not stated by either cited source; this project's edge-triggered choice
(open once per closed-to-open approach) matches every other touch path
already in this crate. **Added during PR #122 review**: a door with a
nonzero `wait` that auto-closes while the player never leaves its touch
volume does not re-open, since the touch-edge state stays high across the
whole open/close cycle; not a crash or a stuck-in-solid regression, just an
unresolved case, now easier to reach than before this milestone, recorded
in item 30 rather than guessed at.

## M9.10 (Rust): `func_breakable` and `func_pushable`

- **Both classnames are implemented, and neither existed before.** A
  `func_breakable` (and a `func_pushable` with its documented "Breakable"
  flag) now takes damage through the shot path, breaks when its documented
  `health` ("Strength") is spent, fires its documented "Target on Break"
  after the documented `delay`, and is removed from both collision and
  render; one with the documented "Only Trigger" flag (or no `health` at
  all) ignores damage and breaks when triggered instead. The documented
  "Touch", "Pressure" and "Instant crowbar" flags are wired too, the last
  two with approximations recorded as project behaviour (`docs/
  FORMAT_SOURCES.md` item 32). A `func_pushable` additionally moves when
  the player walks into it: `ohl_engine::pushables` translates it along the
  player's own movement wish at a `friction`-scaled speed, traced through
  the collision model with the crate's own hull ignored
  (`ohl_physics::CollisionModel::trace_ignoring`, new) so it stops on
  contact with a wall and never overlaps the player. All of it — remaining
  hit points, the broken flag and the push offset — round-trips through a
  **new** optional save section, `SECTION_BREAKABLE_STATE` (tag 33; 32 stays
  reserved for `ohl-player`), with a discriminating round-trip test, a
  pre-tag-33 compatibility regression, and a new golden-bytes test that
  touches no existing golden.
- **What this unblocks, and what it does not.** `.plan/progress-probe-2.md`
  found Office Complex ("c1a2", `ohl_campaign::CHAPTERS`'s own cited table)
  unreachable with a frontier made of `func_pushable`/`func_breakable`/
  `func_wall`/`func_button`/`func_pendulum` brushes; two of those five
  classnames now do something. Whether that map's route actually opens up
  is not claimed here: the reachability dev tool that measured it is still
  an unmerged pull request, so nothing in this tree could be re-measured
  against it, and teaching that walk to break or push a brush is left for
  whoever lands it.
- **Gaps, all recorded rather than guessed at** (`docs/FORMAT_SOURCES.md`
  item 32): no gibs, no per-`material` break sounds, no `spawnobject`
  spawning, no falling/pulling/buoyancy for a pushable, and no transition
  carry (save/load only) — the last for exactly the frozen-section reason
  item 28 records. No `combat-smoke` scenario was added: a bounded,
  aggregate-only probe found both classnames near spawn points on maps the
  suite already visits, but a fresh map spawns the player unarmed, so
  breaking one in a scenario would need a multi-leg authored route to a
  weapon and back — out of this package's budget. Synthetic integration
  fixtures (`crates/ohl-engine/tests/breakable.rs`, `pushable.rs`) stand in
  for one, both driving the real `Game` loop with no forced state.


## M9.11 (Rust): `func_monsterclip` is no longer solid to the player

Status: accepted (Rust); evidence: PR #129 ("Fix `func_monsterclip`
blocking the player"). `.plan/progress-probe-4.md`'s `c4a2` finding
(`docs/FORMAT_SOURCES.md` item 33): `func_monsterclip` was spawned with no
special handling at all, falling through to `ohl_game::brush`'s
"everything not documented otherwise is solid" default and blocking the
player exactly like a `func_wall` — the dominant blocker (~62% of blocked
frontier attempts) on `c4a2` (Gonarch's Lair).

- `func_monsterclip` joins `ohl_game::brush::NEVER_SOLID` and
  `is_never_rendered` (`crates/ohl-game/src/brush.rs`): excluded from
  `solid_model_instances` (so it is never attached to the player's own
  collision model) and `model_instances` (so it is never drawn, matching
  its documented invisibility). It contributes no `ContentsVolumeKind`,
  unlike `func_ladder`/`func_water`: an inert brush has no player-relevant
  contents once it stops being solid to the player.
- **A second collision model for monster navigation, not a silently
  accepted global gap**: an initial version of this fix excluded
  `func_monsterclip` from the one `CollisionModel` `ohl-ai`/`ohl-nav`
  shared with the player, which `combat-smoke`'s own full-suite re-run
  caught as a real regression on "Power Up" (`c2a1`, 29
  `func_monsterclip` entities) — a monster a fence used to keep away from
  the player's scripted route could now reach and, in one run, kill them.
  `Level::monster_collision` (`crates/ohl-engine/src/level.rs`) is now
  built and kept in step (`Level::sync_monster_brush_collision`) alongside
  the player's `Level::collision`, attached with a new
  `ohl_game::brush::monster_solid_model_instances`/`is_solid_to_monster`
  that treats `func_monsterclip` as solid; `ai.rs` reads this model for
  every AI-side trace (navigation-graph build, sensing, movement/steering,
  and the monster attack hit-trace) instead of the player's. See
  `docs/FORMAT_SOURCES.md` item 33 for the full citation, the still-open
  per-monster-spawnflag/`CLIPHULL#` gap this coarser fix does not close,
  and the diagnosis that isolated the regression to the player's own
  changed walkable space (not a bug in the new model).
- **The `c2a1` route was re-authored, not the scenario's assertions
  weakened.** A first attempt dropped "A level change was followed."
  from `progress_c2a1_reach_changelevel.txt`'s own present set (and "The
  player took damage." from its absent set) instead of reaching the exit
  at all; a PR #129 review round rejected that as turning a spawn-to-exit
  progression check into a much weaker "survives, monster dies somewhere"
  one. `xtask/src/combat_smoke.rs`'s `LEVEL_CHANGE_PRESENT_MONSTER_
  ENCOUNTER`/`_ABSENT` (this scenario's original present/absent sets) are
  unchanged; the file's *steps* are new, authored fresh against the
  corrected engine with `.plan/progress-probe-2.md`'s own two-`Game`
  planner/autopilot technique, verified end to end against the real
  binary in 2 iterations (well within that technique's 10-iteration
  budget) — see the scenario file's own header and
  `docs/FORMAT_SOURCES.md` item 33 for the full account.
- **New tests**: three registry-level unit tests in `crates/ohl-game/src/
  brush.rs` (`func_monsterclip_is_never_a_solid_brush`,
  `excludes_func_monsterclip_from_rendering`,
  `func_monsterclip_is_solid_to_monsters`); a new `crates/ohl-engine/src/
  level.rs` unit test, `func_monsterclip_is_attached_only_to_the_monster_
  collision_model`; a new integration test, `crates/ohl-engine/tests/
  monsterclip_corridor.rs`, proving a corridor blocked only by a
  `func_monsterclip` is walkable end to end (with a `func_wall` control
  case proving the same fixture's geometry really is solid when it should
  be), built on a new `ohl_engine::test_support::corridor_brush_entities`
  fixture helper that reuses `rotating_door_bsp`'s own corridor geometry
  with a caller-chosen classname standing in for the door; a new
  `crates/ohl-engine/tests/monster_collision_sync.rs`, proving the two
  collision models are actually kept in step (an opened door's brush pose
  and a killtargeted brush's detachment both agree between them — this
  fails outright if `Level::sync_monster_brush_collision` is ever made a
  no-op); and a new `crates/ohl-engine/tests/monsterclip_blocks_monster.rs`,
  covering the two AI-side `monster_collision` switches
  `monsterclip_corridor.rs` cannot (nav-graph build, attack hit-trace) with
  a fenced-off `monster_human_grunt` fixture — see that file's own doc
  comment for which of `ai.rs`'s three switches this fixture can and
  cannot independently discriminate a revert of.
- **Evidence against a locally imported retail payload** (identified only
  by its sanitized digest; no path or map name left the local boundary,
  per `docs/CLEAN_ROOM.md`): `--reachability-report` on `c4a2` before this
  fix reproduces `.plan/progress-probe-4.md`'s own numbers exactly (round
  0: 12,678 cells reachable; frontier `func_breakable` x3,
  `func_monsterclip` x13, `func_wall` x1); after it, on the same map, cells
  reachable nearly double (23,520) and `func_monsterclip` is gone from the
  frontier entirely (frontier `func_breakable` x3, `func_wall` x3 remain —
  a separate, already-tracked gap, not a regression) — unchanged by the
  dual-collision-model revision, since `c4a2`'s frontier only ever
  reflects the player's own model. A bounded aggregate probe
  (`ohl-formats`' BSP entities parser plus `ohl_assets::AssetFs`,
  resolving exactly `ohl_campaign::CHAPTERS`'/`HAZARD_COURSE_MAPS`'s own
  cited 93 map names — not the payload's whole `maps/` directory — the
  same way the real gameplay path does, including packed `.pak` archive
  entries) found 147 `func_monsterclip` entities across 92 of those 93
  maps' entities lumps (the 93rd, `c1a3d`, fails this project's own
  bounded parser with `InvalidText`, a pre-existing gap `Level::load`
  already silently accepts and this fix does not touch). `cargo xtask
  combat-smoke` (35/35 scenarios, including "walk from spawn to a
  followed level change in Power Up" on its re-authored route) and `cargo
  xtask campaign-smoke` (93/93 maps, 0 missing-map/load-error/timeout/
  crash/blank-capture) both pass against the same payload after this fix.
## M9.12 (Rust): `--start-inventory`, breakables/pushables/long-jump in `--reachability-report`

Status: in progress (Rust); evidence: this PR.

- **Motivation.** `.plan/progress-probe-5.md` re-ran `--reachability-report`
  on "c1a2" (Office Complex) and "c3a2" (Lambda Core) after M9.10 landed
  `func_breakable`/`func_pushable` gameplay, and found two gaps neither
  earlier probe had isolated: the walk itself still never modeled breaking
  or pushing a brush as a round-advance edge (confirmed by source, not
  just by the walk's own output — nothing in `reachability.rs` read either
  component), and both maps' only weapon pickup sits thousands of units
  past their own blocked frontier, an order of magnitude past anything
  either map's own walk reaches — consistent with both being mid-campaign
  maps a real campaign run would already reach armed, which this
  investigation's single-map-load methodology cannot exercise (a cold
  `Game::load` always starts from `ohl_combat::Inventory::new`'s empty
  inventory, not even the crowbar). This package closes both gaps.
- **`ohl_engine::reachability` now models breaking and pushing.**
  `ReachabilityConfig::assume_armed` (default `false`) gates a new
  round-advance edge: a `func_breakable` on the frontier with `health >
  0`, not the documented "Only Trigger" flag, and not already broken is
  reported `FrontierClass::damage_openable` and has its brush detached for
  the next round — the same coarse "remove the brush" treatment a door
  already gets — but only when this flag is set; the walk never checks or
  fabricates an actual inventory, so a run without it leaves a breakable
  on the frontier forever, the same as before this package. A
  `func_pushable` is reported `FrontierClass::push_openable` and detached
  the same way, unconditionally: pushing needs no assumed weapon. Both are
  counted separately in `RoundReport` (`breakables_opened`,
  `pushables_opened`), alongside the existing `doors_opened`. Two new
  regression tests
  (`a_breakable_is_openable_only_when_armed_is_assumed`,
  `a_pushable_is_openable_by_push_without_any_assumed_weapon`) prove the
  round-advance shape against new synthetic fixtures
  (`ohl_engine::test_support::reachability_breakable_bsp`/
  `reachability_pushable_bsp`): a corridor blocked by one obstacle,
  hiding a `trigger_changelevel` beyond it, reached only after the
  obstacle's brush is detached.
- **`--reachability-assume-armed`** (`crates/ohl-app/src/main.rs`,
  `dev-tools` only, `requires = "reachability_report"`): threads
  `ReachabilityConfig::assume_armed` from the command line. The printed
  report now tags each frontier line with "damage-openable (armed
  assumed)"/"push-openable" where applicable, and a round with either kind
  of edge logs "Breaking N breakable(s)..."/"Pushing N pushable(s)..."
  alongside the existing door line.
- **`--start-inventory LIST`** (`crates/ohl-app/src/main.rs`,
  `crates/ohl-engine/src/start_inventory.rs`, both `dev-tools` only): gives
  the player named weapons and ammo right after the map loads, so a
  single-map probe or a `combat-smoke` scenario can model the inventory a
  real campaign run would have carried in from an earlier map, without
  walking the whole campaign chain first. `LIST` is a comma-separated list
  of `weapon_*`/`ammo_*` classnames — the exact vocabulary
  `ohl_combat::classify_classname` already recognises (no new classname
  literal), applied through the same grant path a touch pickup already
  uses (`Inventory::give_weapon`/`give_ammo`,
  `weapon_pickup_ammo`/`ammo_pickup_amount`; `Game::give_start_inventory`).
  An unrecognised classname, or a recognised one that is not a weapon or
  ammo (`item_suit`, `func_healthcharger`, ...), is a clear parse error.
  This never touches the save format: inventory is save tag 23, and
  seeding it at load is ordinary runtime state through the ordinary
  inventory API, not a new grant path or a format change. Unit tests cover
  parsing (`ohl_engine::start_inventory::tests`) and the `xtask
  combat-smoke` `Scenario::start_inventory` field's command-line wiring
  (`xtask::combat_smoke::tests::build_command_*`); no default scenario
  uses it yet, so the existing suite's plain (non-`dev-tools`) release
  binary build is unaffected.
- **`--reachability-assume-longjump`** (`crates/ohl-app/src/main.rs`,
  `dev-tools` only, `requires = "reachability_report"`), added mid-package
  once `.plan/progress-probe-6.md` (a follow-up read-only investigation)
  found "c4a1" (Xen) and "c4a3" (Nihilanth) closing their entire reachable
  area under the walk's existing single running-jump model without ever
  reaching their own `trigger_changelevel`, with no entity of any kind
  left on the frontier to blame — consistent with a jump/mobility
  technique the walk's model could not yet express, and this project's
  own long-jump module (`item_longjump`,
  `ohl_physics::movement`'s `long_jump_ready`/the `long_jump_forward_speed`/
  `long_jump_up_speed` impulse) already existed as exactly that
  technique, unmodeled by the walk. `ohl_engine::reachability` gained a
  third edge attempt, tried only when both the plain step and the
  ordinary jump edge fail: a long-jump edge bounded by
  `JumpBounds::from_long_jump_config`, deriving both its airtime and its
  horizontal reach from `MoveConfig::long_jump_forward_speed`/
  `long_jump_up_speed`/`gravity` alone (unlike the ordinary jump edge,
  the long jump's impulse sets *both* velocity components directly, so
  neither bound involves `max_speed`) — read live from the same `Game`
  the walk runs against, never a restated literal. A cell reached only
  this way is counted in the new `RoundReport::long_jump_cells`. Gated by
  `ReachabilityConfig::assume_longjump` (default `false`) the same way
  `assume_armed` gates the breakable edge: a cold map load owns no items
  at all, so this is a caller-supplied assumption, never an inventory
  check. A new regression test
  (`a_gap_beyond_the_ordinary_jump_is_reached_only_with_assume_longjump`)
  proves this against the existing `reachability_gap_bsp`/
  `reachability_gap_entities` fixture family (already parametrized by gap
  width), widened past the ordinary jump's own reach but within the long
  jump's.
- **`c1a2`/`c3a2` re-measured with `--reachability-assume-armed`; `c4a1`/
  `c4a3` re-measured with `--reachability-assume-longjump`.** All four
  maps' runs are recorded as aggregates only in this package's own pull
  request description, per `docs/CLEAN_ROOM.md`'s reviewed-sanitized-report
  rule; see that PR for the per-map round-by-round result.
- **Scope.** Neither flag changes what a *script* or a *save* can express:
  `--start-inventory` is a dev-tools load-time convenience over the same
  API a pickup touch already calls, not a new inventory mechanism, and the
  reachability walk's push/break edges remain the same approximate
  "detach the brush" treatment a door already gets — not a simulation of
  the real push distance/direction or the real damage-vs-health math. No
  `combat-smoke` scenario was changed to use `--start-inventory`: closing
  the "route-authoring" gap `.plan/progress-probe-5.md`'s ranked list left
  open (item 1) is this package's job; using it to actually author a
  working "c1a2"/"c3a2" route stays a follow-up, per that same list.

## M9.13 (Rust): the entities lump stops failing silently

Closes the gap M9.11's own entry above records as pre-existing and
deliberately untouched: the 93rd map whose entities lump fails this
project's bounded parser with `InvalidText`, which `Level::load` silently
accepted.

- **One campaign map used to load as an empty room, and everything
  reported it as a success.** Its entities lump failed
  `ohl_formats::bsp30::entities::parse` with `FormatError::InvalidText`,
  and `Level::load` ran that through `unwrap_or_default()`: the map then
  built with no entity definitions at all — no player start (so the player
  spawned at the world origin, normally inside solid geometry), no
  monsters, no triggers, no brush entities — while `cargo xtask
  campaign-smoke` still counted it among 93/93 loaded, because it exited
  zero and captured a perfectly healthy-looking frame of that empty room's
  walls.
- **The failure class was encoding, not structure.** A bounded,
  aggregate-only local probe over all 93 cited maps (uncommitted, nothing
  media-derived recorded) found 92 lumps parsing and 1 failing, with **zero
  structural problems anywhere**: the failing lump has balanced blocks, a
  correct terminator, no interior NUL, no control characters and no
  over-long string, and carries **exactly one byte in `0x80..=0xFF` inside
  one quoted value** — the punctuation a legacy single-byte Windows
  codepage writes. The published grammar (Unofficial Quake Specs section 4,
  lump 0) specifies no encoding for a quoted run beyond the ASCII its own
  delimiters use, so that lump is legal and the parser was too strict.
- **The fix, in three parts** (`docs/FORMAT_SOURCES.md` item 34):
  - `ohl-formats` decodes a quoted run as UTF-8 when it is valid UTF-8 and
    otherwise byte-per-byte into the Latin-1 range — total, lossless,
    deterministic, and ASCII-preserving, so no classname, keyname or
    targetname the game logic matches on can change. Structural violations
    are still hard rejections. The new `parse_with_report` /
    `Bsp::entities_with_report` publish an aggregate
    `EntityLumpReport::relaxed_strings` count (never the string, never the
    byte).
  - `Level::from_bytes_with_ramp` no longer swallows the error: an
    unreadable lump is now `EngineError::EntityLumpUnreadable`, and
    `Level::entity_lump_relaxed_strings` republishes the aggregate count.
  - `ohl-app` prints one fixed, content-free line after a successful load
    (`ENTITY_WORLD_OK_LINE`) when the map really has entities *and*
    something to arrive at — a resolved `info_player_start`, or an
    `info_landmark` for a map entered only through a level transition (one
    cited map, `c1a4e`, is exactly that case: 150 entities, two landmarks,
    no player start, and legitimately so) — and a fixed warning
    (`ENTITY_WORLD_EMPTY_LINE`) when it does not. `campaign-smoke` requires
    the former before scoring a map `loaded` and otherwise files it under
    the new, non-passing `empty-entity-world` bucket. An empty-room load
    can never score as a pass again.
- **Before/after, on the map itself.** Run with `--script`/`--script-log`
  against the real payload, the affected map used to log the fixed line
  "The player is inside solid geometry." — the player standing in an empty
  room's geometry with no spawn point to be placed at. With the fix it logs
  the entity-world line and the relaxed-decode note instead, and the
  in-solid guard line is gone. Everything the run prints is a fixed string;
  no map-derived text is logged in either direction.
- **Tests.** Synthetic unit tests in `ohl-formats` reproduce the failure
  class directly (a non-UTF-8 byte in a value, and in a key; a relaxed
  string not stopping later entities; valid UTF-8 never counted as
  relaxed), alongside kept-strict cases (unterminated quote, unclosed
  block, stray `}`, over-long value). A new proptest asserts that a
  well-formed lump parses whatever bytes its values hold and that the
  relaxed count matches exactly. `ohl-engine` tests cover the load path
  end to end: a map whose lump holds a non-UTF-8 byte loads its whole
  entity world and keeps its player start, and a structurally broken lump
  is a load error rather than an empty room, and two more pin
  `Level::has_landmark` on a landmark-only map and on a map with neither.
  `campaign-smoke`'s own tests cover the new bucket and the fixed-line
  check.

## M9.14 (Rust): chapter interior map table (`CHAPTER_MAPS`)

Status: accepted (Rust). `crates/ohl-campaign/src/chapters.rs` gets a new
`CHAPTER_MAPS` table, keyed by chapter title, alongside the existing
`CHAPTERS` table. Two chapters ("Black Mesa Inbound", "Anomalous
Materials") only carried their starting map in `CHAPTERS` (`c0a0`, `c1a0`
respectively) even though the real chapter has several interior maps
reached by level changes off it; PR #134's chained-route walk had no cited
literal for those interior destinations and had to name its route files
ordinally (`<start>-hop1.txt`, ...) instead of by destination map, per
`docs/CLEAN_ROOM.md` rule 7. `CHAPTER_MAPS` closes that gap.

- New per-chapter interior map lists, each cited (see
  `docs/FORMAT_SOURCES.md`'s new "Campaign chapter interior map table"
  section and the doc comment directly above `CHAPTER_MAPS` in
  `chapters.rs`): `c0a0a`..`c0a0e` added for "Black Mesa Inbound"; `c1a0a`,
  `c1a0b`, `c1a0d`, `c1a0e` added for "Anomalous Materials"; every other
  chapter reuses `CHAPTERS`'s own list verbatim, including "Xen"'s
  existing `c4a1a`..`c4a1f`. Two independent, directly-fetched sources
  (SourceRuns Wiki and a second, targeted fetch of the already-cited
  `combineoverwiki-storyline`) agree that "Interloper" begins at `c4a1a`
  rather than `c4a2b`, but that split is *not* adopted here: `CHAPTERS`'s
  own "Xen" row already claims `c4a1a`..`c4a1f`, so giving the same names
  to "Interloper" too would be a conflicting assignment within this table.
  "Interloper" stays empty here, mirroring `CHAPTERS`'s own pre-existing
  "to verify" status; reconciling which chapter those six maps actually
  belong under is left to a follow-up that would also revisit `CHAPTERS`.
- New `ohl_campaign::chapter_maps(title) -> Option<&[&str]>` lookup and
  `ohl_campaign::is_cited_map_name(name) -> bool`, the latter checking
  `STARTMAP`, `TRAINMAP`, `HAZARD_COURSE_MAPS`, `CHAPTERS`, and
  `CHAPTER_MAPS` together — a single place for route/fixture code to
  confirm a map-name literal is traceable to this crate's own citations
  before writing it down elsewhere.
- New tests in `crates/ohl-campaign/src/chapters.rs`: `CHAPTER_MAPS` is
  non-empty per chapter and covers every `CHAPTERS` chapter exactly once,
  every name is lowercase ASCII, no chapter's list has a duplicate name,
  every `CHAPTERS` chapter's first map appears in its `CHAPTER_MAPS` entry,
  the title lookup, and `is_cited_map_name` covering every table.
- ~~**Aggregate-only payload verification** (`docs/CLEAN_ROOM.md` rule 7 —
  the payload was read only to count against the already-cited list, never
  to choose a literal): of the local payload checked, 0 of the 102 total
  cited map names were present, and all 24 of the payload's own `.bsp`
  maps were uncovered by any cited name (that payload holds Team Fortress
  Classic and other non-campaign maps, not the retail single-player
  campaign).~~ **Correction (PR #135 review):** the struck bullet above
  only walked loose `.bsp` files and missed that the campaign maps ship
  inside `valve/pak0.pak`, which `ohl_assets::AssetFs` (the same asset
  store `cargo xtask campaign-smoke` already uses to load all 93
  chapter/hazard-course maps) indexes alongside loose files. Redone
  through `AssetFs` against the same local payload, resolving
  `maps/<name>.bsp` for every cited name: **102 of the 102 total cited map
  names open through the asset store; 0 are absent** (sanity-checked with
  a bogus name correctly reporting absent, and `STARTMAP` independently
  confirmed present, through the same code path).

Closing checks: `cargo fmt --all`; `cargo clippy --workspace --all-targets
--all-features -- -D warnings`; `cargo clippy $NO_STD_CRATES --all-targets
--no-default-features -- -D warnings`; `cargo test --workspace`; `cargo
xtask policy`; `cargo xtask graph` — all clean.

## M9.15 (Rust): the chained campaign walk

Status: implemented (Rust); evidence: this PR ("Walk the campaign as a
chain: `--chain-script` and `cargo xtask chain-walk`").

Every scenario `cargo xtask combat-smoke` runs starts at its own map's
`info_player_start` with an empty inventory. A real campaign never does
that: the player arrives through a `trigger_changelevel`, is placed at the
same offset from the destination map's `info_landmark` they had from the
source map's, and carries health, armor, weapons, ammo and the suit with
them. Per-map routes therefore do not compose, and
`.plan/progress-probe-5.md`/`.plan/progress-probe-7.md` classified several
maps "blocked" partly for that reason: a cold `--map <name>` load has no
campaign state at all, so on those maps the only weapon sits thousands of
units past the frontier that needs it. This milestone adds the missing
methodology — a walk that keeps one process, one `Game` and one inventory
across level changes — rather than another per-map cold load.

- **`--chain-script <PATH>`, repeatable, in chain order**
  (`crates/ohl-app/src/main.rs`, `crates/ohl-app/src/game_run.rs`'s
  `run_chained`). Route 0 runs from the start map's own player start;
  every later route runs from wherever the preceding route's followed
  level change put the player down, through the same
  `ohl_engine::Game::change_level`/`ohl_engine::transition` machinery
  `--follow-level-change` already used, so the carry is the engine's own,
  not a harness fiction. A route ends at the first level change it
  reaches; a route whose ticks run out first ends the chain. Level
  changes are always followed during a chain (a chain that did not follow
  them would just be a `--script` run), so `--follow-level-change` is
  neither needed nor consulted, and `--chain-script` and `--script` are
  mutually exclusive.
- **Three new fixed report lines**, alongside the per-hop "A level change
  was followed." line that already existed: "The chain walk stopped."
  when a route ran out of ticks without reaching a level change, "The
  chain walk has no further route." when every route given did reach one,
  and "The chain walk re-entered a map it had already visited." when a
  route's level change landed back in a map the chain had already been
  in. Two bounded aggregates follow them ("Chain walk depth: N.", "Chain
  walk simulated seconds: X.X."). No map name, entity name or position is
  logged, here or anywhere else in the walk.
- **Depth counts distinct maps, and a re-entry ends the walk as a
  failure.** The first version of this milestone counted map *entries*,
  which a PR review caught reporting "depth 3" for a chain that actually
  went start map -> destination -> start map: the second route walked
  straight back into the boundary it had just arrived through. Counting
  entries lets any `--min-depth` be satisfied indefinitely by
  ping-ponging across a single boundary, which is the one thing a
  progress metric for unblocking the campaign must not do. `run_chained`
  now keeps the set of maps it has entered (in memory, never logged —
  which is why the re-entry line names no map) and stops on a repeat.
- **`crates/ohl-app/src/game_run.rs`'s scripted tick loop is now shared**
  (`run_script_ticks`, `TickOptions`, `TickOutcome`) between `run_scripted`
  and `run_chained`, with `stop_on_level_change` the only behavioural
  difference: a plain `--script` run keeps ticking its remaining ticks on
  the destination map exactly as it did before this milestone.
- **`cargo xtask chain-walk --payload-root <dir>`**
  (`xtask/src/chain_walk.rs`) assembles the chain from a new
  `xtask/chain-routes/` directory, runs it once through the built binary,
  and prints an aggregate-only summary: routes assembled, maps reached
  (chain depth), level changes followed, elapsed game seconds, and which
  fixed terminal line ended the walk. It exits non-zero below
  `--min-depth` (default 2). This is **not** a required CI job: it is an
  xtask subcommand run in a PR's closing checks alongside
  `cargo xtask combat-smoke`, since like that command it needs a locally
  imported payload no CI runner has.
- **Route files are named by position in the chain**, not by destination
  map: `<start>.txt`, then `<start>-hop1.txt`, `-hop2.txt`, ... A missing
  hop ends the chain rather than being skipped, since hop `n`'s route only
  means anything if hop `n-1`'s route delivered the player there. Only
  `<start>` is a real name, and it must be one from `ohl_campaign`'s own
  cited table (`is_campaign_table_name` rejects anything else). Which map
  a `trigger_changelevel` actually lands in is a fact about the user's own
  payload, and `docs/CLEAN_ROOM.md` rule 7 admits only lawfully public
  name literals; naming the file "the route from where the first level
  change out of `c0a0` lands" says what it is for without writing that
  name down.
- **Arrival-point route triage**: `--reachability-report` combined with
  `--chain-script` runs the reachability walk *after* the chain rather
  than instead of it, from wherever the last route left the player
  standing.
  `compute_reachability_report` already walks out from the player's
  current origin, so this needed no new walk — only the ordering. It is
  the one way to triage a route from a level-change arrival point, which
  a cold `--map <name>` load cannot reproduce, and it is what authored the
  route below.
- **`--chain-script` rejects the capture-pose flags** rather than
  silently ignoring them: `--headless-screenshot`, `--viewpoint` and
  `--spawn-offset` are `conflicts_with`. A chain writes no PNG, and a
  frozen or rider pose is defined against one map's geometry while a
  chain deliberately leaves that map partway through.
- **The first two routes are authored and working**
  (`xtask/chain-routes/c0a0.txt`, `xtask/chain-routes/c0a0-hop1.txt`).
  The first is the start map's opening mover ride, which carries the
  player through a level boundary without a movement key pressed; the
  second walks away from that boundary at the arrival point. Against a
  locally imported retail payload (identified only by its sanitized
  digest), the chain reached **distinct depth 2** at this milestone — one
  level change followed, 77.3 simulated seconds, ending on "The chain walk
  stopped." (M9.17 fixed the blocker below and re-authored both later
  routes; the numbers there supersede these.)
- **What blocked the chain at depth 2, from the arrival-point report**:
  8,428 cells are reachable from the first arrival point; the only entity
  on the whole frontier is a single `func_tracktrain`; and the only
  reachable `trigger_changelevel` is the one ~290 units away that the
  player arrived through. Every heading within about 45 degrees of the
  arrival facing walks back into that boundary (which is now a reported
  failure, not depth); headings at 90, 135 and 180 degrees reach no level
  change within 30 simulated seconds; and standing still for over three
  simulated minutes reaches nothing either, on any approach to the
  tracktrain. **The way onward is a tracktrain that never departs in this
  engine** — a concrete, newly isolated campaign blocker, and the first
  one this instrument found rather than inferred.
- **Tests**: `xtask/src/chain_walk.rs`'s own unit tests cover chain
  assembly (consecutive hops, a gap ending the chain, a missing start
  route, a start map outside the cited table, and the shipped chain being
  assemblable and non-empty), report parsing from the app's fixed lines,
  and the summary staying aggregate-only. `crates/ohl-app/tests/
  chain_script.rs` drives the real binary end to end over `ohl-engine`'s
  own synthetic two-map touch-`trigger_changelevel` fixture: a two-route
  chain reports depth 2 and stops, a chain whose first route reaches
  nothing stops at depth 1, and a chain that runs every route reports "no
  further route" instead.
- **The handover itself is tested, not just the hop.** A PR review found
  that flipping `stop_on_level_change` to `false` — deleting the whole
  point of a chain, letting route *n* keep ticking on the destination map
  instead of handing over to route *n+1* — left every test passing.
  `a_route_hands_over_to_the_next_one_at_the_level_change_it_reaches` now
  discriminates it with a two-sided bound on the app's own
  simulated-seconds aggregate: route 0 budgets 900 forward ticks but
  reaches the fixture's trigger in under 300, and route 1 then spends
  exactly 600 waiting, so a total in [10 s, 15 s) is reachable only if
  route 0 stopped early *and* route 1 ran afterwards. The mutant lands at
  exactly 25 s. A second new test,
  `a_route_that_walks_back_through_its_arrival_boundary_fails_as_a_re_entry`,
  stages a destination map whose own touch trigger points back at the
  source and asserts the re-entry line, the distinct depth of 2, and that
  neither other terminal line fires; it fails against the same mutant.


## M9.16 (Rust): a native isolated-worker backend for macOS

Status: accepted (Rust); evidence: PR #<n> ("Add a native macOS
isolated-worker backend so the port runs on Apple Silicon").

**The gap this closes.** `ohl-platform`'s `IsolatedWorker` had exactly one
native backend, Linux x86-64. Every other target compiled the uninhabited
`unsupported` backend, so `launch_isolated_worker` failed with
`IsolatedWorkerError::Unsupported` before doing anything, and with it the
whole import pipeline: on macOS the app built and ran, rendered an
already-imported payload, and could never produce one. The engine, renderer,
audio and packaging were already cross-platform; containment was the single
target-gated hole.

**Why the Linux design could not simply be ported.** The Linux backend
executes a freestanding, statically linked `ET_EXEC` image by descriptor
(`execveat`), confined by Landlock and a seccomp allowlist and observed
through a pidfd. macOS has none of those four primitives, and a macOS
process cannot avoid linking the system's libSystem, so a `-nostdlib`
static image is not a thing that can exist there. The backend is therefore a
different mechanism reaching the same contract, not a port:

- **Confinement**: the Seatbelt system sandbox, applied by the root-owned
  `/usr/bin/sandbox-exec` from a profile this backend renders
  (`crates/ohl-platform/src/isolated_worker/macos_profile.sb`) — `(deny
  default)`, execute-and-read on the one verified image, read-only access
  to the system libraries, and explicit denials for network, writes, `fork`
  and Mach lookups. `sandbox-exec` applies the profile to itself and then
  `exec`s the image in place, so the process the parent spawned *is* the
  worker and its exit status is the worker's.
- **Image identity**: the same compile-fixed install-location walk and
  metadata policy as Linux (now shared in
  `isolated_worker/unix_image.rs`, which both backends call with their own
  format check), specialised to a thin 64-bit `MH_EXECUTE` Mach-O for the
  running CPU type that names `/usr/lib/dyld` as its only dynamic linker,
  `/usr/lib/libSystem.B.dylib` as its only dynamic library, and no
  `LC_RPATH`.
- **Lifecycle**: `kqueue` with an `EVFILT_PROC`/`NOTE_EXIT` registration in
  place of the pidfd poll, and `kill(SIGKILL)` in place of
  `pidfd_send_signal`.
- **Bootstrap**: a `pre_exec` closure that moves the channel and readiness
  pipe onto descriptors 3 and 4, applies six `setrlimit` limits, and
  sweeps every descriptor from 5 to the parent's own `RLIMIT_NOFILE` — the
  macOS counterpart of Linux's `dup3`/`prlimit64`/`close_range` sequence,
  and equally async-signal-safe (only `dup2`, `setrlimit`, `close` and one
  `write`, on integers captured before the fork).

**The image gained a second shape.** Both standalone image packages
(`crates/ohl-test-worker/image`, `crates/ohl-parser-worker/image`) now
select their source by target: `freestanding.rs` (the unchanged `#![no_std]
#![no_main]` Linux x86-64 image) or `hosted.rs` (an ordinary `std` binary
for macOS). One package, one `Cargo.lock`, conditional crate attributes, and
a `build.rs` that emits the `-nostdlib -static -no-pie` link arguments only
for the Linux x86-64 target. The hosted media-parser image hosts the same
`run_parser_worker_service` lifetime over the same descriptors with the same
`contract.rs` exit statuses, so nothing above the transport can tell the two
apart.

Two hosted-only deviations are deliberate and documented in the image's own
module docs. Darwin rejects `setrlimit` for `RLIMIT_AS` and `RLIMIT_DATA`
outright (`EINVAL`), so both are absent from the macOS limit table and there
is no kernel-enforced memory ceiling on the platform at all; the image
imposes its own with a counting global allocator that refuses past 128 MiB (the freestanding image's 96 MiB arena plus its two
1 MiB payload buffers, rounded up), which fails closed exactly as arena
exhaustion does. And `std` has no stable `MSG_PEEK`, so `probe_input` reads
one byte into a private pushback slot that the next `read_exact` drains
first — non-consuming as far as the service can observe, which is what the
transport contract actually requires.

**Three properties are weaker than Linux and are recorded, not claimed
away**: the image is executed by path rather than by descriptor (macOS has
no `fexecve`), so the verification-to-`exec` window rests on the directory
trust policy; the memory limits are self-imposed as above; and there is no
parent-death signal, so an orphaned worker notices only at end-of-file.
Every failure path is still fail-closed — a missing or non-root
`sandbox-exec`, a refused profile, an image path that cannot be spelled as a
Seatbelt literal, a failed limit, or a missing readiness attestation ends the
launch with a sanitized error and no child, never with a weaker sandbox.

**Evidence.** `isolated_worker/macos_tests.rs` mirrors the Linux suite
against a real confined child: frame round-trips, orderly close, hang then
terminate, crash, non-zero exit, startup-deadline expiry, cancellation of a
blocked read, `Drop` reaping a live child, twenty consecutive launches, a
`SIGSTOP`/`SIGCONT` cycle, the six image-verification refusals, and an exact
post-`exec` descriptor inventory of `{0, 1, 2, 3}`. The one test with no
Linux counterpart is `the_sandbox_denies_every_confinement_probe`: a new
worker mode in which the image itself attempts a filesystem read, a
filesystem write, a loopback bind and a process spawn, and reports a bitmask
of what succeeded, which must be zero. That is what makes the sandbox
assertion falsifiable rather than a claim about a profile file — a Seatbelt
denial is an error return, not a kill, so nothing else would have noticed a
profile that silently stopped applying. Unit tests cover the Mach-O verifier
(a valid shape, an extra dylib, an `LC_RPATH`, a foreign dylinker, a missing
dylinker, a dylib/fat file, truncated load commands) and the profile
renderer (the placeholder is filled, `(deny default)` survives, an
unspellable path fails closed) on every host, including Linux.

**CI** now runs `cargo xtask worker-image` on macOS as well as Linux, so the
image identity audit — which on macOS is the Mach-O policy above, and on
Linux the unchanged static-ELF and symbol-table audits — runs natively on
both. A macOS-only diagnostic step, on failure only, compiles the profile
with `sandbox-exec` and dumps the unified log's Sandbox entries, because the
confined worker's stdio is `/dev/null` and a denial is otherwise invisible in
a CI log. `cargo xtask dist` bundles the worker image for a macOS host build
too, so a macOS release archive now ships a working `libexec/`.

**Not claimed.** No medium has been imported on macOS. The backend launches,
the worker hosts the real dispatcher, and the pipeline composes rather than
refusing at launch, but every macOS result here is from project-authored
tests on a CI runner. `docs/IMPORT_READINESS.md`'s matrix records the tuple
as composed-but-unevidenced, and no release-evidence gate is met on any
platform.

## M9.17 (Rust): the ride continues across the level change

Status: implemented (Rust); evidence: this PR ("The tram ride continues
across the first level change").

M9.15's chain walk stopped at distinct depth 2 with a concrete finding
rather than a number: from the first arrival point the only entity on the
whole frontier was a `func_tracktrain` that never departed, no matter the
heading, the wait, or the approach. This milestone is what that instrument
found, root-caused and fixed. Three separate engine gaps stood between the
opening ride and the rest of the campaign, and all three had to go.

- **A moving `func_train`/`func_tracktrain` now travels across a level
  change** (`crates/ohl-engine/src/transition.rs`:
  `TrackTrainCarry`, `capture_track_train`, `restore_track_train`). The
  public rule this project already works from is that entities persist
  across a transition when correlated by a shared `globalname`
  (`docs/FORMAT_SOURCES.md`, "Campaign flow"), and a train's route is a
  chain of `path_corner`/`path_track` nodes addressed by `targetname`
  ("Track trains and paths"). Put together, the only part of a chain
  position that means anything on the other side is the **name** of the
  node the train is at — so that name travels, with the train's progress
  along its active segment, direction, speed, whether it is moving, and
  any `wait` left; never a node index (which belongs to the source map's
  chain) and never a world position. On arrival the destination's own copy
  is re-seated on its node of that name, or given a chain rebuilt from it
  when the chain it built from its own `target` does not contain it.

  Before this, the destination map's copy of the train spawned at its own
  first node and — where its keyvalues start it moving — drove off empty,
  thousands of units from where the transition had put its passenger down.
  That is exactly the "tracktrain that never departs" M9.15 reported: from
  the arrival point it had already gone, and what the frontier saw was the
  car parked at the far end of its own track.

  `TrackTrainCarry` is deliberately **not** a field on
  `transition::EntitySnapshot`: that type is save tag 18 and frozen at its
  current shape (`crate::save`'s "Frozen section shapes" rule), and adding
  a field to it would invalidate every existing save file. It rides on
  `CarriedEntity`, which the save container does not serialize, and is
  applied only through the `globalname` correlation — never through the
  `targetname` fallback, for the same reason a `transform` never travels
  by name: a ride position is a placement.
- **The player's arrival offset is measured from their own origin, not
  their eye** (`Game::capture_transition`). The documented rule places the
  player at the offset from the landmark they had in the source map, and
  `Game::apply_transition` applies that offset to the arriving player's
  *origin*; capturing it from the camera instead raised them by the
  standing view offset on every single level change. Harmless-looking on
  a map you arrive standing on a floor — and fatal on one that hands you
  over mid-ride, where the extra height meant a short fall during which
  the car moved out from under the passenger. The camera is now placed
  above the arriving origin the same way an `info_player_start` spawn
  places it.
- **The collision model is re-baselined after a transition applies**
  (`Game::apply_transition` calls `Level::sync_brush_collision(0.0)`). A
  carried train is placed far from where the destination map spawned it,
  and the first step would otherwise read that placement as one step's
  worth of motion and hand a rider a base velocity of hundreds of
  thousands of units per second. A zero `dt` records the new positions
  with no velocity.
- **`path_track`'s documented fire-on-pass `message` is implemented**
  (`ohl_game::registry::PathFireOnPass`,
  `TrackTrainState::advance_firing`,
  `Simulation::advance_trains`). It was listed in `FORMAT_SOURCES.md` as
  documented-but-unimplemented; the ride needs it, because a scripted ride
  clears its own way — a real map hangs an obstacle over the track and
  moves it aside from a node the ride passes on the approach. Without it
  the obstacle stayed put and the ride's passenger was scraped off against
  it a few hundred units later.
- **The chain walk now reaches distinct depth 4.** Against a locally
  imported retail payload (identified only by its sanitized digest),
  `cargo xtask chain-walk` reports 3 routes assembled, **4 distinct maps**,
  3 level changes followed, 165.3 simulated seconds, ending on "The chain
  walk has no further route." — i.e. it ran out of authored routes rather
  than out of map. `xtask/chain-routes/c0a0-hop1.txt` is re-authored (ride
  the carried mover, then walk the last stretch on foot) and
  `xtask/chain-routes/c0a0-hop2.txt` is new (the ride is still under the
  player when that route starts, so it presses nothing at all).
- **Still open: a rider is not turned by a `func_tracktrain`.** A rider is
  carried by a mover's translation only, so a passenger standing away from
  a long car's own origin keeps their *world* offset from it through a
  corner rather than keeping their seat, and is eventually left hanging
  outside the drawn car. `c0a0-hop1.txt` works around it by stepping
  toward the middle of the car on arrival, which is a thing a player can
  do and the route says so. Closing it properly means posing the car's
  collision hull at the heading it is drawn at, which needs one fact no
  public page supplies: which way a train's compiled geometry faces before
  the engine turns it to face its segment. Measured against a real map,
  taking the drawn yaw literally and taking it 180 degrees away place a
  passenger in two different, both physically plausible, parts of the same
  car, and both were tried; nothing public decides between them, so this
  is recorded rather than guessed (see `docs/FORMAT_SOURCES.md`, "Riding
  movers").
- **Tests**: `crates/ohl-engine/tests/train_across_level_change.rs`, on a
  synthetic two-map fixture whose maps share node names the way two halves
  of one ride do. A moving train arrives where it left and keeps going
  (both the re-seated and the rebuilt-chain branch); a *stopped* train
  stays stopped and stays parked on its own node, so the carry moves real
  state rather than just "it moves"; the arriving player's own origin does
  not move when both maps place the landmark identically; and a node
  carrying a fire-on-pass `message` fires it as the train passes and not
  before. Each was verified to fail against a mutant that removes the
  behaviour it pins.

## M9.18 (Rust): the car a passenger rides is the car it collides with

Closes the "Still open" item M9.17 left above: a `func_tracktrain`'s
collision hull is now posed at the heading the car is drawn at, and a
passenger is turned with it.

- **One transform serves render, collision, `use`-proximity and riders**
  (`ohl_game::pose::brush_pose_rotation`). It reports the axis, angle and
  compiled-frame pivot a brush entity is posed at: a rotating mover's
  angle exactly as `mover_rotation` always did, *and* a `func_tracktrain`'s
  drawn yaw. `Level::sync_brush_collision` and
  `attach_brush_collision` feed it to
  `ohl_physics::CollisionModel::set_brush_pose`,
  `Renderer::draw_brush_entities` feeds it to the same
  `rotated_placement` matrix (which now takes that pivot), and
  `pose::brush_center` rotates the `use` point through it. Before this, a
  train's hull was translated only, so through a bend the drawn car and
  the colliding car pointed different ways.
- **The pivot is the origin brush, or the chain's first node.** A train
  built around an origin brush has its geometry compiled relative to that
  brush, so the pivot is the compiled frame's own `(0, 0, 0)`, as for
  every other rotating mover. A world-baked train (`origin` `0 0 0`,
  absolute vertices) has no origin brush, and the same first-node rule
  M9.16 already uses to measure its *translation* supplies the pivot too —
  a zero pivot would have swung such a car about the world origin, which
  for a car built thousands of units out is not a rotation but a
  teleport.
- **Which heading: project-determined by black-box comparison.** Which of
  the two physically plausible readings of the drawn yaw the published
  game's compiled car uses is stated by no public page, which is exactly
  why M9.17 recorded it rather than guessing. It was settled by building
  this project's own renders of a tram interior under each convention and
  comparing them against public screenshots of that interior: the
  as-shipped convention (`TrackTrainState::yaw_degrees` taken literally,
  no added half turn) matches; the other is the mirror image. No engine or
  SDK source was consulted and no payload-derived name, path or coordinate
  is recorded. See `docs/FORMAT_SOURCES.md`, "Riding movers".
- **A rider is carried through the turn as one rigid step**
  (`ohl_physics::rotational_ride_step`, `Level::rotational_carry`,
  applied in `Systems::player_move`). A train's heading is the direction
  of the straight segment it is on, so it turns the whole angle between
  two segments in the single step it changes segment on. The existing
  `omega x r` base velocity cannot ride that — over one step it walks the
  rider along the tangent instead of around the arc, into the wall the car
  has just swept over them — so the rider is rotated through the same
  angle about the same pivot the hull was, refused if the seat it lands on
  is not free, in which case the rider is left where they were standing
  and `base_velocity` falls back to the ordinary ride blend — the car's
  translation plus the tangential `omega x r` term for the rider's own
  position, carried through the traced move like any other mover ride —
  rather than translation alone. Every rotating mover whose per-step angle
  is small is unaffected: the two agree to a rounding error there.
- **The third hop's arrival point is no longer sealed.** The route
  investigation that closed out M9.17 found that arrival reduced to
  *exactly one* reachable cell with no frontier entity of any kind — no
  movement token could move the player at all, because the parked car's
  unrotated hull sat across the spot the transition hands them off to.
  With the hull posed at the drawn heading, a post-chain
  `--reachability-report` from the same arrival point reports tens of
  thousands of reachable cells, and `xtask/chain-routes/c0a0-hop2.txt` now
  steps off the car into them.
- **The chain walk reports distinct depth 3, down from M9.17's 4, and the
  drop is the fix working.** Both later hops are re-authored:
  `c0a0-hop1.txt` drops its "step toward the middle of the car"
  workaround, which existed only because a rider was scraped off a turning
  car and which now actively nudges the passenger off a seat they would
  otherwise keep. Riding the whole way instead crosses the same map's
  boundary earlier, at the point the car itself reaches it, and lands at a
  different arrival point in the same destination map — one where that
  map's own copy of the track runs out shortly afterwards. M9.17's fourth
  map was reached from the *other* arrival point, and reached it with a
  player who had been left standing frozen in geometry for 53 simulated
  seconds, pressing nothing and moving not at all, until a level change
  fired around them: an artifact of exactly the bug this milestone
  removes, not a route. `cargo xtask chain-walk` passes (its own
  `--min-depth` default is 2) at 3 routes, 3 distinct maps, 2 level
  changes, 93.6 simulated seconds.
- **Still open: the last stretch of hop 2 is behind two closed doors.**
  From the open ground the passenger now steps out into, the nearest
  `trigger_changelevel` is a few hundred units away behind two
  `func_door`s that the reachability walk finds are not use-openable from
  anywhere reachable — a door/trigger question, not a movement one, and
  the natural next thing to pick up.
- **Still open: a rider's *view* does not turn with the car.** Cheap to
  compute, but nothing public states that a GoldSrc mover yaws its rider's
  view, and adding it would silently redefine "forward" for every existing
  scripted route mid-ride. Left out deliberately; see
  `docs/FORMAT_SOURCES.md`, "Riding movers".
- **Tests**: a proptest in `crates/ohl-physics/tests/rotating_riders.rs`
  (a hull seated off the pivot on a body that turns a quarter circle over
  any number of steps, down to one, stays on it, is never inside its
  solid, and ends rotated about that pivot); a unit test in
  `crates/ohl-engine/src/level.rs` asserting render and collision agree on
  a *turning* train's pose at several progress values either side of a
  corner, alongside the existing rotating-door pair; and
  `crates/ohl-engine/tests/track_train_bend.rs`, which rides a synthetic
  square corner and asserts the passenger is never inside solid, never
  leaves the car's own live footprint, and ends up on the turned car
  rather than beside it. Both engine tests were verified to fail against a
  mutant that drops the rider carry.
- **Also, from the M9.17 review**: the post-transition
  `Level::sync_brush_collision(0.0)` re-baseline is now pinned by a test
  (removing it left every other gate green while the first step after a
  handover moved the train's hull at tens of thousands of units per
  second), the no-matching-node fallback of `transition::
  restore_track_train` is covered (a carried node name the destination
  declares nowhere leaves that map's own train on its own spawn node), and
  the two identical zero-`dt` syncs — after a transition and after a save
  restore — now cross-reference each other.


## M9.19 (Rust): a ride whose track runs out is carried onto the next one

Closes the "Still open" item M9.18 left above — the chained walk's third
map ended with the nearest level boundary behind two closed `func_door`s
the reachability walk found were not use-openable from anywhere reachable
— and it turned out not to be a door problem at all.

**What the instrumented walk found.** A local, uncommitted probe of this
project's own fire-chain dispatcher and of that map's own entity data (no
name, path or coordinate from it is recorded anywhere in this repository)
showed that the two blocking doors are named by exactly one thing in the
whole map: the documented fire-on-pass `message` of a `path_track` on the
tram's own `target` chain. Every other scripted beat in that map — the
station stop, the doors, the announcements, and the level change out of it
— hangs off nodes of the same chain. The tram never passed any of them,
because it was parked at the dead end of a *different*, three-node chain:
the short piece of track the ride shares with the previous map, which its
own copy continues for a few hundred units and then ends. The two chains
are over a thousand units apart, so no movement route could ever have
joined them. The map's answer to that gap is a `func_trackautochange`
sitting at the dead end — a rotating lift that takes the car down and
round onto the long chain — and this port implemented neither the entity
nor the `path_track` keyvalue that starts it. The keyvalue was already
recorded as documented-but-unimplemented (alongside `altpath`); the
*entity* appeared nowhere in this repository at all before this milestone
— no doc, no source, no test — so its public sources are written down here
for the first time.

- **`path_track`'s `netname` ("Fire on dead end")**
  (`ohl_game::registry::PathFireOnDeadEnd`,
  `ohl_game::track_train::PathNode::dead_end`). Documented as "entity to
  trigger when func_tracktrain reaches this path_track as a last
  path_track in a chain". `TrackTrainState::advance_firing` pushes it into
  the same fired-names list the fire-on-pass `message` already used, so
  `Simulation::advance_trains` fires it by name with the train as the
  activator, in the same deterministic order. It fires once per *arrival*,
  not once per attempt to leave: a train switched back on while parked at
  a dead end re-enters its travel loop, finds nothing ahead and stops
  again, and that second attempt must not fire the `netname` again — which
  for the real shape this exists for would send the platform straight back
  where it came from (`TrackTrainState::dead_end_fired`, cleared the moment
  the train leaves that node or is relinked).
- **`func_trackchange`/`func_trackautochange`**
  (`ohl_game::registry::TrackChange`/`TrackChangeLinks`,
  `ohl_game::logic::Simulation::{start_track_change, advance_track_changes,
  finish_track_change}`). Activated through the same `Simulation::activate`
  path every other mover uses. It picks up the train its `train` keyvalue
  names **only** when that train is resting on the `path_track` at the end
  it is setting off from, travels for the documented `height / speed`
  seconds, and hands the train over to the chain at the far end — the
  documented "after finishing, the train is assigned to path_track of the
  bottom path".
- **One transform, again.** The platform is an ordinary brush mover as far
  as `ohl_game::pose` is concerned: `track_change_offset` joins the other
  translating movers in `brush_offset` and `track_change_degrees` joins the
  other rotating ones in `mover_rotation`, so M9.18's single
  `brush_pose_rotation` answer already serves its renderer placement, its
  collision hull, its `use`-proximity point and anything riding it, with no
  new pose path. The carried *train* gets a displacement and an extra yaw
  of its own (`TrackTrainState::set_carry`, applied in
  `pose::track_train_transform`), interpolated between the two documented
  endpoint nodes rather than replaying the platform's own `height` — which
  is what makes the arrival land exactly on the node the pages say the
  train is assigned to, with no snap at the end for a passenger to be
  scraped off by. A rider is carried by the same `base_velocity` /
  `rotational_carry` machinery M9.18 built; nothing new was needed for
  them.
- **A relinked train keeps its compiled reference point.**
  `TrackTrainState::first_node_position` used to read the current chain's
  first node. That is the stand-in an origin-brush-less, world-baked car
  has for an origin brush (M9.16), i.e. a fact about where its vertices
  were compiled — so recomputing it from a chain the car was *handed over*
  to teleported such a car by the whole distance between the two tracks
  the instant the platform finished. It is now captured once at spawn and
  survives a relink. Caught by the new engine fixture, whose train is
  deliberately the world-baked shape.
- **Which of the two readings of "Auto Activate train".** The relinked
  train rides on. No page reviewed states what the published game's
  unflagged platform does — TWHL names the flag and leaves its description
  blank, and the only description found is the Sven Co-op *mod's*, a
  different engine. The alternative reading delivers a ride onto a track
  with nothing in its map able to start it again, which is the same shape
  of progression stopper M9.13 already recorded for a zero "New Train
  Speed" taken literally. Recorded as `TODO(black-box)` at the point of
  use and in `docs/FORMAT_SOURCES.md`, "Track trains and paths".
- **The chain walk reports distinct depth 4**, up from M9.18's 3, and the
  third route is now the ride again rather than a step off a parked car.
  `xtask/chain-routes/c0a0-hop2.txt` presses nothing: the car runs its
  track out, is carried down and round, rides on through the doors that
  section of the ride opens ahead of itself, pauses where the ride is
  scripted to pause, and ends at a level change the end of the ride fires
  by name. `cargo xtask chain-walk` passes at 3 routes, 4 distinct maps, 3
  level changes, 167.7 simulated seconds. The passenger is aboard for all
  of it: a probe of `Game::ground_mover_speed` across the route shows it
  changing between the ride's documented speeds and returning to zero only
  at the two scripted station stops, never a fall and never a frozen pose.
- **Still open: the fourth map's arrival point is sealed.** A post-chain
  `--reachability-report` from where the ride sets the player down reports
  exactly **one** reachable cell and no frontier entity of any kind, and
  the nearest `trigger_changelevel` is ~280 units away and unreachable.
  Two simulated minutes of standing still change nothing: no mover comes
  to free the player. That is the same *symptom* M9.18 fixed for the third
  map but not the same cause (that one had a `func_tracktrain` on the
  frontier; this one has no frontier brush entity at all), so no
  `c0a0-hop3.txt` is authored here — there is no honest route out of a
  sealed cell, and guessing one would be exactly the frozen-in-geometry
  artifact M9.18 removed. It is the next thing to pick up.
- **`TODO(black-box)`: the "Start at Bottom" end-of-chain reversal.** The
  published `toptrack`/`bottomtrack` descriptions carry a parenthesised
  clause — with that flag set the two names point at the *other* end of
  each chain — and `finish_track_change` always seats the relinked train
  at node `0` of the chain the destination name resolves to. Such a
  platform's downward destination is therefore documented to be a chain's
  last node, where the train would be handed a one-node chain and dead-end
  on arrival. No page reviewed states which way a train handed a chain's
  far end is meant to travel, and nothing in the tree sets the flag, so it
  is quoted in full and marked rather than guessed at; see
  `docs/FORMAT_SOURCES.md`, "Track trains and paths".
- **Not saved.** A save taken mid-trip, or after a platform has relinked a
  train, restores that train on the chain its own `target` names: save tag
  28's `MoverSnapshot` is index-based against the chain rebuilt at load and
  its wire shape is frozen. The level-change carry is unaffected —
  `transition::capture_track_train` records the node by *name*, which after
  a relink is a node on the new chain, which is why the walk's own hop
  across the boundary after the track change works. Recorded as a known
  gap in `docs/FORMAT_SOURCES.md`.
- **Follow-up, older than this branch: single-tick ride-speed spikes at a
  node.** A per-tick probe of `Game::ground_mover_speed` over the whole
  chain shows isolated one-tick spikes of a thousand-odd to a few thousand
  units per second wherever a `func_tracktrain` changes path segment — the
  per-node yaw snap, whose whole turn happens in one step (M9.18). Most of
  them occur on routes this milestone did not touch, so it predates this
  branch and is left alone here; it wants its own look, since a rider's
  `base_velocity` is read from that same per-step displacement.
- **Tests**: `crates/ohl-engine/tests/track_change.rs` rides a synthetic
  fixture (`test_support::track_change_bsp`: a void world, a car on a
  two-node top chain whose dead end names a `func_trackautochange`, and a
  two-node bottom chain three hundred units below and a quarter turn
  round) and asserts the passenger is never in solid, is on a mover every
  step of the trip, is part-way down at half the documented duration
  rather than teleported at the end, lands on the bottom chain and rides
  off along it. Both tests were verified to fail against two separate
  mutants: one that drops the dead-end fire, one that drops the rider
  carry. `ohl_game::logic`'s own unit tests cover the handover without any
  engine (the platform starts, is part-way at half the trip, arrives, and
  the train ends up riding the bottom chain), that a dead end fires its
  `netname` once per arrival even when the parked train is re-triggered
  there (a second train stands in as the witness, since activating one
  *toggles* it, so "still running a second later" is a parity check on how
  many times the dead end fired — verified to fail with the once-only
  guard removed), and that a platform whose named train is somewhere else
  travels empty instead of dragging it over.

## M9.20 (Rust): the map that stopped the ride it was handed

Closes the "Still open" item M9.19 left above — the chained walk's fourth
map, whose arrival point a `--reachability-report` found sealed at exactly
one reachable cell with no frontier entity of any kind. It was neither a
missing entity nor a geometry problem, and it had two independent causes,
each of which is on its own enough to seal the cell and each of which is on
its own enough to open it again.

- **`trigger_auto`/`trigger_relay`'s documented `triggerstate`.** The
  published pages describe the key as choosing the use *type* a trigger
  sends — "On- turns entity on; Off- Turns entity off; Toggle- turns entity
  On when it's Off and vice versa" — with values `0`/`1`/`2`. This port read
  every fire as a plain toggle. Of the four maps the chain has entered, this
  one alone starts its own copy of the ride with a `trigger_auto` that
  declares the key as **On**, one second after the map loads; read as a
  toggle, it switched off whatever was moving and nothing else in the map
  names the train. Implemented as
  `ohl_game::registry::TriggerUse`/`TriggerUseType`, carried on
  `ohl_game::logic::Fire::use_type` from `Simulation::fire_typed` to
  `Simulation::activate_with`. Only `func_train`/`func_tracktrain` acts on
  it — the one state machine here whose own published keyvalues describe an
  explicit on and an explicit off — because a search summary of TWHL's
  `trigger_relay` page says most entities ignore the signal and just
  toggle, naming `func_door` among them. Citations and three
  `TODO(black-box)` boundaries (no propagation down a fire chain; the frozen
  save section drops the use type; the published "Off" default for an
  *absent* key is knowingly not followed) are in `docs/FORMAT_SOURCES.md`,
  `trigger_auto`.
- **A carried train's ride is no longer discarded with its position.**
  `restore_track_train` correlates a carried ride by the *name* of the node
  it is at. This boundary is the first in the chain where the destination
  map declares no node of that name at all, and the whole carry was then
  dropped — so the destination's copy ran on its own `startspeed` as if the
  ride that had just arrived had never happened, which is also why the
  map's own `trigger_auto` had a moving train to stop in the first place.
  The destination's own chain and its own place along it are kept; only the
  motion travels.
- **A carried mover keeps its own map's compiled keyvalues.** With the ride
  running, a four-leaf door reported as *open* still stood closed across the
  tunnel: its leaves slide up, down, left and right, and all four had been
  handed the previous map's same-named leaves' single move direction and
  travel distance, because a carried `EntitySnapshot` overwrote the whole
  `Door` component rather than its state.
  `EntitySnapshot::apply_onto_existing` now writes only the state and timer
  of a `func_door`/`func_button`/`func_plat`/`func_rotating` the destination
  already declares, for exactly the reason `Transform` has never travelled.
  The one exception is `Door::rotation_axis`, whose axis is compiled but
  whose *sign* is the activator-chosen swing side (M7's rotating-door rule):
  that sign travels onto the destination leaf's own axis, so a carried-open
  rotating door is not mirrored onto the wrong side of its own frame. The
  full snapshot is still applied when the transition creates an entity the
  destination does not declare.
- **The chain walk reports distinct depth 5**, up from M9.19's 4, at 4
  routes, 4 level changes and 243.3 simulated seconds, ending on "no
  further route" rather than a stop or a re-entry.
  `xtask/chain-routes/c0a0-hop3.txt` is one `wait` line and presses nothing.
- **Measured, per tick, across the whole chain** (`PlayerState::ground_brush`
  classname and `Game::ground_mover_speed` logged every tick from a local,
  uncommitted probe of `run_script_ticks`):

  | route | ticks | on a `func_tracktrain` | ride speed > 1 |
  |---|---|---|---|
  | start | 2357 (39.3 s) | 2108 (89.4 %) | 35.1 s |
  | hop1 | 2601 (43.4 s) | 2600 (100 %) | 43.3 s |
  | hop2 | 5103 (85.0 s) | 5102 (100 %) | 70.5 s |
  | hop3 | 4536 (75.6 s) | **0 (0 %)** | 0.0 s |

- **Still open, and the real next item: the carry does not put the
  passenger on the fourth map's train.** The player is aboard on hop2's
  last tick and on none of hop3's. They fall for about nine tenths of a
  second on arrival and then stand on world geometry for the rest of the
  route, while the map's own ride runs and fires the level change by name.
  So hop3 reaches depth 5 *honestly* — the route presses nothing, nothing
  is teleported, and the level change is one the map itself fires — but the
  player is a bystander for it, not a passenger, and this milestone does
  not claim otherwise.

  The mechanism is measured, not guessed. In the car's own frame the
  passenger rides about 115 units behind its centre, in a car whose hull is
  144 units long from that centre: a seat with under thirty units of
  margin, which they are left in by the *start* map, where they spend the
  first four seconds airborne while the car pulls out from under them and
  land near its back wall. At this boundary the destination chain's head
  sits about 26 units short of where the ride crosses, and its first
  segment's heading differs from the arriving car's by about 14 degrees;
  applied to a seat that far off the pivot, that moves it a further 28
  units back — just past the floor. Neither number is something the carry
  can correct by name, so the fix belongs upstream: where a world-baked
  car's hull is placed at spawn (so the passenger starts amidships rather
  than against the back wall), and the documented `wheels`
  heading-lag keyvalue that this port records but does not apply. Both are
  their own milestones.
- **Tests**: `crates/ohl-engine/tests/trigger_state_and_carry.rs`, on
  project-authored synthetic fixtures — a rolling train an "On"
  `trigger_auto` must not stop, one an "Off" one must, one an absent or
  explicit "Toggle" one still toggles; a parked ride that must arrive parked
  even when the destination declares no node of the carried name; a
  `func_door` whose destination-map leaf keeps its own move direction,
  travel distance and `speed` while its open state travels; and a
  `func_door_rotating` that keeps the swing side its activator chose. Each
  of the three new rules was verified to fail with its own arm disabled.

## M9.21 (Rust): the door group the ride arrived at four seconds late

Closes the "Still open" item M9.20 left above. The second door group near
the end of the fourth map's loop was opened and auto-closed about four
seconds before the car reached it. The wiring was followed correctly all
along; two separate timing rules were wrong, and neither alone accounted
for the gap.

- **A `multi_manager` is single-threaded unless it declares the
  "multithreaded" spawnflag.** Three identical `func_tracktrain`s share
  one `path_track` chain here, staggered by a manager's own delays, and
  every one of them fires the shared node's `message` as it passes. This
  port ran a fresh copy of that manager's schedule each time, so the
  manager that opens the door group and re-starts the stopped ride ran
  three times, roughly a second apart. Because a `multi_manager` fire is a
  toggle, the second copy's ride target *stopped* the ride again a second
  after the first had released it, and the third started it once more —
  1.7 s of stall the map never asked for, and the visible "extra stop near
  the station" M9.20 recorded as worth checking. The published default is
  the opposite: a manager still working through its targets ignores a new
  activation, and only the `multithreaded` spawnflag (value 1) lets it run
  more than one copy. Implemented as
  `ohl_game::registry::MultiManager::multithreaded` plus a per-manager
  busy timer in `ohl_game::logic::Simulation`; citations and the
  save-shape `TODO(black-box)` are in `docs/FORMAT_SOURCES.md`.
- **A stopped train resumes at its own `speed`.** The rest of the gap was
  the ride crawling. Its track brakes it down through a documented "New
  Train Speed" ramp on the approach to a scripted halt; the two nodes
  after the halt declare no speed change at all. This port therefore
  released the ride at the crawl the braking ramp had left it at, so it
  needed nine seconds to cover a stretch the door group is only open five
  seconds for — no `wait`, no manager delay and no second-train speed can
  close a gap that large, because the door's opening and the ride's
  release hang off the *same* manager and move together. The published
  pages describe a train's `speed` as the train's own (maximum) speed and
  a node's as an override applied "after reaching this point", so
  `TrackTrainState::turn_on` now restarts a stopped train at its `speed` —
  which is already exactly what `TrackTrainState::spawn` does for a train
  with no `startspeed`. A train that is already moving is untouched.
  `TODO(black-box)` and citations in `docs/FORMAT_SOURCES.md`.
- **Result on the real chain.** Timeline of the ride's last stretch,
  rounded to the second and measured from the ride's own start: the
  manager fires once (was three times); the door group is told to open
  6 s later and stands fully open 7 s later; the ride is released 7 s
  later and now reaches the group 3 s after that, with about 2 s of the
  door's own `wait` still to run. Before, it arrived 15 s after the
  release — 3 s after the group had finished closing. The chain walk still
  reports distinct depth 5 on 4 routes and 4 level changes, now in 233.6
  simulated seconds (was 241.1), ending on "no further route".
- **Tests**: `ohl_game::logic`'s
  `a_released_ride_reaches_the_door_group_before_its_wait_expires` and
  `a_multithreaded_manager_accepts_the_second_re_fire` run a synthetic
  fixture with the same shape — a ride braked by a node override and
  stopped at a scripted halt, two identical trains sharing one chain, and
  the manager they both fire opening a `wait`-timed `func_door` and
  releasing the ride — and each fails if either rule is reverted;
  `ohl_game::track_train`'s `a_restarted_train_resumes_at_its_own_speed`
  pins the restart speed on its own.
- **Still open: the passenger is lost in the transition into this map**,
  before the loop begins, so the ride now runs the loop and clears the
  door group with nobody aboard. The arrival places the player short of
  the car, which drives off at its `startspeed` without them; they stand
  where they landed for the rest of the route and arrive in the fifth map
  from there. That is a transition-placement question, not a map-logic
  one, and is being picked up separately.
## M9.22 (Rust): the yaw-snap ride-speed spike, resolved

Closes the "Follow-up, older than this branch: single-tick ride-speed
spikes at a node" item M9.19 recorded above (M9.20/M9.21, above, landed a separate fix to the same map in the meantime; this one is independent).

**What a per-tick probe found.** A local, uncommitted probe stepped the
`crates/ohl-engine/tests/track_train_bend.rs` fixture at the physics
engine's own fixed tick (`ohl_physics::controller::TICK_SECONDS`, not the
`1.0 / 60.0` most tests use, which does not divide evenly into it and can
coalesce more than one physics step into a single `Game::tick` call,
hiding the very spike being hunted) and logged `Game::ground_mover_speed`
alongside the player's own raw position delta divided by `dt` every tick.
Both read the same large number — several thousand units/second — on the
exact tick a `func_tracktrain` changed path segment, and nowhere else.
That ruled out a pure reporting bug (the earlier guess that
`ground_mover_speed` was dividing a rigid rotational step by `dt` as if it
were an ongoing rate, while the player's actual motion stayed modest): the
rider's seat really was being carried through the corner's whole chord in
one tick, because `TrackTrainState::yaw_degrees` turned the car's hull
through the entire angle between two path segments the instant it reached
the node between them (recorded, not newly introduced, by M9.18's rigid
per-tick carry). `PlayerController::velocity` itself was never touched —
only the rider's position was rotated, so nothing persisted into later
ticks or launched the player onward — but the one-tick chord was still a
real, large jump in where they were, and `Game::ground_mover_speed`
faithfully reported it.

**The fix.** `TrackTrainState::yaw_degrees` now blends a corner's heading
change over a short distance of the new segment (a train's `wheels`
keyvalue when positive, `ohl_game::track_train::DEFAULT_YAW_BLEND_DISTANCE`
otherwise — see that constant's and `TrackTrain::wheels`'s doc comments;
no public source documents `wheels`' real turn-lag formula, so this is a
project-determined choice, not a claimed match to it) rather than
reporting the new segment's exact heading the instant the train reaches
the node. Every consumer of a `func_tracktrain`'s heading —
`ohl_engine::render`'s draw pose, `Level::sync_brush_collision`'s
collision pose, and a rigid-carried rider's own turn — reads this one
function, so blending it there is enough to turn a sharp corner into a
short, smooth swing everywhere at once, with no separate change needed to
keep render, collision and the rider's ride in agreement.
`Game::ground_mover_speed` itself was also changed to measure the actual
per-tick chord `Level::rotational_carry` produces (translation plus the
rotational step's own displacement over `dt`) rather than the
instantaneous tangential rate a spin's angle-per-tick would suggest at the
limit of a vanishingly small step — the two agree closely for an ordinary
slow `func_rotating`/`func_door_rotating` turn, but only the chord measures
what a sharp corner's one-tick heading change actually moved a rider by.

- **New regression test**:
  `crates/ohl-engine/tests/track_train_bend.rs`'s
  `a_riders_reported_speed_never_exceeds_the_cars_own_by_more_than_a_small_bound`
  steps the bend fixture at the engine's own fixed tick through a full
  corner and asserts `Game::ground_mover_speed` never exceeds twice the
  car's own travel speed. `crates/ohl-game/src/track_train.rs` adds unit
  tests for the blend itself (`yaw_blends_from_the_previous_segment_across_the_window`,
  `a_positive_wheels_keyvalue_shortens_the_blend_window`) and
  `crates/ohl-engine/src/level.rs`'s
  `render_and_collision_agree_on_a_turning_track_train_pose` was extended
  to sample far enough past the corner to see the blend actually finish,
  since it now takes longer than one tick. The existing bend, rotating-rider
  and track-change fixtures (M9.18, M9.19) were re-run unchanged and still
  pass, confirming a rider is neither scraped nor dropped by the blended
  turn. `cargo xtask combat-smoke` (37/37) and `cargo xtask chain-walk`
  (same chain depth as before this change) were both re-run against the
  local payload.
- **Review follow-up: the blend missed a looped chain's own wrap
  corner.** `TrackTrainState::previous_node_index` (what both the blend
  above and the parked-at-the-end fallback read "the previous segment's
  heading" through) used raw `checked_sub`/`checked_add` arithmetic
  instead of the chain's own looped-aware `PathChain::prev_index`/
  `next_index` pair that `other_index` (the node *ahead*) already used one
  screen above it. So a *looped* chain's wrap node — `node_index == 0`
  moving forward, where a non-looped chain truly has no previous node —
  reported `None` there too, and the blend fell back to the new segment's
  heading unblended: every interior corner of a loop blended correctly,
  but the wrap corner kept snapping its whole turn in one tick, on exactly
  the kind of track (a loop) this fix exists for. Swapped to the same
  looped-aware pair `other_index` uses; verified locally that a probe of a
  400x400 looped square over 2.5 laps drops from a 90-degree one-tick step
  at the wrap to about 0.35 degrees, matching every other corner. New
  tests: `crates/ohl-game/src/track_train.rs`'s
  `previous_node_index_wraps_on_a_looped_chain_instead_of_reporting_none`
  (direct, unit-level), `a_looped_square_tracks_worst_per_tick_yaw_step_stays_small`
  (a proper four-corner loop, worst per-tick yaw step over 2.5 laps bounded
  well under a one-tick snap) and `yaw_blends_across_a_looped_chains_wrap_corner_too`
  (the yaw right at the wrap reads as the incoming heading, not the
  outgoing one) — all three verified to fail against the pre-fix
  arithmetic and pass against the fix. As a side effect, a train parked
  exactly at a looped chain's wrap node now also resolves a previous
  segment to blend from, where it previously had none to find.
- **Review follow-up: a discriminating test for the `ground_mover_speed`
  rewrite.** Nothing pinned that the metric now reads the actual chord
  rather than the old instantaneous tangential rate, because an ordinary
  `func_rotating` turntable's per-tick angle is small enough that the two
  formulas already agree — the yaw blend above, not the metric rewrite,
  is what carries the existing regression tests. Added
  `crates/ohl-engine/tests/rotating_riders.rs`'s
  `ground_mover_speed_matches_the_chord_even_for_a_large_single_tick_turn`,
  which cranks a turntable's spin to a large single-tick angle (comparable
  to a `func_tracktrain` corner's own pre-blend snap) where the chord and
  the arc-rate clearly disagree, and a companion
  `ground_mover_speed_matches_the_riders_own_observed_per_tick_displacement`
  pinning the ordinary case too. Both verified against restoring the old
  `brush_ride_velocity`-only body.
- **Review follow-up: doc nits.** `docs/FORMAT_SOURCES.md`'s "Riding
  movers" paragraph said a `func_tracktrain`'s hull "turns" (present tense)
  by a whole segment's angle in one tick — true before this milestone,
  stale after it; reworded to the past tense with a pointer to the fix.
  `wheels`' unit was unstated where this milestone's own prose introduced
  it (unlike the neighbouring `speed`/`height` entries, which both say
  theirs); now states it is in map units.

## M9.23 (Rust): the passenger is aboard from the departure's first tick

The campaign's opening ride starts with the player standing in a
`func_tracktrain`. A per-tick probe of that route (local, uncommitted;
player origin, `PlayerState::ground_brush`, `on_ground`, velocity, the
car's own chain position/speed/`moving`, and the `base_velocity` the host
would look up) found the passenger was not riding it at all for the first
few seconds:

- for roughly the first two hundred ticks the player's world position was
  *constant* — not falling, not sliding — with `ground_brush = None`, a
  ride speed of zero, and a vertical velocity pinned at exactly one
  half-gravity step, the signature of a move that is refused outright every
  step rather than one that is falling;
- the car, still parked at that point, then departed and slid out from
  under them, and their seat in the car's own frame travelled from roughly
  +119 units ahead of its centre to roughly 114 units behind it *without
  the player moving at all*;
- they then fell onto the floor near the back of the car and rode the rest
  of the route from there.

**Root cause.** The spawn point placed the player's standing hull inside
the car's own solid, by a handful of units. Every consequence follows from
`ohl_physics::categorize_position`: its ground probe requires
`fraction < 1 && !all_solid && plane_normal.z >= slope_limit`, and an
embedded hull satisfies none of it, so `ground_brush` stays `None` — which
is what the host's `base_velocity` lookup keys off, so the ride never
starts. The traced move cannot recover either: every trace out of solid is
refused, which is why the player did not so much as fall. The state was
therefore self-sustaining until the car's geometry moved far enough to stop
overlapping them. Note that none of the hypotheses about *ordering* held:
the car's hull is attached and posed before the first move, the player's
ground brush is resolved on the first step that runs, and the car does not
begin moving until well after spawn — the passenger was already stuck
before it did anything at all.

**Fix.** `ohl_physics::settle_at_spawn`: the same bounded upward nudge a
landing already uses (`unstick_from_ground`, unchanged bound and step),
followed immediately by `categorize_position`. `ohl_engine::Game` runs it
once when a level is placed from an `info_player_start` — a fresh load, and
a transition that falls back to the destination's own spawn — and both call
sites are gated on the level actually having a spawn point, not merely on
its having collision: with no `info_player_start` the controller sits at
`PlayerController::default`'s world origin, a placement no map authored and
nothing should nudge. On the fresh-load path no brush sync is needed first
— `Level`'s own `attach_brush_collision_with` already attaches each brush
at `origin + ohl_game::pose::brush_offset(..)` while the level loads, and
nothing has moved a `Transform` since, so the mover hulls are posed by the
time the settle runs. (The transition path's own zero-`dt`
`sync_brush_collision` *is* load-bearing, for the opposite reason: there
the carry has just moved movers after attach.) A landmark-relative arrival
deliberately gets no settle: that placement is a pure offset from where the
player stood in the source map, and nudging it would stop a boundary being
a no-op for the physics state. A player spawned in mid-air still falls
exactly as before, and a spot that is solid all the way through the bound
is still left alone.

**Result on the real start map**, same probe: the passenger's ground brush
is the `func_tracktrain` on tick 0 and stays so for the whole route, and
their seat in the car's own frame stays within about four units of where
they spawned for the entire ride, corner included, instead of sliding some
two hundred and thirty units down the car. Per-route aboard fraction over
`cargo xtask chain-walk` (ticks with a `func_tracktrain` as the ground
brush, over ticks run), measured at this milestone's own base (M9.22) both
with and without the settle:

| route | without | with |
|---|---|---|
| 0 | 2108/2357 (89.4%) | **2309/2310 (100%)** |
| 1 | 2600/2601 (100%) | **2600/2601 (100%)** |
| 2 | 5098/5099 (100%) | **5145/5146 (100%)** |
| 3 | 0/4016 (**0%**) | **3952/4016 (98.4%)** |

The one missing tick in the first three is the level-change tick itself, on
which the player has already been placed in the next map. The chain's own
aggregates are unchanged (4 routes, depth 5, 4 level changes, 234.6
simulated seconds, no re-entry).

M9.20 recorded "the carry does not put the passenger on the destination
map's train" as its open item, naming world-baked spawn placement as one of
the two upstream fixes it needed. This is that fix, and it closes the
fourth route too, for the reason M9.20's own arithmetic predicted: the
passenger used to arrive at that boundary seated about 114 units *behind*
the car's centre, and the destination chain's head sits about 26 units
short of where the ride crosses with its first segment about 14 degrees off
the arriving car's heading, which moved that seat just past the back of a
hull 144 units long from the centre. Settled at the seat the map actually
spawns them in — about 119 units the *other* way, toward the front — the
same 26 units and 14 degrees move them further *inside* the car, so they
arrive aboard. Their remaining 64 ticks off the car are the arrival itself
(a ~1.1 s fall onto the destination car, since a landmark-relative arrival
deliberately gets no settle) plus that route's own level-change tick; the
end-of-loop door scrape M9.20 recorded is already closed by M9.21, so
nothing else on that route drops them.

**Tests**: `crates/ohl-engine/tests/spawn_inside_mover.rs` against a new
synthetic fixture (`test_support::embedded_spawn_track_train_bsp`: a void
world, a `func_tracktrain` on a straight two-node `path_track` chain with a
non-zero `startspeed` so it is moving on the first step, and an
`info_player_start` placed twelve units *inside* the car's solid). It
asserts the passenger has settled onto the car's floor before any tick
runs, is riding it at the car's own speed within two steps, and keeps their
seat within four units of where they spawned — and stays on the car's own
footprint — for the whole departure. Both tests were verified to fail with
the settle call removed (seat slid off by step 2; the player was left
twelve units inside the floor).

A third test pins the *gate*: a fixture with real collision but no
`info_player_start`, whose one solid block swallows the world origin, must
leave `PlayerController::default`'s placement untouched. Verified to fail
with the `Level::spawn` half of the guard removed (the player was lifted
28 units out of the block).

**Still open.** This does not place the passenger *amidships*: they are
settled where the map's own spawn point puts them, near one end of a long
car. That is now the favourable end for the fourth boundary, but it is a
seat with under thirty units of margin either way, so a boundary whose
chain head is offset the other way would still lose them; the underlying
gaps (the ~26-unit chain-head offset and the ~14-degree heading difference
at that boundary) are unchanged.

## M9.24 (Rust): the chain walk reaches the opening chapter's last interior map

The chain walk stood at distinct depth 5 (four routes, four level changes).
This milestone extends it by one hop, `xtask/chain-routes/c0a0-hop4.txt`,
authored from a per-tick probe (local, uncommitted; player origin,
velocity, `on_ground`, whether `ground_brush` was attached, ride speed, and
a zero-length trace's `start_solid`/`all_solid`/`brush_index` at the
player's own standing-hull origin) of the fourth boundary's arrival.

**What the probe found.** From the very first simulated tick of this
arrival the player is already `on_ground` with zero velocity and a
constant position — standing on world geometry, never in solid, never
observed to fall. Nothing moves them for the rest of the route; the level
change that ends it is fired by the map's own scripted ride reaching the
far end of this section, by name, not by anything a route presses. (This
map's arrival is distinct from the boundary just before it: M9.21 already
measured that earlier route, from `c0a0-hop3.txt`'s own arrival, at 91.5%
of its ticks with the player aboard the ride's own `func_tracktrain`, so
that route is not a case of the player standing apart from a mover that
left without them — the same measurement re-confirmed here.) A route
pressing nothing at all (one `wait` line) reaches this boundary's level
change honestly.

**Result.** `cargo xtask chain-walk` now reports distinct depth **6** (five
routes, five level changes). The newly-reached sixth map is the last
interior map `ohl_campaign::CHAPTER_MAPS` lists for "Black Mesa Inbound",
where the opening chapter's tram ride ends at the station.

**Blocked past this point.** A further hop (from this sixth map's own
arrival point) was attempted and found genuinely blocked, not merely
unauthored. The same per-tick probe shows the player's standing hull
embedded in solid from the map's very first tick: velocity pinned at
exactly one gravity step, `on_ground` never true, position never changing,
and the targeted trace reporting `start_solid = true`, `all_solid = true`,
`brush_index = None` — embedded in *world* geometry, not in a mover or any
attached brush entity. A post-chain `--reachability-report` from this same
arrival point confirms it independently: round 0 reports exactly one
reachable cell, nothing on the frontier, and the map's own
`trigger_changelevel` roughly 5,200 units away and unreachable.

**Classification: placement.** This is the same family of bug M9.23 fixed
(a spawn/arrival hull embedded in solid, which `categorize_position`'s
ground probe can never resolve and which no trace can recover from), but
at a boundary M9.23 deliberately left unsettled: the arrival here is
landmark-relative, and `Game::from_level`'s settle-at-spawn nudge is only
ever applied to an `info_player_start` placement (a fresh load, or a
transition's landmark-less fallback) — see M9.23's own note that a
landmark-relative arrival "must stay a pure offset" so that a boundary
that lines the maps up exactly is a no-op for the physics state. At this
particular boundary the offset instead lines the player up inside static
world geometry. Not a missing entity, not an unrun trigger chain, not a
mover/ride timing gap, and not scripted or monster gating — the map never
runs a single simulated tick for this player before they are already
stuck. Fixing it belongs with the settle-at-spawn machinery itself
(whether, and how, to bound a nudge for a landmark-relative arrival
without turning it into something other than a pure offset), not with a
route file.

**Gates**: fmt, `cargo test -p xtask`, policy, combat-smoke 37/37,
`cargo xtask chain-walk` (distinct depth 6, five level changes, no
re-entry).

## M9.25 (Rust): a passenger crosses a boundary in their seat, not at an offset

M9.23 settled a player the map spawns inside a mover; M9.24 then found the
next two arrivals still placing them badly and classified both as
"placement". This milestone fixes the placement rule itself. Measured with a
per-tick probe (local, uncommitted, reverted before committing: player
origin, velocity, `on_ground`, `PlayerState::ground_brush` resolved back to
its entity, the ridden car's posed centre/heading/chain state, and a
zero-length trace's `start_solid`/`all_solid`/`brush_index` at the player's
own standing-hull origin).

**The rule.** `ohl_engine::transition::RiderSeat`: when the player's ground
brush at the instant of a `trigger_changelevel` is a named *ride* (an entity
carrying a `TrackTrainState`, i.e. a `func_train`/`func_tracktrain`), their
*seat relative to that ride* is what crosses, and it takes precedence over
the documented landmark offset. Only a ride qualifies, and that is the whole
of the rule's justification: a train is the one brush entity whose placement
comes from a `path_track` chain rather than from where its geometry was
compiled, so it is the only one the landmark offset can disagree with. Every
other mover is placed by its own compiled bounds plus its `origin` keyvalue
in the destination map's own coordinates, which is exactly what the offset
already agrees with, so a player standing on a named `func_door`,
`func_plat` or `func_wall` keeps the documented offset. The seat is recorded in the mover's own
frame — the offset from its posed centre (`ohl_game::pose::brush_center`),
turned back through the mover's own yaw
(`ohl_game::pose::track_train_transform`) — so a destination copy of the
ride that faces a different way still seats the passenger in the same part
of the car.

Why the offset alone cannot do it: it assumes whatever the player stood on
sits in the same place relative to the landmark in both maps. That is true
of world geometry and false of a shared ride, whose destination copy is
placed by the destination map's own `path_track` chain
(`restore_track_train`). On this campaign's tram boundaries the two chains
disagree by tens of units and about a dozen degrees, which was enough to
push a passenger through the car's interior wall. The seat is captured
*before* the transition-volume/radius eligibility test, deliberately: that
test decides which entities travel, and a seat is not an entity — it is part
of the player's own placement, and the player always travels. All the
destination needs to reproduce it is a counterpart of the same name.

A player standing on world geometry, on a mover the destination map does not
declare, or crossing a boundary with no landmark at all is placed exactly as
before.

**A ride that ends in the destination.** The last map of a shared ride parks
its own copy of the car on a chain of exactly **one** `path_track` — no
segment anywhere in it, so `TrackTrainState::yaw_degrees` has neither a
segment ahead nor a segment behind to measure from and the car is posed
*unrotated*, across the track its geometry was compiled along. Measured on
the real boundary: the source car reported a heading of -90 degrees, the
destination's copy reported none, and the arriving passenger's seat landed
in open air with nothing within 256 units below them. The heading the ride
arrived with is therefore carried
(`TrackTrainCarry::yaw`/`RiderSeat::yaw` -> `set_handover_yaw`) and used
strictly as `yaw_degrees`' last fallback: any chain that defines a heading
at all still wins, so a ride that continues is unaffected.

That heading is **save state**, and gets its own section:
`SECTION_TRAIN_HANDOVER_YAW` (tag **35**), one optional `f32` per registry
entity in spawn order. For a car parked on a single-node chain it is the
only heading that car has, and it is load-bearing *player* placement — a
quicksave taken on that arrival and reloaded would otherwise pose the car
unrotated and drop the passenger through where its floor used to be, which
is the same bug this milestone fixes, one save later. A new tag rather than
a field on `TrackTrainSnapshot`: tag 28 is shipped and frozen at its own
wire shape, so new persisted state always gets its own optional tag (tag 32
stays reserved for `ohl-player`). Absent, the section reads as `None` and a
save written before M9.25 loads exactly as it did.

**And the embedded case.** `ohl_physics::settle_if_embedded`: a
landmark-relative arrival still gets no nudge — unless the offset put the
standing hull *inside* solid, which is not an offset the physics state can
carry across at all (no ground brush resolves while `start_solid` holds, and
no traced move out of solid succeeds, so the player is frozen where they
landed rather than standing at an offset from anything). Only then, and only
with the same bounded upward nudge and step a landing already uses
(`UNSTICK_MAX_NUDGE`/`UNSTICK_STEP`), followed by `settle_at_spawn`'s
immediate `categorize_position`. Every boundary that lines up is left
bit-for-bit where the offset put it, which the new tests pin directly.

**Result.** Per-route aboard fraction over `cargo xtask chain-walk` (ticks
with a `func_tracktrain` as the ground brush, over ticks run; the missing
tick in each is the level-change tick itself):

| route | before | after |
|---|---|---|
| 0 | 2309/2310 | 2309/2310 |
| 1 | 2601/2602 | 2601/2602 |
| 2 | 5144/5145 | 5144/5145 |
| 3 | 4015/4016 | **4015/4016** |
| 4 | **0/3261** | **3260/3261** |

The passenger used to arrive in the fifth map beside the ride and stand
there for the whole 54 seconds; they now arrive in their seat and ride it to
the station. The sixth map's arrival, which M9.24 recorded as a hull
embedded in *world* solid (`start_solid`, `all_solid`, `brush_index = None`,
one reachable cell, its `trigger_changelevel` ~5,200 units away and a
vertical scan finding no free standing offset anywhere in the 512 units
above), is gone with it: the passenger now arrives standing on that map's
own parked car, alive, with **eleven** reachable cells and the
`trigger_changelevel` ~630 units away.

**Next.** Depth is unchanged at **6**: no `c0a0-hop5.txt` was authored,
because there is still no honest route out of the sixth map's arrival. The
blocker is no longer placement. The passenger arrives aboard the parked car
and the reachability walk's frontier is the ride itself — a `func_train`
whose bounds are a single door-leaf-shaped brush on a three-node chain, and
the map's own `func_tracktrain`. Nothing fires either in a 150-second wait,
and the map declares no `func_door` at all, so the car's own sliding door
never opens and the passenger cannot step out. That is a
trigger/carry-sequencing question — the ride is handed over already stopped
on its single node, so whatever the map keys its arrival sequence off never
happens — and it is recorded here rather than guessed at.

**Gates**: fmt, clippy (workspace/all-features and no-default),
`cargo test --workspace`, policy, graph, combat-smoke 37/37, campaign-smoke
93/93, `cargo xtask chain-walk` (distinct depth 6, five level changes, no
re-entry).

## M9.26 (Rust): the guard who never arrived, and the door leaf posed at zero

M9.25 got the passenger to the sixth map aboard the parked ride and recorded
the next blocker as a trigger/carry-sequencing question: nothing fired the
car's sliding door (a `func_train` on a three-node chain) in a 150-second
wait. It is a carry question, and this milestone answers it — plus a second,
independent placement fault the answer uncovered. Measured with a local,
uncommitted probe (reverted before committing) of the destination map's
fire-chain dispatcher, its entity-name graph and its posed mover bounds:
classnames, counts, activation order and elapsed seconds only.

**What the map actually does.** In the first 150 seconds of the sixth map,
exactly five activations happen: a `trigger_auto` fires a `multi_manager`,
which fires a second one, which fires a `scripted_sequence` — and there the
chain stops. The door is four links further on: that first
`scripted_sequence`'s own `target` is a `multi_manager` that fires a
`scripted_sentence` and a second `scripted_sequence`, whose `target` is the
`multi_manager` that finally fires the door `func_train` (and the platform
lights, and the announcement). Every link past the first is gated on a
`scripted_sequence` **completing**.

**Why the first one never completed.** All four of that map's
`scripted_sequence`s name the same target monster by `targetname`, and the
map declares **no monster of any classname at all**. The monster they name
is declared by the *previous* map, which is exactly what a level transition
is for: it has to cross. Two separate faults stopped it.

1. **Eligibility.** The documented rule is that an entity travels when it is
   inside a `trigger_transition` volume "or otherwise in the landmark's PVS"
   (`docs/FORMAT_SOURCES.md`, "Campaign flow"). Neither map on that boundary
   declares a transition volume, so this project fell back to
   `DEFAULT_CARRY_RADIUS` — its own documented stand-in "for the PVS test
   this engine does not run at level-change time". It can run it: the
   visibility lump is already decoded for rendering
   (`ohl_world::VisibilitySet`). The monster stands 1,493 units from the
   landmark, so the 512-unit stand-in dropped it. The stand-in now answers
   *only* for the cases a leaf query cannot — a set that could not be
   materialised from the lump at all, a point outside the node tree, and a
   point in leaf `0`, the shared outside/solid leaf that has neither a
   visibility row nor a bit in anyone else's — and never as an `or` over the
   top of a PVS answer of "no".

   **What that widens, measured.** Eligible entities per boundary go
   6 -> 17, 1 -> 11, 5 -> 31, 11 -> 56 and 2 -> 20 across the chain walk's
   five boundaries. 110 entities become eligible only through the PVS path,
   **23 of them brush entities** — so an earlier draft of this section's
   claim that "brush entities keep exactly the rule they had" was wrong, and
   is corrected here. What is true is narrower, and is now enforced: a brush
   entity that becomes eligible this way adds nothing new to what already
   travelled (a *modified* mover's state is captured before the eligibility
   test at all, and an unmodified one applies its own resting state onto a
   same-named counterpart), and it is never materialised. The strict
   fallback is likewise a statement about the rule rather than a behaviour
   change on this payload: measured over all five boundaries, no entity has
   `pvs = false` with `distance <= 512`. `MAX_CARRIED_ENTITIES` (256) still
   bounds the whole set; its truncation is silent and iteration-ordered.

2. **Materialisation.** A carried entity the destination declares no
   counterpart for was already re-created there — but only as a
   `ClassName`, a `Transform` and its carried component snapshot. Every
   later stage of a level's build reads
   `ohl_engine::level::Level::defs` and writes onto the entity at the *same
   index* (`ohl_ai::spawn::attach_monsters`, `AiState::register_brains`,
   `collect_triggers`, `attach_scripts`, `attach_followers`, `nav::build`),
   so an entity with no definition is invisible to all of them: the carried
   monster arrived with no brain, no `Actor` and no hull, and the
   destination's scripts searched for an actor that could not exist.
   `CarriedEntity` now carries the source map's own keyvalues (bounded by
   `MAX_CARRIED_KEYVALUES`), and `TransitionState::place` appends a real
   `EntityDef` alongside the entity it spawns, so a re-created entity is
   built exactly like one the destination declared itself. Its placement is
   rewritten into the destination's coordinates and a `model` keyvalue
   naming a *brush submodel* (`*N`) is dropped, since that index belongs to
   the map that compiled it. A **brush** entity is not materialised at all:
   the cited pages say a brush entity needs "a unique global name to be able
   to be carried over", and that name is how the destination's own copy of
   the brush is found — there is nothing left to create, because a brush
   entity separated from the submodel its own map compiled is nothing. Of
   the 56 entities the chain walk materialises, none is a brush entity:
   `path_track` 31, `multi_manager` 9, `env_message` 7, `ambient_generic` 2,
   `light` 2, `scripted_sequence` 2, and one each of `env_spark`,
   `monster_generic` and `monster_barney`.

3. **Survival.** A materialised entity is appended past the end of the map's
   own entity list, and every index-keyed save section is "one entry per
   registry entity, in spawn order" against a level a load rebuilds from the
   entity lump alone. A quicksave taken after the arrival therefore came
   back with no carried monster and a sequence that could never advance —
   and that map's own opening chain fires a `trigger_autosave`, so it is the
   ordinary case, not a corner. `SECTION_CARRIED_ENTITIES` (**tag 36**, new
   and optional; no frozen section is touched) writes those definitions
   down, and `crate::save_state::restore_carried_entities` rebuilds them
   through the same `materialise_carried` a level change uses — before the
   AI attaches, so a restored monster comes back as a monster rather than as
   the husk this milestone exists to stop being. A save missing tag 36 loads
   exactly as before.

With all of that fixed, the map's own sequence runs: the guard is possessed, walks
his 792 units to the mark, and the chain completes — announcement, platform
lights, and the car's sliding door opening 26.6 seconds after arrival, with
the door's own `path_corner` `message` firing its arrival sound two seconds
later, exactly as authored.

**The door leaf posed at zero.** With the door finally moving, the
reachability walk still reported it as a frontier: the leaf's *pose* was
wrong. `docs/FORMAT_SOURCES.md` already records that this project "turns a
`func_tracktrain` to face its active segment but leaves a `func_train` at
its spawned `angles`" — but `ohl_game::pose::track_train_transform` reported
*no rotation at all* for a non-turning train, which poses it at zero rather
than at its `angles`. That map's door leaf declares a 90-degree yaw, so it
was built across its own doorway instead of in it, and slid to a stop inside
the car. A `func_train`'s `angles` is free to mean an orientation, unlike a
`func_door`'s (whose published meaning is the move direction,
`movedir_from_angles`): a train's direction of travel comes from its
`path_corner` chain, not from a keyvalue. With the leaf posed at its own
`angles` it is no longer a frontier at all.

**Gates**: fmt, clippy (workspace/all-features and no-default),
`cargo test --workspace`, policy, graph, combat-smoke 37/37, campaign-smoke
93/93, `cargo xtask chain-walk`.

**Next.** Depth is still **6**, and no `c0a0-hop5.txt` is authored here.
With the door open and out of the way, the reachability walk from the
arrival point is still enclosed by the *car itself*: its hull is the only
remaining frontier, and the walk finds no opening in it at the heading the
ride arrives with, at the opposite heading, or unrotated. The passenger can
walk the length of the car and no further. That is a separate question — how
the destination's parked copy of a shared ride is posed, and where its own
doorway ends up — recorded here with its measurements rather than guessed
at, and being taken up on its own branch.

Three measurements for whoever takes it, and one hypothesis ruled out.

- **The two bodies are one compiled assembly.** Straight out of the BSP, no
  posing involved: the car's submodel bounds are 288 x 150 x 137 about its
  origin brush and the door leaf's are 57 x 11 x 94 about its own, lying
  strictly inside the car's in x and z with the leaf's *maximum y exactly
  equal to the car's*. The panel is flush with the car's +y face and thin
  (11 units) along that face's own normal, which is only possible if the
  submodel is stored **unrotated** relative to the car. So the leaf's
  `angles` yaw of 90 is a genuine runtime rotation and this milestone's
  `func_train` rule is not double-rotating it — worth ruling out, because a
  submodel that already carried its authored orientation would have produced
  the same symptom.
- **The yaw disagreement is real, and is not the leaf's.** The two `origin`
  keyvalues differ by (-97, +73, +45) and the two path nodes the entities
  are placed at differ by (-72, -101, +53). Neither quantity is touched by
  any posing rule this project applies, and the two agree only if the car is
  turned by **+90** — while the destination map's own chain runs into its
  terminus heading **-90**, which is also the heading the ride arrives with.
  That is a question about how a parked `func_tracktrain`'s heading is
  derived.
- **And a pure-z disagreement no yaw can explain.** Those same two offsets
  disagree by 8 units in z, and the doorway implied by the leaf spans
  z +2..+96 above the car's origin brush while the car's own floor (where
  the arriving passenger stands) is at +43..+65 — an opening ~41 units below
  the floor and only ~31 above it, far short of a standing hull. A rotation
  about Z produces neither figure, so at least part of the remaining
  enclosure is a pivot/reference-point question rather than an angular one.

## M9.27 (Rust): the car was built facing the other way

M9.26 left the arriving passenger sealed inside the parked ride and recorded
the open question honestly: *how* the destination's copy of a shared ride is
posed, and where its own doorway ends up. It also recorded that forcing the
car's heading to +90, -90, 0 and 180 all left the passenger enclosed. That
last observation was the clue: a map that **ends** a shared ride parks its
car on a chain of one node, which defines no heading at all, so
`TrackTrainState::yaw_degrees` never consulted a forced heading — it took the
M9.25 handover fallback every time. The fault was never in the terminus
branch's direction, and the terminus branch is unchanged here: it reports the
direction of travel into the last node, and that direction is right.

**Root cause: the pose, not the heading.** `yaw_degrees` returned the raw
compass heading of the segment, and every consumer — the renderer, the
collision hull, `brush_center`, the rigid rider carry, the cross-level
handover — turned the car's *compiled brushwork* by it. That is only correct
if the brushwork was compiled pointing along `+X`. Three measurements, all
from the maps' own placed poses and keyvalues (no render, no screenshot, no
judgement call), say it was compiled pointing the other way:

1. The station map declares the car's sliding door leaf as a separate brush
   entity on its own path nodes. The leaf's compiled box is a thin panel:
   thinner across one long wall of the car's compiled box than along it, so
   only one of the two candidate car poses gives it a wall to lie flush in.
   That pose is the heading plus half a turn, and it drops the panel into the
   car's own compiled doorway to within a few units on every axis. The other
   puts it through the opposite, solid wall.
2. That map's `info_player_start` lands inside the car directly in front of
   that doorway, facing it, only with the half turn — otherwise at the car's
   far, doorless end, facing away.
3. The campaign's first map stands its player start inside the same compiled
   car, and lands at the same doorway end under the same rule. Its ride also
   begins with the car posed exactly unrotated, which is what building a car
   in place at the head of its own track produces.

`ohl_game::track_train::COMPILED_FACING_OFFSET_DEGREES` records the half
turn; `yaw_degrees` now reports the *pose*, and the new
`travel_heading_degrees` reports the direction of travel, unchanged. A
heading handed across a level change is already a pose and is carried
through untouched, so exactly one half turn is ever applied. Every test that
asserted a yaw now pins the heading and the pose separately, so a future
change cannot let a wrong heading and a wrong offset cancel out.

**This supersedes M9.18's "which heading" finding.** That round chose between
the two conventions by rendering a tram interior each way and comparing
against public screenshots. The comparison had a confound: a passenger is
carried by the car's pose, so turning the car half a turn moves the capture
camera to the mirrored seat *and* turns it to face the mirrored way. Both
builds render "an interior seen from one end", and picking between them
against a promotional shot whose vantage is not the map's own spawn point is
a judgement, not a measurement. `docs/FORMAT_SOURCES.md` keeps both records,
append-only.

**Result.** The passenger is no longer sealed in.
`xtask/chain-routes/c0a0-hop5.txt` waits out the map's own arrival sequence,
steps out of the car onto the platform, follows the corridor to the door at
its end — which the map's own guard, not a `use` press, opens — and crosses
the level boundary beyond it. Chain-walk distinct depth is **7** (6 level
changes, no re-entry, not frozen); removing the half turn drops it back to
**6**, with the passenger unable to leave the car. That is the measurement.

The reachability walk is *not*, and an earlier draft of this entry claimed
it was. It reports the same **75** cells from the arrival point with the
half turn as without — its floor-support edge will not cross the sill
between the car and the platform that a real player steps over — and the
**7,187** figure belongs to a later point in the route, out on the platform,
where it still lists the car as a frontier (a car is a wall on every side
but its doorway). The walk under-reports here; it neither confirmed nor
could have confirmed this fix.

**The z span was the same fault.** M9.26 recorded "a pure-z disagreement no
yaw can explain": the doorway implied by the leaf spans z +2..+96 above the
car's origin brush while the arriving passenger stood at +43..+65. The
opening was never wrong; the *passenger* was. Standing in a car posed half a
turn out, they were held up by the wrong part of its interior. With the pose
right they settle at +43, feet at +7 — on the doorway's own sill — and walk
straight out of it.

What remains is M9.26's own **8**-unit figure: the z difference between the
two entities' `origin` keyvalues and the z difference between the two path
nodes they are placed at disagree by that much, so the leaf hangs 8 units
above where a rigid assembly with the car would put it. (Comparing the
leaf's placed *centre* against that same authored *origin* offset gives 12
instead, but those are two different reference points on the leaf — the
extra 4 is just its own compiled centre-above-origin, present in both
frames and not a discrepancy.) Either way it is real, too small to block a
standing hull, and still not something a rotation about the world up axis
could have produced.

**Still open.** That 8-unit leaf offset, and the map's other
`trigger_changelevel`, which loads with a 2-unit-tall bounding box — not a
plausible authored trigger volume, so more likely a submodel-bounds gap than
a map fact. Neither blocks the route: the leaf clears the doorway, and the
boundary this route crosses fires normally. Recorded rather than guessed at.

**Gates**: fmt, clippy (workspace/all-features and no-default),
`cargo test --workspace`, policy, graph, combat-smoke 37/37, campaign-smoke
93/93, `cargo xtask chain-walk` at depth 7.
