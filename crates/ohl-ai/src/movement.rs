//! Movement glue: routes, a hull-traced step, and a stuck detector.
//!
//! This is deliberately the smallest thing that lets a schedule's movement
//! tasks do something real. [`Route`] is already the shape package 7.6's node
//! graph needs — an ordered waypoint list with a cursor — but today the only
//! constructor is the straight-line fallback. [`move_toward`] uses the same
//! `ohl-physics` clip-hull traces the player does, so a monster and the
//! player collide with the world through one code path.

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull};

/// How far the goal may drift before a route is rebuilt (published
/// behaviour: a route refreshes when the enemy moves more than 80 units).
pub const ROUTE_REFRESH_DISTANCE: f32 = 80.0;

/// The largest number of waypoints a route holds.
pub const MAX_WAYPOINTS: usize = 64;

/// The step height a monster can walk up without jumping.
///
/// The same 18-unit step the player uses; **provisional** for monsters,
/// which may differ per hull, and marked to be black-box observed.
pub const STEP_HEIGHT: f32 = 18.0;

/// How many consecutive ticks of no progress count as stuck.
///
/// **Provisional**: a quarter of a second at the 100 Hz simulation tick.
pub const STUCK_TICKS: u32 = 25;

/// The distance a tick must cover to count as progress.
pub const STUCK_EPSILON: f32 = 0.5;

/// How close counts as having arrived at a waypoint.
pub const WAYPOINT_TOLERANCE: f32 = 8.0;

/// An ordered list of waypoints with a cursor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Route {
    /// The waypoints, in order.
    pub waypoints: Vec<Vec3>,
    /// The index of the waypoint currently being moved to.
    pub current: usize,
    /// The final goal the route was built for, kept so
    /// [`Self::needs_refresh`] can notice it moved.
    pub goal: Vec3,
}

impl Route {
    /// An empty, finished route.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The straight-line fallback: a single waypoint at `goal`.
    ///
    /// Package 7.6 replaces this with an A* path over the node graph; every
    /// caller here already goes through [`Self::waypoint`], so nothing else
    /// changes when it does.
    #[must_use]
    pub fn straight_line(goal: Vec3) -> Self {
        Self {
            waypoints: vec![goal],
            current: 0,
            goal,
        }
    }

    /// A route through explicit waypoints, truncated to [`MAX_WAYPOINTS`].
    #[must_use]
    pub fn through(waypoints: &[Vec3], goal: Vec3) -> Self {
        Self {
            waypoints: waypoints.iter().take(MAX_WAYPOINTS).copied().collect(),
            current: 0,
            goal,
        }
    }

    /// The waypoint being moved to, or `None` when the route is finished.
    #[must_use]
    pub fn waypoint(&self) -> Option<Vec3> {
        self.waypoints.get(self.current).copied()
    }

    /// Whether every waypoint has been reached.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.current >= self.waypoints.len()
    }

    /// Advances the cursor past the current waypoint when `position` is
    /// within [`WAYPOINT_TOLERANCE`] of it, horizontally.
    ///
    /// Returns whether the cursor moved.
    pub fn advance_if_reached(&mut self, position: Vec3) -> bool {
        let Some(waypoint) = self.waypoint() else {
            return false;
        };
        let to_waypoint = Vec3::new(waypoint.x - position.x, waypoint.y - position.y, 0.0);
        if to_waypoint.length() <= WAYPOINT_TOLERANCE {
            self.current += 1;
            true
        } else {
            false
        }
    }

    /// Whether the route should be rebuilt because its goal moved more than
    /// [`ROUTE_REFRESH_DISTANCE`].
    #[must_use]
    pub fn needs_refresh(&self, goal: Vec3) -> bool {
        self.is_finished() || (goal - self.goal).length() > ROUTE_REFRESH_DISTANCE
    }
}

/// What one [`move_toward`] step did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveResult {
    /// Where the mover ended up.
    pub position: Vec3,
    /// How far it actually travelled.
    pub distance: f32,
    /// Whether something stopped it short of the requested step.
    pub blocked: bool,
    /// Whether it had to step up over an obstruction to get there.
    pub stepped_up: bool,
}

/// Moves `from` toward `target` by at most `speed * dt`, using clip-hull
/// traces, with a step up over obstructions no taller than [`STEP_HEIGHT`].
///
/// Only the horizontal component of the direction is used, so a monster
/// never flies at a target above it; the vertical part is the step-up and
/// the settle-down trace. With no usable input the mover simply stays put.
#[must_use]
pub fn move_toward(
    collision: &CollisionModel,
    hull: Hull,
    from: Vec3,
    target: Vec3,
    speed: f32,
    dt: f32,
) -> MoveResult {
    let still = MoveResult {
        position: from,
        distance: 0.0,
        blocked: false,
        stepped_up: false,
    };
    if !(from.is_finite() && target.is_finite() && speed.is_finite() && dt.is_finite()) {
        return still;
    }
    let step = speed * dt;
    if step <= 0.0 {
        return still;
    }

    let horizontal = Vec3::new(target.x - from.x, target.y - from.y, 0.0);
    let length = horizontal.length();
    if length <= f32::EPSILON {
        return still;
    }
    let direction = horizontal / length;
    let goal = from + direction * step.min(length);

    let flat = collision.trace(hull, from, goal);
    if !flat.blocked() && !flat.start_solid {
        return MoveResult {
            position: flat.end_pos,
            distance: (flat.end_pos - from).length(),
            blocked: false,
            stepped_up: false,
        };
    }

    // Blocked: try up, along, then down, the standard step-up sequence.
    let raised = collision.trace(hull, from, from + Vec3::Z * STEP_HEIGHT);
    let up = raised.end_pos;
    let along = collision.trace(hull, up, up + direction * step.min(length));
    let landing = collision.trace(hull, along.end_pos, along.end_pos - Vec3::Z * STEP_HEIGHT);
    let stepped = landing.end_pos;

    let flat_gain = (flat.end_pos - from).length();
    let step_gain = Vec3::new(stepped.x - from.x, stepped.y - from.y, 0.0).length();
    if step_gain > flat_gain + f32::EPSILON {
        MoveResult {
            position: stepped,
            distance: (stepped - from).length(),
            blocked: step_gain + WAYPOINT_TOLERANCE < step,
            stepped_up: true,
        }
    } else {
        MoveResult {
            position: flat.end_pos,
            distance: flat_gain,
            blocked: true,
            stepped_up: false,
        }
    }
}

/// Counts consecutive ticks in which a mover made no meaningful progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StuckDetector {
    ticks: u32,
}

impl StuckDetector {
    /// A detector that has seen no ticks.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one tick's travelled distance and returns whether the mover
    /// is now considered stuck.
    pub fn record(&mut self, distance: f32) -> bool {
        if distance.is_finite() && distance >= STUCK_EPSILON {
            self.ticks = 0;
        } else {
            self.ticks = self.ticks.saturating_add(1);
        }
        self.is_stuck()
    }

    /// Whether enough consecutive no-progress ticks have accumulated.
    #[must_use]
    pub const fn is_stuck(&self) -> bool {
        self.ticks >= STUCK_TICKS
    }

    /// The number of consecutive no-progress ticks.
    #[must_use]
    pub const fn ticks(&self) -> u32 {
        self.ticks
    }

    /// Forgets the accumulated ticks; call whenever movement restarts.
    pub fn reset(&mut self) {
        self.ticks = 0;
    }

    /// Rebuilds a detector with exactly `ticks` accumulated. Additive, for
    /// save-file restore (`.plan/m79-design.md` §6/§8 P4b).
    #[must_use]
    pub const fn from_ticks(ticks: u32) -> Self {
        Self { ticks }
    }
}

/// A yaw, in degrees, that points from `from` toward `to`.
///
/// Returns `None` when the two are vertically aligned.
#[must_use]
pub fn yaw_toward(from: Vec3, to: Vec3) -> Option<f32> {
    let delta = to - from;
    if !delta.is_finite() || delta.x.abs() + delta.y.abs() <= f32::EPSILON {
        return None;
    }
    Some(delta.y.atan2(delta.x).to_degrees())
}

/// Turns `yaw` at most `max_step` degrees toward `target_yaw`, the short way
/// around, and reports whether it arrived.
#[must_use]
pub fn turn_toward(yaw: f32, target_yaw: f32, max_step: f32) -> (f32, bool) {
    if !(yaw.is_finite() && target_yaw.is_finite() && max_step.is_finite()) {
        return (yaw, true);
    }
    let mut delta = (target_yaw - yaw) % 360.0;
    if delta > 180.0 {
        delta -= 360.0;
    } else if delta < -180.0 {
        delta += 360.0;
    }
    if delta.abs() <= max_step.abs() {
        (normalize_yaw(target_yaw), true)
    } else {
        (normalize_yaw(yaw + max_step.abs() * delta.signum()), false)
    }
}

/// Wraps a yaw into `[-180, 180)`.
#[must_use]
pub fn normalize_yaw(yaw: f32) -> f32 {
    if !yaw.is_finite() {
        return 0.0;
    }
    let mut wrapped = yaw % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    } else if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// The unit forward vector for a yaw in degrees.
#[must_use]
pub fn forward_from_yaw(yaw: f32) -> Vec3 {
    if !yaw.is_finite() {
        return Vec3::X;
    }
    let radians = yaw.to_radians();
    Vec3::new(radians.cos(), radians.sin(), 0.0)
}

#[cfg(test)]
mod tests {
    use super::{Route, StuckDetector, forward_from_yaw, normalize_yaw, turn_toward, yaw_toward};
    use glam::Vec3;

    #[test]
    fn a_straight_line_route_finishes_on_arrival() {
        let mut route = Route::straight_line(Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(route.waypoint(), Some(Vec3::new(100.0, 0.0, 0.0)));
        assert!(!route.advance_if_reached(Vec3::ZERO));
        assert!(!route.is_finished());
        assert!(route.advance_if_reached(Vec3::new(97.0, 0.0, 40.0)));
        assert!(route.is_finished());
        assert_eq!(route.waypoint(), None);
        assert!(!route.advance_if_reached(Vec3::ZERO));
    }

    #[test]
    fn a_route_refreshes_when_the_goal_drifts_far_enough() {
        let route = Route::straight_line(Vec3::ZERO);
        assert!(!route.needs_refresh(Vec3::new(79.0, 0.0, 0.0)));
        assert!(route.needs_refresh(Vec3::new(81.0, 0.0, 0.0)));
        assert!(Route::new().needs_refresh(Vec3::ZERO));
    }

    #[test]
    fn explicit_waypoints_are_bounded() {
        let many: Vec<Vec3> = (0..(super::MAX_WAYPOINTS + 16))
            .map(|i| Vec3::X * f32::from(u8::try_from(i % 251).unwrap_or(0)))
            .collect();
        let route = Route::through(&many, Vec3::ZERO);
        assert_eq!(route.waypoints.len(), super::MAX_WAYPOINTS);
    }

    #[test]
    fn the_stuck_detector_needs_consecutive_stalled_ticks() {
        let mut detector = StuckDetector::new();
        for _ in 0..(super::STUCK_TICKS - 1) {
            assert!(!detector.record(0.0));
        }
        assert!(detector.record(0.0));
        assert!(detector.is_stuck());
        assert!(!detector.record(10.0));
        assert_eq!(detector.ticks(), 0);
        detector.record(0.0);
        detector.reset();
        assert_eq!(detector.ticks(), 0);
        assert!(!detector.record(f32::NAN) || detector.ticks() == 1);
    }

    #[test]
    fn yaw_helpers_agree_with_each_other() {
        let yaw = yaw_toward(Vec3::ZERO, Vec3::new(0.0, 10.0, 0.0)).expect("not vertical");
        assert!((yaw - 90.0).abs() < 1e-3);
        assert!(yaw_toward(Vec3::ZERO, Vec3::Z).is_none());

        let forward = forward_from_yaw(yaw);
        assert!((forward - Vec3::Y).length() < 1e-5);

        let (turned, arrived) = turn_toward(0.0, 90.0, 30.0);
        assert!(!arrived);
        assert!((turned - 30.0).abs() < 1e-3);
        let (turned, arrived) = turn_toward(0.0, 10.0, 30.0);
        assert!(arrived);
        assert!((turned - 10.0).abs() < 1e-3);
        // The short way around is backwards through zero.
        let (turned, _) = turn_toward(170.0, -170.0, 5.0);
        assert!((turned - 175.0).abs() < 1e-3, "{turned}");
        assert!((normalize_yaw(540.0) - 180.0).abs() > 1e-3);
        assert!(normalize_yaw(f32::NAN).abs() < f32::EPSILON);
        assert!((turn_toward(f32::NAN, 0.0, 1.0).0).is_nan());
        assert!((forward_from_yaw(f32::INFINITY) - Vec3::X).length() < 1e-6);
    }
}
