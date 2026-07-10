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
