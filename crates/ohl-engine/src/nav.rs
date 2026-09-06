//! Building this map's navigation graph and handing it to the AI.
//!
//! `ohl-nav` builds a [`ohl_nav::NodeGraph`] from node *seeds* and a
//! collision model, and `ohl-ai`'s [`ohl_ai::NavBridge`] wraps that graph
//! with a per-actor path cache and a bounded per-tick search budget. The
//! engine's whole job is to find the seeds — the `info_node` and
//! `info_node_air` entities the map declares, read by
//! [`ohl_ai::node_seeds_from_defs`] — and to attach and detach the bridge
//! across a level change.
//!
//! A map with no node entities, or with no usable collision hulls, leaves
//! the navigator detached. That is a supported state, not an error:
//! `ohl-ai` then follows its straight-line fallback, so monsters still
//! move.
//!
//! # Clean room
//!
//! The two node classnames are published mapping vocabulary, recorded in
//! `docs/FORMAT_SOURCES.md` under "Navigation"; the recognition itself
//! lives in `ohl-ai`, not here. This module adds no behavioural fact.
//!
//! # Logging
//!
//! Nothing here logs; node counts are map-derived and stay data.

use ohl_ai::{NavBridge, NavBridgeLimits, node_seeds_from_defs};
use ohl_game::keyvalues::EntityDef;
use ohl_nav::BuildLimits;
use ohl_physics::CollisionModel;

/// The largest number of node seeds one map contributes, so a map full of
/// `info_node`s cannot make level loading unbounded. Matches the shape of
/// the other per-level caps in [`crate::level`].
pub const MAX_NODE_SEEDS: usize = 4_096;

/// Builds the navigator for a map, or `None` when there is nothing to
/// navigate: no node entities, or no collision hulls to validate links
/// against.
#[must_use]
pub fn build(defs: &[EntityDef], collision: Option<&CollisionModel>) -> Option<NavBridge> {
    let collision = collision?;
    let seeds = node_seeds_from_defs(defs, MAX_NODE_SEEDS);
    if seeds.is_empty() {
        return None;
    }
    Some(NavBridge::build(
        &seeds,
        collision,
        &BuildLimits::default(),
        NavBridgeLimits::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::build;
    use ohl_formats::bsp30::Entity as RawEntity;
    use ohl_game::keyvalues::{Limits, parse_entities};

    fn defs(classnames: &[&str]) -> Vec<ohl_game::EntityDef> {
        let raws: Vec<RawEntity> = classnames
            .iter()
            .map(|classname| {
                let mut raw = RawEntity::new();
                raw.insert("classname".to_string(), (*classname).to_string());
                raw.insert("origin".to_string(), "0 0 0".to_string());
                raw
            })
            .collect();
        parse_entities(&raws, &Limits::default())
    }

    #[test]
    fn a_map_with_no_nodes_has_no_navigator() {
        assert!(build(&defs(&["worldspawn", "info_player_start"]), None).is_none());
    }

    #[test]
    fn a_map_with_nodes_but_no_collision_has_no_navigator() {
        assert!(build(&defs(&["info_node", "info_node"]), None).is_none());
    }
}
