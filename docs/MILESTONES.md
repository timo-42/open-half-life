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

Status: in progress; packages 2–4 establish the capability, cache,
planning/staging, VFS, and application-composition feature baseline at
`df5ea6d51037671ef0165dacac9fe26df1bf4d2b`. Disconnected parser-result
validation was accepted at `909edcc`, followed by portable media cancellation
accepted at `0f2c78d`, trusted parser source reads at `c90f2d1`, and the
disconnected frame channel at `e4b819a`. The trusted parent handshake was
accepted at `13f0fb0`, and the disconnected trusted parent session was accepted
at `7bd9d38`.

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

Remaining M2 work:

- deterministic component selection must precede final layout planning;
  edition-specific selection data may be supplied only through a runtime-only
  local recipe, and any project-owned selection parser requires recorded public
  format provenance
- the constrained parser worker boundary in `MEDIA_IMPORT.md` remains
  mandatory before any third-party parser may feed production extraction. The
  accepted result bridge, source-read broker, frame channel, parent handshake,
  and parent session still need native isolated-worker launch, ownership,
  termination, and reap plus explicit runtime composition, deterministic
  component selection, and staging integration; the worker must have no
  raw-path, destination, or cache authority. Although the abstract
  `IsolatedWorker` lifecycle facade exists, committed HEAD selects only the
  backend returning `unsupported`. The gate is blocked on a successful native
  backend, media-parser worker executable/bootstrap/service loop, and a higher
  process-session owner. That owner must allocate unique session IDs and worker
  epochs, keep the channel alive, close plus `wait()`/reap after orderly
  shutdown, and use `terminate_and_wait()` for failure or orderly-close timeout
- after those lifecycle pieces, resume with handshake/parent-session
  composition, deterministic component selection, and only then staging and
  publication
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
the platform common-worker test at 1/1. These local results cover the trusted
operation table, exact header/payload transfers, validation ordering, session
binding, caller buffer/view lifetimes, concurrency, terminal poisoning,
sanitized transfer errors, and abort wakeup. No hosted result is claimed for
this commit.

The trusted parent handshake was accepted and pushed at
`13f0fb08e7d00159000f3721ebe0b0e1b1481188`. Its local clean
warnings-as-errors build passed 87/87 steps and the full local CTest suite
passed 35/35. Synthetic evidence covers independent hello/header bytes,
exact-empty ready, observation order, deadline/token identity, exact policy and
limits, proof moves and single extraction, downstream session/broker
construction, every public pre-I/O rejection class, caller-buffer invalidation,
sanitized terminal failure, one abort, and no escaped proof or view. No hosted
result is claimed for `13f0fb0`.

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
limitations. No hosted result is claimed for `7bd9d38`.

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

The abstract `IsolatedWorker` facade already supplies lifecycle operations, but
the committed backend returns `unsupported`. Remaining gates are a successful
native backend; the media-parser worker executable, bootstrap, and service loop;
a higher owner for session-ID and worker-epoch uniqueness, channel/session
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
transport, parent-handshake, and parent-session stack supports active M2 work
but is not a production import path.
The result bridge owns
catalog generation, promotion, membership, layout, stream remainder, and
retirement; the read broker owns bounded reads from the retained pinned
capability and their prepare/commit ordering; the frame channel owns only
bounded framing over a caller-supplied byte capability; the handshake proves
only the typed transition and exact broker policy binding; the parent session
owns the guarded composition and transaction state above those pieces. No
runtime target
links these libraries. The frame channel and handshake do not launch or own a
worker or sandbox, accept a source path, read source bytes, select a component,
stage or publish data, or grant runtime/application authority. They have no
process termination or reap authority. The frame channel also accepts no
executable, path, source, component selection, catalog, staging, destination,
publication, cache, or application authority; the result and read bridges also
create no worker or runtime import path and own no staging or publication.
Native isolated-worker process
management and runtime composition remain a later dependency. This work
authorizes no proprietary extraction.

## Later milestones

- M3: BSP rendering
- M4: player movement
- M5: interactive entities
- M6: models and animation
- M7: combat
- M8: full campaign compatibility
- M9: release hardening
