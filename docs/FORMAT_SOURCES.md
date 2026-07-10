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
