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
    game_pixels(context, &mut game)
}

fn game_pixels(context: &GpuContext, game: &mut Game) -> Vec<u8> {
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

fn center(pixels: &[u8]) -> &[u8] {
    let offset = usize::try_from((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4).unwrap();
    &pixels[offset..offset + 4]
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

fn mixed_transparency() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("mixed visual capture skipped: no adapter");
        return;
    };
    let beam = "{\"classname\" \"info_target\" \"targetname\" \"ohl_a\" \"origin\" \"40 -10 40\"}\n{\"classname\" \"info_target\" \"targetname\" \"ohl_b\" \"origin\" \"40 10 40\"}\n{\"classname\" \"env_beam\" \"spawnflags\" \"1\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"BoltWidth\" \"3\" \"renderamt\" \"255\" \"rendercolor\" \"0 255 0\"}";
    let sprite = |x, amount| {
        format!(
            "{{\"classname\" \"env_sprite\" \"model\" \"sprites/ohl_effect.spr\" \"origin\" \"{x} 0 40\" \"scale\" \"2\" \"rendermode\" \"2\" \"renderamt\" \"{amount}\" \"rendercolor\" \"255 0 0\"}}"
        )
    };
    let beam_only = pixels(&context, beam, 0.0);
    let mut composites = Vec::new();
    for (label, x, amount) in [
        ("engine-near-opaque-sprite-rear-beam", 20, 255),
        ("engine-near-partial-sprite-rear-beam", 20, 128),
        ("engine-far-opaque-sprite-front-beam", 60, 255),
        ("engine-far-partial-sprite-front-beam", 60, 128),
    ] {
        let sprite = sprite(x, amount);
        let sprite_only = pixels(&context, &sprite, 0.0);
        let composite = pixels(&context, &format!("{sprite}\n{beam}"), 0.0);
        let green = center(&composite)[1];
        if x < 40 {
            if amount == 255 {
                assert_eq!(
                    center(&composite),
                    center(&sprite_only),
                    "opaque foreground sprite must cover a rear beam"
                );
            } else {
                assert!(
                    (120..=135).contains(&green),
                    "partial foreground sprite must attenuate rear green beam; green={green}"
                );
            }
        } else {
            assert_eq!(
                green,
                center(&beam_only)[1],
                "front beam must remain bright over a rear sprite"
            );
            assert_eq!(
                center(&composite)[0],
                center(&sprite_only)[0],
                "green beam must retain rear sprite red"
            );
        }
        capture(label, &composite);
        composites.push(composite);
    }
    assert!(center(&composites[1])[1] < center(&composites[3])[1]);
    capture_global_transparency_limit(&context);
}

// Deliberate diagnostic, not a compatibility assertion: translucent brushes
// still draw in their existing pass before this bounded sprite/effect sort.
// Capture the rear beam unattenuated through a foreground half-alpha brush.
fn capture_global_transparency_limit(context: &GpuContext) {
    let beam = "{\"classname\" \"info_target\" \"targetname\" \"ohl_a\" \"origin\" \"-10 40 40\"}\n{\"classname\" \"info_target\" \"targetname\" \"ohl_b\" \"origin\" \"10 40 40\"}\n{\"classname\" \"env_beam\" \"spawnflags\" \"1\" \"LightningStart\" \"ohl_a\" \"LightningEnd\" \"ohl_b\" \"BoltWidth\" \"3\" \"renderamt\" \"255\" \"rendercolor\" \"0 255 0\"}";
    let brush = "{\"classname\" \"func_wall\" \"model\" \"*1\" \"rendermode\" \"1\" \"renderamt\" \"128\" \"rendercolor\" \"255 0 0\"}";
    for (label, extra) in [
        ("engine-global-limit-brush", brush.to_owned()),
        ("engine-global-limit-rear-beam", beam.to_owned()),
        (
            "engine-global-limit-brush-rear-beam",
            format!("{brush}\n{beam}"),
        ),
    ] {
        let mut game = game(&extra, 0.0);
        game.set_viewpoint([0.0, -80.0, 40.0], 0.0, 90.0);
        capture(label, &game_pixels(context, &mut game));
    }
}

#[test]
#[ignore = "requires an adapter; opt in with OHL_RENDER_GPU_TEST=1"]
fn sprites_and_effects_composite_in_depth_order() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_none() {
        mixed_transparency();
    }
}

#[test]
fn sprites_and_effects_composite_in_depth_order_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_some() {
        mixed_transparency();
    }
}
