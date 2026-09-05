# Format sources

Compatibility implementations record their public technical basis here.

## ECMA-167 / UDF preflight

- [ECMA-167, second edition, December 1994](https://www.ecma-international.org/wp-content/uploads/ECMA-167_2nd_edition_december_1994.pdf): Part 2 section 9 (volume recognition structures), Part 3 sections 7.2 (descriptor tags), 8.4.2 (volume descriptor sequences), and 10.2 (anchor volume descriptor pointer).
- [ECMA TR/71, February 1998](https://www.ecma-international.org/wp-content/uploads/ECMA_TR-71_1st_edition_february_1998.pdf): sections 2.4 through 2.6 describe the UDF Bridge recognition sequence, anchor placement, and descriptor profile for read-only media.
- [ECMA TR/112-7, December 2023](https://ecma-international.org/wp-content/uploads/ECMA_TR-112-7_1st_edition_december_2023.pdf): consolidated UDF requirements used during validation review.

The project-owned parser is deliberately only a bounded NSR02 structural
preflight; it does not claim full UDF conformance. Full read-only filesystem
interpretation is delegated to the independently maintained `libudfread`
dependency described in `THIRD_PARTY_NOTICES.md`.

## Media provenance digest

- [NIST FIPS PUB 180-4, Secure Hash Standard](https://doi.org/10.6028/NIST.FIPS.180-4):
  SHA-256 message padding, schedule, compression function, and digest encoding.

The project-owned SHA-256 implementation is used only to assign a stable
content identity to user-provided media. Known-answer tests use the published
empty-string and `abc` vectors plus a multi-block vector; no proprietary data
is used in tests.

## Cabinet and component-selection parsing

Project-owned cabinet or installer component-selection parsing logic requires
public-source provenance before implementation begins. Add the lawfully public
technical source to this file with its version, stable link, relevant sections,
and the exact project behavior it supports. Owned-media observations alone may
confirm a bounded compatibility result, but they must not supply record layouts,
field meanings, selection rules, names, paths, constants, fixtures, or parsing
algorithms.

No public technical source for project-owned cabinet or component-selection
parsing is currently recorded in this file. Until one is reviewed, such logic
must remain unimplemented. The current third-party metadata adapter is
experimental, default-off, and runs in-process. It is not a format specification
and must not be used to reverse-engineer project-owned parsing rules. Before it
may process media for production import, it must run behind the reviewed,
constrained worker isolation required by `MEDIA_IMPORT.md`. Any permanent
fixture must be independently authored synthetic data or approved public data
with compatible provenance and redistribution terms, as required by
`CLEAN_ROOM.md` and `MEDIA_IMPORT.md`.

### Reviewed, lawfully usable, but not sufficient for container parsing

- [RFC 1950, ZLIB Compressed Data Format Specification version 3.3, May 1996](https://www.rfc-editor.org/rfc/rfc1950)
  (Deutsch, Gailly; IETF Informational): sections 2.2 (zlib stream header, `CMF`/`FLG`,
  window size) and 2.3 (Adler-32 trailer).
- [RFC 1951, DEFLATE Compressed Data Format Specification version 1.3, May 1996](https://www.rfc-editor.org/rfc/rfc1951)
  (Deutsch; IETF Informational): sections 3.2.1 through 3.2.7 (block headers, stored,
  fixed and dynamic Huffman blocks, length/distance codes).
  Terms: IETF RFC text, unrestricted reproduction; the specifications are public
  standards. They may support a project-owned zlib/DEFLATE stream decoder only. They
  describe no cabinet header, directory, descriptor table, volume-splitting, or string
  table layout, and they do not establish that any particular installer container
  frames its payload as zlib or raw DEFLATE.
- Official InstallShield product documentation, "Overview of InstallScript .cab and
  .hdr Files" (Flexera/Revenera, current help library,
  <https://docs.revenera.com/installshield/helplibrary/IsCabViewOverview.htm>):
  states only the roles of the paired files, that the header carries project,
  file, component, and feature information, and that the viewer can report on them.
  Terms: vendor copyright, viewable reference; cite facts only, do not reproduce.
  It may support naming and role vocabulary in project documentation. It publishes
  no field, offset, table, ordering, or compression detail, so it cannot support
  parsing.

### Reviewed and rejected

- Unshield project wiki (<https://github.com/twogood/unshield/wiki>): reviewed
  2026-09-05 and empty; no format documentation pages exist. Its `README.md` records
  project history and build steps and contains no field-level layout. NOT usable.
- Unshield source headers and helpers (`cabfile.h`, `helper.c`) and any derived
  restatement of them: this is implementation source, not documentation, and the
  project states its format knowledge came from inspecting InstallShield binaries and
  from the tools below. Its MIT license does not cure that provenance. NOT usable as a
  format source for project-owned parsing.
- file(1) magic patch for InstallShield `*.HDR`, `*.LID`, `*.INS`, `*.TAG`
  (Jörg Jenderek, file mailing list, 2021-11-04,
  <https://mailman.astron.com/pipermail/file/2021-November/000628.html>): documents a
  signature and several early header offsets, but the author states the information
  was taken from the Unshield implementation and from TrID definitions, and that no
  official or complete documentation exists. Provenance is derived, not independent.
  NOT usable.
- `i5comp` (fOSSiL, 1999) and `i6comp` (Morlac and DarkSoul, 2002) and their
  documentation: these tools and their format knowledge originate from examination of
  InstallShield's own binaries, and they call vendor decompression code directly.
  Disassembly-derived provenance. NOT usable.
- Third-party wiki summaries (Just Solve the File Format Problem "InstallShield CAB";
  File Formats Wiki "Cabinet file"; forum posts describing a "ZLIBX" block framing):
  each restates the tools above and cites no independent primary source. The common
  claim that the payload is zlib is itself reported as the result of running `strings`
  over proprietary InstallShield binaries by a third party. Provenance unclear or
  derived. NOT usable.

No independently authored public specification of the InstallShield 5/6-era cabinet
container was located. Every layout-bearing candidate traces back to disassembly of
proprietary installer binaries, so project-owned cabinet or component-selection
parsing must remain unimplemented.

## ECMA-119 / ISO 9660 and Joliet

- [ECMA-119, 4th edition, June 2019](https://ecma-international.org/publications-and-standards/standards/ecma-119/)
  (also published as the 3rd edition, December 2017; both are freely available and
  technically equivalent for the structures used here): section 6.2 (logical sectors
  and logical blocks), section 7.2.3 (both-byte-order numerical values), section 8.1
  (volume descriptor set, standard identifier `CD001`, descriptor version and types),
  section 8.4 (primary volume descriptor: volume identifier, volume space size,
  logical block size, path table size and locations, root directory record), section
  8.5 (supplementary volume descriptor and its escape sequences), section 8.3 (volume
  descriptor set terminator), section 6.8 and section 9.1 (directory records: record
  length, extended attribute record length, extent location, data length, file flags,
  file unit size, interleave gap size, volume sequence number, file identifier length
  and identifier), section 7.5/7.6 (file identifiers, the `;` separator and the file
  version number), and section 6.8.2.2 (the `0x00` and `0x01` identifiers for the
  current and parent directory).
- Microsoft, "Joliet Specification: Extensions to the CD-ROM Recording Standard
  ISO 9660:1988", version 1, May 1995 (public specification, widely mirrored, for
  example at <https://web.archive.org/web/20070308034426/http://bmrc.berkeley.edu/people/chaffee/jolspec.html>):
  the reserved escape sequences `%/@`, `%/C` and `%/E` in the supplementary volume
  descriptor, UCS-2 big-endian encoding of the volume identifier and of directory
  record file identifiers, and the requirement that a Joliet directory hierarchy
  duplicates the primary one.

Project behaviour supported: the bounded preflight in `src/media` classifies media
that carry no ECMA-167 volume recognition sequence by reading the volume descriptor
set from logical sector 16, requiring a primary volume descriptor, a descriptor set
terminator, a 2,048-byte logical block size, a volume space size that fits inside the
pinned source, path table locations inside the volume, and a root directory record
whose extent lies inside the volume. The project-owned reader in `src/vfs`
(`Iso9660Archive`) walks directory extents one logical sector at a time, checks every
both-byte-order pair for agreement, rejects extended attribute records, multi-extent
files and interleaved layouts, strips the `;N` version suffix, skips the `0x00` and
`0x01` structural entries, decodes Joliet UCS-2 identifiers to UTF-8, and prefers a
valid Joliet tree over the primary tree. Anti-abuse limits (directory depth, entries
and bytes per page, records examined, directory extent sectors, decoded name bytes)
and extent-based cycle detection bound every walk.

The implementation is clean room: it was written from the two public documents above
only. No other implementation's source code was consulted, and no bytes, names,
listings or layouts from any real medium appear in the code, tests or this file. All
test fixtures are generated by the project-authored synthetic image builder in
`tests/media/synthetic_media_test_support.hpp`.
