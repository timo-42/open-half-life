//! Engine render bridge checks using only project-authored BSP/SPR fixtures.

use ohl_engine::{
    Game, Input, MemoryAssets, RenderTarget,
    test_support::{SYNTHETIC_MAP, synthetic_map_bsp_with_entities},
};
use ohl_formats::test_support::build_minimal_spr;
use ohl_render::{GpuContext, OFFSCREEN_FORMAT, OffscreenTarget};

const WIDTH: u32 = 192;
const HEIGHT: u32 = 144;

fn game(extra: &str, seconds: f32) -> Game {
    let entities = format!(
        "{{\"classname\" \"worldspawn\"}}\n{{\"classname\" \"info_player_start\" \"origin\" \"0 0 32\"}}\n{extra}"
    );
    let map = synthetic_map_bsp_with_entities(&entities);
    let mut assets = MemoryAssets::new();
    let mut sprite = build_minimal_spr();
    // Bright, visibly different frames in the synthetic grayscale palette.
    let frame_start = 40 + 2 + 256 * 3;
    sprite[frame_start + 20..frame_start + 20 + 64].fill(240);
    sprite[frame_start + 20 + 64 + 20..].fill(80);
    assets.insert("sprites/ohl_effect.spr", sprite);
    let mut game = Game::from_map_bytes(&assets, SYNTHETIC_MAP, &map).unwrap();
    let mut remaining = seconds;
    while remaining > 0.0 {
        game.tick(1.0 / 60.0, &Input::default());
        remaining -= 1.0 / 60.0;
    }
    game.set_viewpoint([0.0, 0.0, 40.0], 0.0, 0.0);
    game
}

fn pixels(context: &GpuContext, extra: &str, seconds: f32) -> Vec<u8> {
    let mut game = game(extra, seconds);
    let target = OffscreenTarget::new(context, WIDTH, HEIGHT).unwrap();
    game.render(
        context,
        RenderTarget {
            view: target.view(),
            width: WIDTH,
            height: HEIGHT,
            format: OFFSCREEN_FORMAT,
        },
    )
    .unwrap();
    target.read_rgba(context).unwrap()
}

fn capture(label: &str, pixels: &[u8]) {
    let Some(directory) = std::env::var_os("OHL_RENDER_CAPTURE_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    ppm.extend(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| pixel[..3].iter().copied()),
    );
    std::fs::write(directory.join(format!("{label}.ppm")), ppm).unwrap();
}

const ENDPOINTS: &str = "{\"classname\" \"info_target\" \"targetname\" \"ohl_a\" \"origin\" \"35 -10 40\"}\n{\"classname\" \"info_target\" \"targetname\" \"ohl_b\" \"origin\" \"35 10 40\"}\n";

#[allow(clippy::too_many_lines)]
fn run() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("engine effect capture skipped: no adapter");
        return;
    };
    let baseline = pixels(&context, "", 0.0);
    capture("engine-baseline", &baseline);
    let beam = format!(
        "{ENDPOINTS}{{\"classname\" \"env_beam\" \"spawnflags\" \"1\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"BoltWidth\" \"3\" \"rendercolor\" \"0 255 32\"}}"
    );
    let laser = format!(
        "{ENDPOINTS}{{\"classname\" \"env_laser\" \"spawnflags\" \"1\" \"origin\" \"35 -10 40\" \"LaserTarget\" \"ohl_b\" \"width\" \"3\" \"rendercolor\" \"255 32 0\"}}"
    );
    let spark = "{\"classname\" \"env_spark\" \"spawnflags\" \"96\" \"origin\" \"35 0 40\" \"MaxDelay\" \"1\"}";
    for (label, extra, time) in [
        ("engine-beam", beam.as_str(), 0.0),
        ("engine-laser", laser.as_str(), 0.0),
        ("engine-sparks", spark, 0.1),
    ] {
        let rendered = pixels(&context, extra, time);
        assert_ne!(rendered, baseline, "{label} must pass through Game::render");
        capture(label, &rendered);
    }
    assert_eq!(
        pixels(
            &context,
            &beam.replace("\"spawnflags\" \"1\"", "\"spawnflags\" \"0\""),
            0.0
        ),
        baseline
    );
    assert_eq!(
        pixels(&context, spark, 0.5),
        baseline,
        "expired spark burst must leave no pixels"
    );

    let sprite = |class, tint: &str, rate: &str| {
        format!(
            "{{\"classname\" \"{class}\" \"model\" \"sprites/ohl_effect.spr\" \"origin\" \"35 0 40\" \"scale\" \"2\" \"rendermode\" \"5\" \"renderamt\" \"255\" \"rendercolor\" \"{tint}\" \"framerate\" \"{rate}\"}}"
        )
    };
    let red = sprite("env_sprite", "255 0 0", "2");
    let blue = sprite("env_sprite", "0 0 255", "2");
    let red_pixels = pixels(&context, &red, 0.0);
    let blue_pixels = pixels(&context, &blue, 0.0);
    assert_ne!(
        red_pixels, blue_pixels,
        "sprite rendercolor must affect RGB"
    );
    capture("engine-red-sprite", &red_pixels);
    capture("engine-blue-sprite", &blue_pixels);
    assert_eq!(
        red_pixels,
        pixels(&context, &red, 0.15),
        "declared low framerate must keep first frame"
    );
    assert_ne!(
        red_pixels,
        pixels(&context, &red, 0.55),
        "declared rate must eventually advance"
    );
    let color_mode = red.replace("\"rendermode\" \"5\"", "\"rendermode\" \"1\"");
    assert_eq!(
        pixels(&context, &color_mode, 0.0),
        pixels(&context, &color_mode, 0.55),
        "Color mode must replace palette RGB while preserving its alpha"
    );
    let glow = sprite("env_glow", "255 255 255", "10");
    assert_eq!(
        pixels(&context, &glow, 0.0),
        pixels(&context, &glow, 0.15),
        "env_glow must stay on its first frame"
    );
    assert_ne!(pixels(&context, &glow, 0.0), baseline);
    assert_eq!(
        pixels(
            &context,
            &red.replace("\"model\"", "\"targetname\" \"ohl_hidden\" \"model\""),
            0.0
        ),
        baseline,
        "named sprite without Start On must begin hidden"
    );
    let dim = red.replace("\"renderamt\" \"255\"", "\"renderamt\" \"32\"");
    let center = usize::try_from((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4).unwrap();
    assert!(
        red_pixels[center] > pixels(&context, &dim, 0.0)[center],
        "sprite brightness must modulate additive RGB"
    );
    capture("engine-glow", &pixels(&context, &glow, 0.15));
}

#[test]
#[ignore = "requires an adapter; opt in with OHL_RENDER_GPU_TEST=1"]
fn map_effects_and_sprite_properties_reach_the_frame() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_none() {
        run();
    }
}

#[test]
fn map_effects_and_sprite_properties_reach_the_frame_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_some() {
        run();
    }
}
