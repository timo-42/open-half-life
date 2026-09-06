//! A rendered frame with a view model and a transient sprite must differ
//! from the same frame without them.
//!
//! Like `tests/offscreen_render.rs` this is `#[ignore]`d by default and
//! skips itself, without failing, when no adapter exists. Run it with
//! `cargo test -p ohl-engine -- --ignored`, or opt in from the environment
//! with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_engine::test_support::{SYNTHETIC_MAP, synthetic_map_bsp_with_extra_entity};
use ohl_engine::{Game, Input, MemoryAssets, RenderTarget};
use ohl_formats::test_support::{build_minimal_mdl10, build_minimal_spr};
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};

const WIDTH: u32 = 128;
const HEIGHT: u32 = 96;
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn a_view_model_and_a_transient_sprite_change_the_frame() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run();
}

#[test]
fn a_view_model_and_a_transient_sprite_change_the_frame_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the viewmodel/sprite render test");
        return;
    }
    run();
}

fn build_game() -> Game {
    // One `monster_generic` prop (a loaded studio model at slot 0, for the
    // view model to reuse — see `crate::viewmodel`'s module doc) and one
    // `env_sprite` (a loaded sprite asset at slot 0, for the transient
    // sprite to reuse — see `crate::sprites`'s module doc).
    let extra = "{\n\"classname\" \"monster_generic\"\n\
         \"model\" \"models/ohl_prop.mdl\"\n\
         \"origin\" \"0 0 40\"\n}\n\
         {\n\"classname\" \"env_sprite\"\n\
         \"model\" \"sprites/ohl_glow.spr\"\n\
         \"origin\" \"0 0 40\"\n}\n";
    let map = synthetic_map_bsp_with_extra_entity(SYNTHETIC_MAP, extra);

    let mut assets = MemoryAssets::new();
    assets.insert(&format!("maps/{SYNTHETIC_MAP}.bsp"), map.clone());
    let (mdl_bytes, _layout) = build_minimal_mdl10();
    assets.insert("models/ohl_prop.mdl", mdl_bytes);
    assets.insert("sprites/ohl_glow.spr", build_minimal_spr());

    let mut game = Game::from_map_bytes(&assets, SYNTHETIC_MAP, &map).expect("the map loads");
    game.set_viewpoint([0.0, 0.0, 40.0], 0.0, 0.0);
    game
}

fn render_frame(game: &mut Game, context: &GpuContext) -> Vec<u8> {
    let target = OffscreenTarget::new(context, WIDTH, HEIGHT).expect("offscreen target");
    game.tick(1.0 / 60.0, &Input::default());
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
    context.wait();
    target.read_rgba(context).expect("frame reads back")
}

fn run() {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping viewmodel/sprite render test: {error}");
            return;
        }
    };

    let mut baseline = build_game();
    let baseline_pixels = render_frame(&mut baseline, &context);

    let mut with_extras = build_game();
    with_extras.debug_show_viewmodel_and_sprite(0);
    let extras_pixels = render_frame(&mut with_extras, &context);

    assert_eq!(baseline_pixels.len(), extras_pixels.len());
    assert_ne!(
        baseline_pixels, extras_pixels,
        "a frame with a view model and a transient sprite must differ from one without them"
    );
}
