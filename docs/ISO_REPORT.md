# Sanitized media inspection report

This report describes the local, user-provided initial development medium
without redistributing its content.

Inspection date: 2026-07-10. The outer image was inspected read-only with
7-Zip 25.01 and the project detector; libudfread 1.2.0 was then used to verify
that the root filesystem can be mounted and enumerated. Commands emitted no
file contents and their raw listings are not retained in the repository.
Unshield 1.5.1 was used locally to test the visible cabinet set; the integrated
code pins Unshield 1.6.2 and parses the same cabinet through VFS callbacks. A
separate read-only check on 2026-07-11 reconfirmed the complete-source size and
SHA-256, UDF classification, 2,048-byte cluster size, and outer-image
integrity. It did not rerun the project detector, libudfread enumeration, or
Unshield checks.

## Medium

- Edition: Half-Life Game of the Year Edition, user-identified build 929
- Container size: 389,873,664 bytes
- SHA-256: `eb8331a333b33c43165705d387f2331d9eb941f808cd305541ec825b767dcab7`
- Filesystem: read-only UDF 1.02
- Logical block size: 2,048 bytes
- Source-control status: ignored; no media content is tracked
- Outer-image integrity: archive reader reported no structural errors

## High-level layout

The root contains a late-1990s Windows installer layout, compressed installer
archives, setup metadata, and optional period hardware-support material. The
medium is suitable as an import source, but does not expose an immediately
portable installed game tree.

The outer filesystem contains 1,804 files in 53 directories, totaling
383,349,646 logical bytes. Most entries are legacy Windows setup or driver
material; the principal game payload appears to be held in installer
containers. This is a layout-based inference, not a claim based on extracting
their contents. The filename's build-929 label has not yet been independently
verified.

No executable from the medium was run. No archive was extracted, and no file
content, CD key, or proprietary asset is included in this report.

## Engineering implications

The engine now performs a bounded ECMA-167 preflight, mounts the UDF tree
read-only through libudfread, and creates a content-addressed, metadata-only
provenance cache entry. The default-off experimental InstallShield adapter can
parse cabinet metadata in place through Unshield. Invalid cabinet descriptors
are counted and make entry-count-bounded adapter output unsuitable for import
instead of being silently skipped.
That adapter is disabled by default because Unshield has not been audited for
adversarial cabinets. M2 still needs parser isolation and safe, atomic payload
extraction. The current standard-library link checks reduce accidental
traversal but do not yet provide race-proof OS-level no-follow operations for
hostile cache directories. Imported data must live outside the source tree and
never be treated as redistributable output.
