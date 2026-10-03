//! `monster_apache` and `monster_osprey`: flight along a `path_corner`
//! route.
//!
//! ## What is published (see `docs/FORMAT_SOURCES.md`, "Monster
//! definitions", "Wave 1 batch B")
//!
//! - `TWHL:Monster_apache`: its `target` is "The Apache's first
//!   `path_corner`", and the `path_corner`s form "a cyclic route"; spawnflag
//!   `Start Inactive` (64): "Must be triggered to start".
//! - `TWHL:Monster_osprey`: "_target_ needs to point to a `path_corner`
//!   which targets other `path_corners` forming a cyclic route"; of its
//!   spawnflags only `Start Inactive (64)` works, which "Requires the Osprey
//!   to be triggered to start".
//!
//! ## This project's reading
//!
//! A [`FlightPlan`] is the route's node positions and waits. It does not
//! move the aircraft itself: every tick [`crate::monsters::bosses`] asks it
//! for a [`FlightOrder`] and turns that into the same route and move speed
//! a path task would set, so the aircraft is moved by the AI's ordinary
//! movement step. Both aircraft are on the point hull, which is what makes
//! that step fly them — the full three-dimensional line, stopped by what
//! the hull's own trace says is solid (`crate::movement::flies`, the seam
//! the alien controller already flies on) — and turn them toward where
//! they are going. A route the nav bridge can find goes through the air
//! subgraph; otherwise the bridge's traced fallback flies the straight
//! line.
//!
//! `TODO(black-box)`: no page gives a flight speed ([`FLIGHT_SPEED`]) or
//! says how close counts as reaching a node ([`FLIGHT_ARRIVAL_RADIUS`]).
//! The published "angles of the `path_corner`s are used to orient the
//! Apache" is not modelled: `ohl_game::PathChain` does not carry node
//! angles, so the airframe faces its direction of travel. Whether a second
//! `use` stops an aircraft again is not published; a `use` here only ever
//! starts one ([`FlightPlan::activate`]).

use glam::Vec3;
use ohl_game::PathChain;

/// The flight speed, in units per second. **`TODO(black-box)`**: not
/// published.
pub const FLIGHT_SPEED: f32 = 400.0;

/// How close (in three dimensions, world units) counts as reaching a route
/// node. **`TODO(black-box)`**: not published. Kept above
/// `crate::movement::WAYPOINT_TOLERANCE`, so the movement step's own route
/// never finishes short of what this plan counts as arriving.
pub const FLIGHT_ARRIVAL_RADIUS: f32 = 32.0;

/// The most route nodes a flight plan holds.
pub const MAX_FLIGHT_NODES: usize = 64;

/// What the aircraft should do this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlightOrder {
    /// Hover where it is: not started, waiting out a node's `wait`, or at
    /// the end of a route that does not close.
    Hold,
    /// Fly toward this point.
    FlyTo(Vec3),
}

/// What one [`FlightPlan::steer`] call decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightStep {
    /// What to do this tick.
    pub order: FlightOrder,
    /// The route node reached this tick, if any.
    pub arrived_at: Option<usize>,
}

/// Where a [`FlightPlan`] is along its route: the part of it that changes
/// at runtime (the route itself is rebuilt from the map on every load), in
/// the form a save file holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightProgress {
    /// The index of the node being flown to.
    pub current: usize,
    /// Whether the plan has been started.
    pub active: bool,
    /// Seconds of the reached node's `wait` still to hover out.
    pub wait_left: f32,
}

/// A route of positions to fly through, with per-node waits: a component
/// on a `monster_apache` or `monster_osprey`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightPlan {
    waypoints: Vec<Vec3>,
    waits: Vec<f32>,
    looped: bool,
    current: usize,
    speed: f32,
    active: bool,
    wait_left: f32,
}

impl FlightPlan {
    /// A started plan through `waypoints` (truncated to
    /// [`MAX_FLIGHT_NODES`]) with no waits, looping back to the first when
    /// `looped`, at `speed`.
    #[must_use]
    pub fn new(waypoints: Vec<Vec3>, looped: bool, speed: f32) -> Self {
        let mut waypoints = waypoints;
        waypoints.truncate(MAX_FLIGHT_NODES);
        let waits = vec![0.0; waypoints.len()];
        Self {
            waypoints,
            waits,
            looped,
            current: 0,
            speed: if speed.is_finite() && speed > 0.0 {
                speed
            } else {
                FLIGHT_SPEED
            },
            active: true,
            wait_left: 0.0,
        }
    }

    /// A plan over a resolved `path_corner` chain: its node positions,
    /// each node's `wait`, and whether it closes into a loop.
    #[must_use]
    pub fn from_chain(chain: &PathChain, speed: f32) -> Self {
        let mut plan = Self::new(
            chain.nodes.iter().map(|node| node.position).collect(),
            chain.looped,
            speed,
        );
        plan.waits = chain
            .nodes
            .iter()
            .take(plan.waypoints.len())
            .map(|node| {
                if node.wait.is_finite() {
                    node.wait.max(0.0)
                } else {
                    0.0
                }
            })
            .collect();
        plan
    }

    /// The same plan, parked until [`Self::activate`] (the published `Start
    /// Inactive` spawnflag).
    #[must_use]
    pub fn starting_inactive(mut self) -> Self {
        self.active = false;
        self
    }

    /// Starts flying (a `use` reaching a `Start Inactive` aircraft).
    pub fn activate(&mut self) {
        self.active = true;
    }

    /// Whether the plan has been started.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The flight speed, in units per second.
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// The route positions.
    #[must_use]
    pub fn waypoints(&self) -> &[Vec3] {
        &self.waypoints
    }

    /// Each route node's `wait`, in seconds.
    #[must_use]
    pub fn waits(&self) -> &[f32] {
        &self.waits
    }

    /// Whether the route closes into a loop.
    #[must_use]
    pub fn is_looped(&self) -> bool {
        self.looped
    }

    /// The node being flown to, or `None` when the route is empty or a
    /// non-looped route has been flown to its end.
    #[must_use]
    pub fn destination(&self) -> Option<Vec3> {
        self.waypoints.get(self.current).copied()
    }

    /// Whether a non-looped route has been flown to its end.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        !self.looped && self.current >= self.waypoints.len()
    }

    /// Decides this tick's flight from `origin`: hover out the wait at the
    /// node just reached; else, when within [`FLIGHT_ARRIVAL_RADIUS`] of
    /// the node being flown to, count it reached (starting its wait and
    /// moving on to the next node, or back to the first on a loop); else
    /// fly at the node.
    pub fn steer(&mut self, origin: Vec3, dt: f32) -> FlightStep {
        let hold = FlightStep {
            order: FlightOrder::Hold,
            arrived_at: None,
        };
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        if !self.active || !origin.is_finite() {
            return hold;
        }
        if self.wait_left > 0.0 {
            self.wait_left = (self.wait_left - dt).max(0.0);
            if self.wait_left > 0.0 {
                return hold;
            }
        }
        let Some(destination) = self.destination() else {
            return hold;
        };
        if (destination - origin).length() > FLIGHT_ARRIVAL_RADIUS {
            return FlightStep {
                order: FlightOrder::FlyTo(destination),
                arrived_at: None,
            };
        }
        let reached = self.current;
        self.wait_left = self.waits.get(reached).copied().unwrap_or(0.0);
        self.current = if self.current + 1 < self.waypoints.len() {
            self.current + 1
        } else if self.looped {
            0
        } else {
            self.waypoints.len()
        };
        let order = match self.destination() {
            Some(next) if self.wait_left <= 0.0 => FlightOrder::FlyTo(next),
            _ => FlightOrder::Hold,
        };
        FlightStep {
            order,
            arrived_at: Some(reached),
        }
    }

    /// Where the plan is along its route, for a save.
    #[must_use]
    pub fn progress(&self) -> FlightProgress {
        FlightProgress {
            current: self.current,
            active: self.active,
            wait_left: self.wait_left,
        }
    }

    /// Puts the plan back where `progress` says. A node index past the
    /// route (a save from a map whose route has since shrunk) is clamped to
    /// the route's end, and a bad wait to none.
    pub fn restore(&mut self, progress: FlightProgress) {
        self.current = progress.current.min(self.waypoints.len());
        self.active = progress.active;
        self.wait_left = if progress.wait_left.is_finite() {
            progress.wait_left.max(0.0)
        } else {
            0.0
        };
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FLIGHT_ARRIVAL_RADIUS, FLIGHT_SPEED, FlightOrder, FlightPlan, FlightProgress,
        MAX_FLIGHT_NODES,
    };
    use glam::Vec3;

    fn square() -> Vec<Vec3> {
        vec![
            Vec3::new(0.0, 0.0, 512.0),
            Vec3::new(1_000.0, 0.0, 512.0),
            Vec3::new(1_000.0, 1_000.0, 640.0),
            Vec3::new(0.0, 1_000.0, 640.0),
        ]
    }

    /// Flies `plan` from `origin` with a stand-in for the movement step:
    /// a straight three-dimensional step at the plan's speed.
    fn fly(plan: &mut FlightPlan, mut origin: Vec3, ticks: usize, dt: f32) -> (Vec3, Vec<usize>) {
        let mut arrivals = Vec::new();
        for _ in 0..ticks {
            let step = plan.steer(origin, dt);
            if let Some(index) = step.arrived_at {
                arrivals.push(index);
            }
            if let FlightOrder::FlyTo(to) = step.order {
                let delta = to - origin;
                let travel = (plan.speed() * dt).min(delta.length());
                origin += delta.normalize_or_zero() * travel;
            }
        }
        (origin, arrivals)
    }

    #[test]
    fn a_looped_plan_flies_node_to_node_in_three_dimensions_and_wraps() {
        let mut plan = FlightPlan::new(square(), true, 500.0);
        let (origin, arrivals) = fly(&mut plan, Vec3::new(-2_000.0, 0.0, 0.0), 3_000, 0.01);
        assert!(
            arrivals.len() >= 5,
            "wrapped past the last node: {arrivals:?}"
        );
        assert_eq!(&arrivals[..5], &[0, 1, 2, 3, 0]);
        assert!(!plan.is_finished());
        // It climbed to the higher nodes rather than staying at z = 0.
        assert!(origin.z > 400.0);
    }

    #[test]
    fn a_non_looped_plan_holds_at_its_last_node() {
        let mut plan = FlightPlan::new(vec![Vec3::new(50.0, 0.0, 0.0)], false, 100.0);
        assert_eq!(
            plan.steer(Vec3::ZERO, 0.01).order,
            FlightOrder::FlyTo(Vec3::new(50.0, 0.0, 0.0))
        );
        let step = plan.steer(
            Vec3::new(50.0 - FLIGHT_ARRIVAL_RADIUS * 0.5, 0.0, 0.0),
            0.01,
        );
        assert_eq!(step.arrived_at, Some(0));
        assert_eq!(step.order, FlightOrder::Hold);
        assert!(plan.is_finished());
        assert_eq!(plan.destination(), None);
        let after = plan.steer(Vec3::ZERO, 1.0);
        assert_eq!(after.order, FlightOrder::Hold);
        assert_eq!(after.arrived_at, None);
    }

    /// Arrival is three-dimensional: a node straight overhead is not
    /// reached until the aircraft has climbed to it.
    #[test]
    fn a_node_overhead_is_reached_only_at_its_height() {
        let mut plan = FlightPlan::new(vec![Vec3::new(0.0, 0.0, 500.0)], false, 100.0);
        let below = plan.steer(Vec3::ZERO, 0.01);
        assert_eq!(below.arrived_at, None);
        assert_eq!(below.order, FlightOrder::FlyTo(Vec3::new(0.0, 0.0, 500.0)));
        assert_eq!(
            plan.steer(Vec3::new(0.0, 0.0, 490.0), 0.01).arrived_at,
            Some(0)
        );
    }

    #[test]
    fn an_inactive_plan_holds_until_activated() {
        let mut plan = FlightPlan::new(square(), true, FLIGHT_SPEED).starting_inactive();
        assert!(!plan.is_active());
        for _ in 0..100 {
            assert_eq!(plan.steer(Vec3::ZERO, 0.1).order, FlightOrder::Hold);
        }
        plan.activate();
        assert!(plan.is_active());
        assert_eq!(
            plan.steer(Vec3::ZERO, 0.1).order,
            FlightOrder::FlyTo(square()[0])
        );
    }

    #[test]
    fn a_node_wait_holds_the_aircraft_for_that_long() {
        let mut plan = FlightPlan::new(vec![Vec3::ZERO, Vec3::new(1_000.0, 0.0, 0.0)], true, 100.0);
        plan.waits[0] = 2.0;
        let at = Vec3::new(FLIGHT_ARRIVAL_RADIUS * 0.5, 0.0, 0.0);
        let step = plan.steer(at, 0.01);
        assert_eq!(step.arrived_at, Some(0));
        assert_eq!(step.order, FlightOrder::Hold);
        let mut held = 0.0f32;
        loop {
            let step = plan.steer(at, 0.5);
            held += 0.5;
            if step.order != FlightOrder::Hold {
                assert_eq!(step.order, FlightOrder::FlyTo(Vec3::new(1_000.0, 0.0, 0.0)));
                break;
            }
            assert!(held <= 3.0, "held too long");
        }
        assert!((held - 2.0).abs() < 1e-3, "{held}");
    }

    #[test]
    fn a_plan_is_bounded_and_tolerates_bad_inputs() {
        let many: Vec<Vec3> = (0..(MAX_FLIGHT_NODES + 5))
            .map(|i| Vec3::new(f32::from(u8::try_from(i).unwrap_or(u8::MAX)), 0.0, 0.0))
            .collect();
        let plan = FlightPlan::new(many, true, f32::NAN);
        assert_eq!(plan.waypoints().len(), MAX_FLIGHT_NODES);
        assert!((plan.speed() - FLIGHT_SPEED).abs() < 1e-6);
        assert!(plan.is_looped());
        let mut plan = plan;
        assert_eq!(plan.steer(Vec3::NAN, 0.1).order, FlightOrder::Hold);
        assert_eq!(
            FlightPlan::new(Vec::new(), true, 1.0)
                .steer(Vec3::ZERO, 0.1)
                .order,
            FlightOrder::Hold
        );
    }

    #[test]
    fn progress_round_trips_and_a_restore_is_clamped() {
        let mut plan = FlightPlan::new(square(), true, 500.0).starting_inactive();
        plan.activate();
        let _ = plan.steer(square()[0], 0.01);
        plan.waits[1] = 3.0;
        let _ = plan.steer(square()[1], 0.01);
        let progress = plan.progress();
        assert_eq!(progress.current, 2);
        assert!(progress.active);
        assert!((progress.wait_left - 3.0).abs() < 1e-6);

        let mut fresh = FlightPlan::new(square(), true, 500.0).starting_inactive();
        fresh.waits[1] = 3.0;
        fresh.restore(progress);
        assert_eq!(fresh.progress(), progress);
        assert_eq!(fresh.steer(square()[1], 0.01).order, FlightOrder::Hold);

        fresh.restore(FlightProgress {
            current: 99,
            active: true,
            wait_left: f32::NAN,
        });
        assert_eq!(fresh.progress().current, square().len());
        assert!(fresh.progress().wait_left.abs() < 1e-6);
    }

    #[test]
    fn a_plan_over_a_path_chain_keeps_positions_waits_and_the_loop() {
        use ohl_game::keyvalues::Limits;
        use ohl_game::{EntityDef, Registry};
        use std::collections::BTreeMap;

        // Project-authored synthetic names and positions.
        let corner = |name: &str, origin: [f32; 3], target: &str, wait: &str| EntityDef {
            classname: "path_corner".to_string(),
            keyvalues: [
                ("target".to_string(), target.to_string()),
                ("wait".to_string(), wait.to_string()),
            ]
            .into_iter()
            .collect(),
            origin,
            angles: [0.0; 3],
            targetname: Some(name.to_string()),
            target: Some(target.to_string()),
            spawnflags: 0,
            model: None,
            render: ohl_game::keyvalues::RenderProps::default(),
        };
        let defs = vec![
            corner("p1", [0.0, 0.0, 256.0], "p2", "0"),
            corner("p2", [512.0, 0.0, 256.0], "p3", "1.5"),
            corner("p3", [512.0, 512.0, 256.0], "p1", "0"),
        ];
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let chain = ohl_game::PathChain::build(&registry, "p1", 0.0).expect("chain");
        let plan = FlightPlan::from_chain(&chain, FLIGHT_SPEED);
        assert_eq!(plan.waypoints().len(), 3);
        assert!(plan.is_looped());
        assert!((plan.waits()[1] - 1.5).abs() < 1e-6);
        assert_eq!(plan.waypoints()[2], Vec3::new(512.0, 512.0, 256.0));
    }
}
