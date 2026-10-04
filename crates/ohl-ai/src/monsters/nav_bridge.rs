//! [`NavBridge`]: package 7.6's real, `ohl-nav`-backed router.
//!
//! [`integration::Navigator`](super::integration::Navigator) forward-declares
//! the *minimal* seam a monster's movement needs from pathfinding:
//! `(origin, goal, max_step) -> next position`. A real implementation needs
//! two things that shape does not carry: **which entity is asking**, so a
//! path can be cached and only rebuilt when its goal drifts (the same
//! published "route refreshes when the enemy moves more than 80 units" rule
//! [`crate::movement::Route::needs_refresh`] already implements), and
//! **which hull it moves with**, already known per-monster via
//! [`crate::world::Actor::hull`] (set from
//! [`super::table::MonsterSpec::hull`], itself keyed off
//! [`super::table::SizeClass`]). So rather than widen `Navigator` itself,
//! [`NavBridge`] is a concrete type with its own richer `next_move`, and
//! [`crate::world::AiWorld::attach_navigator`] takes it directly.
//!
//! Node kind (ground/air/water) is only partly `ohl-nav`'s concern. A graph
//! built from [`ohl_nav::node_seeds_from_entities`]-style seeds keeps
//! ground links and air/water links in disjoint subgraphs (see
//! `ohl_nav::graph`'s module doc), but endpoint attachment and A* are
//! validated per [`Hull`], not per node kind, and every ground link is
//! also validated for the point hull. So the right hull alone does *not*
//! keep a flier (the point hull, [`crate::movement::flies`]) off the
//! ground subgraph: attached to its nearest node, which may be a ground
//! node, it would be routed through floor-level waypoints (a ground node's
//! waypoint is lifted by the hull's foot offset, which is zero for the
//! point hull) and dive to them. This bridge therefore drops any route for
//! a flier that passes through a grounded node and lets it fly the traced
//! fallback below instead. **`TODO`**: attaching a flier only to air nodes
//! in the first place needs a node-kind filter in `ohl_nav::find_path`;
//! until then a flier whose nearest node is a ground node gets no graph
//! route at all, even where an air route exists.
//!
//! Falls back to one traced step, [`crate::movement::move_toward`] —
//! horizontal with a step-up for a walker, the full line for a flier —
//! whenever the graph has no nodes, no path can be found this tick, or
//! this tick's bounded path-search budget is spent, so a monster is never
//! left unable to move and never moved through a wall to get there. The
//! one exception is a monster a script is walking to its mark, which keeps
//! the old wall-ignoring straight line ([`Fallback::StraightLine`] says
//! why, with its `TODO`).

use std::collections::HashMap;

use glam::Vec3;
use hecs::Entity;
use ohl_nav::{
    BuildLimits, NodeGraph, NodeKind, NodeSeed, Path, PathLimits, Steer, SteerLimits, find_path,
    straight_path_if_clear,
};
use ohl_physics::{CollisionModel, Hull};

use super::integration::{Navigator, StraightLineNavigator};
use crate::movement::{ROUTE_REFRESH_DISTANCE, STUCK_PROGRESS_FRACTION};

/// How far a cached path's goal may drift before it is rebuilt.
///
/// The same published 80-unit rule [`crate::movement::Route::needs_refresh`]
/// already uses, so a monster's node-graph route and its high-level `Route`
/// bookkeeping refresh on the same cited threshold.
pub const PATH_REFRESH_DISTANCE: f32 = ROUTE_REFRESH_DISTANCE;

/// Bounds and tolerances for [`NavBridge`], beyond the graph itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavBridgeLimits {
    /// Bounds for each `ohl_nav::find_path` call.
    pub path: PathLimits,
    /// Bounds for each `ohl_nav::Steer::next_move` call.
    pub steer: SteerLimits,
    /// The most `find_path` searches one tick may spend, across every
    /// actor, so a fixed tick stays bounded even when many monsters need a
    /// fresh route the same tick. A project choice; default 8.
    pub max_searches_per_tick: usize,
}

impl Default for NavBridgeLimits {
    fn default() -> Self {
        Self {
            path: PathLimits::default(),
            steer: SteerLimits::default(),
            max_searches_per_tick: 8,
        }
    }
}

/// What [`NavBridge::next_move_with`] does when neither a straight line nor
/// a graph route reaches the goal this tick (no nodes, no path, or the
/// tick's search budget spent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// One traced [`crate::movement::move_toward`] step: whatever the
    /// hull's own trace says is solid stops it. What every monster moving
    /// under its own brain gets.
    Traced,
    /// The [`StraightLineNavigator`] step, which ignores collision. Kept
    /// only for a monster a `scripted_sequence` is walking to its mark: a
    /// mark this graph cannot route to is a gap in this project's routing,
    /// not the map's intent, and a script whose monster never arrives
    /// stalls whatever the map chained onto it (the measured case is a
    /// carried guard whose scripted walk opens the only door out of a map:
    /// with a traced fallback the chain walk stops there, at depth 6).
    /// **`TODO`**: route such marks properly and retire this.
    StraightLine,
}

/// One actor's cached route: the path it is following, the hull and goal it
/// was built for, and the local steering cursor over it.
#[derive(Debug, Clone, PartialEq)]
struct CachedRoute {
    goal: Vec3,
    hull: Hull,
    path: Path,
    steer: Steer,
    expected_origin: Vec3,
    direct: bool,
    walking_attachment: bool,
    attachment: InitialAttachment,
}

/// Derived route state, discarded with the cache; never part of a save.
#[derive(Debug, Clone, Copy, PartialEq)]
enum InitialAttachment {
    Unchecked,
    Descending { lowest_z: f32 },
    Ready,
}

/// Aggregate routing diagnostics. Data only: no entity or map identifiers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NavigationStats {
    /// Steps following a directly connected path.
    pub direct_steps: u64,
    /// Steps following a node-graph path.
    pub graph_steps: u64,
    /// Collision-checked local fallback steps.
    pub traced_steps: u64,
    /// Compatibility fallback steps without collision.
    pub untraced_steps: u64,
    /// Calls whose centered starting hull was already in solid.
    pub start_solid: u64,
}

impl NavigationStats {
    /// Adds counters observed between two snapshots (saturating at u64::MAX).
    pub fn add_delta(&mut self, before: Self, after: Self) {
        self.direct_steps = self
            .direct_steps
            .saturating_add(after.direct_steps.saturating_sub(before.direct_steps));
        self.graph_steps = self
            .graph_steps
            .saturating_add(after.graph_steps.saturating_sub(before.graph_steps));
        self.traced_steps = self
            .traced_steps
            .saturating_add(after.traced_steps.saturating_sub(before.traced_steps));
        self.untraced_steps = self
            .untraced_steps
            .saturating_add(after.untraced_steps.saturating_sub(before.untraced_steps));
        self.start_solid = self
            .start_solid
            .saturating_add(after.start_solid.saturating_sub(before.start_solid));
    }
}

/// The real navigator: an `ohl-nav` [`NodeGraph`] plus a per-actor path
/// cache and a bounded per-tick search budget.
///
/// Built once per map (`NavBridge::build`), then driven one call per actor
/// per tick through [`NavBridge::next_move`]. [`Self::begin_tick`] must be
/// called once per tick, before any `next_move`, to reset the search budget
/// and drop cache entries for actors that were not ticked.
#[derive(Debug)]
pub struct NavBridge {
    graph: NodeGraph,
    limits: NavBridgeLimits,
    cache: HashMap<Entity, CachedRoute>,
    searches_used: usize,
    stats: NavigationStats,
    max_attachment_drop: f32,
}

impl NavBridge {
    /// Builds the node graph for a map from `seeds` (ground/air/water node
    /// positions — see [`node_seeds_from_defs`] or
    /// `ohl_nav::node_seeds_from_entities`) and its `collision` model.
    #[must_use]
    pub fn build(
        seeds: &[NodeSeed],
        collision: &CollisionModel,
        build_limits: &BuildLimits,
        limits: NavBridgeLimits,
    ) -> Self {
        Self {
            graph: NodeGraph::build(seeds, collision, build_limits),
            limits,
            cache: HashMap::new(),
            searches_used: 0,
            stats: NavigationStats::default(),
            // Match BuildLimits' finite-positive normalization.
            max_attachment_drop: if build_limits.max_drop.is_finite() && build_limits.max_drop > 0.0
            {
                build_limits.max_drop
            } else {
                BuildLimits::default().max_drop
            },
        }
    }

    /// The number of nodes in the built graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// The number of directed links in the built graph.
    #[must_use]
    pub fn link_count(&self) -> usize {
        self.graph.link_count()
    }

    /// Aggregate inspection counters; never logs payload data.
    #[must_use]
    pub fn stats(&self) -> NavigationStats {
        self.stats
    }

    /// Discards transient paths after a restore or explicit world change.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    /// Resets this tick's path-search budget and drops cached routes for
    /// actors not present in `live` (a best-effort bound on cache growth as
    /// monsters die or despawn). Call once per tick before any
    /// [`Self::next_move`].
    pub fn begin_tick(&mut self, live: &[Entity]) {
        self.searches_used = 0;
        if self.cache.is_empty() {
            return;
        }
        let live: std::collections::HashSet<Entity> = live.iter().copied().collect();
        self.cache.retain(|entity, _| live.contains(entity));
    }

    /// The next position `actor` should move toward, at most `max_step`
    /// world units from `origin`, on its way to `goal`. Both endpoints and
    /// the returned position are centered hull queries, not model anchors.
    ///
    /// Reuses `actor`'s cached path while it was built for the same hull and
    /// its goal has not drifted more than [`PATH_REFRESH_DISTANCE`];
    /// otherwise tries the direct line, then spends one of this tick's
    /// bounded `find_path` searches, and finally falls back to
    /// one traced [`crate::movement::move_toward`] step when neither finds
    /// a route ([`Fallback::Traced`]).
    #[must_use]
    pub fn next_move(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        max_step: f32,
    ) -> Vec3 {
        self.next_move_with(
            actor,
            origin,
            goal,
            hull,
            collision,
            max_step,
            Fallback::Traced,
        )
    }

    /// [`Self::next_move`], with the caller choosing what happens when no
    /// straight line and no graph route reaches `goal` this tick.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "next_move's six, plus the one choice this variant exists for"
    )]
    pub fn next_move_with(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        max_step: f32,
        fallback: Fallback,
    ) -> Vec3 {
        self.next_move_impl(
            actor, origin, goal, hull, collision, max_step, fallback, false,
        )
    }

    /// A centered query whose caller has verified a living, solid walking
    /// actor. Allows supported descent at the first grounded graph attachment;
    /// generic queries retain their existing movement contract.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn next_move_with_walking_attachment(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        max_step: f32,
        fallback: Fallback,
    ) -> Vec3 {
        self.next_move_impl(
            actor, origin, goal, hull, collision, max_step, fallback, true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn next_move_impl(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        max_step: f32,
        fallback: Fallback,
        walking_attachment: bool,
    ) -> Vec3 {
        if !origin.is_finite() || !goal.is_finite() || !max_step.is_finite() {
            // `origin` itself may be the non-finite value, so it cannot be
            // handed back as-is; a fixed, finite point is the only answer
            // that keeps this call total.
            return Vec3::ZERO;
        }

        if collision.trace(hull, origin, origin).start_solid {
            self.stats.start_solid = self.stats.start_solid.saturating_add(1);
        }

        let stale = self.cache.get(&actor).is_none_or(|cached| {
            cached.hull != hull
                || cached.walking_attachment != walking_attachment
                || (cached.goal - goal).length() > PATH_REFRESH_DISTANCE
                || !cached.expected_origin.abs_diff_eq(origin, 0.001)
        });
        if stale {
            self.rebuild(actor, origin, goal, hull, collision, walking_attachment);
        }

        let Some(cached) = self.cache.get_mut(&actor) else {
            return match fallback {
                Fallback::Traced => {
                    self.stats.traced_steps = self.stats.traced_steps.saturating_add(1);
                    crate::movement::move_toward(collision, hull, origin, goal, max_step, 1.0)
                        .position
                }
                Fallback::StraightLine => {
                    self.stats.untraced_steps = self.stats.untraced_steps.saturating_add(1);
                    StraightLineNavigator.next_move(origin, goal, max_step)
                }
            };
        };

        if cached.direct {
            self.stats.direct_steps = self.stats.direct_steps.saturating_add(1);
        } else {
            self.stats.graph_steps = self.stats.graph_steps.saturating_add(1);
        }

        let steer_limits = own_pace_steer_limits(&self.limits.steer, max_step);
        if let Some(next) = initial_attachment_step(
            cached,
            origin,
            collision,
            max_step,
            self.max_attachment_drop,
            steer_limits.arrive_radius,
        ) {
            cached.expected_origin = next;
            return next;
        }
        let intent = cached
            .steer
            .next_move(origin, &cached.path, hull, collision, &steer_limits);
        if intent.reached {
            // Arrival tolerance is not authority to teleport. A live brush
            // can have closed across this final segment since path creation.
            let delta = goal - origin;
            let step = delta.clamp_length_max(max_step.max(0.0));
            let trace = collision.trace(hull, origin, origin + step);
            let next = if trace.start_solid {
                origin
            } else {
                trace.end_pos
            };
            cached.expected_origin = next;
            if trace.blocked() {
                self.cache.remove(&actor);
            }
            return next;
        }
        if intent.dir.length_squared() <= f32::EPSILON {
            return origin;
        }
        let waypoint_distance = cached
            .path
            .waypoints
            .get(cached.steer.cursor())
            .map_or(max_step, |waypoint| (*waypoint - origin).length());
        let travel =
            max_step.max(0.0).min(waypoint_distance.max(0.0)) * intent.speed_scale.clamp(0.0, 1.0);
        let next = if travel <= 0.0 {
            origin
        } else {
            origin + intent.dir * travel
        };
        cached.expected_origin = next;
        next
    }

    /// Rebuilds (or drops) `actor`'s cached route toward `goal`.
    #[allow(clippy::too_many_arguments)]
    fn rebuild(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
        walking_attachment: bool,
    ) {
        if let Some(path) = straight_path_if_clear(collision, origin, goal, hull) {
            self.cache.insert(
                actor,
                CachedRoute {
                    goal,
                    hull,
                    path,
                    steer: Steer::new(),
                    expected_origin: origin,
                    direct: true,
                    walking_attachment,
                    attachment: InitialAttachment::Ready,
                },
            );
            return;
        }
        if self.graph.node_count() > 0 && self.searches_used < self.limits.max_searches_per_tick {
            self.searches_used += 1;
            if let Some(path) = find_path(
                &self.graph,
                collision,
                origin,
                goal,
                hull,
                &self.limits.path,
            )
            .filter(|path| !crate::movement::flies(hull) || !self.uses_ground_node(path))
            {
                let attachment = if walking_attachment
                    && !crate::movement::flies(hull)
                    && path
                        .nodes
                        .first()
                        .and_then(|node| self.graph.node(*node))
                        .is_some_and(|node| node.kind == NodeKind::Ground && node.snapped)
                {
                    InitialAttachment::Unchecked
                } else {
                    InitialAttachment::Ready
                };
                self.cache.insert(
                    actor,
                    CachedRoute {
                        goal,
                        hull,
                        path,
                        steer: Steer::new(),
                        expected_origin: origin,
                        direct: false,
                        walking_attachment,
                        attachment,
                    },
                );
                return;
            }
        }
        self.cache.remove(&actor);
    }

    /// Whether `path` passes through a grounded node: a route a flier must
    /// not take, since a ground node's waypoint for the point hull is the
    /// floor itself (see the module doc).
    fn uses_ground_node(&self, path: &Path) -> bool {
        path.nodes.iter().any(|index| {
            self.graph
                .node(*index)
                .is_some_and(|node| node.kind.is_grounded())
        })
    }
}

/// A pending first attachment owns vertical movement until actual support.
/// Returning None leaves ordinary steering untouched; Some can be a blocked
/// retry at the current center. This never advances Steer's cursor itself.
fn initial_attachment_step(
    cached: &mut CachedRoute,
    origin: Vec3,
    collision: &CollisionModel,
    max_step: f32,
    max_drop: f32,
    arrive_radius: f32,
) -> Option<Vec3> {
    if cached.attachment == InitialAttachment::Ready {
        return None;
    }
    if max_step <= 0.0 {
        return Some(origin);
    }
    let waypoint = *cached.path.waypoints.first()?;
    if cached.attachment == InitialAttachment::Unchecked {
        if cached.steer.cursor() != 0 {
            cached.attachment = InitialAttachment::Ready;
            return None;
        }
        let delta = waypoint - origin;
        if delta.z >= 0.0
            || delta.truncate().length() > arrive_radius
            || delta.length() <= arrive_radius
        {
            return None;
        }
        cached.attachment = InitialAttachment::Descending {
            lowest_z: origin.z - max_drop,
        };
    }
    let InitialAttachment::Descending { lowest_z } = cached.attachment else {
        return None;
    };
    let Some((next, landed)) = crate::movement::descend_to_ground(
        collision,
        cached.hull,
        origin,
        waypoint.z,
        lowest_z,
        max_step,
    ) else {
        return Some(origin);
    };
    if landed {
        cached.attachment = InitialAttachment::Ready;
    }
    Some(next)
}

/// `limits` with its stuck window measured against this mover's own pace.
///
/// `ohl-nav`'s steering calls a mover stuck when one window
/// ([`SteerLimits::stuck_window_ticks`]) covers less than
/// [`SteerLimits::min_window_progress`] — by default 8 units in 20 ticks,
/// which is 40 units per second at the 100 Hz tick. A monster walking at
/// the default walk of 40 sits exactly on that line and, a rounding error
/// short of it, is told to side-step instead of walking on. The window is
/// therefore never asked for more than [`STUCK_PROGRESS_FRACTION`] of what
/// `max_step` a tick would cover over it, the same measure
/// [`crate::movement::StuckDetector::record_step`] applies per tick; a fast
/// mover's window is the configured one.
fn own_pace_steer_limits(limits: &SteerLimits, max_step: f32) -> SteerLimits {
    let window = u16::try_from(limits.stuck_window_ticks.max(1)).map_or(f32::MAX, f32::from);
    let own = STUCK_PROGRESS_FRACTION * max_step * window;
    if own.is_finite() && own > 0.0 && own < limits.min_window_progress {
        SteerLimits {
            min_window_progress: own,
            ..*limits
        }
    } else {
        *limits
    }
}

/// Builds `ohl_nav` node seeds from already-typed `ohl_game::EntityDef`s.
///
/// The same recognised classnames as `ohl_nav::node_seeds_from_entities`
/// (`info_node` a ground node, `info_node_air` a flying one; see
/// `docs/FORMAT_SOURCES.md`, "Navigation"), applied to a
/// `Registry`-adjacent typed entity list instead of an untyped BSP entities
/// lump, so a caller that already ran `ohl_game::parse_entities` does not
/// need to keep the raw lump around just to build a [`NavBridge`].
#[must_use]
pub fn node_seeds_from_defs(defs: &[ohl_game::EntityDef], max_nodes: usize) -> Vec<NodeSeed> {
    let mut seeds = Vec::new();
    for def in defs {
        if seeds.len() >= max_nodes {
            break;
        }
        let kind = match def.classname.as_str() {
            "info_node" => NodeKind::Ground,
            "info_node_air" => NodeKind::Air,
            _ => continue,
        };
        seeds.push(NodeSeed::new(Vec3::from_array(def.origin), kind));
    }
    seeds
}

#[cfg(test)]
mod tests {
    use super::{NavBridge, NavBridgeLimits, node_seeds_from_defs};
    use ohl_formats::bsp30::{Bsp, Entity as RawEntity, Limits};
    use ohl_formats::test_support::{Bsp30Builder, CollisionBrush};
    use ohl_game::keyvalues::{Limits as KeyvalueLimits, parse_entities};
    use ohl_nav::{BuildLimits, NodeKind, NodeSeed};
    use ohl_physics::CollisionModel;

    fn entity_def(classname: &str, origin: [f32; 3]) -> ohl_game::EntityDef {
        let mut raw: RawEntity = RawEntity::new();
        raw.insert("classname".to_string(), classname.to_string());
        raw.insert(
            "origin".to_string(),
            format!("{} {} {}", origin[0], origin[1], origin[2]),
        );
        parse_entities(&[raw], &KeyvalueLimits::default())
            .into_iter()
            .next()
            .expect("one entity in, one def out")
    }

    #[test]
    fn node_seeds_are_read_from_the_published_classnames_only() {
        let node = entity_def("info_node", [10.0, 20.0, 30.0]);
        let air = entity_def("info_node_air", [1.0, 2.0, 3.0]);
        let ignored = entity_def("info_target", [0.0, 0.0, 0.0]);

        let seeds = node_seeds_from_defs(&[node, air, ignored], 16);
        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[0].kind, NodeKind::Ground);
        assert_eq!(seeds[1].kind, NodeKind::Air);
    }

    #[test]
    fn node_seeds_are_bounded() {
        let defs: Vec<ohl_game::EntityDef> = (0..8)
            .map(|_| entity_def("info_node", [0.0, 0.0, 0.0]))
            .collect();
        assert_eq!(node_seeds_from_defs(&defs, 3).len(), 3);
    }

    fn open_room() -> CollisionModel {
        let mut builder = Bsp30Builder::new();
        builder.set_entities_text("{\n\"classname\" \"worldspawn\"\n}\n");
        let heads = builder.push_collision_hulls(&[
            CollisionBrush::half_space([0.0, 0.0, 1.0], 0.0),
            CollisionBrush::half_space([0.0, 0.0, -1.0], -256.0),
            CollisionBrush::half_space([-1.0, 0.0, 0.0], -512.0),
            CollisionBrush::half_space([1.0, 0.0, 0.0], -512.0),
            CollisionBrush::half_space([0.0, -1.0, 0.0], -512.0),
            CollisionBrush::half_space([0.0, 1.0, 0.0], -512.0),
        ]);
        builder.push_model(
            [-512.0, -512.0, 0.0],
            [512.0, 512.0, 256.0],
            [0.0, 0.0, 0.0],
            heads,
            2,
            0,
            0,
        );
        let bytes = builder.build();
        let limits = Limits::default();
        let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
        CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
    }

    #[test]
    fn an_empty_graph_reports_no_nodes_or_links() {
        let seeds: Vec<NodeSeed> = Vec::new();
        let collision = open_room();
        let bridge = NavBridge::build(
            &seeds,
            &collision,
            &BuildLimits::default(),
            NavBridgeLimits::default(),
        );
        assert_eq!(bridge.node_count(), 0);
        assert_eq!(bridge.link_count(), 0);
    }

    /// Wave 1 batch A: the flight seam. Against a real collision model, a
    /// point-hull mover follows the full three-dimensional line to a
    /// target above it (an alien controller climbing), a box-hull mover
    /// only ever the horizontal part (a walker never walks at something
    /// above it), and a flier flying into the ceiling is stopped by it
    /// rather than passing through — the same trace a walker is stopped
    /// by, just not flattened first.
    #[test]
    fn a_point_hull_mover_flies_and_a_box_hull_mover_walks() {
        use crate::movement::{flies, move_toward};
        use glam::Vec3;
        use ohl_physics::Hull;
        let collision = open_room();
        let from = Vec3::new(0.0, 0.0, 64.0);
        let above = Vec3::new(100.0, 0.0, 200.0);

        assert!(flies(Hull::Point));
        assert!(!flies(Hull::Standing));

        let flown = move_toward(&collision, Hull::Point, from, above, 100.0, 1.0);
        assert!(
            flown.position.z > from.z + 10.0,
            "a flier climbs: {flown:?}"
        );
        assert!(flown.position.x > from.x, "and closes: {flown:?}");
        assert!(!flown.blocked);

        let walked = move_toward(&collision, Hull::Standing, from, above, 100.0, 1.0);
        assert!(
            (walked.position.z - from.z).abs() < 1.0,
            "a walker keeps its height: {walked:?}"
        );
        assert!(walked.position.x > from.x, "but still closes: {walked:?}");

        // Straight up into the ceiling at 256: stopped short, and blocked.
        let ceiling = Vec3::new(0.0, 0.0, 512.0);
        let bumped = move_toward(&collision, Hull::Point, from, ceiling, 1_000.0, 1.0);
        assert!(bumped.blocked, "the ceiling stops a flier: {bumped:?}");
        assert!(bumped.position.z <= 256.0 + f32::EPSILON, "{bumped:?}");
        assert!(
            bumped.position.z > from.z,
            "it still flew up to it: {bumped:?}"
        );
    }
}
