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
use crate::registry::{BrushModel, Registry, Transform};

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

/// Collects one [`ModelInstance`] per entity that has both a [`BrushModel`]
/// and a [`Transform`], in registry spawn order.
#[must_use]
pub fn model_instances(registry: &Registry) -> Vec<ModelInstance> {
    let mut out = Vec::new();
    for (entity, model, transform, render) in
        &mut registry
            .world
            .query::<(Entity, &BrushModel, &Transform, &RenderProps)>()
    {
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
}
