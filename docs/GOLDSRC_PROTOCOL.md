# Original Half-Life multiplayer protocol evidence

Research began 2026-10-16; target and query-codec review: 2026-10-17.
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
reference, not a completed runtime comparison. The exact Linux server release,
build and observed protocol still need verification. Valve's
[Steam release announcement][P16] dates the public client release to
September 12, 2003. Do not equate the selected WON target with a later Steam
build by changing a protocol constant.

All protocol-48 observations below remain historical evidence for their stated
Steam server/proxy distributions. They do not verify the selected pre-Steam
target. A separately acquired public full-client update is being inspected
through static archive extraction and component-provenance checks. No new
pre-Steam reference pass or player execution is recorded. The ISO execution
prohibition remains unchanged.

## Required client/server pairings

All four pairings must run the original wire protocol and the complete
deathmatch lifecycle:

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

`ohl-net::channel::decode_datagram` requires the caller-selected
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
inferred word operation for the tested official proxy-to-server direction,
protocol 48, app-90 distribution build 15961496, observed 2026-10-16. The
model was calibrated only from synthetic text; its first fully held-out
check matched 60/60 known texts, with one earlier candidate excluded. The
specification records the frozen model/capture digests, exact operation and
inverse, measured coverage and extrapolated applicability.
Scoped Rust codec checks and private replay results are recorded there
separately from the prior parser/prefix validation above.

The bounded codec operates on complete words after the selected eight-byte
view and leaves a final zero-through-three-byte tail opaque. No known text
entered that incomplete tail, so tail plaintext is unverified. First-word
bit 30 must be clear. No packet send path, automatic svc decoding, reliable
channel, authentication or normal-player compatibility follows from this
result. Other directions, header extent, higher-counter/body-range
applicability and complete transform variants remain evidence gaps.

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
playable-client result. The existing CLI/browser still uses the earlier A2S
query client; no host/join gameplay CLI was added.

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
  [full-word proxy-to-server model](GOLDSRC_TRANSFORM.md) does not establish
  other directions/variants, every counter or body range, complete header
  extent or trailing-byte rules. No reviewed public prose source here
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
no player join or replacement reference pass has completed.

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
