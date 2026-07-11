# Architecture

Open Half-Life is split into narrow C++ libraries. Dependencies point from
high-level orchestration toward low-level services; cyclic module dependencies
are not allowed.

The intended module set is:

- `core`: logging, diagnostics, common utilities
- `platform`: operating-system and architecture abstraction
- `parser`: accepted bounded protocol infrastructure; not a worker runtime
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

The `core`, `parser`, `platform`, `media`, `vfs`, and `app` modules now exist.
New modules must expose public headers beneath `include/ohl/<module>` and keep
implementation details in `src`. Third-party API types should not leak across
module interfaces.

The current dependency direction is:

```text
app -> core + platform + media + vfs
media -> platform + standard library; core is a private implementation edge
media_parser_results -> media + parser; disconnected from runtime targets
media_parser_reads -> media_parser_results; disconnected from runtime targets
media_parser_transport -> parser + platform + Threads; disconnected from runtime targets
media_parser_handshake -> media_parser_reads + media_parser_transport; disconnected from runtime targets
media_parser_parent_session -> media_parser_handshake; disconnected from runtime targets
vfs -> platform + standard library; libudfread is a private implementation edge
parser -> standard library
core/platform -> standard library

experimental media cabinet adapter -> vfs + Unshield + zlib
```

The current `parser` target is deliberately isolated: no runtime target depends
on it, and its only allowed dependency edge is toward the standard library. Its
accepted OWP/1 protocol layer provides canonical bounded framing and headers,
generic bounded primitive payload readers and writers, per-frame and cumulative
message/payload budgets, and fail-closed session ordering. Accepted typed
schemas now cover `hello`, exact-empty `ready`, `enumerate`, `stream_entry`,
`read_request`, `read_reply`, `entry_batch`, `data_chunk`, `complete`, `cancel`,
`cancel_ack`, and `shutdown`: all twelve OWP/1 message families. The
`stream_entry` payload is exactly one canonical 8-byte
little-endian `source_token`. It is an opaque project-owned identifier; zero
and every other `uint64_t` value, including the all-ones value, are valid at
this codec boundary. The codec alone establishes neither token membership nor
source authority. The disconnected trusted result bridge described below owns
membership only after complete catalog validation.

The `entry_batch` wire payload begins with a canonical little-endian `u16`
entry count from 1 through 256. Each record then contains, in order, a
little-endian `u64 source_token`, `u64 size_bytes`, `u16 archive_path_length`
from 1 through 4,096, and exactly that many printable ASCII bytes (`0x20`
through `0x7e`). The generic 1 MiB frame ceiling still applies. Its trusted
cumulative policy permits at most 50,000 remaining entries, 64 MiB of remaining
path bytes, 8 GiB for any entry, and 32 GiB of remaining declared data; callers
may tighten but not raise those ceilings. Tokens must increase strictly within
and across batches relative to the caller's previous-token context. Zero is a
valid first candidate, but ordering alone grants no membership or authority.

The entry-batch decoder performs an allocation-free validation pass over the
entire payload and policy before checking capacity and populating caller-owned
entry storage in a second pass. The decoded entry span aliases that storage,
while each path view aliases the frame payload; both backing stores must remain
alive and unchanged while those views are used. Printable archive spellings,
including traversal-like or absolute-looking text, remain untrusted spellings,
not normalized destination paths. An empty batch is noncanonical; an empty
enumeration is represented by a valid success-only `complete` with no preceding
batch.

A `data_chunk` is the opaque whole payload with no prefix, offset, token, or
status field; its accepted size is 1 byte through 256 KiB, so a zero-byte chunk
is noncanonical. Its typed codec also takes a trusted nonzero
`remaining_entry_bytes` context and rejects a chunk larger than that bound.
Decoding returns a non-owning span that aliases the frame payload, whose
storage must remain alive and unchanged while the span is used. The caller,
not the codec, owns remainder accounting and may decrement it only after the
accepted bytes have been written downstream. The decoders validate the
complete frame and payload shape, require full payload consumption, and enforce
the applicable source-size, read-size, range, sequence, status, reply-data,
chunk-size, and remaining-entry bounds. The success-only `complete` payload is
exactly four canonical little-endian bytes: a `u16 ProtocolStatus` followed by
a `u16 ProtocolPhase` (`00 00 04 00` for the sole accepted pair). Its trusted
expected-operation context must be `enumerate` or `stream`, and the only
accepted wire pair in either context is `(ok, complete)`. Every other known
pair and every pair containing an unknown value is rejected. Failure-result
representation remains deferred, and the message grants no worker failure,
destination, or publication authority. Typed coverage does not implement or
authorize worker creation, process isolation, source access, component
selection, trusted catalog membership, payload extraction, destination
mutation, or cache publication.

The accepted session-ordering contract handles duplex cancellation races
without granting message content any trust. Completion wins when `complete` and
`cancel` cross, provided no read remains unresolved; one immediately late
same-request `cancel` is then consumed as stale without `cancel_ack`. While
cancellation is pending, only bounded same-request crossing traffic is
accepted. A read first observed after cancellation cannot be serviced and does
not open a drain allowance. If `cancel_ack` overtakes the already-enqueued reply
for a read that was outstanding before cancellation, the cancelled session may
consume exactly one same-request parent-to-worker `read_reply` solely to drain
the transport. A wrong identifier or direction, a duplicate, shutdown, or any
terminal failure rejects or closes that one-shot allowance.

Header validity and session ordering are not sufficient to trust a message.
Before any state transition or use of message content, a message-specific typed
decoder must apply explicit bounds to every payload field, count, and length,
reject noncanonical values, and require complete payload consumption. The
twelve accepted decoders provide that validation for `hello`, `ready`,
`enumerate`, `stream_entry`, `read_request`, `read_reply`, `entry_batch`,
`data_chunk`, `complete`, `cancel`, `cancel_ack`, and `shutdown`.

Commit `909edcc` adds `OpenHalfLife::media_parser_results`, a trusted but
deliberately disconnected receiver for those validated results. A caller gives
each session a nonzero worker epoch that must be unique while an old catalog
handle could remain reachable. Each enumeration adds a session-local sequence
to that epoch. Entry batches remain candidates while their cumulative quotas
and strictly increasing tokens are checked; archive-path views are copied into
owned strings before frame storage can disappear. A successful `complete`
runs the existing payload-layout planner over the whole candidate, thereby
normalizing paths, applying aggregate counts and sizes, and rejecting unsafe or
conflicting layouts before atomically promoting a catalog. Promotion builds a
sorted token index, and a later stream must present both the exact
epoch/sequence generation and a token in that index. A restarted worker cannot
reuse an old generation merely by reusing its enumeration number and token.

For an authorized stream, the bridge initializes a trusted remainder from the
catalog entry, rejects chunks larger than that remainder, writes through the
provided sink, and decrements only after the sink accepts the whole chunk.
Stream completion requires a zero remainder. Cancellation immediately removes
catalog authority and prevents a candidate from being promoted while retaining
only the bounded state needed to validate already-crossing same-request frames.
A post-cancel read request is validated and ordered but returned as ignored
with no actionable read metadata; only a read already outstanding before
cancellation may receive its crossing reply. A new enumeration, cancellation
acknowledgement, shutdown, protocol or layout failure, downstream failure,
source invalidation, worker failure, or destruction retires the applicable
catalog, candidate, and stream bindings.

The result bridge itself owns no transport, worker process, native sandbox,
component-selection recipe, staging transaction, or publication authority.
Neither `app`, `ohl_media`, nor `stage_payload` links it.

Commit `c90f2d1` adds the separate, disconnected
`OpenHalfLife::media_parser_reads` library. Its only project dependency is the
trusted parser-result/session stack. Construction retains the exact shared,
pinned `MediaSource` capability carried by a valid `ValidatedMedia`; source
size comes from that proof's fingerprint and is checked against the retained
capability. `maximum_read_bytes` is separate trusted constructor configuration
and must exactly match the value in the accepted typed `hello`. The broker
alone cannot verify that handshake binding. The accepted parent-handshake proof
below records the source-size and maximum-read values, not media identity.
Trusted later composition must use that proof's exact limits and the same
`ValidatedMedia`. The broker accepts no path or replacement source. Each
`read_request` is decoded through the typed schema before session observation.
It owns the first/subsequent sequence for each request identifier, resets it for
a new request, and enforces independent request-count and cumulative
reply-payload budgets before source access.

For a serviceable request, the source-read broker checks the retained
capability for change before the read and again after either success or
failure. It maps stable generic read errors to `source_read_failed` and observed
mutation, range loss, or early EOF to `source_changed`. Replies are canonical
and bounded: success carries exactly the requested bytes after the fixed
prefix, while either source failure carries the prefix only. The temporary
read scratch is scrubbed after encoding, including bytes written by a partial
failed read. Prepared reply storage remains caller-owned and borrowed under a
unique ticket; only `commit_reply_sent()` after full transport acceptance
observes the reply in the session and advances sequencing. A stale ticket,
partial/failed delivery through `abandon_reply()`, source failure commit, or
broker destruction during an active session retires the broker and result
session terminally, including any catalog authority.

Cancellation preserves the session's duplex ordering: a reply prepared before
cancel may cross, including the one drain allowed when `cancel_ack` overtakes
it, while a read first seen after cancel is ignored without source access,
budget charge, output mutation, or ticket. The optional operation table exists
only as a trusted deterministic-test seam: the all-null value selects native
operations and a supplied table must be complete. The broker passes the same
retained capability as the callback source argument, but cannot constrain a
callback's ambient process authority; only trusted project/test code may supply
callbacks, and worker or media input must never configure them.

The broker creates no worker, sandbox, or transport and sends no frame. It has
no runtime-import, raw-path, component-selection, destination, staging, cache,
or publication authority. Native worker construction/isolation, IPC delivery,
and explicit composition with selection and staging remain prerequisites.

Commit `e4b819a` adds the separate, disconnected
`OpenHalfLife::media_parser_transport` library. Its only dependencies are
`OpenHalfLife::parser`, `OpenHalfLife::platform`, and `Threads::Threads`; no
runtime target links it. A caller supplies a nonzero session identifier and a
complete, trusted, non-owning exact-I/O operation table. The table's context
and underlying byte channel must outlive the frame channel and every active
operation. An existing adapter constructs an operation table whose callbacks
directly forward to an already-created `platform::IsolatedWorker` through
`read_exact()`, `write_all()`, and `abort_io()`. It grants no process launch,
ownership, termination, or reap authority.

Each frame is transferred as one exact canonical 32-byte OWP/1 header followed
by a separate exact payload transfer when the bounded payload is nonempty.
The channel passes the caller's deadline and cancellation token unchanged to
each header and payload operation. It allocates and owns neither payload
buffer: an outgoing payload must remain alive and unchanged until `send()`
returns, while a successful received frame view aliases caller-owned storage
that must remain alive and unchanged while the view is used. Receive requires
capacity for the protocol maximum before consuming a header. Once payload I/O
has begun, a failed read may leave an untrusted partial prefix followed by
stale bytes; the entire supplied buffer is invalid as a frame and no portion
may be parsed until the caller reinitializes it.

Validation precedes the operation that could consume or emit the corresponding
bytes. Construction rejects a zero session or incomplete operation table.
Send validates the header, exact session binding, payload ceiling, and declared
length before header encoding or I/O. Receive checks caller capacity before
header I/O, then decodes and validates the complete header and exact session
before selecting the bounded payload span. Invalid configuration is terminal
without I/O. Protocol and transport failures poison the channel terminally,
retain the first failure, invoke the operation table's idempotent `abort_io()`
once, and make later calls return the retained failure without further I/O.
Impossible exact-I/O reports, including a successful short transfer or a count
above the requested span, are sanitized to `io_failure`; no partial byte count
or frame view is returned to the caller.

At most one send and one receive may be active concurrently. A second operation
in the same direction is rejected without I/O and does not by itself poison the
channel. The exact-I/O capability must therefore support one active read and
one active write, and its abort operation must be concurrency-safe, must
promptly wake either or both directions, and must not re-enter or destroy the
frame channel. Destruction may not race active operations. The future owner is
responsible for orderly channel close followed by `wait()`/reap; only failure
or orderly-close timeout uses `terminate_and_wait()` to terminate and reap.

Terminal `abort_io()` interrupts the trusted byte channel; it is not process
termination or reap authority. Trusted custom callback code retains its ambient
process authority, so restricting callback suppliers is composition policy,
not mechanical confinement. The transport otherwise has no process launch,
ownership, termination, reap, executable or path selection, source-read,
component-selection, catalog, destination, staging, publication, cache,
application, or runtime-import authority. Linux x86-64 native isolated-worker
containment now exists as a disconnected source-selected backend. Parser-worker
process management plus composition with the result bridge, source-read broker,
selection policy, staging, and the application remain later dependencies.

Commit `13f0fb0` adds the disconnected
`OpenHalfLife::media_parser_handshake` library. Its direct dependencies are
`OpenHalfLife::media_parser_reads` and
`OpenHalfLife::media_parser_transport`; no runtime target links it. The caller
gives it exclusive access to a fresh frame channel for one synchronous parent
`hello` / worker `ready` exchange. The channel, `ValidatedMedia`, and receive
storage are borrowed for the call. Source-read limits, protocol budgets, and
deadline are copied values; the copied cancellation token shares its source's
state.

Through the borrowed `ValidatedMedia`, the handshake temporarily receives its
pinned source capability only to query the source's captured size. It does not
read source bytes, and neither the handshake nor its proof retains or grants
that capability.

Before any handshake I/O, the function rejects a terminal channel, invalid or
moved-from media, a missing pinned source, disagreement between the pinned
source's captured size and the validated fingerprint, invalid source-read
limits or protocol budgets, budgets below two messages or 12 payload bytes,
receive storage smaller than the protocol maximum, and invalid derived source
policy or validator configuration. These pre-I/O failures return no proof and
do not abort or otherwise use a nonterminal channel.

The parent sends a canonical `hello` with request zero and the frame channel's
nonzero session. Its exact 12-byte payload binds the validated fingerprint's
source size as a little-endian `u64` and the trusted
`maximum_read_bytes` limit as a little-endian `u32`. Only after the complete
frame send succeeds does the handshake observe that parent-to-worker header in
its validator. It then receives a frame, decodes an exact-empty typed `ready`,
and only after typed acceptance observes the worker-to-parent header. The same
caller deadline and cancellation token are passed unchanged to both channel
calls.

Success requires the validator to be exactly idle with two messages and 12
payload bytes charged. The returned proof is move-only and single-consumption:
it retains that validator plus the exact source-read limits and derived source
policy, and `take_protocol()` invalidates the proof and its containing result.
Later trusted composition must move that validator into a
`ParserResultSession`, then create the `ParserSourceReadBroker` from the same
`ValidatedMedia` and exact limits. The proof retains copies of the limits and
derived policy, but no media identity: it does not mechanically prove that the
later broker receives the same media. That is a trusted composition
requirement. The proof itself owns no source or process capability.
It does borrow the exact channel identity: that channel must outlive the proof
until the proof is consumed or discarded. After successful factory
consumption, the same channel must instead outlive the created parent session
and all of its active calls.

Receive storage is never scrubbed and no frame or payload view escapes. Once
payload I/O begins, or if typed-ready validation fails, an attacker-controlled
partial or full prefix may be followed by stale prior bytes. The entire buffer
is untrusted and invalid as a frame until the caller reinitializes it. A
protocol or channel failure after interaction returns no proof and terminally
aborts the channel; the channel retains its first cause, sanitizes impossible
exact-I/O reports, and permits no later I/O.

The handshake accepts no source path and grants no source-byte, process launch,
process ownership, termination, reap, component-selection, staging,
publication, cache, runtime-import, or application authority. It neither
creates a worker nor composes the downstream session, broker, selection,
staging, or application. Those remain explicit responsibilities of later
trusted native composition.

Commit `7bd9d38` adds the disconnected
`OpenHalfLife::media_parser_parent_session` library. Its only direct project
dependency is `OpenHalfLife::media_parser_handshake`; no application or runtime
target links it. The factory consumes a valid move-only handshake proof only
after validating the exact borrowed frame-channel object, a nonterminal
channel, the supplied `ValidatedMedia`, the media fingerprint's captured size,
the proof's exact source-read policy and limits, a nonzero worker epoch, and
bounded import limits. A distinct channel with the same session identifier is
rejected because the proof binds object identity as well as the identifier.
Rejected configuration leaves the proof unconsumed. The proof records source
size and maximum-read policy, not media identity, so the factory's requirement
that trusted composition supply the same `ValidatedMedia` used for the
handshake remains a caller obligation rather than a mechanical proof.

The parent session owns the result session, source-read broker, monotonic
nonzero request identifiers, active operation, reply-ticket sequencing,
outbound transaction arbitration, cancellation state, and first terminal
result. It borrows the exact frame channel for its complete lifetime; the
channel and its exact-I/O context must outlive the session and all calls, and
the caller must not perform direct protocol I/O through that channel while the
session exists. A stream sink is retained from a successfully sent stream
request through stream completion, cancellation acknowledgement, or terminal
failure. The sink must outlive that interval, remain synchronous and
nonthrowing, and neither re-enter nor destroy the session. Destruction may not
race any active call. Destroying an open nonterminal session retires result
authority and aborts the channel; destruction after an orderly `shutdown`
does not abort it.

Enumeration and streaming stage the result-session transition, then send with
the parent-session mutex released. The session maintains an explicit in-flight
transaction for enumerate, stream, read reply, cancel, or shutdown so external
write and abort callbacks never run under that mutex. Competing mutations are
rejected with `concurrent_operation` and perform no I/O. During any staged
outbound transaction, the read-only `state()` and `result()` surface the last
committed wrapper state and `catalog()` is deliberately hidden. Frame-channel
callbacks may re-enter only `terminal()`, `state()`, `result()`, and
`catalog()`; mutating calls, lifecycle notifications, and destruction are
prohibited. Sink callbacks may not re-enter at all.

`receive_one()` is available only while enumerating, streaming, or cancelling,
which prevents an idle worker from pre-sending a guessed next-request result.
The channel receive occurs outside the transaction mutex, allowing cancel to
cross a blocked receive. After receive returns, consumption waits for any
outbound transaction to commit, so completion may win a completion/cancel
crossing under the established protocol ordering. Entry batches advance
enumeration, completion atomically promotes a catalog and returns to idle,
data chunks are synchronously forwarded to the retained sink, and stream
completion requires an exact zero remainder. A rejecting sink may have made
caller-owned partial effects before the session becomes terminal.

A typed read request is passed to the owned broker. A serviceable request
prepares a canonical reply in caller storage under a unique ticket; the parent
commits that ticket only after the exact frame send succeeds, and abandons it
after partial or failed delivery. A read first seen after cancellation is
ignored without source or reply-buffer access. Parent-level arbitration does
not allow a staged read reply and cancel to overtake one another: once the
read-reply transaction is staged, `request_cancel()` returns
`concurrent_operation` without I/O; if cancel stages first, the returning
receive waits for it to commit and the broker ignores the newly observed read.
The lower result/broker one-shot drain remains part of their independent
contract, but this parent-session arbitration cannot produce a `cancel_ack`
overtaking its already-staged read reply. The factory accepts `ValidatedMedia`
and the owned broker retains its pinned source capability, with reads bounded
by the exact policy accepted in the handshake. After construction, the parent
accepts no raw path or replacement source capability.

Before any receive I/O, the parent requires caller-owned receive storage for
the protocol maximum, scratch storage for `maximum_read_bytes`, and reply
storage for the fixed read-reply prefix plus that maximum. All three must be
nonnull and pairwise disjoint. No frame, payload, or scratch view escapes.
Failed receive I/O may leave attacker-controlled bytes followed by a stale
suffix, so the whole receive buffer is invalid as a frame until reinitialized.
The broker scrubs the scratch prefix actually used for a read; an unused suffix
may remain stale. Reply storage is not scrubbed and may retain private source
bytes after either success or failure, so its owner must sanitize it before
logging or unrelated reuse.

`notify_worker_failed()` and `invalidate_source()` claim the first terminal
cause while holding the mutex, clear catalog and stream authority, then abort
the channel after unlocking. This promptly wakes blocked reads or writes and
prevents a returning operation from replacing the retained cause. Protocol,
channel, result, source, worker, source-invalidation, allocation, internal,
request-ID exhaustion, invalid-state/configuration, small-output, overlap, and
concurrent-operation outcomes remain sanitized project errors. `shutdown` is
accepted only from idle or cancelled state with no active receive or outbound
transaction; it closes protocol state but does not close, terminate, wait for,
or reap the borrowed worker or channel.

This composition has no executable, service, path, or component-selection
authority; no worker/process launch, ownership, termination, or reap
authority; and no destination, staging, cache-publication, application, or
runtime-import authority. The abstract `platform::IsolatedWorker` facade
already defines launch, exact I/O, abort/close, wait, and terminate-and-wait
lifecycle operations, and committed HEAD source-selects a native containment
backend only for Linux x86-64. Other platforms and Linux architectures select
the unsupported backend. Production composition is still missing the
media-parser worker executable, bootstrap and service loop, install rule,
runtime selection, staging/publication integration, and the higher
process-session owner. That owner must allocate fresh protocol session IDs and
worker epochs under an explicit uniqueness policy, keep the exact channel alive
through handshake-proof consumption and the parent session, close the channel
and `wait()`/reap after orderly protocol shutdown, and use
`terminate_and_wait()` only for failure or orderly-close timeout paths.
ParentSession owns none of those lifecycle actions.

Work resumes in dependency order: qualify the Linux x86-64 native backend or
add another tuple's native backend together with the worker/bootstrap/service
loop; add the process-session owner and session-ID / epoch policy; compose
handshake and ParentSession; then integrate a deterministic component-selection
recipe before staging and publication. No further coherent disconnected
parent-session package removes those blockers. Its tests are project-authored
and synthetic and authorize no proprietary extraction.

Deterministic parser fuzz validation was accepted at `81a7ee9`; its typed
dispatch was extended at `d59b6c5`, for `stream_entry` at `f4d908a`, for
`data_chunk` at `c28ea9f`, for `complete` at `2d71079`, and for `entry_batch`
at `ba84cfc`. The opt-in
libFuzzer target exercises bounded frame decoding, generic payload reading,
session ordering, and all twelve accepted typed decoders. Read-message dispatch
uses bounded matching and deliberately mismatching contexts. Entry-batch
dispatch uses fixed storage for 256 entries and bounded broad, matching-token,
replay-token, and reduced-budget policies. Its deterministic self-check covers
canonical and matching-token batches plus replay, non-printable, and budget
rejection. Data-chunk
dispatch selects bounded, independently reachable contexts for the exact
payload remainder, a smaller remainder, and zero remainder without allocating
or copying the frame payload. Complete dispatch selects both valid operation
contexts and an invalid context while arbitrary payloads reach disallowed
pairs. Its deterministic self-check proves both valid contexts, disallowed
status and phase pairs, and the invalid context. Unit validation exhausts all
ten known statuses by all five known phases in both valid contexts and checks
that typed rejection occurs before state observation. The hosted smoke job
replays the fixed project-authored synthetic corpus twice and verifies that the
seeds are not mutated. This fuzz evidence validates the protocol
infrastructure; the trusted bridge has separate synthetic unit coverage, and
neither result is evidence of worker transport, native isolation, or runtime
wiring.

There is intentionally no `vfs -> media` edge. Both modules consume the same
low-level `platform::MediaSource` capability, while `app` is the composition
root that decides when a structurally validated source may be mounted or
cached. The default `media -> platform` and `vfs -> platform` edges therefore
do not form a cycle. Enabling the default-off experimental adapter adds the
one-way `media -> vfs` edge shown above.

## Pinned media capability and content validation

`platform::MediaSource` is a read-only capability for one natively opened file
object. Path resolution occurs only during acquisition; sharing the capability
shares the same pinned native identity, and positional reads never reopen the
path. Identity pinning prevents a later pathname replacement from retargeting
the source. It does not make the underlying bytes immutable: an external
writer may still mutate the opened object. Phase boundaries therefore call
`verify_unchanged()` to compare native identity, type, size, and available
content-change indicators with the acquisition snapshot.

`media::ValidatedMedia` binds that same capability to a bounded structural
inspection and full SHA-256 fingerprint. It is evidence that the source passed
those gates at validation time, not a promise that future content cannot
change. Before metadata-only cache publication, `media` verifies the pinned
source again, rehashes it end to end, requires the digest to equal the
validation fingerprint, and publishes nothing on mismatch or read failure.
Source paths and media bytes are not persisted.

The metadata cache uses the verified digest as its source-directory identity.
Current standard-library checks require an absolute cache root and reject
observed symbolic links or non-directory components. Cache preparation writes
the metadata-only manifest to a same-directory temporary file, then publishes
it with an atomic hard-link insertion that never replaces an existing
destination. If another writer wins that race, the existing regular manifest
is opened through the pinned no-follow source boundary and reused only when its
complete contents match exactly; a mismatch is a manifest conflict. Filesystems
without usable hard links fail publication safely. The standard-library
directory-component checks are not a fully pinned native traversal; native
directory handling remains separate hardening work for payload import.

The application acquires the source once, discards the selected path, validates
that capability, mounts `validated.source()` through `vfs`, and passes the same
`ValidatedMedia` to cache preparation. It never reopens the original path.
This acquire-once flow is the accepted composition for the current startup;
`app` remains the only composition root.

## Bounded read-only VFS

The `vfs` target wraps libudfread behind C++ pImpl types, so third-party API
types do not leak into the engine. It retains the pinned `MediaSource` for the
mount lifetime, checks source stability around operations, serializes
third-party access, and exposes normalized read-only paths, seekable files, and
explicitly shared read-only mount handles.

Directory enumeration is bounded and opaque. `list_page()` returns entries in
provider order plus a move-only `DirectoryCursor` only when continuation is
available. `continue_list()` consumes a cursor; cursors are valid only for the
same mounted state (including its explicit shares), and foreign, stale,
default, moved-from, or already consumed cursors fail without partial output.
Errors and source changes return an empty, tokenless page. The compatibility
`list()` API consumes the same pages internally and returns either the complete
listing or an error with no entries.

The package-4 directory ceilings are fixed upper bounds: 64 normalized path
components; 256 entries, 64 KiB of names, 96 KiB of logical result data, and
1,024 provider-work units per page; and 64 pages plus 65,536 provider-work
units per cursor. Callers may lower these limits before mount, but zero or
raised limits are rejected. Dot entries count as provider work, page assembly
uses checked arithmetic, and no native host path is exposed.

The experimental parser receives callbacks backed by a shared VFS handle,
allowing nested containers to be investigated without copying them out of the
image or borrowing the caller's lifetime. Its bounded adapter output reports
invalid descriptors rather than silently omitting them. This integration is
not production extraction: the parser remains default-off and must run behind
the constrained worker protocol in `MEDIA_IMPORT.md` before it may supply
production import data.

## Media import planning and staging

The always-built `media` path and layout policy is independent of Unshield. It
normalizes archive-controlled names to a strict printable-ASCII subset,
creates deterministic case-folded keys, rejects portable path conflicts, and
applies metadata and declared-size quotas before any destination is opened.
Those path and layout checks are lexical and planning checks only. The
platform-independent streaming boundary gives every `PayloadSource` the exact
pinned `MediaSource` from the accepted `ValidatedMedia`, the planned entry's
opaque token, and the staging `CancellationToken`. It wraps the caller's byte sink,
checks cancellation before source dispatch, before each sink write, and after
source return, rejects chunks that exceed the declared size before they reach
that sink, requires an exact final byte count, and reports source, destination,
overflow, underflow, and cancellation failures separately.

Commit `0f2c78d` replaces media's standard-library stop types with media-owned
`CancellationToken` and `CancellationSource` value types. Their API follows
standard-like cooperative polling semantics: sources share one atomic state;
tokens and sources are copyable and equality compares shared identity;
`request_stop()` is non-throwing and returns true only for the first request;
and consumers poll `stop_requested()` at the existing boundaries. A default
token has no state. An unstopped token reports that stopping is no longer
possible after its final source is destroyed, while a requested token retains
its requested state. This removes the AppleClang 17 libc++ dependency on
experimental `std::stop_token` support without adding an experimental ABI flag
or changing the media dependency graph. It is a common portability correction,
not a native macOS backend. Hosted AppleClang 17 coverage for the replacement
is recorded below.

The platform-independent `stage_payload` orchestrator now requires
`ValidatedMedia`; its request no longer accepts a caller-supplied source
identity. Its local `ohl-payload-v2-sha256` stage identity binds the accepted
whole-source size and SHA-256, a non-empty trusted recipe identity bounded to
4,096 bytes, and the normalized layout's entry count, declared total, paths,
and declared sizes. Transport-local source tokens are deliberately excluded.
The complete plan is validated before the first injected-store call. Staging
then streams and seals each payload file, seals completion metadata, reverifies
the complete pinned source against the accepted size and SHA-256, and performs
a final cancellation check. After that final check, `publish_no_replace()` is
the next store operation. A verification or cancellation failure before
publication either occurs before a transaction exists or aborts the owned
transaction, and publishes nothing. The orchestrator also models cache hits,
conflicts, no-replace publication races, explicit cleanup, and whether the
backend's post-publication parent-sync operation completed or failed. Cleanup
failures are surfaced and may leave the transaction's owned private staging in
place. A completed sync is not presented as a universal durability guarantee.
These accepted boundaries perform no runtime extraction: production extraction
remains absent and still requires accepted native backend qualification for each
supported platform tuple.
The component-based store is tested with a deterministic in-memory fake and a
Linux implementation. On Linux, an existing absolute root is walked from `/`
through no-follow directory descriptors, ownership and mode are checked, and
ext-family filesystems, XFS, Btrfs, and tmpfs are the only accepted filesystem
types.
The ext-family admission uses the shared `0xEF53` statfs magic and therefore
cannot distinguish ext2, ext3, and ext4. Native qualification currently covers
ext4 and tmpfs only; ext2, ext3, XFS, and Btrfs remain unqualified.
Private create-new staging is populated below `files/`, an exact binary marker
is synced last, and `renameat2(RENAME_NOREPLACE)` is the only publication
operation. Each open byte sink is bound to a shared transaction lifetime and
monotonic generation; sealing rejects dead, stale, or foreign bindings without
using a transaction object's reusable address as identity. Cleanup and
structural probes remain descriptor-relative; failed
abort cleanup is reported and may retain owned staging. A matching probe proves
exact names, safe types, link counts, declared sizes, and that `files/` and its
nested directories remain on the final directory's device, but it does not
authenticate same-size file contents. Non-Linux factories report unsupported.
The backend remains disconnected from the app and metadata cache; macOS and
Windows implementations, native qualification on every supported platform,
component selection, and the constrained parser worker protocol are still
required before any production-safety claim or production extraction.
The Linux root is a trusted same-euid namespace: callers must prevent untrusted
processes running as the same effective user from renaming or replacing entries
inside it while a store is active. Descriptor confinement, no-follow opens, and
device/inode revalidation prevent link traversal and detect observed
replacement, but Linux offers no conditional unlink-by-inode primitive that
could make top-level cleanup atomic against a hostile same-euid renamer.
Because Unshield is not hardened for malicious cabinet metadata, the adapter
is excluded from default builds and normal startup. The app is the only
composition root.

## Hosted qualification

The accepted package-1 through package-4 state at
`df5ea6d51037671ef0165dacac9fe26df1bf4d2b` is current in hosted CI. The
required jobs pass on Linux x64, Linux x64 with address/undefined-behavior
sanitizers, Linux x64 with the experimental adapter enabled, Windows x64, and
macOS Apple Silicon. This evidence qualifies the implemented capability,
validation, cache, VFS, and application-composition behavior described above;
it does not qualify the absent production extraction path.

The later trusted-result bridge at `909edcc` and media cancellation migration
at `0f2c78d` are covered by exact-SHA hosted build run `29147060407` at
`ca576e9`. GNU 13 Linux passed all 32 tests, including the bridge; the
experimental and sanitizer jobs passed; and Windows passed. AppleClang 17 on
macOS passed all 22 tests, including `media.cancellation` and the bridge. This
confirms the common cancellation portability correction and disconnected
result-validation target on the required hosted platforms; it does not qualify
a native worker, source broker, staging composition, macOS atomic store, or
runtime import path. Because the tests-only `ca576e9` change did not trigger
the parser-fuzz workflow, the accepted hosted fuzz evidence for the typed
protocol at that point remained the earlier `ba84cfc` run.

The trusted source-read broker was accepted and pushed at
`c90f2d1a7cbabdb90b688197d2d34ceb48526aeb`. The full local CTest suite passed
33/33, including synthetic sequence, budget, stability, failure, cancellation,
ticket, buffer, and capability-lifetime coverage. Exact-commit hosted build run
`29148133002` also passed Linux x64, sanitizers, the experimental cabinet
adapter, Windows x64, and macOS Apple Silicon; this is the cross-platform broker
evidence. Fuzz run `29148132997` separately passed Clang 18/libFuzzer for the
typed protocol only and did not build or fuzz the broker. The build evidence
qualifies the disconnected broker on the tested hosts, not a worker, transport,
runtime import path, staging, or publication.

## Local parser transport and parent-session qualification

The accepted frame-channel commit is
`e4b819a9efa37d5e401d111c4ac591365ce669ae`. Local validation completed a clean
warnings-as-errors build with 83/83 steps, the full CTest suite at 34/34, 50
consecutive passes each of `media.parser_frame_channel` and
`repository.policy`, and the existing platform common-worker test at 1/1.
The focused tests cover canonical 32-byte output, payload boundaries, validation
ordering, caller storage and view lifetimes, malformed exact-I/O reports,
sticky first-failure poisoning, cancellation and peer closure at header and
payload stages, same-direction exclusion, duplex progress, and abort wakeup.
These are local results only; no hosted result is claimed for `e4b819a` here.

The trusted parent handshake was accepted at
`13f0fb08e7d00159000f3721ebe0b0e1b1481188`. Local validation completed a clean
warnings-as-errors build with 87/87 steps and the full CTest suite at 35/35.
Synthetic tests independently verify canonical hello/header bytes, exact-empty
ready, ordering, exact limits and policy, proof movement and consumption,
downstream session/broker construction, pre-I/O rejection, unsanitized buffer
invalidation, sanitized terminal transport failures, and absence of escaped
views or proofs on failure. These are local results only; no hosted result is
claimed for `13f0fb0`.

The trusted parent session was accepted at
`7bd9d38213c7df160e0e84fcb50a9cacb0095558`. Hash, index, manifest, and diff
guards showed that the seven-file parent-session package matched the commit.
Independent pristine local verification used `git archive` of exact tree
`f28715ef827044928a0c9cc1ce45464d5c8d9519`; the archive SHA-256 was
`6361378e63c5de330784836106851fd4b0afb4d4b239d10b495912fe585a8123`.
It compiled committed `isolated_worker_unsupported.cpp` and excluded all dirty
native-backend files from the shared worktree. A clean Linux GCC 14 Debug
warnings-as-errors build passed 91/91 steps, the full CTest suite passed 36/36,
and the explicit repository-policy test passed 1/1. Synthetic behavioral
coverage includes exact-channel factory binding, proof preservation on
rejection, request and operation transitions, atomic outbound callback
crossings, receive/cancel races, prompt first-cause notifications, ticket
commit/abandon behavior, sink and buffer lifetime rules, privacy-sensitive
scratch/reply handling, terminal destruction, and no escaped frame or payload
result. Separate API, source, and CMake review establishes the disconnected
dependency edge and absence of launch, path, staging, publication, and runtime
authority. The practically unreachable `uint64_t` request-ID exhaustion path
has no counter-injection seam, and stable injected source-read failure remains
covered at the broker layer because the parent factory exposes no source-read
operation-table seam. These are accepted test limitations. These results are
from a pristine exact tree but remain local only; no hosted result is claimed
for `7bd9d38`.

The intended gameplay/rendering graph remains under design. Each new edge must
be expressed explicitly with `target_link_libraries` so CMake remains the
source of truth and cycles can be rejected in review.
