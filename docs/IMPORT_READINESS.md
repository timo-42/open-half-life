# Production import readiness

Production payload import is unavailable on every platform. No current build
can extract a user medium into a playable payload set, publish that payload for
runtime use, or claim end-to-end production qualification.

This page records release evidence that must exist before that status changes.
Checklist items are unmet unless a concrete review, test, hosted run, or
release artifact is linked from the item. Absence of a link means absence of
evidence.

## Status boundaries

- Build availability means the relevant source can configure, compile, and run
  its implemented tests on a host tuple. It does not imply payload import.
- App preflight and metadata-only cache mean the app can acquire a user ISO
  once, validate the pinned source, mount the read-only UDF root, and publish a
  local provenance manifest without media bytes or extracted output. It does
  not imply payload extraction.
- Isolated-worker containment means a native backend can launch a fixed worker
  identity with reduced authority and bounded IPC. Linux x86-64 has a
  source-selected containment backend with synthetic tests and an installed
  minimal production worker artifact that attests readiness and services only
  process/channel lifecycle. It still lacks parser/import semantics, a
  lifecycle coordinator, a composed real payload dispatcher/parser,
  deterministic component selection, runtime staging/publication integration,
  and production qualification.
- Parser-worker service availability means the private, non-installed static
  service can drive a synthetic worker-side OWP/1 session over injected
  transport and dispatcher callbacks. It is disconnected from the application,
  native worker, and import stack and therefore does not imply containment,
  payload parsing, extraction, publication, or production import.
- Production end-to-end qualification means a supported platform tuple can
  perform the complete import path from pinned source through contained parser,
  deterministic selection, trusted staging, no-replace publication, runtime
  discovery, cancellation/failure cleanup, and review-approved release gates.
  This is absent on every platform.

## Platform matrix

| Platform | Build | App preflight and metadata-only cache | Isolated-worker containment | Production end-to-end qualification |
| --- | --- | --- | --- | --- |
| Linux x86-64 | Implemented. Existing Linux build evidence is not a production import tuple. | Implemented; no payload extraction. | Implemented as a source-selected native backend with project-authored synthetic tests. A minimal installed production worker artifact covers packaging and process/channel lifecycle only; the private parser-worker service is not linked into it, and a real dispatcher/parser, orchestration, runtime selection, and staging/publication integration remain absent. | Absent; import unavailable. |
| Linux other architectures | Unevidenced and unqualified as import tuples. | Code path exists where the build is available; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; import unavailable. |
| Windows x64 | Exact documented build/preflight tuple. | Implemented in hosted evidence; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; import unavailable. |
| Windows other architectures | Unevidenced and unqualified. | Unevidenced for release qualification. | Unsupported; CMake selects the unsupported backend. | Absent; import unavailable. |
| macOS Apple Silicon | Exact documented build/preflight tuple. | Implemented in hosted evidence; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; import unavailable. |
| macOS other architectures | Unevidenced and unqualified. | Unevidenced for release qualification. | Unsupported; CMake selects the unsupported backend. | Absent; import unavailable. |

Platform-independent staging and the Linux atomic-directory store are
implemented but disconnected from the application and parser stack. They do
not change any production-import status in this matrix.

## Current disconnected parser-worker service

`OpenHalfLife::parser_worker_service` is an internal static library whose only
project dependency is `OpenHalfLife::parser`. It is not installed and exposes
no supported public API. It drives bounded worker-side protocol mechanics using
trusted project-supplied callback tables and caller-owned scratch buffers, but
does not select a real payload parser or compose with a native worker.

The trusted parent retains the pinned source capability and sole authority over
result acceptance, component selection, destinations, staging, and
publication. The worker and its output remain untrusted at that boundary and
receive none of those authorities. Synthetic local evidence passed the focused
service test 1/1, the development suite 39/39, and the ASan plus UBSan suite
40/40. That evidence does not cover a service-bearing bootstrap, a real
dispatcher/parser, native containment composition, proprietary media,
extraction, publication, supported-host qualification, or end-to-end import.

The next dependency is a separately reviewed worker bootstrap and real
dispatcher/parser, followed by native lifecycle/runtime composition. Production
payload import remains unavailable on every platform, and M2 remains in
progress.

## Input and fixture provenance

Release evidence may use only independently authored synthetic inputs or public
redistributable inputs with recorded provenance and compatible use terms. Do
not use proprietary or media-derived fixtures, corpora, minimized reproducers,
identifiers, path literals, archive keys, hashes of internal records, or raw
diagnostic output as committed evidence. A privately owned medium may be used
only for local manual investigation under [MEDIA_IMPORT.md](MEDIA_IMPORT.md);
observations from it do not become redistributable tests, identifiers, or
release gates.

## Objective release-evidence gates

Every gate below is required before production payload import can be announced
for any platform tuple. A gate is unmet until it links to evidence for the
exact tuple and feature surface being claimed.

- Installed package inventory and identity: the release artifact records every
  installed executable, library, data directory, worker path, owner/mode or
  signature identity, and package digest. Runtime launch rejects missing,
  mismatched, mutable, or unexpected worker identities before any source read.
- Installed-prefix end-to-end happy path: an installed package, not a build tree,
  performs a synthetic complete import from pinned source through contained
  parser, deterministic selection, staging, no-replace publication, runtime
  discovery, and cleanup. Evidence records the exact command, input provenance,
  exact expected published tree and cryptographic digests, sanitized stdout and
  stderr or structured diagnostics, exit status, post-run directory inventory,
  no staging residue, and no live or zombie child.
- Source pinning/no reopen: the complete import uses one pinned read-only
  source capability and never reopens, canonicalizes, or delegates the original
  path after acquisition. Capability probes fail closed; missing kernel,
  filesystem, sandbox, permission, or identity features skip no checks and
  produce stable unsupported or unavailable categories.
- Worker minimal authority: the parser worker has no raw-path, destination,
  cache, environment-selection, recipe-selection, or publication authority, and
  tests verify that attempts to acquire those authorities fail closed.
- Trusted private destination root/staging: publication writes only below an
  explicitly trusted private root, uses create-new private staging, rejects
  hostile roots, documents supported filesystem semantics, and records
  directory-sync uncertainty without upgrading it to a durability claim.
- Typed bounded IPC: every parser message is typed, bounded, sequence-checked,
  budgeted, and fail-closed before any source read, sink write, state
  transition, or publication decision. Malformed IPC, truncated exact I/O,
  replay, over-budget, out-of-order, and peer-close cases map to stable
  sanitized categories.
- Phase-specific failure, cancel, and timeout behavior: acquisition,
  validation, worker launch, handshake, enumeration, read, staging, sealing,
  final verification, publication, runtime discovery, cleanup, shutdown, and
  reap each have tests for failure, cancellation, timeout, malformed input,
  worker crash, worker hang, and resource exhaustion where applicable. Evidence
  records elapsed-time bounds, publication outcome, cleanup or preservation
  outcome, IPC close, termination, and reap assertions.
- Final complete-source verification and no-replace publication: the full
  source is verified immediately before publication, and the next store
  operation is a no-replace publish that never overwrites an existing payload.
  Same-size mutation is not accepted by structural probes alone; final
  cryptographic verification must detect it.
- Crash, restart, and publication recovery: interrupted imports classify
  orphaned staging and published directories, preserve or clean them according
  to a documented policy, retry idempotently, and exercise parent crash and
  power-loss boundaries, parent-directory sync uncertainty, and concurrent
  matching and conflicting publication races. Recovery never overwrites or
  trusts ambiguous state and preserves unrelated staging and valid published
  trees.
- Cancellation, cleanup, termination, and reap: cancellation and all handled
  failures clean or isolate owned staging, close IPC, terminate on failed or
  timed-out shutdown, and reap the worker without abandoning a live child.
- Hostile-worker and unavailable-capability failures: tests prove malicious,
  malformed, silent, crashing, slow, over-budget, resource-exhausting, and
  unavailable workers fail closed without source, destination, cache, or
  publication authority.
- Sanitizer, fuzz, stress, and platform evidence: parser, protocol, staging,
  store, worker lifecycle, cancellation, cleanup, recovery, packaging, and
  installed-prefix import have auditable sanitizer, fuzz, stress, race,
  crash-restart, and platform evidence using allowed inputs only. Each record
  names the sanitizer set; fuzz targets, corpus provenance, duration, and final
  findings; stress repetitions or duration; race, leak, descriptor, process,
  memory, and disk-growth limits; and pass/fail counts. Applicable sanitizer
  jobs complete cleanly with zero unresolved sanitizer, race, leak, or crash
  findings; every required fuzz target completes its specified campaign without
  unresolved crash or hang; and stress and resource measurements remain within
  the stated limits. Exceptions are allowed only when documented and
  independently approved. Required jobs permit no unsupported skips or fallback
  backends.
- Explicit supported tuples: release notes state the exact supported operating
  systems, architectures, kernel and confinement features, filesystems, package
  formats, worker installation contract, unsupported cases, and unevidenced
  tuples. Evidence identifies the exact environment and release artifact tested.
- Independent architecture, security, reliability, release, and product review:
  separate named roles approve the architecture boundary, confinement model,
  failure and recovery behavior, installed package inventory, user-facing
  product behavior, and evidence links. Approval records are bound to exact
  source SHAs and package digests, and any source, dependency, build recipe,
  package, installer, capability contract, recovery-policy, or evidence change
  invalidates the affected approvals until renewed.

## Unresolved product choices

The following remain unresolved and must not be implied by implementation
details or documentation wording:

- supported operating-system, architecture, filesystem, and package-contract
  tuples;
- recipe format, component-selection policy, and selection UX;
- rollout, rollback, and compatibility policy for imported payload versions;
- diagnostics, sanitized aggregate reporting, and support-bundle contents; and
- recovery semantics for interrupted imports, retained staging, cache
  conflicts, and publication races.

Resolving those choices requires explicit product and engineering review. This
document deliberately does not select them. Production qualification is
impossible until recovery semantics are decided, documented, implemented, and
evidenced for the exact tuple and package being claimed.
