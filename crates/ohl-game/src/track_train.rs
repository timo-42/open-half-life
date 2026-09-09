//! `func_train`/`func_tracktrain` spawn placement and `path_corner`/
//! `path_track` chain following.
//!
//! Semantics are taken only from public mapping documentation (TWHL and
//! Valve Developer Community pages for `func_tracktrain`, `path_track`,
//! `func_train` and `path_corner`, plus TWHL's "Tutorial: Trains"); see
//! `docs/FORMAT_SOURCES.md` ("Track trains and paths") for the exact pages
//! and which fact came from which one. No SDK source or decompiled logic
//! was consulted. Everything not confirmed by a public source is marked
//! `TODO(black-box)` below and left as a documented, defensible choice
//! rather than a guess presented as fact.
//!
//! A [`TrackTrain`] is the static keyvalue data (`func_train`/
//! `func_tracktrain`'s own spawn-time properties); a [`TrackTrainState`] is
//! the mutable runtime position, built once at load time by walking the
//! entity's `target` chain of `path_corner`/`path_track` nodes into a
//! [`PathChain`], then advanced by [`TrackTrainState::advance`] each fixed
//! timestep. `ohl-engine` reads [`TrackTrainState::position`] and
//! [`TrackTrainState::yaw_degrees`] each frame the same way it already reads
//! a door's timer to place a brush submodel; see
//! `crates/ohl-engine/src/render.rs`'s `door_offset`.

use glam::Vec3;
use hecs::Entity;

use crate::registry::{Path, PathFireOnPass, Registry, Target, Transform};

/// Largest number of nodes one path chain follows before giving up,
/// bounding both a malformed non-terminating scan and the memory one
/// [`TrackTrainState`] holds. GoldSrc maps do not chain hundreds of
/// `path_track`s for one train; this is a generous, deterministic ceiling
/// rather than an observed limit.
pub const MAX_PATH_NODES: usize = 256;

/// Largest number of node-boundary transitions [`TrackTrainState::advance`]
/// processes in one call, so a run of zero-length (coincident) nodes cannot
/// spin the loop forever at a high frame time or `speed`.
const MAX_TRANSITIONS_PER_TICK: usize = 64;

/// `path_track`'s documented "Wait for retrigger" spawnflag: the train
/// stops here and does not continue until it is triggered again, rather
/// than resuming automatically. Bit value per public search-engine summaries
/// of the TWHL and Sven Co-op wiki `path_track`/`path_corner` pages (the
/// TWHL page itself returns HTTP 403 to automated fetches from this
/// environment, matching the precedent already recorded for the other
/// entity pages in `docs/FORMAT_SOURCES.md`).
///
/// TODO(black-box): the exact bit position is drawn from a fan/derivative
/// wiki (Sven Co-op), not a fetchable primary HL1 SDK source; treat this as
/// a defensible best-public-source value, not a confirmed engine constant.
const PATH_TRACK_STOP_FLAG: u32 = 1;

/// `func_tracktrain`'s documented "No User Control" spawnflag: the train
/// cannot be steered/accelerated by a player standing on it. This project
/// implements no player-driven control at all (only triggered path
/// following), so the flag is recorded on [`TrackTrain`] for completeness
/// but has no runtime effect today.
///
/// TODO(black-box): the exact bit value is drawn from search-engine
/// summaries of the TWHL `func_tracktrain` page (also 403 to automated
/// fetches); treat it the same way as [`PATH_TRACK_STOP_FLAG`].
const TRACKTRAIN_NO_USER_CONTROL_FLAG: u32 = 2;

/// One `path_corner`/`path_track` node, resolved into world space (its
/// `height` offset, when the owning train supplied one, already added to
/// `position`).
#[derive(Debug, Clone, PartialEq)]
pub struct PathNode {
    /// The node's own entity, kept so a caller could look up further
    /// keyvalues.
    pub entity: Entity,
    /// World-space position, `height` already applied.
    pub position: Vec3,
    /// Seconds to pause here before auto-continuing (`path_corner`/
    /// `path_track`'s `wait`), `0` for no pause.
    pub wait: f32,
    /// `path_track`'s documented "New Train Speed": when present, the
    /// train's speed is set to this value on passing the node.
    pub speed: Option<f32>,
    /// The documented "Wait for retrigger" spawnflag: the train stops here
    /// and needs an explicit trigger (see [`TrackTrainState::toggle`]) to
    /// resume, rather than continuing after `wait` seconds.
    pub stop: bool,
    /// The documented fire-on-pass `message`: the name of an entity fired
    /// as a follower passes this node
    /// ([`crate::registry::PathFireOnPass`]).
    pub message: Option<String>,
    /// The documented fire-on-dead-end `netname`: the name of an entity
    /// fired when a `func_tracktrain` reaches this node *as the last node
    /// of its chain* ([`crate::registry::PathFireOnDeadEnd`]).
    pub dead_end: Option<String>,
}

/// The three fields of a node the train has just reached that
/// [`TrackTrainState::advance_firing`] still needs after it stops borrowing
/// the chain, so the fire-on-pass `message` can be read by reference
/// instead of cloning a whole [`PathNode`] per boundary crossing.
#[derive(Debug, Clone, Copy)]
struct PassedNode {
    speed: Option<f32>,
    stop: bool,
    wait: f32,
}

/// A resolved `path_corner`/`path_track` chain, walked once at load time
/// from a train's first node name (its `target` keyvalue) via each node's
/// own `target` (the same generic [`Target`] component every entity with a
/// `target` keyvalue carries).
///
/// Branching (`path_track`'s documented `altpath`) is not implemented: this
/// project only follows the single `target` chain a map's primary route
/// describes. TODO(black-box): `altpath` branch selection semantics are not
/// implemented; a map that relies on it will have its train follow the
/// chain as if `altpath` did not exist.
#[derive(Debug, Clone, PartialEq)]
pub struct PathChain {
    /// The resolved nodes, in chain order starting from the train's first
    /// node.
    pub nodes: Vec<PathNode>,
    /// `true` when the chain's last node's `target` resolves back to the
    /// first node (a closed loop, e.g. a tram that circles back to its
    /// start), letting [`TrackTrainState`] wrap past the last/first node
    /// instead of dead-ending.
    pub looped: bool,
}

impl PathChain {
    /// Walks `first_name` and its `target` chain into a resolved
    /// [`PathChain`], adding `height` to every node's stored position.
    /// Returns `None` when the first node cannot be found. Bounded to
    /// [`MAX_PATH_NODES`]; a chain that has not closed a loop or dead-ended
    /// by then simply stops there.
    #[must_use]
    pub fn build(registry: &Registry, first_name: &str, height: f32) -> Option<Self> {
        let mut nodes: Vec<PathNode> = Vec::new();
        let mut current_name = first_name.to_string();
        let mut looped = false;

        while nodes.len() < MAX_PATH_NODES {
            let Some(&entity) = registry.find(&current_name).first() else {
                break;
            };
            if nodes.iter().any(|node| node.entity == entity) {
                looped = nodes.first().is_some_and(|first| first.entity == entity);
                break;
            }
            let Ok(path) = registry.world.get::<&Path>(entity) else {
                break;
            };
            let path = *path;
            let position = registry
                .world
                .get::<&Transform>(entity)
                .map_or(Vec3::ZERO, |transform| transform.origin)
                + Vec3::Z * height;
            let next = registry
                .world
                .get::<&Target>(entity)
                .ok()
                .map(|target| target.0.clone());
            let message = registry
                .world
                .get::<&PathFireOnPass>(entity)
                .ok()
                .map(|fire| fire.0.clone());
            let dead_end = registry
                .world
                .get::<&crate::registry::PathFireOnDeadEnd>(entity)
                .ok()
                .map(|fire| fire.0.clone());
            nodes.push(PathNode {
                entity,
                position,
                wait: path.wait,
                speed: path.speed,
                stop: path.stop,
                message,
                dead_end,
            });
            match next {
                Some(next_name) => current_name = next_name,
                None => break,
            }
        }

        if nodes.is_empty() {
            None
        } else {
            Some(Self { nodes, looped })
        }
    }

    /// The node index a train moving forward from `index` would reach next,
    /// wrapping to `0` when [`Self::looped`] and `index` is the last node;
    /// `None` at a non-looped chain's last node (a documented dead end).
    ///
    /// `pub(crate)` so `crate::camera::TriggerCameraState` can walk the same
    /// chain type a `trigger_camera`'s `moveto` resolves into, per the
    /// public documentation recorded in `docs/FORMAT_SOURCES.md` ("Camera
    /// sequences") that a `trigger_camera` follows `path_corner`s "similar
    /// to a func_train".
    pub(crate) fn next_index(&self, index: usize) -> Option<usize> {
        if index + 1 < self.nodes.len() {
            Some(index + 1)
        } else if self.looped && !self.nodes.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// The mirror of [`Self::next_index`] for a train moving backward.
    fn prev_index(&self, index: usize) -> Option<usize> {
        if index > 0 {
            Some(index - 1)
        } else if self.looped && !self.nodes.is_empty() {
            Some(self.nodes.len() - 1)
        } else {
            None
        }
    }

    /// `pub(crate)`; see [`Self::next_index`]'s doc comment.
    pub(crate) fn segment_len(&self, a: usize, b: usize) -> f32 {
        self.nodes[a].position.distance(self.nodes[b].position)
    }
}

/// `func_train`/`func_tracktrain`'s own spawn-time keyvalues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackTrain {
    /// `true` for `func_tracktrain`, which the public documentation states
    /// turns to face the next `path_track` as it travels; `false` for
    /// `func_train`, which (per the same documentation) does not.
    pub turns_to_face: bool,
    /// `speed`: cruise speed in units/second, reassigned whenever the train
    /// passes a `path_track` that carries its own `speed` override.
    pub speed: f32,
    /// `startspeed`: the speed (and, via its sign, direction) the train
    /// starts at.
    ///
    /// TODO(black-box): a negative `startspeed` meaning "start moving
    /// backward" is this project's own defensible reading of "the speed the
    /// train starts at", not a confirmed engine behaviour; a zero
    /// `startspeed` is read as "does not move until triggered", matching how
    /// every other triggered mover in this crate (door/button/platform)
    /// behaves.
    pub start_speed: f32,
    /// `height`: the vertical offset above the path the train rides at.
    pub height: f32,
    /// `bank`: documented bank angle applied on turns.
    ///
    /// TODO(black-box): recorded but not applied to the placed transform;
    /// no public source documents the exact roll-vs-turn-angle formula, and
    /// guessing one would silently misrender every turn.
    pub bank: f32,
    /// `dmg`: documented crush damage dealt when the train's movement is
    /// blocked.
    ///
    /// TODO(black-box): recorded but not applied; this project's map logic
    /// simulation does not yet model blocking/crushing for any mover.
    pub dmg: f32,
    /// `wheels`: documented "front wheel" distance used to compute heading
    /// lag on corners.
    ///
    /// TODO(black-box): recorded but not applied; [`TrackTrainState::yaw_degrees`]
    /// faces the current segment directly rather than modelling a
    /// wheel-offset turn lag, since no public source documents that
    /// formula.
    pub wheels: f32,
    /// The documented "No User Control" spawnflag. See
    /// [`TRACKTRAIN_NO_USER_CONTROL_FLAG`]; recorded but unused, since this
    /// project implements no player-driven train control.
    pub no_user_control: bool,
}

impl TrackTrain {
    /// Reads the documented "No User Control" spawnflag out of a raw
    /// `spawnflags` bitmask.
    #[must_use]
    pub fn no_user_control_from_flags(spawnflags: u32) -> bool {
        spawnflags & TRACKTRAIN_NO_USER_CONTROL_FLAG != 0
    }
}

/// Interprets a `path_track`'s raw `speed` ("New Train Speed") keyvalue as
/// an override, or as "leave the train's speed alone".
///
/// Public documentation (see `docs/FORMAT_SOURCES.md`, "Track trains and
/// paths") describes the keyvalue as "as the train passes this point, this
/// speed will be assigned to it", gives it a default of `0`, and states
/// that `0` means no speed change — the same reading the same family of
/// pages already records for `func_train`'s own `speed`, which is
/// documented as "defaulting to 100 if left blank or zero", i.e. a zero
/// speed keyvalue is "unset", not "stand still". A node that carried a
/// literal zero override would otherwise park its train forever at that
/// node, since nothing in the published behaviour ever restores a speed
/// the train no longer has.
///
/// A non-finite value is likewise no override, so malformed map data
/// cannot poison a train's speed.
#[must_use]
pub fn path_speed_override(raw: f32) -> Option<f32> {
    (raw.is_finite() && raw != 0.0).then_some(raw)
}

/// Reads the documented `path_track`/`path_corner` "Wait for retrigger"
/// spawnflag out of a raw `spawnflags` bitmask.
#[must_use]
pub fn path_stop_from_flags(spawnflags: u32) -> bool {
    spawnflags & PATH_TRACK_STOP_FLAG != 0
}

/// A train's runtime position along its [`PathChain`]: which segment it is
/// on, how far across that segment, which way it is travelling, and whether
/// it is currently moving at all.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackTrainState {
    chain: PathChain,
    /// The node the train last departed from (or is currently at rest at).
    node_index: usize,
    /// Progress from `node_index` toward [`Self::other_index`], in `0..=1`.
    t: f32,
    /// `1.0` moving toward the chain's next node, `-1.0` moving toward its
    /// previous node.
    direction: f32,
    /// Current speed magnitude, units/second; sign is carried by
    /// `direction`, not by this field.
    speed: f32,
    /// `false` when stopped (either never started, or halted at a
    /// documented "Wait for retrigger" node, or waiting out a `wait`
    /// timer, or at a non-looped chain's dead end).
    moving: bool,
    /// Seconds remaining in a `path_track`'s `wait` pause before this train
    /// auto-continues.
    wait_timer: f32,
    /// `true` once the node the train is resting at has fired its
    /// documented fire-on-dead-end `netname`, so one arrival at a dead end
    /// fires it exactly once however long the train sits there. Cleared
    /// the moment the train leaves that node (it starts moving again, is
    /// re-seated, or is relinked to another chain).
    dead_end_fired: bool,
    /// A world-space displacement added to [`Self::position`] by whatever
    /// is *carrying* the whole train — today only a
    /// `func_trackchange`/`func_trackautochange` platform mid-travel (see
    /// [`Self::set_carry`]). `Vec3::ZERO` whenever the train is riding its
    /// own track under its own power, which is every step for every train
    /// no platform names.
    carry_offset: Vec3,
    /// Degrees added to [`Self::yaw_degrees`] by the same carrier, about
    /// the world up axis. `0.0` in the same "not being carried" case.
    carry_yaw: f32,
    /// The position of the first node of the chain this train *spawned*
    /// on, kept even after a [`Self::relink`] puts it on another one.
    ///
    /// This is the stand-in for an origin brush a world-baked car has no
    /// other reference point for (see
    /// [`crate::pose::track_train_transform`]): a property of where the
    /// car's vertices were compiled, not of which track it happens to be
    /// riding. Recomputing it from the current chain would teleport such a
    /// car by the whole distance between two tracks the moment a
    /// `func_trackchange` handed it over.
    first_node: Vec3,
    /// This train's own `speed` keyvalue — the speed it runs at when it
    /// is started from rest, before any `path_track` "New Train Speed"
    /// override reassigns [`Self::speed`]. Map data, rebuilt identically
    /// every load from the [`TrackTrain`] component, so it is not part of
    /// [`Self::dynamic_state`].
    cruise_speed: f32,
}

impl TrackTrainState {
    /// Places a train at the first node of `chain`: physically on the
    /// track at the node's (height-adjusted) position, facing toward the
    /// second node when `train.turns_to_face`. Starts already moving when
    /// `train.start_speed` is non-zero (its sign sets the initial
    /// direction); otherwise the train sits still until
    /// [`Self::toggle`]/[`Self::turn_on`] is called (matching every other
    /// triggered mover in this crate).
    #[must_use]
    pub fn spawn(train: &TrackTrain, chain: PathChain) -> Self {
        let moving = train.start_speed.abs() > f32::EPSILON;
        let direction = if train.start_speed < 0.0 { -1.0 } else { 1.0 };
        let first_node = chain.nodes.first().map_or(Vec3::ZERO, |node| node.position);
        Self {
            chain,
            node_index: 0,
            t: 0.0,
            direction,
            speed: if moving {
                train.start_speed.abs()
            } else {
                train.speed
            },
            moving,
            wait_timer: 0.0,
            dead_end_fired: false,
            carry_offset: Vec3::ZERO,
            carry_yaw: 0.0,
            first_node,
            cruise_speed: train.speed,
        }
    }

    /// The displacement and extra yaw a carrier is currently applying to
    /// this whole train (see [`Self::set_carry`]). `(Vec3::ZERO, 0.0)`
    /// unless a `func_trackchange`/`func_trackautochange` is mid-travel
    /// with this train aboard.
    #[must_use]
    pub fn carry(&self) -> (Vec3, f32) {
        (self.carry_offset, self.carry_yaw)
    }

    /// Sets the displacement and extra yaw a carrier applies to the whole
    /// train, in world space and degrees about the world up axis. Both are
    /// sanitized: a non-finite value is dropped rather than propagated
    /// into the placement every consumer of a pose reads.
    pub fn set_carry(&mut self, offset: Vec3, yaw_degrees: f32) {
        self.carry_offset = if offset.is_finite() {
            offset
        } else {
            Vec3::ZERO
        };
        self.carry_yaw = if yaw_degrees.is_finite() {
            yaw_degrees
        } else {
            0.0
        };
    }

    /// Puts this train on `chain`, at its first node, facing along its
    /// first segment, and clears any carrier displacement — the state a
    /// `func_trackchange`/`func_trackautochange` leaves the train it has
    /// just carried in ("after finishing, the train is assigned to
    /// path_track of the bottom path"; see `docs/FORMAT_SOURCES.md`,
    /// "Track trains and paths").
    ///
    /// `moving` says whether the train rides on from there. Its speed,
    /// direction and `wait` timer are left as they were: a relink changes
    /// which track the train is on, not how fast it travels.
    pub fn relink(&mut self, chain: PathChain, moving: bool) {
        self.chain = chain;
        self.node_index = 0;
        self.t = 0.0;
        self.direction = 1.0;
        self.wait_timer = 0.0;
        self.dead_end_fired = false;
        self.carry_offset = Vec3::ZERO;
        self.carry_yaw = 0.0;
        self.moving = moving;
    }

    /// The node currently ahead of the train in its direction of travel,
    /// i.e. the far end of its active segment.
    fn other_index(&self) -> Option<usize> {
        if self.direction >= 0.0 {
            self.chain.next_index(self.node_index)
        } else {
            self.chain.prev_index(self.node_index)
        }
    }

    /// The train's current world-space position: exactly at a node, or
    /// linearly interpolated along its active segment. Always a point on
    /// the chain's polyline.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        let start = self.chain.nodes[self.node_index].position;
        match self.other_index() {
            Some(other) => start.lerp(self.chain.nodes[other].position, self.t.clamp(0.0, 1.0)),
            None => start,
        }
    }

    /// Where this train's chain *started*: the world-space position of the
    /// first `path_track`/`path_corner` of the chain it spawned on,
    /// `height` already applied — the point [`Self::position`] returned
    /// before the train had moved at all. A [`Self::relink`] onto another
    /// chain does not change it; see [`Self::first_node`].
    ///
    /// Exposed for [`crate::pose::track_train_transform`]'s world-baked
    /// placement rule (see that function's doc comment): a train whose
    /// brushes were compiled in absolute world space has no origin brush
    /// to measure its path displacement from, and its first node is the
    /// only published reference point that stands in for one.
    #[must_use]
    pub fn first_node_position(&self) -> Vec3 {
        self.first_node
    }

    /// The train's yaw, in degrees (matching [`crate::registry::movedir_from_angles`]'s
    /// convention: counter-clockwise around `+Z` from `+X`), facing along
    /// the active segment toward the node it is heading for; `None` when
    /// `train.turns_to_face` is `false` (a plain `func_train`, which this
    /// project leaves at its spawned `angles`) or the segment has no
    /// horizontal extent (a purely vertical hop between two nodes, which
    /// carries no defined yaw).
    ///
    /// A train parked at the *end* of a non-looped chain (no node ahead of
    /// it to face) has no active segment for [`Self::other_index`] to
    /// resolve, but it did not spin back to `0` when it got there: it
    /// keeps the heading of the last segment it actually travelled, the
    /// same pose the renderer, [`crate::pose::brush_pose_rotation`]'s
    /// collision hull and `brush_center` all read this value through. That
    /// fallback mirrors [`Self::other_index`]'s own direction convention —
    /// the heading is measured from the neighbour behind the direction of
    /// travel to the current node, i.e. the same segment/order pair that
    /// was in effect right up to the step the train stopped advancing —
    /// so it agrees exactly with the yaw this method reported the instant
    /// before the train parked.
    #[must_use]
    pub fn yaw_degrees(&self, train: &TrackTrain) -> Option<f32> {
        if !train.turns_to_face {
            return None;
        }
        let direction = if let Some(other) = self.other_index() {
            self.chain.nodes[other].position - self.chain.nodes[self.node_index].position
        } else {
            let last = if self.direction >= 0.0 {
                self.node_index.checked_sub(1)
            } else {
                self.node_index
                    .checked_add(1)
                    .filter(|&index| index < self.chain.nodes.len())
            }?;
            self.chain.nodes[self.node_index].position - self.chain.nodes[last].position
        };
        if direction.x.abs() < f32::EPSILON && direction.y.abs() < f32::EPSILON {
            return None;
        }
        Some(direction.y.atan2(direction.x).to_degrees())
    }

    /// The [`PathChain`] this train follows, so a caller that has to
    /// correlate the train's current node with another map's copy of the
    /// same track can read the node entities by name.
    ///
    /// Used by `ohl_engine::transition` to carry a moving train across a
    /// level change: the documented cross-level correlation key is a
    /// shared `globalname`, and the only reference a chain position has
    /// that means anything in the destination map is the `targetname` of
    /// the `path_track` the train is currently at.
    #[must_use]
    pub fn chain(&self) -> &PathChain {
        &self.chain
    }

    /// Starts the train moving (in its current direction) if it is
    /// stopped, at its own `speed` keyvalue.
    ///
    /// A train started from rest runs at its `speed`, not at whatever a
    /// `path_track` last assigned it before it stopped: the published
    /// pages describe `speed` as the train's own speed ("Maximum speed of
    /// the track train" — Sven Co-op's `func_tracktrain`) and a node's
    /// "New Train Speed" as something that "overrides train speed after
    /// reaching this point" (Sven Co-op's `path_track`), i.e. a property
    /// of passing that node rather than of the train. [`Self::spawn`]
    /// already starts a train with no `startspeed` at exactly this speed;
    /// this keeps a train that has stopped and been re-triggered
    /// consistent with one that never moved. Without it, a track whose
    /// nodes brake the train down to a crawl on the way into a scripted
    /// stop leaves it crawling for the whole of the rest of its run,
    /// since nothing in the published behaviour ever restores the speed.
    ///
    /// **`TODO(black-box)`**: no reviewed public page states which speed
    /// a stopped `func_tracktrain` resumes at, so this is a
    /// project-determined reading of the two literals above (see
    /// `docs/FORMAT_SOURCES.md`, "Track trains and paths"). A train that
    /// is already moving is untouched, so an explicit "on" sent to a
    /// running train still changes nothing.
    ///
    /// A `speed 0` train — one whose own "Maximum speed" keyvalue is
    /// zero, but which still moves under a non-zero `startspeed` or a
    /// `path_track` "New Train Speed" override — is left at whatever
    /// speed it was already carrying instead of being snapped to `0`:
    /// `speed` names the train's own cruise speed on the two cited pages,
    /// not "the speed to resume at", and restoring a literal `0` would
    /// turn `moving` back on while leaving the train parked at its
    /// current node forever, which is exactly the stuck-crawl failure
    /// this method exists to prevent, just at the opposite extreme.
    pub fn turn_on(&mut self) {
        if !self.moving && self.cruise_speed.abs() > f32::EPSILON {
            self.speed = self.cruise_speed.abs();
        }
        self.moving = true;
    }

    /// Stops the train where it stands.
    pub fn turn_off(&mut self) {
        self.moving = false;
        self.wait_timer = 0.0;
    }

    /// Starts the train if stopped, stops it if moving. This is what a
    /// `trigger_*`/`multi_manager` "use"ing a `func_train`/`func_tracktrain`
    /// does in this project; see [`crate::logic::Simulation::activate`].
    pub fn toggle(&mut self) {
        if self.moving {
            self.turn_off();
        } else {
            self.turn_on();
        }
    }

    /// Reverses the train's direction of travel, preserving its current
    /// physical position (the active segment's endpoints and progress are
    /// re-expressed for the new direction rather than jumping the train).
    pub fn reverse(&mut self) {
        if let Some(other) = self.other_index() {
            self.node_index = other;
            self.t = 1.0 - self.t;
        }
        self.direction = -self.direction;
    }

    /// This train's own runtime fields — everything but [`Self::chain`],
    /// which the level rebuilds fresh from the map's own `path_track`
    /// chain at attach time — as a save-friendly tuple:
    /// `(node_index, t, direction, speed, moving, wait_timer)`. Used only
    /// by `ohl-engine`'s `SECTION_MOVER_STATE` (tag 28); see that crate's
    /// `save_state::TrackTrainSnapshot`.
    #[must_use]
    pub fn dynamic_state(&self) -> (usize, f32, f32, f32, bool, f32) {
        (
            self.node_index,
            self.t,
            self.direction,
            self.speed,
            self.moving,
            self.wait_timer,
        )
    }

    /// Restores fields captured by [`Self::dynamic_state`] onto this
    /// (freshly attach-level-spawned) train. `node_index` is clamped into
    /// the rebuilt chain's own bounds and every float is sanitized (a
    /// non-finite value falls back to a safe default, `direction` is
    /// forced to exactly `1.0`/`-1.0`), so a corrupt save or one taken
    /// against a different map's chain length cannot hand this train an
    /// out-of-range index or a `NaN`/`inf` timer.
    pub fn restore_dynamic_state(
        &mut self,
        node_index: usize,
        t: f32,
        direction: f32,
        speed: f32,
        moving: bool,
        wait_timer: f32,
    ) {
        let last = self.chain.nodes.len().saturating_sub(1);
        self.node_index = node_index.min(last);
        self.t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.direction = if direction < 0.0 { -1.0 } else { 1.0 };
        self.speed = if speed.is_finite() {
            speed.max(0.0)
        } else {
            0.0
        };
        self.moving = moving;
        self.wait_timer = if wait_timer.is_finite() {
            wait_timer.max(0.0)
        } else {
            0.0
        };
    }

    /// Advances this train by `dt` seconds: moves it along the chain at its
    /// current speed, applying a `path_track`'s `speed` override, `wait`
    /// pause, or "Wait for retrigger" stop as each node is passed, and
    /// halting at a non-looped chain's dead end. A no-op while stopped or
    /// waiting. Bounded to [`MAX_TRANSITIONS_PER_TICK`] node-boundary
    /// crossings so a run of zero-length nodes at a very large `dt` or
    /// `speed` cannot spin unboundedly; any leftover distance in that case
    /// is simply dropped for this tick; `dt` is clamped to `0..` first, so
    /// a negative caller value cannot run this backward.
    pub fn advance(&mut self, dt: f32) {
        self.advance_firing(dt, &mut Vec::new());
    }

    /// [`Self::advance`], additionally appending the documented fire-on-pass
    /// `message` of every node the train passes this step to `fired`, in
    /// the order they were passed. `ohl-game`'s own map-logic simulation
    /// fires each one by name; see
    /// [`crate::logic::Simulation::advance_trains`].
    pub fn advance_firing(&mut self, dt: f32, fired: &mut Vec<String>) {
        if !self.moving {
            return;
        }
        if self.wait_timer > 0.0 {
            self.wait_timer = (self.wait_timer - dt.max(0.0)).max(0.0);
            if self.wait_timer > 0.0 {
                return;
            }
        }
        let mut remaining = self.speed.max(0.0) * dt.max(0.0);
        let mut transitions = 0;
        while remaining > 0.0 && self.moving && transitions < MAX_TRANSITIONS_PER_TICK {
            let Some(other) = self.other_index() else {
                // A dead end: the chain has no node ahead in this
                // direction. The train comes to rest here, and the node's
                // documented `netname` ("fire on dead end") fires — once
                // per arrival, however many steps the train then sits
                // here for.
                self.moving = false;
                if !self.dead_end_fired {
                    self.dead_end_fired = true;
                    if let Some(dead_end) = self.chain.nodes[self.node_index].dead_end.as_ref() {
                        fired.push(dead_end.clone());
                    }
                }
                break;
            };
            let len = self.chain.segment_len(self.node_index, other);
            let remaining_in_segment = (1.0 - self.t.clamp(0.0, 1.0)) * len;
            if len <= f32::EPSILON || remaining >= remaining_in_segment {
                remaining -= remaining_in_segment.max(0.0);
                self.node_index = other;
                self.t = 0.0;
                self.dead_end_fired = false;
                transitions += 1;
                let node = &self.chain.nodes[self.node_index];
                if let Some(message) = node.message.as_ref() {
                    fired.push(message.clone());
                }
                let node = PassedNode {
                    speed: node.speed,
                    stop: node.stop,
                    wait: node.wait,
                };
                if let Some(speed) = node.speed {
                    self.speed = speed.abs();
                }
                if node.stop {
                    self.moving = false;
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
}

/// Builds a [`TrackTrainState`] for every `func_train`/`func_tracktrain`
/// entity whose `target` (first path node) resolves to a `path_corner`/
/// `path_track` chain, and inserts it as a component alongside the
/// entity's existing [`TrackTrain`]. Called once, after every entity (and
/// so the whole `targetname` index) exists, since a train's first node
/// commonly appears later in the entities lump than the train itself.
pub fn spawn_all(registry: &mut Registry) {
    let candidates: Vec<(Entity, TrackTrain, String)> = registry
        .world
        .query::<(Entity, &TrackTrain, &Target)>()
        .iter()
        .map(|(entity, train, target)| (entity, *train, target.0.clone()))
        .collect();
    for (entity, train, first_name) in candidates {
        if let Some(chain) = PathChain::build(registry, &first_name, train.height) {
            let state = TrackTrainState::spawn(&train, chain);
            registry.world.insert_one(entity, state).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::Registry;
    use ohl_formats::bsp30::Entity as RawEntity;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// Bounding-box tolerance for the polyline-containment proptest, to
    /// absorb ordinary `f32` accumulation over many fixed-timestep
    /// advances rather than requiring bit-exact containment.
    const POLYLINE_SLACK: f32 = 1e-2;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// A synthetic three-node straight track (`node1` -> `node2` -> `node3`,
    /// 100 units apart along `+X`) plus one `func_tracktrain` targeting
    /// `node1`. All keyvalues are authored for this test; none are
    /// derived from any payload.
    fn three_node_track(train_extra: &[(&str, &str)]) -> Vec<RawEntity> {
        let mut train_kv = vec![
            ("classname", "func_tracktrain"),
            ("targetname", "tram"),
            ("target", "node1"),
            // Zeroed so straight-line position assertions do not also have
            // to account for the default `height` keyvalue; see
            // `height_offsets_every_node` for that behaviour specifically.
            ("height", "0"),
        ];
        train_kv.extend_from_slice(train_extra);
        vec![
            raw(&train_kv),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node1"),
                ("target", "node2"),
                ("origin", "0 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node2"),
                ("target", "node3"),
                ("origin", "100 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node3"),
                ("origin", "200 0 0"),
            ]),
        ]
    }

    /// A synthetic bent track (`node1` at the origin, `node2` 100 units
    /// along `+X`, `node3` 100 units further along `+Y`, a non-looped dead
    /// end) plus one `func_tracktrain` targeting `node1`. Used to check
    /// what yaw a train reports once it has run off the end of its chain
    /// and parked, as opposed to [`three_node_track`]'s collinear layout,
    /// where a stale `None` and the correct persisted heading would
    /// coincidentally both round-trip through `Some(0.0)`.
    fn bent_track(train_extra: &[(&str, &str)]) -> Vec<RawEntity> {
        let mut train_kv = vec![
            ("classname", "func_tracktrain"),
            ("targetname", "tram"),
            ("target", "node1"),
            ("height", "0"),
        ];
        train_kv.extend_from_slice(train_extra);
        vec![
            raw(&train_kv),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node1"),
                ("target", "node2"),
                ("origin", "0 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node2"),
                ("target", "node3"),
                ("origin", "100 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node3"),
                ("origin", "100 100 0"),
            ]),
        ]
    }

    fn build_registry(entities: &[RawEntity]) -> Registry {
        let defs = parse_entities(entities, &Limits::default());
        Registry::build(&defs, &BTreeMap::new(), &Limits::default())
    }

    fn train_state(registry: &Registry) -> TrackTrainState {
        let entity = registry.find("tram")[0];
        let state = registry
            .world
            .get::<&TrackTrainState>(entity)
            .expect("track train state");
        (*state).clone()
    }

    fn train_component(registry: &Registry) -> TrackTrain {
        let entity = registry.find("tram")[0];
        *registry
            .world
            .get::<&TrackTrain>(entity)
            .expect("track train")
    }

    /// Float comparison with slack for positions reached by accumulating
    /// many small fixed-timestep advances, rather than by an exact
    /// node-boundary snap.
    fn assert_close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 1e-2,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn spawns_on_first_node_facing_the_second() {
        let entities = three_node_track(&[]);
        let registry = build_registry(&entities);
        let state = train_state(&registry);
        let train = train_component(&registry);
        assert_eq!(state.position(), Vec3::ZERO);
        assert_eq!(state.yaw_degrees(&train), Some(0.0));
    }

    /// A train is placed on the *first node of its own path* at spawn:
    /// [`TrackTrainState::position`] is that node, whatever the map
    /// authored the train's brushes at. `ohl-engine`'s
    /// `track_train_transform` turns that into a placement offset by
    /// subtracting the entity's own `origin` keyvalue — the origin-brush
    /// position the compiler wrote there, and the frame the submodel's
    /// geometry is stored relative to — so the sum every caller already
    /// forms (`origin` + offset) cancels to this absolute position exactly
    /// once.
    ///
    /// This also keeps fidelity round 2 finding E1 fixed: the raw polyline
    /// coordinate must never be returned as an offset, since the caller
    /// would then add the `origin` keyvalue to it a second time.
    #[test]
    fn a_train_is_placed_on_its_first_node_at_spawn() {
        let entities = three_node_track(&[]);
        let registry = build_registry(&entities);
        let state = train_state(&registry);
        assert_eq!(
            state.position(),
            Vec3::ZERO,
            "a train that has not moved must sit on the first node of its own path"
        );
    }

    #[test]
    fn position_tracks_travel_along_the_chain() {
        let entities = three_node_track(&[("speed", "50")]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        // Halfway from node1 (0,0,0) to node2 (100,0,0): 50 units at
        // 50 units/sec = 1s.
        for _ in 0..100 {
            state.advance(0.01);
        }
        assert_close(state.position(), Vec3::new(50.0, 0.0, 0.0));
    }

    #[test]
    fn height_offsets_every_node() {
        let entities = three_node_track(&[("height", "16")]);
        let registry = build_registry(&entities);
        let state = train_state(&registry);
        assert_eq!(state.position(), Vec3::new(0.0, 0.0, 16.0));
    }

    #[test]
    fn moves_along_the_chain_and_stops_at_the_dead_end() {
        let entities = three_node_track(&[("speed", "50")]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        // 100 units at 50 units/sec = 2s to the first node, 4s total to the
        // dead end at node3.
        for _ in 0..500 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(200.0, 0.0, 0.0));
        assert!(!state.moving);
    }

    #[test]
    fn node_wait_pauses_then_auto_continues() {
        let mut entities = three_node_track(&[("speed", "100")]);
        // node2 (index 2 in the vec) gets a 1-second wait.
        entities[2].insert("wait".to_string(), "1".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        // 100 units at 100/sec = 1s to reach node2; a handful of extra
        // ticks cover the fixed-point slack of summing many `0.01`s.
        for _ in 0..105 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(100.0, 0.0, 0.0));
        assert!(state.moving);
        // Still paused just before the wait elapses.
        for _ in 0..89 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(100.0, 0.0, 0.0));
        // The wait elapses and the train resumes without being triggered.
        for _ in 0..300 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(200.0, 0.0, 0.0));
    }

    /// A train that a node's `speed` override slowed down, then stopped,
    /// resumes at its own `speed` when it is started again — not at the
    /// override the stop left behind (see [`TrackTrainState::turn_on`]).
    #[test]
    fn a_restarted_train_resumes_at_its_own_speed() {
        let mut entities = three_node_track(&[("speed", "300")]);
        entities[2].insert("speed".to_string(), "100".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        let train = train_component(&registry);
        state.turn_on();
        assert!((state.speed - train.speed).abs() < f32::EPSILON);

        // Cross the first node, which reassigns the train's speed.
        state.advance(2.0);
        assert!(
            state.speed < train.speed,
            "the node override should have slowed the train, got {}",
            state.speed
        );

        state.turn_off();
        state.turn_on();
        assert!(
            (state.speed - train.speed).abs() < f32::EPSILON,
            "a restarted train should resume at its own speed, got {}",
            state.speed
        );
    }

    /// A `speed 0` train that only moves under its `startspeed` (or a
    /// `path_track` override) must not be snapped to `speed 0` the moment
    /// it is stopped and re-triggered: that would set `moving = true` at
    /// `speed 0`, which never advances and parks the train forever. This
    /// pins the exact probe from the PR #146 review: halted at node index
    /// 1 with `speed == 300` (a `func_tracktrain`'s own "Maximum speed"
    /// keyvalue does not override a resumed train's carried speed; see
    /// [`TrackTrainState::turn_on`]), toggling it back on must resume at
    /// that same 300, not reset to the train's `speed 0` keyvalue.
    #[test]
    fn a_zero_speed_train_restarts_at_its_carried_speed_not_zero() {
        let mut entities = three_node_track(&[("speed", "0"), ("startspeed", "300")]);
        // node2's "Wait for retrigger" spawnflag.
        entities[2].insert("spawnflags".to_string(), "1".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);

        // `startspeed` alone puts the train in motion at spawn.
        assert!(state.moving);
        assert!((state.speed - 300.0).abs() < f32::EPSILON);

        for _ in 0..50 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(100.0, 0.0, 0.0));
        assert!(!state.moving);
        assert_eq!(state.node_index, 1);
        assert!(
            (state.speed - 300.0).abs() < f32::EPSILON,
            "halting should not touch the carried speed, got {}",
            state.speed
        );

        state.toggle();
        assert!(state.moving);
        assert!(
            (state.speed - 300.0).abs() < f32::EPSILON,
            "a speed-0 train resumed from a stop must keep its carried \
             speed of 300, not reset to its own speed 0, got {}",
            state.speed
        );

        for _ in 0..50 {
            state.advance(0.01);
        }
        assert_eq!(
            state.position(),
            Vec3::new(200.0, 0.0, 0.0),
            "the train must keep advancing past node 1 instead of parking there"
        );
    }

    #[test]
    fn stop_flag_halts_until_toggled() {
        let mut entities = three_node_track(&[("speed", "100")]);
        // node2's "Wait for retrigger" spawnflag.
        entities[2].insert("spawnflags".to_string(), "1".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        for _ in 0..200 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(100.0, 0.0, 0.0));
        assert!(!state.moving);
        // Advancing further does nothing until re-triggered.
        for _ in 0..500 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(100.0, 0.0, 0.0));
        state.toggle();
        for _ in 0..105 {
            state.advance(0.01);
        }
        assert_eq!(state.position(), Vec3::new(200.0, 0.0, 0.0));
    }

    #[test]
    fn path_track_speed_override_takes_effect_at_the_node() {
        let mut entities = three_node_track(&[("speed", "100")]);
        // node2 (index 2) slows the train to 10/sec as it passes, so the
        // node2 -> node3 leg takes 10s instead of the 1s the node1 -> node2
        // leg (at the un-overridden 100/sec) took.
        entities[2].insert("speed".to_string(), "10".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        for _ in 0..100 {
            state.advance(0.01);
        }
        assert_close(state.position(), Vec3::new(100.0, 0.0, 0.0));
        // Without the override this leg would also finish in ~1s (100 more
        // steps); confirm it has not, proving node2's override took effect.
        for _ in 0..100 {
            state.advance(0.01);
        }
        assert!(state.position().x < 150.0);
        for _ in 0..950 {
            state.advance(0.01);
        }
        assert_close(state.position(), Vec3::new(200.0, 0.0, 0.0));
    }

    /// The documented `path_track` default for "New Train Speed" is `0`,
    /// and `0` is documented to mean *no* speed change (see
    /// [`path_speed_override`]). A node carrying it must therefore be
    /// passed through at the train's current speed.
    ///
    /// Regression: read literally, a zero override set the train's speed
    /// to zero, which parked it at that node forever — it stayed "moving"
    /// but covered no distance, so every node after it, and anything
    /// waiting for the train to arrive there, was unreachable. A ride that
    /// is meant to carry its passenger through a level boundary simply
    /// stopped short of it.
    #[test]
    fn a_zero_speed_node_is_no_override_and_does_not_park_the_train() {
        let mut entities = three_node_track(&[("speed", "100")]);
        // node2 (index 2) carries the keyvalue's own default value.
        entities[2].insert("speed".to_string(), "0".to_string());
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        // 200 units at the un-overridden 100/sec is 2s; give it 3s.
        for _ in 0..300 {
            state.advance(0.01);
        }
        assert_close(state.position(), Vec3::new(200.0, 0.0, 0.0));
        assert!(
            !state.moving,
            "the train must have reached the chain's dead end, not stalled at node2"
        );
    }

    /// The same fact one level down: the parsed node carries no override at
    /// all, rather than an override of zero.
    #[test]
    fn a_zero_speed_keyvalue_parses_as_no_override() {
        let mut entities = three_node_track(&[("speed", "100")]);
        entities[2].insert("speed".to_string(), "0".to_string());
        entities[3].insert("speed".to_string(), "25".to_string());
        let registry = build_registry(&entities);
        let state = train_state(&registry);
        assert_eq!(state.chain.nodes[1].speed, None);
        assert_eq!(state.chain.nodes[2].speed, Some(25.0));
    }

    #[test]
    fn path_speed_override_reads_zero_and_non_finite_as_no_override() {
        assert_eq!(path_speed_override(0.0), None);
        assert_eq!(path_speed_override(-0.0), None);
        assert_eq!(path_speed_override(f32::NAN), None);
        assert_eq!(path_speed_override(f32::INFINITY), None);
        assert_eq!(path_speed_override(250.0), Some(250.0));
    }

    #[test]
    fn reverse_preserves_position_and_walks_back() {
        let entities = three_node_track(&[("speed", "50")]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        for _ in 0..100 {
            state.advance(0.01);
        }
        let midpoint = state.position();
        assert_close(midpoint, Vec3::new(50.0, 0.0, 0.0));
        state.reverse();
        assert_close(state.position(), midpoint);
        for _ in 0..150 {
            state.advance(0.01);
        }
        assert_close(state.position(), Vec3::ZERO);
        assert!(!state.moving);
    }

    #[test]
    fn looped_chain_wraps_instead_of_dead_ending() {
        let entities = vec![
            raw(&[
                ("classname", "func_tracktrain"),
                ("targetname", "tram"),
                ("target", "a"),
                ("speed", "100"),
                ("height", "0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "a"),
                ("target", "b"),
                ("origin", "0 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "b"),
                ("target", "c"),
                ("origin", "100 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "c"),
                ("target", "a"),
                ("origin", "200 0 0"),
            ]),
        ];
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        // One full lap (a -> b -> c -> a) is 400 units, 4s at 100/sec; a
        // non-looped chain would instead have dead-ended at "c" after just
        // 200 units (2s). Run for three laps' worth of time and confirm the
        // train is still moving and still sitting on the polyline.
        for _ in 0..1200 {
            state.advance(0.01);
        }
        assert!(state.moving);
        let position = state.position();
        assert!(position.y.abs() < 1e-3 && position.z.abs() < 1e-3);
        assert!((0.0..=200.0).contains(&position.x));
    }

    #[test]
    fn func_train_does_not_turn_to_face() {
        let mut entities = three_node_track(&[]);
        entities[0].insert("classname".to_string(), "func_train".to_string());
        let registry = build_registry(&entities);
        let train = train_component(&registry);
        let state = train_state(&registry);
        assert!(!train.turns_to_face);
        assert_eq!(state.yaw_degrees(&train), None);
    }

    /// A train that runs off the end of a non-looped chain parks facing
    /// the way it was already heading — the last segment it actually
    /// travelled, `node2` -> `node3` here, 90 degrees — rather than
    /// reporting no heading at all and snapping its drawn/collision pose
    /// back to whatever `angles` it spawned at. This is
    /// [`TrackTrainState::yaw_degrees`]'s own persisted-yaw fallback;
    /// `crates/ohl-engine/tests/track_train_bend.rs` checks the same fact
    /// end to end, including that a rider is not left stranded when the
    /// hull stops turning under them.
    #[test]
    fn a_train_parked_at_the_end_of_a_bend_keeps_its_last_heading() {
        let entities = bent_track(&[("speed", "1000"), ("startspeed", "1000")]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        let train = train_component(&registry);
        state.turn_on();
        // Far more distance than the 200-unit chain covers, so the train
        // is guaranteed to have run off the end and stopped.
        state.advance(10.0);
        assert!(
            !state.moving,
            "the train should have parked at the dead end"
        );
        assert_eq!(state.position(), Vec3::new(100.0, 100.0, 0.0));
        assert_eq!(
            state.yaw_degrees(&train),
            Some(90.0),
            "a parked train must keep the heading of the segment it arrived on"
        );
    }

    #[test]
    fn no_user_control_flag_is_recorded() {
        assert!(TrackTrain::no_user_control_from_flags(2));
        assert!(!TrackTrain::no_user_control_from_flags(0));
    }

    #[test]
    fn dynamic_state_round_trips_mid_segment() {
        let entities = three_node_track(&[("speed", "100")]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.turn_on();
        state.advance(0.4);
        let captured = state.dynamic_state();

        // A fresh state, as a load would rebuild before restoring.
        let mut restored = train_state(&registry);
        restored.restore_dynamic_state(
            captured.0, captured.1, captured.2, captured.3, captured.4, captured.5,
        );
        assert_eq!(restored.dynamic_state(), captured);
        assert_eq!(restored.position(), state.position());
    }

    #[test]
    fn restore_dynamic_state_clamps_a_node_index_past_the_rebuilt_chain() {
        let entities = three_node_track(&[]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.restore_dynamic_state(9_999, 0.5, 1.0, 50.0, true, 0.0);
        assert!(state.node_index < state.chain.nodes.len());
    }

    #[test]
    fn restore_dynamic_state_sanitizes_non_finite_input() {
        let entities = three_node_track(&[]);
        let registry = build_registry(&entities);
        let mut state = train_state(&registry);
        state.restore_dynamic_state(0, f32::NAN, f32::NAN, f32::NAN, true, f32::NAN);
        let (_, t, direction, speed, _, wait_timer) = state.dynamic_state();
        assert_eq!(t.to_bits(), 0.0f32.to_bits());
        assert!((direction - 1.0).abs() < f32::EPSILON);
        assert_eq!(speed.to_bits(), 0.0f32.to_bits());
        assert_eq!(wait_timer.to_bits(), 0.0f32.to_bits());
    }

    proptest! {
        /// However a chain is shaped (arbitrary, possibly-coincident node
        /// positions; looped or not) and however a train is driven along
        /// it (arbitrary speed, arbitrary sequence of fixed-timestep
        /// advances), the position it reports is always a convex
        /// combination of two adjacent chain nodes and so always lies
        /// within the bounding box of every node's own position, and is
        /// never `NaN`/infinite.
        #[test]
        fn position_stays_on_the_polyline_and_finite(
            coords in prop::collection::vec(
                (-1000.0f32..1000.0f32, -1000.0f32..1000.0f32, -1000.0f32..1000.0f32),
                2..8,
            ),
            looped in prop::bool::ANY,
            forward in prop::bool::ANY,
            speed in 0.0f32..500.0,
            steps in prop::collection::vec(0.0f32..0.5, 0..200),
        ) {
            let mut world = hecs::World::new();
            let nodes: Vec<PathNode> = coords
                .iter()
                .map(|&(x, y, z)| PathNode {
                    entity: world.spawn(()),
                    position: Vec3::new(x, y, z),
                    wait: 0.0,
                    speed: None,
                    stop: false,
                    message: None,
                    dead_end: None,
                })
                .collect();
            let min = coords.iter().fold(Vec3::splat(f32::MAX), |acc, &(x, y, z)| {
                acc.min(Vec3::new(x, y, z))
            });
            let max = coords.iter().fold(Vec3::splat(f32::MIN), |acc, &(x, y, z)| {
                acc.max(Vec3::new(x, y, z))
            });
            let mut state = TrackTrainState {
                chain: PathChain { nodes, looped },
                node_index: 0,
                t: 0.0,
                direction: if forward { 1.0 } else { -1.0 },
                speed,
                moving: true,
                wait_timer: 0.0,
                dead_end_fired: false,
                carry_offset: Vec3::ZERO,
                carry_yaw: 0.0,
                first_node: Vec3::ZERO,
                cruise_speed: speed,
            };
            for dt in steps {
                state.advance(dt);
                let position = state.position();
                prop_assert!(position.is_finite());
                prop_assert!(
                    position.x >= min.x - POLYLINE_SLACK && position.x <= max.x + POLYLINE_SLACK
                );
                prop_assert!(
                    position.y >= min.y - POLYLINE_SLACK && position.y <= max.y + POLYLINE_SLACK
                );
                prop_assert!(
                    position.z >= min.z - POLYLINE_SLACK && position.z <= max.z + POLYLINE_SLACK
                );
            }
        }
    }
}
