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
staging stop token. The local version-2 stage identity also binds a non-empty
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

## Parser worker isolation

Untrusted container and filesystem metadata must be handled by a bounded parser
boundary. Where a parser is not independently hardened, run it in a constrained
worker process before allowing it to feed production import.

The current `parser` library is accepted as bounded protocol infrastructure
only. It provides canonical OWP/1 framing, generic bounded primitive payload
helpers, frame and cumulative budgets, and fail-closed session ordering,
including the cancellation and one-shot late-reply drain rules documented in
`ARCHITECTURE.md`. Accepted typed schemas cover `hello`, exact-empty `ready`,
`enumerate`, `cancel`, `cancel_ack`, and `shutdown`, plus `stream_entry`,
`read_request`, and `read_reply`. The `stream_entry` payload is exactly one
canonical 8-byte little-endian opaque `source_token`; zero and every other
`uint64_t` value, including the all-ones value, are valid at the codec boundary.
Membership and lifetime checks remain the responsibility of a future trusted
token owner, and decoding the token grants no source authority. The typed
schemas validate complete frame and payload shape, require full payload
consumption, and enforce the applicable source/read bounds, sequence matching,
allowed reply statuses, and reply-data shape. Typed schemas for `entry_batch`,
`data_chunk`, and `complete` remain absent. The library is not a worker,
sandbox, transport, payload extractor, or runtime import path. No runtime
target depends on it, and this protocol work authorizes no proprietary
extraction.

Deterministic parser fuzz validation accepted at `81a7ee9` and extended at
`d59b6c5` and `f4d908a` exercises frame decoding, generic payload reading,
session ordering, and all nine accepted typed decoders, including typed
`stream_entry` dispatch, with bounded matching and deliberately mismatching
read contexts. A deterministic self-check establishes canonical
read-request/read-reply decode reachability and both context branches. Its
opt-in libFuzzer target is exercised by a hosted smoke job that replays the
fixed project-authored synthetic corpus twice and checks that the seeds remain
unchanged. The fuzz target does not establish worker transport, native
isolation, extraction, runtime integration, or coverage for the three schemas
that remain absent.

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
Only after that complete validation may the receiver use message content or
transition production session state. The nine accepted typed decoders satisfy
that payload rule only for their own message types; they are not connected to
runtime state transitions. In particular, a decoded `stream_entry` token has
not been checked for membership or lifetime and conveys no authority. The
remaining message families, generic payload helpers, and header/state validator
must remain outside runtime import until the complete typed boundary and
process-isolation requirements above are implemented and accepted.

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
