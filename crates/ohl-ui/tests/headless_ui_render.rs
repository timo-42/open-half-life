//! Renders one frame of the HUD and an open developer console into an
//! offscreen wgpu texture and asserts the result is not just the cleared
//! background.
//!
//! CI runners generally have no GPU, so this test is `#[ignore]`d by default
//! and additionally skips itself, without failing, when no adapter exists.
//! Run it explicitly with `cargo test -p ohl-ui -- --ignored`, or opt in from
//! the environment with `OHL_RENDER_GPU_TEST=1`, matching `ohl-render`'s own
//! offscreen render tests.

use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};
use ohl_ui::console::Console;
use ohl_ui::hud::{self, HudState};
use ohl_ui::{UiLayer, root_ui};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

/// The environment variable that opts the non-ignored wrapper in.
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// Clears `target` to a fixed, near-black colour so the test can tell
/// rendered UI pixels apart from an untouched texture.
fn clear(context: &GpuContext, target: &OffscreenTarget) {
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ohl-ui test clear"),
        });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ohl-ui test clear pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.01,
                        g: 0.01,
                        b: 0.02,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    context.queue.submit(Some(encoder.finish()));
}

fn render_frame() -> Option<Vec<u8>> {
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("skipping offscreen UI render test: {error}");
            return None;
        }
    };

    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    clear(&context, &target);

    let mut layer = UiLayer::new_headless(&context.device, OFFSCREEN_FORMAT);

    let mut console = Console::new();
    console.set_open(true);
    console.submit_line("echo hud smoke test");

    let mut hud_state = HudState {
        health: 55,
        armor: 25,
        clip_ammo: Some(12),
        reserve_ammo: Some(90),
        ..HudState::default()
    };
    hud_state.trigger_damage_flash();

    layer.begin_frame_headless([WIDTH, HEIGHT], 1.0);
    {
        let ctx = layer.context().clone();
        let mut ui = root_ui(&ctx);
        ohl_ui::console::draw_console(&mut ui, &mut console);
        hud::draw(&ctx, &hud_state);
    }

    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ohl-ui test frame"),
        });
    layer.end_frame_and_render(
        &context.device,
        &context.queue,
        &mut encoder,
        target.view(),
        [WIDTH, HEIGHT],
    );
    context.queue.submit(Some(encoder.finish()));
    context.wait();

    Some(target.read_rgba(&context).expect("frame reads back"))
}

fn check(pixels: &[u8]) {
    assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    let (rgba, _) = pixels.as_chunks::<4>();
    // The console panel, its text, and the HUD numerals/crosshair/damage
    // flash must all paint pixels well away from the near-black clear
    // colour somewhere in the frame.
    let non_background = rgba
        .iter()
        .filter(|pixel| u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]) > 30)
        .count();
    assert!(
        non_background > (WIDTH * HEIGHT / 50) as usize,
        "expected the HUD and console to paint a meaningful part of the frame, got {non_background} non-background pixels"
    );
    assert!(
        rgba.iter().all(|pixel| pixel[3] == 255),
        "the colour target is opaque"
    );
}

#[test]
#[ignore = "requires a graphics adapter; run with --ignored or set OHL_RENDER_GPU_TEST=1"]
fn renders_hud_and_console_offscreen() {
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}

#[test]
fn renders_hud_and_console_offscreen_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen UI render test");
        return;
    }
    if let Some(pixels) = render_frame() {
        check(&pixels);
    }
}
