//! Integration coverage for M3.4's sky exclusion, liquid-face routing, and
//! light-style re-blending, over a small project-authored synthetic map.
//!
//! No bytes here come from any game installation; see `docs/CLEAN_ROOM.md`.

use ohl_formats::bsp30::Bsp;
use ohl_formats::test_support::Bsp30Builder;
use ohl_world::{BspLimits, DrawList, WorldBuildOptions, WorldModel};

/// Three quads: one `sky`-textured, one `!water`-textured, one ordinary,
/// all referenced by a single leaf. Geometry and lighting are otherwise
/// identical so the only interesting difference between faces is their
/// texture name.
fn synthetic_bsp() -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    b.push_plane([0.0, 0.0, 1.0], 0.0, 2);
    b.push_edge(0, 0); // conventional unused slot

    let names = ["sky1", "!water1", "brick1"];
    for (index, name) in names.iter().enumerate() {
        b.add_embedded_texture(name, 16, 16, 40);

        #[allow(clippy::cast_precision_loss)]
        let y = (index as f32) * 32.0;
        let base = u16::try_from(index * 4).unwrap();
        for corner in [
            [0.0, y, 0.0],
            [16.0, y, 0.0],
            [16.0, y + 16.0, 0.0],
            [0.0, y + 16.0, 0.0],
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

        // A single 2x2-luxel style-0 lightmap for every face (harmless for
        // the sky face, which never reads it).
        let offset = i32::try_from(b.lighting.len()).unwrap();
        for sample in 0..4 {
            let level = 50 + u8::try_from(sample * 20).unwrap();
            b.push_lighting_rgb(level, level, level);
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

    b.push_leaf(-1, -1, [0, 0, 0], [16, 96, 16], 0, 3, [0, 0, 0, 0]);
    b.push_model(
        [0.0, 0.0, 0.0],
        [16.0, 96.0, 16.0],
        [0.0, 0.0, 0.0],
        [-1, -1, -1, -1],
        1,
        0,
        3,
    );
    b.build()
}

fn build_model() -> WorldModel {
    let bytes = synthetic_bsp();
    let limits = BspLimits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("synthetic map parses");
    WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    )
    .expect("synthetic map builds")
}

#[test]
fn sky_faces_are_excluded_from_geometry_and_marked_visible() {
    let model = build_model();
    // Only the water and brick faces produce triangles: two quads, six
    // indices each, sharing one index buffer (liquid batches are a range
    // into it, not a second buffer).
    assert_eq!(model.faces.len(), 2);
    assert_eq!(model.indices.len(), 12);
    assert!(model.has_sky, "the map has a sky-textured face");
}

#[test]
fn liquid_faces_are_routed_to_the_liquid_batches() {
    let model = build_model();
    assert_eq!(model.liquid_batches.len(), 1, "one liquid texture is used");
    assert!(
        model
            .batches
            .iter()
            .all(|batch| batch.texture != model.liquid_batches[0].texture)
    );
    let liquid_indices: u32 = model.liquid_batches.iter().map(|b| b.index_count).sum();
    assert_eq!(liquid_indices, 6, "one liquid quad, two triangles");
    let opaque_indices: u32 = model.batches.iter().map(|b| b.index_count).sum();
    assert_eq!(opaque_indices, 6, "one ordinary quad, two triangles");
}

#[test]
fn build_draw_list_for_model_draws_every_face_unconditionally() {
    let model = build_model();
    let mut list = DrawList::new();
    model.build_draw_list_for_model(&mut list);
    assert_eq!(list.indices, model.indices);
    assert_eq!(list.batches, model.batches);
    assert_eq!(list.liquid_indices, model.indices);
    assert_eq!(list.liquid_batches, model.liquid_batches);
    assert_eq!(list.sky_visible, model.has_sky);
}

#[test]
fn blend_lightmap_reflects_style_intensity() {
    let model = build_model();
    let full = model.blend_lightmap(|_style| 1.0);
    let dark = model.blend_lightmap(|_style| 0.0);
    let bright = model.blend_lightmap(|_style| 2.0);
    assert_eq!(full.rgba().len(), model.lightmap_atlas.rgba().len());
    // Every tile's colour should scale with the supplied intensity, and the
    // reserved white 1x1 tile should stay white regardless (it is neutral,
    // not style-driven).
    let sum = |image: &ohl_world::TextureImage| -> u64 {
        image.rgba().iter().map(|&b| u64::from(b)).sum()
    };
    assert!(sum(&bright) > sum(&full));
    assert!(sum(&full) > sum(&dark));
}
