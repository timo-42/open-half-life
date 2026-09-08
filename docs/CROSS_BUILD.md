# Cross-building release binaries from Linux

`scripts/cross-build.sh` links the `open-half-life` application on an x86-64
Linux host for the two supported non-Linux release targets:

- Windows x86-64, using Rust's `x86_64-pc-windows-gnu` target and MinGW-w64;
- macOS Apple Silicon, using Rust's `aarch64-apple-darwin` target, osxcross,
  and an Apple SDK obtained from a local Xcode or Command Line Tools install.

This is a real release link, not `cargo check`. The script requires the output
file to exist and uses `file` to reject an artifact that is not PE32+ x86-64
or Mach-O arm64 as appropriate. The Windows check also rejects dependencies
on the MinGW runtime DLLs that are not shipped with the application. The
script prints a SHA-256 digest for every output. A cross-built binary cannot
be executed on the Linux builder, so the native Windows and macOS CI jobs
remain responsible for tests and runtime smoke checks.

## Common prerequisites

Use an x86-64 Linux machine with this repository's pinned Rust toolchain,
Cargo, `rustup`, `file`, and `sha256sum`. Install both Rust standard libraries
on the pinned toolchain:

```sh
rustup target add --toolchain 1.98.1 \
  x86_64-pc-windows-gnu aarch64-apple-darwin
```

The explicit `--toolchain` matters when another toolchain is the rustup
default: adding a target to the default does not add it to the repository's
`rust-toolchain.toml` override.

By default Cargo writes under `target/cross`. CI and concurrent builds should
give each run a separate directory:

```sh
export CROSS_BUILD_TARGET_DIR="$RUNNER_TEMP/open-half-life-cross"
```

## Windows x86-64

On Debian or Ubuntu, install the linker and headers:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 file
```

Then build and verify the executable:

```sh
scripts/cross-build.sh windows
```

The artifact is
`$CROSS_BUILD_TARGET_DIR/windows/x86_64-pc-windows-gnu/release/open-half-life.exe`.
`WINDOWS_CROSS_CC`, `WINDOWS_CROSS_AR`, and `WINDOWS_CROSS_OBJDUMP` can select
non-default MinGW-w64 tools.

A Linux CI job can run the same sequence directly. This complements the
native Windows job; it does not replace execution on Windows.

## macOS Apple Silicon

Use an official SDK from a local Xcode or Command Line Tools installation and
keep it outside the repository. The validated build used the Command Line
Tools macOS 15.4 SDK and macOS 11.0 as the minimum deployment target.

Resolve the physical SDK directory on the Apple machine before archiving it;
`MacOSX.sdk` is commonly a symlink:

```sh
sdk_link="$(xcrun --sdk macosx --show-sdk-path)"
sdk_root="$(cd "$sdk_link" && pwd -P)"
tar -c -C "$(dirname "$sdk_root")" "$(basename "$sdk_root")" \
  | xz -T0 -3 > "$(basename "$sdk_root").tar.xz"
```

Transfer that archive privately to the Linux builder. Build osxcross from the
pinned revision used for validation:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  bash clang cmake python3 git make patch sed tar gzip xz-utils bzip2 cpio \
  libxml2-dev libssl-dev zlib1g-dev liblzma-dev libbz2-dev llvm-dev uuid-dev \
  file

git clone https://github.com/tpoechtrager/osxcross.git "$HOME/osxcross"
git -C "$HOME/osxcross" checkout 27d21e4977c9751d01199c7a226a6faf494c3dd9
git -C "$HOME/osxcross" submodule update --init --recursive
cp /private/path/MacOSX15.4.sdk.tar.xz "$HOME/osxcross/tarballs/"

UNATTENDED=1 BUILD_FLAVOR=latest ENABLE_ARCHS=arm64 \
OSX_VERSION_MIN=11.0 TARGET_DIR="$HOME/osxcross-target" \
  "$HOME/osxcross/build.sh"
```

Point the cross-build script at the installed toolchain:

```sh
OSXCROSS_ROOT="$HOME/osxcross-target" \
  scripts/cross-build.sh macos
```

The application artifact is
`$CROSS_BUILD_TARGET_DIR/macos/aarch64-apple-darwin/release/open-half-life`.
The script also builds the standalone parser-worker manifest directly for the
same target and installs it with mode `0555` at
`release/libexec/open-half-life/ohl-media-parser-worker`. The release and two
`libexec` directories are normalized to mode `0755` as required by the
launcher. `MACOS_CROSS_CC` and `MACOS_CROSS_AR` can select explicit osxcross
wrappers.

Keep native macOS CI because only macOS can run the pair, exercise Metal and
CoreAudio, and test the Seatbelt confinement profile. The native
`cargo xtask worker-image` command also performs host-specific image audits;
the Linux cross-build performs format and architecture checks instead.

## Validated toolchains and outputs

The commands above were run on an x86-64 Debian Linux host with Rust 1.98.1 on
September 8, 2026. The Windows build used Debian's MinGW-w64 GCC 16 toolchain.
The Apple build used osxcross
`27d21e4977c9751d01199c7a226a6faf494c3dd9`, the official Command Line Tools
macOS 15.4 SDK, and a minimum deployment target of macOS 11.0.

The script produced and checked these release artifacts:

- Windows application: PE32+ console executable for x86-64; SHA-256
  `4b0303dcf22a91d3d74932358ffce7ea732fb6d56fbcc98232f678067af2d74d`.
  Its PE import table contained Windows system DLLs and no GCC, libstdc++, or
  winpthreads runtime DLL.
- macOS application: Mach-O 64-bit arm64 executable; SHA-256
  `0e2b4c30320cd721fc8d7a4b61988d4effb132f2562b7c0c8cab43fe657b010e`.
- macOS parser worker: Mach-O 64-bit arm64 executable; SHA-256
  `38949c9e0243df7e9bdef250ed3a52de9650f240adea52c997b9a94e187c8809`.
  Its only dynamic dependency was `/usr/lib/libSystem.B.dylib`.

After copying the outputs from the Linux builder, both applications completed
native `--version` smoke runs on their target OS. On Apple Silicon, the
cross-built worker also passed all five production worker-lifetime scenarios
under the real Seatbelt launcher. Artifact hashes include the source and
`OHL_VERSION` inputs, so a later revision is expected to produce different
values.
