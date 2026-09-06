//! Offscreen render tests for the sky pass and the translucent water pass.
//!
//! Like `headless_render.rs`, both are `#[ignore]`d by default and also skip
//! themselves, without failing, when no adapter exists. Run them explicitly
//! with `cargo test -p ohl-render -- --ignored`, or opt in from the
//! environment with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_formats::test_support::Bsp30Builder;
use ohl_render::{
    FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, SkyRenderer, WorldRenderer,
};
use ohl_world::{BspLimits, SkyboxAsset, WorldBuildOptions, WorldModel};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

fn headless() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipping offscreen render test: {error}");
            None
        }
    }
}

/// A minimal 1x1 24-bit uncompressed TGA filled with `color`.
fn solid_tga(color: [u8; 3]) -> Vec<u8> {
    let mut bytes = vec![0u8; 18];
    bytes[2] = 2; // uncompressed true-color
    bytes[12] = 1;
    bytes[14] = 1;
    bytes[16] = 24;
    bytes.extend_from_slice(&[color[2], color[1], color[0]]);
    bytes
}

/// A single liquid (`!water`) quad in the XY plane at `z = 0`, spanning
/// `-64..64` on both axes, lit and facing up.
fn liquid_quad_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_edge(0, 0);
    b.add_embedded_texture("!water1", 16, 16, 220);

    for corner in [
        [-64.0, -64.0, 0.0],
        [64.0, -64.0, 0.0],
        [64.0, 64.0, 0.0],
        [-64.0, 64.0, 0.0],
    ] {
        b.push_vertex(corner);
    }
    for corner in 0..4u16 {
        b.push_edge(corner, (corner + 1) % 4);
    }
    for step in 0..4 {
        b.push_surfedge(1 + step);
    }
    b.push_texinfo([1.0, 0.0, 0.0], 0.0, [0.0, 1.0, 0.0], 0.0, 0, 0);
    let offset = i32::try_from(b.lighting.len()).unwrap();
    for _ in 0..25 {
        b.push_lighting_rgb(255, 255, 255);
    }
    b.push_face(0, 0, 0, 4, 0, [0, 0xFF, 0xFF, 0xFF], offset);
    b.push_marksurface(0);
    b.push_leaf(-1, -1, [-64, -64, 0], [64, 64, 16], 0, 1, [0, 0, 0, 0]);
    b.push_model(
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 16.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        1,
    );
    b.build()
}

fn overhead_camera() -> FreeFlyCamera {
    FreeFlyCamera {
        // Close enough over the `-64..64` quad (see `liquid_quad_bsp`) that
        // it fills a clear majority of the frame: at the default 75-degree
        // vertical FOV and this test's 4:3 aspect, a camera 200 units above
        // only ever put the 128-unit-wide quad across about an eighth of
        // the frame, so `liquid_pass_blends_over_the_cleared_background_*`
        // could never see enough blended pixels to satisfy its coverage
        // assertion regardless of the render pass's correctness.
        position: [0.0, 0.0, 90.0],
        yaw: 0.0,
        pitch: 89.0,
        ..FreeFlyCamera::default()
    }
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn sky_pass_fills_the_frame_when_nothing_else_is_drawn() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_sky_pass_test();
}

#[test]
fn sky_pass_fills_the_frame_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen sky render test");
        return;
    }
    run_sky_pass_test();
}

fn run_sky_pass_test() {
    let Some(context) = headless() else {
        return;
    };
    // Six distinct solid colours so any sampled direction reads a known
    // face.
    let colors: [[u8; 3]; 6] = [
        [200, 0, 0],
        [0, 200, 0],
        [0, 0, 200],
        [200, 200, 0],
        [0, 200, 200],
        [200, 0, 200],
    ];
    let tgas: Vec<Vec<u8>> = colors.iter().map(|&c| solid_tga(c)).collect();
    let refs: [&[u8]; 6] = core::array::from_fn(|i| tgas[i].as_slice());
    let skybox = SkyboxAsset::build(refs).expect("six solid faces decode");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let sky = SkyRenderer::new(&context, &skybox, OFFSCREEN_FORMAT).expect("sky renderer builds");

    // A bare depth buffer, cleared to the far value, standing in for "no
    // opaque geometry was drawn this frame" without depending on
    // `WorldRenderer`.
    let depth_texture = context.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test depth"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ohl_render::DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    context.queue.submit(Some(encoder.finish()));

    let camera = FreeFlyCamera::default();
    sky.render(&context, &camera, target.view(), &depth_view, WIDTH, HEIGHT);
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    // With nothing else drawn, the sky must cover the whole frame: every
    // pixel should be a saturated colour (one channel near 200, matching
    // one of the six faces) rather than the black clear colour.
    let sky_covered = rgba
        .iter()
        .filter(|pixel| pixel[0] > 100 || pixel[1] > 100 || pixel[2] > 100)
        .count();
    assert!(
        sky_covered > (WIDTH * HEIGHT) as usize * 9 / 10,
        "the sky should cover almost the entire frame when nothing occludes it"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn liquid_pass_blends_over_the_cleared_background() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_liquid_pass_test();
}

#[test]
fn liquid_pass_blends_over_the_cleared_background_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen water render test");
        return;
    }
    run_liquid_pass_test();
}

fn run_liquid_pass_test() {
    let Some(context) = headless() else {
        return;
    };
    let bytes = liquid_quad_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic liquid map parses");
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .expect("synthetic liquid map builds");
    assert!(
        !model.liquid_batches.is_empty(),
        "the quad is a liquid face"
    );
    assert!(model.batches.is_empty(), "there is no opaque geometry");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &model, OFFSCREEN_FORMAT).expect("renderer builds");
    let camera = overhead_camera();

    // The opaque pass draws nothing (no opaque batches) but still clears
    // colour and depth, establishing the background the liquid pass blends
    // over.
    renderer.render(&context, &model, &camera, target.view(), WIDTH, HEIGHT);
    renderer.render_liquid(&context, &camera, target.view(), WIDTH, HEIGHT, 0.0, 0.5);
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    // The clear colour is a near-black blue (red channel ~5/255); a 50%
    // blend of a bright, fully-lit texture over it must land clearly above
    // the background but (barring an additive-blend bug) below the
    // texture's own near-white brightness.
    let blended = rgba
        .iter()
        .filter(|pixel| pixel[0] > 20 && pixel[0] < 230)
        .count();
    assert!(
        blended > (WIDTH * HEIGHT) as usize / 4,
        "expected a meaningful part of the frame to show blended water pixels"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target itself stays opaque"
    );
}

/// A single, large, opaque, unlit wall quad facing the camera along `+X`,
/// far enough away (`WALL_DISTANCE`) that it does not fit inside
/// `sky.wgsl`'s `HALF_EXTENT`-sized cube.
const WALL_DISTANCE: f32 = 200.0;

/// Half the wall's extent on `y` and `z`; large enough that, at
/// `WALL_DISTANCE` and this test's 4:3 aspect ratio, it fills the whole
/// frame at this test's field of view (the wider, horizontal FOV needs more
/// than `WALL_DISTANCE` itself to fully cover the frame's corners).
const WALL_HALF_SIZE: f32 = 320.0;

/// A dark grey fill (`synthetic_palette`'s index `10`), chosen so it cannot
/// be confused with any of `run_sky_pass_test`'s saturated (~200) face
/// colours.
const WALL_TEXTURE_FILL: u8 = 10;

#[allow(clippy::cast_possible_truncation)]
fn distant_wall_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    b.push_plane([1.0, 0.0, 0.0], WALL_DISTANCE, 0);
    b.push_edge(0, 0);
    b.add_embedded_texture("ohlwall", 16, 16, WALL_TEXTURE_FILL);

    for corner in [
        [WALL_DISTANCE, -WALL_HALF_SIZE, -WALL_HALF_SIZE],
        [WALL_DISTANCE, WALL_HALF_SIZE, -WALL_HALF_SIZE],
        [WALL_DISTANCE, WALL_HALF_SIZE, WALL_HALF_SIZE],
        [WALL_DISTANCE, -WALL_HALF_SIZE, WALL_HALF_SIZE],
    ] {
        b.push_vertex(corner);
    }
    for corner in 0..4u16 {
        b.push_edge(corner, (corner + 1) % 4);
    }
    for step in 0..4 {
        b.push_surfedge(1 + step);
    }
    b.push_texinfo([0.0, 1.0, 0.0], 0.0, [0.0, 0.0, 1.0], 0.0, 0, 0);
    // Unlit/fullbright (`lightmap_offset < 0`), so this fixture needs no
    // lighting lump: the wall reads back as its own texture colour.
    b.push_face(0, 0, 0, 4, 0, [0xFF, 0xFF, 0xFF, 0xFF], -1);
    b.push_marksurface(0);
    let extent = WALL_HALF_SIZE as i16;
    let far = WALL_DISTANCE as i16 + extent;
    b.push_leaf(
        -1,
        -1,
        [0, -extent, -extent],
        [far, extent, extent],
        0,
        1,
        [0, 0, 0, 0],
    );
    b.push_model(
        [0.0, -WALL_HALF_SIZE, -WALL_HALF_SIZE],
        [
            WALL_DISTANCE + WALL_HALF_SIZE,
            WALL_HALF_SIZE,
            WALL_HALF_SIZE,
        ],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        1,
    );
    b.build()
}

/// Reproduces fidelity finding F1: a wall farther from the camera than
/// `sky.wgsl`'s fixed-size `HALF_EXTENT` cube used to be drawn *over* by the
/// sky pass regardless of how much nearer the wall's own, correctly
/// rendered and depth-written geometry was — not because the wall's
/// geometry failed to build or was culled (both were independently ruled
/// out for the real map this reproduces; see the fidelity-round-3 F1
/// follow-up), but because the sky cube's clip-space depth reflected its
/// arbitrary ~64-unit geometric size instead of always reading back as the
/// far plane. A wall further than that from the camera has a *larger*
/// (farther) depth value than the sky cube, so the sky pass's own
/// `LessEqual` depth test let it win and overpaint the wall.
#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn sky_pass_does_not_overpaint_a_distant_occluder() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_distant_occluder_test();
}

#[test]
fn sky_pass_does_not_overpaint_a_distant_occluder_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen distant-occluder render test");
        return;
    }
    run_distant_occluder_test();
}

fn run_distant_occluder_test() {
    let Some(context) = headless() else {
        return;
    };
    let bytes = distant_wall_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic wall map parses");
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .expect("synthetic wall map builds");
    assert!(!model.batches.is_empty(), "the wall is opaque geometry");

    // Six distinct, saturated colours, matching `run_sky_pass_test`; none is
    // anywhere near the wall's dark-grey fill, so any sky-coloured pixel in
    // the wall's frame-filling footprint is unambiguously the bug.
    let colors: [[u8; 3]; 6] = [
        [200, 0, 0],
        [0, 200, 0],
        [0, 0, 200],
        [200, 200, 0],
        [0, 200, 200],
        [200, 0, 200],
    ];
    let tgas: Vec<Vec<u8>> = colors.iter().map(|&c| solid_tga(c)).collect();
    let refs: [&[u8]; 6] = core::array::from_fn(|i| tgas[i].as_slice());
    let skybox = SkyboxAsset::build(refs).expect("six solid faces decode");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &model, OFFSCREEN_FORMAT).expect("renderer builds");
    let sky = SkyRenderer::new(&context, &skybox, OFFSCREEN_FORMAT).expect("sky renderer builds");
    let camera = FreeFlyCamera {
        position: [0.0, 0.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        ..FreeFlyCamera::default()
    };

    // The opaque pass draws (and depth-writes) the wall first, exactly as
    // `ohl-engine`'s frame order does; the sky pass then draws into the
    // same colour/depth target, "behind everything already written".
    renderer.render(&context, &model, &camera, target.view(), WIDTH, HEIGHT);
    let depth = renderer
        .depth_view()
        .cloned()
        .expect("world pass has depth");
    sky.render(&context, &camera, target.view(), &depth, WIDTH, HEIGHT);
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    let sky_colored = rgba
        .iter()
        .filter(|pixel| pixel[0] > 100 || pixel[1] > 100 || pixel[2] > 100)
        .count();
    assert_eq!(
        sky_colored, 0,
        "the sky pass must not draw over the wall's pixels: the wall is \
         nearer than the sky in every direction the camera can see, \
         regardless of the sky cube's own arbitrary geometric size"
    );
}
