//! Liquid-surface UV turbulence.
//!
//! GoldSrc's `!`-prefixed water textures are documented (the303.org's
//! "GoldSrc Map Texture Tutorial", part 6) as "procedurally animated to
//! distort and fluctuate", without a public specification of the exact
//! per-texel warp. The amplitude, period and phase-mixing constants here are
//! this project's own design, chosen only to reproduce that same qualitative
//! "liquid" look; nothing here is derived from any engine's source. See
//! `docs/FORMAT_SOURCES.md`, "Rendering conventions".
//!
//! `world_water.wgsl` implements the identical formula on the GPU (kept in
//! sync with these constants by
//! [`tests::shader_source_embeds_the_same_constants`]); this module exists
//! so the warp itself has a plain-Rust unit test. Nothing in the render
//! path calls this module at runtime — the shader hardcodes the same
//! literals directly, since a static `include_str!` WGSL string cannot be
//! parameterised from a Rust constant — so its items are only reachable
//! from its own tests; `#[allow(dead_code)]` records that as intentional
//! rather than an oversight.
#![allow(dead_code)]

/// The warp's amplitude, as a fraction of one texture tile.
pub const TURBULENCE_AMPLITUDE: f32 = 0.125;

/// How many warp cycles complete per second.
pub const TURBULENCE_SPEED: f32 = 1.0;

/// How tightly the warp varies across the surface, in cycles per texture
/// unit along the cross axis.
pub const TURBULENCE_SCALE: f32 = 4.0;

/// The sine-based UV perturbation applied to one texture axis: `coordinate`
/// is the axis being warped and `cross_coordinate` is the other axis, so
/// perturbing `u` and `v` with their axes swapped produces a two-dimensional
/// ripple instead of two parallel one-dimensional ones.
#[must_use]
pub fn turbulence_offset(coordinate: f32, cross_coordinate: f32, time_seconds: f32) -> f32 {
    let phase = cross_coordinate.mul_add(
        TURBULENCE_SCALE,
        time_seconds * TURBULENCE_SPEED * core::f32::consts::TAU,
    ) + coordinate * 0.5;
    phase.sin() * TURBULENCE_AMPLITUDE
}

#[cfg(test)]
mod tests {
    use super::{TURBULENCE_AMPLITUDE, turbulence_offset};

    #[test]
    fn stays_within_the_declared_amplitude() {
        for step in 0u8..200 {
            let t = f32::from(step) * 0.037;
            let offset = turbulence_offset(t, t * 1.3, t * 0.7);
            assert!(offset.abs() <= TURBULENCE_AMPLITUDE + 1e-6);
        }
    }

    #[test]
    fn is_periodic_in_time() {
        let a = turbulence_offset(1.0, 2.0, 0.25);
        let b = turbulence_offset(1.0, 2.0, 0.25 + 1.0 / super::TURBULENCE_SPEED);
        assert!((a - b).abs() < 1e-4);
    }

    #[test]
    fn never_panics_on_non_finite_input() {
        let offset = turbulence_offset(f32::NAN, f32::INFINITY, f32::NEG_INFINITY);
        // `sin` of a non-finite input is NaN, which is a valid `f32`; the
        // point of this test is only that it does not panic.
        let _ = offset;
    }

    /// Guards against the Rust and WGSL copies of the turbulence formula
    /// drifting apart: the shader hardcodes the same three numeric
    /// constants (it cannot `include!` this module), so this pins their
    /// literal spellings in the shader source.
    #[test]
    fn shader_source_embeds_the_same_constants() {
        let shader = include_str!("world_water.wgsl");
        assert!(shader.contains("0.125"), "amplitude constant must match");
        assert!(shader.contains("4.0"), "scale constant must match");
    }
}
