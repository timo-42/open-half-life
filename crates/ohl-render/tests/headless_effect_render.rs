//! Actual GPU captures of project-authored geometry; no payload assets.

use ohl_formats::bsp30::Bsp;
use ohl_render::{
    EffectInstance, EffectRenderer, FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget,
    WorldRenderer, math,
};
use ohl_world::{
    BspLimits, WorldBuildOptions, WorldModel,
    test_support::{synthetic_room_bsp, synthetic_room_wad},
};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;

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

#[allow(clippy::too_many_lines)]
fn run() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("effect capture skipped: no adapter");
        return;
    };
    let bytes = synthetic_room_bsp();
    let wad = synthetic_room_wad();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).unwrap();
    let model = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[&wad],
            limits,
            ..Default::default()
        },
    )
    .unwrap();
    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).unwrap();
    let mut world = WorldRenderer::new(&context, &model, OFFSCREEN_FORMAT).unwrap();
    let renderer = EffectRenderer::new(&context, OFFSCREEN_FORMAT);
    let camera = FreeFlyCamera::default();
    let mut frame = |instances: &[EffectInstance]| {
        world.render(&context, &model, &camera, target.view(), WIDTH, HEIGHT);
        renderer.draw(
            &context,
            instances,
            &camera,
            target.view(),
            world.depth_view().unwrap(),
            WIDTH,
            HEIGHT,
        );
        target.read_rgba(&context).unwrap()
    };
    let baseline = frame(&[]);
    let beam = |brightness| EffectInstance::Beam {
        start: [30.0, -8.0, 64.0],
        end: [30.0, 8.0, 64.0],
        width: 3.0,
        color: [0.0, 1.0, 0.3, brightness],
    };
    let particle = EffectInstance::Particle {
        origin: [30.0, 0.0, 64.0],
        size: 5.0,
        color: [1.0, 0.3, 0.0, 1.0],
    };
    let bullet = EffectInstance::Decal {
        origin: [30.0, 0.0, 64.0],
        normal: [-1.0, 0.0, 0.0],
        radius: 3.0,
        color: [0.05, 0.03, 0.01, 0.9],
    };
    let blood = EffectInstance::Decal {
        origin: [30.0, 0.0, 64.0],
        normal: [-1.0, 0.0, 0.0],
        radius: 3.0,
        color: [0.5, 0.0, 0.0, 0.8],
    };
    let mut transform = math::identity();
    transform[12] = 30.0;
    transform[14] = 64.0;
    let gib = EffectInstance::Gib {
        transform,
        half_extents: [3.0, 4.0, 5.0],
        color: [0.4, 0.15, 0.05, 1.0],
    };
    capture("primitive-baseline", &baseline);
    for (label, instance) in [
        ("primitive-beam", beam(1.0)),
        ("primitive-particle", particle),
        ("primitive-bullet-mark", bullet),
        ("primitive-blood-mark", blood),
        ("primitive-gib", gib),
    ] {
        let pixels = frame(&[instance]);
        assert_ne!(pixels, baseline, "{label} must change actual pixels");
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
        capture(label, &pixels);
    }
    let mut debris_transform = math::identity();
    debris_transform[12] = 30.0;
    debris_transform[13] = 10.0;
    debris_transform[14] = 63.0;
    capture(
        "primitive-combined",
        &frame(&[
            EffectInstance::Beam {
                start: [30.0, -12.0, 72.0],
                end: [30.0, 12.0, 72.0],
                width: 1.5,
                color: [0.0, 1.0, 0.3, 1.0],
            },
            EffectInstance::Particle {
                origin: [25.0, -9.0, 64.0],
                size: 3.0,
                color: [1.0, 0.3, 0.0, 1.0],
            },
            EffectInstance::Decal {
                origin: [30.0, -4.0, 60.0],
                normal: [-1.0, 0.0, 0.0],
                radius: 2.0,
                color: [0.05, 0.03, 0.01, 0.9],
            },
            EffectInstance::Decal {
                origin: [30.0, 4.0, 60.0],
                normal: [-1.0, 0.0, 0.0],
                radius: 2.5,
                color: [0.5, 0.0, 0.0, 0.8],
            },
            EffectInstance::Gib {
                transform: debris_transform,
                half_extents: [3.0, 3.0, 4.0],
                color: [0.4, 0.15, 0.05, 1.0],
            },
        ]),
    );
    let center =
        |pixels: &[u8]| pixels[usize::try_from((HEIGHT / 2 * WIDTH + WIDTH / 2) * 4 + 1).unwrap()];
    assert!(
        center(&frame(&[beam(1.0)])) > center(&frame(&[beam(0.1)])),
        "additive brightness must modulate RGB"
    );
    let occluded = EffectInstance::Beam {
        start: [512.0, -8.0, 64.0],
        end: [512.0, 8.0, 64.0],
        width: 3.0,
        color: [1.0; 4],
    };
    assert_eq!(
        frame(&[occluded]),
        baseline,
        "world depth must occlude effects"
    );
    let behind_gib = EffectInstance::Particle {
        origin: [40.0, 0.0, 64.0],
        size: 5.0,
        color: [0.0, 0.0, 1.0, 1.0],
    };
    assert_eq!(
        frame(&[gib, behind_gib]),
        frame(&[gib]),
        "opaque gib geometry must write depth"
    );
}

#[test]
#[ignore = "requires an adapter; opt in with OHL_RENDER_GPU_TEST=1"]
fn effect_primitives_render_and_obey_depth() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_none() {
        run();
    }
}

#[test]
fn effect_primitives_render_and_obey_depth_when_opted_in() {
    if std::env::var_os("OHL_RENDER_GPU_TEST").is_some() {
        run();
    }
}
