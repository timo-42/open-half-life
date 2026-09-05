# Renderer dependencies

This document covers two separate stacks:

1. the Rust renderer's crates.io dependencies (`crates/ohl-render`,
   `crates/ohl-app`), which are the direction of travel; and
2. the historical C++/CMake "P4 renderer dependency facade"
   (`cmake/RenderDependencies.cmake`), retained below while the C++ tree is
   still buildable.

## Rust renderer crates

The Rust renderer links no C libraries and contains no `unsafe` code: wgpu and
winit are safe Rust APIs and the workspace's `forbid(unsafe_code)` lint applies
to `ohl-render` and `ohl-app` unchanged. Versions are pinned exactly in each
crate's `Cargo.toml` (`=x.y.z`) and resolved reproducibly through `Cargo.lock`;
`cargo deny check licenses` is the enforced gate.

| Crate | Pinned version | License | Used by | Purpose |
| --- | --- | --- | --- | --- |
| `wgpu` | `=30.0.1` | MIT OR Apache-2.0 | `ohl-render` | Graphics API: instance/adapter/device/surface, WGSL pipelines, textures, buffers |
| `winit` | `=0.30.13` | Apache-2.0 | `ohl-app` (`dev-tools` feature only) | Window creation, keyboard and pointer events |
| `pollster` | `=1.0.1` | Apache-2.0 OR MIT | `ohl-render` | Blocks on wgpu's adapter/device/map futures without pulling in an async runtime |

`winit` is Apache-2.0 only (not dual-licensed); that is on `deny.toml`'s allow
list and is compatible with the repository's MIT-licensed code, but binary
distributors must ship its `LICENSE` text with the notices required for every
crate actually linked, which `Cargo.lock` enumerates.

### Backend policy

Per `PROMPT.md`, `ohl-render` requests wgpu's Vulkan backend on Linux and
Windows and its Metal backend on macOS, then widens the search to
`wgpu::Backends::PRIMARY` if the preferred backend produced no adapter.
MoltenVK is not used: the Rust port links no C libraries, so a
Vulkan-over-Metal translation layer is not an option.

Every entry point that needs an adapter returns a fixed `RenderError::NoAdapter`
rather than panicking, so a machine with no GPU — the normal case for a CI
runner — can still build and unit-test the whole crate. The offscreen render
test skips itself on such a machine, and is `#[ignore]`d by default with an
`OHL_RENDER_GPU_TEST=1` opt-in.

### Texture formats and gamma

GoldSrc palettes and lightmaps are already gamma-encoded and are composited in
that space with no overbright multiplier, so world textures and the lightmap
atlas are uploaded as `Rgba8Unorm` (never `...Srgb`) and the shader multiplies
the two samples directly. When a window surface offers only an sRGB format the
shader converts the result back to linear first, so the hardware's sRGB encode
round-trips to the same pixels the non-sRGB path produces.

## Audio backend (`ohl-audio`)

`ohl-audio` (the M5 sound package: bounded WAV decoding, the software mixer,
and output device selection) follows the same "no C libraries" rule as
`ohl-render`, but reaches it differently because of how each platform exposes
its audio API:

- **Linux has no way to reach ALSA (or PulseAudio/PipeWire) without linking a
  C library.** `cpal`'s only Linux backend goes through the `alsa` crate,
  whose `alsa-sys` dependency locates and links `libasound.so` through a
  build-time `pkg-config` lookup — an actual link-time dependency on a
  third-party C library, not a runtime `dlopen`. That is a different, more
  direct kind of coupling than wgpu's Vulkan backend, which reaches the
  Vulkan loader through `ash`'s runtime `libloading`-based `dlopen` and links
  no Vulkan C library at build time. Because there is no pure-Rust
  alternative to `alsa-sys`'s linkage on Linux (and the `jack`/`pulseaudio`
  backends have the same problem with their own C libraries), `cpal` is
  declared only as a `[target.'cfg(any(target_os = "macos", target_os =
  "windows"))'.dependencies]` entry in `crates/ohl-audio/Cargo.toml`. Cargo
  never resolves or builds `cpal` (or `alsa`/`alsa-sys`) at all when the
  target is Linux, so a Linux CI runner needs no `libasound2-dev` package and
  never touches `pkg-config` for this crate. There is consequently no
  feature flag to gate: there is no Linux `cpal` code path to gate in the
  first place. `ohl-audio::device` always resolves to the crate's own
  `NullSink` on Linux.
- **macOS (CoreAudio) and Windows (WASAPI) are reached without linking any
  third-party C library.** `cpal`'s backends for those two platforms go
  through `coreaudio-rs`/`objc2` and the official `windows` crate
  respectively: both are safe(r) Rust bindings directly to the OS's own
  audio API surface (an Apple framework, and a Windows COM/system DLL
  interface), the same tier of exception already accepted for wgpu's
  Vulkan-loader and Metal access in `ohl-render`. Nothing here vendors,
  fetches, or links a redistributable third-party library the way libudfread
  or Unshield do.

`ohl_audio::device::open_default_device` therefore only ever attempts a real
`cpal` device on macOS/Windows, and always falls back to `NullSink` (used
directly and exclusively on Linux) when no such device is available — so a
headless CI runner on any of the three platforms builds, lints, and runs the
full test suite without an audio device.

## P4 renderer dependency facade (C++/CMake)

This document records the accepted dependency facade implemented by
`cmake/RenderDependencies.cmake` and its isolated probe. It does not mean that
the production renderer is integrated or that the game is playable. Native
Windows x64 and macOS Apple Silicon qualification has not yet been performed.

The facade publishes project-owned `ThirdParty::SDL3`,
`ThirdParty::VulkanHeaders`, and `ThirdParty::volk` targets. On Apple platforms
it also publishes `ThirdParty::MoltenVKRuntime`. Including the module has no
network or target side effects; `ohl_add_render_dependencies()` performs the
validation and is deliberately single-use.

## Acquisition and trust summary

| Component | Accepted input | Pinning and trust boundary |
| --- | --- | --- |
| SDL system mode | Imported shared SDL 3.4 or newer | Unpinned system package; no source archive, URL, version pin, or content hash is claimed. |
| SDL official-binary mode | Official Windows x64 VC archive for SDL 3.4.10 | Archive URL and SHA-256 are pinned; all 105 archive members are checked and only 64 individually hash-checked files are extracted. |
| Vulkan-Headers 1.4.350 | 24 raw files at commit `8864cdc896bbc2a9b6eb36b3218fc9ef57908d77` | Every file has its own SHA-256. A full repository archive is never fetched or accepted. |
| volk 1.4.350 | Three raw files at commit `3ca312a4f38baa63d8006b6905abbeeb89c8087d` | Every file has its own SHA-256 and the cache root is closed to any fourth file. |
| MoltenVK | Absolute path to an already provisioned dynamic `.dylib` or framework binary, plus an absolute license-file path | Explicitly unpinned external input. No MoltenVK version, archive, URL, commit, or hash is asserted. |

System SDL and external MoltenVK remain trust inputs supplied by the builder.
The checks described below narrow their shape; they do not establish their
publisher, provenance, full contents, or fitness for release.

## SDL shared-only policy

`OHL_RENDER_DEPS_SDL_MODE` accepts exactly `SYSTEM` (the default) or
`OFFICIAL_BINARY`. Both modes expose shared SDL only. Static SDL targets and
SDL source builds are outside this facade.

### System mode

The system path locates an SDL CMake config and adjacent config-version file.
`SDL3_DIR`, when supplied, must be absolute. The package must report version
3.4 or newer and define an imported `SDL3::SDL3-shared` target whose runtime
locations exist and have the platform's dynamic-library form. Windows packages
must also provide existing `.lib` or `.dll.a` import libraries.

This is compatibility validation, not provenance validation. The accepted
package version is reported as package metadata, but no `OHL_PINNED_VERSION`,
`OHL_PINNED_SHA256`, or `OHL_SOURCE_URL` property is published. The facade does
not hash the system runtime or headers, audit the package's complete file set,
or validate its signatures. Distributors incorporating that shared library
must collect and ship the license and notices that apply to the actual package
they use.

### Official Windows x64 binary mode

`OFFICIAL_BINARY` is limited to 64-bit Windows with an explicit processor
spelling of `x86_64`, `AMD64`, or `x64` (case-insensitive). It acquires only:

- URL: `https://github.com/libsdl-org/SDL/releases/download/release-3.4.10/SDL3-devel-3.4.10-VC.zip`
- archive SHA-256: `e2b336b10b037934af98308027410732ef7b22f2c6697d58092aa1c209fae7d7`

The wrapper compares the archive's complete sorted member list with an exact
105-member manifest before extraction. It extracts exactly 64 members: all 61
non-test public SDL headers selected by the wrapper, `lib/x64/SDL3.dll`,
`lib/x64/SDL3.lib`, and `LICENSE.txt`. Every exposed member has its own pinned
SHA-256 and the extraction root rejects any unlisted file.

The wrapper never exposes the archive's x86 or arm64 binaries, PDB files,
`SDL3_test` library, CMake package files, readmes, `.git-hash`, test headers,
OpenGL/OpenGL ES headers, or other unselected surfaces. It rejects exposed
test, static (`.a`), PDB, and non-x64 library paths and checks that no C, C++,
Objective-C, or Objective-C++ source appears in the extracted prefix. The
result is an imported shared target backed only by the selected x64 DLL and
import library; it is not an SDL source or static build.

The archive manifest check is deliberately stricter than extracting known
paths: an upstream member addition, deletion, or rename fails configuration.
The archive and all exposed hashes must be reviewed together for any update.
Packaging the DLL requires its supplied `LICENSE.txt` and any additional
notices applicable to code incorporated in that exact upstream binary. The
allowlist does not by itself prove the binary's internal composition.

## Vulkan-Headers raw-file allowlist

Vulkan-Headers is pinned to version 1.4.350 and commit
`8864cdc896bbc2a9b6eb36b3218fc9ef57908d77`. The production path downloads the
following 24 files individually from the commit-addressed
`raw.githubusercontent.com` base URL and checks each SHA-256:

| Raw path | SHA-256 |
| --- | --- |
| `LICENSE.md` | `ac24e5ea920e4318e4d02c4086ae51f53cfb03feed06c18df1019e7ada1ec7bc` |
| `LICENSES/Apache-2.0.txt` | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |
| `include/vulkan/vk_platform.h` | `949d517bb83e1d88fd4f1cef02bd3cb9ab50d44e8354cc68227cf2dccfdd3307` |
| `include/vulkan/vulkan.h` | `72b952cc6de70ee12d118d3095e80346b42ff01cfc6f2bbb37ec01800eab1da6` |
| `include/vulkan/vulkan_core.h` | `6d2ba4755774b1d129da6b8e661268b494d2d609df6217c6b6485acf7666b6c2` |
| `include/vulkan/vulkan_macos.h` | `00f532e9b7229488ac9932c2b391d74e9e8566ed91d0dbbad10e15dc356c9293` |
| `include/vulkan/vulkan_metal.h` | `8f84536896b8d3c6c49883ce34242f8922840617d0b184b338c0f83da5bfaaaa` |
| `include/vulkan/vulkan_wayland.h` | `d43ca1c45cd4b2e1119c168b780e2864205ad8119e5f42d9d99aa29d2654a3f2` |
| `include/vulkan/vulkan_win32.h` | `0b11442e7efa0d5b6732b9964edf6dbdf3f6cf1dd58afe7572cc9b7f6de24317` |
| `include/vulkan/vulkan_xcb.h` | `1e1525fc64826ab3ea9e8a6ee6a618ef14db3e27397fcf8fb070bb1ddaf00c0b` |
| `include/vulkan/vulkan_xlib.h` | `82346aaa80a5c984b6a26f55c7c363c65be43029da539ba0fbdfea9840051cca` |
| `include/vulkan/vulkan_xlib_xrandr.h` | `da5d5311bcffa49c614e9f397784e44668466890aeaf4e54c2e0064c20259089` |
| `include/vk_video/vulkan_video_codec_av1std_decode.h` | `f03abf49fcaf2bd179d48d768164f9494264b58401b98899e8860ba297a1e7aa` |
| `include/vk_video/vulkan_video_codec_av1std_encode.h` | `40f84c98d0341246ead3e864e1378df47d861505c5c139da5ed85c352d5e2300` |
| `include/vk_video/vulkan_video_codec_av1std.h` | `9b4ebcef0d6844b226803fa91b4c4c8bf9eb941aa31b19cea48ded0886a8f9e2` |
| `include/vk_video/vulkan_video_codec_h264std_decode.h` | `8c4682860954d6bfc603e200d9e12586bc57265bd37b73a12b1ecf0c1ad42721` |
| `include/vk_video/vulkan_video_codec_h264std_encode.h` | `75cc54e489b1ec7e3c635680cabaf7c9c2ddd4071a485440e5368e76ede30a85` |
| `include/vk_video/vulkan_video_codec_h264std.h` | `f6691f82e4637adda20e56d56951c673cc1ecdf2fa77e09cc8eba4b1f45118c1` |
| `include/vk_video/vulkan_video_codec_h265std_decode.h` | `926a24d94afed2b1ab030e81c3b9e2e6730cca530f7a5a5bd821e7b896b93a41` |
| `include/vk_video/vulkan_video_codec_h265std_encode.h` | `8a56e0c78496affa84eca39ec0f5636df681c04623dd0291aeeadd012df324be` |
| `include/vk_video/vulkan_video_codec_h265std.h` | `01b5f3d0fd9e273a68ed90f898ba94c05c0305280a753bf95fab60d60727c22e` |
| `include/vk_video/vulkan_video_codecs_common.h` | `be6d2495d19e96aca6aa5c11e5c418d1cd72beac23fe19eb9169c06e2843f0af` |
| `include/vk_video/vulkan_video_codec_vp9std_decode.h` | `c3929bbdd0ab79128c7f9bfb9641c9a017b3983529315b771caaafc74fdbb89a` |
| `include/vk_video/vulkan_video_codec_vp9std.h` | `e18e20b197945929d5871836bfde923e38b350b09e1cbe430f1bd802d9eda34e` |

The cache root must contain exactly those files. In particular, loader-facing
`vk_icd.h` and `vk_layer.h`, any archive, and any shadow or extra file are
rejected. The full Apache License 2.0 text is therefore a pinned payload, not a
link to be resolved later. Both `LICENSE.md` and `LICENSES/Apache-2.0.txt` are
published to packaging. Header version 1.4.350 does not raise the facade's
declared Vulkan API baseline above 1.1.

## volk raw-file allowlist

volk is pinned to version 1.4.350 and commit
`3ca312a4f38baa63d8006b6905abbeeb89c8087d`. Its complete accepted root is:

| Raw path | SHA-256 |
| --- | --- |
| `volk.h` | `479bab725a424d54715fe9e54b1a69b344a9c36fb617c9aa2cc4289814d8acdf` |
| `volk.c` | `698571dde0c43bc296ab5997ffebf5d8176a9d9e586894590069dbae5ace3c68` |
| `LICENSE.md` | `04a0693a84f19e53d281ca98bbb0c86ca77251ab13769c6168e6684feb9a1436` |

Each file is fetched by its commit-addressed raw URL and independently hashed;
any fourth file fails configuration. `volk.c` is compiled once as project
target `ohl_volk_impl` with `VK_NO_PROTOTYPES` and `VOLK_NAMESPACE`. The target
links the platform dynamic-loading library where CMake requires one.

## External MoltenVK boundary on Apple platforms

The wrapper never downloads, extracts, or identifies a MoltenVK release.
Builders must set both `OHL_RENDER_DEPS_MOLTENVK_LIBRARY` and
`OHL_RENDER_DEPS_MOLTENVK_LICENSE_FILE` to absolute, existing, non-directory
paths. `OHL_RENDER_DEPS_MOLTENVK_STATIC_LIBRARY` is rejected.

The runtime path must end in `.dylib` or name a binary within a `.framework`.
Its bytes must be a structurally valid 64-bit Mach-O dynamic library with an
arm64 slice. Thin arm64 and bounded fat32/fat64, big/little-endian containers
are accepted; no-arm64, wrong file type, static archives, malformed ranges,
overlaps, truncation, and overflow are rejected. This establishes only that an
arm64-capable `MH_DYLIB` structure is present.

The resulting imported shared target is labeled `unpinned-external-dynamic`
and publishes no pinned version, hash, source URL, archive, or commit. The
wrapper records an `@rpath` runtime expectation and that Vulkan portability
enumeration is required. It does not validate code signing, install names,
framework layout, deployment target, loader behavior, license contents, or
the actual binary's transitive components. Those items, including embedding,
signing, runtime loading, and Apple Silicon execution, require native release
qualification. Packaging must include the supplied MoltenVK license and the
licenses/notices for the actual supplied binary and its actual transitives;
no fixed MoltenVK release or transitive set may be inferred from this facade.

## Fail-closed controls and final link surface

Before publishing any facade target, the wrapper validates all inputs and:

- rejects `CMAKE_PROJECT_TOP_LEVEL_INCLUDES`, which prevents dependency
  providers or top-level dependency injection from controlling acquisition;
- includes the selected SDL config directly and rejects pre-existing facade
  targets and pre-existing `SDL3::SDL3-shared`, `volk::volk`, or
  `Vulkan::Headers` targets;
- rejects case-insensitive `FETCHCONTENT_SOURCE_DIR_OHL_SDL3`,
  `..._VULKAN_HEADERS`, `..._VOLK`, and `..._MOLTENVK` overrides;
- requires `OHL_RENDER_DEPS_FETCH` to be exactly `ON` or `OFF` and requires an
  absolute `OHL_RENDER_DEPS_CACHE_DIR`;
- with fetch disabled, accepts already cached pinned files only after their
  hashes and closed roots pass, and fails if any required file is missing;
- prevents synthetic probe fixtures from substituting official SDL binary
  provenance.

`OHL_RENDER_DEPS_FETCH` governs only acquisition of the pinned raw files and,
in official Windows mode, the pinned SDL archive. It does not make unpinned
system SDL or external MoltenVK reproducible, and it is not a promise that a
fresh empty cache can configure offline.

The isolated probe verifies that a final executable consumes shared SDL, the
single volk implementation, required platform dynamic-loading libraries, and
on Apple the supplied dynamic MoltenVK. It rejects a final command containing
`-lvulkan`, `vulkan-1.lib`, or a direct `libvulkan` link. The facade records
`SDL_Vulkan_GetVkGetInstanceProcAddr` as the intended loader-function source
and `volkInitializeCustom` as the intended dispatch initialization entrypoint.
The probe validates those metadata values and the final link surface only; it
does not perform the SDL-to-volk runtime handoff. That handoff remains
unimplemented future renderer startup work. The wrapper publishes no Vulkan
loader target and records no `volkInitialize()` path.

This is dependency-facade and probe evidence only. Production renderer
startup, SDL window creation, Vulkan instance/device selection, portability
enumeration, runtime library deployment, and end-to-end rendering remain
future integration work.

## Packaging obligations and qualification limits

Binary distributors must make an artifact-specific inventory rather than
treating facade provenance properties as a complete legal or security audit:

- for system SDL, include the license and incorporated-component notices from
  the exact package selected by the release build;
- for the official Windows SDL DLL, include the extracted `LICENSE.txt` plus
  every notice applicable to code incorporated in that exact binary;
- include Vulkan-Headers' pinned `LICENSE.md` and full
  `LICENSES/Apache-2.0.txt` payloads;
- include volk's pinned `LICENSE.md`;
- on Apple, include the mandatory externally supplied MoltenVK license and
  all license/notice material required by that binary's actual transitives.

The Linux-hosted synthetic probe validates policy branches and link shape. It
does not replace native Windows x64 archive/link/load testing or macOS Apple
Silicon framework/dylib, rpath, signing, loader, and execution testing. Those
native gates remain mandatory before production renderer integration or
release packaging can be accepted.
