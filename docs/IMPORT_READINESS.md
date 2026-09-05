# Production import readiness

Linux x86-64 can now extract a user medium into a payload set and publish it
for runtime use. It is **not** production-qualified, and every other platform
tuple remains unavailable: no build on any other tuple can extract a medium at
all, and no tuple on any platform meets the release-evidence gates below.

This page records release evidence that must exist before that status changes.
Checklist items are unmet unless a concrete review, test, hosted run, or
release artifact is linked from the item. Absence of a link means absence of
evidence.

**Read the R4.7b section first.** Everything below the R4.7a section
predates the worker's real dispatcher and describes a build that could not
extract anything. It is retained as the specification and the historical
qualification record; where it says import is unavailable on every platform,
read "unavailable on every platform except Linux x86-64, which is implemented
but unqualified".

**Rust transition note.** The project's implementation is now entirely Rust;
the C++ tree this page originally described (parser-worker service, Linux
B1 bootstrap, process-session owner, staging/store backend) was removed at
Rust M1 parity. None of that worker/staging surface has been ported to Rust
yet — it is still-ahead work (`ohl-parser-worker-service`, `ohl-parser-worker`,
`ohl-cabinet*`, `ohl-import`; see `docs/MILESTONES.md`'s M2 entry). The
detailed narrative and evidence links below this note describe that removed
C++ implementation and remain in this file only as the specification and
historical qualification record the Rust port must reproduce or exceed. The
bottom-line conclusion is unchanged by the language migration: **production
payload import is unavailable on every platform, in both the historical C++
build and the current Rust build.** Rust has since ported the parser
protocol, the worker service and its installed freestanding image, the
parent-side session stack, the payload staging boundary, and — at R4.7a, see
the next section — the composition that joins them (`ohl-parser-protocol`,
`ohl-parser-worker-service`, `ohl-parser-worker`, `ohl-import`,
`ohl-payload`, `ohl-app`). The one thing that composition still cannot do is
the thing this page is about: the worker has no real dispatcher, so nothing
is ever extracted or published.

## Rust R4.7a: the parent-side pipeline is composed, import is still unavailable

The Rust parent side is now composed end to end, up to the point where the
worker refuses. `ohl_import::pipeline::run_import` locates one container in a
mounted medium, hands a confined `IsolatedWorker` a bounded window over that
container, drives the handshake and enumeration, applies the user's
runtime-only recipe, plans a layout, streams every planned entry through
`ohl_payload`'s create-new staging, reverifies the complete pinned source, and
publishes once with no-replace. `ohl-app` runs it after its M1 flow.

This changes no row below. **Production payload import remains unavailable on
every platform**, because the installed worker's compile-fixed dispatcher
answers `unsupported` for every enumeration and stream. No build can extract a
user medium, and no payload has ever been published from one.

Evidence, all local and all on Linux x86-64:

- The composed pipeline publishes a payload only when a worker actually
  enumerates and streams, which today only the crate's scripted synthetic
  worker does (`crates/ohl-import/tests/pipeline.rs`): a complete run, a
  repeat run that is a cache hit, a cancellation mid-stream that discards the
  stage and terminates the worker once, a transport failure, a refusal, and a
  medium with no container that never starts a worker.
- Container location is covered against project-authored synthetic PE images
  and synthetic ISOs (`crates/ohl-import/tests/locate.rs`): a two-section PE
  with a Z-signature overlay, a PE with no overlay, truncated and malformed
  headers, an impossible section count, a cabinet at offset 0, an
  under-minimum file, a plain file, an exhausted read budget, and an observed
  cancellation.
- The `#[ignore]`d `crates/ohl-app/tests/worker_image.rs` runs the shipped
  binary against a synthetic ISO with the **real** installed worker image
  (after `cargo xtask worker-image`) and requires the sanitized `unsupported`
  outcome, exit status 0, and nothing published.
- One manual run against a lawfully owned medium reached the confined worker
  and was refused. Only the fixed log lines and the exit status were recorded;
  no name, count, or byte from that medium is in this repository.

The real dispatcher that finishes this arrived at R4.7b; see the next section.

## Rust R4.7b: Linux x86-64 imports end to end; no tuple is qualified

The worker now has a real dispatcher. `ohl-parser-backends` recognises a Wise
installer overlay, a Microsoft cabinet or an InstallShield 3 Z archive from
the source window the parent pins over the container, enumerates it into
`entry_batch` frames and streams each entry as verified `data_chunk`s, all
over the OWP/1 pull model with no capability of its own. `ohl-app` runs the
import on first run and rediscovers the published tree afterwards. The whole
path — pinned source, contained parser, selection, staging, complete-source
reverification, no-replace publication, provenance, runtime discovery — now
executes for real on Linux x86-64.

**This is an implementation status, not a qualification status.** Every
objective release-evidence gate listed further down remains unmet: there is no
installed-package inventory, no installed-prefix hosted end-to-end run, no
crash/restart/publication-recovery evidence, no sanitizer/fuzz/stress campaign
over the new back ends, and no independent architecture, security, reliability,
release, or product review. The "production end-to-end qualification" column
below therefore stays "absent" for every tuple, Linux x86-64 included.

Every other tuple is unavailable for a simpler reason: containment is
source-selected for Linux x86-64 only, and everywhere else the unsupported
backend refuses to launch a worker, so no import can begin.

Evidence, all local and all on Linux x86-64:

- Back-end coverage runs the real worker service over a scripted transport,
  against synthetic Wise, cabinet and Z packages built by the decoders' own
  project-authored writers (`crates/ohl-parser-backends/tests/service.rs`):
  enumerate/stream/complete for each container kind, a checksum-mismatched
  Wise stream that is withheld from the offer rather than failed mid-stream, a
  stream whose source bytes change after enumeration and is refused at
  verification instead of published, a cancel between chunks, a source-read
  failure, an exhausted walk budget, and an unrecognised container.
- Pipeline coverage against the scripted synthetic worker is unchanged
  (`crates/ohl-import/tests/pipeline.rs`), extended for the new
  `WiseOverlay` candidate kind, the `Wise > Z > largest cabinet` choice, and
  runtime discovery of an already-published tree
  (`crates/ohl-import/tests/locate.rs`).
- The `#[ignore]`d `crates/ohl-app/tests/worker_image.rs` runs the shipped
  binary with the **real** installed worker image (after
  `cargo xtask worker-image`) against a synthetic ISO carrying a synthetic
  PE with a Wise overlay, and verifies the published tree's CRCs; a second
  case still requires the sanitized refusal for a container the build cannot
  decode. Both pass.
- One manual run against a lawfully owned medium completed the import, and a
  second run reused the published tree. Only the fixed log lines, the exit
  status, the elapsed time and aggregate counts were recorded; no name, path,
  or byte from that medium is in this repository.

The worker image's heap is a fixed 96 MiB `.bss` bump arena installed as the
`#[global_allocator]`, well inside the launcher's `RLIMIT_DATA` (256 MiB) and
`RLIMIT_AS` (512 MiB). It never calls `brk` or `mmap`, never reclaims, and
panics into a fail-closed exit when exhausted. It is the image's only new
`unsafe` site and is documented as row 6 of that package's `unsafe` inventory;
every other crate keeps `forbid(unsafe_code)`. See
[MEDIA_IMPORT.md](MEDIA_IMPORT.md), "The worker's container back ends
(R4.7b)".

## Status boundaries

- Build availability means the relevant source can configure, compile, and run
  its implemented tests on a host tuple. It does not imply payload import.
- App preflight and metadata-only cache mean the app can acquire a user ISO
  once, validate the pinned source, mount the read-only UDF root, and publish a
  local provenance manifest without media bytes or extracted output. It does
  not imply payload extraction.
- Payload import implemented means the tuple can actually run the whole path —
  locate, contained parse, selection, staging, complete-source reverification,
  no-replace publication, provenance, runtime discovery — against a real
  medium. It carries no qualification claim whatsoever: see the last column.
- Isolated-worker containment means a native backend can launch a fixed worker
  identity with reduced authority and bounded IPC. Linux x86-64 has a
  source-selected containment backend with synthetic tests and an installed
  production worker artifact that attests readiness and hosts one bounded OWP/1
  service lifetime over fd 3. Its compile-fixed dispatcher rejects payload
  operations as unsupported, so it still lacks real parser/import semantics,
  deterministic component selection, runtime staging/publication integration,
  and production qualification. A higher parent process-session owner now
  exists as the disconnected `OpenHalfLife::media_parser_process_session`
  library (`ParserProcessSession`, accepted at `537c11b`,
  [PR #12](https://github.com/timo-42/open-half-life/pull/12)), but it is not
  composed with the worker, so this does not change any status below.
- Parser-worker service availability means the private, non-installed static
  service can drive a synthetic worker-side OWP/1 session over injected
  transport and dispatcher callbacks. Linux x86-64 also links a private
  freestanding copy of that implementation into its contained installed worker.
  The Rust port provides the same service as the `ohl-parser-worker-service`
  crate over the `ohl-parser-protocol` crate, and `ohl-parser-worker` builds
  and installs the freestanding `ohl-media-parser-worker` image that hosts one
  lifetime with the compile-fixed unsupported dispatcher. The service remains
  disconnected from the application and import stack, and no form implies
  payload parsing, extraction, publication, or production import.
- Production end-to-end qualification means a supported platform tuple can
  perform the complete import path from pinned source through contained parser,
  deterministic selection, trusted staging, no-replace publication, runtime
  discovery, cancellation/failure cleanup, and review-approved release gates.
  This is absent on every platform.

## Platform matrix

| Platform | Build | App preflight and metadata-only cache | Isolated-worker containment | Payload import implemented | Production end-to-end qualification |
| --- | --- | --- | --- | --- | --- |
| Linux x86-64 | Implemented. Existing Linux build evidence is not a production import tuple. | Implemented; no payload extraction. | Implemented as a source-selected native backend with project-authored synthetic tests. The installed static worker hosts exact OWP/1 hello/ready/shutdown on fd 3 under the compile-fixed identity and native confinement. Its dispatcher decodes Wise, MS-CAB and IS3 Z containers for real (R4.7b), and the process-session owner, runtime selection and staging/publication are composed with it. | Implemented end to end (R4.7b): Wise overlay, MS-CAB and IS3 Z containers enumerate and stream through the confined worker, and a published payload is rediscovered at runtime. Local synthetic and manual evidence only. | Absent; implemented but unqualified — no release-evidence gate below is met. |
| Linux other architectures | Unevidenced and unqualified as import tuples. | Code path exists where the build is available; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; containment selects the unsupported backend, so no worker launches. | Absent; import unavailable. |
| Windows x64 | Exact documented build/preflight tuple. | Implemented in hosted evidence; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; containment selects the unsupported backend, so no worker launches. | Absent; import unavailable. |
| Windows other architectures | Unevidenced and unqualified. | Unevidenced for release qualification. | Unsupported; CMake selects the unsupported backend. | Absent; containment selects the unsupported backend, so no worker launches. | Absent; import unavailable. |
| macOS Apple Silicon | Exact documented build/preflight tuple. | Implemented in hosted evidence; no payload extraction. | Unsupported; CMake selects the unsupported backend. | Absent; containment selects the unsupported backend, so no worker launches. | Absent; import unavailable. |
| macOS other architectures | Unevidenced and unqualified. | Unevidenced for release qualification. | Unsupported; CMake selects the unsupported backend. | Absent; containment selects the unsupported backend, so no worker launches. | Absent; import unavailable. |

Platform-independent staging and the Linux atomic-directory store are
implemented but disconnected from the application and parser stack. They do
not change any production-import status in this matrix.

## Current parser-worker service and Linux bootstrap

`OpenHalfLife::parser_worker_service` is an internal static library whose only
project dependency is `OpenHalfLife::parser`. It is not installed and exposes
no supported public API. It drives bounded worker-side protocol mechanics using
trusted project-supplied callback tables and caller-owned scratch buffers, but
does not select a real payload parser. The Linux x86-64 installed worker now
links a separate private freestanding runtime built from the same protocol and
service implementation; the application and import stack remain disconnected.
The Rust equivalents are the `ohl-parser-worker-service` crate and the
`ohl-parser-worker` crate, whose `ohl-media-parser-worker` image is installed
at `libexec/open-half-life/` beside the launching executable and answers every
payload request with `unsupported`; production import stays unavailable.

The trusted parent retains the pinned source capability and sole authority over
result acceptance, component selection, destinations, staging, and
publication. The worker and its output remain untrusted at that boundary and
receive none of those authorities. Initial local synthetic evidence passed the
focused service test 1/1, the development suite 39/39, and the ASan plus UBSan
suite 40/40.

[PR #7](https://github.com/timo-42/open-half-life/pull/7) accepted the P1
service at reviewed head
[`3ec70b34f461ec7dddb1ca26770544df6debfe0f`](https://github.com/timo-42/open-half-life/commit/3ec70b34f461ec7dddb1ca26770544df6debfe0f),
then rebased it onto `main` as
[`6b3df8f1cf6660eed46246790bff382c6c4001b6`](https://github.com/timo-42/open-half-life/commit/6b3df8f1cf6660eed46246790bff382c6c4001b6).
The two commits have the same exact tree,
`888bee1be57b45c7583fe05bcf22698725f5f651`. All 12 hosted jobs in the PR
rollup passed: the five-job Build matrix and one parser-fuzz smoke job each ran
for push and pull-request events. Build runs
[`29195613360`](https://github.com/timo-42/open-half-life/actions/runs/29195613360)
and
[`29195614365`](https://github.com/timo-42/open-half-life/actions/runs/29195614365)
include the service test on Linux x64, sanitizers, the experimental Linux
configuration, Windows x64, and macOS Apple Silicon; fuzz runs
[`29195613343`](https://github.com/timo-42/open-half-life/actions/runs/29195613343)
and
[`29195614314`](https://github.com/timo-42/open-half-life/actions/runs/29195614314)
cover the existing typed protocol only. The final test-only change replaced an
oversized fixed stack buffer with payload-sized dynamic storage so the Windows
x64 synthetic test could run; production code and the private service contract
were unchanged.

This accepted hosted evidence qualifies the private disconnected service
boundary on the tested hosts. It predates and does not qualify the later B1
Linux bootstrap, a real dispatcher/parser, proprietary media, extraction,
publication, or end-to-end import.

The B1 bootstrap is Linux x86-64-only. The installed static worker emits and
closes the exact readiness record on fd 4, then hosts a single bounded OWP/1
lifetime over fd 3. It accepts canonical `hello`, emits exact-empty `ready`, and
supports protocol shutdown and orderly peer closure. Its project-authored,
compile-fixed dispatcher returns `unsupported` for enumeration or streaming;
media cannot select or configure callbacks, and no payload parser or source-read
implementation is present. Protocol, unsupported-operation, transport, and
internal outcomes become sanitized child exits and the public lifecycle's
bounded exit categories.

`platform.isolated_worker.linux` verifies the exact production worker bytes
through the public native launcher, so launch success crosses the compile-fixed
static ELF identity checks, inherited nonblocking channel, resource limits,
no-new-privileges, Landlock, seccomp, readiness EOF, and pidfd lifecycle. Its
synthetic cases cover fragmented hello/ready/shutdown and clean reap, malformed
protocol failure, unsupported enumeration, IPC peer closure, idempotent close,
cached waits, and owned termination/reap. Direct service and installed-artifact
tests add truncated I/O, clean peer-close, non-writable/non-set-id static-image,
payload-arena, readiness, and exact fd-3 lifecycle coverage. This is local
Linux x86-64 bootstrap evidence only, not cross-platform or production-import
qualification.
Final B1 validation passed the focused bootstrap set 4/4, the full development
suite 40/40, and 50 repeated real-launcher cases 50/50. Owned termination can
classify as `clean` or `terminated` if orderly peer EOF wins; either result is
terminal, cached, and reaped.

That dispatcher, and the composition of the process-session owner with
handshake/session, selection, staging, publication and runtime discovery,
landed at R4.7a and R4.7b. The next dependencies are the objective
release-evidence gates below — installed-package inventory and identity,
installed-prefix hosted end-to-end runs, crash/restart/publication recovery,
sanitizer/fuzz/stress campaigns over the new back ends, and independent
review — plus containment backends for the other platform tuples. Production
payload import remains unqualified on every platform and unavailable on every
tuple except Linux x86-64, and M2 remains in progress.

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
