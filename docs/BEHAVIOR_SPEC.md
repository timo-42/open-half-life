# Half-Life behavior specification: clean-room first pass

Investigation date: 2026-10-16. Scope: original Half-Life single-player
behavior, using public documentation and eventual black-box observations.
This is a research baseline, not a claim of retail compatibility. No original
runtime was executed, and no original code was disassembled or decompiled.

The later network-query investigation and its separate original-server
reference runs are recorded in [GOLDSRC_PROTOCOL.md](GOLDSRC_PROTOCOL.md).
Those runs do not change the gameplay-observation status of this first pass.

The rules in [CLEAN_ROOM.md](CLEAN_ROOM.md) and
[MEDIA_IMPORT.md](MEDIA_IMPORT.md) apply to this investigation. Game media,
recordings, saves, console captures, and tool responses containing private
identifiers stay local. This document contains no media-derived asset names,
file listings, or implementation recovered from the game.

## Evidence and target identity

Use these evidence states independently of implementation status:

- **Documented**: a public prose source describes the behavior. Its
  applicability to the selected retail version still needs observation.
- **Observed**: repeatable black-box runs of an identified reference runtime
  support a bounded result. There are **zero** such runs in this first pass.
- **Project choice**: an implementation or experiment design decision, not an
  assertion about the original game.
- **Unresolved**: evidence is missing or conflicting. An existing project
  constant or passing project test does not resolve it.

The user identified the source as a Game of the Year Edition ISO. Two complete
SHA-256 reads through the same read-only, no-follow file descriptor agreed;
file identity, size, modification time, and change time were unchanged across
the reads. This checks the bytes read, not the authenticity or runtime version
of the medium.

| Field | Result |
| --- | --- |
| Complete-source size | 398,952,448 bytes |
| Complete-source SHA-256 | `15d882b14b3479501ce2c89c454d9700c3128c0a16b5d7b37d46c7edd27259dc` |
| Read-only filesystem check | ISO 9660 with Joliet; existing mount smoke test passed |
| Original executable/build identity | Unverified |
| Original-runtime behavior observations | None |
| Engine-code analysis | None |

This source differs from the image recorded in
[ISO_REPORT.md](ISO_REPORT.md), whose size is 389,873,664 bytes and whose
complete-source digest is different. That report's layout, import results,
and user-supplied build label must not be attributed to this source.

Do not substitute a current Steam build for the retail reference without
recording the change. Valve's [25th anniversary change notes][D5] explicitly
describe changes to physics, controls, saves, and rendering. They establish
that versions differ; they do not establish this ISO's behavior.

## What REA established

[REA][R1] 6.5.0 was available through its CLI. The read-only
`inspect-artifact` command was attempted against the supplied ISO. It returned
`target_unavailable`, category `unsupported_target`, with the message
“Cannot open artifact: unsupported binary format.” No artifact graph or
successful analysis Evidence record was returned. This is an unsupported
REA input, not a diagnosis that the medium is corrupt.

The source digest above was computed separately; it is **not** a REA finding.
The project's existing read-only mount smoke test passed against this source
at repository commit `f574c47b058dad043f29189bb51bde9fb04f5693`, identifying
ISO 9660 with Joliet. This establishes a mountable outer filesystem, not
successful payload import. Neither operation observes gameplay.

REA's [CLI and Evidence guide][R2] documents native analysis through a
disassembler. That route is outside this clean-room investigation. Having a
working native backend does not make its output an allowed behavior source.
Also, REA's advertised native UI scenario and native-call observation provider
was unavailable on this Linux host. No supported REA capture of an original
Half-Life runtime was established here.

For a repeatable media check, set `OHL_TEST_ISO` locally and run from the
repository root:

```sh
: "${OHL_TEST_ISO:?Set this to the absolute path of your ISO}"
OHL_TEST_ISO="$OHL_TEST_ISO" cargo test -p ohl-vfs --test mount \
  manual_mount_aggregates_over_a_real_iso \
  -- --ignored --exact --nocapture
```

The test prints only filesystem classification and bounded aggregates. It
does not extract or execute the game. Do not run the application's normal
import path merely to reproduce this check.

To reproduce the REA limitation, set `OHL_TEST_ISO` to the same local file and
choose an absolute, private `REA_RESEARCH_DIR` outside source control:

```sh
: "${OHL_TEST_ISO:?Set this to the absolute path of your ISO}"
: "${REA_RESEARCH_DIR:?Set an absolute private output directory}"
umask 077
mkdir -p "$REA_RESEARCH_DIR"
rea --version
rea inspect-artifact "$OHL_TEST_ISO" --json \
  > "$REA_RESEARCH_DIR/iso-artifact.private.json" \
  2> "$REA_RESEARCH_DIR/iso-artifact.private.stderr"
```

With 6.5.0 this last command exits 1. The response contains the supplied path,
so inspect it locally and publish only a reviewed, project-defined error
category. Recheck support after a tool upgrade rather than silently routing
the ISO or extracted executables to native analysis. Retain private diagnostics
only for the requested investigation; remove those owned diagnostic files when
they are no longer needed, preserving the source image.

## Public behavior baseline

The following expectations come from the linked authors' public prose, read
on 2026-10-16. They are **documented**, not observed on the selected source.
No source implementation is needed to understand or test them.

| ID | Documented expectation | Required black-box observation |
| --- | --- | --- |
| M01 | Ground movement includes acceleration and friction. [D1] | From rest, hold one movement direction, release it, then repeat with a turn. Measure displacement and stopping time; do not infer them from this project's movement constants. |
| M02 | Ducking has a transition on the ground and different behavior in the air; visible eye movement and collision clearance need not change together. [D2] (§5.1) | Repeat short taps, held crouches, airborne crouches, and release under low clearance. Record visible timing and whether passage succeeds separately. |
| M03 | A grounded jump launches the player upward; short steps may be climbed without a jump. [D2] (§§5.2–5.3) | Measure a jump from rest and approach independently authored steps of known dimensions. Repeat jump press/hold/release sequences and different frame-rate limits. |
| W01 | An ordinary unnamed sliding door can activate on contact; named doors have a remote activation path, and a use-only option exists. [D3] (Attributes/Flags/Notes) | Test contact, explicit use, and remote activation as separate cases. Establish how naming, locking, and flags interact on the selected version. |
| W02 | Door movement has configurable speed, travel, waiting, and blocking behavior. [D3] (Attributes/Notes) | Measure opening, waiting, closing, and obstruction separately. Record reversal and health changes without assuming that one timing or damage cadence applies to every mover. |
| T01 | A level-change trigger loads a destination using a shared landmark. An event-driven transition can require the player to be within an associated transition region. [D4] (Attributes/Notes) | Compare activation inside and outside the region. Measure relative placement and separately check which player/world state survives; the public description does not specify every retained field. |

[D1]: https://www.jwchong.com/hl/movement
[D2]: https://www.jwchong.com/hl/duckjump
[D3]: https://twhl.info/wiki/page/func_door
[D4]: https://twhl.info/wiki/page/trigger_changelevel
[D5]: https://www.half-life.com/en/halflife/25th
[R1]: https://github.com/morluto/rea
[R2]: https://github.com/morluto/rea/blob/main/docs/cli.md

Source details: D1 is Chong Jiang Wei's *Half-Life Physics Reference*, chapter
4, “Player movement basics”; D2 is chapter 5, “Ducking and jumping.” D3 and
D4 are TWHL's respective entity documentation, restricted here to the prose
attributes, flags, and notes. D5 is Valve's official update announcement,
used only to distinguish reference versions. R1/R2 document the analysis
tool, not Half-Life behavior. The prose is paraphrased; source images, sound
lists, examples, formulas, and code are not reproduced.

## Priority questions in the current implementation

These are **unresolved** research assignments. References to project source
locate the current assumption; they provide no evidence of the original
behavior. Earlier public citations and their limitations remain in
[FORMAT_SOURCES.md](FORMAT_SOURCES.md), particularly “Collision hulls and
player movement” and “Player systems.”

1. **Ladder entry and exit:** measure climb speed, sideways movement,
   jump-off displacement, and the earliest possible reattachment. The
   ladder speed, detach speed, and reattach delay in
   `crates/ohl-physics/src/movement.rs` are explicitly placeholders.
2. **Long jump:** compare ownership absent/present, short and long crouch
   holds, stationary and moving starts, and release timing. Measure range
   and height separately. The same module marks impulse magnitudes and the
   input window as placeholders.
3. **Ground contact:** compare slow edge departures, descending steps,
   sloped landings, and low overhangs. Measure contact/clearance outcomes;
   do not infer an original probe distance or recovery algorithm from
   `crates/ohl-physics/src/movement.rs`.
4. **Damage-volume contact:** enter and leave a narrow hazard at its edge
   and far from its center. Compare contact duration with health changes.
   The radius approximation in `crates/ohl-engine/src/systems.rs` requires
   validation; an original contact volume cannot be inferred from it.
5. **Pickup contact:** approach an item from different heights, directions,
   and postures. Test at capacity and with inventory space. The synthetic
   box in `crates/ohl-engine/src/pickups.rs` is a project choice.
6. **Water, suit, and persistence:** measure immersion transitions, recovery
   after resurfacing, flashlight drain/recharge, and restoration across a
   save/load. Check partial resources and an active timed interaction,
   rather than only an idle full-health state. Relevant project areas are
   `crates/ohl-player/src/` and `crates/ohl-engine/src/save_state.rs`.

Weapon timing and spread, monster decisions, scripted campaign sequences,
audio timing, and rendering equivalence remain outside this first baseline.
They need their own public-source review and observable scenarios before
this document can support claims about them.

## Observation and acceptance protocol

This protocol is a **project choice**. It defines how to collect evidence,
not how Half-Life internally works.

1. Use a separately installed, legally obtained reference runtime. The ISO
   remains read-only; do not run its installer or binaries. An installation
   made by this project's asset importer is not an original reference
   runtime. No suitable runnable reference was established in this pass.
2. Identify the reference version and patches independently. Keep executable
   and complete-media digests distinct: the media digest does not identify
   the installed runtime. Record platform, difficulty, input bindings,
   rendering mode, frame-rate limit, and relevant runtime settings locally.
3. Use independently authored synthetic environments where geometry must
   be controlled. For a retail scene, keep its selector and any capture
   private. A path or name seen on the medium is not a publishable fixture
   identifier. Use the project-authored scenario IDs above in reviewed
   results.
4. Reset to the same initial state for each run. Describe inputs as timed
   presses, holds, releases, and view changes. Measure visible displacement,
   elapsed time, clearance outcomes, and resource changes through public
   interfaces. Do not inspect memory, instruction traces, or recovered code.
5. Repeat each case at least three times. For timing and movement cases,
   repeat at two frame-rate limits. Record actual achieved rates and input
   uncertainty; compare real elapsed time, not corresponding frame numbers.
   These repetition counts are experiment design choices, not game rules.
6. Run the same scenario in Open Half-Life. Define tolerances from measurement
   resolution before judging a match; do not tune them to hide a discrepancy.
   Distinguish a documented qualitative match, a measured quantitative match,
   a measured discrepancy, and an inconclusive run.
7. Review the local record against the media policy before publishing a
   minimal result. Keep raw video, screenshots, audio, saves, console output,
   and private selectors out of source control and hosted artifacts.

A local finding should identify its scenario, reference identity, public
basis, initial conditions, input sequence, repeat count, measurement method,
uncertainty, and unresolved variables. If a later supported REA runtime
workflow is used, retain its exact Evidence ID and limitations locally.
Do not invent an Evidence ID for this pass's rejected artifact command.

Only promote an unresolved behavior to observed when that record exists and
has been reviewed. Add synthetic regression coverage for the independently
specified result; do not turn private captures or recovered implementation
into tests. Passing the project's existing campaign/combat smokes remains
project execution evidence, not a comparison against the original runtime.
