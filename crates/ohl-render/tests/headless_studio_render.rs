//! Renders the project-authored synthetic studio model offscreen and reads
//! the frame back.
//!
//! Like `headless_render.rs`, this is `#[ignore]`d by default and also skips
//! itself, without failing, when no adapter exists. Run it explicitly with
//! `cargo test -p ohl-render -- --ignored`, or opt in from the environment
//! with `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::mdl10::Limits;
use ohl_formats::test_support::build_minimal_mdl10;
use ohl_render::{
    FreeFlyCamera, GpuContext, ModelInstance, OFFSCREEN_FORMAT, OffscreenTarget, StudioRenderer,
    placement,
};
use ohl_world::{StudioModel, StudioPose};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

/// The environment variable that opts the non-ignored wrapper in.
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

fn render_frame() -> Option<Vec<u8>> {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping offscreen studio render test: {error}");
            return None;
        }
    };

    let (bytes, _) = build_minimal_mdl10();
    let model = StudioModel::parse(&bytes, &Limits::default()).expect("synthetic model builds");
    assert_eq!(model.meshes.len(), 1, "the synthetic model has one mesh");

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        StudioRenderer::new(&context, &model, OFFSCREEN_FORMAT).expect("renderer builds");

    // Half a frame into the sequence, so the pose exercises the
    // interpolation path as well as the skinning one. At that point bone 0's
    // X channel sits halfway between the fixture's two animated values, so
    // the quad (a unit square in the model's XY plane) is centred near
    // x = 15.5, y = 0.5, z = 0.
    let pose = StudioPose::sample(&model, 0, 0.05).expect("pose samples");
    let instance = ModelInstance {
        transform: placement([0.0, 0.0, 0.0], 0.0),
        pose: &pose,
        body: &[],
        skin: 0,
        ambient: [1.0, 1.0, 1.0],
        light_direction: ModelInstance::default_light_direction(),
        light_color: [0.6, 0.6, 0.6],
    };

    // Look straight down at the quad from two units up.
    let camera = FreeFlyCamera {
        position: [15.5, 0.5, 2.0],
        yaw: 0.0,
        pitch: 89.0,
        fov_y_degrees: 60.0,
        near: 0.1,
        ..FreeFlyCamera::default()
    };

    renderer.render(
        &context,
        &model,
        &camera,
        std::slice::from_ref(&instance),
        target.view(),
        target.width(),
        target.height(),
        None,
    );
    context.wait();
    assert_eq!(
        renderer.last_triangle_count(),
        2,
        "the synthetic quad is two triangles"
    );

    Some(target.read_rgba(&context).expect("frame reads back"))
}

/// Asserts that a meaningful part of the frame is *not* the background.
///
/// The fixture's texture is a dark grey from the project's synthetic
/// grayscale palette, so an absolute brightness threshold would be the wrong
/// test here; what matters is that the model's pixels differ from the
/// cleared background at all. The background is whatever colour covers most
/// of the frame.
fn check(pixels: &[u8]) {
    assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    let (rgba, _) = pixels.as_chunks::<4>();

    let mut counts: Vec<([u8; 4], usize)> = Vec::new();
    for pixel in rgba {
        match counts.iter_mut().find(|(value, _)| value == pixel) {
            Some((_, count)) => *count += 1,
            None => counts.push((*pixel, 1)),
        }
    }
    let background = counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(value, _)| *value)
        .expect("the frame has pixels");
    let foreground = rgba.iter().filter(|pixel| **pixel != background).count();
    assert!(
        foreground > (WIDTH * HEIGHT / 100) as usize,
        "expected the model to cover a meaningful part of the frame, got {foreground} non-background pixels"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target is opaque"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn renders_the_synthetic_model_offscreen() {
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}

#[test]
fn renders_the_synthetic_model_offscreen_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen studio render test");
        return;
    }
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}
