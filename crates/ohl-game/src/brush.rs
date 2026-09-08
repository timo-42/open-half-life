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
use crate::registry::{Breakable, BrushModel, ClassName, Liquid, Registry, Transform, Water};

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
///
/// `func_monsterclip` joins this list for the same reason: TWHL wiki
/// `func_monsterclip` (search-engine result summary; the page itself
/// returns HTTP 403 to automated fetches from this environment, the same
/// caveat `docs/FORMAT_SOURCES.md` already records for other TWHL
/// citations) describes it as "an invisible brush entity" that is "solid to
/// monsters" but "not solid to players", used to shape monster paths
/// without affecting player movement or visibility. See
/// `docs/FORMAT_SOURCES.md`, item 33, for the full citation and the
/// project's own reading of it.
fn is_never_rendered(classname: &str) -> bool {
    classname.starts_with("trigger_")
        || classname == "func_ladder"
        || classname == "func_monsterclip"
}

/// Brush-entity classnames that are documented as *not* solid to the
/// player, and so must never be attached to the collision model with
/// [`solid_model_instances`]/`ohl_physics::CollisionModel::attach_brush`.
///
/// - `func_illusionary`: TWHL wiki, "func_illusionary" — a brush that is
///   drawn but has no collision, the standard way to build a non-solid
///   decoration. Contributes no contents at all.
/// - `func_ladder`: TWHL wiki, "func_ladder" — an invisible brush the
///   player climbs rather than collides with. Still marks its own space
///   climbable; see [`contents_model_instances`].
/// - `func_water`: a swimmable liquid volume, not a wall. Still marks its
///   own space with the liquid its `skin` keyvalue selects; see
///   [`contents_model_instances`].
/// - `func_monsterclip`: TWHL wiki `func_monsterclip` (search-engine result
///   summary; page returns HTTP 403 to automated fetches from this
///   environment) — "an invisible brush entity" that is "solid to
///   monsters" but "not solid to players". This project's collision model
///   is shared unmodified between the player and monster navigation (see
///   `docs/FORMAT_SOURCES.md` item 33), so excluding it here also makes it
///   non-solid to monsters, a documented, deliberate gap rather than a
///   silent one.
/// - every `trigger_*`: collision-only *volumes* that fire map logic when
///   the player is inside them, which is impossible if they push the
///   player out (see [`is_never_rendered`]).
///
/// See `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic", and item
/// 30.
const NEVER_SOLID: [&str; 4] = [
    "func_illusionary",
    "func_ladder",
    "func_water",
    "func_monsterclip",
];

/// Whether a brush entity with this classname blocks the player.
///
/// Everything a map compiles into its own `BSPMODEL` is solid unless it is
/// documented otherwise: `func_wall`, `func_door`, `func_plat`,
/// `func_train`, `func_breakable`, `func_button` and the rest all stop the
/// player, and a map routinely builds a floor out of one. The exceptions
/// are in [`NEVER_SOLID`] and [`is_never_rendered`]'s `trigger_*` family.
#[must_use]
pub fn is_solid_brush(classname: &str) -> bool {
    !classname.starts_with("trigger_") && !NEVER_SOLID.contains(&classname)
}

/// Whether this entity is a `func_breakable`/`func_pushable` that has
/// already broken, and so must be left out of both the drawn list
/// ([`model_instances`]) and the solid list
/// ([`solid_model_instances`]) — a broken brush is gone from the world
/// entirely (`docs/FORMAT_SOURCES.md`, item 32). An entity with no
/// [`Breakable`] is never broken.
fn is_broken(registry: &Registry, entity: Entity) -> bool {
    registry
        .world
        .get::<&Breakable>(entity)
        .is_ok_and(|breakable| breakable.broken)
}

/// Collects one [`ModelInstance`] per brush entity that is solid to the
/// player (see [`is_solid_brush`]), in registry spawn order.
///
/// This is the collision counterpart of [`model_instances`]: the two lists
/// differ, because a brush can be solid and invisible (a `func_wall` with
/// an aaatrigger-style texture) or visible and non-solid
/// (`func_illusionary`).
#[must_use]
pub fn solid_model_instances(registry: &Registry) -> Vec<ModelInstance> {
    collect_solid_model_instances(registry, is_solid_brush)
}

/// Whether a brush entity with this classname blocks *monster* navigation
/// — as opposed to the player; see [`is_solid_brush`] for that side.
///
/// Identical to [`is_solid_brush`] except `func_monsterclip`, which counts
/// as solid here: TWHL wiki `func_monsterclip` (search-engine result
/// summary; see `docs/FORMAT_SOURCES.md` item 33) documents it as "solid
/// to monsters" though "not solid to players". Every other classname's
/// player-side solidity already matches what a monster should collide
/// with too (a door, wall or breakable blocks both), so this is
/// [`is_solid_brush`] plus exactly the one classname the two sides
/// disagree about.
#[must_use]
pub fn is_solid_to_monster(classname: &str) -> bool {
    is_solid_brush(classname) || classname == "func_monsterclip"
}

/// As [`solid_model_instances`], but for the collision model
/// `ohl-engine` builds for monster navigation (see [`is_solid_to_monster`]):
/// includes `func_monsterclip`, which [`solid_model_instances`] excludes.
#[must_use]
pub fn monster_solid_model_instances(registry: &Registry) -> Vec<ModelInstance> {
    collect_solid_model_instances(registry, is_solid_to_monster)
}

fn collect_solid_model_instances(
    registry: &Registry,
    is_solid: fn(&str) -> bool,
) -> Vec<ModelInstance> {
    let mut out = Vec::new();
    for (entity, model, transform, render, classname) in
        &mut registry
            .world
            .query::<(Entity, &BrushModel, &Transform, &RenderProps, &ClassName)>()
    {
        if !is_solid(&classname.0) || is_broken(registry, entity) {
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

/// The non-solid contents a `func_ladder`/`func_water` submodel
/// contributes to the collision model. Maps directly onto
/// `ohl_physics::hull::ContentsKind`; kept as this crate's own type (rather
/// than depending on `ohl-physics`) so `ohl-game` stays engine-agnostic —
/// see `ohl-engine`'s `level.rs` for the mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentsVolumeKind {
    /// `func_ladder`.
    Ladder,
    /// `func_water`, carrying which liquid its `skin` keyvalue selected.
    Water(Liquid),
}

/// Collects one ([`ModelInstance`], [`ContentsVolumeKind`]) pair per
/// `func_ladder`/`func_water` entity that carries a [`BrushModel`], in
/// registry spawn order.
///
/// This is the non-solid counterpart of [`solid_model_instances`]: neither
/// classname is solid (see [`NEVER_SOLID`]), but both still mark the space
/// their submodel occupies with their own contents (climbable, or a
/// swimmable liquid) once a host attaches them with
/// `ohl_physics::CollisionModel::attach_contents_brush`.
#[must_use]
pub fn contents_model_instances(registry: &Registry) -> Vec<(ModelInstance, ContentsVolumeKind)> {
    let mut out = Vec::new();
    for (entity, model, transform, render, classname) in
        &mut registry
            .world
            .query::<(Entity, &BrushModel, &Transform, &RenderProps, &ClassName)>()
    {
        let kind = match classname.0.as_str() {
            "func_ladder" => ContentsVolumeKind::Ladder,
            "func_water" => {
                let liquid = registry
                    .world
                    .get::<&Water>(entity)
                    .map_or(Liquid::Water, |water| water.0);
                ContentsVolumeKind::Water(liquid)
            }
            _ => continue,
        };
        out.push((
            ModelInstance {
                entity,
                model_index: model.0,
                origin: transform.origin,
                angles: transform.angles,
                render: *render,
            },
            kind,
        ));
    }
    out
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
        if is_never_rendered(&classname.0) || is_broken(registry, entity) {
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
    use super::{ContentsVolumeKind, contents_model_instances, model_instances};
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::{Liquid, Registry};
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

    /// `func_monsterclip` is documented (TWHL wiki, "func_monsterclip",
    /// search-engine result summary; see `docs/FORMAT_SOURCES.md` item 33)
    /// as an invisible brush that is not solid to the player; it must
    /// neither render nor attach to player collision.
    #[test]
    fn excludes_func_monsterclip_from_rendering() {
        let entities = vec![raw(&[("classname", "func_monsterclip"), ("model", "*7")])];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        assert!(model_instances(&registry).is_empty());
    }

    /// The collision counterpart of the render test above:
    /// `solid_model_instances` (what `ohl-engine`'s `attach_brush_collision`
    /// walks to build the shared player/monster `CollisionModel`) must not
    /// include a `func_monsterclip`, or the map's own monster-only clip
    /// brush would block the player exactly like a `func_wall` — the
    /// engine gap this milestone fixes.
    #[test]
    fn func_monsterclip_is_never_a_solid_brush() {
        assert!(!super::is_solid_brush("func_monsterclip"));
        let entities = vec![
            raw(&[("classname", "func_monsterclip"), ("model", "*7")]),
            raw(&[("classname", "func_wall"), ("model", "*8")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let instances = super::solid_model_instances(&registry);
        assert_eq!(instances.len(), 1, "only the func_wall should be solid");
        assert_eq!(instances[0].model_index, 8);
    }

    /// The monster-navigation counterpart of the test above:
    /// `func_monsterclip` is solid to a monster's own collision model (see
    /// `docs/FORMAT_SOURCES.md` item 33), unlike the player's.
    #[test]
    fn func_monsterclip_is_solid_to_monsters() {
        assert!(super::is_solid_to_monster("func_monsterclip"));
        let entities = vec![
            raw(&[("classname", "func_monsterclip"), ("model", "*7")]),
            raw(&[("classname", "func_wall"), ("model", "*8")]),
            raw(&[("classname", "func_illusionary"), ("model", "*9")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut instances = super::monster_solid_model_instances(&registry);
        instances.sort_by_key(|instance| instance.model_index);
        assert_eq!(
            instances.len(),
            2,
            "func_monsterclip and func_wall are solid to a monster; func_illusionary never is"
        );
        assert_eq!(instances[0].model_index, 7);
        assert_eq!(instances[1].model_index, 8);
    }

    #[test]
    fn contents_model_instances_collects_ladder_and_water_with_their_kind() {
        let entities = vec![
            raw(&[("classname", "func_ladder"), ("model", "*2")]),
            raw(&[("classname", "func_water"), ("model", "*3"), ("skin", "-5")]),
            raw(&[("classname", "func_wall"), ("model", "*4")]),
            raw(&[("classname", "trigger_once"), ("model", "*5")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut instances = contents_model_instances(&registry);
        instances.sort_by_key(|(instance, _)| instance.model_index);

        assert_eq!(instances.len(), 2, "only the ladder and the pool qualify");
        assert_eq!(instances[0].0.model_index, 2);
        assert_eq!(instances[0].1, ContentsVolumeKind::Ladder);
        assert_eq!(instances[1].0.model_index, 3);
        assert_eq!(instances[1].1, ContentsVolumeKind::Water(Liquid::Lava));
    }
}
