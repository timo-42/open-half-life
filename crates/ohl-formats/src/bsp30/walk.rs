//! Walking the BSP node tree to find the leaf containing a point.
//!
//! Each node stores a plane; the point is classified against that plane and
//! recurses into the front or back child. A child index encodes either
//! another node (`>= 0`) or a leaf (`< 0`, leaf index is `!child`), matching
//! the signed-child convention documented for `BSPNODE`/`node_t` in both
//! sources recorded in `docs/FORMAT_SOURCES.md`.
//!
//! The walk is iterative and bounded by
//! [`crate::bsp30::Limits::max_walk_depth`], so a cyclic or malformed node
//! tree is rejected instead of looping or overflowing the stack.

use crate::bsp30::raw::{Node, Plane};
use crate::error::{FormatError, Result};

/// Classifies `point` against `plane` and returns which child to descend
/// into: `true` for front (at or in front of the plane), `false` for back.
fn is_front(point: [f32; 3], plane: &Plane) -> bool {
    let normal = [
        plane.normal[0].get(),
        plane.normal[1].get(),
        plane.normal[2].get(),
    ];
    let dot = point[0] * normal[0] + point[1] * normal[1] + point[2] * normal[2];
    dot >= plane.dist.get()
}

/// Finds the leaf index containing `point`, starting the walk at
/// `head_node`.
///
/// `planes` and `nodes` are the BSP30 planes and nodes lumps; every plane
/// and child reference is bounds-checked. Returns
/// [`FormatError::RecursionLimitExceeded`] if the walk exceeds
/// `max_depth` steps, which is the only way a cyclic node graph can be
/// detected without an unbounded visited-set.
pub fn find_leaf(
    nodes: &[Node],
    planes: &[Plane],
    head_node: i32,
    point: [f32; 3],
    max_depth: u32,
) -> Result<usize> {
    // A negative head node means the model's tree is a single leaf.
    if head_node < 0 {
        return leaf_index_from_child(head_node);
    }

    let mut current = head_node;
    for _ in 0..max_depth {
        let node_index = usize::try_from(current).map_err(|_| FormatError::IndexOutOfRange)?;
        let node = nodes.get(node_index).ok_or(FormatError::IndexOutOfRange)?;
        let plane_index = node.plane.get() as usize;
        let plane = planes
            .get(plane_index)
            .ok_or(FormatError::IndexOutOfRange)?;
        let front = is_front(point, plane);
        let child: i16 = if front {
            node.children[0].get()
        } else {
            node.children[1].get()
        };
        if child < 0 {
            return leaf_index_from_child(i32::from(child));
        }
        current = i32::from(child);
    }
    Err(FormatError::RecursionLimitExceeded)
}

fn leaf_index_from_child(child: i32) -> Result<usize> {
    // Leaf indices are encoded as the bitwise complement of a negative
    // child value (BSP convention: `-1` is the shared "solid" leaf 0).
    let encoded = u32::try_from(!child).map_err(|_| FormatError::IndexOutOfRange)?;
    Ok(encoded as usize)
}

#[cfg(test)]
mod tests {
    use super::find_leaf;
    use crate::bsp30::raw::{Node, Plane};
    use zerocopy::byteorder::little_endian::{F32, I16, I32, U16, U32};

    fn plane(normal: [f32; 3], dist: f32) -> Plane {
        Plane {
            normal: [
                F32::new(normal[0]),
                F32::new(normal[1]),
                F32::new(normal[2]),
            ],
            dist: F32::new(dist),
            kind: I32::new(0),
        }
    }

    fn node(plane: u32, front: i16, back: i16) -> Node {
        Node {
            plane: U32::new(plane),
            children: [I16::new(front), I16::new(back)],
            mins: [I16::new(0); 3],
            maxs: [I16::new(0); 3],
            first_face: U16::new(0),
            num_faces: U16::new(0),
        }
    }

    #[test]
    fn finds_leaf_on_each_side_of_a_splitting_plane() {
        let planes = [plane([1.0, 0.0, 0.0], 0.0)];
        // Front (x >= 0) goes to leaf 0 (child = !0 = -1); back goes to leaf 1.
        let nodes = [node(0, -1, -2)];
        let leaf = find_leaf(&nodes, &planes, 0, [5.0, 0.0, 0.0], 64).unwrap();
        assert_eq!(leaf, 0);
        let leaf = find_leaf(&nodes, &planes, 0, [-5.0, 0.0, 0.0], 64).unwrap();
        assert_eq!(leaf, 1);
    }

    #[test]
    fn single_leaf_model_resolves_without_nodes() {
        let leaf = find_leaf(&[], &[], -1, [0.0, 0.0, 0.0], 64).unwrap();
        assert_eq!(leaf, 0);
    }

    #[test]
    fn rejects_a_node_cycle() {
        // Node 0 points at itself, which would loop forever without a depth
        // limit.
        let planes = [plane([1.0, 0.0, 0.0], 0.0)];
        let nodes = [node(0, 0, 0)];
        let result = find_leaf(&nodes, &planes, 0, [1.0, 0.0, 0.0], 16);
        assert!(result.is_err());
    }
}
