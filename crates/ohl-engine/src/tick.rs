//! The engine's fixed timestep.
//!
//! Every subsystem this crate composes is advanced at one rate, so a frame's
//! wall-clock duration never changes what the simulation does. [`TickClock`]
//! turns a variable frame time into whole [`TICK_SECONDS`] steps and carries
//! the remainder, and clamps the burst at [`MAX_TICKS_PER_FRAME`] so a
//! stalled frame cannot make one call run unbounded work.
//!
//! Both constants are re-exported from [`ohl_physics::controller`] rather
//! than restated: the player controller already sub-steps at that rate, and
//! the two must not be able to drift apart.

/// One simulation step, in seconds.
pub const TICK_SECONDS: f32 = ohl_physics::controller::TICK_SECONDS;

/// The largest number of steps one [`crate::Game::tick`] call runs.
pub const MAX_TICKS_PER_FRAME: u32 = ohl_physics::controller::MAX_TICKS_PER_FRAME;

/// Accumulates frame time and yields whole fixed steps, carrying the
/// remainder.
///
/// The clock is the only thing that decides how many times a frame steps the
/// simulation, so two runs that deliver the same total time in different
/// frame sizes step the same number of times (as long as neither run trips
/// the [`MAX_TICKS_PER_FRAME`] clamp).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TickClock {
    /// Frame time accumulated but not yet released as a whole step. Always
    /// finite and in `0.0..TICK_SECONDS` after [`TickClock::steps`] returns.
    leftover: f32,
}

impl TickClock {
    /// A clock with nothing banked.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many whole steps `dt` releases, banking the remainder.
    ///
    /// A non-finite or non-positive `dt` releases nothing and changes
    /// nothing. When the count would exceed [`MAX_TICKS_PER_FRAME`] the
    /// backlog is dropped rather than banked, so a long stall costs one
    /// clamped frame instead of an ever-growing catch-up.
    pub fn steps(&mut self, dt: f32) -> u32 {
        if !dt.is_finite() || dt <= 0.0 {
            return 0;
        }
        self.leftover += dt;
        if !self.leftover.is_finite() {
            self.leftover = 0.0;
            return MAX_TICKS_PER_FRAME;
        }
        let mut steps = 0;
        while self.leftover >= TICK_SECONDS && steps < MAX_TICKS_PER_FRAME {
            self.leftover -= TICK_SECONDS;
            steps += 1;
        }
        if steps == MAX_TICKS_PER_FRAME {
            // Drop the backlog instead of catching up forever, matching
            // `ohl_physics::PlayerController::advance`.
            self.leftover = 0.0;
        }
        steps
    }

    /// How far into the next step the banked remainder stands, in `0..1`.
    ///
    /// A renderer may use this to interpolate between the last two
    /// simulated states; nothing in this crate requires it yet.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        (self.leftover / TICK_SECONDS).clamp(0.0, 1.0)
    }

    /// The banked remainder, in seconds.
    #[must_use]
    pub fn leftover(&self) -> f32 {
        self.leftover
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_TICKS_PER_FRAME, TICK_SECONDS, TickClock};

    #[test]
    fn a_whole_multiple_releases_exactly_that_many_steps() {
        let mut clock = TickClock::new();
        assert_eq!(clock.steps(TICK_SECONDS), 1);
        assert!(clock.leftover() < TICK_SECONDS);
        assert_eq!(clock.steps(TICK_SECONDS * 3.0), 3);
    }

    #[test]
    fn a_partial_step_banks_rather_than_rounding() {
        let mut clock = TickClock::new();
        assert_eq!(clock.steps(TICK_SECONDS * 0.5), 0);
        assert_eq!(
            clock.steps(TICK_SECONDS * 0.5),
            1,
            "the two halves add up to one whole step"
        );
    }

    #[test]
    fn the_clamp_drops_the_backlog_instead_of_banking_it() {
        let mut clock = TickClock::new();
        let overlong = TICK_SECONDS * f32::from(u16::try_from(MAX_TICKS_PER_FRAME).unwrap()) * 10.0;
        assert_eq!(clock.steps(overlong), MAX_TICKS_PER_FRAME);
        assert!(
            clock.leftover() <= f32::EPSILON,
            "an overlong frame leaves nothing banked"
        );
        assert_eq!(clock.steps(0.0), 0);
    }

    #[test]
    fn a_non_finite_or_negative_frame_releases_nothing() {
        let mut clock = TickClock::new();
        for dt in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0] {
            assert_eq!(clock.steps(dt), 0);
            assert!(clock.leftover().is_finite());
        }
    }

    #[test]
    fn alpha_stays_inside_the_unit_interval() {
        let mut clock = TickClock::new();
        clock.steps(TICK_SECONDS * 0.25);
        let alpha = clock.alpha();
        assert!((0.0..=1.0).contains(&alpha));
    }

    #[test]
    fn the_same_total_time_releases_the_same_step_count() {
        let mut coarse = TickClock::new();
        let mut fine = TickClock::new();
        let mut coarse_steps = 0;
        let mut fine_steps = 0;
        for _ in 0..10 {
            coarse_steps += coarse.steps(TICK_SECONDS * 10.0);
        }
        for _ in 0..100 {
            fine_steps += fine.steps(TICK_SECONDS);
        }
        assert_eq!(coarse_steps, fine_steps);
    }
}
