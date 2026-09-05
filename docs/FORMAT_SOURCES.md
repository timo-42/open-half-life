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

## GoldSrc BSP v30 and WAD3

- "Unofficial Quake Specs", Quake Documentation version 3.4, Section 4 "Level
  Map Models" (compiled by Uwe Girlich with contributions credited on the
  document including Olivier Montanuy, Frank Maddin, Simon Cooke, and Frans
  P. de Vries; public since 1996), mirrored at
  <https://www.gamers.org/dEngine/quake/spec/quake-spec34/qkspec_4.htm>:
  documents the id Software BSP29 `dheader_t` (magic-free `version` field
  followed by 15 `{offset, length}` directory lumps in a fixed order:
  entities, planes, miptex, vertices, visilist, nodes, texinfo, faces,
  lightmaps, clipnodes, leaves, lface, edges, ledges, models), the
  `plane_t`, `miptex_t`/`mipheader_t`, `vertex_t`, run-length-encoded
  `vislist` (a `0` byte begins a run of empty/all-invisible bytes whose count
  follows in the next byte), `node_t`, `surface_t` (texinfo), `face_t`,
  `lightmap` byte array, `clipnode_t`, `dleaf_t`, `lface` mark-surface array,
  `edge_t`, signed `ledges` (surfedges), and `model_t` layouts. This project's
  `bsp30` module reuses this lump order, directory shape, and every struct
  layout except where GoldSrc BSP v30 diverges (see below); it is the basis
  for `Bsp30Header`, `Plane`, `Vertex`, `Edge`, the signed `Surfedge`, and the
  `Model` accessor.
- Valve Developer Community, "BSP (GoldSrc)"
  (<https://developer.valvesoftware.com/wiki/BSP_(GoldSrc)>, community wiki
  content under CC BY-NC-SA 3.0, reviewed 2026-09-05; the article is marked a
  stub and carries no separate author/date byline): documents that GoldSrc
  raised the BSP version field to `30`, keeps the same 15-lump directory
  shape as BSP29, and diverges from BSP29 in the following struct fields used
  by this project: `BSPNODE` (`iPlane: uint32`, signed `int16` children,
  `int16` mins/maxs, `uint16` firstFace/nFaces), `BSPCLIPNODE` (`int32`
  `iPlane`, two `int16` children), `BSPLEAF` (`int32 nContents`, `int32
  nVisOffset`, `int16` mins/maxs, `uint16` iFirstMarkSurface/nMarkSurfaces,
  four `uint8 nAmbientLevels`), `BSPFACE` (`uint16 iPlane`, `uint16
  nPlaneSide`, `uint32 iFirstEdge`, `uint16 nEdges`, `uint16 iTextureInfo`,
  four `uint8 nStyles`, `int32 nLightmapOffset`), `BSPTEXTUREINFO` (two
  `VECTOR3D`+`float` shift pairs, `uint32 iMiptex`, `uint32 nFlags`), the
  `BSPMIPTEX` in-lump header (`char szName[16]`, `uint32 nWidth/nHeight`, four
  `uint32 nOffsets` that are `0` when the texture is stored externally in a
  WAD instead of embedded), and that the lighting lump is an array of RGB
  byte triples rather than BSP29's single-channel samples. This project's
  `bsp30` module uses this page for `Node`, `Clipnode`, `Leaf`, `Face`,
  `TexInfo`, the embedded/external `Miptex` header, and the RGB `Lighting`
  lump; the RLE visibility algorithm itself is taken from the BSP29 source
  above, which this page does not restate. This page documents only the
  20-byte `BSPMIPTEX` header (name/width/height/four offsets) and the
  external-texture convention (all four offsets `0`); it does not describe
  the embedded pixel/palette body. For an embedded miptex's mip pixel arrays
  and trailing palette this project reuses the WAD3 miptex body layout
  documented below, since GoldSrc embeds the identical miptex body inline in
  the BSP texture lump instead of inside a WAD3 directory entry.
- "Unofficial Half-Life WAD3 and SPRITE file format specification", Revision
  05, Author: Yuraj, Date: 15.01.2012, published at
  <http://yuraj.ucoz.com/> (stable mirror used for this review:
  <https://yuraj11.github.io/old-files/half-life-formats.pdf>): documents the
  WAD3 header (`"WAD3"` magic, `uint32` texture count, `uint32` directory
  offset), the 32-byte directory entry (`uint32` offset, `uint32` compressed
  length, `uint32` full length, `uint8` type, `uint8` compression, 2 bytes
  padding, 16-byte null-padded name), the entry type bytes (`0x40`
  spraydecal/tempdecal, `0x42` qpic/cached, `0x43` miptex, `0x46` font), and
  the miptex texture body (16-byte name, `uint32` width/height, four `uint32`
  mip offsets relative to the texture body start, indexed pixel data sized
  `width*height`, `width/2*height/2`, `width/4*height/4`, `width/8*height/8`
  for the four mip levels, a trailing `uint16` palette-length field observed
  as `256`, and 768 bytes of 24-bit RGB palette). This project's `wad3`
  module implements `Wad3Header`, `DirectoryEntry`, the entry-type constants,
  and `Miptex` directly from this document.

The project-owned `bsp30` and `wad3` decoders in `crates/ohl-formats` are
zero-copy, bounds-checked readers over these documented layouts; every count,
offset, and index is validated against the actual lump/file bounds before use
(see `crates/ohl-formats/src/error.rs` for the fixed rejection reasons). All
test fixtures are synthesized in-process by this project's own writers in
`crates/ohl-formats/src/test_support.rs`; no bytes, names, or other content
from any game installation are used or committed.

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
