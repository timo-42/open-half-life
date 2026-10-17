# Original Half-Life multiplayer protocol evidence

Research began 2026-10-16; target and protocol-cohort review: 2026-10-17.
The current requested result is the full final pre-Steam WON Half-Life
deathmatch experience, with Rust client and server implementations using the
original network protocol. Compatibility means
exchanging the original wire messages with an identified reference runtime.
A custom Rust-to-Rust transport, a working server browser, or a successful
challenge response does not establish that result. Full deathmatch gameplay
and original-client/server gameplay interoperability remain unverified.

This ledger supplements [BEHAVIOR_SPEC.md](BEHAVIOR_SPEC.md). The
[clean-room policy](CLEAN_ROOM.md) and [local data boundary](MEDIA_IMPORT.md)
apply throughout. Its sources are published prose, packet-format tables and
reviewed lawful black-box observations.
No engine source, decompilation, disassembly, or recovered implementation is
an implementation source. The user's ISO remains read-only, and binaries or
installers from it must not be executed.

## Evidence and version boundaries

- **Documented** means the linked public source describes the fact. It does
  not establish applicability to every Half-Life build.
- **Observed** requires a repeatable black-box experiment against an
  identified, lawfully acquired runtime. No normal-player gameplay comparison
  or decoded-channel compatibility result is recorded. Opaque official HLTV
  proxy traffic and bounded packet-prefix correlations are recorded below;
  the separate word-transform model has its own scoped validation record.
- **Inferred** identifies a provisional interpretation selected for an
  experiment or parser; it is not a verified original-protocol rule.
- **Project choice** describes a proposed design, budget, or test constraint.
- **Unresolved** means the evidence is insufficient to implement or claim
  compatibility.

Keep retail/WON and modern Steam targets separate. The ISO's runtime and
network version are unverified; a downloaded dedicated server cannot identify
them. An older report of a handshake is evidence for its stated historical
target. For example, a [firsthand published console report][P5] identifies
protocol 48 for two particular 2009/2013 builds; it does not establish this
retail medium's protocol. Valve's [anniversary release notes][P6] also record
later multiplayer and networking changes.

Record the selected reference's distribution, observed protocol version,
build, and relevant settings in a reviewed local record before a comparison.
Do not select an older packet layout merely by changing a protocol-number
constant. A Source-format server-info response also does not turn GoldSrc
gameplay into the Source engine's gameplay protocol.

The user's correction selects the release family immediately before Steam,
not the separately downloaded app-90 Steam server. Valve's mirrored
[1.1.1.0 client readme][P15] is dated June 12, 2002; it is a candidate client
reference, not a completed gameplay comparison. The Linux server package and
query observations are identified below; its exact native build remains
unconfirmed. Valve's
[Steam release announcement][P16] dates the public client release to
September 12, 2003. Do not equate the selected WON target with a later Steam
build by changing a protocol constant.

All protocol-48 observations below remain historical evidence for their stated
Steam server/proxy distributions. They do not verify the selected pre-Steam
target. A separately acquired public full-client update is being inspected
through static extraction, component-provenance checks and bounded launcher
startup. The pre-Steam server queries below establish no player sign-on or
gameplay result. The ISO execution prohibition remains unchanged.

## Required client/server pairings

All four pairings remain pending. They must run the original wire protocol
and the complete deathmatch lifecycle:

| Client | Server | Required comparison |
| --- | --- | --- |
| Original pre-Steam | Original pre-Steam | Identified baseline with matching local content and admission settings |
| Rust | Original pre-Steam | Client sign-on, authoritative updates, input and complete match behavior |
| Original pre-Steam | Rust | Server admission, bootstrap, validated commands and complete match behavior |
| Rust | Rust | The same original-wire path, checked against both cross-pairings |

The first gameplay milestone is an ordinary player reaching active/spawned
state with the required map, resources and delta bootstrap, followed by a
validated input/no-op round trip on both cross-pairings. Query success,
proxy attachment or a reserved player slot does not meet it. Movement,
combat, scoring, pickups, respawn, chat, prediction and map transitions then
need their own comparisons across all pairings.

## Pre-Steam reference observation (2026-10-17)

The public Linux [base][P17] and [d-update][P18] packages' bundled readmes
label them 3.1.1.1 (May 1, 2003) and 3.1.1.1d (July 30, 2003).
The executed launcher's digest matched its staged d-update origin. Native
output reported version 3.1.1.1 and protocol 46, without a displayed `d`
suffix or confirmed build. Whole-source SHA-256 provenance:

| Source | SHA-256 |
| --- | --- |
| Linux base package | `d852368e606736033be85992459e343e994775ab36463aced44b04a7672484ea` |
| Linux d-update package | `5536a28dae2424ab142d0586dbb533a688cb21a844f79757855f299ccc4dd1f7` |
| Public full-client update | `35baa2058bf41f872b418e1545d3a8bb92f13f7db211d734e522863b95651604` |
| Query 001 recording | `14af134e701648220352a2d3e46dbedcb3b0678145cfd6b0d055f378d0e2ec57` |
| Query 002 recording | `17865c9143f28442b209dee802ba0b1edf10e43258b78e0e3d9cb94a21abe89d` |
| Query 003 recording | `78b70edff7e0f2000b85e98c1e0d95c8ef67e7fcc4426524febc5613de3bff80` |
| Public `won46` CLI recording | `2032707a999e19307e0787e0104146ae9fffd8fb723f803fd83f27a4f5308107` |

These package hashes do not cover every dynamic runtime dependency. Mirror
MD5 agreement is integrity evidence, not publisher cryptographic authentication.

Query 001 sent three exact unterminated `details` requests. Each 60-byte reply
reported protocol 46, zero players and capacity four. Published fields consumed
59 bytes, followed by one observed zero byte with unassigned meaning;
`PublishedAscii` correctly rejected the trailing data. Query 002 independently
sent one unterminated `details`, `players` and `rules` request each. Details
had the same boundary; empty Players consumed six bytes and the 48-rule reply
all 767 bytes under the selected LE/NUL/UTF-8 parser. Query 003 used the new
production `ObservedLinux3111d` profile and passed all three query families.
This is post-implementation conformance, not held-out structural discovery.
Populated player scores/times, arbitrary text encodings and other field
combinations remain unverified.

All three bounded server runs exited 0. Each recording has 12 records across
two relay boundaries, with all six paired payloads identical. REA 6.5.0 and
TShark 4.6.9 inspected only `udp.length` for Query 001: packet limit 128,
match limit 12; evidence
`ev_4c576e089c991e1aa44548580345525f690844d440b9adfb91651ebc03a804aa`
returned all 12 matches, with no more results. This is field extraction, not
message decoding. Synthesized IPv4/UDP envelopes and call-level timestamps
establish neither distinct NIC packets nor original wire timing. Raw payloads
and native text remain private.

The public-update-only client stage's executable/config origins matched
verified extracted entries plus project-authored configuration. The current
reference gate is `UnclassifiedPreEngineInputDialog`: the initial two fresh
short-C-drive starts, including Wine's documented Windows XP64 setting,
retained matching stage hashes but observed no engine/game-client load or client
protocol. No UDP reached the owned observation endpoint; this is not a
system-wide traffic measurement. The dialog's cause is unclassified, with
no established missing-key or missing-data diagnosis. Owned processes were
stopped; no installer, ISO code or ISO scripts were executed.

A later bounded file/loader trace found one unique verified public-patch asset
at a different location. One isolated exact-byte copy to the absent requested
location changed that lookup to four nonerror resolutions. All 28 compared
PID-owned UI category/length fields stayed unchanged across three samples;
there was no engine/game-client load or traffic at the owned endpoint.
The gate persists, with no missing-key/data diagnosis or observed startup
remedy. Original data, identities and UI text remain private.

## Protocol-46 spectator packet cohorts

Two separately bounded loopback HLDS/HLTV runs used the base-plus-d-update
distribution above. Both executable origins matched the d update; native
output remained 3.1.1.1/protocol 46, not a confirmed build identifier.
Both native processes exited 0 in each run; each proxy reported three
server-source statuses, with owned cleanup within 25 seconds. Each proxy
log retained one loader-category and two other error indicators; these were
not warning-free runtime results.

| Cohort | Relay records / equal pairs | Eligible echo matches | Complete-capture SHA-256 |
| --- | --- | --- | --- |
| Baseline | 1,480 / 740 | 733 / 733 | `ed6097fd53b796c2d4fff8502a1dcb4af6b62817216d0efbfbc7a970b2e84471` |
| Synthetic text | 1,430 / 715 | 708 / 708 | `711e5f5fce9d2a9cc3432332f2aa7408716dd83fa543e7214813fcb01e29da2c` |

The unchanged high-water/bit-31-toggle candidate matched both spectator
directions, excluding one unseen initial reference per run. The selected
eight-byte view, low-30-bit projections, 2^29 guards and 512-state history
remain project bounds; width, wrap, complete header, fragments and reliable
delivery are unresolved. The relay synthesized envelopes and call-level
timestamps; records at two boundaries are not distinct NIC packets.

The unchanged [frozen word model](GOLDSRC_TRANSFORM.md) matched 60 unique
authored texts of 96..103 bytes: 5,962 known bytes, 60 distinct low-counter
bytes and all 16 mask slots covered by fully known complete words. Each
packet/text length residue had 15 samples. Known text began at absolute UDP
offset 14 and ended at offsets 110..117 exclusive. Two earlier command-sized
candidates were excluded; there was no low-byte decrease and no known text
in a partial-word tail. These results cover only proxy-to-server packets
with first bit 30 clear, without fitting coefficients or identifying the
whole remainder as plaintext.

The explicit profiles are `PrefixProfile::ExperimentalProtocol46Linux3111d`
and `WordTransformProfile::ExperimentalProtocol46Linux3111dHltvToHlds`;
the historical protocol-48 selectors remain distinct. Tracker
`observe_prefix` now returns `Result` and rejects foreign prefixes with
`ChannelError::ProfileMismatch` before changing state/history. `compare_echo`
returns `EchoComparison::ProfileMismatch`
for a foreign observation; word operations return
`WordTransformError::ProfileMismatch` before output changes. One unchanged
coefficient table serves all word profiles. No profile is detected from bytes.

Production-profile Rust replay matched 708 eligible echoes, excluded one
unseen reference and matched all 60 texts/5,962 bytes, plus 60 inverse,
60 output-sentinel and 60 opaque-tail preservation checks. This is
post-implementation conformance to the frozen hypothesis, not an independent
held-out discovery or tail-plaintext result. Six new synthetic tests bring
focused `ohl-net` validation to 111 passed and one ignored; strict crate
Clippy, targeted formatting and diff checks passed.

An earlier passive reverse scan found no authored text in 288 clear-bit-30
candidates, without establishing delivery. The separate server-originated
cohort below supplies narrower known-text evidence. Neither result establishes
ordinary-player sign-on, complete channel behavior or a playable CLI.

## Protocol-46 reverse known text

The same base-plus-d-update HLDS/HLTV reference was measured in the
server-to-proxy direction with the unchanged frozen model. A pilot identified
60 authored 63-byte prefixes at absolute UDP offset 18: 3,780 known bytes,
900 wholly known words, 15 whole mask slots, 63 byte positions and 60 low-counter
bytes. Its 63-byte endpoint was selected after the run; full intended text
delivery and a general truncation rule were not established.

| Reverse cohort | Relay records / equal pairs | Complete-capture SHA-256 |
| --- | --- | --- |
| Pilot, post-run prefix coverage | 1,559 / 779 | `5d00c593ff25bbd98bd31547bd96b28827a90259b2209de789cb170ee5ed4508` |
| Fresh predeclared full texts | 1,547 / 773 | `c7fd6f782eca67abe22a54f8d2a00244320bdcd5203f71a073aa9cb3f40eaae9` |

Each recording also retains one unpaired proxy-to-server cleanup ingress
with an explicit `connection_refused` send failure and no successful egress;
it is excluded from equal pairs and proves no delivery. Envelopes and
call-level timestamps are synthesized application-relay records, not NIC data.

The fresh run froze 60 new texts, lengths 24..31, and the 16-byte marker
window `[18,34)` before launch. All 60 full texts appeared in both endpoint
consoles and matched at offset 18, ending at offsets 42..49 exclusive. That
covers 1,642 known bytes, 358 wholly known words, seven whole mask slots,
31 byte positions and 60 distinct low-counter bytes, with zero contradictions
or missing/duplicate events and no model/offset fitting. Matched packet
length residues 0/1/2/3 had 15/16/14/15 samples; text residues had 15 each.
Of 346 server-to-proxy datagrams, three connectionless/split and 25 first-bit-30
candidates were excluded; 318 selected candidates comprise 60 matches and
258 retained unmatched records. All 104 partial-tail bytes remain unknown;
no known text entered a partial tail. Cleanup finished in 19,216 ms with
707 runtime hashes/modes unchanged and owned processes/ports released.

`WordTransformProfile::ExperimentalProtocol46Linux3111dHldsToHltv` requires
`PrefixProfile::ExperimentalProtocol46Linux3111d` and first bit 30 clear.
The prefix cannot establish direction: callers select the measured direction
explicitly. There is no reverse protocol-48 profile or automatic fallback.
Offsets 8..17, other positions/keys, higher guarded counters and longer
periods extrapolate this reverse cohort. The full-word/tail boundary and
other limits are unchanged in [the specification](GOLDSRC_TRANSFORM.md).

Separate production-profile replay matched 60 full texts/1,642 known bytes and
60 candidate whole-word buffers, inverse regions, sentinels and opaque-tail
preservation checks. This is post-implementation conformance to the frozen
model, not an additional held-out discovery or unknown-plaintext validation.
No ordinary-player, sign-on, reliability or complete-header claim follows.

## Bounded query observation

On 2026-10-16 the coordinator ran the opt-in
`original_server_query_interoperability` test against a separately downloaded
official Half-Life Dedicated Server, obtained anonymously as Steam app 90 and
running on loopback. The reference reported protocol 48. The Rust INFO,
PLAYER, and RULES requests completed, and the test passed. Original binaries,
server text, rule strings, and packet payloads stayed outside this ledger.

The reproducible test entry point is:

```sh
: "${OHL_GOLDSRC_SERVER:?Set the socket address of the separate loopback reference}"
OHL_GOLDSRC_SERVER="$OHL_GOLDSRC_SERVER" \
  cargo test --locked -p ohl-net --test reference -- --ignored
```

This records query interoperability for that reference, not a retail-media
version determination or a gameplay connection. It does not establish every
split layout, a populated player list, join authentication, sign-on,
movement, or a playable match. Synthetic tests cover additional documented
query layouts independently. The current query subset deliberately excludes
Source-style/compressed splits and high-bit GoldSource request IDs, requires
fragment zero to contain the complete logical-response header, and imposes
project budgets. Those restrictions do not describe all original servers.

REA 6.5.0 also inspected a private recording of the real Rust query CLI's
exchange with that reference, using `inspect-packet-capture` and TShark
4.6.9. Evidence
`ev_13a346f84f74a27b75a91763f3309d9304afe0836008cb06d50cf99ea95dcfdd`
returned all four recorded UDP entries within the selected packet window.
Only UDP port and length fields were selected. An application-level relay
recorded the request and response at its two boundaries; the IPv4/UDP
envelopes and timestamps were synthesized by the recorder. This was not a
NIC capture and establishes neither original IP headers nor wire timing.
REA retained an optional-plugin warning in its private diagnostics; the
UDP-field inspection completed successfully. Payloads, endpoint fields and
raw diagnostics remain local. This evidence supports the bounded query
experiment only, not the unresolved gameplay channel.

## Bounded HLTV proxy observation

Valve's [2003 HLTV manual][P13], §§4 and 7, documents that HLDS supplies the
proxy's required files, its server connection and bind options, and the
`sv_proxies 1` admission setting. Its spectator role cannot supply ordinary
player input verification. The author's [admin FAQ][P14] documents Linux
`hltv` and the same server admission gate.

On 2026-10-16 the coordinator ran the official proxy against the separate
protocol-48 app-90 HLDS on loopback, with `sv_proxies 1`. A bounded repeat
scheduled status every five seconds and quit at nineteen seconds, within a
twenty-second outer deadline. The proxy can be started directly with these
documented controls and project-selected ports:

```sh
: "${HL_REFERENCE_DIR:?Set the private official reference directory}"
cd "$HL_REFERENCE_DIR"
timeout 20s ./hltv -ip 127.0.0.1 -port 27032 -nodns \
  +nomaster 1 +maxclients 0 +autoretry 0 \
  +connect 127.0.0.1:27031 +loopcmd 1 5 status +loopcmd 2 19 quit
```

For the recorded observation, the coordinator routed the connection through
a private application UDP relay; its configuration remains local.
The process exited 0 and emitted three status headings and a connection
indicator. Two missing-configuration-category diagnostic lines and one
unclassified diagnostic were retained privately; this was not a fully clean
runtime result. An application UDP relay recorded 1,616 entries across its
**two boundaries**, containing 59,148 combined payload bytes. These are not
1,616 distinct wire packets. Of those entries, 1,606 lacked the documented
connectionless header prefixes; their channel semantics were not decoded.

REA 6.5.0 `inspect-packet-capture` with TShark 4.6.9 produced evidence
`ev_820483dc0f57a6afdbad432d5169119be5b16e9cfb5553f26b87c66b78a0c868`.
With only UDP port/length fields selected and a packet limit of 2,048, it
reported 1,616 matching records, returned an eight-record window, and set
`has_more` to true. Envelopes and timestamps were synthesized by the relay
recorder: this was not hardware acquisition and does not establish original
IP headers, NIC timing, or complete-payload semantics. Raw recordings,
logs, and runtime diagnostics remain ignored/private; all owned reference processes
were stopped after the experiment.

This provides a partial spectator-proxy reference, not decoded sign-on or
normal-client interoperability. Original gameplay channel implementation
remains open. Normal-player input, authentication, and prediction still
require a runnable original player client and separate validation. The
prohibition on executing binaries from the supplied ISO remains in force.

## Controlled packet-prefix observations

The coordinator and independent analyst compared six controlled loopback
HLDS/HLTV runs with one held-out run. The controls included bounded local
packet loss, duplication and reordering. All six controlled runs ended
with exit status 0 before nineteen seconds within a twenty-five-second hard
deadline, with three status headings each. These are the same separately
downloaded official protocol-48 server and spectator-proxy reference class;
no normal player client or ISO binary was run.

A candidate model reads an eight-byte prefix view as two little-endian
32-bit words at offsets 0 and 4. It retains a high-water counter and toggles
an initially false parity state when an accepted increasing first counter
has bit 31 set. At the exact accepted counter referenced by the opposite
word, it predicted all 5,342 eligible opposite-word observations, with zero
contradictions. Seven initial observations were excluded because the
referenced state had not
been seen. The bounded controls also yielded one duplicate and one stale
prefix rejected without advancing the model. A competing model that mirrored
the referenced packet's first bit-31 flag had 777 contradictions even with
its best fixed inversion per run/direction. These are counts of model
comparisons, not distinct wire packets or proof of reliable delivery.

The observed counter values do not distinguish 29-bit and 30-bit widths.
The selected profile projects the low 30 bits of both words and guards both
below 2^29; this is a project limit, not an observed wrap boundary. Bit 30
remains separately opaque. These correlations establish neither complete
header size, sequence wrap/reset rules, reliable acknowledgement or retransmission
semantics, fragmentation, byte munging nor normal-player compatibility.
An experimental prefix tracker must keep the remaining bytes opaque and
must not automatically pass them to the server-message reader.

For this historical cohort, `ohl-net::channel::decode_datagram` selects
`PrefixProfile::ExperimentalProtocol48`. It classifies the documented query
markers without validating their bodies, or returns a `PacketPrefix` and
an explicitly unvalidated remainder. A 1,400-byte datagram cap and
`ReceiveTracker`'s fixed history of 512 exact accepted-counter states are
project bounds. Echo comparisons exclude unseen or evicted references;
duplicate and stale observations do not advance state. Debug output redacts
raw words and remainders. Callers must associate directions and streams;
the tracker provides no peer authentication, reset detection or send path.

Independent Rust replay of all seven reference traces matched 5,342 exact
historical echo observations and excluded seven unobserved references. It
classified one duplicate and one stale prefix, and 35 typed connectionless
errors left tracker state unchanged. No prefix/model mismatches were found;
the fixed 512-state history retained all eligible references in these runs.

Before the final diagnostic opcode-redaction fix, the full locked workspace
test fallback passed 3,280 tests with 38 ignored across 253 result summaries.
After that diagnostic-only fix, the focused locked `ohl-net` run passed 52
tests (15 svc, 14 prefix and 23 query tests), with one original-reference
test ignored. Full workspace formatting, strict workspace/all-target Clippy
and diff checks passed after the fix. The full suite was not repeated for
the diagnostic change; these checks do not establish gameplay compatibility.

The independently reviewed SHA-256 values below identify the **complete
private capture files**. Run labels name project controls; they are not
original-protocol identifiers. The captures use an application UDP relay
with records at two boundaries and synthesized IP/UDP envelopes and
call-level timestamps. They establish no original IP headers, NIC timing
or hardware acquisition. Raw captures, commands and diagnostics remain
ignored/private.

| Project run | Complete capture SHA-256 |
| --- | --- |
| `baseline` | `af16158c4961d4b6d5a1b256e154917f857208e6cd62f73a590977dd4f29629c` |
| `drop_proxy` | `45bd5a79d029e7647cf5c31c7be6ab1d3ed5ba73e531eb2e9a2b933738cbdce7` |
| `drop_server` | `a33d8c807bf3180767212a819953ad068a2539d0a326f8423630ab1bc822c626` |
| `drop_server_bit_thirty` | `6ff4fc5d557ec8248276e29e79925527128d0b2a93f065d707725974c33889da` |
| `duplicate_server` | `3a46bff7c77e08d2bfa9f7e6d2f0bf7dd0544151537acc9a7346538d6c5b941d` |
| `reorder_server` | `2a86abdad4f01bd47eaaacba2c2de70adef4fd102fe3277659852fe573a270ea` |
| `heldout_known_command` | `0fc97b1e190545998b37def38de31f05451e7b28e5ca9ab714e1331e7c056508` |

For the held-out capture, REA 6.5.0 with TShark 4.6.9 produced evidence
`ev_80a020ccd607baa9a211df8682b102af4d6218268df55e85e8ccff3b1320ed1c`.
With a packet limit of 2,048 it reported twelve matching records, returned
eight and set `has_more` to true. Eight selected UDP payloads matched an
independent extraction byte for byte. This checks extraction fidelity only;
byte/field extraction is not evidence of protocol semantics, a complete
payload decoder, a solved transform or a successful sign-on session.

## Experimental full-word transform

[GOLDSRC_TRANSFORM.md](GOLDSRC_TRANSFORM.md) specifies an independently
inferred word operation for the historical official proxy-to-server direction,
protocol 48, app-90 distribution build 15961496, observed 2026-10-16. The
model was calibrated only from synthetic text; its first fully held-out
check matched 60/60 known texts, with one earlier candidate excluded. The
specification records the frozen model/capture digests, exact operation and
inverse, measured coverage and extrapolated applicability.
Scoped Rust codec checks and private replay results are recorded there
separately from the prior parser/prefix validation above.
The separate protocol-46 cohort above uses its own explicit profiles and the
unchanged model; the protocol-48 observations are not relabelled.

The bounded codec operates on complete words after the selected eight-byte
view and leaves a final zero-through-three-byte tail opaque. No known text
entered that incomplete tail, so tail plaintext is unverified. First-word
bit 30 must be clear. No packet send path, automatic svc decoding, reliable
channel, authentication or normal-player compatibility follows from this
result. Untested runtime/direction combinations, header extent,
higher-counter/body-range applicability and complete transform variants
remain evidence gaps.

## Documented connectionless query formats

The primary specification is Valve Developer Community's [Server queries][P1],
revision 333666, retained in the [2023-08-31 archive][P1A]. These are UDP
queries. Integers use little-endian ordering; strings end with a zero byte.
For an unsplit datagram, `FF FF FF FF` precedes its message type.

- Info request: prefix, `54`, then `Source Engine Query\0`; a challenge may
  follow. Responses use `49` for the Source-format layout or `6D` for the
  older GoldSource layout.
- Players request: prefix, `55`, and a four-byte challenge; reply type `44`.
- Rules request: prefix, `56`, and a four-byte challenge; reply type `45`.
  Both request a fresh challenge initially with `FF FF FF FF` in that field.
- A challenge reply uses prefix, `41`, and four challenge bytes. Retry the
  original query with those bytes; this challenge is distinct from joining.
- GoldSource split replies use `FE FF FF FF`, a four-byte request ID, and one
  packed byte: upper four bits are the part index, lower four the total
  count, from 2 through 15. Reassemble ordered payloads before parsing their
  included unsplit header. This format is distinct from Source split replies.

These statements specify discovery, not gameplay sign-on or authentication.
The entire typed response layouts remain in the cited specification. Parser
budgets, retry counts, deadlines, and rejection of unsupported layouts are
project choices, not original-engine constants.

The GameStat author's [historical query tables][P2] separately describe
unterminated ASCII `details`, `rules` and `players` requests after the
four-byte all-ones prefix, with `m`, `E` and `D` reply families and
NUL-terminated response strings. They do not identify an exact runtime
release or establish every primitive representation.
Do not commit the published packet dumps or observed server/player strings as
fixtures; construct synthetic fields instead.

## Bidirectional query-codec checkpoint (2026-10-17)

[`ohl-net::server`](../crates/ohl-net/src/server.rs) adds pure
`decode_request`, `encode_response` and `goldsrc_response_datagrams` APIs
for the documented later A2S layouts. The obsolete query `W` request is
separate from a player's textual join challenge. The encoder preserves the
existing later `m` model, including its bot byte. Its host owns sender
association, token generation/expiry, rate and challenge policy; the codec
provides no sockets or player admission. The split packetizer has a project
1,400-byte datagram limit, fifteen parts with 1,391 payload bytes each and
caller-selected IDs whose high bit must be clear. A logical response can
fit the 64 KiB encoder but exceed that split budget.

[`ohl-net::legacy`](../crates/ohl-net/src/legacy.rs) separately provides
request/response encoders and decoders requiring `Profile::PublishedAscii`.
Its own `Details` ends at `secure_server`; absent bot counts remain absent.
`Modification::unused` preserves a complete string. There is no implicit
conversion to the later INFO model. `PlayerInfo` and `ServerRule` entries
are reused within a separate response enum.

That profile selects little-endian integers, IEEE-754 binary32 player time,
strict UTF-8 response text and canonical boolean bytes 0/1. Those selections
are not all established by the historical prose or verified for the chosen
pre-Steam runtime. Requests have no terminator under this explicit profile;
suffixes and later layouts are rejected without fallback. Bounds are project
choices: 1,400 request bytes, 64 KiB logical responses, 4,096 UTF-8 bytes per
string, 255 players and 1,024 rules. This module supplies no UDP, splitting,
challenge exchange or gameplay channel; oversized logical responses need a
separately evidenced transport.

Both encoders preserve caller output on failure. New wrappers and errors
redact packet/text data in automatic formatting. The focused locked crate run
passed 98 tests with one explicit original-reference test ignored, including
16 new server and 17 historical-codec synthetic tests. Independent review
found no blocking issue; strict all-target crate Clippy, targeted formatting
and diff checks passed. Final locked workspace tests passed 3,327 tests with
38 ignored across 256 result summaries. Full workspace formatting, strict
workspace/all-target Clippy and cargo-deny also passed; the lockfile is
unchanged. These are codec checks, not a new original-runtime or
playable-client result. The later opt-in query API/CLI below extends historical
metadata queries; the browser remains A2S-only. No host/join gameplay CLI was
added.

### Observed pre-Steam query profile

`legacy::Profile::ObservedLinux3111d` explicitly selects the dated reference
cohort above, not every protocol-46 implementation. Its
`Response::ObservedDetails` carries
`ObservedDetails { published: Details, opaque_extension: u8 }`, with a
protocol-46 safety guard and exactly one extension byte. The byte is preserved
without bot semantics or a synthesized default; nonzero values and other
field combinations remain structural candidates. `PublishedAscii` is
unchanged, and mismatched profile/response shapes are rejected without
fallback. Existing budgets, atomic output and redacted diagnostics remain.
Players/Rules reuse the published candidate layouts, with only the bounded
empty-player/rule cohort observed above. Seven new synthetic tests bring the
focused locked crate result to 105 passed and one ignored. Final locked
workspace tests passed 3,334 tests with 38 ignored across 256 summaries;
workspace formatting, strict locked workspace/all-target Clippy and cargo-deny
passed, with no lockfile change. This adds no gameplay transport or playable CLI.

### Strict historical query API and CLI

`ohl_net::query_legacy(address, timeout, profile, request)` returns the separate
`legacy::Response` or fixed `LegacyQueryError` categories. It sends one exact
ASCII request through a connected UDP socket with an explicit profile.
Project bounds are a nonzero absolute deadline of at most 30 seconds,
64 received datagrams and 1,400 bytes per datagram. Well-formed wrong-kind
replies share that deadline/budget; malformed replies and splits fail
without A2S conversion, profile fallback or challenge exchange.

`--query-server IP[:PORT] --query-profile won46` selects
`ObservedLinux3111d` Details before media import or GUI startup. Omitted or
`a2s` profile retains modern A2S behavior; the menu browser remains A2S-only.
Addresses are numeric IPv4/IPv6 with default port 27015. The CLI uses a
project-chosen 1.5-second deadline and emits only protocol, players, capacity
and password status. Profile selection requires a query target and rejects
media/play, scripted capture, profiling and development-viewer conflicts.

One invocation of the existing compiled public CLI against a separate
707-file verified original stage exited 0, as did HLDS. Actual output and an
independent strict reply traversal agreed on protocol 46, players 0,
capacity 4 and password false. The 60-byte reply retained exactly one opaque
extension byte. Four relay records form two equal receive/forward pairs;
the request was the exact unterminated ASCII form. The complete recording
hash is in the source table above. Owned cleanup completed in 1,228 ms;
707 native hashes/modes, six selected Rust source hashes and the CLI digest
were unchanged, and owned processes/ports were released. These are
synthesized-envelope application-relay observations, not NIC capture.

The tested CLI executable SHA-256 was
`4aa2d119401225471ced3e0d4c102bfdca769a4430324a7f4132ef0326e4a6ff`.
Its prior build had no retained executable digest; the builder's handoff
and unchanged source hashes associate the currently bound artifact with
that build. This metadata result establishes no admission or gameplay.
Focused final validation passed 123 `ohl-net` tests with one reference ignored,
183 `ohl-app` tests with four ignored and one targeted `dev-tools` conflict
test. Strict crate Clippy, formatting and diff checks passed.

Final checkpoint validation after the packaging ZIP update passed 3,373 tests
with 38 ignored across 258 result summaries using
`cargo test --workspace --locked`. Workspace formatting and strict locked
workspace/all-target Clippy passed. The post-update cargo-deny check passed
advisories, bans, licenses and sources, retaining 18 `duplicate` warnings and
one `license-not-encountered` warning. `Cargo.lock` changed only for ZIP 9.0.4
and its required closure, documented in
[the packaging provenance note](FORMAT_SOURCES.md#release-zip-dependency-maintenance-2026-10-17).
These project checks add no ordinary-player interoperability result.

## Documented join outline

Luigi Auriemma's [2002 firsthand protocol description][P3], concerning
Half-Life through the stated 1.1.0.9 target, describes this sequence:

1. Send `FF FF FF FF` followed by `getchallenge\n` over UDP.
2. Receive a challenge value.
3. Send a textual connection request with protocol, challenge,
   authentication material, and player information.
4. Receive acceptance or rejection.

That report documents the broad handshake and the default server port
27015. It does not establish the exact final pre-Steam build's CD-key/WON
admission, acceptance, sign-on exchange or gameplay channel. It also does not
define a modern Steam ticket exchange. Do not generate fake ownership
credentials, reuse another client's
tickets, or interpret an accepted connectionless request as a spawned player.

## Server-message evidence and remaining gaps

AlliedModders' [Half-Life 1 Engine Messages][P4], revision 11325, publishes
server-message numbers and field tables. Examples include no-payload
`SVC_NOP` (1), disconnect reason string (2), floating-point server time (7),
and sign-on number (25). It describes bit-packed client state, entity
state/baselines, resources, and delta-description exchange. Game-specific messages are
documented separately in [Half-Life 1 Game Events][P4G]. These are useful
payload references after a compatible channel has been established; they
are not a specification of UDP channel framing.

`ohl-net::message::MessageReader` independently implements 15 of those
byte-aligned opcode layouts for an **already isolated payload**. Callers
must explicitly select
`ByteModeProfile::InferredLittleEndianNulStrings`: 16/32-bit little-endian
integer words, four uninterpreted bytes per float, and NUL-terminated byte
strings. Applying Valve's published query primitive rules to svc messages is
an inference, not an original-gameplay verification. `WireFloat` performs no
floating-point conversion, and borrowed `WireText` performs no encoding,
command, path or display interpretation. Debug output redacts text, opaque
hashes and map CRC fingerprints.

The allocation-free iterator yields a valid prefix, then one error and stops
permanently on malformed, unsupported or over-budget input. It never skips
an unknown layout or scans for a later opcode. Project bounds are 64 KiB per
payload, 4,096 messages, 4,096 bytes per regular text field and 8,192 bytes for
mapcycle text. Sign-on number 25 accepts only the documented literal byte 1;
this does not implement a sign-on session. Opcode 39 remains unsupported
because its published name width contradicts the accompanying prose. Tests
use authored synthetic bytes only. No channel adapter passes recorded
traffic to this reader, and it does not execute server-supplied commands.

The following still require admissible documentation or reviewed black-box
evidence. Existing project code or another engine's implementation cannot
close an evidence gap:

- Sequenced datagram framing, sequence-number width/wrap rules, reliable
  acknowledgement flags, duplicate/out-of-order handling, retransmission,
  and choke behavior for each target version.
- Reliable normal/file fragment headers, identifiers, offsets, reassembly,
  interleaving, compression, and size limits. Connectionless query splits
  above do not specify these channel fragments.
- Complete GoldSrc byte munging: the experimental
  [full-word spectator models](GOLDSRC_TRANSFORM.md) do not establish
  untested runtime/direction combinations, every counter or body range,
  complete header extent or trailing-byte rules. No reviewed public prose source here
  establishes those remaining details.
- Delta field metadata bootstrap, complete field types, bit order,
  quantization, signed values, conditional fields, and baseline selection.
  Published message tables name delta-compressed payloads without resolving
  every rule needed to produce them.
- Client-to-server movement message opcodes, command batching/backups,
  delta serialization, move checksum/sequence binding, command timing, and
  acknowledgements needed for prediction/reconciliation.
- Complete sign-on transitions and map/resource consistency checks; public
  game-message registration, scoring, combat, respawn, map changes, and
  intermission behavior for the selected deathmatch version.
- Final pre-Steam CD-key/WON admission and its LAN behavior. Historical
  challenge/connect prose does not supply a complete credential or join
  implementation. Steam authentication is a separate historical-reference
  boundary: Valve's [ticket documentation][P7] does not specify WON admission
  or GoldSrc's ticket embedding.

Do not fill these gaps using Quake netchan documentation, a Source protocol,
automated summaries of engine source, or a guessed XOR/munging algorithm.
Such substitution would not verify original Half-Life compatibility.

## Historical Steam reference setup

This recipe documents the already observed protocol-48 research reference.
It does not obtain the user's selected pre-Steam runtime. A public pre-Steam
client update and Linux server distribution are being checked separately;
the pre-Steam query observations above are separate from this recipe, and no
ordinary-player sign-on has completed.

Valve's [dedicated-server distribution documentation][P8] describes anonymous
SteamCMD server downloads without a signed-in Steam client. The [SteamCMD
documentation][P9] is the entry point; direct access was blocked during this
research. The [Steam community setup guide][P10] documents app 90 and
repeated validation when its first download is incomplete. The [Slackware
administrator guide][P11] documents starting the downloaded Half-Life server.
These are executable software for an external local reference only, never
source or dependencies for the Rust engine.

The commands below are a **project experiment recipe** for a separate
reference. Query and opaque proxy observations are recorded above; a normal
player gameplay join is still pending. Select fresh private directories outside
tracked files, either outside the checkout or under an ignored private cache.
The map name is runtime input from an independently public
source or an independently authored test map; do not derive a checked-in
literal from private media. Run as an unprivileged user.

```sh
: "${HL_STEAMCMD_DIR:?Set a private SteamCMD directory outside the checkout}"
: "${HL_REFERENCE_DIR:?Set a separate private reference directory}"
: "${HL_REFERENCE_MAP:?Set the publicly documented reference map name}"
"$HL_STEAMCMD_DIR/steamcmd.sh" \
  +force_install_dir "$HL_REFERENCE_DIR" \
  +login anonymous +app_set_config 90 mod valve +app_update 90 validate +quit
cd "$HL_REFERENCE_DIR"
./hlds_run -game valve -insecure -port 27015 \
  +ip 127.0.0.1 +sv_lan 1 +maxplayers 4 +map "$HL_REFERENCE_MAP"
```

`valve`, `hlds_run`, and the launch syntax have public provenance in [P11].
Valve's [GoldSrc server options][P12] documents `+sv_lan 1`, binding with
`+ip`, the default port, and `-insecure` disabling VAC. Disabling VAC does
not establish that player authentication is disabled. The same document
says `-nomaster` also disables server queries, so omit it for query tests.
Do not infer that LAN mode accepts an unauthenticated Rust client: measure
the actual local reference's admission behavior.

Downloaded original binaries, assets, process output, packet payloads,
authentication material, and resulting captures stay private and untracked.
Do not run original code through REA's disassembly/decompilation
route. REA's earlier inability to open the ISO is not evidence about any
network format.

## Implementation and validation order

This is a **project plan**, not a statement of completed compatibility:

1. Implement bounded connectionless query codecs and exercise them with
   independently authored synthetic datagrams, including truncation,
   challenges, reordered/duplicate split parts, and total-size limits.
2. Against the loopback reference, perform one bounded query exchange at a
   time. Compare only documented framing and fields. Keep payloads local;
   export reviewed pass/fail categories and protocol/version facts only.
   A project-to-project query test is not an interoperability result.
3. Identify the final pre-Steam client/server reference pair and observe a normal local
   connection. Separate query challenges, join challenges, acceptance,
   sign-on, and actual play. Keep genuine authentication opaque and local.
4. Close the selected version's channel/munging gaps with documented or observed
   evidence before enabling a network channel. Then validate original
   client to Rust server and Rust client to original server separately. Reuse
   that original-wire implementation for Rust-to-Rust tests; retain the
   original-to-original baseline rather than allowing matching Rust bugs to
   define compatibility.
5. Validate channel reliability with bounded local packet loss, duplication,
   reordering, and reconnects. Derive all limits from target evidence or
   identify them explicitly as project constraints.
6. Add delta state, movement commands, prediction/reconciliation, and the
   complete deathmatch lifecycle. Compare weapon behavior, damage,
   scoring, pickups, respawn, chat, spectator behavior, map transitions,
   and intermission under local multiplayer scenarios.

Until these gates pass, describe the work as protocol research, discovery and
bounded payload parsing. Do not advertise a complete deathmatch implementation
or replace the requested original wire protocol with an unlabelled custom
protocol.

[P1]: https://developer.valvesoftware.com/wiki/Server_queries
[P1A]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries
[P2]: https://int64.org/docs/gamestat-protocols/halflife.html
[P3]: https://seclists.org/bugtraq/2002/Jun/254
[P4]: https://wiki.alliedmods.net/index.php?title=Half-Life_1_Engine_Messages&oldid=11325
[P4G]: https://wiki.alliedmods.net/Half-Life_1_Game_Events
[P5]: https://forums.sourceruns.org/t/goldsrc-package-2-4/2634
[P6]: https://www.half-life.com/en/halflife/25th
[P7]: https://partner.steamgames.com/doc/features/auth
[P8]: https://partner.steamgames.com/doc/sdk/uploading/distributing_gs
[P9]: https://developer.valvesoftware.com/wiki/SteamCMD
[P10]: https://steamcommunity.com/sharedfiles/filedetails/?id=2028698488
[P11]: https://slackdocs.slackware.nl/howtos%3Agames%3Ahalflife_dedicated_server
[P12]: https://developer.valvesoftware.com/wiki/CHILLMODEA/Pages/Command_line_options_%28GoldSrc_Dedicated_Server%29
[P13]: https://www.slipgate.de/download/HLTV-Readme.txt
[P14]: https://www.slipgate.de/hltv/hltv_FAQ_admins.html
[P15]: https://www.filefactory.org/tfc/file/776
[P16]: https://store.steampowered.com/oldnews/183
[P17]: https://archive.org/download/hlds_l_3111_full_202503/hlds_l_3111_full.bin
[P18]: https://archive.org/download/goldsrc_hlds_won_linux/hlds_l_3111d_update.tar.gz
