# Frame profiling

Run the release binary against the same imported payload and start map for
each comparison. A headless benchmark needs a working graphics adapter and
does not open a window or write a screenshot:

```sh
cargo run --release -p ohl-app -- --training --benchmark-seconds 30
```

Add the ordinary `--iso PATH` or `--payload-root PATH` argument when needed.
The benchmark renders at 1280×720, warms up for five wall-clock seconds, then
measures completed frames for the requested duration (1–3600 seconds). Each
frame advances the simulation by 1/60 second with no input and waits for all
GPU commands to finish. A level change or player death stops the benchmark
with a fixed warning and no summary, keeping results within one scene.

The `frame profile device` line identifies the adapter, backend and render
size. The final `headless_completed` profile reports FPS, nearest-rank
median/p95 frame times, average simulation time, CPU rendering/submission
time and GPU completion wait. The wait is CPU wall time waiting for work
already submitted; it is not a GPU timestamp measurement. Acquisition and
UI/presentation times are zero in headless mode. No pixel readback or image
encoding is included.

The optimized build also reports resource uploads during the measured window.
Brush preparations, static bytes, and texture uploads should remain zero
after warmup. World lightmap uploads occur only when a style actually used
by the map changes brightness. Window profiling reports these counters
cumulatively for the current level.

For comparisons, use three 30-second runs of each build on the same adapter.
Headless FPS measures serialized completed-frame throughput; windowed FPS
also depends on presentation, vsync and display resolution. Simulation
advances per rendered frame, so faster builds advance more game time over a
wall-clock benchmark; use matching-pose captures to check visual output.

```sh
cargo run --release -p ohl-app -- --training --profile-frames
```

Window profiling logs every two seconds. Median/p95 describe frame-start
intervals; the average stages cover simulation, surface acquisition, CPU
rendering/submission, and UI/presentation. Rendering/submission is not a GPU
duration. The profiling window does not request focus or grab the mouse,
and mouse motion does not move the camera; keyboard movement still works
after focusing it. Normal interactive runs keep their usual input behavior.

For a deeper CPU breakdown, set `OHL_PROFILE_RENDER_STAGES=1` on either
command. Every 120 rendered frames it reports average preparation and
submission time for lightmaps, world geometry, studio models, sky, brushes,
liquids, sprites, and the viewmodel. These timings include driver blocking
inside submissions and are not GPU timestamps. The first report includes
warmup frames; use later reports to assess steady-state work.

Brush geometry and lightmaps are prepared once per level; submodels share
the world renderer's diffuse texture bindings. Visible brushes share one
render pass and submission, with separate uniforms for each instance's
placement and render properties. Changing levels or quickloading discards the cache.
Frustum rejection uses all eight transformed bounds corners, conservatively
retaining models with invalid bounds.

## Training benchmark, 2026-09-08

Measured on an AMD Ryzen 7 7735HS running Linux x86-64 with Mesa 26.1.6
llvmpipe (LLVM 21.1.8), using software Vulkan at 1280×720. Each version ran
three 30-second trials with five seconds of warmup per trial; no compilation
or other test jobs ran during measurements. The baseline was `d4c4857` with the same app timing
instrumentation added and its renderer unchanged.

| Version | FPS across trials | Median frame time | p95 frame time |
| --- | ---: | ---: | ---: |
| Baseline | 1.2 | 819.2–833.6 ms | 853.6–878.2 ms |
| Resource caching, lightmap caching, brush culling | 29.0–29.5 | 33.6–34.1 ms | 36.6–37.4 ms |
| Above plus one-pass brush batching | 62.9–66.5 | 14.6–15.7 ms | 17.4–18.2 ms |

The final trials performed zero static brush uploads or world lightmap
updates after warmup. Separate pass profiling reduced CPU time attributed
to the brush pass from approximately 25.2 ms to 2.5 ms. Submission timing
can include waits for preceding GPU work, so individual stages are not
independent GPU costs.

The eight-frame training spawn capture and the `training_start.txt`
scripted movement capture were each byte-for-byte identical to baseline.
These measurements establish completed-frame throughput on software
Vulkan; they do not establish Metal or windowed performance on a Mac, and
the p95 results do not imply every frame meets a 16.7 ms budget.

GPU-enabled tests for `ohl-app`, `ohl-engine`, `ohl-render`, and `ohl-world`
passed: 485 tests, zero failures. The 19 ignored tests comprise 15 duplicate
GPU entrypoints (their opt-in counterparts ran), two tests requiring a
separately installed confined worker image, and two manual fixture writers.
The explicit quickload resource-lifecycle GPU test also passed separately.
Workspace formatting and Clippy (`--all-targets --all-features -D warnings`)
passed, as did an app test rerun after the final profiling-code cleanup.
