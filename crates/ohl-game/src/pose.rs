//! Where a brush entity's geometry currently sits, in world space.
//!
//! Every consumer of a brush mover's position — the renderer's draw
//! transform, the collision model's attached brush pose, and the
//! proximity point a `use` press targets ([`crate::find_usable_within`]) —
//! has to agree on one answer, or the player collides with a door that is
//! drawn somewhere else, or presses `use` at a point the door never
//! occupies. This module is that one answer; `ohl-engine` builds both its
//! render transform and its collision pose out of the functions below
//! rather than deriving a second copy of the same arithmetic.
//!
//! The shape a placed brush entity takes is uniform across every kind of
//! brush entity this project loads:
//!
//! 1. the submodel's own compiled geometry, as the map compiler wrote it;
//! 2. rotated about the submodel's own local `(0, 0, 0)`, when the entity
//!    is a rotating mover (see [`mover_rotation`]);
//! 3. translated by the entity's `origin` keyvalue;
//! 4. translated further by however far a translating mover has currently
//!    travelled (see [`brush_offset`]).
//!
//! Steps 3 and 4 are what `ohl-engine`'s brush-collision attachment
//! already applies unconditionally to every brush entity it attaches, so
//! they apply here unconditionally too: whether a particular map's
//! particular brush entity leaves `origin` at `0 0 0` (the common case for
//! geometry compiled in absolute world space) or sets it (the required
//! case for an entity built around an "origin brush", whose geometry is
//! compiled *relative to* that brush) is a per-entity fact this module
//! never has to branch on. See `docs/FORMAT_SOURCES.md`, "Entity
//! keyvalues and map logic".

use glam::{Quat, Vec3};
use hecs::Entity;

use crate::registry::{BrushCenter, Door, MoverState, Platform, Registry, Rotator, Transform};
use crate::track_train::{TrackTrain, TrackTrainState};

/// The `0.0..=1.0` progress fraction [`mover_offset`] (a translating
/// door/platform) and [`door_rotation_degrees`] (a rotating one) both
/// scale their travel distance by, factored out so the two stay in
/// lock-step by construction rather than by two doc comments claiming they
/// mirror each other.
#[must_use]
pub fn mover_fraction(speed: f32, travel_distance: f32, state: MoverState, timer: f32) -> f32 {
    let travel_seconds = if speed > 0.0 {
        travel_distance / speed
    } else {
        0.0
    };
    if travel_seconds <= 0.0 {
        // An instantly-travelling mover has no intermediate position to
        // show; it is either where it started or fully open.
        return f32::from(u8::from(state == MoverState::Open));
    }
    let progress = (timer / travel_seconds).clamp(0.0, 1.0);
    match state {
        MoverState::Closed => 0.0,
        MoverState::Open => 1.0,
        MoverState::Opening => 1.0 - progress,
        MoverState::Closing => progress,
    }
}

/// How far a translating brush mover has slid along its move direction,
/// from `speed`/`travel_distance`/`movedir` and its own current
/// `state`/`timer` (the shape [`Door`] and [`Platform`] both carry
/// identically; see `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
/// logic"). Shared by [`door_offset`] and [`platform_offset`] so the two
/// staying in lock-step is a compile-time fact.
#[must_use]
pub fn mover_offset(
    speed: f32,
    travel_distance: f32,
    movedir: Vec3,
    state: MoverState,
    timer: f32,
) -> Vec3 {
    movedir * travel_distance * mover_fraction(speed, travel_distance, state, timer)
}

/// How far a `func_door` has slid along its move direction, from the state
/// machine [`crate::Simulation`] advances. `Vec3::ZERO` for an entity with
/// no [`Door`], and for a rotating door (whose `movedir` is left zero).
#[must_use]
pub fn door_offset(registry: &Registry, entity: Entity) -> Vec3 {
    let Ok(door) = registry.world.get::<&Door>(entity) else {
        return Vec3::ZERO;
    };
    mover_offset(
        door.speed,
        door.travel_distance,
        door.movedir,
        door.state,
        door.timer,
    )
}

/// How far a `func_plat`/`func_platform` has slid along its move
/// direction, from the state machine [`crate::Simulation`] advances.
#[must_use]
pub fn platform_offset(registry: &Registry, entity: Entity) -> Vec3 {
    let Ok(platform) = registry.world.get::<&Platform>(entity) else {
        return Vec3::ZERO;
    };
    mover_offset(
        platform.speed,
        platform.travel_distance,
        platform.movedir,
        platform.state,
        platform.timer,
    )
}

/// A `func_train`/`func_tracktrain`'s current placement, read from the
/// `ohl-game`-side [`TrackTrainState`] the map logic simulation advances
/// each tick (see `crates/ohl-game/src/track_train.rs`): a world-space
/// *delta* offset from the entity's own `origin` keyvalue, and, for a
/// `func_tracktrain` (which the public documentation says turns to face
/// the next `path_track`), the yaw to face instead of the entity's own
/// spawned `angles`. Returns `(Vec3::ZERO, None)` for any entity that is
/// not a train with a resolved path (a caller then falls back to the
/// door/static placement path).
///
/// Every caller adds this to the entity's `origin` keyvalue (the renderer
/// through `ModelInstance::origin`, the collision model through
/// `Level::sync_brush_collision`), so returning `position() - origin`
/// places the train *at* [`TrackTrainState::position`] — which is what the
/// public documentation describes: a train rides the path with its origin
/// brush on it, the compiler writes that origin brush's position into the
/// entity's `origin` keyvalue and stores the submodel's geometry relative
/// to it, and `height` is documented as the offset "above the path_track
/// that the train will ride, **based on the location of the train's origin
/// brush**" (`docs/FORMAT_SOURCES.md`, "Track trains and paths"). A train
/// is therefore drawn and collided wherever its path currently puts it,
/// not wherever its brushes happened to be built — a map may author the
/// brush anywhere and let the first `path_track` place it at spawn.
///
/// The `docs/CLEAN_ROOM.md`-governed `.plan/fidelity-round-2.md` finding
/// E1 (returning the raw polyline coordinate, which the caller then adds
/// the `origin` keyvalue to and so double-applies it) stays fixed: the
/// `origin` keyvalue is subtracted here precisely so the sum cancels to
/// the absolute position exactly once. Subtracting the chain's first node
/// instead — the previous behaviour — cancelled to a zero offset at spawn
/// and so left the train frozen at wherever it was compiled, however far
/// from its own track that is.
///
/// The cancellation is exact only for a train that has an origin brush,
/// which is the only shape the documentation describes (`height` is
/// defined against that brush) and the shape a compiler leaves the
/// geometry in: vertices stored relative to the brush, its world position
/// in the `origin` keyvalue. A train authored *without* one has a `0 0 0`
/// keyvalue and world-baked vertices, so nothing cancels and it is placed
/// at the absolute polyline coordinate — the same thing an engine that
/// simply assigns the entity's origin from the path does, and a map shape
/// the documentation gives no other meaning to.
#[must_use]
pub fn track_train_transform(registry: &Registry, entity: Entity) -> (Vec3, Option<f32>) {
    let Ok(state) = registry.world.get::<&TrackTrainState>(entity) else {
        return (Vec3::ZERO, None);
    };
    let Ok(train) = registry.world.get::<&TrackTrain>(entity) else {
        return (Vec3::ZERO, None);
    };
    let authored = registry
        .world
        .get::<&Transform>(entity)
        .map_or(Vec3::ZERO, |transform| transform.origin);
    (state.position() - authored, state.yaw_degrees(&train))
}

/// How far a brush entity has moved from where its geometry was compiled
/// and placed — the sum of every translating-mover displacement it could
/// carry (they are mutually exclusive in practice, and each is zero for an
/// entity without that component).
#[must_use]
pub fn brush_offset(registry: &Registry, entity: Entity) -> Vec3 {
    door_offset(registry, entity)
        + platform_offset(registry, entity)
        + track_train_transform(registry, entity).0
}

/// How far a `func_door_rotating` has swung, in degrees, from the same
/// shared [`Door`] `state`/`timer` [`mover_offset`] reads for a
/// translating door — see [`mover_fraction`]. `Vec3::ZERO`/`0.0` for a
/// [`Door`] with no `rotation_axis` (an ordinary translating `func_door`).
#[must_use]
pub fn door_rotation_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    let Ok(door) = registry.world.get::<&Door>(entity) else {
        return (Vec3::ZERO, 0.0);
    };
    let Some(axis) = door.rotation_axis else {
        return (Vec3::ZERO, 0.0);
    };
    let fraction = mover_fraction(door.speed, door.travel_distance, door.state, door.timer);
    (axis, door.travel_distance * fraction)
}

/// How far a `func_rotating` has spun, in degrees, from its own
/// continuously-accumulated [`Rotator::angle_deg`]. `Vec3::ZERO`/`0.0` for
/// any entity without a [`Rotator`].
#[must_use]
pub fn rotator_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&Rotator>(entity)
        .map_or((Vec3::ZERO, 0.0), |rotator| {
            (rotator.axis, rotator.angle_deg)
        })
}

/// The signed rotation axis and current angle (degrees) a rotating brush
/// mover — `func_door_rotating` or `func_rotating`, mutually exclusive
/// components on any one entity — is currently posed at. `Vec3::ZERO`/
/// `0.0` (no rotation) for every other brush entity, so a caller can
/// branch on `axis != Vec3::ZERO` to tell a rotating mover from a
/// translating one.
#[must_use]
pub fn mover_rotation(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    let (axis, degrees) = door_rotation_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    rotator_degrees(registry, entity)
}

/// Where `entity`'s brush geometry is centred *right now*, in world space,
/// or `None` for an entity that carries no [`BrushCenter`] (a point entity,
/// or a brush entity whose submodel bounds were unavailable at load).
///
/// [`BrushCenter`] itself is the entity's resting placed centre (compiled
/// bounds midpoint plus its `origin` keyvalue). This adds whatever its own
/// state machine has moved it by since: a rotating mover swings its centre
/// about the pivot its `origin` keyvalue names, and a translating one
/// slides its centre along its move direction — the same two cases, in the
/// same order, that `ohl-engine`'s render transform and collision brush
/// pose are built from, so "use it where it looks like it is" holds for a
/// mover caught mid-travel exactly as it does for one at rest.
#[must_use]
pub fn brush_center(registry: &Registry, entity: Entity) -> Option<Vec3> {
    let center = registry.world.get::<&BrushCenter>(entity).ok()?.0;
    let (axis, degrees) = mover_rotation(registry, entity);
    if axis == Vec3::ZERO {
        return Some(center + brush_offset(registry, entity));
    }
    let pivot = registry
        .world
        .get::<&Transform>(entity)
        .map_or(Vec3::ZERO, |transform| transform.origin);
    let rotation = Quat::from_axis_angle(axis.normalize(), degrees.to_radians());
    Some(pivot + rotation * (center - pivot))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glam::Vec3;

    use super::brush_center;
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::{Door, MoverState, Registry};
    use ohl_formats::bsp30::Entity as RawEntity;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// Builds a one-door registry whose submodel `*1` is compiled relative
    /// to its origin brush (the shape an entity with an origin brush
    /// always takes), with `keys` merged in.
    fn registry_with_door(keys: &[(&str, &str)]) -> Registry {
        let mut pairs = vec![
            ("targetname", "door1"),
            ("model", "*1"),
            ("origin", "1000 -500 64"),
        ];
        pairs.extend_from_slice(keys);
        let defs = parse_entities(&[raw(&pairs)], &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([-8.0, 0.0, -48.0], [8.0, 64.0, 48.0]));
        Registry::build(&defs, &bounds, &Limits::default())
    }

    fn set_state(registry: &Registry, state: MoverState) {
        let entity = registry.find("door1")[0];
        let mut door = registry
            .world
            .get::<&mut Door>(entity)
            .expect("the fixture entity is a door");
        door.state = state;
        door.timer = 0.0;
    }

    /// A closed rotating door's proximity point is its placed resting
    /// centre — compiled centre plus the `origin` keyvalue.
    #[test]
    fn a_closed_rotating_door_uses_its_placed_centre() {
        let registry = registry_with_door(&[
            ("classname", "func_door_rotating"),
            ("distance", "90"),
            ("speed", "120"),
        ]);
        let center = brush_center(&registry, registry.find("door1")[0]).expect("a placed centre");
        assert!((center - Vec3::new(1000.0, -468.0, 64.0)).length() < 1e-3);
    }

    /// Once open, it swings about the pivot its `origin` keyvalue names,
    /// exactly the way its collision brush and its draw transform do — a
    /// quarter turn about `+Z` carries the local `(0, 32, 0)` centre to
    /// local `(-32, 0, 0)`.
    #[test]
    fn an_open_rotating_door_follows_its_swung_pose() {
        let registry = registry_with_door(&[
            ("classname", "func_door_rotating"),
            ("distance", "90"),
            ("speed", "120"),
        ]);
        set_state(&registry, MoverState::Open);
        let center = brush_center(&registry, registry.find("door1")[0]).expect("a placed centre");
        assert!((center - Vec3::new(968.0, -500.0, 64.0)).length() < 1e-3);
    }

    /// A translating door slides its proximity point along its own move
    /// direction by its own travel distance, the same offset the renderer
    /// and the collision model apply.
    #[test]
    fn an_open_translating_door_follows_its_slid_pose() {
        let registry = registry_with_door(&[
            ("classname", "func_door"),
            ("angle", "-1"),
            ("speed", "100"),
            ("lip", "0"),
        ]);
        let entity = registry.find("door1")[0];
        let travel = registry
            .world
            .get::<&Door>(entity)
            .expect("the fixture entity is a door")
            .travel_distance;
        assert!(travel > 0.0, "the fixture door has somewhere to travel");
        set_state(&registry, MoverState::Open);
        let center = brush_center(&registry, entity).expect("a placed centre");
        assert!((center - Vec3::new(1000.0, -468.0, 64.0 + travel)).length() < 1e-3);
    }
}
