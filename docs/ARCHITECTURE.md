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
`read_request`, `read_reply`, `data_chunk`, `complete`, `cancel`, `cancel_ack`,
and `shutdown`. The `stream_entry` payload is exactly one canonical 8-byte
little-endian `source_token`. It is an opaque project-owned identifier; zero
and every other `uint64_t` value, including the all-ones value, are valid at
this codec boundary. Token membership and lifetime validation are deferred to
a future trusted owner and are not authorization to access a source. A
`data_chunk` is the opaque whole payload with no prefix, offset, token, or
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
destination, or publication authority. The only absent typed schema is
`entry_batch`. The target does not implement or authorize worker creation,
process isolation, source access, component selection, payload extraction,
destination mutation, or cache publication.

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
Before any production state transition or use of message content, a
message-specific typed decoder must apply explicit bounds to every payload
field, count, and length, reject noncanonical values, and require complete
payload consumption. The eleven accepted decoders provide that validation for
`hello`, `ready`, `enumerate`, `stream_entry`, `read_request`, `read_reply`,
`data_chunk`, `complete`, `cancel`, `cancel_ack`, and `shutdown`; they are not
wired to production state transitions. In particular, decoding a
`stream_entry` token does not establish its membership, lifetime, or authority,
and accepting a `data_chunk` does not identify an entry or update its trusted
remainder. A future receiver must validate `complete` before offering its
header to the state validator; it must also establish every operation-specific
read, result, remainder, and downstream-write prerequisite before treating the
message as success. The remaining typed schema, generic codec, and header/state
infrastructure must remain disconnected from runtime import until the full
message set and worker-isolation requirements in `MEDIA_IMPORT.md` are
implemented and accepted.

Deterministic parser fuzz validation was accepted at `81a7ee9`; its typed
dispatch was extended at `d59b6c5`, for `stream_entry` at `f4d908a`, and for
`data_chunk` at `c28ea9f`, then for `complete` at `2d71079`. The opt-in
libFuzzer target exercises bounded frame decoding, generic payload reading,
session ordering, and all eleven accepted typed decoders. Read-message dispatch
uses bounded matching and deliberately mismatching contexts. Data-chunk
dispatch selects bounded, independently reachable contexts for the exact
payload remainder, a smaller remainder, and zero remainder without allocating
or copying the frame payload. Complete dispatch selects both valid operation
contexts and an invalid context while arbitrary payloads reach disallowed
pairs. Its deterministic self-check proves both valid contexts, disallowed
status and phase pairs, and the invalid context. Unit validation exhausts all
ten known statuses by all five known phases in both valid contexts and checks
that typed rejection occurs before state observation. The hosted smoke job
replays the fixed project-authored synthetic corpus twice and verifies that the
seeds are not mutated. This is validation of the protocol infrastructure, not
evidence of a worker transport, native isolation, runtime wiring, or coverage
for the sole schema that remains absent.

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
opaque token, and the staging stop token. It wraps the caller's byte sink,
checks cancellation before source dispatch, before each sink write, and after
source return, rejects chunks that exceed the declared size before they reach
that sink, requires an exact final byte count, and reports source, destination,
overflow, underflow, and cancellation failures separately.

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
remains absent and still requires accepted native backends on every supported
platform.
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

The intended gameplay/rendering graph remains under design. Each new edge must
be expressed explicitly with `target_link_libraries` so CMake remains the
source of truth and cycles can be rejected in review.
