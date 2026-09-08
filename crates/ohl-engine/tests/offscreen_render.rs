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

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn brush_resources_survive_frames_and_reset_on_level_change_and_quickload() {
    if std::env::var_os(OPT_IN).is_none() {
        run_resource_lifecycle();
    }
}

#[test]
fn brush_resource_lifecycle_when_opted_in() {
    if std::env::var_os(OPT_IN).is_some() {
        run_resource_lifecycle();
    }
}

fn render_pixels(game: &mut Game, context: &GpuContext, target: &OffscreenTarget) -> Vec<u8> {
    game.render(
        context,
        RenderTarget {
            view: target.view(),
            width: WIDTH,
            height: HEIGHT,
            format: OFFSCREEN_FORMAT,
        },
    )
    .expect("the frame renders");
    target.read_rgba(context).expect("frame reads back")
}

fn run_resource_lifecycle() {
    use ohl_engine::test_support::{LANDMARK, NEXT_MAP, synthetic_map_bsp_with_entities};

    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping offscreen resource lifecycle test: {error}");
            return;
        }
    };
    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), synthetic_map_bsp());
    // The destination retains the same room and BSP model indices but does
    // not instantiate any brush entity. It must not retain the first map's
    // prepared models or draw its door.
    let destination = synthetic_map_bsp_with_entities(&format!(
        "{{\n\"classname\" \"worldspawn\"\n}}\n\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 32\"\n}}\n\
         {{\n\"classname\" \"info_landmark\"\n\"targetname\" \"{LANDMARK}\"\n\
         \"origin\" \"16 0 0\"\n}}\n"
    ));
    assets.insert(&format!("maps/{NEXT_MAP}.bsp"), destination);
    let mut game = Game::load(&assets, SYNTHETIC_MAP).expect("first map loads");
    game.set_viewpoint([0.0, -150.0, 50.0], 0.0, 90.0);
    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("target builds");
    assert_eq!(
        game.render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
    let reference = render_pixels(&mut game, &context, &target);
    let prepared_stats = game.render_resource_stats();
    assert!(prepared_stats.submodels.preparations > 0);
    assert!(prepared_stats.submodels.static_upload_bytes > 0);
    assert_eq!(
        prepared_stats.submodels.texture_uploads, prepared_stats.submodels.preparations,
        "each brush only uploads its lightmap; diffuse textures are shared"
    );
    for _ in 0..3 {
        assert_eq!(render_pixels(&mut game, &context, &target), reference);
        assert_eq!(game.render_resource_stats(), prepared_stats);
    }
    let saved = game
        .save_bytes(1_700_000_000)
        .expect("quicksave serializes");
    game.change_level(&assets, NEXT_MAP, LANDMARK)
        .expect("actual level transition succeeds");
    assert_eq!(
        game.render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
    game.set_viewpoint([0.0, -150.0, 50.0], 0.0, 90.0);
    let destination_pixels = render_pixels(&mut game, &context, &target);
    assert_eq!(game.render_resource_stats().submodels.preparations, 0);
    assert_ne!(destination_pixels, reference, "the old door must disappear");

    // Quickload replaces the Game just as the host does, restoring the saved
    // map but rebuilding that map's GPU resources on its next draw. The
    // debug viewpoint puts the eye directly at the controller origin; saves
    // restore a normal player eye offset and do not retain debug noclip.
    // Reapply the capture pose so this comparison isolates GPU resource reuse.
    game = Game::load_bytes(&assets, &saved).expect("quicksave reloads");
    assert_eq!(game.map(), SYNTHETIC_MAP);
    game.set_viewpoint([0.0, -150.0, 50.0], 0.0, 90.0);
    assert_eq!(
        game.render_resource_stats(),
        ohl_engine::RenderResourceStats::default()
    );
    assert_eq!(render_pixels(&mut game, &context, &target), reference);
    assert_eq!(game.render_resource_stats(), prepared_stats);
    assert_eq!(render_pixels(&mut game, &context, &target), reference);
    assert_eq!(game.render_resource_stats(), prepared_stats);
}
