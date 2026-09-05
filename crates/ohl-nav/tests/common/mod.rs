//! Synthetic collision fixtures shared by the navigation tests.
//!
//! Every byte here is authored by this project from the documented BSP v30
//! layout and hull table (`ohl_formats::test_support`); no game data is read.

#![allow(dead_code)]

use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
use ohl_nav::Vec3;
use ohl_physics::CollisionModel;

/// Builds a collision model from a brush list, wrapping it in a minimal but
/// complete BSP30 file so the ordinary loading path is exercised.
pub fn model_from_brushes(brushes: &[CollisionBrush]) -> CollisionModel {
    let mut builder = Bsp30Builder::new();
    builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
    let heads = builder.push_collision_hulls(brushes);
    builder.push_model(
        [-4096.0, -4096.0, -4096.0],
        [4096.0, 4096.0, 4096.0],
        [0.0, 0.0, 0.0],
        heads,
        2,
        0,
        0,
    );
    let bytes = builder.build();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

/// A flat floor at `z = 0` inside a 1,024-unit box, with nothing in it.
pub fn open_room() -> CollisionModel {
    model_from_brushes(&room_shell())
}

/// The floor, ceiling and four outer walls of every fixture room.
fn room_shell() -> Vec<CollisionBrush> {
    vec![
        CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
        CollisionBrush::half_space([0.0, 0.0, -1.0], -512.0),
        CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
        CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
    ]
}

/// The same room, split in two by a wall at `x = 0` with a doorway of
/// `gap` units centred on `y = 0`.
///
/// With `gap = 40` the 32-unit-wide humanoid hull fits through with 4 units
/// to spare on each side and the 64-unit-wide large hull does not, which is
/// what the per-hull link validation is asserted against.
pub fn doorway_room(gap: f32) -> CollisionModel {
    let mut brushes = room_shell();
    brushes.push(CollisionBrush::box_brush(
        [-8.0, -512.0, -16.0],
        [8.0, -gap / 2.0, 128.0],
    ));
    brushes.push(CollisionBrush::box_brush(
        [-8.0, gap / 2.0, -16.0],
        [8.0, 512.0, 128.0],
    ));
    model_from_brushes(&brushes)
}

/// An L-shaped corridor 128 units wide: west to east along `y = 0` up to
/// `x = 64`, then north along `x = 0` up to `y = 512`.
pub fn corner_corridor() -> CollisionModel {
    let mut brushes = room_shell();
    for (mins, maxs) in [
        ([-512.0, 64.0, -16.0], [-64.0, 512.0, 256.0]),
        ([64.0, 64.0, -16.0], [512.0, 512.0, 256.0]),
        ([-512.0, -512.0, -16.0], [512.0, -64.0, 256.0]),
        ([64.0, -64.0, -16.0], [512.0, 64.0, 256.0]),
        ([-512.0, -64.0, -16.0], [-256.0, 64.0, 256.0]),
    ] {
        brushes.push(CollisionBrush::box_brush(mins, maxs));
    }
    model_from_brushes(&brushes)
}

/// Advances `pos` by `delta`, stopping at whatever the hull hits, the way a
/// fixed-tick mover would.
pub fn slide_move(
    collision: &CollisionModel,
    hull: ohl_physics::Hull,
    pos: Vec3,
    delta: Vec3,
) -> Vec3 {
    let trace = collision.trace(hull, pos, pos + delta);
    if trace.start_solid {
        pos
    } else {
        trace.end_pos
    }
}

/// A ground seed on the floor at `x, y`.
pub fn ground(x: f32, y: f32) -> ohl_nav::NodeSeed {
    ohl_nav::NodeSeed::new(Vec3::new(x, y, 8.0), ohl_nav::NodeKind::Ground)
}

/// The lattice used with [`doorway_room`]: an outlying node on each side of
/// the wall (0 and 4, whose straight line crosses solid wall), an approach
/// node on each side lined up with the doorway (1 and 3), and the doorway
/// node itself (2).
pub fn doorway_lattice() -> Vec<ohl_nav::NodeSeed> {
    vec![
        ground(-192.0, 128.0),
        ground(-96.0, 0.0),
        ground(0.0, 0.0),
        ground(96.0, 0.0),
        ground(192.0, 128.0),
    ]
}
