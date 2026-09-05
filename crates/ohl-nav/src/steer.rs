//! Local steering: turning a [`Path`] into a per-tick movement intent.
//!
//! The graph gets a monster roughly to its goal; steering does the last few
//! units, keeps it off walls between two waypoints, and notices when it has
//! stopped making progress. The output is an *intent*, not a move: the
//! caller feeds `dir` and `speed_scale` into its own movement code (in this
//! project `ohl_physics::player_move`), so monsters and the player keep
//! sharing one collision path.
//!
//! All defaults in [`SteerLimits`] are project choices.

use glam::Vec3;
use ohl_physics::{CollisionModel, Hull};

use crate::path::Path;

/// What the mover should do this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveIntent {
    /// Unit direction to move in, or zero when the path is finished or
    /// unusable.
    pub dir: Vec3,
    /// Fraction of the mover's full speed to use, in `0.0..=1.0`. It drops
    /// when steering had to slide along or turn away from an obstacle.
    pub speed_scale: f32,
    /// The final waypoint has been reached.
    pub reached: bool,
    /// Every probe was obstructed this tick; `dir` is the fallback.
    pub blocked: bool,
}

impl MoveIntent {
    /// A "nothing to do" intent.
    #[must_use]
    pub const fn idle(reached: bool) -> Self {
        Self {
            dir: Vec3::ZERO,
            speed_scale: 0.0,
            reached,
            blocked: false,
        }
    }
}

/// Bounds and tolerances for [`Steer::next_move`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SteerLimits {
    /// How close counts as standing on a waypoint. Default 24 units, less
    /// than one humanoid hull width so waypoints are not skipped.
    pub arrive_radius: f32,
    /// How far ahead each probe trace looks. Default 48 units.
    pub probe_distance: f32,
    /// How far left and right the two side probes are rotated, in degrees.
    /// Default 40.
    pub probe_angle_degrees: f32,
    /// How many ticks of movement are measured before progress is judged.
    /// Default 20 (0.2 s at the project's 100 Hz tick).
    pub stuck_window_ticks: u32,
    /// How far the mover must travel within one window to count as making
    /// progress. Default 8 units.
    pub min_window_progress: f32,
    /// How many waypoints ahead the cursor may skip to in one tick when the
    /// next one is both closer and directly traceable, so a mover that has
    /// drifted past a waypoint walks on instead of doubling back. Default 2.
    pub max_skip_probes: u32,
}

impl Default for SteerLimits {
    fn default() -> Self {
        Self {
            arrive_radius: 24.0,
            probe_distance: 48.0,
            probe_angle_degrees: 40.0,
            stuck_window_ticks: 20,
            min_window_progress: 8.0,
            max_skip_probes: 2,
        }
    }
}

/// Per-mover steering state: the waypoint cursor and the progress window.
///
/// One `Steer` follows one [`Path`]; call [`Steer::reset`] when the path is
/// replaced.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Steer {
    cursor: usize,
    ticks: u32,
    window_start: Vec3,
    window_open: bool,
    stuck: bool,
}

impl Steer {
    /// A fresh steering state at waypoint 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the cursor and the progress window.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The waypoint currently being steered toward.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the last progress window saw less than
    /// [`SteerLimits::min_window_progress`] of movement.
    ///
    /// This is the documented fallback contract: while stuck, `next_move`
    /// keeps returning a sidestep (the horizontal perpendicular of the
    /// desired direction, sign chosen by the clearer side probe) so a mover
    /// wedged on a corner shakes loose on its own, and the caller should
    /// request a fresh path — steering alone cannot route around a
    /// obstruction it cannot see past.
    #[must_use]
    pub const fn is_stuck(&self) -> bool {
        self.stuck
    }

    /// The intent for this tick.
    ///
    /// Advances the cursor past every waypoint already within
    /// [`SteerLimits::arrive_radius`], probes forward, slides along the
    /// blocking plane or turns to the clearer side when the way ahead is
    /// obstructed, and updates the stuck window.
    pub fn next_move(
        &mut self,
        pos: Vec3,
        path: &Path,
        hull: Hull,
        collision: &CollisionModel,
        limits: &SteerLimits,
    ) -> MoveIntent {
        self.update_progress(pos, limits);
        if !pos.is_finite() || path.waypoints.is_empty() {
            return MoveIntent::idle(false);
        }
        let last = path.waypoints.len() - 1;
        self.cursor = self.cursor.min(last);
        let arrive_squared = limits.arrive_radius * limits.arrive_radius;
        // Skip ahead to a waypoint that is both nearer and directly
        // traceable: the mover may have drifted past the current one, and
        // walking back to it would look (and path) badly.
        for _ in 0..limits.max_skip_probes {
            if self.cursor >= last {
                break;
            }
            let (Some(current), Some(next)) = (
                path.waypoints.get(self.cursor),
                path.waypoints.get(self.cursor + 1),
            ) else {
                break;
            };
            if (*next - pos).length_squared() >= (*current - pos).length_squared()
                || collision.trace(hull, pos, *next).blocked()
            {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor < last {
            let Some(target) = path.waypoints.get(self.cursor) else {
                break;
            };
            if (*target - pos).length_squared() > arrive_squared {
                break;
            }
            self.cursor += 1;
        }

        let Some(target) = path.waypoints.get(self.cursor).copied() else {
            return MoveIntent::idle(false);
        };
        if self.cursor == last && (target - pos).length_squared() <= arrive_squared {
            return MoveIntent::idle(true);
        }

        let Some(desired) = travel_dir(pos, target, hull) else {
            return MoveIntent::idle(false);
        };
        if self.stuck {
            let side = clearer_side(pos, desired, hull, collision, limits);
            return MoveIntent {
                dir: side,
                speed_scale: 0.5,
                reached: false,
                blocked: true,
            };
        }
        probe(pos, desired, hull, collision, limits)
    }

    /// Closes the progress window every `stuck_window_ticks` ticks and sets
    /// or clears [`Steer::is_stuck`].
    fn update_progress(&mut self, pos: Vec3, limits: &SteerLimits) {
        if !self.window_open {
            self.window_start = pos;
            self.window_open = true;
            self.ticks = 0;
            return;
        }
        self.ticks = self.ticks.saturating_add(1);
        if self.ticks < limits.stuck_window_ticks.max(1) {
            return;
        }
        let travelled = (pos - self.window_start).length();
        self.stuck = travelled < limits.min_window_progress;
        self.window_start = pos;
        self.ticks = 0;
    }
}

/// Forward probe, then a slide along the blocking plane, then the two
/// rotated side probes, then the fallback.
fn probe(
    pos: Vec3,
    desired: Vec3,
    hull: Hull,
    collision: &CollisionModel,
    limits: &SteerLimits,
) -> MoveIntent {
    let reach = limits.probe_distance.max(1.0);
    let forward = collision.trace(hull, pos, pos + desired * reach);
    if !forward.blocked() {
        return MoveIntent {
            dir: desired,
            speed_scale: 1.0,
            reached: false,
            blocked: false,
        };
    }

    if !forward.start_solid && forward.plane_normal.length_squared() > 0.0 {
        let slide = desired - forward.plane_normal * desired.dot(forward.plane_normal);
        if let Some(slide) = normalize_horizontal(slide, hull) {
            let trace = collision.trace(hull, pos, pos + slide * reach);
            if !trace.blocked() {
                return MoveIntent {
                    dir: slide,
                    speed_scale: 0.7,
                    reached: false,
                    blocked: false,
                };
            }
        }
    }

    let mut best: Option<(f32, Vec3)> = None;
    for sign in [1.0f32, -1.0] {
        let Some(candidate) = normalize_horizontal(
            rotate_z(desired, limits.probe_angle_degrees.to_radians() * sign),
            hull,
        ) else {
            continue;
        };
        let trace = collision.trace(hull, pos, pos + candidate * reach);
        let score = if trace.start_solid {
            0.0
        } else {
            trace.fraction
        };
        // `>` keeps the first (left) candidate on a tie, so steering is
        // deterministic.
        if best.is_none_or(|(previous, _)| score > previous) {
            best = Some((score, candidate));
        }
    }

    match best {
        Some((score, dir)) if score >= 1.0 => MoveIntent {
            dir,
            speed_scale: 0.5,
            reached: false,
            blocked: false,
        },
        // Fallback: nothing is clear, so creep toward the least
        // obstructed probe at a quarter speed and let the stuck window
        // decide whether a new path is needed.
        Some((_, dir)) => MoveIntent {
            dir,
            speed_scale: 0.25,
            reached: false,
            blocked: true,
        },
        None => MoveIntent {
            dir: Vec3::ZERO,
            speed_scale: 0.0,
            reached: false,
            blocked: true,
        },
    }
}

/// The horizontal perpendicular of `desired` on whichever side traces
/// further.
fn clearer_side(
    pos: Vec3,
    desired: Vec3,
    hull: Hull,
    collision: &CollisionModel,
    limits: &SteerLimits,
) -> Vec3 {
    let reach = limits.probe_distance.max(1.0);
    let left = rotate_z(desired, std::f32::consts::FRAC_PI_2);
    let right = -left;
    let left_fraction = collision.trace(hull, pos, pos + left * reach).fraction;
    let right_fraction = collision.trace(hull, pos, pos + right * reach).fraction;
    if right_fraction > left_fraction {
        right
    } else {
        left
    }
}

/// The unit direction from `pos` to `target`, flattened for hulls that walk.
fn travel_dir(pos: Vec3, target: Vec3, hull: Hull) -> Option<Vec3> {
    normalize_horizontal(target - pos, hull)
}

/// Normalizes `vector`, dropping the vertical component for the box hulls
/// (walkers and swimmers steer horizontally; the point hull, used by fliers,
/// keeps its full 3D direction).
fn normalize_horizontal(vector: Vec3, hull: Hull) -> Option<Vec3> {
    let vector = if matches!(hull, Hull::Point) {
        vector
    } else {
        Vec3::new(vector.x, vector.y, 0.0)
    };
    if !vector.is_finite() || vector.length_squared() <= f32::EPSILON {
        return None;
    }
    Some(vector.normalize())
}

/// Rotates `vector` about the world Z axis by `radians`.
fn rotate_z(vector: Vec3, radians: f32) -> Vec3 {
    let (sin, cos) = radians.sin_cos();
    Vec3::new(
        vector.x * cos - vector.y * sin,
        vector.x * sin + vector.y * cos,
        vector.z,
    )
}
