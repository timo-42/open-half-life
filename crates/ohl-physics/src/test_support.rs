//! Synthetic collision fixtures for the player-systems tests.
//!
//! `ohl-formats`' own fixture writers cover solid brushes and a water pool.
//! The player systems additionally need volumes with the *other* documented
//! non-solid contents values — `CONTENTS_LADDER` (`func_ladder`),
//! `CONTENTS_SLIME` and `CONTENTS_LAVA` — so this module adds a small
//! contents-generic emitter on top of the public [`Bsp30Builder`] API and a
//! handful of ready-made rooms.
//!
//! Everything here is written by this project; no bytes from any game
//! installation are read, embedded or committed. The module is behind the
//! `test-support` feature so it is not part of the shipping library.

use alloc::vec;
use alloc::vec::Vec;

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush, FIXTURE_HULL_SIZES, HullPlane};

use crate::hull::{CollisionModel, contents};

/// The on-disk size of one `BSPLEAF`, used to turn the builder's raw leaf
/// lump length back into a leaf index.
const LEAF_BYTES: usize = 28;

/// `!0`: leaf 0, the shared solid leaf, as encoded in a node child link.
const SOLID_LEAF_CHILD: i16 = -1;
/// `!1`: leaf 1, the shared empty leaf.
const EMPTY_LEAF_CHILD: i16 = -2;
/// `CONTENTS_SOLID` / `CONTENTS_EMPTY` as encoded in a clipnode child link.
const SOLID_CLIPNODE_CHILD: i16 = -2;
const EMPTY_CLIPNODE_CHILD: i16 = -1;

/// A non-solid volume: the union of `brushes`, filled with `contents`.
#[derive(Debug, Clone)]
pub struct ContentsVolume {
    /// The convex brushes whose union the volume occupies.
    pub brushes: Vec<CollisionBrush>,
    /// The contents value inside it (one of [`crate::hull::contents`]'s
    /// non-solid values).
    pub contents: i32,
}

impl ContentsVolume {
    /// A volume from a single axis-aligned box.
    #[must_use]
    pub fn box_volume(mins: [f32; 3], maxs: [f32; 3], contents: i32) -> Self {
        Self {
            brushes: vec![CollisionBrush::box_brush(mins, maxs)],
            contents,
        }
    }

    /// A volume filling one half-space (`normal · x <= dist`).
    #[must_use]
    pub fn half_space(normal: [f32; 3], dist: f32, contents: i32) -> Self {
        Self {
            brushes: vec![CollisionBrush::half_space(normal, dist)],
            contents,
        }
    }
}

/// Offsets `plane` outward by the support distance of the box
/// `mins..maxs`, the documented hull-expansion rule that turns a box sweep
/// into a point trace through the matching pre-expanded hull.
fn expand_plane(plane: HullPlane, mins: [f32; 3], maxs: [f32; 3]) -> HullPlane {
    let mut support = 0.0f32;
    for axis in 0..3 {
        let n = plane.normal[axis];
        support += if n >= 0.0 {
            n * mins[axis]
        } else {
            n * maxs[axis]
        };
    }
    HullPlane::new(plane.normal, plane.dist - support)
}

/// Which of the two tree kinds is being emitted: hull 0's BSP node tree
/// (children reference leaves) or a clipnode tree for hulls 1-3 (children
/// reference contents values directly).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Nodes,
    Clipnodes,
}

struct Emitter<'a> {
    builder: &'a mut Bsp30Builder,
    kind: TreeKind,
    /// `contents value -> child link`, in the encoding `kind` uses.
    links: Vec<(i32, i16)>,
}

impl Emitter<'_> {
    fn plane_count(&self) -> usize {
        self.builder.planes.len() / 20
    }

    fn link_for(&self, value: i32) -> i16 {
        self.links
            .iter()
            .find(|(contents, _)| *contents == value)
            .map_or(-1, |(_, link)| *link)
    }

    /// Emits the tree deciding `brushes`: every point inside one of them
    /// links to `inside`, everything else to `outside`.
    fn emit_union(
        &mut self,
        brushes: &[CollisionBrush],
        mins: [f32; 3],
        maxs: [f32; 3],
        inside: i16,
        outside: i16,
    ) -> i16 {
        let Some((first, rest_brushes)) = brushes.split_first() else {
            return outside;
        };
        let rest = self.emit_union(rest_brushes, mins, maxs, inside, outside);
        let mut back = inside;
        for plane in first.planes.iter().rev() {
            let expanded = expand_plane(*plane, mins, maxs);
            let plane_index = self.plane_count();
            // Plane type 3 is the documented "any/non-axial" classification,
            // always valid for any normal.
            self.builder.push_plane(expanded.normal, expanded.dist, 3);
            let index = match self.kind {
                TreeKind::Nodes => {
                    let index = self.builder.nodes.len() / 24;
                    self.builder.push_node(
                        u32::try_from(plane_index).expect("fixture plane index fits in u32"),
                        rest,
                        back,
                        [-4096; 3],
                        [4096; 3],
                        0,
                        0,
                    );
                    index
                }
                TreeKind::Clipnodes => {
                    let index = self.builder.clipnodes.len() / 8;
                    self.builder.push_clipnode(
                        i32::try_from(plane_index).expect("fixture plane index fits in i32"),
                        rest,
                        back,
                    );
                    index
                }
            };
            back = i16::try_from(index).expect("fixture node index fits in i16");
        }
        back
    }
}

/// Builds a complete BSP30 file whose collision hulls describe the union of
/// `solid`, with each entry of `volumes` filled with its own contents value
/// (the first volume that contains a point wins). `entities` is the raw
/// entity lump text.
///
/// Returns the file bytes.
#[must_use]
pub fn build_contents_bsp(
    entities: &str,
    solid: &[CollisionBrush],
    volumes: &[ContentsVolume],
) -> Vec<u8> {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text(entities);

    // Leaf 0 is the shared solid leaf and leaf 1 the empty leaf, matching
    // the convention `ohl-formats`' own fixtures use; one further leaf is
    // added per distinct volume contents value.
    builder.push_leaf(contents::SOLID, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
    builder.push_leaf(contents::EMPTY, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
    let mut leaf_links: Vec<(i32, i16)> = Vec::new();
    for volume in volumes {
        if leaf_links
            .iter()
            .any(|(value, _)| *value == volume.contents)
        {
            continue;
        }
        let index =
            i16::try_from(builder.leaves.len() / LEAF_BYTES).expect("fixture leaf index fits");
        builder.push_leaf(volume.contents, -1, [-4096; 3], [4096; 3], 0, 0, [0; 4]);
        // A negative child link stores the bitwise complement of the leaf
        // index.
        leaf_links.push((volume.contents, !index));
    }

    let mut heads = [0i32; 4];
    for (hull, (mins, maxs)) in FIXTURE_HULL_SIZES.iter().enumerate() {
        let (kind, empty, solid_child, links) = if hull == 0 {
            // Hull 0's children name leaves as `!index`: leaf 1 is empty and
            // leaf 0 is solid.
            (
                TreeKind::Nodes,
                EMPTY_LEAF_CHILD,
                SOLID_LEAF_CHILD,
                leaf_links.clone(),
            )
        } else {
            // Clipnode children carry the contents value itself.
            (
                TreeKind::Clipnodes,
                EMPTY_CLIPNODE_CHILD,
                SOLID_CLIPNODE_CHILD,
                volumes
                    .iter()
                    .map(|volume| {
                        (
                            volume.contents,
                            i16::try_from(volume.contents).expect("contents fit in i16"),
                        )
                    })
                    .collect(),
            )
        };
        let mut emitter = Emitter {
            builder: &mut builder,
            kind,
            links,
        };
        // Innermost fallback is empty; each volume is layered on top of it,
        // and the solid union is layered on top of everything.
        let mut tree = empty;
        for volume in volumes.iter().rev() {
            let inside = emitter.link_for(volume.contents);
            tree = emitter.emit_union(&volume.brushes, *mins, *maxs, inside, tree);
        }
        heads[hull] = i32::from(emitter.emit_union(solid, *mins, *maxs, solid_child, tree));
    }

    builder.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        heads,
        i32::try_from(builder.leaves.len() / LEAF_BYTES).expect("fixture leaf count fits"),
        0,
        0,
    );
    builder.build()
}

/// Parses `bytes` and builds its collision model, panicking on a fixture
/// this crate itself just wrote that does not round-trip.
///
/// # Panics
///
/// If the bytes do not parse as a BSP v30 file with usable collision hulls.
#[must_use]
pub fn collision_model_from(bytes: &[u8]) -> CollisionModel {
    let limits = Limits::default();
    let bsp = Bsp::parse(bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

const WORLDSPAWN_ONLY: &str = "{\n\"classname\" \"worldspawn\"\n}\n";

/// A room with a floor at `z = 0`, a wall filling `x >= 96`, and a
/// `func_ladder` volume (`CONTENTS_LADDER`) against that wall spanning
/// `x` 56..96, `y` -32..32 and `z` 0..256.
///
/// A player standing at `(72, 0, 36)` is inside the volume, and the only
/// open horizontal face is toward `-X`, so the ladder's outward normal is
/// `(-1, 0, 0)`.
#[must_use]
pub fn build_ladder_room_bsp() -> Vec<u8> {
    build_contents_bsp(
        WORLDSPAWN_ONLY,
        &[
            CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
            CollisionBrush::half_space([-1.0, 0.0, 0.0], -96.0),
        ],
        &[ContentsVolume::box_volume(
            [56.0, -32.0, 0.0],
            [96.0, 32.0, 256.0],
            contents::LADDER,
        )],
    )
}

/// The surface height of [`build_liquid_room_bsp`]'s pool. It is deep
/// enough that a standing player (origin 36 above their feet, eye 28 above
/// the origin) can occupy every one of the four documented water levels
/// inside it.
pub const LIQUID_SURFACE_Z: f32 = 200.0;

/// A room with a floor at `z = 0` and a pool of `liquid` contents filling
/// `0 < z <= `[`LIQUID_SURFACE_Z`], so water/slime/lava level transitions
/// can be tested with the same geometry.
#[must_use]
pub fn build_liquid_room_bsp(liquid: i32) -> Vec<u8> {
    build_contents_bsp(
        WORLDSPAWN_ONLY,
        &[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)],
        &[ContentsVolume::half_space(
            [0.0, 0.0, 1.0],
            LIQUID_SURFACE_Z,
            liquid,
        )],
    )
}

/// A flat floor at `z = 0` with nothing else in it, for fall-damage and
/// platform-ride tests.
#[must_use]
pub fn build_flat_floor_bsp() -> Vec<u8> {
    build_contents_bsp(
        WORLDSPAWN_ONLY,
        &[CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0)],
        &[],
    )
}
