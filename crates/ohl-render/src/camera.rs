//! A free-fly camera in GoldSrc world units.
//!
//! GoldSrc measures distance in units where one unit is roughly one inch and
//! orients the world Z-up with +X forward and +Y left. Yaw therefore turns
//! counter-clockwise around +Z starting at +X, and positive pitch looks
//! down, matching the angles stored in a map's entities.

use ohl_world::{Frustum, PlayerSpawn};

use crate::math::{self, Mat4};

/// Which way the camera is being pushed this frame, in its own frame of
/// reference. Each field is `-1`, `0` or `+1` after [`MoveInput::clamped`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveInput {
    /// Forward (`+1`) and back (`-1`), i.e. W and S.
    pub forward: i8,
    /// Right (`+1`) and left (`-1`), i.e. D and A.
    pub right: i8,
    /// Up (`+1`) and down (`-1`), i.e. Space and Ctrl.
    pub up: i8,
    /// Whether the "move faster" modifier is held.
    pub fast: bool,
}

impl MoveInput {
    /// Clamps each axis into `-1..=1`.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            forward: self.forward.clamp(-1, 1),
            right: self.right.clamp(-1, 1),
            up: self.up.clamp(-1, 1),
            fast: self.fast,
        }
    }
}

/// A WASD + mouse-look camera.
#[derive(Debug, Clone, Copy)]
pub struct FreeFlyCamera {
    /// Eye position in GoldSrc world units.
    pub position: [f32; 3],
    /// Yaw in degrees, counter-clockwise around +Z from +X.
    pub yaw: f32,
    /// Pitch in degrees; positive looks down.
    pub pitch: f32,
    /// Vertical field of view in degrees.
    pub fov_y_degrees: f32,
    /// Movement speed in units per second.
    pub speed: f32,
    /// Degrees of rotation per pixel of mouse motion.
    pub sensitivity: f32,
    /// Near clip distance in units.
    pub near: f32,
    /// Far clip distance in units.
    pub far: f32,
}

impl Default for FreeFlyCamera {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 64.0],
            yaw: 0.0,
            pitch: 0.0,
            // GoldSrc's default vertical field of view.
            fov_y_degrees: 75.0,
            speed: 320.0,
            sensitivity: 0.15,
            near: 4.0,
            far: 16_384.0,
        }
    }
}

/// The steepest pitch the camera may reach, in degrees, so the view
/// direction never becomes parallel to the up axis.
const MAX_PITCH_DEGREES: f32 = 89.0;

impl FreeFlyCamera {
    /// Places the camera at a map's `info_player_start`, at roughly eye
    /// height above the spawn origin.
    #[must_use]
    pub fn at_spawn(spawn: PlayerSpawn) -> Self {
        Self {
            position: [
                spawn.origin[0],
                spawn.origin[1],
                // GoldSrc's standing view offset above the spawn origin.
                spawn.origin[2] + 28.0,
            ],
            yaw: spawn.yaw,
            pitch: spawn.pitch.clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES),
            ..Self::default()
        }
    }

    /// The unit view direction in GoldSrc world space.
    #[must_use]
    pub fn direction(&self) -> [f32; 3] {
        let yaw = self.yaw.to_radians();
        let pitch = self.pitch.to_radians();
        let (yaw_sin, yaw_cos) = yaw.sin_cos();
        let (pitch_sin, pitch_cos) = pitch.sin_cos();
        math::normalize([pitch_cos * yaw_cos, pitch_cos * yaw_sin, -pitch_sin])
    }

    /// Applies relative mouse motion in pixels.
    pub fn apply_mouse_delta(&mut self, delta_x: f32, delta_y: f32) {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return;
        }
        self.yaw = (self.yaw - delta_x * self.sensitivity).rem_euclid(360.0);
        self.pitch =
            (self.pitch + delta_y * self.sensitivity).clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
    }

    /// Advances the camera by `seconds` of movement.
    pub fn update(&mut self, input: MoveInput, seconds: f32) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        let input = input.clamped();
        let forward = self.direction();
        let right = math::normalize(math::cross(forward, [0.0, 0.0, 1.0]));
        let speed = self.speed * if input.fast { 3.0 } else { 1.0 } * seconds;
        for axis in 0..3 {
            self.position[axis] += (forward[axis] * f32::from(input.forward)
                + right[axis] * f32::from(input.right))
                * speed;
        }
        self.position[2] += f32::from(input.up) * speed;
    }

    /// The view matrix.
    #[must_use]
    pub fn view(&self) -> Mat4 {
        math::look_to_rh(self.position, self.direction(), [0.0, 0.0, 1.0])
    }

    /// The projection matrix for a viewport of the given aspect ratio.
    #[must_use]
    pub fn projection(&self, aspect: f32) -> Mat4 {
        math::perspective_rh(self.fov_y_degrees.to_radians(), aspect, self.near, self.far)
    }

    /// The combined view-projection matrix.
    #[must_use]
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        math::multiply(&self.projection(aspect), &self.view())
    }

    /// The world-space frustum for a viewport of the given aspect ratio.
    #[must_use]
    pub fn frustum(&self, aspect: f32) -> Frustum {
        Frustum::from_view_projection(&self.view_projection(aspect))
    }
}

#[cfg(test)]
mod tests {
    use super::{FreeFlyCamera, MAX_PITCH_DEGREES, MoveInput};
    use ohl_world::PlayerSpawn;

    #[test]
    fn yaw_zero_looks_along_positive_x() {
        let camera = FreeFlyCamera::default();
        let direction = camera.direction();
        assert!((direction[0] - 1.0).abs() < 1e-5);
        assert!(direction[1].abs() < 1e-5 && direction[2].abs() < 1e-5);
    }

    #[test]
    fn pitch_is_clamped_and_yaw_wraps() {
        let mut camera = FreeFlyCamera::default();
        camera.apply_mouse_delta(0.0, 100_000.0);
        assert!((camera.pitch - MAX_PITCH_DEGREES).abs() < 1e-4);
        camera.apply_mouse_delta(-100_000.0, 0.0);
        assert!((0.0..360.0).contains(&camera.yaw));
        camera.apply_mouse_delta(f32::NAN, 0.0);
        assert!(camera.yaw.is_finite());
    }

    #[test]
    fn forward_input_moves_along_the_view_direction() {
        let mut camera = FreeFlyCamera {
            speed: 100.0,
            ..FreeFlyCamera::default()
        };
        camera.update(
            MoveInput {
                forward: 1,
                ..MoveInput::default()
            },
            1.0,
        );
        assert!((camera.position[0] - 100.0).abs() < 1e-3);
    }

    #[test]
    fn strafing_is_perpendicular_and_vertical_input_is_world_up() {
        let mut camera = FreeFlyCamera {
            speed: 10.0,
            ..FreeFlyCamera::default()
        };
        let start = camera.position;
        camera.update(
            MoveInput {
                right: 1,
                up: 1,
                ..MoveInput::default()
            },
            1.0,
        );
        assert!((camera.position[0] - start[0]).abs() < 1e-3);
        // +X forward, +Z up, so "right" is -Y in GoldSrc's left-handed-ish
        // axis naming.
        assert!(camera.position[1] < start[1]);
        assert!((camera.position[2] - start[2] - 10.0).abs() < 1e-3);
    }

    #[test]
    fn spawn_places_the_camera_at_eye_height() {
        let camera = FreeFlyCamera::at_spawn(PlayerSpawn {
            origin: [1.0, 2.0, 3.0],
            yaw: 90.0,
            pitch: 200.0,
        });
        assert!((camera.position[2] - 31.0).abs() < 1e-4);
        assert!((camera.pitch - MAX_PITCH_DEGREES).abs() < 1e-4);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_zero_timestep_does_not_move() {
        let mut camera = FreeFlyCamera::default();
        let start = camera.position;
        camera.update(
            MoveInput {
                forward: 1,
                ..MoveInput::default()
            },
            0.0,
        );
        assert_eq!(camera.position, start);
    }
}
