//! `monster_bigmomma` (the Gonarch): the `info_bigmomma` trail and its
//! health-per-node progression.
//!
//! ## What is published (see `docs/FORMAT_SOURCES.md`, "Monster
//! definitions", "Wave 1 batch B")
//!
//! - `TWHL:Monster_bigmomma`: its behaviour "can be controlled by a trail of
//!   `info_bigmomma` nodes. The Gonarch will be invincible as long as it's
//!   still 'on the trail': applying enough damage will make it move to the
//!   next node." On reaching a node with a health value, "the Gonarch's
//!   health will be set to that value and it will start fighting the player
//!   again. It can only be killed at the end of the trail, when there is no
//!   next node to move to." The first node is named by the monster's
//!   `netname` keyvalue ([`FIRST_NODE_KEY`]).
//! - `TWHL:Info_bigmomma`: `target` ("Next node in path. If this node
//!   specifies a health value, then big momma will only continue to the next
//!   node after its health has been depleted"), `reachdelay` ("Time spent at
//!   node"), `reachtarget` ("Target to activate on reaching the node"),
//!   `reachsequence` ("The name of a scripted_sequence to use on approach"),
//!   `health` ("Sets big momma's health to the specified value"),
//!   `killtarget`, and the spawnflags `Run to Node` (1) and `Wait
//!   Indefinitely` (2).
//!
//! ## This project's reading of the gaps
//!
//! - A node's `health` is scaled by the same published per-difficulty
//!   factor as the monster's own base health
//!   (`table::BIGMOMMA_HEALTH_FACTOR`); the page gives the factor for the
//!   base and nothing for the nodes, so applying it to both is a project
//!   decision. `TODO(black-box)`.
//! - Depleting a node's health does not leave the Gonarch at zero: it
//!   leaves for the next node shielded, and its health is restored to its
//!   spawn value so a later node *without* a health value fights at the
//!   base figure rather than at one hit point. `TODO(black-box)`.
//! - A node with no health value that is not the last is passed through
//!   after `reachdelay`, still shielded; `Wait Indefinitely` holds the
//!   Gonarch there until [`GonarchTrail::release`] or, on a node with
//!   health, until that health is depleted. What calls `release` is a
//!   `use` of the Gonarch's own name (`ohl_game::registry::
//!   MonsterActivation`, drained by `crate::monsters::bosses`): the flag
//!   has no published description at all, and a wait nothing could end
//!   would leave a map's Gonarch shielded for good. `TODO(black-box)`.
//! - How close counts as "reaching" a node ([`TRAIL_ARRIVAL_RADIUS`]) is a
//!   placeholder.
//! - A leg that stalls counts as arriving: when the Gonarch has gained no
//!   ground on the node it is travelling to (at least
//!   [`TRAIL_PROGRESS_STEP`] units nearer than its best so far) for
//!   [`TRAIL_STALL_SECONDS`], [`GonarchTrail::note_progress`] says so and
//!   the driver treats it as reaching the node: the node's effects fire and
//!   its health is set where the Gonarch stands. Project-authored, not a
//!   published rule: nothing published says what a Gonarch does when it
//!   cannot reach its next node, but one that waited for it forever would
//!   stay shielded, and so unkillable, for good. Arriving rather than
//!   skipping the node keeps the node's `reachtarget` firing, which a map
//!   may need to go on. The stall clock is not saved: a load starts it
//!   again from zero.
//!
//! The trail is built once from the map's entity definitions
//! ([`Trail::from_defs`]), bounded by [`MAX_TRAIL_NODES`] and loop-safe.
//! Nothing here moves the monster: [`crate::monsters::bosses`] turns the
//! current [`TrailPhase`] into a route and a condition every tick, and
//! [`crate::monsters::lifecycle`] asks [`GonarchTrail::absorb_damage`]
//! before any hit costs health.

use glam::Vec3;
use ohl_game::EntityDef;

/// The `info_bigmomma` classname.
pub const NODE_CLASSNAME: &str = "info_bigmomma";

/// The `monster_bigmomma` keyvalue naming its first node (published as the
/// monster's `netname`, which for this one kind is therefore *not* a squad
/// name).
pub const FIRST_NODE_KEY: &str = "netname";

/// `info_bigmomma`'s published `Run to Node` spawnflag.
pub const SPAWNFLAG_NODE_RUN: u32 = 1;

/// `info_bigmomma`'s published `Wait Indefinitely` spawnflag.
pub const SPAWNFLAG_NODE_WAIT_INDEFINITELY: u32 = 2;

/// The most nodes one trail may hold, however a map chains them.
pub const MAX_TRAIL_NODES: usize = 64;

/// How close (horizontally, in world units) the Gonarch must get to a node
/// to count as having reached it. **`TODO(black-box)`**: project placeholder.
pub const TRAIL_ARRIVAL_RADIUS: f32 = 48.0;

/// How long, in seconds, a travel leg may go without progress before it
/// counts as arriving (see the module doc comment). Project-authored.
pub const TRAIL_STALL_SECONDS: f32 = 5.0;

/// How much nearer (horizontally, in world units) than its best so far the
/// Gonarch must get to count as progress on a leg. Project-authored.
pub const TRAIL_PROGRESS_STEP: f32 = 16.0;

/// One `info_bigmomma` node, read from its keyvalues.
#[derive(Debug, Clone, PartialEq)]
pub struct TrailNode {
    /// The node's `targetname`.
    pub name: String,
    /// The node's world position.
    pub position: Vec3,
    /// `target`: the next node's name, when there is one.
    pub next: Option<String>,
    /// `health`: the health set on approach, unscaled, when the node gives
    /// one (a non-finite or non-positive value is read as none).
    pub health: Option<f32>,
    /// `reachdelay`: seconds spent at the node after arriving.
    pub wait: f32,
    /// `reachtarget`: fired by name on arrival.
    pub fire_on_reach: Option<String>,
    /// `killtarget`: removed by name on arrival.
    pub kill_on_reach: Option<String>,
    /// `reachsequence`: the `scripted_sequence` to run on arrival.
    pub sequence_on_reach: Option<String>,
    /// The `Run to Node` spawnflag.
    pub run: bool,
    /// The `Wait Indefinitely` spawnflag.
    pub wait_indefinitely: bool,
}

impl TrailNode {
    /// Reads one node off `def`'s keyvalues; `None` when it is not an
    /// `info_bigmomma` or has no `targetname`.
    #[must_use]
    pub fn from_def(def: &EntityDef) -> Option<Self> {
        if def.classname != NODE_CLASSNAME {
            return None;
        }
        let name = def.targetname.as_deref().map(str::trim)?;
        if name.is_empty() {
            return None;
        }
        let string = |key: &str| {
            def.keyvalues
                .get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let number = |key: &str| {
            def.keyvalues
                .get(key)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|value| value.is_finite())
        };
        Some(Self {
            name: name.to_string(),
            position: Vec3::from_array(def.origin),
            next: string("target"),
            health: number("health").filter(|health| *health > 0.0),
            wait: number("reachdelay").unwrap_or(0.0).max(0.0),
            fire_on_reach: string("reachtarget"),
            kill_on_reach: string("killtarget"),
            sequence_on_reach: string("reachsequence"),
            run: def.spawnflags & SPAWNFLAG_NODE_RUN != 0,
            wait_indefinitely: def.spawnflags & SPAWNFLAG_NODE_WAIT_INDEFINITELY != 0,
        })
    }
}

/// A resolved chain of [`TrailNode`]s, first node first.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Trail {
    nodes: Vec<TrailNode>,
}

impl Trail {
    /// A trail over `nodes` as given, truncated to [`MAX_TRAIL_NODES`].
    #[must_use]
    pub fn new(nodes: Vec<TrailNode>) -> Self {
        let mut nodes = nodes;
        nodes.truncate(MAX_TRAIL_NODES);
        Self { nodes }
    }

    /// Walks `first` and each node's `target` through `defs` into a trail.
    /// Stops at a node whose `target` names nothing, at a node already on
    /// the trail (a loop), or at [`MAX_TRAIL_NODES`]. `None` when `first`
    /// names no `info_bigmomma`.
    #[must_use]
    pub fn from_defs(defs: &[EntityDef], first: &str) -> Option<Self> {
        let mut nodes: Vec<TrailNode> = Vec::new();
        let mut current = first.trim().to_string();
        while nodes.len() < MAX_TRAIL_NODES && !current.is_empty() {
            if nodes.iter().any(|node| node.name == current) {
                break;
            }
            let Some(node) = defs
                .iter()
                .filter(|def| def.classname == NODE_CLASSNAME)
                .find(|def| def.targetname.as_deref().map(str::trim) == Some(current.as_str()))
                .and_then(TrailNode::from_def)
            else {
                break;
            };
            let next = node.next.clone();
            nodes.push(node);
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        if nodes.is_empty() {
            None
        } else {
            Some(Self { nodes })
        }
    }

    /// The nodes, first first.
    #[must_use]
    pub fn nodes(&self) -> &[TrailNode] {
        &self.nodes
    }

    /// The number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the trail has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether `index` is the trail's last node — the only place the
    /// Gonarch can be killed.
    #[must_use]
    pub fn is_last(&self, index: usize) -> bool {
        index + 1 >= self.nodes.len()
    }
}

/// Where the Gonarch is along its trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrailPhase {
    /// Moving toward node `to`; shielded.
    Traveling {
        /// The node being moved to.
        to: usize,
    },
    /// Just reached node `at`, sitting out its `reachdelay`; shielded.
    Arriving {
        /// The node reached.
        at: usize,
        /// Seconds of `reachdelay` left.
        wait_left: f32,
    },
    /// Standing at node `at` after its delay: fighting down the node's
    /// health when it has one (shielded until then only if the node is not
    /// the last), or holding for an indefinite wait.
    Holding {
        /// The node held at.
        at: usize,
    },
    /// No trail, or the trail is over: an ordinary, killable monster.
    Free,
}

/// What reaching a node asks the host to do.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArrivalEffects {
    /// The health to set, already scaled by the difficulty factor.
    pub set_health: Option<f32>,
    /// The `reachtarget` to fire by name.
    pub fire: Option<String>,
    /// The `killtarget` to remove by name.
    pub kill: Option<String>,
    /// The `reachsequence` to run.
    pub sequence: Option<String>,
}

/// What one hit does, given the trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageVerdict {
    /// The trail keeps the hit from costing any health.
    Shielded,
    /// The hit costs health normally (the trail's end, or no trail).
    Applied,
    /// The hit depleted this node's health: the Gonarch shrugs the rest
    /// off, has its health restored to its spawn value, and leaves for
    /// `next`.
    Depleted {
        /// The node it now travels to.
        next: usize,
    },
}

/// The per-monster trail state: a component on a `monster_bigmomma`.
#[derive(Debug, Clone, PartialEq)]
pub struct GonarchTrail {
    trail: Trail,
    phase: TrailPhase,
    /// The leg the stall clock is measuring, its nearest approach so far,
    /// and the seconds since that last improved.
    stall: Option<(usize, f32, f32)>,
    /// The published per-difficulty factor a node's `health` is scaled by.
    health_factor: f32,
    /// The health the monster spawned with, restored on depletion.
    base_health: f32,
}

impl GonarchTrail {
    /// A Gonarch about to set off for `trail`'s first node (or free at
    /// once when the trail is empty), scaling node health by
    /// `health_factor` and restoring `base_health` on depletion.
    #[must_use]
    pub fn new(trail: Trail, health_factor: f32, base_health: f32) -> Self {
        let phase = if trail.is_empty() {
            TrailPhase::Free
        } else {
            TrailPhase::Traveling { to: 0 }
        };
        Self {
            trail,
            phase,
            stall: None,
            health_factor: if health_factor.is_finite() && health_factor > 0.0 {
                health_factor
            } else {
                1.0
            },
            base_health: if base_health.is_finite() && base_health > 0.0 {
                base_health
            } else {
                1.0
            },
        }
    }

    /// The trail itself.
    #[must_use]
    pub fn trail(&self) -> &Trail {
        &self.trail
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> TrailPhase {
        self.phase
    }

    /// The health restored when a node's health is depleted.
    #[must_use]
    pub fn base_health(&self) -> f32 {
        self.base_health
    }

    /// The node position currently being travelled to, if travelling.
    #[must_use]
    pub fn destination(&self) -> Option<Vec3> {
        match self.phase {
            TrailPhase::Traveling { to } => self.trail.nodes.get(to).map(|node| node.position),
            _ => None,
        }
    }

    /// Whether the leg being travelled carries the `Run to Node` flag.
    #[must_use]
    pub fn runs_to_destination(&self) -> bool {
        match self.phase {
            TrailPhase::Traveling { to } => self.trail.nodes.get(to).is_some_and(|node| node.run),
            _ => false,
        }
    }

    /// Whether the trail currently keeps hits from costing health.
    #[must_use]
    pub fn is_shielded(&self) -> bool {
        match self.phase {
            TrailPhase::Traveling { .. } | TrailPhase::Arriving { .. } => true,
            TrailPhase::Holding { at } => {
                !self.trail.is_last(at) && self.node(at).is_none_or(|node| node.health.is_none())
            }
            TrailPhase::Free => false,
        }
    }

    fn node(&self, index: usize) -> Option<&TrailNode> {
        self.trail.nodes.get(index)
    }

    /// Whether `position` has reached the node being travelled to.
    #[must_use]
    pub fn has_reached_destination(&self, position: Vec3) -> bool {
        self.destination().is_some_and(|destination| {
            let delta = Vec3::new(destination.x - position.x, destination.y - position.y, 0.0);
            delta.length() <= TRAIL_ARRIVAL_RADIUS
        })
    }

    /// Measures progress on the leg being travelled, `dt` seconds after the
    /// last call, with the Gonarch at `position`. Returns `true` once the
    /// leg has gone [`TRAIL_STALL_SECONDS`] without getting
    /// [`TRAIL_PROGRESS_STEP`] nearer to its node than its best so far:
    /// the leg has stalled, and the caller should treat it as arriving.
    /// Always `false` while not travelling; a new leg starts the clock
    /// again.
    pub fn note_progress(&mut self, position: Vec3, dt: f32) -> bool {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        let (TrailPhase::Traveling { to }, Some(destination)) = (self.phase, self.destination())
        else {
            self.stall = None;
            return false;
        };
        let distance =
            Vec3::new(destination.x - position.x, destination.y - position.y, 0.0).length();
        if !distance.is_finite() {
            return false;
        }
        let (best, stalled) = match self.stall {
            Some((leg, best, stalled)) if leg == to => {
                if distance <= best - TRAIL_PROGRESS_STEP {
                    (distance, 0.0)
                } else {
                    (best, stalled + dt)
                }
            }
            _ => (distance, 0.0),
        };
        self.stall = Some((to, best, stalled));
        stalled >= TRAIL_STALL_SECONDS
    }

    /// Records arrival at the node being travelled to and returns what the
    /// host should do about it. `None` when not travelling.
    pub fn arrive(&mut self) -> Option<ArrivalEffects> {
        let TrailPhase::Traveling { to } = self.phase else {
            return None;
        };
        self.stall = None;
        let node = self.node(to)?;
        let effects = ArrivalEffects {
            set_health: node.health.map(|health| health * self.health_factor),
            fire: node.fire_on_reach.clone(),
            kill: node.kill_on_reach.clone(),
            sequence: node.sequence_on_reach.clone(),
        };
        if node.wait > 0.0 {
            self.phase = TrailPhase::Arriving {
                at: to,
                wait_left: node.wait,
            };
        } else {
            self.phase = TrailPhase::Holding { at: to };
            // No delay and nothing to hold it: straight on to the next leg.
            self.depart_if_passable(to);
        }
        Some(effects)
    }

    /// Advances the post-arrival wait by `dt` seconds. Returns the node
    /// index the Gonarch set off for, when the wait ending sent it on.
    pub fn tick(&mut self, dt: f32) -> Option<usize> {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        let TrailPhase::Arriving { at, wait_left } = self.phase else {
            return None;
        };
        let left = wait_left - dt;
        if left > 0.0 {
            self.phase = TrailPhase::Arriving {
                at,
                wait_left: left,
            };
            return None;
        }
        self.phase = TrailPhase::Holding { at };
        self.depart_if_passable(at)
    }

    /// Leaves node `at` for the next one when nothing holds the Gonarch
    /// there: no health to fight down, no indefinite wait, and a next node
    /// to go to.
    fn depart_if_passable(&mut self, at: usize) -> Option<usize> {
        let node = self.node(at)?;
        if node.health.is_some() || node.wait_indefinitely || self.trail.is_last(at) {
            return None;
        }
        let next = at + 1;
        self.phase = TrailPhase::Traveling { to: next };
        Some(next)
    }

    /// Ends an indefinite wait (an external trigger): the Gonarch leaves
    /// for the next node. Returns the node it set off for, or `None` when
    /// it was not holding, the node still has health to fight down, or
    /// there is no next node.
    pub fn release(&mut self) -> Option<usize> {
        let TrailPhase::Holding { at } = self.phase else {
            return None;
        };
        let node = self.node(at)?;
        if node.health.is_some() || self.trail.is_last(at) {
            return None;
        }
        let next = at + 1;
        self.phase = TrailPhase::Traveling { to: next };
        Some(next)
    }

    /// Puts the trail back at `phase` (a save's), checked against this
    /// trail: a node index the trail does not have (a save from a map whose
    /// trail has since changed) leaves the Gonarch free rather than pointed
    /// at nothing, and a bad wait becomes none.
    pub fn restore(&mut self, phase: TrailPhase) {
        let valid = |index: usize| index < self.trail.len();
        self.phase = match phase {
            TrailPhase::Traveling { to } if valid(to) => phase,
            TrailPhase::Holding { at } if valid(at) => phase,
            TrailPhase::Arriving { at, wait_left } if valid(at) => TrailPhase::Arriving {
                at,
                wait_left: if wait_left.is_finite() {
                    wait_left.max(0.0)
                } else {
                    0.0
                },
            },
            _ => TrailPhase::Free,
        };
    }

    /// Decides what a hit of `amount` does to a Gonarch at `health_before`.
    ///
    /// The published rules, in order: shielded while travelling or in a
    /// node's arrival delay; on the last node every hit applies; on a node
    /// with a health value the hit applies, and the one that depletes it
    /// sends the Gonarch on (`Depleted`); on any other node it is still on
    /// the trail and shielded.
    pub fn absorb_damage(&mut self, health_before: f32, amount: f32) -> DamageVerdict {
        match self.phase {
            TrailPhase::Free => DamageVerdict::Applied,
            TrailPhase::Traveling { .. } | TrailPhase::Arriving { .. } => DamageVerdict::Shielded,
            TrailPhase::Holding { at } => {
                if self.trail.is_last(at) {
                    return DamageVerdict::Applied;
                }
                match self.node(at).and_then(|node| node.health) {
                    Some(_) if health_before - amount <= 0.0 => {
                        let next = at + 1;
                        self.phase = TrailPhase::Traveling { to: next };
                        DamageVerdict::Depleted { next }
                    }
                    Some(_) => DamageVerdict::Applied,
                    None => DamageVerdict::Shielded,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DamageVerdict, FIRST_NODE_KEY, GonarchTrail, MAX_TRAIL_NODES, SPAWNFLAG_NODE_RUN,
        SPAWNFLAG_NODE_WAIT_INDEFINITELY, TRAIL_ARRIVAL_RADIUS, Trail, TrailNode, TrailPhase,
    };
    use glam::Vec3;
    use ohl_game::EntityDef;
    use ohl_game::keyvalues::RenderProps;

    fn node_def(name: &str, origin: [f32; 3], keys: &[(&str, &str)], spawnflags: u32) -> EntityDef {
        EntityDef {
            classname: "info_bigmomma".to_string(),
            keyvalues: keys
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            origin,
            angles: [0.0; 3],
            targetname: Some(name.to_string()),
            target: None,
            spawnflags,
            model: None,
            render: RenderProps::default(),
        }
    }

    /// A three-node trail: a pass-through node with a delay, a fighting
    /// node with health, and a final node with health.
    fn defs() -> Vec<EntityDef> {
        vec![
            node_def(
                "n1",
                [0.0, 0.0, 0.0],
                &[
                    ("target", "n2"),
                    ("reachdelay", "1.5"),
                    ("reachtarget", "door_a"),
                ],
                SPAWNFLAG_NODE_RUN,
            ),
            node_def(
                "n2",
                [256.0, 0.0, 0.0],
                &[
                    ("target", "n3"),
                    ("health", "100"),
                    ("reachsequence", "seq_b"),
                ],
                0,
            ),
            node_def("n3", [512.0, 0.0, 0.0], &[("health", "60")], 0),
        ]
    }

    fn trail() -> Trail {
        Trail::from_defs(&defs(), "n1").expect("the first node resolves")
    }

    #[test]
    fn a_trail_is_walked_from_the_first_node_through_each_target() {
        let trail = trail();
        assert_eq!(trail.len(), 3);
        let names: Vec<&str> = trail.nodes().iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["n1", "n2", "n3"]);
        assert!(trail.nodes()[0].run);
        assert!((trail.nodes()[0].wait - 1.5).abs() < 1e-6);
        assert_eq!(trail.nodes()[0].fire_on_reach.as_deref(), Some("door_a"));
        assert_eq!(trail.nodes()[0].health, None);
        assert_eq!(trail.nodes()[1].health, Some(100.0));
        assert_eq!(trail.nodes()[1].sequence_on_reach.as_deref(), Some("seq_b"));
        assert!(trail.is_last(2));
        assert!(!trail.is_last(1));
        assert_eq!(FIRST_NODE_KEY, "netname");
    }

    #[test]
    fn a_looping_or_dangling_chain_stops_and_an_unknown_first_node_is_none() {
        let mut looped = defs();
        looped[2]
            .keyvalues
            .insert("target".to_string(), "n1".to_string());
        assert_eq!(Trail::from_defs(&looped, "n1").expect("resolves").len(), 3);

        let mut dangling = defs();
        dangling[1]
            .keyvalues
            .insert("target".to_string(), "nowhere".to_string());
        assert_eq!(
            Trail::from_defs(&dangling, "n1").expect("resolves").len(),
            2
        );

        assert!(Trail::from_defs(&defs(), "missing").is_none());
        assert!(Trail::from_defs(&defs(), "").is_none());
    }

    #[test]
    fn a_trail_is_bounded_however_long_the_map_chains_it() {
        let defs: Vec<EntityDef> = (0..(MAX_TRAIL_NODES + 10))
            .map(|i| {
                node_def(
                    &format!("n{i}"),
                    [0.0; 3],
                    &[("target", &format!("n{}", i + 1))],
                    0,
                )
            })
            .collect();
        let trail = Trail::from_defs(&defs, "n0").expect("resolves");
        assert_eq!(trail.len(), MAX_TRAIL_NODES);
        assert_eq!(Trail::new(trail.nodes().to_vec()).len(), MAX_TRAIL_NODES);
    }

    #[test]
    fn a_node_with_a_bad_health_value_reads_as_having_none() {
        for value in ["0", "-5", "nan", "inf", "x"] {
            let def = node_def("n", [0.0; 3], &[("health", value)], 0);
            assert_eq!(
                TrailNode::from_def(&def).expect("node").health,
                None,
                "{value}"
            );
        }
        let mut not_a_node = node_def("n", [0.0; 3], &[], 0);
        not_a_node.classname = "info_target".to_string();
        assert!(TrailNode::from_def(&not_a_node).is_none());
        let mut unnamed = node_def("n", [0.0; 3], &[], 0);
        unnamed.targetname = None;
        assert!(TrailNode::from_def(&unnamed).is_none());
    }

    /// The published progression end to end: shielded on the way, the
    /// first node passed after its delay, the second fought down and left
    /// on depletion, the last node the only place a hit can kill.
    #[test]
    fn the_gonarch_progresses_node_by_node_and_is_killable_only_at_the_end() {
        let mut gonarch = GonarchTrail::new(trail(), 1.5, 225.0);
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 0 });
        assert!(gonarch.is_shielded());
        assert!(gonarch.runs_to_destination());
        assert_eq!(gonarch.destination(), Some(Vec3::ZERO));
        assert_eq!(
            gonarch.absorb_damage(225.0, 1_000.0),
            DamageVerdict::Shielded
        );

        // Reaching the first node: fires its target, waits out the delay,
        // still shielded, then moves on by itself (no health to fight).
        assert!(!gonarch.has_reached_destination(Vec3::new(TRAIL_ARRIVAL_RADIUS + 1.0, 0.0, 0.0)));
        assert!(gonarch.has_reached_destination(Vec3::new(TRAIL_ARRIVAL_RADIUS - 1.0, 0.0, 0.0)));
        let effects = gonarch.arrive().expect("arrived");
        assert_eq!(effects.fire.as_deref(), Some("door_a"));
        assert_eq!(effects.set_health, None);
        assert!(matches!(
            gonarch.phase(),
            TrailPhase::Arriving { at: 0, wait_left } if (wait_left - 1.5).abs() < 1e-6
        ));
        assert_eq!(gonarch.absorb_damage(225.0, 50.0), DamageVerdict::Shielded);
        assert_eq!(gonarch.tick(1.0), None);
        assert_eq!(gonarch.tick(1.0), Some(1));
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 1 });
        assert!(!gonarch.runs_to_destination());
        assert_eq!(gonarch.destination(), Some(Vec3::new(256.0, 0.0, 0.0)));

        // Reaching the second node: health set to the node's value times
        // the difficulty factor, the sequence requested, fight until
        // depleted, then leave shielded.
        let effects = gonarch.arrive().expect("arrived at n2");
        assert!((effects.set_health.expect("health set") - 150.0).abs() < 1e-6);
        assert_eq!(effects.sequence.as_deref(), Some("seq_b"));
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 1 });
        assert!(!gonarch.is_shielded());
        assert_eq!(gonarch.absorb_damage(150.0, 100.0), DamageVerdict::Applied);
        assert_eq!(
            gonarch.absorb_damage(50.0, 100.0),
            DamageVerdict::Depleted { next: 2 }
        );
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 2 });
        assert!(gonarch.is_shielded());
        assert!((gonarch.base_health() - 225.0).abs() < 1e-6);

        // The last node: health set again, and now every hit applies,
        // including the one that would kill.
        let effects = gonarch.arrive().expect("arrived at n3");
        assert!((effects.set_health.expect("health set") - 90.0).abs() < 1e-6);
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 2 });
        assert!(!gonarch.is_shielded());
        assert_eq!(gonarch.absorb_damage(90.0, 30.0), DamageVerdict::Applied);
        assert_eq!(gonarch.absorb_damage(60.0, 60.0), DamageVerdict::Applied);
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 2 });
        assert!(gonarch.tick(10.0).is_none());
        assert!(gonarch.release().is_none());
    }

    #[test]
    fn a_pass_through_node_that_is_last_is_killable_without_a_health_value() {
        let defs = vec![
            node_def("a", [0.0; 3], &[("target", "b")], 0),
            node_def("b", [64.0, 0.0, 0.0], &[], 0),
        ];
        let mut gonarch =
            GonarchTrail::new(Trail::from_defs(&defs, "a").expect("trail"), 1.0, 150.0);
        gonarch.arrive().expect("at a");
        // No delay, no health, not last: straight on to b.
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 1 });
        gonarch.arrive().expect("at b");
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 1 });
        assert!(!gonarch.is_shielded());
        assert_eq!(gonarch.absorb_damage(150.0, 500.0), DamageVerdict::Applied);
    }

    #[test]
    fn an_indefinite_wait_holds_until_released() {
        let defs = vec![
            node_def(
                "a",
                [0.0; 3],
                &[("target", "b")],
                SPAWNFLAG_NODE_WAIT_INDEFINITELY,
            ),
            node_def("b", [64.0, 0.0, 0.0], &[], 0),
        ];
        let mut gonarch =
            GonarchTrail::new(Trail::from_defs(&defs, "a").expect("trail"), 1.0, 150.0);
        gonarch.arrive().expect("at a");
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 0 });
        assert!(gonarch.is_shielded(), "still on the trail");
        assert_eq!(
            gonarch.absorb_damage(150.0, 1_000.0),
            DamageVerdict::Shielded
        );
        assert!(gonarch.tick(100.0).is_none(), "time alone does not end it");
        assert_eq!(gonarch.release(), Some(1));
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 1 });
        assert!(gonarch.release().is_none());
    }

    #[test]
    fn a_restored_phase_resumes_mid_trail_and_a_bad_one_frees_the_gonarch() {
        let mut gonarch = GonarchTrail::new(trail(), 1.5, 225.0);
        let _ = gonarch.arrive();
        let _ = gonarch.tick(10.0);
        let _ = gonarch.arrive();
        let saved = gonarch.phase();
        assert_eq!(saved, TrailPhase::Holding { at: 1 });

        let mut loaded = GonarchTrail::new(trail(), 1.5, 225.0);
        assert_eq!(loaded.phase(), TrailPhase::Traveling { to: 0 });
        loaded.restore(saved);
        assert_eq!(loaded.phase(), saved);
        assert!(!loaded.is_shielded(), "fighting at the second node");
        assert_eq!(
            loaded.absorb_damage(10.0, 50.0),
            DamageVerdict::Depleted { next: 2 }
        );

        loaded.restore(TrailPhase::Arriving {
            at: 0,
            wait_left: f32::NAN,
        });
        assert_eq!(
            loaded.phase(),
            TrailPhase::Arriving {
                at: 0,
                wait_left: 0.0
            }
        );
        for out_of_range in [
            TrailPhase::Traveling { to: 3 },
            TrailPhase::Holding { at: 9 },
            TrailPhase::Arriving {
                at: 3,
                wait_left: 1.0,
            },
        ] {
            loaded.restore(out_of_range);
            assert_eq!(loaded.phase(), TrailPhase::Free, "{out_of_range:?}");
        }
    }

    /// The stall clock: progress keeps resetting it, standing still (or
    /// creeping by less than the step) runs it out, a new leg starts it
    /// again, and it never runs when the Gonarch is not travelling.
    #[test]
    fn a_leg_with_no_progress_stalls_and_a_new_leg_starts_the_clock_again() {
        use super::{TRAIL_PROGRESS_STEP, TRAIL_STALL_SECONDS};
        let mut gonarch = GonarchTrail::new(trail(), 1.0, 150.0);
        // Node 0 is at the origin; start 1000 units away.
        let mut position = Vec3::new(1_000.0, 0.0, 0.0);
        // Walking steadily toward it never stalls, however long.
        for _ in 0..1_000 {
            position.x -= 0.5;
            assert!(!gonarch.note_progress(position, 0.01));
        }
        // Creeping by less than the step does not count as progress.
        let mut elapsed = 0.0;
        loop {
            position.x -= TRAIL_PROGRESS_STEP * 0.001;
            elapsed += 0.1;
            if gonarch.note_progress(position, 0.1) {
                break;
            }
            assert!(elapsed < TRAIL_STALL_SECONDS + 1.0, "never stalled");
        }
        assert!(
            elapsed >= TRAIL_STALL_SECONDS * 0.9,
            "stalled early: {elapsed}"
        );
        // A new leg starts the clock again.
        let _ = gonarch.arrive();
        let _ = gonarch.tick(10.0);
        assert_eq!(gonarch.phase(), TrailPhase::Traveling { to: 1 });
        assert!(
            !gonarch.note_progress(position, TRAIL_STALL_SECONDS * 10.0),
            "a leg's first sample only starts its clock"
        );
        assert!(!gonarch.note_progress(position, TRAIL_STALL_SECONDS * 0.5));
        assert!(gonarch.note_progress(position, TRAIL_STALL_SECONDS * 0.6));
        // Not travelling: never stalled.
        let _ = gonarch.arrive();
        assert_eq!(gonarch.phase(), TrailPhase::Holding { at: 1 });
        assert!(!gonarch.note_progress(position, TRAIL_STALL_SECONDS * 10.0));
    }

    #[test]
    fn an_empty_trail_leaves_the_gonarch_free_and_killable() {
        let mut gonarch = GonarchTrail::new(Trail::default(), 2.0, 300.0);
        assert_eq!(gonarch.phase(), TrailPhase::Free);
        assert!(!gonarch.is_shielded());
        assert_eq!(gonarch.destination(), None);
        assert_eq!(gonarch.absorb_damage(300.0, 300.0), DamageVerdict::Applied);
        assert!(gonarch.arrive().is_none());
        assert!(gonarch.tick(1.0).is_none());
    }

    #[test]
    fn a_bad_factor_or_base_health_falls_back_rather_than_poisoning_the_state() {
        let gonarch = GonarchTrail::new(trail(), f32::NAN, -1.0);
        assert!((gonarch.base_health() - 1.0).abs() < 1e-6);
        let mut gonarch = gonarch;
        gonarch.arrive();
        gonarch.tick(f32::NAN);
        gonarch.tick(5.0);
        let effects = gonarch.arrive().expect("at n2");
        assert!(
            (effects.set_health.expect("set") - 100.0).abs() < 1e-6,
            "factor fell back to 1"
        );
    }
}
