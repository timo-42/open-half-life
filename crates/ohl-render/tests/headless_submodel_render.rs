//! Offscreen render test for [`WorldRenderer::draw_world_submodel`]: a
//! brush-entity submodel drawn translucently (`RenderMode::Texture`) over
//! an otherwise empty (liquid-only) worldspawn pass.
//!
//! Like `headless_sky_water_render.rs`, this is `#[ignore]`d by default and
//! also skips itself, without failing, when no adapter exists. Run it
//! explicitly with `cargo test -p ohl-render -- --ignored`, or opt in from
//! the environment with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_formats::test_support::Bsp30Builder;
use ohl_render::{
    FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, RenderMode, RenderProps,
    SubmodelInstance, WorldRenderer,
};
use ohl_world::{BspLimits, WorldBuildOptions, WorldModel};

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

/// Two quads sharing one leaf: submodel 0 (worldspawn) is a liquid
/// (`!water1`) quad, so its own opaque pass draws nothing (matching
/// `headless_sky_water_render.rs`'s liquid fixture); submodel 1 is an
/// ordinary opaque (`brick1`) quad at the same location, meant to be drawn
/// with [`ohl_render::WorldRenderer::draw_world_submodel`] instead of as
/// part of worldspawn.
fn liquid_world_with_brick_submodel_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_edge(0, 0); // conventional unused slot

    let names = ["!water1", "brick1"];
    for (index, name) in names.iter().enumerate() {
        b.add_embedded_texture(name, 16, 16, 220);
        let base = u16::try_from(index * 4).unwrap();
        for corner in [
            [-64.0, -64.0, 0.0],
            [64.0, -64.0, 0.0],
            [64.0, 64.0, 0.0],
            [-64.0, 64.0, 0.0],
        ] {
            b.push_vertex(corner);
        }
        for corner in 0..4u16 {
            let next = (corner + 1) % 4;
            b.push_edge(base + corner, base + next);
        }
        let first_edge = i32::try_from(index * 4 + 1).unwrap();
        for step in 0..4 {
            b.push_surfedge(first_edge + step);
        }
        b.push_texinfo(
            [1.0, 0.0, 0.0],
            0.0,
            [0.0, 1.0, 0.0],
            0.0,
            u32::try_from(index).unwrap(),
            0,
        );
        let offset = i32::try_from(b.lighting.len()).unwrap();
        for _ in 0..25 {
            b.push_lighting_rgb(255, 255, 255);
        }
        b.push_face(
            0,
            0,
            u32::try_from(index * 4).unwrap(),
            4,
            u16::try_from(index).unwrap(),
            [0, 0xFF, 0xFF, 0xFF],
            offset,
        );
        b.push_marksurface(u16::try_from(index).unwrap());
    }

    b.push_leaf(-1, -1, [-64, -64, 0], [64, 64, 16], 0, 2, [0, 0, 0, 0]);
    // Submodel 0 (worldspawn): only the liquid face.
    b.push_model(
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 16.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        1,
    );
    // Submodel 1 (the brush entity under test): only the opaque brick face.
    b.push_model(
        [-64.0, -64.0, 0.0],
        [64.0, 64.0, 16.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        1,
        1,
    );
    b.build()
}

fn overhead_camera() -> FreeFlyCamera {
    FreeFlyCamera {
        position: [0.0, 0.0, 200.0],
        yaw: 0.0,
        pitch: 89.0,
        ..FreeFlyCamera::default()
    }
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn translucent_submodel_blends_over_the_cleared_background() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_submodel_test();
}

#[test]
fn translucent_submodel_blends_over_the_cleared_background_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen submodel render test");
        return;
    }
    run_submodel_test();
}

fn run_submodel_test() {
    let Some(context) = headless() else {
        return;
    };
    let bytes = liquid_world_with_brick_submodel_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic map parses");
    let world = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .expect("worldspawn (liquid-only) builds");
    assert!(
        world.batches.is_empty(),
        "worldspawn has no opaque geometry, only a liquid face"
    );
    let submodel = WorldModel::build_submodel(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
        1,
    )
    .expect("brick submodel builds");
    assert!(
        !submodel.batches.is_empty(),
        "the submodel's brick face is ordinary opaque geometry"
    );

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &world, OFFSCREEN_FORMAT).expect("renderer builds");
    let camera = overhead_camera();

    // The opaque pass draws nothing (worldspawn has no opaque batches) but
    // still clears colour and depth, establishing the background the
    // translucent submodel blends over.
    renderer.render(&context, &world, &camera, target.view(), WIDTH, HEIGHT);
    renderer.draw_world_submodel(
        &context,
        SubmodelInstance {
            model: &submodel,
            transform: ohl_render::math::identity(),
        },
        RenderProps {
            mode: RenderMode::Texture,
            amount: 128,
            ..RenderProps::default()
        },
        &camera,
        target.view(),
        WIDTH,
        HEIGHT,
    );
    context.wait();

    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    // The clear colour is a near-black blue (red channel ~5/255); a ~50%
    // blend of a bright, fully-lit texture over it must land clearly above
    // the background but below the texture's own near-white brightness.
    let blended = rgba
        .iter()
        .filter(|pixel| pixel[0] > 20 && pixel[0] < 230)
        .count();
    assert!(
        blended > (WIDTH * HEIGHT) as usize / 4,
        "expected a meaningful part of the frame to show blended submodel pixels"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target itself stays opaque"
    );
}
