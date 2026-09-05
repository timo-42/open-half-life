//! Offscreen render test for an *opaque* brush-entity submodel: a
//! `func_train`-like box (model index 1) placed between the camera and a
//! darker worldspawn wall must occlude it.
//!
//! This covers the failure that made the first chapter's tram car invisible:
//! a brush entity with `rendermode` 0 and no `renderamt` key was taking
//! `renderamt`'s `0` default verbatim and drawing fully transparent. See
//! `ohl_render::RenderProps::from_entity`.
//!
//! Like the other headless tests here it is `#[ignore]`d by default and
//! skips itself, without failing, when no adapter exists. Run it with
//! `cargo test -p ohl-render -- --ignored`, or set `OHL_RENDER_GPU_TEST=1`.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_formats::test_support::Bsp30Builder;
use ohl_render::{
    FreeFlyCamera, GpuContext, OFFSCREEN_FORMAT, OffscreenTarget, RenderProps, SubmodelInstance,
    WorldRenderer,
};
use ohl_world::{BspLimits, WorldBuildOptions, WorldModel};

const WIDTH: u32 = 160;
const HEIGHT: u32 = 120;
const OPT_IN: &str = "OHL_RENDER_GPU_TEST";

/// The worldspawn wall's grey level, and the submodel car's; the fixture's
/// palette is a plain 0..255 grey ramp, so a fully lit surface renders at
/// about its own fill level.
const WALL_FILL: u8 = 40;
const CAR_FILL: u8 = 230;

fn headless() -> Option<GpuContext> {
    match GpuContext::headless() {
        Ok(context) => Some(context),
        Err(error) => {
            eprintln!("skipping offscreen render test: {error}");
            None
        }
    }
}

/// Submodel 0 (worldspawn) is a large, dark, fully lit floor quad at `z = 0`;
/// submodel 1 is a smaller, bright quad floating at `z = 64`, i.e. between
/// the overhead camera and the floor — the stand-in for a train car's brush
/// model.
fn wall_world_with_opaque_submodel_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"func_train\"\n\"model\" \"*1\"\n}\n",
    );
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_plane([0.0, 0.0, 1.0], 64.0, 2);
    b.push_edge(0, 0); // conventional unused slot

    let faces = [
        ("wall1", WALL_FILL, 128.0f32, 0.0f32),
        ("tram1", CAR_FILL, 48.0, 64.0),
    ];
    for (index, (name, fill, half, height)) in faces.into_iter().enumerate() {
        b.add_embedded_texture(name, 16, 16, fill);
        let base = u16::try_from(index * 4).unwrap();
        for corner in [
            [-half, -half, height],
            [half, -half, height],
            [half, half, height],
            [-half, half, height],
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
        // Fully lit: the ramp fixes 255 at 255, so each surface renders at
        // its own texture level and the two are trivially distinguishable.
        for _ in 0..400 {
            b.push_lighting_rgb(255, 255, 255);
        }
        b.push_face(
            u16::try_from(index).unwrap(),
            0,
            u32::try_from(index * 4).unwrap(),
            4,
            u16::try_from(index).unwrap(),
            [0, 0xFF, 0xFF, 0xFF],
            offset,
        );
        b.push_marksurface(u16::try_from(index).unwrap());
    }

    b.push_leaf(-1, -1, [-128, -128, 0], [128, 128, 128], 0, 2, [0, 0, 0, 0]);
    // Submodel 0 (worldspawn): the dark floor only.
    b.push_model(
        [-128.0, -128.0, 0.0],
        [128.0, 128.0, 8.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        1,
    );
    // Submodel 1 (the brush entity under test): the bright quad only.
    b.push_model(
        [-48.0, -48.0, 64.0],
        [48.0, 48.0, 72.0],
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
fn an_opaque_submodel_with_no_renderamt_key_still_occludes_the_world() {
    if std::env::var_os(OPT_IN).is_some() {
        return;
    }
    run_opaque_submodel_test();
}

#[test]
fn an_opaque_submodel_with_no_renderamt_key_still_occludes_the_world_when_opted_in() {
    if std::env::var_os(OPT_IN).is_none() {
        eprintln!("set {OPT_IN}=1 to run the offscreen opaque-submodel render test");
        return;
    }
    run_opaque_submodel_test();
}

fn run_opaque_submodel_test() {
    let Some(context) = headless() else {
        return;
    };
    let bytes = wall_world_with_opaque_submodel_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic map parses");
    let options = WorldBuildOptions {
        wads: &[],
        limits,
        ..WorldBuildOptions::default()
    };
    let world = WorldModel::build(&bsp, &options).expect("worldspawn builds");
    assert_eq!(
        WorldModel::submodel_count(&bsp, &limits).expect("model lump decodes"),
        2
    );
    let set = WorldModel::build_submodels(&bsp, &options, &[1]);
    assert_eq!(
        set.failure_count(),
        0,
        "the brush entity's submodel must build, not be dropped"
    );
    let (_index, submodel) = &set.models[0];

    // The two keyvalue combinations a mapper actually leaves behind on an
    // opaque brush entity: no `renderamt` key at all (parsed as 0) with
    // `rendermode` 0, and the same with `rendermode` 4 (`kRenderTransAlpha`).
    for props in [
        RenderProps::from_entity(0, 0, [255, 255, 255], 0),
        RenderProps::from_entity(4, 0, [255, 255, 255], 0),
    ] {
        assert!((props.alpha() - 1.0).abs() < f32::EPSILON);
        let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
        let mut renderer =
            WorldRenderer::new(&context, &world, OFFSCREEN_FORMAT).expect("renderer builds");
        let camera = overhead_camera();
        renderer.render(&context, &world, &camera, target.view(), WIDTH, HEIGHT);
        renderer.draw_world_submodel(
            &context,
            SubmodelInstance {
                model: submodel,
                transform: ohl_render::math::identity(),
            },
            props,
            &camera,
            target.view(),
            WIDTH,
            HEIGHT,
        );
        context.wait();

        let pixels = target.read_rgba(&context).expect("frame reads back");
        let (rgba, _) = pixels.as_chunks::<4>();
        let centre = rgba[(HEIGHT as usize / 2) * WIDTH as usize + WIDTH as usize / 2];
        assert!(
            u32::from(centre[0]) > u32::from(WALL_FILL) + 60,
            "the submodel must occlude the darker world wall, got {centre:?} for {:?}",
            props.mode
        );
        let bright = rgba.iter().filter(|pixel| pixel[0] > 128).count();
        assert!(
            bright * 50 > (WIDTH * HEIGHT) as usize,
            "at least 2% of the frame must exceed code value 128; got {bright} pixels"
        );
    }

    // Negative control: an entity that really is `rendermode 2` with
    // `renderamt 0` is fully transparent, so the assertion above has teeth.
    let target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("offscreen target");
    let mut renderer =
        WorldRenderer::new(&context, &world, OFFSCREEN_FORMAT).expect("renderer builds");
    let camera = overhead_camera();
    renderer.render(&context, &world, &camera, target.view(), WIDTH, HEIGHT);
    renderer.draw_world_submodel(
        &context,
        SubmodelInstance {
            model: submodel,
            transform: ohl_render::math::identity(),
        },
        RenderProps::from_entity(2, 0, [255, 255, 255], 0),
        &camera,
        target.view(),
        WIDTH,
        HEIGHT,
    );
    context.wait();
    let pixels = target.read_rgba(&context).expect("frame reads back");
    let (rgba, _) = pixels.as_chunks::<4>();
    let centre = rgba[(HEIGHT as usize / 2) * WIDTH as usize + WIDTH as usize / 2];
    assert!(
        u32::from(centre[0]) < u32::from(WALL_FILL) + 60,
        "a genuinely transparent entity must not occlude, got {centre:?}"
    );
}
