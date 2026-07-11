# Third-party software

## libudfread 1.2.0

Open Half-Life uses VideoLAN's libudfread for read-only access to UDF image
files. By default CMake fetches the official 1.2.0 release archive and verifies
its published SHA-512 digest. Developers may explicitly select a system
libudfread 1.1.2 or newer, which is less reproducible. Upstream source:
<https://code.videolan.org/videolan/libudfread>.

libudfread is copyright the VLC authors and VideoLAN and is licensed under
the GNU Lesser General Public License, version 2.1 or later. Its complete
license text is included in the fetched source archive as `COPYING`.

Repository-authored Open Half-Life code remains MIT licensed. Distributors
must separately comply with libudfread's LGPL terms, including the applicable
relinking or shared-library requirements. Release packaging will preserve the
license text and corresponding-source information.

The current install target includes relevant license texts, but it is not a
release artifact. Static-link relinking and corresponding-source delivery must
be verified as part of M9 before binary distribution.

## Unshield 1.6.2

The default-off experimental cabinet adapter uses Unshield to read
InstallShield 5+ metadata through project-provided read-only VFS callbacks.
Version 1.6.2 is pinned at commit
`51de441ba6893f11026d4671ccef9e8e2a4634fa`. Upstream source:
<https://github.com/twogood/unshield>. Unshield is copyright David Eriksson
and other contributors and is licensed under the MIT license.

## zlib 1.3.1

Unshield uses zlib for decompression. CMake fetches zlib 1.3.1 pinned at commit
`51b7f2abdade71cd9bb0e7a373ef2610ec6f9daf`. Upstream source:
<https://github.com/madler/zlib>. zlib is copyright Jean-loup Gailly and Mark
Adler and is distributed under the zlib license.

Unshield's optional built-in MD5 implementation contains RSA Data Security,
Inc.'s 1991–1992 MD5 reference code. It is used for archive integrity checks;
the upstream copyright and permission notice is retained in the fetched
source, and this documentation identifies it as the RSA Data Security, Inc.
MD5 Message-Digest Algorithm as required by that notice.

## P4 graphics dependencies

The accepted P4 dependency facade is described in
[`docs/RENDER_DEPENDENCIES.md`](docs/RENDER_DEPENDENCIES.md). It is not evidence
that the production renderer is integrated or that the game is playable.

### SDL 3 shared runtime

SDL is distributed under the zlib license. The default facade accepts an
unpinned system SDL package at version 3.4 or newer and requires an imported
shared-library target. It does not fetch or redistribute SDL source and does
not claim a URL, content hash, or exact version for the selected system
package. A distributor that incorporates the selected shared runtime must
include the SDL license and every additional notice applicable to components
incorporated in that actual package.

On Windows x64, an optional reproducible mode uses the official SDL 3.4.10 VC
binary archive. Its complete 105-member archive manifest is validated, but
only 61 selected public headers, `SDL3.dll`, `SDL3.lib`, and `LICENSE.txt` are
extracted and independently hash checked. No SDL source, static library, test
library, debug symbols, or non-x64 binary is exposed. Redistributing the DLL
requires the extracted SDL license and any additional notices that apply to
code incorporated in that exact upstream binary. The file allowlist is not a
substitute for an artifact-specific binary notice audit.

### Vulkan-Headers 1.4.350

The Vulkan API headers are pinned to commit
`8864cdc896bbc2a9b6eb36b3218fc9ef57908d77`. Upstream source:
<https://github.com/KhronosGroup/Vulkan-Headers>. The facade acquires 24
individually pinned raw files and never a full repository archive. Those files
include upstream `LICENSE.md` and the complete Apache License 2.0 text at
`LICENSES/Apache-2.0.txt`; both must accompany redistributed headers and must
be included in applicable binary-package notice material.

### volk 1.4.350

The private Vulkan dispatch loader is pinned to commit
`3ca312a4f38baa63d8006b6905abbeeb89c8087d`. Upstream source:
<https://github.com/zeux/volk>. The complete accepted input is the individually
hash-checked `volk.h`, `volk.c`, and `LICENSE.md`; any additional file in the
volk root is rejected. volk is distributed under the MIT license. Its exact
license notice must accompany redistributed source and binaries containing
the compiled volk implementation.

### Externally supplied MoltenVK runtime

On Apple platforms the facade accepts an absolute path to a pre-provisioned
dynamic MoltenVK `.dylib` or framework binary and separately requires an
absolute path to its supplied license file. The runtime is explicitly
unpinned: this repository does not acquire a MoltenVK archive and does not
claim a MoltenVK version, commit, URL, or content hash. Structural validation
requires an arm64-capable Mach-O dynamic library, but does not identify its
release, audit its license contents, or inventory its transitive components.

Any package containing that runtime must include the mandatory supplied
MoltenVK license and the licenses, notices, and attribution files required by
the actual supplied binary and its actual transitive dependencies. The
packager must derive that inventory from the selected artifact; no MoltenVK
1.4.1 or fixed SPIRV-Cross, SPIRV-Tools, SPIRV-Headers, cereal, or
Vulkan-Headers version is implied by the facade. Native Apple Silicon
embedding, signing, rpath, load, and execution qualification remains required
before release packaging.
