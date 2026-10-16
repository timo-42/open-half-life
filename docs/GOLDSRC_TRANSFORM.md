# Experimental full-word transform profiles

Reviewed 2026-10-16; separate protocol-46 cohort accepted 2026-10-17.
This is an independently authored mathematical model
from controlled black-box experiments using project-authored synthetic
command text. It describes a bounded word operation, not a complete GoldSrc
channel or a playable client. The [protocol ledger](GOLDSRC_PROTOCOL.md)
records the broader compatibility requirement and remaining gates.

## Reference and supported scope

The historical reference pair was the separately downloaded official Steam
app-90 HLDS/HLTV distribution, build ID **15961496**, advertising protocol **48**,
observed on 2026-10-16. The build ID identifies the distribution, not a
GoldSrc engine build number. Reviewed whole launcher/executable digests
identify those files only; they do not fingerprint every dynamically loaded
runtime component. Do not generalize these observations to retail/WON or
every server advertising protocol 48. The user's ISO remained unexecuted.
The separately measured protocol-46 cohort below has its own profile and
uses the unchanged mathematical operation; these historical observations
are not evidence for the user's selected pre-Steam target.

The measured direction is the official spectator proxy to its server, for
ordinary selected packets with first-word bit 30 clear. The experimental
prefix profile projects low-30-bit candidate counters and requires both to
be below the project guard 2^29. Counter width and wrap remain unresolved.
The low byte used below belongs to the first candidate counter; no reliable
acknowledgement or authentication meaning is assigned to it.

## Mathematical definition

Offsets start at byte zero of the **UDP payload**, excluding IP/UDP headers.
Bytes 0 through 7 are the existing selected prefix view. Eight is not a
verified complete channel-header size. Let:

- `i` be a zero-based word index after that view;
- `s = 8 + 4*i` be the absolute UDP-payload offset of that word;
- `q = first_counter & 255`;
- `C_i[j]` and `P_i[j]` be cipher and candidate plaintext bytes at index
  `j = 0..3` within the word;
- `pi = [3, 2, 1, 0]` be the destination plaintext index for cipher index `j`;
- `M[r][j]` be the fixed position mask at absolute offset residue `r`;
- `K(q) = [q, 0, 0, q]`.

The 16 four-byte functional masks have a 64-byte position period. Residues
are `0, 4, ..., 60`. Their single implementation copy is in
[word_transform.rs](../crates/ohl-net/src/word_transform.rs); the table is
not duplicated here. The first processed word uses residue 8, followed by
12 through 60, then 0 and 4. Relative word index zero must not select mask
residue zero.

Decode each complete word with bytewise XOR, in this order:

```text
T_i[j]       = C_i[j] XOR M[s % 64][j] XOR K(q)[j]
P_i[pi[j]]   = T_i[j]                         for j = 0..3
```

The exact inverse is:

```text
C_i[j] = P_i[pi[j]] XOR M[s % 64][j] XOR K(q)[j]
```

The mask and key indices refer to the cipher byte index, before the
permutation. XOR is not integer addition. No endian conversion is applied
to these four data bytes; only the candidate prefix counter interpretation
uses the separately selected little-endian profile.

These independently chosen synthetic words exercise the first position and
the period boundary. Remove the cipher-lane mask and key to obtain `T`,
then place its bytes at `P[3], P[2], P[1], P[0]`.

| Absolute `s` | `q` | Plaintext `P` | Cipher `C` | Intermediate `T` |
| --- | --- | --- | --- | --- |
| 8 | `5A` | `10 21 32 43` | `43 2A 21 4A` | `43 32 21 10` |
| 64 | `C7` | `19 A2 3B C4` | `03 63 F2 DE` | `C4 3B A2 19` |

Offsets are decimal; `q`, `P`, `C` and `T` entries are hexadecimal. These
are authored mathematical fixtures, not original packet samples or
observations at those absolute offsets.

## Word boundaries and opaque tails

For datagram length `L >= 8`, the candidate range has `L - 8` bytes. The
operation processes exactly `floor((L - 8) / 4)` complete words. The final
`(L - 8) % 4` bytes are an **opaque original tail** of length zero through
three. They remain untouched and are not exposed as decoded plaintext.
Leaving them untouched is this bounded codec's policy, not a verified
statement of the original engine's tail rule.

Encoding accepts complete words only. It must reject a partial word without
adding padding, constructing a prefix or modifying counters. The word
operation does not identify svc boundaries, decode a whole channel payload,
send packets or authenticate a peer. Its output must not automatically enter
the server-message reader.

The `ohl-net::word_transform` API requires an explicit word profile and a
`PacketPrefix` from its matching prefix profile:

| Prefix profile | Word profile |
| --- | --- |
| `ExperimentalProtocol48` | `ExperimentalProtocol48HltvToHlds` |
| `ExperimentalProtocol46Linux3111d` | `ExperimentalProtocol46Linux3111dHltvToHlds` |

`WordTransformError::ProfileMismatch` rejects a foreign prefix before output
changes. `decode_full_words` writes only complete candidate words into
caller storage and returns them separately from `OpaqueTail` borrowed from
the original input. `encode_full_words` returns encoded full words only;
its input length is independent of the prefix's old remainder length.
The project cap is 1,392 complete-word bytes, consistent with the existing
1,400-byte datagram cap and selected eight-byte view. Output capacity and
partial-word errors are explicit. Byte-wrapper Debug output is redacted.
Second-word bit 30 and both bit-31 flags are not key inputs; this assigns
them no transform or channel meaning.

Applying the candidate operation to initial offsets 8 through 23 for the
historical protocol-48 cohort, or 8 through 13 for protocol 46, extrapolates
measured known text. Higher counters, other low-counter bytes, longer bodies
and further period repetitions are also inferred applicability. Synthetic-text
windows do not verify every byte range to which a generic word operation
can be applied. In particular, the bytes immediately after the selected
prefix may include additional header or framing data.

## Historical protocol-48 derivation and validation

The coordinator inferred the permutation, counter-byte dependence and
position masks numerically using only known project-authored synthetic
plaintext. Each mask position was reconstructed from covered known-text
blocks, with repeated position coverage and cross-session comparisons.
The parameters describe the fixed candidate function; they are not a
captured byte slice, resource content, authentication material or a
per-session secret. No engine/SDK/reimplementation source, disassembly or
decompilation supplied an algorithm or constant.

The complete frozen model artifact has SHA-256:

```text
42f60ab9020a4698895916e95264cb13a00de25ee2015f3e89a3e1a28510b3c0
```

The calibration included 60 synthetic-pattern packets. Nine packets from
the earlier 12-packet synthetic-command run also helped select phase and
permutation; only three of that earlier run were unused at that step.
Consequently its 12/12 result is not a fully held-out check of the complete
model. That run's held-out status for the separate prefix model must not be
reused as a transform-validation claim.

The first fully held-out check used a fresh run after the complete model
was frozen. An independent inverse used the frozen model and its specified
operation convention without fitting or trying alternatives. Results were:

- 60/60 exact expected synthetic texts, lengths 96 through 103 bytes;
- 57 distinct low-counter bytes, including one low-byte decrease transition;
- all 16 mask positions covered by complete known-text words;
- all four packet-length and text-length residues, 15 observations each;
- one earlier unmatched command-sized candidate explicitly excluded from
  the text comparisons; and
- zero known-text samples entering the final incomplete word.

The complete held-out capture SHA-256 is:

```text
e8ca1f1c095611f38376a0fcac5169193919cdb3e6a8ffb6c7f6bd3229c5e275
```

The capture had 779 relay pairs, equal across the two boundaries, and an
18,229 ms call-stamp span. Its IP/UDP envelopes and timestamps were
synthesized by an application relay. It was not NIC acquisition or a wire
timing measurement. A low-byte decrease does not establish full-counter
wrap behavior. Text-length residue coverage does not establish tail
plaintext coverage; the tails remain unverified.

A separate footer-only follow-up used 15 aligned packets from run 004 as
calibration, then froze its prediction before a fresh reversed-text run 005.
It matched 60/60 predicted footers and 45/45 raw tails: 15 each of lengths
one, two and three, spanning 46 distinct byte values. That capture had 776
equal relay pairs and two earlier nonmarker exclusions; its complete
SHA-256 is `2339b78becd15e92291a4b5f80d60134b8494d1a54fcf4a6ca44e4914282060b`.
This narrower footer cohort establishes no arbitrary-body tail identity
rule. No footer content/constants or tail API are added. The current
`OpaqueTail` contract and run 004's zero known-text tail coverage remain
unchanged.

A separate supporting intervention applied 376 first-counter-plus-one
rewrites with independently checked repairs to 1,105 complete words, and
recorded 301 checked inverse-echo rewrites during about 18.8 seconds of
continued exchange.
The complete repaired capture SHA-256 is
`db5191fce54994074d23e293c07c36504fc6920eda2d8374f54780f60b0e7d8e`.
This is a bounded correlation consistent with the candidate model; it does
not prove endpoint command consumption or complete plaintext semantics.

## Historical protocol-48 Rust implementation checks

The final locked `ohl-net` run passed 65 tests: 13 new synthetic word-codec
tests plus the prior 52 query/prefix/message tests. One explicit original
query-reference test remained ignored. Strict all-target crate Clippy,
targeted formatting and diff checks passed; the workspace lock was unchanged.

The final integration run, `cargo test --workspace --locked`, passed 3,294
tests with 38 ignored across 254 result summaries. Full workspace formatting,
strict workspace/all-target Clippy and cargo-deny checks also passed, with
no workspace-lock change. These checks do not establish a gameplay connection.

A separate private Rust replay of the fully held-out capture matched all
60 selected datagrams' complete-word buffers against the independent frozen
model, all 60 known synthetic texts, and all 60 inverse encoded-word buffers.
Sixty tail-preservation and sixty output-sentinel checks passed. These check
the bounded buffer contract; they do not validate tail plaintext. No capture
or recovered message was added to the Rust fixtures or automatic test suite.

## Separate protocol-46 cohort

The 2026-10-17 Linux HLDS 3.1.1.1 base-plus-d-update spectator reference
reported native version 3.1.1.1/protocol 46, without a confirmed build.
[The protocol ledger](GOLDSRC_PROTOCOL.md#protocol-46-spectator-packet-cohorts)
records its distribution hashes, baseline and synthetic-run capture digests,
warnings and relay limits. Both spectator directions support only the
selected prefix model; the word profile covers proxy-to-server packets
with first bit 30 clear.

The existing frozen model matched 60/60 unique authored texts, lengths
96..103, totaling 5,962 known bytes at 60 distinct low-counter bytes. All
16 mask slots had fully known complete words; each packet/text length
residue had 15 samples. Known text began at absolute UDP offset 14 and ended
at offsets 110..117 exclusive. Two earlier command-sized candidates were
excluded. No low-byte decrease or known text in a partial tail was observed.
No coefficients were fitted, and the sole implementation table is unchanged.

The explicit protocol-46 production-profile replay matched all 60 known
texts/5,962 bytes and passed 60 inverse-region, 60 output-sentinel and
60 opaque-tail preservation checks. This is post-implementation conformance
to the frozen hypothesis, not independent held-out discovery or tail
plaintext verification. The focused locked crate run passed 111 tests with
one reference test ignored, including six new synthetic profile/association
tests; strict crate Clippy, targeted formatting and diff checks passed.

## Clean-room and compatibility boundary

[CLEAN_ROOM.md](CLEAN_ROOM.md) permits independent behavior observations of
lawfully obtained software. The reviewed mathematical rules and minimal
functional coefficients have that provenance. [MEDIA_IMPORT.md](MEDIA_IMPORT.md)
continues to protect original captures, recovered messages, calibration
intermediates, internal names and credentials. They remain local and ignored;
tests and worked examples use authored synthetic data only. A small or
transformed packet sample does not become a publishable fixture.

Server-to-proxy transforms, first-bit-30-set packets, general partial-word/tail rules,
complete header extent, compression, fragmentation, reliable delivery,
sign-on/authentication and ordinary-player commands remain unverified.
This word model supplies no normal-player or complete gameplay-channel
interoperability result.
