//! The small amount of linear algebra the renderer needs.
//!
//! Matrices are stored column-major as `[f32; 16]` (`m[column * 4 + row]`),
//! which is what WGSL's `mat4x4<f32>` expects in a uniform buffer and what
//! [`ohl_world::Frustum`] reads back.

/// A column-major 4x4 matrix.
pub type Mat4 = [f32; 16];

/// The identity matrix.
#[must_use]
pub fn identity() -> Mat4 {
    let mut m = [0.0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// Right-handed perspective projection mapping depth into `0..=1`, matching
/// wgpu's clip-space convention.
#[must_use]
pub fn perspective_rh(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let focal = 1.0 / (fov_y_radians * 0.5).tan();
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        1.0
    };
    let mut m = [0.0f32; 16];
    m[0] = focal / aspect;
    m[5] = focal;
    m[10] = far / (near - far);
    m[11] = -1.0;
    m[14] = (near * far) / (near - far);
    m
}

/// Right-handed view matrix looking from `eye` along `direction`.
///
/// `up` is the world's up axis; in GoldSrc space that is `+Z`, which is what
/// turns the map's Z-up coordinates into the renderer's Y-up clip space
/// without any separate conversion matrix.
#[must_use]
pub fn look_to_rh(eye: [f32; 3], direction: [f32; 3], up: [f32; 3]) -> Mat4 {
    let forward = normalize(direction);
    let side = normalize(cross(forward, up));
    let true_up = cross(side, forward);
    let mut m = [0.0f32; 16];
    m[0] = side[0];
    m[1] = true_up[0];
    m[2] = -forward[0];
    m[4] = side[1];
    m[5] = true_up[1];
    m[6] = -forward[1];
    m[8] = side[2];
    m[9] = true_up[2];
    m[10] = -forward[2];
    m[12] = -dot(side, eye);
    m[13] = -dot(true_up, eye);
    m[14] = dot(forward, eye);
    m[15] = 1.0;
    m
}

/// Matrix product `a * b`, applying `b` first.
#[must_use]
pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[column * 4 + k];
            }
            out[column * 4 + row] = sum;
        }
    }
    out
}

/// Dot product.
#[must_use]
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
#[must_use]
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Normalizes `v`, returning `+X` for a zero-length or non-finite input so
/// the caller never propagates a NaN into a matrix.
#[must_use]
pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length.is_finite() && length > 1e-6 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        [1.0, 0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::{identity, look_to_rh, multiply, normalize, perspective_rh};

    #[test]
    fn identity_is_a_multiplicative_unit() {
        let a = perspective_rh(1.0, 1.5, 1.0, 100.0);
        let product = multiply(&a, &identity());
        for (left, right) in product.iter().zip(a.iter()) {
            assert!((left - right).abs() < 1e-6);
        }
    }

    #[test]
    fn perspective_maps_near_and_far_to_zero_and_one() {
        let m = perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
        // A point on the -Z axis at distance `d` has clip z = m[10]*(-d) +
        // m[14] and clip w = d.
        for (distance, expected) in [(1.0f32, 0.0f32), (100.0, 1.0)] {
            let clip_z = m[10] * -distance + m[14];
            let clip_w = distance;
            assert!((clip_z / clip_w - expected).abs() < 1e-4);
        }
    }

    #[test]
    fn look_to_places_the_eye_at_the_origin() {
        let view = look_to_rh([10.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        // Transforming the eye must give the origin in view space.
        let eye = [10.0f32, 0.0, 0.0, 1.0];
        let mut out = [0.0f32; 4];
        for row in 0..4 {
            out[row] = (0..4).map(|k| view[k * 4 + row] * eye[k]).sum();
        }
        assert!(out[0].abs() < 1e-5 && out[1].abs() < 1e-5 && out[2].abs() < 1e-5);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn normalize_rejects_degenerate_input() {
        assert_eq!(normalize([0.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(normalize([f32::NAN, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        let unit = normalize([0.0, 3.0, 4.0]);
        assert!((unit[1] - 0.6).abs() < 1e-6 && (unit[2] - 0.8).abs() < 1e-6);
    }
}
