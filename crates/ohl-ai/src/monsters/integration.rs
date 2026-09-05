//! Forward-declared seams for packages 7.6 (`ohl-nav`, node graph/A*/
//! steering) and 7.3 (`ohl-combat`, projectiles/explosions), which are being
//! built concurrently with this one.
//!
//! `ohl-ai` cannot depend on either crate yet (`xtask/src/graph.rs` does not
//! list that edge), and this package must not touch `ohl-nav` or
//! `ohl-combat`'s own files. So rather than block on them, this module
//! defines the two minimal traits package 7.7's monster brains need from
//! them, each with a working, dependency-free default implementation
//! (straight-line movement, a no-op attack sink) so the crate is usable
//! today. When 7.6/7.3 land, a real implementation of these same traits
//! (kept deliberately small) is dropped in at the composition root without
//! this crate's data — `MonsterSpec`, `MonsterBrain`, the schedules — having
//! to change at all.

use glam::Vec3;

/// What a monster's ranged attack actually needs from pathfinding: one step
/// toward a goal. A real implementation (package 7.6) would query the node
/// graph and run A*/steering over it; [`StraightLineNavigator`] just walks
/// the straight line, exactly like [`crate::movement::move_toward`]'s own
/// no-collision-data fallback, so a monster is never left unable to move
/// before the real navigator exists.
pub trait Navigator {
    /// The next position to move toward, one step closer to `goal` from
    /// `origin`, without exceeding `max_step` world units of travel.
    fn next_move(&self, origin: Vec3, goal: Vec3, max_step: f32) -> Vec3;
}

/// A dependency-free [`Navigator`] that walks straight at the goal, ignoring
/// obstacles. The same shape `crate::movement`'s existing straight-line
/// fallback already uses, exposed here as a trait object so a caller can
/// substitute the real node-graph navigator later without this crate's
/// schedules or brains changing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StraightLineNavigator;

impl Navigator for StraightLineNavigator {
    fn next_move(&self, origin: Vec3, goal: Vec3, max_step: f32) -> Vec3 {
        let delta = Vec3::new(goal.x - origin.x, goal.y - origin.y, 0.0);
        let length = delta.length();
        if length <= f32::EPSILON || max_step <= 0.0 {
            return origin;
        }
        let travel = max_step.min(length);
        origin + delta / length * travel
    }
}

/// What a monster's ranged attack needs from the projectile/explosion
/// system: somewhere to hand off "spawn this kind of shot, from here, in
/// this direction" once it has decided to fire (see
/// `crate::schedule::Task::RangeAttack1`/`RangeAttack2`, executed today as
/// just an `AiEventKind::Attack` event by `AiWorld`'s task executor). A real
/// implementation (package 7.3) would spawn an actual projectile/explosion
/// entity; [`NoOpRangedAttackSink`] does nothing, so a monster's schedule
/// still runs to completion before that entity exists.
pub trait RangedAttackSink {
    /// Spawns (or would spawn) one shot of `kind`, a project-owned tag
    /// naming which attack this is (matching, for example, a
    /// [`super::table::MonsterKind`]'s attack name), from `origin` toward
    /// unit vector `dir`.
    fn spawn(&mut self, kind: &str, origin: Vec3, dir: Vec3);
}

/// A [`RangedAttackSink`] that drops every shot on the floor. The default
/// until package 7.3 lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoOpRangedAttackSink;

impl RangedAttackSink for NoOpRangedAttackSink {
    fn spawn(&mut self, _kind: &str, _origin: Vec3, _dir: Vec3) {}
}

#[cfg(test)]
mod tests {
    use super::{Navigator, NoOpRangedAttackSink, RangedAttackSink, StraightLineNavigator};
    use glam::Vec3;

    #[test]
    fn the_straight_line_navigator_takes_a_bounded_step_toward_the_goal() {
        let navigator = StraightLineNavigator;
        let next = navigator.next_move(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0), 10.0);
        assert!((next.x - 10.0).abs() < 1e-4);
        assert!((next.y).abs() < 1e-4);

        // A step larger than the remaining distance lands exactly on the
        // goal rather than overshooting.
        let arrived = navigator.next_move(Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0), 100.0);
        assert!((arrived.x - 5.0).abs() < 1e-4);
    }

    #[test]
    fn a_zero_step_or_coincident_goal_does_not_move() {
        let navigator = StraightLineNavigator;
        let stayed = navigator.next_move(Vec3::ONE, Vec3::new(50.0, 0.0, 0.0), 0.0);
        assert_eq!(stayed, Vec3::ONE);
        let same_spot = navigator.next_move(Vec3::ONE, Vec3::ONE, 10.0);
        assert_eq!(same_spot, Vec3::ONE);
    }

    #[test]
    fn the_no_op_sink_accepts_every_call_and_spawns_nothing_observable() {
        let mut sink = NoOpRangedAttackSink;
        sink.spawn("houndeye_blast", Vec3::ZERO, Vec3::X);
        // Nothing to assert beyond "did not panic": the whole point of the
        // no-op default is that it is indistinguishable from doing nothing.
    }
}
