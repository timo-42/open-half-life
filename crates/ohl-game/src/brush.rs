//! Per-entity brush-model instancing data for the renderer.
//!
//! `ohl-world`'s `WorldModel` only builds worldspawn (submodel 0) geometry;
//! submodels 1.. (doors, buttons, platforms and other brush entities) need
//! an entity-driven transform to place them, which is exactly what the
//! [`Registry`] carries. This module just gathers that placement data; the
//! actual per-model geometry comes from `ohl_world::brush`.

use glam::Vec3;
use hecs::Entity;

use crate::keyvalues::RenderProps;
use crate::registry::{BrushModel, ClassName, Registry, Transform};

/// One brush-model entity's placement: which submodel to draw, and where.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelInstance {
    /// The entity this instance was built from, for callers that need to
    /// look up further components (e.g. a `Door`'s current state) while
    /// drawing.
    pub entity: Entity,
    /// Index into `BSP::models` (and `WorldModel`'s per-model draw list).
    pub model_index: u32,
    /// World-space origin. For brush entities this is normally `[0,0,0]`
    /// plus whatever offset the map logic simulation has applied (e.g. a
    /// door mid-slide); the brush geometry itself is already baked in
    /// world space.
    pub origin: Vec3,
    /// `pitch yaw roll`, in degrees.
    pub angles: Vec3,
    /// `rendermode`/`renderamt`/`rendercolor`.
    pub render: RenderProps,
}

/// Whether `classname` names a brush entity GoldSrc never draws client-side,
/// regardless of its `rendermode`/texture: every `trigger_*` entity (see
/// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic") is a
/// collision-only volume, and `func_ladder` is documented (TWHL wiki,
/// "func_ladder") as "creat[ing] an invisible brush which, when touched by
/// the player, allows them to climb". A map can and does place one of these
/// (for example a `trigger_transition` at a level's exit) so that it
/// encloses the player, and rendering it as ordinary opaque geometry means
/// the camera ends up embedded in it from the very first frame.
fn is_never_rendered(classname: &str) -> bool {
    classname.starts_with("trigger_") || classname == "func_ladder"
}

/// Collects one [`ModelInstance`] per entity that has a [`BrushModel`], a
/// [`Transform`] and a classname GoldSrc actually draws (excluding
/// collision-only volumes; see [`is_never_rendered`]), in registry spawn
/// order.
#[must_use]
pub fn model_instances(registry: &Registry) -> Vec<ModelInstance> {
    let mut out = Vec::new();
    for (entity, model, transform, render, classname) in
        &mut registry
            .world
            .query::<(Entity, &BrushModel, &Transform, &RenderProps, &ClassName)>()
    {
        if is_never_rendered(&classname.0) {
            continue;
        }
        out.push(ModelInstance {
            entity,
            model_index: model.0,
            origin: transform.origin,
            angles: transform.angles,
            render: *render,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::model_instances;
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::Registry;
    use ohl_formats::bsp30::Entity as RawEntity;
    use std::collections::BTreeMap;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn gathers_only_brush_model_entities() {
        let entities = vec![
            raw(&[("classname", "func_door"), ("model", "*2")]),
            raw(&[("classname", "info_player_start")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let instances = model_instances(&registry);
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].model_index, 2);
    }

    /// A `trigger_transition` (or any other `trigger_*`) volume is a
    /// collision-only brush GoldSrc never draws client-side; a map can
    /// legitimately place one so that it encloses the player (a level's
    /// `trigger_transition` commonly sits right at the exit the player
    /// walks up to, exactly where a capture's camera ends up standing).
    /// `model_instances` must exclude it while still leaving its
    /// `BrushModel`/bounds available to `ohl-engine`'s transition logic.
    #[test]
    fn excludes_trigger_volumes_that_carry_a_brush_model() {
        let entities = vec![
            raw(&[("classname", "trigger_transition"), ("model", "*3")]),
            raw(&[("classname", "trigger_multiple"), ("model", "*4")]),
            raw(&[("classname", "func_wall"), ("model", "*5")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let instances = model_instances(&registry);
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].model_index, 5);
    }

    /// `func_ladder` is documented as an invisible climb volume (TWHL wiki,
    /// "func_ladder"); it must not be drawn either.
    #[test]
    fn excludes_func_ladder() {
        let entities = vec![raw(&[("classname", "func_ladder"), ("model", "*6")])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        assert!(model_instances(&registry).is_empty());
    }
}
