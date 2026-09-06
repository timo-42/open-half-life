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

  Not yet done: M7.9 P4b (the five additive save sections in
  `ohl-engine::save`/`save_state`) and, once M7.9 P1 lands, wiring the four
  TODO(P1) milestone lines above.

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
  F2).** `trigger_auto` itself is implemented (M7.11 wires it into the
  scripted-sequence "use"/target-firing path), but `trigger_camera` — the
  entity that actually takes over the view for a scripted intro shot — has
  no implementation yet.
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
