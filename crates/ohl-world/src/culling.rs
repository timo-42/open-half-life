//! Axis-aligned bounds and a view-frustum test.
//!
//! Planes are extracted from a view-projection matrix with the standard
//! Gribb/Hartmann method (adding or subtracting the matrix's w row from each
//! of its x/y/z rows yields the six clip-space planes in world space). The
//! matrix is taken column-major, matching WGSL's `mat4x4<f32>` layout, and
//! the resulting planes are therefore in the same GoldSrc world space as the
//! geometry.

/// An axis-aligned bounding box in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Component-wise minimum corner.
    pub min: [f32; 3],
    /// Component-wise maximum corner.
    pub max: [f32; 3],
}

impl Aabb {
    /// An empty box that grows to fit whatever is added to it.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }

    /// Grows the box to contain `point`.
    pub fn extend(&mut self, point: [f32; 3]) {
        for ((min, max), value) in self.min.iter_mut().zip(self.max.iter_mut()).zip(point) {
            *min = min.min(value);
            *max = max.max(value);
        }
    }

    /// Whether the box contains at least one point.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }
}

/// Six world-space half-spaces; a point is inside when it is on the positive
/// side of every one of them.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    /// Extracts the frustum from a column-major view-projection matrix
    /// (`matrix[column * 4 + row]`).
    #[must_use]
    pub fn from_view_projection(matrix: &[f32; 16]) -> Self {
        // Row `r` of the matrix, given column-major storage.
        let row = |r: usize| [matrix[r], matrix[4 + r], matrix[8 + r], matrix[12 + r]];
        let (x, y, z, w) = (row(0), row(1), row(2), row(3));
        let combine = |a: [f32; 4], b: [f32; 4], add: bool| {
            let mut plane = [0.0f32; 4];
            for i in 0..4 {
                plane[i] = if add { a[i] + b[i] } else { a[i] - b[i] };
            }
            normalize(plane)
        };
        Self {
            planes: [
                combine(w, x, true),  // left
                combine(w, x, false), // right
                combine(w, y, true),  // bottom
                combine(w, y, false), // top
                // wgpu clip space keeps depth in `0..=1`, so the near plane
                // is the z row alone rather than `w + z`.
                normalize(z),
                combine(w, z, false), // far
            ],
        }
    }

    /// Whether `bounds` is at least partially inside the frustum.
    ///
    /// Conservative: a box that is fully outside one plane is rejected, and
    /// anything else is accepted, so no visible geometry is ever culled.
    #[must_use]
    pub fn intersects(&self, bounds: &Aabb) -> bool {
        if !bounds.is_valid() {
            return false;
        }
        for plane in &self.planes {
            // The box corner furthest along the plane normal; if even that
            // is behind the plane, every corner is.
            let mut distance = plane[3];
            for ((coefficient, min), max) in plane.iter().copied().zip(bounds.min).zip(bounds.max) {
                distance += coefficient * if coefficient >= 0.0 { max } else { min };
            }
            if distance < 0.0 {
                return false;
            }
        }
        true
    }

    /// Whether `point` is inside every plane.
    #[must_use]
    pub fn contains_point(&self, point: [f32; 3]) -> bool {
        self.planes.iter().all(|plane| {
            plane[0] * point[0] + plane[1] * point[1] + plane[2] * point[2] + plane[3] >= 0.0
        })
    }
}

fn normalize(plane: [f32; 4]) -> [f32; 4] {
    let length = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
    if length > 0.0 && length.is_finite() {
        [
            plane[0] / length,
            plane[1] / length,
            plane[2] / length,
            plane[3] / length,
        ]
    } else {
        plane
    }
}

#[cfg(test)]
mod tests {
    use super::{Aabb, Frustum};

    /// A column-major orthographic matrix mapping `[-1, 1]^2` in x/y and
    /// `[0, 2]` in z to wgpu clip space, so the frustum is a unit-ish box.
    fn ortho_box() -> [f32; 16] {
        let mut m = [0.0f32; 16];
        m[0] = 1.0; // x scale
        m[5] = 1.0; // y scale
        m[10] = 0.5; // z: 0..2 -> 0..1
        m[15] = 1.0;
        m
    }

    #[test]
    fn contains_points_inside_the_box() {
        let frustum = Frustum::from_view_projection(&ortho_box());
        assert!(frustum.contains_point([0.0, 0.0, 1.0]));
        assert!(!frustum.contains_point([5.0, 0.0, 1.0]));
        assert!(!frustum.contains_point([0.0, 0.0, -1.0]));
    }

    #[test]
    fn accepts_straddling_and_rejects_outside_boxes() {
        let frustum = Frustum::from_view_projection(&ortho_box());
        let inside = Aabb {
            min: [-0.5, -0.5, 0.5],
            max: [0.5, 0.5, 1.5],
        };
        let straddling = Aabb {
            min: [0.5, -0.5, 0.5],
            max: [9.0, 0.5, 1.5],
        };
        let outside = Aabb {
            min: [4.0, -0.5, 0.5],
            max: [9.0, 0.5, 1.5],
        };
        assert!(frustum.intersects(&inside));
        assert!(frustum.intersects(&straddling));
        assert!(!frustum.intersects(&outside));
        assert!(!frustum.intersects(&Aabb::empty()));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn extend_grows_bounds() {
        let mut bounds = Aabb::empty();
        assert!(!bounds.is_valid());
        bounds.extend([1.0, 2.0, 3.0]);
        bounds.extend([-1.0, 5.0, 0.0]);
        assert_eq!(bounds.min, [-1.0, 2.0, 0.0]);
        assert_eq!(bounds.max, [1.0, 5.0, 3.0]);
    }
}
