//! Offscreen render of a whole [`Game`] frame over the project's synthetic
//! map, exercising the composed pass order (world, studio, sky, brush
//! submodels, liquids) end to end.
//!
//! Like `ohl-render`'s own headless tests this is `#[ignore]`d by default
//! and skips itself, without failing, when no adapter exists. Run it with
//! `cargo test -p ohl-engine -- --ignored`, or opt in from the environment
//! with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{SYNTHETIC_MAP, synthetic_map_bsp};
use ohl_engine::{Game, Input, MemoryAssets, RenderTarget};
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};

const WIDTH: u32 = 192;
const HEIGHT: u32 = 144;
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn a_rendered_frame_is_not_empty() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run();
}

#[test]
fn a_rendered_frame_is_not_empty_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen render test");
        return;
    }
    run();
}

fn run() {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping offscreen render test: {error}");
            return;
        }
    };

    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), synthetic_map_bsp());
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("the synthetic map loads");
    // Look down on the lit floor from inside the room.
    game.set_viewpoint([0.0, 0.0, 150.0], 70.0, 0.0);

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    for _ in 0..3 {
        game.tick(1.0 / 60.0, &Input::default());
        game.render(
            &context,
            RenderTarget {
                view: target.view(),
                width: WIDTH,
                height: HEIGHT,
                format: OFFSCREEN_FORMAT,
            },
        )
        .expect("the frame renders");
    }
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    assert_eq!(rgba.len(), (WIDTH * HEIGHT) as usize);
    // The clear colour is a near-black blue; a lit floor must put a
    // meaningful number of clearly brighter pixels on screen.
    let lit = rgba.iter().filter(|pixel| pixel[0] > 40).count();
    assert!(
        lit > rgba.len() / 20,
        "expected the lit world to cover part of the frame, saw {lit} pixels"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target stays opaque"
    );
}
