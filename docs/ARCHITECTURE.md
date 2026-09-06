# Architecture

Open Half-Life is a Rust workspace split into narrow crates under `crates/`.
Dependencies point from high-level orchestration toward low-level services;
`cargo xtask graph` enforces that the intra-workspace dependency graph stays
acyclic and matches the table below, which restates
`.plan/rust-architecture-r1.md` section 1.

| crate | responsibility | std? |
| --- | --- | --- |
| `ohl-core` | errors, sanitized diagnostics, SHA-256, bounded arithmetic | `no_std` + `alloc` |
| `ohl-parser-protocol` | OWP/1 framing, twelve typed schemas, budgets, ordering | `no_std`, no `alloc` |
| `ohl-parser-worker-service` | one bounded worker-side OWP/1 lifetime | `no_std`, no `alloc` |
| `ohl-parser-worker` | binary: fd 4 readiness, fd 3 lifetime, fixed dispatcher | `no_std` + `no_main` |
| `ohl-media-archive` | block-source trait, bounded listing model, path rules, classification vocabulary shared by both media readers and `ohl-vfs` | `no_std` + `alloc` |
| `ohl-iso9660`, `ohl-udf` | thin wrappers over pinned `hadris-iso`/`hadris-udf` 2.3.0 plus anti-abuse limits | `no_std` + `alloc` |
| `ohl-cabinet-format` | cabinet structure parsing/validation (Unshield translation) | `no_std` + `alloc` |
| `ohl-cabinet` | inflate, deobfuscation, split volumes, chunk streaming over `VolumeSource` | `no_std` + `alloc` |
| `ohl-platform` | `MediaSource`, atomic-directory publication, per-OS sandbox | std, `unsafe` allowed |
| `ohl-vfs` | mounts, normalization, bounded paged enumeration, read-only files | std |
| `ohl-media` | preflight-result mapping, fingerprint, `ValidatedMedia`, provenance cache | std |
| `ohl-import` | handshake/parent/process session, read broker, result bridge | std |
| `ohl-payload` | payload path policy, layout planning, component selection recipe, staging/publication | std |
| `ohl-formats` | BSP30/WAD3/MDL10/SPR decoders | `no_std` + `alloc` |
| `ohl-render`, `ohl-audio` | wgpu passes; cpal/rodio (winit window/input events are handled directly in `ohl-app`, not a separate crate) | std |
| `ohl-world`, `ohl-physics`, `ohl-game`, `ohl-ui` | world state, GoldSrc hulls, rules, egui HUD | std |
| `ohl-combat` | damage model, attack traces against hulls and studio hitboxes, combat events | std |
| `ohl-ai` | monster conditions, senses, schedules, squads, movement glue | std |
| `ohl-nav` | node-graph navigation: A* plus local steering, independent of the entity layer | std |
| `ohl-player` | non-motion player systems: health/HEV armor, fall damage, drowning, flashlight, long jump, HUD/save projection | std |
| `ohl-gameplay` | bridges `ohl-combat` events into `ohl-ui::HudState`, sound cues and viewmodel actions | std |
| `ohl-campaign` | sourced chapter/map sequence, difficulty enum, `skill.cfg` table | `no_std` + `alloc` |
| `ohl-save` | project-owned versioned save-file container (not the GoldSrc `.sav` format) | std |
| `ohl-assets` | GoldSrc-style asset filesystem over an imported payload tree (loose files plus Quake `PACK` archives) | std |
| `ohl-engine` | composes the crates above into one `Game`: fixed-tick simulation, save wiring, headless/scripted-input support | std |
| `ohl-parser-backends` | the parser worker's container back ends: joins `ohl-wise`/`ohl-mscab`/`ohl-isz` to the OWP/1 dispatcher | `no_std` + `alloc` |
| `ohl-wise` | clean-room reader for Wise Installation System packages (PE/NE overlay) | `no_std` + `alloc` |
| `ohl-mscab` | clean-room, bounds-checked reader for the Microsoft Cabinet (MS-CAB) container | `no_std` + `alloc` |
| `ohl-isz` | clean-room decoder for InstallShield 3 "Z" archives and PKWARE "imploded" streams | `no_std` + `alloc` |
| `ohl-test-worker` | development-only support for the freestanding Linux isolated-worker test image | std |
| `ohl-app` | composition root binary (`open-half-life`) | std |
| `xtask` | policy check, worker image build, packaging, campaign/combat smokes | std |

All 36 crates above exist today (verify with `cargo xtask graph`, which
checks every crate's dependency edges against `xtask/src/graph.rs`'s
`ALLOWED_EDGES` table and reports the count it validated). New crates must
add their dependency-edge row to that table before `cargo xtask graph`
passes, and must keep the workspace-wide `unsafe_code = "forbid"` lint
unless they are one of the two exempt crates below. See `docs/MILESTONES.md`
for what each package built and what remains open per milestone.

The allowed direct intra-workspace dependency edges are:

```text
ohl-core            -> (none)
ohl-parser-protocol -> ohl-core
ohl-parser-worker-service -> ohl-parser-protocol
ohl-parser-worker   -> ohl-parser-worker-service
ohl-media-archive   -> ohl-core
ohl-iso9660 | ohl-udf -> ohl-core, ohl-media-archive
ohl-cabinet         -> ohl-cabinet-format -> ohl-core
ohl-platform        -> ohl-core
ohl-vfs             -> ohl-core, ohl-platform, ohl-media-archive, ohl-iso9660, ohl-udf
ohl-media           -> ohl-platform, ohl-core
ohl-import          -> ohl-media, ohl-vfs, ohl-parser-protocol, ohl-platform
ohl-formats         -> ohl-core
ohl-world/physics   -> ohl-formats, ohl-vfs
ohl-render/audio/input/ui -> ohl-core (+ ohl-world for render)
ohl-app             -> any crate above (the composition root)
```

There is deliberately no `ohl-vfs -> ohl-media` edge, so a container reader
can never reach the provenance cache or fingerprinting logic; the two cabinet
crates isolate a licensed MIT-derived translation (~3,200 lines) from
clean-room code and run only inside the sandboxed parser worker.

The block above restates the original migration architecture's package-level
edges and is not exhaustive: `cargo xtask graph`'s `ALLOWED_EDGES` table in
`xtask/src/graph.rs` is the current source of truth and also covers the
gameplay/campaign/engine crates added since (`ohl-nav`, `ohl-player`,
`ohl-gameplay`, `ohl-campaign`, `ohl-save`, `ohl-assets`, `ohl-engine`,
`ohl-parser-backends`, `ohl-wise`, `ohl-mscab`, `ohl-isz`,
`ohl-test-worker`). In broad strokes, `ohl-engine` depends on the whole gameplay stack
(`ohl-world`, `ohl-physics`, `ohl-game`, `ohl-combat`, `ohl-ai`, `ohl-nav`,
`ohl-player`, `ohl-gameplay`, `ohl-campaign`, `ohl-save`, `ohl-assets`,
`ohl-render`, `ohl-ui`) and exposes one composed `Game` type with exactly two
verbs (`tick`, `render`); `ohl-app` is the composition root and depends on
`ohl-engine` directly, but also depends directly on several of the same
gameplay crates (`ohl-game`, `ohl-campaign`, `ohl-save`, `ohl-assets`,
`ohl-render`, `ohl-world`, `ohl-physics`) for its own CLI wiring (headless
capture, save-slot handling, asset mounting), plus the whole import/media
stack (`ohl-import`, `ohl-payload`, `ohl-media`, `ohl-vfs`, `ohl-iso9660`,
`ohl-udf`, `ohl-media-archive`, `ohl-platform`) for the import path.

## `unsafe` inventory

Every crate carries the workspace-wide `#![forbid(unsafe_code)]` lint except
two, which instead `allow` it (a `forbid` level cannot be relaxed by a
crate-level attribute, so the allowance lives in each crate's own
`Cargo.toml`):

- **`ohl-platform`** — Windows FFI (`GetFileType`, `GetFileInformationByHandle`
  for the pinned native identity `MediaSource` needs but `std` does not
  expose) and, once the Linux worker launcher lands, the fork/exec, seccomp
  install, pidfd, and `renameat2` calls that give the isolated parser worker
  its sandboxed lifetime. Every unsafe site carries a `// SAFETY:` comment and
  is inventoried in the crate's own module documentation.
- **`ohl-parser-worker`** — the freestanding binary's own `_start` entry
  point, because a `#![no_std] #![no_main]` binary has no runtime to hand
  control to `main` for it.

Both crates carry `#![deny(unsafe_op_in_unsafe_fn)]`. No other crate,
including every parser and format decoder, contains an `unsafe` block.


The `ohl-parser-protocol` crate is deliberately isolated: nothing outside the
parser-worker chain depends on it, and its only allowed dependency edge is
`ohl-core` (see the crate table above). The `no_std`, no-`alloc` bound means
the same protocol sources build unchanged into the freestanding
`ohl-parser-worker` binary. Its accepted OWP/1 protocol layer provides
canonical bounded framing and headers,
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

> The remainder of this section (through "Media import planning and
> staging") specifies the accepted result-bridge, read-broker, handshake, and
> parent/process-session design the removed C++ tree implemented and hosted
> CI qualified. It is retained as the byte-for-byte specification the Rust
> `ohl-import` crate (package R4.5 in `.plan/rust-architecture-r1.md`) must
> reproduce; that crate does not exist yet, so the `OpenHalfLife::` names and
> commit hashes below are historical identifiers into git history before the
> C++ removal, not current Rust crate or module names. See
> `docs/MILESTONES.md` for what has actually landed in Rust so far.

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
containment now exists as a disconnected source-selected backend. Its installed
worker now hosts the bounded worker-side OWP/1 service described below. The
higher parent process-session owner now exists too, as the disconnected
`OpenHalfLife::media_parser_process_session` target described below. Its
composition with the result bridge, source-read broker, selection policy,
staging, and the application remain later dependencies.

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
the unsupported backend. The Linux x86-64 installed worker now contains the
service-bearing media-parser bootstrap, but production composition is still
missing a real payload dispatcher/parser, runtime selection, and
staging/publication integration. The canonical private worker-service library
remains a non-installed artifact; the Linux worker links a separate private
freestanding runtime built from the same protocol and service implementation.

The higher process-session owner now exists as the disconnected
`OpenHalfLife::media_parser_process_session` library
(`ParserProcessSession`, `src/media/parser_process_session.cpp`), accepted at
`537c11b` ([PR #12](https://github.com/timo-42/open-half-life/pull/12)). Its
paired `ParserSessionIdAllocator` hands out nonzero, monotonic, unique session
IDs and worker epochs and fails closed on exhaustion rather than wrapping.
`ParserProcessSession` owns exactly one `platform::IsolatedWorker` for one
session's lifetime: `open()` performs the handshake and builds the
`ParserParentSession` from its proof; orderly shutdown drives protocol
shutdown, then `close_channel()`, then `wait()`; any failure, cancellation, or
timeout escalates to a single cached `terminate_and_wait()`. The destructor
never abandons a live worker. It is move-only and non-copyable, and has no
raw-path, destination, cache, staging, publication, or component-selection
authority; it is still disconnected from the application.
ParentSession owns none of those lifecycle actions.

The accepted P1 worker-side boundary is a separate static target,
`OpenHalfLife::parser_worker_service`, whose only project dependency is
`OpenHalfLife::parser`. It is not installed or exported, and its header and
callback contracts remain private implementation details. On Linux x86-64, the
installed native worker links a private freestanding runtime copy of the same
protocol and service sources; the application, media stack, and other platform
workers still do not link the canonical target. The service drives one bounded
OWP/1 worker lifetime over caller-supplied synchronous transport and dispatcher
operation tables plus caller-owned, disjoint scratch buffers. It applies
protocol ordering and
budgets while mediating enumerate, stream, parent-owned source reads,
cancellation, and shutdown. It does not select or implement a payload parser.

The trusted parent remains the sole owner of the pinned source capability,
catalog acceptance, deterministic selection, destination, staging, and
publication. From that parent boundary, the worker and everything it emits are
untrusted and must still pass independent typed parent/result validation. The
service operation tables may be supplied only by trusted project composition;
media cannot configure them. Their code executes within the worker's ambient
authority, so the private service contract is not a substitute for native
containment.

Initial local synthetic service validation passed 1/1, the complete development
suite passed 39/39, and the AddressSanitizer plus UndefinedBehaviorSanitizer
suite passed 40/40. The merged hosted qualification for this disconnected
boundary is recorded below. None of this evidence exercises proprietary media
or a real payload parser.

The B1 Linux x86-64 bootstrap binds that service to the native worker's exact
descriptor inventory. The worker emits the fixed readiness attestation on fd 4,
closes that descriptor, then hosts one OWP/1 lifetime over the inherited
full-duplex fd 3. A canonical parent `hello` produces the exact empty worker
`ready`; canonical shutdown and orderly peer close end cleanly. Malformed or
truncated frames and transport failures map to stable sanitized failure exits,
and the public native lifecycle exposes only the existing clean, failed,
crashed, resource-limit, terminated, or unknown categories.

The worker's dispatcher is compile-fixed project code and currently returns
`unsupported` when enumeration or streaming begins. It cannot be selected or
configured by media. Consequently the service can enforce handshake, protocol,
cancellation, shutdown, and lifecycle rules inside containment, but cannot
enumerate, parse, source-read, or extract a payload. The trusted parent retains
all source, selection, destination, staging, publication, cache, and runtime
authority.

The build links a private static freestanding protocol/service runtime into the
static x86-64 worker. The worker identity remains the compile-fixed
`/usr/libexec/open-half-life/ohl-media-parser-worker`; the launcher pins and
verifies its no-follow, non-writable, non-set-id, static x86-64 ELF identity
before applying resource limits, no-new-privileges, Landlock, seccomp, the
fixed descriptor inventory, readiness framing, and pidfd-backed lifecycle.
This bootstrap is implemented and tested only on Linux x86-64. It is not a
supported worker backend for another Linux architecture, Windows, or macOS.

Local evidence includes `platform.isolated_worker.linux`, which stages the
exact production-target bytes at the test backend's compile-fixed identity and
launches them through the public native launcher. It covers fragmented
`hello`/canonical `ready`/shutdown and clean reap, malformed-frame failure,
compile-fixed unsupported enumeration, peer closure, idempotent channel close,
cached terminal wait, and owned terminate-and-reap. The direct
`platform.media_parser_worker.service` test additionally covers truncated
headers and payloads plus orderly peer close, while
`platform.media_parser_worker.install_smoke` checks the installed static image,
non-writable/non-set-id mode, payload arenas, readiness framing, canonical
fd-3 handshake, shutdown, and clean exit. These are project-authored synthetic
Linux x86-64 tests, not proprietary-media or production-import evidence. Final
B1 validation passed the four focused service/bootstrap tests at 4/4, the full
development suite at 40/40, and 50 consecutive real-launcher test runs at
50/50. Owned termination may observe either `clean` or `terminated` when
orderly peer EOF wins the race with pidfd termination; both results are
terminal, cached, and reaped.

Work resumes in dependency order: add another tuple's native backend; add a
real dispatcher/parser as a separate scope; compose the now-accepted
process-session owner with the handshake and `ParentSession`; then integrate
a deterministic component-selection recipe before staging and publication.
The installed Linux worker hosts the service but removes none of those later
blockers, and the synthetic evidence authorizes no proprietary extraction.

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

`ohl_platform::MediaSource` is a read-only, move-only capability for one
natively opened file object. Path resolution occurs only during acquisition;
sharing the capability (behind an `Arc`) shares the same pinned native
identity, and positional reads never reopen the path. Identity pinning
prevents a later pathname replacement from retargeting the source. It does
not make the underlying bytes immutable: an external writer may still mutate
the opened object. Phase boundaries therefore call `verify_unchanged()` to
compare native identity, type, size, and available content-change indicators
with the acquisition snapshot.

`ohl_media::ValidatedMedia` binds that same capability to a
[`MediaDescription`](../crates/ohl-media/src/description.rs) (the plain value
`ohl-app` maps a preflight crate's result onto) and a full SHA-256 fingerprint
(`ohl_media::fingerprint`). It is a move-only, non-`Clone` proof: evidence
that the source passed those gates at validation time, not a promise that
future content cannot change. Before metadata-only cache publication,
`ohl_media::prepare_import_cache` verifies the pinned source again, rehashes
it end to end, requires the digest to equal the validation fingerprint, and
publishes nothing on mismatch or read failure. Source paths and media bytes
are not persisted.

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

`ohl-app` acquires the source once, discards the selected path, runs the ISO
9660 preflight then the UDF preflight (`ohl_iso9660::preflight`,
`ohl_udf::preflight`) to classify it, mounts the same shared `Arc<MediaSource>`
through `ohl_vfs::Mount`, and passes the same `ValidatedMedia` to cache
preparation. It never reopens the original path. This acquire-once flow is
the accepted composition for startup; `ohl-app` remains the only composition
root.

## Bounded read-only VFS

`ohl-vfs`'s `Mount` type wraps the `ohl-iso9660` and `ohl-udf` readers (which
in turn wrap the pinned `hadris-iso`/`hadris-udf` crates) behind one facade
type, so neither third-party reader's API types leak into the engine. It
retains the pinned `Arc<MediaSource>` for the mount lifetime via
`MediaSourceBlockReader`, checks source stability around operations
(periodically and via `verify_unchanged()`), serializes third-party access
behind a mutex, and exposes normalized read-only paths, seekable files, and
explicitly shared (`Mount::share()`) read-only mount handles.

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

> This section specifies the accepted path-normalization, layout, staging,
> and native-store design the removed C++ tree implemented (Unshield-backed
> cabinet extraction, `renameat2(RENAME_NOREPLACE)` publication, the Linux
> native-store qualification). None of it exists in Rust yet: `ohl-import`
> and the two `ohl-cabinet*` crates (package R4.4/R4.6 in
> `.plan/rust-architecture-r1.md`) are still ahead. It is retained as the
> specification those crates must reproduce; `media`, `Unshield`, and the
> commit hashes below are the historical C++ identity, not current Rust
> names.

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

## Save files

`ohl-save` is a project-owned, versioned save-file container over `ohl-core`
only. **It is not the id Tech/GoldSrc `.sav`/`.hl1` save format**: the layout
below was designed from scratch for this project. A file is a fixed 8-byte
magic, a `u16` major/minor format version, a bounded header (a game-version
string, a Unix creation timestamp, a map-identity string, a chapter/title
string, and a reserved thumbnail byte slot — every variable-length field is
length-prefixed and checked against a fixed maximum on both write and read),
a `u32` section count, a section table (one `tag: u32, offset: u64,
length: u64, sha256: [u8; 32]` entry per section), the concatenated section
payloads in table order, and a whole-file SHA-256 trailer. `SaveReader::open`
validates every table entry's offset and length against the file size and a
caller-supplied `Limits` (maximum section count, maximum single-section
size, maximum file size) before trusting them, verifies every section's
digest and the whole-file trailer, and never panics on truncated or
adversarially crafted input. A major-version mismatch is always rejected
(`SaveError::UnsupportedMajorVersion`); a minor-version mismatch is
tolerated, as are section-table entries whose tag falls below
`MIN_APPLICATION_TAG` (reserved for this crate's own current or future use)
that this build does not otherwise interpret — their integrity is still
verified, but they are excluded from ordinary lookups and reported via
`SaveReader::unknown_section_count` instead of causing a failure.
`SaveWriter::begin(header)` / `add_section` / `add_section_serde` (the
latter encoding a `serde` value with `postcard`) / `finish(&limits)` builds
a file; `SaveReader::open(bytes, &limits)` reads one back and exposes
`header()`, `sections()`, `section(tag)`, and `deserialize::<T>(tag)`.
`SaveSlot` layers a directory of `<slot>.ohlsave` files on top, with a
default per-user save directory resolved through `directories`,
`AUTOSAVE_SLOT_NAME`/`QUICKSAVE_SLOT_NAME` conventions, bounded `list()`,
`delete()`, and write-to-temp-then-rename publication (documented per-target
guarantee in `crates/ohl-save/src/slot.rs`; unlike
`ohl_platform::atomic_directory`'s create-only primitive, a slot write is
meant to replace its previous contents). `ohl-engine` is now the one crate
that defines section tags and actually serializes/restores world and entity
state through this container; see "The engine's save container" below for
the tag map it wires in.

## The fixed-tick engine loop

`ohl-engine::Game` is the composition of every gameplay crate into one
type with exactly two verbs: `Game::tick` advances one frame from an
`Input` snapshot (banking any leftover time and running zero or more fixed
steps of `ohl_engine::tick::TICK_SECONDS`, so simulation results never
depend on the host's frame rate) and `Game::render` draws the current
frame into a caller-supplied target. `ohl_engine::systems::Systems::step`
is the fixed step's body and its order is normative, not incidental — two
rules from its own module docs are worth restating here because they
constrain how new phases must be added: damage is resolved *after* AI
thinks, not before (so a monster shot this step reacts on the next one
rather than the think phase mutating health it just read), and movers run
*last* (so a door blocked or damaged this step resolves against positions
everything else has already agreed on). The current phase order is:

1. latch input (edges apply to the frame's first step only, held axes to
   every step)
2. player movement (`ohl-physics`' `PlayerController`). Before tracing
   moves, `Level::sync_brush_collision` first pulls every solid brush
   entity's current position into the collision model (`ohl_physics::hull`'s
   `BrushPart`/`CollisionModel::attach_brush`), so a door, platform, or
   train the previous step's phase 12 moved is collided against where it
   now is, not where the map was compiled — the player collides against
   brush entities the same way it collides against worldspawn, not only
   against the static world.
3. player systems (`ohl-player`: health/armor, fall damage, drowning, the
   HEV suit, flashlight, long jump)
4. actor sync (camera/controller state written back onto the entity world)
5. rebuild the combat hitbox index (`ohl_combat::HitboxIndex`, from every
   entity carrying a `StudioAnim`)
6. weapons (firing state, hitscan/melee/beam resolution against the
   rebuilt hitbox index, ammo bank)
7. projectiles
8. AI think (`ohl-ai`'s senses/schedules over an `ohl_ai::SightContext`
   built this step; `ohl-nav`'s node graph and A* drive movement)
9. resolve queued damage (the one place health/armor actually change)
10. lifecycle (corpse/gib decisions, corpse fade, monster
    `TriggerCondition`/`TriggerTarget` firing, `monstermaker` ticking)
11. pickups and chargers
12. triggers and movers (map-logic `Simulation::fire`, doors, buttons,
    `func_train`/`func_tracktrain`, `multi_manager`). This is also where
    `ohl_game::logic::Simulation::touch_triggers` runs: it tests the
    player's own standing-hull bounding box (not just a single point)
    against every `trigger_once`/`trigger_multiple` volume, so walking
    into a touch trigger fires it the same way a real touch trigger tests
    brush-against-brush, without needing a `use` press. The crouched hull
    is not yet threaded through this phase, so a crouching player is still
    tested against the standing box.
13. presentation (`ohl-gameplay::GameplayBridge` turning combat/pickup
    events into HUD state and sound cues)

Determinism follows from two properties: no phase reads a wall clock or
iterates a hash map, and there is exactly one root random stream (seeded
from `SystemsConfig::rng_seed`, itself a fixed constant unless a caller
overrides it, never the environment or a clock) that every other generator
in the simulation is seeded *from* rather than re-seeded with the same
number. Two games built from the same map bytes, the same seed and the
same input sequence therefore reach the same `ai_state_hash` and the same
save bytes.

## The engine's save container

`ohl-engine::save::GameSave` is what actually goes into an `ohl-save`
container (see "Save files" above for the container format itself); it is
still a from-scratch, project-designed layout, not the GoldSrc `.sav`/
`.hl1` format. One tag per subsystem, so a later milestone can add a
section without renumbering, and an older build reports an unrecognized
tag as unknown rather than failing to open the file:

| tag | section | contents |
| --- | --- | --- |
| 16 | `SECTION_ENGINE_HEADER` | map name, resolved chapter title, difficulty, elapsed simulated time |
| 17 | `SECTION_PLAYER_CARRY` | the `PlayerCarry` cross-level-transition hook's state |
| 18 | `SECTION_ENTITY_REGISTRY` | one `EntitySnapshot` per registry entity, in spawn order |
| 19 | `SECTION_SIMULATION` | the map-logic simulation's scheduled events and trigger cooldowns |
| 20 | `SECTION_GLOBAL_STATE` | the `globalname`/`env_global` state table |
| 21 | `SECTION_LIGHT_STYLE_TIME` | the time the light-style animation is evaluated at |
| 22 | `SECTION_VIEW` | the camera/player pose, so a load resumes exactly where the save was taken |
| 23 | `SECTION_INVENTORY` | owned weapons, clips, ammo reserves, selection, the drawn weapon's firing summary (M7.9 P4b) |
| 24 | `SECTION_ENTITY_COMBAT` | one `EntityCombatSnapshot` per registry entity, in spawn order (M7.9 P4b) |
| 25 | `SECTION_AI` | one optional `AiSnapshot` per registry entity, in spawn order (M7.9 P4b) |
| 26 | `SECTION_PROJECTILES` | live projectiles and placed deployables (satchels, tripmines) (M7.9 P4b) |
| 27 | `SECTION_RNG` | the shared random stream and the substep counter (M7.9 P4b) |
| 32 | *(reserved, `ohl-player`)* | a `PlayerSnapshot`, written through `Player::snapshot()` once a later package wires it; not produced by any current build |

Serialization goes through `postcard` via `ohl_save::SaveWriter::
add_section_serde`, which is deterministic: the same game state and header
always produce byte-identical files (asserted by the save -> load -> save
round-trip test). Tags 23-27 (M7.9 P4b) are read as `None`/a default when
absent, so a save written before that package still loads; a section that
is present but fails to decode fails the whole read closed
(`EngineError::SaveUnreadable`), same as every other section. Restoring
tags 26/27 does not by itself bring back a deployable's or a model-backed
projectile's drawn stand-in entity or `hecs::Entity` handle (neither is
serializable): `ProjectileSystem::restore_snapshot` re-spawns a fresh stand-in
for every restored satchel, tripmine, or in-flight model-backed projectile
so it draws and stays damageable again after a load. `docs/MILESTONES.md`'s
M7.9 P4b note tracks the one known gap: weapon *inventory* (owned weapons,
clips, reserve ammo) currently rides inside `SECTION_PLAYER_CARRY`'s ad hoc
byte encoding rather than its own section, which still round-trips
correctly today but is not yet self-describing independent of that carry
state's shape. Neither `TriggerCameraState` nor `TrackTrainState` is part
of any save section yet, for the same self-describing-format reason: a
save taken mid-camera-sequence or mid-route resumes with that state
dormant instead of where it left off.

## The asset layer and PAK precedence

`ohl-assets::AssetFs` is the one surface every gameplay crate resolves a
game-relative path (`maps/<name>.bsp`, `sprites/…`, `models/…`, `sound/…`,
`gfx/…`) through, over an imported payload tree's `files/` directory.
`AssetFs::mount` walks a list of mod directories in GoldSrc's own
search-path order (the mod actually being played first, `valve` last as
the shared base content), and for each one discovers `pak0.pak`,
`pak1.pak`, ... (a contiguous ascending run) plus any other `*.pak` file,
reading only each archive's header and directory bytes, then walks loose
files bounded by `Limits::max_depth`/`Limits::max_indexed_files`. Everything
merges into one case-insensitive index under a strict, deterministic
precedence: a loose file always beats a PAK entry of the same name, an
earlier PAK beats a later one, and an earlier mod directory beats a later
one. `AssetFs::open` resolves a path against that index and returns a
`Read + Seek` handle over either a whole loose file or a bounded byte range
inside a PAK; `AssetFs::resolve_wads` resolves a worldspawn `wad` key's
semicolon-separated, mapper-authored absolute paths by basename only, the
same way GoldSrc does. No path this crate resolves (the caller's asset
path, a worldspawn `wad` value, a loose filesystem path, a PAK member name)
ever reaches a log line or an error variant.

## Headless capture and scripted input

`crates/ohl-app/src/game_run.rs` is the one production playable loop; it
resolves every asset through `ohl_assets::AssetFs` over a published payload
tree (never straight off disk the way the `dev-tools`-gated `--dev-bsp`
path does) and composes the whole frame through `ohl_engine::Game`.
`--headless-screenshot PATH` renders offscreen instead of opening a window,
advancing the simulation a fixed `CAPTURE_STEP` (`1/60`\ s) per tick for
`--frames` steps so a capture is reproducible regardless of host speed,
then writes one 1280x720 PNG. `--viewpoint X,Y,Z,PITCH,YAW` captures from an
explicit camera pose and `--spawn-offset DX,DY,DZ,DPITCH,DYAW` from one
relative to the map's own `info_player_start`; without either, the capture
stands at the player start directly. This offscreen path needs a Vulkan
(or Metal) adapter but not a display server, which is what makes
`cargo xtask campaign-smoke` viable on a headless CI runner; a machine with
no real GPU can still exercise it under a software Vulkan implementation by
setting `OHL_RENDER_GPU_TEST=1` (see `docs/RENDER_DEPENDENCIES.md`).

`crates/ohl-app/src/script.rs` defines a project-owned, project-authored
(no published source needed) deterministic scripted-input format consumed
by `--script PATH`: plain text, one `<ticks> <token> [args]...` line per
action, driving the same `ohl_engine::Input` fields the real input path
sets (movement/look axes, held buttons, edges, weapon-slot selection),
bounded to 4,096 non-comment lines and 100,000 total ticks. A scripted run
works with or without `--headless-screenshot`: without one, the ticks still
run headlessly with no GPU needed at all, just with no PNG written at the
end. `--script-log` (only meaningful with `--script`) enables the fixed
milestone log lines documented in `docs/m79-design.md` §7 (for example "A
scripted sequence started."); nothing map-derived is ever interpolated into
them. `cargo xtask combat-smoke` drives this path over
`xtask/smoke-scenarios/*.txt` and checks each run's stderr against those
exact fixed lines.

By default, neither capture path follows a `trigger_changelevel`: a headless
or scripted run just logs a fixed "not followed" line and keeps rendering
the map it started on, so an ordinary capture never silently jumps to a
different map. `--follow-level-change` opts into calling the same
`Game::change_level` path the interactive window uses instead, and keeps
ticking (or capturing) on whatever destination map the transition leads
to; `crate::game_run`'s two capture paths share one `handle_level_change`
helper for this. Behind `--features dev-tools`,
`--viewpoint-at-nearest-monster DISTANCE` places a headless capture's eye
`DISTANCE` units from the spawned monster nearest the map's player start,
at its eye height, facing it, in noclip, backed by an additive
`Game::nearest_monster_position` (data only, never logged) — useful for
confirming a monster actually rendered without hand-computing a
`--viewpoint`.

## Hosted qualification history

The two sections that used to follow here recorded exact-commit hosted CI
evidence (Build run IDs, CTest pass counts, per-platform matrices) for the
C++ tree removed in this pull request. Every commit hash they cited is still
reachable in git history before the "Remove C++ implementation superseded by
the Rust workspace" commit, so that qualification evidence is not lost, only
no longer current. Current Rust acceptance evidence — which PR accepted each
milestone, and what tests and lints ran — lives in `docs/MILESTONES.md`
instead of here, so it does not go stale the same way.

The intended gameplay/rendering crate graph remains under design. Each new
edge must be added explicitly to `xtask/src/graph.rs`'s `ALLOWED_EDGES` table
so `cargo xtask graph` remains the source of truth and cycles are rejected
automatically rather than in review alone.
