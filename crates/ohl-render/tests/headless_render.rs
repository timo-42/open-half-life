//! Renders the project-authored synthetic room offscreen and reads the
//! frame back.
//!
//! CI runners generally have no GPU, so this test is `#[ignore]`d by default
//! and additionally skips itself, without failing, when no adapter exists.
//! Run it explicitly with `cargo test -p ohl-render -- --ignored`, or opt in
//! from the environment with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_render::{FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, WorldRenderer};
use ohl_world::test_support::{synthetic_room_bsp, synthetic_room_wad};
use ohl_world::{BspLimits, WorldBuildOptions, WorldModel};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

/// The environment variable that opts the non-ignored wrapper in.
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

fn render_frame() -> Option<Vec<u8>> {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            // Expected on a machine with no GPU; not a test failure.
            eprintln!("skipping offscreen render test: {error}");
            return None;
        }
    };

    let bytes = synthetic_room_bsp();
    let wad = synthetic_room_wad();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic room parses");
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[&wad],
            limits,
        },
    )
    .expect("synthetic room builds");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &model, OFFSCREEN_FORMAT).expect("renderer builds");

    // Stand at the map's player start, looking across the room.
    let camera = FreeFlyCamera::at_spawn(model.spawn.expect("the room has a player start"));
    renderer.render(
        &context,
        &model,
        &camera,
        target.view(),
        target.width(),
        target.height(),
    );
    context.wait();
    assert!(
        renderer.last_triangle_count() > 0,
        "the camera stands inside the room, so something must survive culling"
    );

    Some(target.read_rgba(&context).expect("frame reads back"))
}

fn check(pixels: &[u8]) {
    assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    // The clear colour is a near-black blue; a rendered room must produce
    // pixels that are brighter than it somewhere.
    let (rgba, _) = pixels.as_chunks::<4>();
    let lit = rgba
        .iter()
        .filter(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]) > 60)
        .count();
    assert!(
        lit > (WIDTH * HEIGHT / 20) as usize,
        "expected the lit room to cover a meaningful part of the frame"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target is opaque"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn renders_the_synthetic_room_offscreen() {
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}

#[test]
fn renders_the_synthetic_room_offscreen_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen render test");
        return;
    }
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}
