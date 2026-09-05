//! Local steering in a fixed-tick simulation.

mod common;

use common::{corner_corridor, open_room, slide_move};
use ohl_nav::{MoveIntent, Path, Steer, SteerLimits, Vec3};
use ohl_physics::{CollisionModel, Hull};

/// The project's simulation tick, matching `ohl_physics::controller`.
const TICK_SECONDS: f32 = 0.01;
/// A plausible monster speed, in units per second. A project choice: this
/// test only needs a mover, not a published value.
const SPEED: f32 = 200.0;

fn path_through(points: &[Vec3]) -> Path {
    Path {
        waypoints: points.to_vec(),
        nodes: Vec::new(),
        cost: 0.0,
        explored: 0,
    }
}

/// Runs the steering loop until it reports arrival or `max_ticks` elapse.
fn simulate(
    collision: &CollisionModel,
    hull: Hull,
    start: Vec3,
    path: &Path,
    max_ticks: u32,
) -> (Vec3, bool, u32, Steer) {
    let limits = SteerLimits::default();
    let mut steer = Steer::new();
    let mut pos = start;
    for tick in 0..max_ticks {
        let intent: MoveIntent = steer.next_move(pos, path, hull, collision, &limits);
        assert!(
            (0.0..=1.0).contains(&intent.speed_scale),
            "speed scale out of range: {}",
            intent.speed_scale
        );
        if intent.reached {
            return (pos, true, tick, steer);
        }
        assert!(
            intent.dir.length() < 1.001,
            "the intent direction must be a unit vector or zero"
        );
        pos = slide_move(
            collision,
            hull,
            pos,
            intent.dir * (SPEED * intent.speed_scale * TICK_SECONDS),
        );
    }
    (pos, false, max_ticks, steer)
}

#[test]
fn steering_follows_a_path_around_a_corner() {
    let collision = corner_corridor();
    let hull = Hull::Standing;
    let foot = hull.foot_offset();
    let start = Vec3::new(-192.0, 0.0, foot);
    let path = path_through(&[
        Vec3::new(-192.0, 0.0, foot),
        Vec3::new(0.0, 0.0, foot),
        Vec3::new(0.0, 384.0, foot),
    ]);

    let (end, reached, ticks, steer) = simulate(&collision, hull, start, &path, 1000);

    assert!(
        reached,
        "steering did not reach the goal, stopped at {end:?}"
    );
    assert!(
        (end - Vec3::new(0.0, 384.0, foot)).length() <= SteerLimits::default().arrive_radius,
        "ended at {end:?}"
    );
    // The route is 192 + 384 units long, so at 2 units per tick it cannot
    // possibly take fewer than ~280 ticks, and should not take many more.
    assert!((250..800).contains(&ticks), "took {ticks} ticks");
    assert!(!steer.is_stuck());
    assert_eq!(steer.cursor(), 2);
}

#[test]
fn a_mover_pushed_into_a_wall_slides_instead_of_stopping() {
    let collision = corner_corridor();
    let hull = Hull::Standing;
    let foot = hull.foot_offset();
    // Aim diagonally into the north wall of the east-west corridor; the
    // waypoint is reachable only by sliding along it.
    let start = Vec3::new(-192.0, 40.0, foot);
    let path = path_through(&[Vec3::new(0.0, 40.0, foot)]);

    let (end, reached, _, _) = simulate(&collision, hull, start, &path, 600);
    assert!(
        reached,
        "sliding did not reach the goal, stopped at {end:?}"
    );
}

#[test]
fn a_mover_wedged_in_a_corner_reports_being_stuck() {
    let collision = corner_corridor();
    let hull = Hull::Standing;
    let foot = hull.foot_offset();
    // A waypoint behind solid wall, with the mover pressed against it.
    let start = Vec3::new(-232.0, 0.0, foot);
    let path = path_through(&[Vec3::new(-512.0, 0.0, foot)]);

    let (_, reached, _, steer) = simulate(&collision, hull, start, &path, 200);
    assert!(!reached);
    assert!(
        steer.is_stuck(),
        "a mover that made no progress must report being stuck"
    );
}

#[test]
fn waypoints_already_reached_are_skipped_and_the_path_can_be_replaced() {
    let collision = open_room();
    let hull = Hull::Standing;
    let foot = hull.foot_offset();
    let mut steer = Steer::new();
    let limits = SteerLimits::default();
    let path = path_through(&[
        Vec3::new(0.0, 0.0, foot),
        Vec3::new(64.0, 0.0, foot),
        Vec3::new(128.0, 0.0, foot),
    ]);

    // Standing on the second waypoint skips the first.
    let intent = steer.next_move(Vec3::new(64.0, 0.0, foot), &path, hull, &collision, &limits);
    assert_eq!(steer.cursor(), 2);
    assert!(!intent.reached);
    assert!(intent.dir.x > 0.9, "should head to the last waypoint");

    // Standing on the last waypoint reports arrival.
    let intent = steer.next_move(
        Vec3::new(128.0, 0.0, foot),
        &path,
        hull,
        &collision,
        &limits,
    );
    assert!(intent.reached);
    assert_eq!(intent.dir, Vec3::ZERO);

    steer.reset();
    assert_eq!(steer.cursor(), 0);
    assert!(!steer.is_stuck());

    // An empty path is inert rather than a panic.
    let empty = path_through(&[]);
    let intent = steer.next_move(Vec3::ZERO, &empty, hull, &collision, &limits);
    assert!(!intent.reached);
    assert_eq!(intent.dir, Vec3::ZERO);
}
