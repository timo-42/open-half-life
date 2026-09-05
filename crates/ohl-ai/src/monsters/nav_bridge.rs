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
//! Node kind (ground/air/water) is entirely `ohl-nav`'s concern: a graph
//! built from [`ohl_nav::node_seeds_from_entities`]-style seeds already
//! keeps ground links and air/water links in disjoint subgraphs (see
//! `ohl_nav::graph`'s module doc), and both endpoint attachment and A* are
//! validated per [`Hull`], not per node kind. A flying or swimming monster
//! therefore only needs the right hull passed in — which it already has —
//! for its moves to land on, and stay within, the air/water subgraph; this
//! bridge does not need its own kind-aware attachment logic on top of that.
//!
//! Falls back to [`StraightLineNavigator`] whenever the graph has no nodes,
//! no path can be found this tick, or this tick's bounded path-search budget
//! is spent, so a monster is never left unable to move.

use std::collections::HashMap;

use glam::Vec3;
use hecs::Entity;
use ohl_nav::{
    BuildLimits, NodeGraph, NodeKind, NodeSeed, Path, PathLimits, Steer, SteerLimits, find_path,
    straight_path_if_clear,
};
use ohl_physics::{CollisionModel, Hull};

use super::integration::{Navigator, StraightLineNavigator};
use crate::movement::ROUTE_REFRESH_DISTANCE;

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

/// One actor's cached route: the path it is following, the hull and goal it
/// was built for, and the local steering cursor over it.
#[derive(Debug, Clone, PartialEq)]
struct CachedRoute {
    goal: Vec3,
    hull: Hull,
    path: Path,
    steer: Steer,
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
    /// world units from `origin`, on its way to `goal`.
    ///
    /// Reuses `actor`'s cached path while it was built for the same hull and
    /// its goal has not drifted more than [`PATH_REFRESH_DISTANCE`];
    /// otherwise tries the direct line, then spends one of this tick's
    /// bounded `find_path` searches, and finally falls back to
    /// [`StraightLineNavigator`] when neither finds a route.
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
        if !origin.is_finite() || !goal.is_finite() || !max_step.is_finite() {
            // `origin` itself may be the non-finite value, so it cannot be
            // handed back as-is; a fixed, finite point is the only answer
            // that keeps this call total.
            return Vec3::ZERO;
        }

        let stale = self.cache.get(&actor).is_none_or(|cached| {
            cached.hull != hull || (cached.goal - goal).length() > PATH_REFRESH_DISTANCE
        });
        if stale {
            self.rebuild(actor, origin, goal, hull, collision);
        }

        let Some(cached) = self.cache.get_mut(&actor) else {
            return StraightLineNavigator.next_move(origin, goal, max_step);
        };

        let intent =
            cached
                .steer
                .next_move(origin, &cached.path, hull, collision, &self.limits.steer);
        if intent.reached {
            return goal;
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
        if travel <= 0.0 {
            origin
        } else {
            origin + intent.dir * travel
        }
    }

    /// Rebuilds (or drops) `actor`'s cached route toward `goal`.
    fn rebuild(
        &mut self,
        actor: Entity,
        origin: Vec3,
        goal: Vec3,
        hull: Hull,
        collision: &CollisionModel,
    ) {
        if let Some(path) = straight_path_if_clear(collision, origin, goal, hull) {
            self.cache.insert(
                actor,
                CachedRoute {
                    goal,
                    hull,
                    path,
                    steer: Steer::new(),
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
            ) {
                self.cache.insert(
                    actor,
                    CachedRoute {
                        goal,
                        hull,
                        path,
                        steer: Steer::new(),
                    },
                );
                return;
            }
        }
        self.cache.remove(&actor);
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
}
