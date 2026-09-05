//! `WorldModel` construction and its per-frame queries must never panic on
//! any structurally valid BSP the synthetic writer can produce, including
//! ones whose cross-lump references are nonsense.
//!
//! The generator deliberately emits *structurally* well-formed files (a real
//! header, whole records, a well-formed texture directory) with arbitrary
//! field values, which is the shape a hostile map file would take: every
//! lump parses, and only the indices between them are wrong.

use ohl_formats::bsp30::Bsp;
use ohl_formats::test_support::Bsp30Builder;
use ohl_world::{BspLimits, DrawList, Frustum, WorldBuildOptions, WorldModel};
use proptest::prelude::*;

#[derive(Debug, Clone)]
struct FaceSpec {
    plane: u16,
    plane_side: u16,
    first_edge: u32,
    num_edges: u16,
    texinfo: u16,
    style: u8,
    lightmap_offset: i32,
}

#[derive(Debug, Clone)]
struct MapSpec {
    vertices: Vec<[f32; 3]>,
    edges: Vec<(u16, u16)>,
    surfedges: Vec<i32>,
    faces: Vec<FaceSpec>,
    texinfo_axes: Vec<([f32; 3], [f32; 3], u32, u32)>,
    marksurfaces: Vec<u16>,
    leaves: Vec<(i32, u16, u16)>,
    lighting: Vec<u8>,
    visibility: Vec<u8>,
    model_faces: (i32, i32),
    embedded_textures: usize,
    external_textures: usize,
}

fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        9 => -4096.0f32..4096.0,
        1 => Just(0.0f32),
    ]
}

fn map_spec() -> impl Strategy<Value = MapSpec> {
    (
        proptest::collection::vec(
            (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| [x, y, z]),
            0..24,
        ),
        proptest::collection::vec((any::<u16>(), any::<u16>()), 0..24),
        proptest::collection::vec(-32i32..32, 0..32),
        proptest::collection::vec(
            (
                any::<u16>(),
                any::<u16>(),
                0u32..32,
                0u16..8,
                any::<u16>(),
                any::<u8>(),
                -8i32..512,
            )
                .prop_map(
                    |(
                        plane,
                        plane_side,
                        first_edge,
                        num_edges,
                        texinfo,
                        style,
                        lightmap_offset,
                    )| {
                        FaceSpec {
                            plane,
                            plane_side,
                            first_edge,
                            num_edges,
                            texinfo,
                            style,
                            lightmap_offset,
                        }
                    },
                ),
            0..12,
        ),
        proptest::collection::vec(
            (
                (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| [x, y, z]),
                (coordinate(), coordinate(), coordinate()).prop_map(|(x, y, z)| [x, y, z]),
                0u32..6,
                0u32..4,
            ),
            0..8,
        ),
        proptest::collection::vec(any::<u16>(), 0..24),
        proptest::collection::vec((-3i32..64, any::<u16>(), 0u16..8), 0..8),
        proptest::collection::vec(any::<u8>(), 0..512),
        proptest::collection::vec(any::<u8>(), 0..64),
        (-4i32..16, -4i32..16),
        0usize..3,
        0usize..3,
    )
        .prop_map(
            |(
                vertices,
                edges,
                surfedges,
                faces,
                texinfo_axes,
                marksurfaces,
                leaves,
                lighting,
                visibility,
                model_faces,
                embedded_textures,
                external_textures,
            )| MapSpec {
                vertices,
                edges,
                surfedges,
                faces,
                texinfo_axes,
                marksurfaces,
                leaves,
                lighting,
                visibility,
                model_faces,
                embedded_textures,
                external_textures,
            },
        )
}

fn build_bytes(spec: &MapSpec) -> Vec<u8> {
    let mut b = Bsp30Builder::new();
    b.set_entities_text(
        "{\n\"classname\" \"worldspawn\"\n}\n\
         {\n\"classname\" \"info_player_start\"\n\"origin\" \"1 2 3\"\n}\n",
    );
    b.push_plane([1.0, 0.0, 0.0], 0.0, 0);
    b.push_plane([0.0, 0.0, 1.0], 16.0, 2);

    for vertex in &spec.vertices {
        b.push_vertex(*vertex);
    }
    for (v0, v1) in &spec.edges {
        b.push_edge(*v0, *v1);
    }
    for surfedge in &spec.surfedges {
        b.push_surfedge(*surfedge);
    }
    for (s_vector, t_vector, miptex, flags) in &spec.texinfo_axes {
        b.push_texinfo(*s_vector, 0.5, *t_vector, -0.5, *miptex, *flags);
    }
    for face in &spec.faces {
        b.push_face(
            face.plane,
            face.plane_side,
            face.first_edge,
            face.num_edges,
            face.texinfo,
            [face.style, 0xFF, 0xFF, 0xFF],
            face.lightmap_offset,
        );
    }
    for mark in &spec.marksurfaces {
        b.push_marksurface(*mark);
    }
    for (vis_offset, first_marksurface, num_marksurfaces) in &spec.leaves {
        b.push_leaf(
            -1,
            *vis_offset,
            [-1, -1, -1],
            [1, 1, 1],
            *first_marksurface,
            *num_marksurfaces,
            [0, 0, 0, 0],
        );
    }
    b.lighting.extend_from_slice(&spec.lighting);
    b.visibility.extend_from_slice(&spec.visibility);
    b.push_node(0, -1, -2, [-1, -1, -1], [1, 1, 1], 0, 1);
    b.push_node(1, 0, -3, [-1, -1, -1], [1, 1, 1], 0, 1);
    b.push_clipnode(0, -1, -2);
    b.push_model(
        [-1.0, -1.0, -1.0],
        [1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0],
        [1, 0, 0, 0],
        2,
        spec.model_faces.0,
        spec.model_faces.1,
    );
    for index in 0..spec.embedded_textures {
        #[allow(clippy::cast_possible_truncation)]
        b.add_embedded_texture("ohltex", 16, 16, index as u8);
    }
    for _ in 0..spec.external_textures {
        b.add_external_texture("ohlext", 16, 16);
    }
    b.build()
}

fn exercise(bytes: &[u8]) {
    let limits = BspLimits::default();
    let Ok(bsp) = Bsp::parse(bytes, &limits) else {
        return;
    };
    let Ok(model) = WorldModel::build(
        &bsp,
        &WorldBuildOptions {
            wads: &[],
            limits,
            ..WorldBuildOptions::default()
        },
    ) else {
        return;
    };

    // Every emitted index must address a real vertex, and every face range a
    // real slice of the index buffer.
    for index in &model.indices {
        assert!((*index as usize) < model.vertices.len());
    }
    for face in &model.faces {
        let end = face.first_index as usize + face.index_count as usize;
        assert!(end <= model.indices.len());
        assert_eq!(face.index_count % 3, 0);
        assert!(face.texture < model.textures.len());
    }

    let mut list = DrawList::new();
    let mut identity = [0.0f32; 16];
    identity[0] = 1.0;
    identity[5] = 1.0;
    identity[10] = 1.0;
    identity[15] = 1.0;
    let frustum = Frustum::from_view_projection(&identity);
    for eye in [[0.0, 0.0, 0.0], [1e6, -1e6, 0.5], [-3.0, 2.0, 8.0]] {
        let _ = model.leaf_at(eye);
        model.build_draw_list(eye, Some(&frustum), &mut list);
        model.build_draw_list(eye, None, &mut list);
        assert_eq!(list.indices.len() % 3, 0);
        let total: usize = list.batches.iter().map(|b| b.index_count as usize).sum();
        assert_eq!(total, list.indices.len());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn world_model_construction_never_panics(spec in map_spec()) {
        exercise(&build_bytes(&spec));
    }
}
