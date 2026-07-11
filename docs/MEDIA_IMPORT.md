# Local media import data policy

Open Half-Life requires users to provide their own lawfully obtained game
media. Possession of that media does not grant this project permission to
publish, redistribute, upload, or commit any part of it. Import therefore
operates across a strict local proprietary-data boundary.

This policy applies to runtime import, developer investigation, tests, fuzzing,
diagnostics, and support workflows. It supplements `CLEAN_ROOM.md`; it does not
authorize extraction or other behavior that the runtime has not implemented.

## Data classification

Treat the following as **local proprietary data** by default:

| Data | Examples of the protected class |
| --- | --- |
| Source media | ISO bytes, sectors, filesystem structures, and container bytes |
| Internal identifiers | Internal paths, directory names, entry names, component names, file-group names, and archive keys |
| Imported output | Extracted files, decoded data, assets, binaries, and intermediate byte streams |
| Parser communication | IPC messages, parser requests and responses, debug traces, token streams, and rejected-record dumps |
| Process diagnostics | Core files, minidumps, crash dumps, heap or stack snapshots, and debugger captures from a process that accessed media |
| Dynamic-analysis output | Sanitizer reports or artifacts that may contain media-derived bytes, names, paths, memory, or process arguments |
| Fuzzing material | Seeds, corpora, minimized reproducers, and coverage artifacts derived from user media |

Derived or transformed data does not become redistributable merely because it
is smaller, encoded differently, compressed, minimized, or embedded in another
artifact. If provenance is uncertain, keep the data local and request a
clean-room review.

## Outputs that are forbidden

Local proprietary data must not be:

- written to application or developer logs;
- sent through telemetry, analytics, issue-reporting, or support collection;
- included in automatic crash, sanitizer, or fuzzing uploads;
- placed in CI artifacts, build logs, screenshots, issue descriptions, or pull
  request comments;
- committed as a fixture, golden file, snapshot, corpus, or test expectation;
- copied into a release, source archive, binary package, container image, or
  redistributable cache; or
- shared with another person unless that transfer is independently lawful and
  outside the project distribution workflow.

Media import must not enable telemetry or automatic crash upload. A failure may
report a project-defined error category, bounded numeric stage, and other
non-sensitive status, but it must not report source paths, internal names,
record bytes, extracted bytes, parser messages, or memory dumps.

## Sanitized reporting

A carefully reviewed report may contain only the minimum information needed to
identify or diagnose a compatibility class. Suitable fields can include:

- a digest of the complete source or a project-authored synthetic fixture;
- total container size and coarse aggregate counts or sizes;
- documented container or filesystem type and version;
- high-level categories expressed with project-owned vocabulary; and
- project-defined error codes and limit names.

Sanitization is contextual, not a mechanical allowlist. Omit a field when its
combination with other fields could reveal a proprietary internal identifier or
uniquely reconstruct protected metadata. Never include internal names, partial
content, raw records, source locations, user identifiers, secrets, or keys.
Digesting media is a read of the media and must be part of the user-authorized
runtime/import operation; a digest is not permission to retain or redistribute
the source bytes.

## Runtime-only local recipe input

Selection information needed for a particular lawfully owned edition is a
runtime-only local recipe supplied by or generated for the user. A recipe that
contains media-derived identifiers is itself local proprietary data.

- Do not compile recipes into the engine or commit them to the repository.
- Do not infer a redistributable recipe from private media observations.
- Keep project-owned selection and validation algorithms generic and based on
  recorded public format sources.
- Load a recipe only for the current local import and pass only the minimum
  validated selection decisions to later stages.
- Do not log recipe contents or include them in provenance manifests, crash
  reports, telemetry, or cache keys that may be shared.

## Local storage, retention, and cleanup

Source media, local recipes, imported payloads, parser artifacts, and caches
belong in repository-ignored or per-user application storage. The repository
policy and `.gitignore` are defenses against accidental commits, not permission
to store or share proprietary data.

- Prefer a per-user cache outside the source checkout.
- Use private permissions where the platform permits them.
- Retain only data required for the user's local runtime or an explicitly
  requested local diagnostic session.
- Provide a clear local cache/diagnostic cleanup path and document what it
  removes before deletion.
- Remove owned temporary staging and parser IPC artifacts after success and
  after handled failure when safe cleanup is possible.
- If a transactional cleanup fails, report only a sanitized error and leave the
  owned local artifact isolated for explicit retry; never upload it.
- Never delete, rewrite, normalize, or otherwise mutate the user's source media.
- Before committing, inspect staged and untracked state so ignored or generated
  proprietary artifacts cannot enter a patch by mistake.

Crash dumps, sanitizer artifacts, and fuzzing reproducers from media-processing
work are opt-in local diagnostics with the shortest practical retention. They
must be deleted when no longer needed. A user may always choose to clear local
imported data and repeat import from their own media.

## Pinned source and mutation semantics

An import must bind all phases to one validated source object instead of
canonicalizing or reopening a pathname independently for preflight, hashing,
VFS access, and parsing.

1. Acquire the user-selected source read-only through one atomic platform-native
   operation that pins the resolved regular-file object. If policy rejects a
   symbolic link or reparse point, apply that rule within the same acquisition,
   not with a check followed by an open.
2. After acquisition, never canonicalize or reopen the original pathname.
   Record stable identity and relevant size/change metadata from the pinned
   object itself.
3. Share that pinned handle, or constrained read-only duplicates of it, across
   preflight, digest, VFS, and parser phases.
4. Do not give a parser worker authority to resolve or reopen the original
   pathname.
5. Revalidate identity and mutation indicators at defined phase boundaries and
   reject truncation, replacement, or observed modification.
6. Establish content stability through one of these complete-source modes:
   - After all payload reads, reread the entire pinned source and require its
     digest to equal the accepted whole-source digest immediately before
     publication. A read error, metadata change, or digest mismatch fails the
     import and prevents publication.
   - Copy the entire pinned source into create-new private storage, verify its
     whole-source digest, seal it against further writes, and perform all
     parsing and payload reads only from that sealed private snapshot.

These rules prevent path replacement from causing different phases to inspect
or import different files and prevent publication based on an unverified mix of
source states. They do not make mutable media trustworthy; observed mutation or
failed end-to-end verification fails the import.

The accepted platform-independent staging boundary applies the complete-source
mode above but is not wired to runtime extraction. `stage_payload` requires a
`ValidatedMedia` proof, derives source identity only from its accepted
whole-source size and SHA-256, and no longer accepts caller-supplied source
identity. Every `PayloadSource` invocation receives the exact pinned
`MediaSource` from that proof, the planned opaque source token, and the same
staging `CancellationToken`. The local version-2 stage identity also binds a non-empty
trusted recipe identity bounded to 4,096 bytes and the normalized layout's
entry count, declared total, paths, and declared sizes; transport-local source
tokens are excluded.

After plan validation, each payload file and then the completion metadata are
sealed before the complete pinned source is reverified against the accepted
size and SHA-256. A final cancellation check follows verification, and
`publish_no_replace()` is the next store operation after that check. Any
verification or cancellation failure before publication either occurs before a
transaction exists or aborts the owned transaction; none publishes a staged
directory. These guarantees cover only the accepted staging candidate and do
not authorize or establish parser-driven extraction, runtime integration, or
playability.

Media cancellation uses project-owned `CancellationToken` and
`CancellationSource` value types with standard-like cooperative polling
semantics. Copies share identity and one atomic requested state;
`request_stop()` is non-throwing and succeeds only for the first request, while
consumers poll `stop_requested()` at explicit boundaries. A default token can
never be stopped. After the last source disappears, an unstopped token reports
that stopping is no longer possible, whereas an already requested token keeps
reporting the request. This preserves the existing stream, stage, and complete
source-verification cancellation checks without callbacks or asynchronous
interruption. Commit `0f2c78d` removes media's dependency on the AppleClang 17
libc++ experimental `std::stop_token` surface without enabling an experimental
ABI. Hosted build run `29147060407` at `ca576e9` confirms the replacement on
AppleClang 17: macOS passed all 22 tests, including `media.cancellation` and the
parser-result bridge. This is common portability evidence, not acceptance of a
macOS native-store implementation.

## Parser worker isolation

Untrusted container and filesystem metadata must be handled by a bounded parser
boundary. Where a parser is not independently hardened, run it in a constrained
worker process before allowing it to feed production import.

The current `parser` library is accepted as bounded protocol infrastructure
only. It provides canonical OWP/1 framing, generic bounded primitive payload
helpers, frame and cumulative budgets, and fail-closed session ordering,
including the cancellation and one-shot late-reply drain rules documented in
`ARCHITECTURE.md`. Accepted typed schemas cover `hello`, exact-empty `ready`,
`enumerate`, `stream_entry`, `read_request`, `read_reply`, `data_chunk`,
`entry_batch`, `complete`, `cancel`, `cancel_ack`, and `shutdown`; all twelve
message families now have typed codecs. The `stream_entry` payload is exactly
one canonical 8-byte little-endian opaque `source_token`; zero and every other
`uint64_t` value, including the all-ones value, are valid at the codec boundary.
Decoding alone grants no membership or source authority. The accepted
disconnected result bridge below establishes membership only after complete
catalog validation.

An `entry_batch` begins with a canonical little-endian `u16` count from 1
through 256. Every record contains, in order, a little-endian `u64` source
token, `u64` size, a `u16` path length from 1 through 4,096, and that many
printable ASCII bytes (`0x20` through `0x7e`). The 1 MiB frame ceiling still
applies. Its trusted cumulative policy caps remaining entries at 50,000,
remaining path bytes at 64 MiB, each entry at 8 GiB, and remaining declared
data at 32 GiB. Callers may use lower ceilings. Tokens must increase strictly
within a batch and relative to the trusted previous-token context; zero is a
valid first candidate. The codec does not generate, register, promote, retire,
or authorize tokens.

Decoding is allocation-free and two-pass: the entire payload and policy must
validate before capacity is checked and caller-owned entry storage is
populated. The result span aliases that storage, and each archive-path view
aliases the frame payload, so both backing stores must remain alive and
unchanged while in use. Printable archive spellings can include unsafe-looking
text and are not trusted normalized paths. Trusted code must normalize them,
reject traversal/absolute/conflicting layouts, validate aggregate catalog
claims across batches, and promote token membership before staging. Callers
advance cumulative policy and the previous token only after acceptance. An
empty batch is rejected; an empty enumeration uses a valid `complete` without
an entry batch.

A `data_chunk` is the opaque whole payload, with no prefix, offset, token, or
status field. It must contain 1 byte through 256 KiB and must not exceed the
trusted nonzero remaining-entry context supplied to its codec. The decoded
span aliases the frame payload and remains usable only while that storage stays
alive and unchanged. The caller owns the trusted remainder and decrements it
only after an accepted downstream write; the codec does not mutate or confer
that authority. The success-only `complete` payload is exactly four canonical
little-endian bytes: `u16 ProtocolStatus` followed by `u16 ProtocolPhase`. Its
trusted expected-operation context must be `enumerate` or `stream`; only `(ok,
complete)` is accepted in either context. Every other known pair and every pair
containing an unknown value is rejected, leaving failure-result representation
and authority deferred. The typed schemas validate complete frame and payload
shape, require full payload consumption, and enforce the applicable
source/read bounds, sequence matching, allowed reply statuses, reply-data
shape, entry-count/path/size/cumulative policy, token ordering, chunk-size,
remaining-entry, and completion-context bounds. The library is not a trusted
worker, sandbox, transport, payload extractor, or runtime import path. No
runtime target depends on it, and this protocol work authorizes no
proprietary extraction, completion-failure reporting, destination mutation, or
cache publication.

Commit `909edcc` adds the separate
`OpenHalfLife::media_parser_results` target as the trusted owner of candidate
and promoted result metadata. Its caller supplies a nonzero epoch unique to a
worker lifetime; the bridge combines that epoch with a monotonically increasing
enumeration sequence so a restart cannot revive an old catalog by reusing a
token and enumeration number. It copies every accepted archive path out of the
frame, advances cumulative quotas and token ordering only after typed and state
validation, and retains batches as candidates. On successful enumeration
completion it runs the existing payload-layout planner over the aggregate,
rejects invalid or conflicting normalized paths and count/size inconsistencies,
then atomically promotes the layout and a sorted source-token membership index.
Empty catalogs are valid promotions.

A stream request must present the exact promoted epoch/sequence generation and
a member token. The bridge derives the byte remainder from that catalog entry,
passes only bounded chunks to a supplied sink, decrements the remainder only
after a successful whole-chunk write, and accepts stream completion only at
zero. Cancellation retires catalog authority immediately and makes an active
candidate non-promotable while keeping only bounded crossing-frame validation
state. A read first observed after cancellation is validated and ordered but
returned ignored with zero actionable metadata and must not be serviced; a
reply remains legitimate only for a read that preceded cancellation. New
enumeration, cancellation acknowledgement, shutdown, any terminal protocol,
layout, sink, or allocation failure, trusted source invalidation, and worker
failure retire the relevant catalog, candidate, and stream bindings.

The result bridge is disconnected: neither the app, `ohl_media`, nor staging
links it. It creates no worker or transport, selects no component, opens no
destination, and owns no staging or publication operation.

Commit `c90f2d1` adds the disconnected
`OpenHalfLife::media_parser_reads` library above that result/session stack. It
retains the exact shared, pinned source capability from a valid
`ValidatedMedia`. Source size comes from its validated fingerprint and is
checked against the retained capability. `maximum_read_bytes` is trusted
constructor configuration that must exactly equal the accepted typed `hello`
value; the broker alone cannot verify that binding. The accepted parent
handshake below records the source-size and maximum-read values, not media
identity. Trusted later composition must use its exact limits and the same
`ValidatedMedia`. The broker accepts no raw path or substitute source. Typed
request decoding occurs before session observation. The broker,
not the worker, owns per-request read sequencing, resets sequencing for a new
top-level request, and applies request and cumulative reply-byte budgets before
touching the source.

Each serviceable read verifies source stability before reading and after the
read or read failure. Stable generic failures become canonical prefix-only
`source_read_failed` replies; observed change, lost range, or early EOF becomes
prefix-only `source_changed`; success contains exactly the bounded requested
bytes. Temporary scratch is scrubbed after reply encoding, including data left
by a partial failed read. `prepare()` returns a borrowed, caller-owned reply and
a unique ticket without observing a parent-to-worker reply. Only after a future
transport accepts the exact header and payload in full may
`commit_reply_sent()` consume that ticket, observe the reply, and advance the
sequence. Abandonment, stale tickets, committed source failure, terminal broker
failure, or destruction during an active session retires the associated result
session and its catalog authority.

A read already prepared before cancellation may cross normally, and remains
the sole drain candidate if `cancel_ack` overtakes it. A read first observed
after cancellation is validated and ordered but ignored without source access,
budget charge, output mutation, or reply ticket. The optional source-operation
table is a trusted deterministic-test seam only: all-null selects the native
capability operations and a non-null table must be complete. The broker passes
the same retained capability as each callback's source argument; it does not
mechanically restrict callback code's ambient process authority. Only trusted
project/test code may supply callbacks, and no worker or media field may
configure them.

This source-read broker creates no worker, sandbox, or transport, sends no
frame, and grants no runtime-import, raw-path, component-selection,
destination, staging, cache, or publication authority. Native worker isolation
and IPC plus explicit selection/staging composition are still required.

Commit `e4b819a` adds the disconnected
`OpenHalfLife::media_parser_transport` frame channel. It depends only on the
accepted parser protocol, platform worker interface, and `Threads::Threads`;
the app, runtime import, result bridge, source-read broker, staging, and
publication paths do not link it. Construction binds one nonzero session to a
complete trusted, non-owning exact-I/O operation table whose context and byte
channel must outlive the frame channel and all active operations. The supplied
adapter constructs an operation table whose callbacks directly forward to an
already-created `IsolatedWorker`; it grants no process launch, ownership,
termination, or reap authority.

Send and receive use a separate exact canonical 32-byte header transfer and,
for a nonempty bounded payload, a separate exact payload transfer. The same
deadline and cancellation token supplied by the caller are forwarded unchanged
to both stages. Send validates the protocol header, session binding, payload
ceiling, and declared length before emitting a header. Receive requires a
caller buffer large enough for the protocol maximum before consuming a header,
then decodes and validates that header and its exact session before reading the
declared bounded payload. A successful frame view aliases caller-owned receive
storage; outgoing and incoming backing storage must remain alive and unchanged
for their documented operation or view lifetime. If payload reading fails, a
partial untrusted prefix and stale suffix may remain, and the whole supplied
buffer is invalid as a frame until reinitialized.

One send may overlap one receive, but a duplicate active operation in either
direction is rejected without I/O. Any protocol or transport failure retains
the first terminal cause, invokes the trusted idempotent abort callback once to
wake active I/O, and prevents later I/O. Explicit abort has the same terminal,
one-shot behavior. A reported success with a short count, or any count greater
than the requested span, is sanitized to `io_failure`; no failed receive
returns a usable frame view. The operation table must support concurrent read
and write, prompt abort wakeup, and no re-entry or destruction of the channel.

Terminal `abort_io()` interrupts the trusted byte channel; it is not process
termination or reap authority. Trusted custom callbacks retain their ambient
process authority, so limiting callback suppliers to trusted project code is a
composition policy rather than mechanical confinement. The frame channel owns
no process launch, ownership, termination, reap, executable, pathname, source,
component-selection, staging, destination, publication, cache, catalog,
application, or runtime authority. Native worker composition remains a later
dependency: runtime wiring to the validated result, source-read, selection,
staging, and publication boundaries is still absent. The accepted local
evidence at
`e4b819a9efa37d5e401d111c4ac591365ce669ae` is a clean warnings-as-errors build
of 83/83 steps, full CTest at 34/34, 50 consecutive frame-channel and repository
policy passes, and the platform common-worker test at 1/1. No hosted result is
claimed for this commit.

Commit `13f0fb0` adds the disconnected
`OpenHalfLife::media_parser_handshake` library above the source-read broker and
frame transport, its only direct dependencies. It borrows exclusive access to
a fresh channel plus the `ValidatedMedia` and receive storage for one
synchronous parent handshake.
Source-read limits, protocol budgets, and deadline are copied values; the
copied cancellation token shares its source's state. No runtime target links
the library. Before I/O it rejects a terminal channel, invalid
or moved-from `ValidatedMedia`, a missing source, mismatch between the pinned
captured size and fingerprint, invalid source-read limits or protocol budgets,
budgets below two messages or 12 payload bytes, insufficient receive storage,
and an invalid derived policy or validator. A rejected nonterminal
configuration performs no channel I/O or abort and returns no proof.

The borrowed `ValidatedMedia` temporarily gives the handshake its pinned source
capability only so it can query the captured size. The handshake reads no source
bytes, and neither it nor the returned proof retains or grants that capability.

The canonical parent `hello` uses the channel's nonzero session and request
zero. Its exact 12-byte payload contains the fingerprint source size as a
little-endian `u64` and the trusted `maximum_read_bytes` as a little-endian
`u32`. The sent header is observed only after the exact header and payload have
been accepted by the channel. The worker response must decode as an exact-empty
typed `ready`; its header is observed only after that typed validation. Both
channel calls receive the same deadline and cancellation token unchanged.

Success returns a move-only, single-consumption proof holding an exactly idle
validator charged for two messages and 12 payload bytes, the exact source-read
limits, and the derived source policy. Taking the validator invalidates both
the proof and its containing result. Later trusted composition must move it into
a `ParserResultSession` and construct the `ParserSourceReadBroker` from the same
`ValidatedMedia` and exact limits, preserving the source-size and maximum-read
binding. The proof retains copies of the limits and derived policy, not media
identity, so it does not mechanically prove same-media use; that remains a
trusted composition requirement.

The caller's maximum-sized receive storage is not scrubbed, and no frame or
payload view escapes the result. Payload I/O or typed-ready failure may leave
an attacker-controlled partial or full prefix followed by stale prior bytes;
the whole buffer is invalid as a frame until reinitialized. A protocol or
channel failure after interaction returns no proof and terminally aborts the
channel. The frame channel preserves the first failure, sanitizes impossible
exact-I/O reports, and performs no later I/O.

This handshake accepts no source path, reads no source bytes, and neither it nor
the proof retains or grants the temporarily borrowed pinned-source capability
after the call; it has no worker or process launch, process ownership,
termination, reap, component-selection, staging, destination, publication,
cache, runtime-import, or application authority. It
does not create or own a worker and does not perform downstream session/broker,
selection, staging, publication, or app composition. The accepted commit is
`13f0fb08e7d00159000f3721ebe0b0e1b1481188`; local validation passed a clean
warnings-as-errors build at 87/87 and the full CTest suite at 35/35. No hosted
result is claimed for `13f0fb0`.

The earlier exact-SHA hosted build run `29147060407` at `ca576e9`, covering the
trusted result bridge and media cancellation migration, passed all 32 GNU 13
Linux tests plus the experimental, sanitizer, and Windows jobs. It validates
that disconnected result boundary across the required hosted platforms, not
`e4b819a` or the absent worker, source, staging, or runtime edges. The
tests-only `ca576e9` change did not trigger parser fuzzing; accepted
typed-protocol fuzz evidence at that point remained the separate earlier
`ba84cfc` run.

The source-read broker commit
`c90f2d1a7cbabdb90b688197d2d34ceb48526aeb` passed the complete local CTest
suite 33/33 with synthetic broker coverage. Exact-commit hosted build run
`29148133002` passed Linux x64, sanitizers, the experimental cabinet adapter,
Windows x64, and macOS Apple Silicon; this is the cross-platform broker
evidence. Fuzz run `29148132997` separately passed Clang 18/libFuzzer for the
typed protocol only and did not build or fuzz the broker. These results do not
establish any absent worker, transport, runtime, staging, or publication
authority.

Deterministic parser fuzz validation accepted at `81a7ee9` and extended at
`d59b6c5`, `f4d908a`, `c28ea9f`, `2d71079`, and `ba84cfc` exercises frame
decoding, generic payload reading, session ordering, and all twelve accepted
typed decoders. Entry-batch dispatch uses fixed 256-entry caller storage and
bounded broad, matching previous-token, replay-token, and reduced-count-budget
contexts. Its deterministic self-check reaches canonical and matching-token
acceptance plus replay, non-printable, and budget rejection. Unit validation
covers the exact wire order, all count/path/ASCII/token/size/cumulative/frame
boundaries, truncations, caller-storage capacities and lifetimes, multi-batch
ordering, and decode-before-observe atomicity. Read, data-chunk, and completion
dispatch retain their matching/mismatching context checks. The opt-in
libFuzzer target is exercised by a hosted smoke job that replays the fixed
project-authored synthetic corpus twice and checks that the seeds remain
unchanged. The fuzz target does not establish a trusted catalog, worker
transport, native isolation, extraction, or runtime integration.

- Give the worker read-only access only to the pinned source or bounded byte
  ranges; do not give it a destination path, cache authority, network access,
  credentials, or permission to execute media content.
- Validate and bound every IPC message, count, offset, size, nesting depth, and
  total byte budget on both sides of the boundary.
- Use opaque project-owned identifiers across IPC when an internal media name is
  not required by the trusted side.
- Keep parser traces and IPC captures disabled by default. If explicitly enabled
  for local diagnosis, classify and retain them as local proprietary data.
- Convert worker crashes and limit violations to sanitized project error codes;
  never attach a dump or raw parser transcript automatically.
- Validate all selected paths, declared sizes, streamed sizes, and conflicts in
  trusted project code before destination mutation.

Framing and header/state checks must never be treated as payload acceptance. A
production receiver must decode each message through its specific typed schema,
bound every field, count, length, and cumulative resource use, reject malformed
or noncanonical values, and prove that the decoder consumed the entire payload.
Only after that complete validation may a receiver use message content or
transition state. The twelve accepted typed decoders satisfy that payload rule
only for their own message types. The disconnected result bridge now supplies
the catalog generation, aggregate layout, membership, stream-remainder, and
downstream-write prerequisites described above, and it performs typed decoding
before protocol observation. The success-only `complete` pair still does not
prove that a worker or transport exists, that every required source read was
requested, or that component selection, staging, or publication succeeded. The
typed protocol, result bridge, source-read broker, and disconnected frame
channel must remain outside runtime import until native process isolation and
process launch, ownership, termination, and reap plus transport-to-worker,
selection, and staging composition are implemented and accepted.

Isolation limits parser authority; it does not make parser output safe to log,
commit, or trust without validation.

## Fixtures and fuzz reproducers

Permanent fixtures must be either independently authored synthetic data or data
whose public source and redistribution terms are recorded and compatible with
the project.

- Invent synthetic paths, names, records, and byte sequences for tests.
- Record the public specification or public fixture provenance used to create a
  parser or compatibility test.
- Do not transcribe a media-derived name, path, record, directory layout, or byte
  sequence into a fixture, assertion, comment, or test parameter.
- Generate shareable fuzz seeds and reproducers only from synthetic or approved
  public inputs. A reproducer minimized from user media remains proprietary and
  local.
- Reproduce private-media failures with a new synthetic input that exercises the
  same documented rule without retaining protected content.
- Submit any proposed media-derived literal for provenance review under
  `CLEAN_ROOM.md` before it enters a patch.

Public availability alone is not enough: provenance and license or permission
must allow the intended repository and redistribution use. When that cannot be
established, the fixture stays local and out of project artifacts.
