//! `trigger_camera`: a scripted view-override sequence, optionally following
//! a `path_corner`/`path_track` chain.
//!
//! This module owns the *runtime* half of a `trigger_camera` — how far along
//! its `moveto` path it has travelled and how much of its `wait` hold
//! remains — the same way `crate::track_train` owns a `func_train`'s runtime
//! position. The static keyvalues live on
//! [`crate::registry::TriggerCamera`]; the entity carries both components.
//! `ohl-engine` reads [`TriggerCameraState::path_position`]/
//! [`TriggerCameraState::is_active`] each frame the same way it already
//! reads a track train's resolved position, to override the world camera
//! and to gate player input while "Freeze Player" is set; see
//! `crates/ohl-engine/src/camera.rs`.
//!
//! Semantics are taken only from public mapping documentation (TWHL's
//! `trigger_camera` page and its "Tutorial: trigger_camera"); see
//! `docs/FORMAT_SOURCES.md` ("Camera sequences") for the exact pages and
//! which fact came from which one, and for every `TODO(black-box)` this
//! module leaves undecided rather than guessed at silently. No SDK source or
//! decompiled logic was consulted.

use glam::Vec3;
use hecs::Entity;

use crate::registry::{Registry, Target, TriggerCamera};
use crate::track_train::PathChain;

/// Largest number of node-boundary transitions [`TriggerCameraState::advance`]
/// processes in one call, mirroring
/// [`crate::track_train::TrackTrainState::advance`]'s own bound so a run of
/// zero-length (coincident) nodes cannot spin the loop forever at a large
/// `dt` or `speed`.
const MAX_TRANSITIONS_PER_TICK: usize = 64;

/// A `trigger_camera`'s runtime state: whether it is currently overriding
/// the player's view, how much of its `wait` hold remains, and (when it has
/// a `moveto` path) how far along that path it has travelled.
///
/// Not (de)serializable, matching
/// [`crate::track_train::TrackTrainState`]'s own choice: see
/// `docs/FORMAT_SOURCES.md` ("Camera sequences") for why this project does
/// not currently carry it across a save/load.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerCameraState {
    chain: Option<PathChain>,
    node_index: usize,
    /// Progress from `node_index` toward the next node, in `0..=1`.
    t: f32,
    /// Current travel speed, units/second. See [`TriggerCamera::acceleration`]
    /// for why this never changes except at a `path_corner`'s own `speed`
    /// override: no public source documents an accel/decel curve.
    speed: f32,
    /// Seconds remaining in a `path_corner`'s own `wait` pause.
    wait_timer: f32,
    /// Whether this sequence is currently overriding the player's view.
    active: bool,
    /// Seconds remaining before the sequence reverts the player's view,
    /// counting down from `TriggerCamera::hold_seconds` on activation.
    hold_remaining: f32,
}

impl TriggerCameraState {
    /// A dormant state for `camera`, with `chain` already resolved from its
    /// `moveto` keyvalue (`None` for a stationary camera).
    #[must_use]
    pub fn spawn(camera: &TriggerCamera, chain: Option<PathChain>) -> Self {
        Self {
            chain,
            node_index: 0,
            t: 0.0,
            speed: camera.speed.max(0.0),
            wait_timer: 0.0,
            active: false,
            hold_remaining: 0.0,
        }
    }

    /// Whether this sequence is currently overriding the player's view.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// The camera's current position along its `moveto` path: exactly at a
    /// node, or linearly interpolated along its active segment. `None` for a
    /// stationary camera (no `moveto`), in which case a caller falls back to
    /// the entity's own placed `Transform::origin`.
    #[must_use]
    pub fn path_position(&self) -> Option<Vec3> {
        let chain = self.chain.as_ref()?;
        let start = chain.nodes.get(self.node_index)?.position;
        Some(match chain.next_index(self.node_index) {
            Some(next) => start.lerp(chain.nodes[next].position, self.t.clamp(0.0, 1.0)),
            None => start,
        })
    }

    /// The horizontal yaw (degrees, counter-clockwise around `+Z` from `+X`,
    /// matching [`crate::registry::movedir_from_angles`]'s convention)
    /// facing along the active path segment, for a caller that has no
    /// look-at target to aim at instead. `None` without a `moveto` path, at
    /// its last (non-looped) node, or across a purely vertical segment.
    #[must_use]
    pub fn path_yaw_degrees(&self) -> Option<f32> {
        let chain = self.chain.as_ref()?;
        let next = chain.next_index(self.node_index)?;
        let direction = chain.nodes[next].position - chain.nodes[self.node_index].position;
        if direction.x.abs() < f32::EPSILON && direction.y.abs() < f32::EPSILON {
            return None;
        }
        Some(direction.y.atan2(direction.x).to_degrees())
    }

    /// Starts or stops this sequence, per the documented "An active camera
    /// can be stopped by triggering it again." A stopped sequence does not
    /// fire `target` (only a natural completion via [`Self::advance`] does)
    /// and does not resume from where it left off on a later activation —
    /// it restarts from the first path node.
    ///
    /// **`TODO(black-box)`**: neither source page states whether stopping
    /// counts as "finishing" for the completion `target`, or whether a
    /// later activation should instead resume mid-path; this project's
    /// choice (no fire, always restart) is the one least likely to strand a
    /// map waiting on a `target` that a manual stop never sends.
    pub fn trigger(&mut self, camera: &TriggerCamera) {
        if self.active {
            self.active = false;
            self.wait_timer = 0.0;
            return;
        }
        self.active = true;
        self.node_index = 0;
        self.t = 0.0;
        self.wait_timer = 0.0;
        self.speed = camera.speed.max(0.0);
        self.hold_remaining = if camera.hold_seconds.is_finite() {
            camera.hold_seconds.max(0.0)
        } else {
            0.0
        };
    }

    /// Advances this sequence by `dt` seconds. Returns `true` exactly the
    /// tick it transitions from active to inactive naturally (the `wait`
    /// hold elapsed, or — per this project's reading of "for `wait` seconds
    /// ... or until the path ends", see `docs/FORMAT_SOURCES.md` — a
    /// non-looped path reached its dead end first), which is the caller's
    /// cue to fire `target` exactly once. A no-op, returning `false`, while
    /// dormant. `dt` is clamped to `0..` first, so a non-finite or negative
    /// caller value cannot run this backward or corrupt the timers.
    pub fn advance(&mut self, dt: f32) -> bool {
        if !self.active {
            return false;
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.hold_remaining = (self.hold_remaining - dt).max(0.0);

        let mut dead_ended = false;
        if self.wait_timer > 0.0 {
            self.wait_timer = (self.wait_timer - dt).max(0.0);
        } else if let Some(chain) = self.chain.clone() {
            let mut remaining = self.speed.max(0.0) * dt;
            let mut transitions = 0;
            while remaining > 0.0 && transitions < MAX_TRANSITIONS_PER_TICK {
                let Some(next) = chain.next_index(self.node_index) else {
                    dead_ended = true;
                    break;
                };
                let len = chain.segment_len(self.node_index, next);
                let remaining_in_segment = (1.0 - self.t.clamp(0.0, 1.0)) * len;
                if len <= f32::EPSILON || remaining >= remaining_in_segment {
                    remaining -= remaining_in_segment.max(0.0);
                    self.node_index = next;
                    self.t = 0.0;
                    transitions += 1;
                    let node = chain.nodes[self.node_index];
                    if let Some(speed) = node.speed {
                        self.speed = speed.abs();
                    }
                    // A `path_corner`'s own "Wait for retrigger" mid-path is
                    // read the same way a dead end is: see the
                    // `TODO(black-box)` in `docs/FORMAT_SOURCES.md`.
                    if node.stop {
                        dead_ended = true;
                        break;
                    }
                    if node.wait > 0.0 {
                        self.wait_timer = node.wait;
                        break;
                    }
                } else {
                    self.t += remaining / len;
                    remaining = 0.0;
                }
            }
        }

        if self.hold_remaining <= 0.0 || dead_ended {
            self.active = false;
            return true;
        }
        false
    }
}

/// Builds a [`TriggerCameraState`] for every `trigger_camera` entity,
/// resolving its `moveto` (when non-empty) into a [`PathChain`] the same way
/// `crate::track_train::spawn_all` resolves a train's `target`. Called once,
/// after every entity (and so the whole `targetname` index) exists, since a
/// camera's first path node commonly appears later in the entities lump
/// than the camera itself.
pub fn spawn_all(registry: &mut Registry) {
    let candidates: Vec<(Entity, TriggerCamera)> = registry
        .world
        .query::<(Entity, &TriggerCamera)>()
        .iter()
        .map(|(entity, camera)| (entity, camera.clone()))
        .collect();
    for (entity, camera) in candidates {
        let chain = (!camera.move_to.is_empty())
            .then(|| PathChain::build(registry, &camera.move_to, 0.0))
            .flatten();
        let state = TriggerCameraState::spawn(&camera, chain);
        registry.world.insert_one(entity, state).ok();
    }
}

/// The entity `camera_entity`'s completion `target`, when it has one, as
/// recorded by the generic [`Target`] component every entity with a
/// `target` keyvalue carries.
#[must_use]
pub(crate) fn completion_target(registry: &Registry, camera_entity: Entity) -> Option<String> {
    registry
        .world
        .get::<&Target>(camera_entity)
        .ok()
        .map(|target| target.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvalues::{Limits, parse_entities};
    use ohl_formats::bsp30::Entity as RawEntity;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn build_registry(entities: &[RawEntity]) -> Registry {
        let defs = parse_entities(entities, &Limits::default());
        Registry::build(&defs, &BTreeMap::new(), &Limits::default())
    }

    fn camera_state(registry: &Registry, name: &str) -> TriggerCameraState {
        let entity = registry.find(name)[0];
        (*registry
            .world
            .get::<&TriggerCameraState>(entity)
            .expect("camera state"))
        .clone()
    }

    fn camera_component(registry: &Registry, name: &str) -> TriggerCamera {
        let entity = registry.find(name)[0];
        (*registry
            .world
            .get::<&TriggerCamera>(entity)
            .expect("camera"))
        .clone()
    }

    #[test]
    fn a_stationary_camera_holds_for_wait_seconds_then_completes() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
            ("target", "after"),
            ("wait", "2"),
        ])];
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        let mut state = camera_state(&registry, "cam1");
        assert!(!state.is_active());
        state.trigger(&camera);
        assert!(state.is_active());
        assert!(state.path_position().is_none());
        for _ in 0..190 {
            assert!(!state.advance(0.01));
        }
        assert!(state.is_active());
        // A handful of extra ticks covers the fixed-point slack of summing
        // many `0.01`s, the same margin `track_train`'s own tests give
        // themselves.
        let mut completed = false;
        for _ in 0..20 {
            if state.advance(0.01) {
                completed = true;
                break;
            }
        }
        assert!(completed, "the hold has now elapsed");
        assert!(!state.is_active());
    }

    #[test]
    fn defaults_match_the_published_fgd_summary() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
        ])];
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        assert!((camera.hold_seconds - 10.0).abs() < f32::EPSILON);
        assert!((camera.speed - 0.0).abs() < f32::EPSILON);
        assert!((camera.acceleration - 500.0).abs() < f32::EPSILON);
        assert!((camera.deceleration - 500.0).abs() < f32::EPSILON);
        assert!(camera.move_to.is_empty());
        assert!(!camera.start_at_player);
        assert!(!camera.follow_player);
        assert!(!camera.freeze_player);
    }

    #[test]
    fn spawnflags_are_read() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
            ("spawnflags", "7"),
        ])];
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        assert!(camera.start_at_player);
        assert!(camera.follow_player);
        assert!(camera.freeze_player);
    }

    /// A three-node straight track, exactly like
    /// `track_train`'s own fixture, but targeted by a `trigger_camera`'s
    /// `moveto` instead of a `func_tracktrain`'s `target`. All keyvalues are
    /// authored for this test; none are derived from any payload.
    fn three_node_path(camera_extra: &[(&str, &str)]) -> Vec<RawEntity> {
        let mut camera_kv = vec![
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
            ("moveto", "node1"),
            ("target", "after"),
        ];
        camera_kv.extend_from_slice(camera_extra);
        vec![
            raw(&camera_kv),
            raw(&[
                ("classname", "path_corner"),
                ("targetname", "node1"),
                ("target", "node2"),
                ("origin", "0 0 0"),
            ]),
            raw(&[
                ("classname", "path_corner"),
                ("targetname", "node2"),
                ("target", "node3"),
                ("origin", "100 0 0"),
            ]),
            raw(&[
                ("classname", "path_corner"),
                ("targetname", "node3"),
                ("origin", "200 0 0"),
            ]),
        ]
    }

    #[test]
    fn a_camera_travels_its_moveto_path_and_the_dead_end_completes_it_early() {
        let entities = three_node_path(&[("wait", "1000"), ("speed", "50")]);
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        let mut state = camera_state(&registry, "cam1");
        state.trigger(&camera);
        assert_eq!(state.path_position(), Some(Vec3::ZERO));
        // 200 units at 50/sec = 4s to the dead end at node3, far short of
        // the 1000s `wait`: the dead end must end the sequence first.
        let mut completed_at = None;
        for step in 0..500 {
            if state.advance(0.01) {
                completed_at = Some(step);
                break;
            }
        }
        assert!(completed_at.is_some(), "the dead end must end the sequence");
        assert!(!state.is_active());
        assert_eq!(state.path_position(), Some(Vec3::new(200.0, 0.0, 0.0)));
    }

    #[test]
    fn a_path_corner_wait_pauses_the_camera_mid_path() {
        let mut entities = three_node_path(&[("wait", "1000"), ("speed", "100")]);
        // node2 (index 2 of the vec: camera, node1, node2, node3) gets a
        // 1-second wait.
        entities[2].insert("wait".to_string(), "1".to_string());
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        let mut state = camera_state(&registry, "cam1");
        state.trigger(&camera);
        // 100 units at 100/sec = 1s to reach node2.
        for _ in 0..105 {
            assert!(!state.advance(0.01));
        }
        assert_eq!(state.path_position(), Some(Vec3::new(100.0, 0.0, 0.0)));
        // Still paused just before the 1-second wait elapses.
        for _ in 0..89 {
            assert!(!state.advance(0.01));
        }
        assert_eq!(state.path_position(), Some(Vec3::new(100.0, 0.0, 0.0)));
        // The wait elapses and the camera resumes toward the dead end.
        for _ in 0..300 {
            if state.advance(0.01) {
                break;
            }
        }
        assert!(!state.is_active());
        assert_eq!(state.path_position(), Some(Vec3::new(200.0, 0.0, 0.0)));
    }

    #[test]
    fn retriggering_an_active_camera_stops_it_without_firing_and_a_later_trigger_restarts() {
        let entities = three_node_path(&[("wait", "1000"), ("speed", "50")]);
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        let mut state = camera_state(&registry, "cam1");
        state.trigger(&camera);
        for _ in 0..100 {
            state.advance(0.01);
        }
        let midpoint = state.path_position();
        assert_ne!(midpoint, Some(Vec3::ZERO));
        // Stop it: no completion signal from `trigger`, and the sequence is
        // no longer active.
        state.trigger(&camera);
        assert!(!state.is_active());
        // A later activation restarts from the first node, not from the
        // stopped midpoint (this project's documented choice).
        state.trigger(&camera);
        assert!(state.is_active());
        assert_eq!(state.path_position(), Some(Vec3::ZERO));
    }

    #[test]
    fn completion_fires_the_target_exactly_once() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
            ("target", "after"),
            ("wait", "0.2"),
        ])];
        let registry = build_registry(&entities);
        let camera = camera_component(&registry, "cam1");
        let mut state = camera_state(&registry, "cam1");
        state.trigger(&camera);
        let mut completions = 0;
        for _ in 0..40 {
            if state.advance(0.01) {
                completions += 1;
            }
        }
        assert_eq!(completions, 1, "completion must signal exactly once");
    }

    #[test]
    fn completion_target_is_read_from_the_generic_target_component() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
            ("target", "after_cam"),
        ])];
        let registry = build_registry(&entities);
        let entity = registry.find("cam1")[0];
        assert_eq!(
            completion_target(&registry, entity),
            Some("after_cam".to_string())
        );
    }

    #[test]
    fn a_camera_with_no_target_has_no_completion_target() {
        let entities = vec![raw(&[
            ("classname", "trigger_camera"),
            ("targetname", "cam1"),
        ])];
        let registry = build_registry(&entities);
        let entity = registry.find("cam1")[0];
        assert_eq!(completion_target(&registry, entity), None);
    }

    proptest! {
        /// However a chain is shaped and however this state is advanced
        /// (arbitrary speed, arbitrary sequence of fixed-timestep advances,
        /// possibly non-finite `dt`), the reported path position is always
        /// finite and always a convex combination of two adjacent chain
        /// nodes, and `advance` always terminates within this call (the
        /// bound proptest itself relies on).
        #[test]
        fn advance_stays_finite_and_on_the_polyline(
            coords in prop::collection::vec(
                (-1000.0f32..1000.0f32, -1000.0f32..1000.0f32, -1000.0f32..1000.0f32),
                1..8,
            ),
            speed in 0.0f32..500.0,
            hold in 0.0f32..50.0,
            steps in prop::collection::vec(
                prop_oneof![Just(f32::NAN), Just(f32::INFINITY), 0.0f32..0.5],
                0..200,
            ),
        ) {
            let mut world = hecs::World::new();
            let nodes: Vec<crate::track_train::PathNode> = coords
                .iter()
                .map(|&(x, y, z)| crate::track_train::PathNode {
                    entity: world.spawn(()),
                    position: Vec3::new(x, y, z),
                    wait: 0.0,
                    speed: None,
                    stop: false,
                })
                .collect();
            let min = coords.iter().fold(Vec3::splat(f32::MAX), |acc, &(x, y, z)| {
                acc.min(Vec3::new(x, y, z))
            });
            let max = coords.iter().fold(Vec3::splat(f32::MIN), |acc, &(x, y, z)| {
                acc.max(Vec3::new(x, y, z))
            });
            let mut state = TriggerCameraState {
                chain: Some(PathChain { nodes, looped: false }),
                node_index: 0,
                t: 0.0,
                speed,
                wait_timer: 0.0,
                active: true,
                hold_remaining: hold,
            };
            for dt in steps {
                state.advance(dt);
                if let Some(position) = state.path_position() {
                    prop_assert!(position.is_finite());
                    prop_assert!(position.x >= min.x - 1e-2 && position.x <= max.x + 1e-2);
                    prop_assert!(position.y >= min.y - 1e-2 && position.y <= max.y + 1e-2);
                    prop_assert!(position.z >= min.z - 1e-2 && position.z <= max.z + 1e-2);
                }
            }
        }
    }
}
