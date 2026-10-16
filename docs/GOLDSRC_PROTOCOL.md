# Original Half-Life multiplayer protocol evidence

Research date: 2026-10-16. The requested result is a Rust deathmatch client
and server using the original Half-Life network protocol. Compatibility means
exchanging the original wire messages with an identified reference runtime.
A custom Rust-to-Rust transport, a working server browser, or a successful
challenge response does not establish that result. Full deathmatch gameplay
and original-server gameplay interoperability remain unverified.

This ledger supplements [BEHAVIOR_SPEC.md](BEHAVIOR_SPEC.md). The
[clean-room policy](CLEAN_ROOM.md) and [local data boundary](MEDIA_IMPORT.md)
apply throughout. Its sources are published prose and packet-format tables.
No engine source, decompilation, disassembly, or recovered implementation is
an implementation source. The user's ISO remains read-only, and binaries or
installers from it must not be executed.

## Evidence and version boundaries

- **Documented** means the linked public source describes the fact. It does
  not establish applicability to every Half-Life build.
- **Observed** requires a repeatable black-box experiment against an
  identified, lawfully acquired runtime. No normal-player gameplay comparison
  or decoded-channel compatibility result is recorded. Opaque official HLTV
  proxy traffic is recorded below.
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

For older targets, the GameStat author's [Half-Life Protocol][P2] separately
documents the ASCII requests `details`, `rules`, and `players` after the
four-byte all-ones prefix. Its `m` info response includes five strings
(address, server name, map, game directory, description), player counts and
protocol version, then server/mod metadata. Its players reply contains an
index, terminated player name, signed 32-bit score, and 32-bit floating-point
time per player. Its rules reply has a 16-bit count and string key/value
pairs. These historical forms must be tested independently of modern queries.
Do not commit the published packet dumps or observed server/player strings as
fixtures; construct synthetic fields instead.

## Documented join outline

Luigi Auriemma's [2002 firsthand protocol description][P3], concerning
Half-Life through the stated 1.1.0.9 target, describes this sequence:

1. Send `FF FF FF FF` followed by `getchallenge\n` over UDP.
2. Receive a challenge value.
3. Send a textual connection request with protocol, challenge,
   authentication material, and player information.
4. Receive acceptance or rejection.

That report documents the broad handshake and the default server port
27015. It does not define a complete modern Steam connection request,
ticket generation, exact acceptance layout, sign-on exchange, or gameplay
channel. Do not generate fake ownership credentials, reuse another client's
tickets, or interpret an accepted connectionless request as a spawned player.

## Server-message evidence and remaining gaps

AlliedModders' [Half-Life 1 Engine Messages][P4] publishes server-message
numbers and field tables. Examples include no-payload `SVC_NOP` (1),
disconnect reason string (2), floating-point server time (7), and sign-on
number (25). It describes bit-packed client state, entity state/baselines,
resources, and delta-description exchange. Game-specific messages are
documented separately in [Half-Life 1 Game Events][P4G]. These are useful
payload references after a compatible channel has been established; they
are not a specification of UDP channel framing.

The following still require admissible documentation or reviewed black-box
evidence. Existing project code or another engine's implementation cannot
close an evidence gap:

- Sequenced datagram framing, sequence-number width/wrap rules, reliable
  acknowledgement flags, duplicate/out-of-order handling, retransmission,
  and choke behavior for each target version.
- Reliable normal/file fragment headers, identifiers, offsets, reassembly,
  interleaving, compression, and size limits. Connectionless query splits
  above do not specify these channel fragments.
- GoldSrc byte munging: transforms, constants, direction, key/sequence
  relationship, covered byte range, and treatment of trailing bytes. No
  reviewed public prose source here establishes these details.
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
- Current Steam authentication, ticket transport, ownership checks, and
  server admission. Valve's [authentication documentation][P7] explains
  genuine session tickets but does not specify GoldSrc's on-wire embedding.
  It is not evidence that a pure Rust client already authenticates.

Do not fill these gaps using Quake netchan documentation, a Source protocol,
automated summaries of engine source, or a guessed XOR/munging algorithm.
Such substitution would not verify original Half-Life compatibility.

## Obtaining a separate black-box reference

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
3. Establish a legal original client reference and observe a normal local
   connection. Separate query challenges, join challenges, acceptance,
   sign-on, and actual play. Keep genuine authentication opaque and local.
4. Close the listed channel/munging gaps with documented or observed
   evidence before enabling a network channel. Then validate original
   client to Rust server and Rust client to original server separately.
5. Validate channel reliability with bounded local packet loss, duplication,
   reordering, and reconnects. Derive all limits from target evidence or
   identify them explicitly as project constraints.
6. Add delta state, movement commands, prediction/reconciliation, and the
   complete deathmatch lifecycle. Compare weapon behavior, damage,
   scoring, pickups, respawn, chat, spectator behavior, map transitions,
   and intermission under local multiplayer scenarios.

Until these gates pass, describe the work as protocol research and discovery
support. Do not advertise a complete deathmatch implementation or replace the
requested original wire protocol with an unlabelled custom protocol.

[P1]: https://developer.valvesoftware.com/wiki/Server_queries
[P1A]: https://web.archive.org/web/20230831123627id_/https://developer.valvesoftware.com/wiki/Server_queries
[P2]: https://int64.org/docs/gamestat-protocols/halflife.html
[P3]: https://seclists.org/bugtraq/2002/Jun/254
[P4]: https://wiki.alliedmods.net/Half-Life_1_Engine_Messages
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
