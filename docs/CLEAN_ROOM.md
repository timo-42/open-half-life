# Clean-room development policy

All implementation work in this repository must be independently authored
from lawful, public documentation and behavior observed by running legally
obtained software. This project must not contain Valve source code, code
derived from decompilation, or proprietary game content.

Contributors must follow these rules:

1. Do not decompile, disassemble, or copy from the original engine.
2. Do not use leaked or otherwise unauthorized source material.
3. Do not commit game media, extracted assets, installer files, CD keys, or
   detailed dumps containing proprietary content.
4. Record the public sources used when implementing a file format or behavior.
5. Keep compatibility observations factual and minimal. A reviewed sanitized
   report may contain carefully bounded aggregates or complete-source digests,
   project-defined error codes, and documented container or filesystem
   type/version facts. It must not contain internal names, paths, or file
   contents.
6. Treat all user-provided media as read-only data. Never execute installers,
   drivers, scripts, or binaries found on it.
7. Require a clean-room provenance review before adding any name or path literal
   derived from user media. The review must identify an independently authored
   synthetic origin or a lawfully public source with compatible use terms. A
   literal observed only in private media must not enter source, tests,
   documentation, fixtures, snapshots, comments, or issue reports.

Compatibility code should be reviewable without access to copyrighted source
or assets. New third-party dependencies must have licenses compatible with
the project and be recorded before distribution.

The media preflight is based on the published ECMA-167 second edition, ECMA
TR/71, and the consolidated UDF requirements in ECMA TR/112-7. Exact sections
and links are recorded in `FORMAT_SOURCES.md`. Synthetic test images contain
only descriptors authored for this project and no game data. Full UDF reading
uses the open-source libudfread dependency and is isolated behind the VFS API.
Experimental InstallShield cabinet interpretation uses the MIT-licensed
Unshield library; the installer itself is never run. This adapter is disabled
by default pending malformed-input hardening or process isolation.

`MEDIA_IMPORT.md` defines the local proprietary-data boundary for ISO bytes,
internal identifiers, extracted output, parser communication, diagnostics,
sanitizer artifacts, and fuzzing material. Those rules also prohibit logs,
telemetry, automatic uploads, committed proprietary fixtures, and redistribution.
Only a reviewed sanitized report may leave the local boundary. Its allowed
fields are limited to carefully bounded aggregates or complete-source digests,
project-defined error codes, and documented container or filesystem
type/version facts.

Validated source media is fingerprinted with SHA-256 and identified in the
local cache only by its digest, size, sanitized filesystem description, and
sanitized volume label. The provenance manifest deliberately omits the source
path and contains no game bytes. Cache contents remain local and ignored by
the repository policy.
