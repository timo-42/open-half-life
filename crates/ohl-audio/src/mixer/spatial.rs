//! Listener position/orientation and GoldSrc-style distance attenuation.
//!
//! GoldSrc-family engines spatialize a mono sound with a fixed
//! `attenuation` parameter (`0.0` for `ATTN_NONE`, larger values fall off
//! faster; this is the shape commonly described in public Quake-/GoldSrc-
//! engine modding references, not anything read from GoldSrc or Valve
//! source) and a listener position/orientation. This module implements a
//! small, self-contained version of that: linear distance falloff plus
//! equal-power stereo panning. A hand-rolled three-component vector is used
//! instead of pulling in a math crate (`glam` and similar), since dot
//! product and length are the only operations this needs.

/// A position or direction in world space.
pub type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: Vec3) -> f32 {
    dot(v, v).sqrt()
}

/// The distance, in world units, at which linear attenuation with
/// `attenuation == 1.0` reaches full silence. This mirrors the "sound clip
/// distance" constant commonly documented for Quake-family engines; it is a
/// tuning constant of this mixer, not a value read from any proprietary
/// source.
pub const MAX_AUDIBLE_DISTANCE: f32 = 1000.0;

/// The listener's position and stereo axis (`right`, unit length) in world
/// space. Only the horizontal pan axis is modeled; there is no elevation or
/// front/back distinction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    pub right: Vec3,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            right: [1.0, 0.0, 0.0],
        }
    }
}

/// A sound's position and GoldSrc-style `attenuation` parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundSpatial {
    pub position: Vec3,
    /// `0.0` means no distance attenuation (`ATTN_NONE`); larger values
    /// fall off faster. Values are expected in roughly `0.0..=4.0`, matching
    /// the documented GoldSrc `ATTN_*` range, but any non-negative value is
    /// accepted.
    pub attenuation: f32,
}

/// Per-ear gain multipliers, already folded together with a channel's
/// volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoGain {
    pub left: f32,
    pub right: f32,
}

/// Computes linear distance attenuation in `[0, 1]`: `1.0` at zero
/// distance, falling linearly to `0.0` at
/// `MAX_AUDIBLE_DISTANCE / attenuation.max(1.0)`... in this simplified
/// model, at `MAX_AUDIBLE_DISTANCE` scaled by `attenuation` directly, so
/// `attenuation == 0.0` never attenuates and larger `attenuation` values
/// reach silence at a closer distance. Monotonically non-increasing in
/// `distance` for a fixed `attenuation`.
#[must_use]
pub fn distance_attenuation(distance: f32, attenuation: f32) -> f32 {
    if attenuation <= 0.0 {
        return 1.0;
    }
    let falloff = (distance / MAX_AUDIBLE_DISTANCE) * attenuation;
    (1.0 - falloff).clamp(0.0, 1.0)
}

/// Computes equal-power stereo pan gains from `pan` in `[-1, 1]` (`-1` is
/// full left, `0` is centered, `1` is full right). `left` is monotonically
/// non-increasing and `right` is monotonically non-decreasing as `pan`
/// increases.
#[must_use]
pub fn equal_power_pan(pan: f32) -> StereoGain {
    let pan = pan.clamp(-1.0, 1.0);
    StereoGain {
        left: f32::midpoint(1.0, -pan).sqrt(),
        right: f32::midpoint(1.0, pan).sqrt(),
    }
}

/// Computes the full spatial stereo gain (attenuation folded with pan) for
/// a mono source, given the `listener` and the sound's `spatial` parameters
/// and `volume`.
#[must_use]
pub fn spatial_gain(listener: &Listener, spatial: SoundSpatial, volume: f32) -> StereoGain {
    let delta = sub(spatial.position, listener.position);
    let distance = length(delta);
    let attenuation_gain = distance_attenuation(distance, spatial.attenuation);

    let pan = if distance < 1e-6 {
        0.0
    } else {
        let direction = [
            delta[0] / distance,
            delta[1] / distance,
            delta[2] / distance,
        ];
        dot(direction, listener.right).clamp(-1.0, 1.0)
    };

    let StereoGain { left, right } = equal_power_pan(pan);
    let gain = volume * attenuation_gain;
    StereoGain {
        left: left * gain,
        right: right * gain,
    }
}

#[cfg(test)]
mod tests {
    use super::{Listener, SoundSpatial, distance_attenuation, equal_power_pan, spatial_gain};

    #[test]
    fn attenuation_is_full_at_zero_distance() {
        assert!((distance_attenuation(0.0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn attenuation_zero_never_falls_off() {
        assert!((distance_attenuation(10_000.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn attenuation_is_monotonically_non_increasing_with_distance() {
        let mut previous = distance_attenuation(0.0, 1.0);
        for step in 1..=20u32 {
            let distance = f32::from(u16::try_from(step).expect("small test step")) * 100.0;
            let gain = distance_attenuation(distance, 1.0);
            assert!(gain <= previous + 1e-6, "gain increased with distance");
            previous = gain;
        }
    }

    #[test]
    fn attenuation_reaches_silence_at_max_distance() {
        assert!(distance_attenuation(1000.0, 1.0) <= 1e-6);
        assert!(distance_attenuation(5000.0, 1.0) <= 1e-6);
    }

    #[test]
    fn pan_is_centered_and_equal_at_zero() {
        let gains = equal_power_pan(0.0);
        assert!((gains.left - gains.right).abs() < 1e-6);
        assert!((gains.left - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn pan_left_monotonically_decreases_right_increases() {
        let mut previous_left = equal_power_pan(-1.0).left;
        let mut previous_right = equal_power_pan(-1.0).right;
        let mut pan = -1.0f32;
        while pan <= 1.0 {
            let gains = equal_power_pan(pan);
            assert!(gains.left <= previous_left + 1e-6);
            assert!(gains.right >= previous_right - 1e-6);
            previous_left = gains.left;
            previous_right = gains.right;
            pan += 0.1;
        }
    }

    #[test]
    fn sound_directly_right_of_listener_is_louder_in_right_ear() {
        let listener = Listener::default();
        let spatial = SoundSpatial {
            position: [10.0, 0.0, 0.0],
            attenuation: 0.0,
        };
        let gains = spatial_gain(&listener, spatial, 1.0);
        assert!(gains.right > gains.left);
    }
}
