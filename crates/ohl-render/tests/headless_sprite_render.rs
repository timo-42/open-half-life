//! Offscreen render test for the sprite billboard pass.
//!
//! Like the other `headless_*` tests, this is `#[ignore]`d by default and
//! also skips itself, without failing, when no adapter exists. Run it
//! explicitly with `cargo test -p ohl-render -- --ignored`, or opt in from
//! the environment with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_formats::palette::PALETTE_LEN;
use ohl_formats::spr::Limits as SprLimits;
use ohl_formats::test_support::{Bsp30Builder, synthetic_palette};
use ohl_render::{
    FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, RenderMode, RenderProps,
    SpriteInstance, WorldRenderer,
};
use ohl_world::{BspLimits, SpriteAsset, WorldBuildOptions, WorldModel};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

fn headless() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipping offscreen sprite render test: {error}");
            None
        }
    }
}

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// A minimal single-frame SPR file: a `SPR_VP_PARALLEL` (fully
/// camera-aligned) sprite in the documented `SPR_ADDITIVE` texture format,
/// its one frame filled with palette index `fill` (the project's synthetic
/// grayscale ramp palette, so `fill = 255` decodes to opaque white).
fn additive_sprite_bytes(fill: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"IDSP");
    push_i32(&mut out, 2); // version
    push_i32(&mut out, 2); // type: SPR_VP_PARALLEL
    push_i32(&mut out, 1); // texture_format: SPR_ADDITIVE
    push_f32(&mut out, 5.656_854); // bounding_radius
    push_u32(&mut out, 8); // max_width
    push_u32(&mut out, 8); // max_height
    push_u32(&mut out, 1); // num_frames
    push_f32(&mut out, 0.0); // beam_length
    push_i32(&mut out, 0); // sync_type: synchronized

    let palette = synthetic_palette();
    push_u16(&mut out, u16::try_from(PALETTE_LEN).unwrap());
    for entry in &palette {
        out.push(entry.r);
        out.push(entry.g);
        out.push(entry.b);
    }

    push_u32(&mut out, 0); // group
    push_i32(&mut out, -4); // origin_x
    push_i32(&mut out, -4); // origin_y
    push_u32(&mut out, 8); // width
    push_u32(&mut out, 8); // height
    out.extend(core::iter::repeat_n(fill, 64));
    out
}

/// A world with one liquid quad far below the camera's line of sight: gives
/// `WorldRenderer::new` the non-empty vertex buffer it requires while
/// leaving `model.batches` empty, so `WorldRenderer::render` only clears the
/// frame (nothing to occlude or shade the sprite pass with).
fn backdrop_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_edge(0, 0);
    b.add_embedded_texture("!water1", 16, 16, 128);
    for corner in [
        [-64.0, -64.0, -512.0],
        [64.0, -64.0, -512.0],
        [64.0, 64.0, -512.0],
        [-64.0, 64.0, -512.0],
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
    b.push_leaf(-1, -1, [-64, -64, -512], [64, 64, -496], 0, 1, [0, 0, 0, 0]);
    b.push_model(
        [-64.0, -64.0, -512.0],
        [64.0, 64.0, -496.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        1,
    );
    b.build()
}

fn run_additive_sprite_test() {
    let Some(context) = headless() else {
        return;
    };

    let bytes = backdrop_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic backdrop parses");
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..Default::default()
        },
    )
    .expect("synthetic backdrop builds");
    assert!(model.batches.is_empty(), "there is no opaque geometry");

    let sprite_bytes = additive_sprite_bytes(255);
    let asset = SpriteAsset::build(&sprite_bytes, &SprLimits::default())
        .expect("synthetic additive sprite decodes");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &model, OFFSCREEN_FORMAT).expect("renderer builds");
    let camera = FreeFlyCamera::default();

    // The opaque pass draws nothing (no opaque batches) but still clears
    // colour and depth, establishing the background the sprite pass draws
    // over, and the depth buffer the sprite pass tests against.
    renderer.render(&context, &model, &camera, target.view(), WIDTH, HEIGHT);

    let instance = SpriteInstance {
        asset: &asset,
        // Directly ahead of the default camera (which looks along +X from
        // the origin), well inside the near/far planes.
        origin: [50.0, 0.0, 64.0],
        scale: 1.25,
        render_props: RenderProps {
            mode: RenderMode::Additive,
            amount: 255,
            ..RenderProps::default()
        },
        frame_time: 0.0,
    };
    renderer.draw_sprites(
        &context,
        std::slice::from_ref(&instance),
        &camera,
        target.view(),
        WIDTH,
        HEIGHT,
    );
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    let background = rgba[0];
    let center = rgba[(HEIGHT / 2 * WIDTH + WIDTH / 2) as usize];
    assert!(
        u16::from(center[0]) > u16::from(background[0]) + 60,
        "expected the additive sprite to brighten the centre of the frame: \
         background {background:?}, centre {center:?}"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target itself stays opaque"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn additive_sprite_brightens_the_centre_of_the_frame() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_additive_sprite_test();
}

#[test]
fn additive_sprite_brightens_the_centre_of_the_frame_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen sprite render test");
        return;
    }
    run_additive_sprite_test();
}
