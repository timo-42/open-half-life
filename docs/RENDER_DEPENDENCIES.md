# Renderer dependencies

This document covers the Rust renderer's crates.io dependencies
(`crates/ohl-render`, `crates/ohl-app`). It also keeps a short historical
note about the C++/CMake "P4 renderer dependency facade", removed along with
the rest of the C++ tree; see the final section below.

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

## P4 renderer dependency facade (C++/CMake, removed)

The historical C++/CMake "P4 renderer dependency facade"
(`cmake/RenderDependencies.cmake`: a pinned SDL 3/Vulkan-Headers/volk/
MoltenVK acquisition and link-surface probe) was removed along with the rest
of the C++ tree once the Rust port reached M1 parity (see
`docs/MILESTONES.md`). It has no Rust counterpart: the Rust renderer table
above links wgpu and winit directly through Cargo, with no equivalent
acquisition facade to document, since `cargo deny check licenses` and pinned
`Cargo.lock` entries already cover provenance and licensing for those crates.
The full historical description remains available in git history immediately
before the "Remove C++ implementation superseded by the Rust workspace"
commit, for reference if a future Windows/macOS packaging story needs the
same kind of pinned, hash-checked acquisition record this facade kept for
SDL/Vulkan-Headers/volk/MoltenVK.
