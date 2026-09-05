//! A small, seeded, permuted congruential generator.
//!
//! Written from the public PCG paper — Melissa E. O'Neill, "PCG: A Family of
//! Simple Fast Space-Efficient Statistically Good Algorithms for Random
//! Number Generation", Harvey Mudd College technical report HMC-CS-2014-0905
//! (2014), <https://www.pcg-random.org/paper.html> — which specifies the
//! `PCG-XSH-RR 64/32` variant used here: a 64-bit LCG state advanced by
//! `state * 6364136223846793005 + increment`, then output-permuted by an
//! xorshift-high followed by a random rotate down to 32 bits.
//!
//! It exists so the AI tick has a deterministic, save-restorable random
//! stream with no third-party dependency at all. Nothing here is
//! cryptographic.

/// The multiplier the PCG paper specifies for the 64-bit LCG state.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// The default stream selector, used when a caller does not pick one.
const DEFAULT_STREAM: u64 = 1_442_695_040_888_963_407;

/// A `PCG-XSH-RR 64/32` generator.
///
/// Two generators seeded identically produce identical sequences on every
/// platform: all arithmetic is wrapping integer arithmetic on `u64`/`u32`,
/// with no floating point in the state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    /// Always odd, as the paper requires of the LCG increment.
    increment: u64,
}

impl Pcg32 {
    /// Seeds a generator on the default stream.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_stream(seed, DEFAULT_STREAM)
    }

    /// Seeds a generator on an explicit stream.
    ///
    /// The paper's seeding routine: start from zero, step once, add the
    /// seed, step again. `stream` is shifted left and forced odd to become
    /// the increment, so distinct streams never overlap.
    #[must_use]
    pub fn with_stream(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (stream << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(seed);
        rng.step();
        rng
    }

    /// Restores a generator from a previously saved [`Self::snapshot`].
    #[must_use]
    pub fn from_snapshot(snapshot: (u64, u64)) -> Self {
        Self {
            state: snapshot.0,
            increment: snapshot.1 | 1,
        }
    }

    /// The raw state, for save files and determinism hashes.
    #[must_use]
    pub fn snapshot(self) -> (u64, u64) {
        (self.state, self.increment)
    }

    fn step(&mut self) {
        self.state = self
            .state
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(self.increment);
    }

    /// The next 32-bit output.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        // XSH: xorshift the high bits down, then RR: rotate right by the
        // top five bits of the old state.
        #[allow(clippy::cast_possible_truncation)]
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let rotation = (old >> 59) as u32;
        xorshifted.rotate_right(rotation)
    }

    /// A uniform value in `0..bound`, or `0` when `bound` is zero.
    ///
    /// Uses the paper's rejection bound so the result is unbiased.
    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_u32();
            if value >= threshold {
                return value % bound;
            }
        }
    }

    /// A uniform `f32` in `[0, 1)`, built from the top 24 output bits so the
    /// result is exactly representable.
    pub fn next_f32(&mut self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let mantissa = (self.next_u32() >> 8) as f32;
        mantissa / f32::from(1u16 << 8) / 65_536.0
    }

    /// A uniform `f32` in `[low, high)`, or `low` when the range is empty or
    /// not finite.
    pub fn range_f32(&mut self, low: f32, high: f32) -> f32 {
        if !(low.is_finite() && high.is_finite()) || high <= low {
            return low;
        }
        low + (high - low) * self.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::Pcg32;

    #[test]
    fn the_same_seed_replays_the_same_sequence() {
        let mut a = Pcg32::new(0x0BAD_F00D);
        let mut b = Pcg32::new(0x0BAD_F00D);
        for _ in 0..1_000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Pcg32::new(1);
        let mut b = Pcg32::new(2);
        let differing = (0..64).filter(|_| a.next_u32() != b.next_u32()).count();
        assert!(differing > 60, "only {differing} of 64 outputs differ");
    }

    #[test]
    fn a_snapshot_round_trips() {
        let mut rng = Pcg32::with_stream(7, 9);
        for _ in 0..10 {
            rng.next_u32();
        }
        let mut restored = Pcg32::from_snapshot(rng.snapshot());
        assert_eq!(rng.next_u32(), restored.next_u32());
    }

    #[test]
    fn bounded_values_stay_in_range() {
        let mut rng = Pcg32::new(42);
        for bound in 1u32..64 {
            for _ in 0..32 {
                assert!(rng.below(bound) < bound);
            }
        }
        assert_eq!(rng.below(0), 0);
    }

    #[test]
    fn floats_stay_in_the_unit_interval() {
        let mut rng = Pcg32::new(11);
        for _ in 0..4_096 {
            let value = rng.next_f32();
            assert!((0.0..1.0).contains(&value), "{value} left [0, 1)");
        }
        assert!((rng.range_f32(5.0, 5.0) - 5.0).abs() < f32::EPSILON);
        assert!((rng.range_f32(5.0, 1.0) - 5.0).abs() < f32::EPSILON);
        let value = rng.range_f32(2.0, 4.0);
        assert!((2.0..4.0).contains(&value));
    }
}
