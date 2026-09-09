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

use crate::registry::{
    BrushCenter, Door, MomentaryDoor, MomentaryRotButton, MoverState, Pendulum, Platform, Pushable,
    Registry, RotButton, Rotator, TrackChange, Transform,
};
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
/// which is the shape the documentation describes in most detail (`height`
/// is defined against that brush) and the shape a compiler leaves the
/// geometry in: vertices stored relative to the brush, its world position
/// in the `origin` keyvalue.
///
/// A train authored *without* an origin brush has a `0 0 0` keyvalue and
/// world-baked vertices, so nothing cancels; placing it at the absolute
/// polyline coordinate (the previous behaviour here) teleports the whole
/// brush by the full magnitude of its own track coordinates the instant
/// the level loads — several thousand units on a real map — dropping any
/// player the map spawned standing inside it. That is observable, and it
/// is not what the published `path_corner`/`path_track` documentation
/// describes: a train "rides the path". A world-baked train has no origin
/// brush to measure that ride from, and the first node of the path it
/// rides is the one reference point its own data supplies, so this
/// function measures such a train's displacement from
/// [`TrackTrainState::first_node_position`] instead: zero at spawn, so the
/// car stays exactly where it was compiled, and growing exactly as far as
/// the train travels along its own polyline afterwards.
///
/// This is a *relative* placement, not a claim that the car is already
/// sitting on its first node. An earlier draft argued the latter; measured
/// across every `func_train`/`func_tracktrain` in this project's own cited
/// map list, well under half of the world-baked ones have their first node
/// anywhere inside the compiled car, so the rule rests only on the
/// reference-point argument above (`docs/FORMAT_SOURCES.md`, "Track trains
/// and paths").
///
/// The two cases are told apart by the `origin` keyvalue itself: a
/// compiler writes an origin brush's own world position there, and leaves
/// a brush entity built without one at `0 0 0`.
///
/// A geometric test — "do the entity's placed
/// [`crate::registry::BrushBounds`] contain its `origin` point" — was
/// written first, on the reasoning that an origin brush is part of the
/// entity and so must lie inside it. Measurement across every
/// `func_train`/`func_tracktrain` in this project's own cited map list
/// rejected it: a compiler drops the origin brush's own faces from the
/// model, so a real origin brush routinely sits flush with, or a unit or
/// two past, the compiled bounds. That test read a handful of ordinary
/// origin-brush trains as world-baked — one of them moving its spawn
/// placement by hundreds of units, the very regression this rule exists to
/// prevent, in the other direction — and sat within a few units of
/// flipping for most of the rest. The `origin` keyvalue separated the same
/// set with no misclassifications.
///
/// TODO(black-box): for a world-baked train whose first node happens to
/// sit inside the car it drew, the rule above makes the placement
/// self-consistent, and with it [`brush_center`] agrees with the renderer
/// and the collision model at spawn and stays with the car as it moves.
/// Measured across the 93 cited maps that is well under half of the
/// world-baked trains, not the general case. It does *not* fix
/// the pathological world-baked shape whose first node sits nowhere near
/// its compiled geometry: for that one `brush_center` still adds the path
/// displacement on top of an unrelated compiled midpoint. See
/// `docs/FORMAT_SOURCES.md` item 31 and this module's own
/// `a_tracktrain_without_an_origin_brush_gives_a_wrong_brush_center` test,
/// which pins that remaining case so a future fix has to update it
/// deliberately.
#[must_use]
pub fn track_train_transform(registry: &Registry, entity: Entity) -> (Vec3, Option<f32>) {
    let Some((state, train, _, reference)) = train_placement_reference(registry, entity) else {
        return (Vec3::ZERO, None);
    };
    // A train being carried by a `func_trackchange`/`func_trackautochange`
    // is displaced (and spun) bodily by the platform under it, on top of
    // wherever its own chain currently puts it; both are zero for every
    // train no platform is carrying. See
    // `ohl_game::track_train::TrackTrainState::set_carry`.
    let (carry_offset, carry_yaw) = state.carry();
    // A train that does not turn to face its path keeps the `angles` its
    // own map spawned it at — the sentence `docs/FORMAT_SOURCES.md`
    // ("Track trains and paths") already records for this entity: this
    // project "turns a `func_tracktrain` to face its active segment but
    // leaves a `func_train` at its spawned `angles`". Leaving it *at* them
    // means posing it there; reporting no rotation at all instead poses it
    // at zero, which is a different placement for every `func_train` whose
    // map turned it. A `func_train`'s `angles` is free to mean an
    // orientation, unlike a `func_door`'s (whose published meaning is the
    // move direction, `movedir_from_angles`): a train's direction of travel
    // comes from its `path_corner` chain, not from a keyvalue.
    let authored_yaw = (!train.turns_to_face)
        .then(|| authored_angles(registry, entity))
        .filter(|yaw| yaw.is_finite() && *yaw != 0.0);
    (
        state.position() - reference + carry_offset,
        state
            .yaw_degrees(&train)
            .or(authored_yaw)
            .map(|yaw| yaw + carry_yaw)
            .or((carry_yaw != 0.0).then_some(carry_yaw)),
    )
}

/// `entity`'s authored yaw, in degrees: the middle component of its
/// `angles` keyvalue, in the same convention
/// [`crate::registry::movedir_from_angles`] reads.
fn authored_angles(registry: &Registry, entity: Entity) -> f32 {
    registry
        .world
        .get::<&Transform>(entity)
        .map_or(0.0, |transform| transform.angles.y)
}

/// The pieces every train placement is built from: its live state, its
/// static keyvalues, the `origin` keyvalue its geometry was placed by, and
/// the world-space *reference point* its path displacement (and, for a
/// `func_tracktrain`, its yaw) is measured from — the origin brush for an
/// ordinary train, the chain's first node for a world-baked one. See
/// [`track_train_transform`]'s doc comment for why those two.
fn train_placement_reference(
    registry: &Registry,
    entity: Entity,
) -> Option<(
    hecs::Ref<'_, TrackTrainState>,
    hecs::Ref<'_, TrackTrain>,
    Vec3,
    Vec3,
)> {
    let state = registry.world.get::<&TrackTrainState>(entity).ok()?;
    let train = registry.world.get::<&TrackTrain>(entity).ok()?;
    let authored = registry
        .world
        .get::<&Transform>(entity)
        .map_or(Vec3::ZERO, |transform| transform.origin);
    let reference = if train_geometry_is_world_baked(authored) {
        state.first_node_position()
    } else {
        authored
    };
    Some((state, train, authored, reference))
}

/// The point a `func_tracktrain` turns about, expressed in the *compiled*
/// frame its submodel's own vertices are stored in — the frame
/// [`ohl_physics::CollisionModel::set_brush_pose`]'s `pivot` and
/// `ohl-engine`'s render placement both take.
///
/// A train built around an origin brush has its geometry compiled relative
/// to that brush, so the brush — the point the published `height`
/// keyvalue is defined against, and therefore the point the car rides its
/// track on ([`track_train_transform`]) — is the compiled frame's own
/// `(0, 0, 0)`. A world-baked train has no origin brush, and the same
/// first-node rule that stands in for one when measuring its *translation*
/// stands in for one here: its first `path_track`, whose world coordinates
/// are also compiled-frame coordinates because a world-baked submodel's
/// vertices are absolute.
///
/// `Vec3::ZERO` for any entity that is not a train, which is also the
/// no-op every other brush entity wants.
#[must_use]
pub fn track_train_pivot(registry: &Registry, entity: Entity) -> Vec3 {
    let Some((_, _, authored, reference)) = train_placement_reference(registry, entity) else {
        return Vec3::ZERO;
    };
    reference - authored
}

/// Whether `entity`'s brush geometry was compiled in absolute world space
/// rather than relative to an origin brush: its `origin` keyvalue is
/// `0 0 0`, which is what a compiler leaves a brush entity with no origin
/// brush at. See [`track_train_transform`]'s doc comment for why this, and
/// not the geometric bounds-containment test tried first, is the
/// discriminator.
fn train_geometry_is_world_baked(authored: Vec3) -> bool {
    authored == Vec3::ZERO
}

/// How far a `momentary_door` has slid toward whichever
/// `momentary_rot_button` is currently driving it, from its own
/// `0.0..=1.0` [`MomentaryDoor::fraction`] (there is no open/close timer to
/// derive a fraction from — it is pushed toward a commanded value every
/// tick a driving button changes, see `crate::logic::Simulation::
/// drive_momentary_rot_button`). `Vec3::ZERO` for any entity without one.
#[must_use]
pub fn momentary_door_offset(registry: &Registry, entity: Entity) -> Vec3 {
    registry
        .world
        .get::<&MomentaryDoor>(entity)
        .map_or(Vec3::ZERO, |door| {
            door.movedir * door.travel_distance * door.fraction.clamp(0.0, 1.0)
        })
}

/// How far a `func_pushable` has been pushed from where its geometry was
/// compiled, straight out of its own [`Pushable::offset`] (the push
/// integration itself lives in `ohl-engine`, which is the only crate that
/// can see the player and the collision model; see
/// `ohl_engine::pushables`). `Vec3::ZERO` for any entity without one.
#[must_use]
pub fn pushable_offset(registry: &Registry, entity: Entity) -> Vec3 {
    registry
        .world
        .get::<&Pushable>(entity)
        .map_or(Vec3::ZERO, |pushable| pushable.offset)
}

/// How far a `func_trackchange`/`func_trackautochange` platform has
/// travelled from where its geometry was compiled, straight out of its own
/// [`TrackChange::offset`]. `Vec3::ZERO` for any entity without one.
#[must_use]
pub fn track_change_offset(registry: &Registry, entity: Entity) -> Vec3 {
    registry
        .world
        .get::<&TrackChange>(entity)
        .map_or(Vec3::ZERO, |change| change.offset())
}

/// The signed axis and angle (degrees) a
/// `func_trackchange`/`func_trackautochange` platform is currently spun
/// to, straight out of its own [`TrackChange::degrees`]. `Vec3::ZERO`/
/// `0.0` for any entity without one, so [`mover_rotation`] can chain it
/// with every other rotating mover.
#[must_use]
pub fn track_change_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&TrackChange>(entity)
        .map_or((Vec3::ZERO, 0.0), |change| (change.axis, change.degrees()))
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
        + momentary_door_offset(registry, entity)
        + pushable_offset(registry, entity)
        + track_change_offset(registry, entity)
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

/// How far a `func_rot_button` has swung, in degrees, from the same
/// shared `speed`/`distance`/`state`/`timer` shape [`door_rotation_degrees`]
/// reads (see [`crate::registry::RotButton`]'s own doc comment for why it
/// is a separate component rather than a [`Door`]). `Vec3::ZERO`/`0.0` for
/// any entity without a [`RotButton`].
#[must_use]
pub fn rot_button_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&RotButton>(entity)
        .map_or((Vec3::ZERO, 0.0), |button| {
            let fraction =
                mover_fraction(button.speed, button.distance, button.state, button.timer);
            (button.axis, button.distance * fraction)
        })
}

/// How far a `momentary_rot_button` has turned, in degrees, directly from
/// its own `0.0..=1.0` [`MomentaryRotButton::fraction`] (there is no
/// open/close timer to derive a fraction from — it is driven every tick
/// `use` is held, see `crate::logic::Simulation::
/// drive_momentary_rot_button`). `Vec3::ZERO`/`0.0` for any entity without
/// one.
#[must_use]
pub fn momentary_rot_button_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&MomentaryRotButton>(entity)
        .map_or((Vec3::ZERO, 0.0), |button| {
            (button.axis, button.distance * button.fraction)
        })
}

/// How far a `func_pendulum` has swung, in degrees, directly from its own
/// [`Pendulum::angle_deg`] (already the fully resolved pose — see
/// `crate::logic::Simulation::advance_pendulums`). `Vec3::ZERO`/`0.0` for
/// any entity without one.
#[must_use]
pub fn pendulum_degrees(registry: &Registry, entity: Entity) -> (Vec3, f32) {
    registry
        .world
        .get::<&Pendulum>(entity)
        .map_or((Vec3::ZERO, 0.0), |pendulum| {
            (pendulum.axis, pendulum.angle_deg)
        })
}

/// The signed rotation axis and current angle (degrees) a rotating brush
/// mover — `func_door_rotating`, `func_rotating`, `func_rot_button`,
/// `momentary_rot_button`, or `func_pendulum`, mutually exclusive
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
    let (axis, degrees) = rotator_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    let (axis, degrees) = rot_button_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    let (axis, degrees) = momentary_rot_button_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    let (axis, degrees) = pendulum_degrees(registry, entity);
    if axis != Vec3::ZERO {
        return (axis, degrees);
    }
    track_change_degrees(registry, entity)
}

/// The rotation a brush entity's geometry is currently posed at, as the
/// three values every consumer of a pose needs: the signed rotation axis,
/// the angle about it in degrees, and the pivot to rotate about *in the
/// submodel's own compiled frame* (the frame
/// `ohl_physics::CollisionModel::set_brush_pose`'s `pivot` parameter and
/// `ohl-engine`'s render placement are both expressed in). `Vec3::ZERO`
/// for the axis means "no rotation", so a caller can branch on it exactly
/// as it already branches on [`mover_rotation`].
///
/// This is the superset [`mover_rotation`] used to be: a rotating mover
/// (`func_door_rotating`/`func_rotating`/`func_rot_button`/
/// `momentary_rot_button`/`func_pendulum`) reports what it always did,
/// about its own compiled origin, *and* a `func_tracktrain` now reports the
/// yaw it is drawn at — the same [`TrackTrainState::yaw_degrees`] the
/// renderer has always applied — about [`track_train_pivot`].
///
/// Reporting it here is what makes one transform serve the renderer, the
/// collision hull, [`brush_center`]'s `use`-proximity point and a rider's
/// tangential ride velocity: before this, only the renderer turned a
/// `func_tracktrain`, so through a bend its drawn car and its collision
/// hull pointed different ways and a passenger standing away from the
/// pivot was carried by the car's translation alone and scraped off
/// against the geometry the track runs through (`docs/FORMAT_SOURCES.md`,
/// "Riding movers").
///
/// The sign and offset of that yaw are `TrackTrainState::yaw_degrees`'s,
/// unchanged and with no added half turn: which of the two physically
/// plausible conventions the published game uses is a project-determined
/// finding from a black-box comparison of this project's own renders
/// against public screenshots, not a fact any public page states. A plain
/// `func_train` reports no yaw at all (it keeps its authored `angles`),
/// because `yaw_degrees` returns `None` unless the entity is the
/// turns-to-face kind.
#[must_use]
pub fn brush_pose_rotation(registry: &Registry, entity: Entity) -> (Vec3, f32, Vec3) {
    let (axis, degrees) = mover_rotation(registry, entity);
    if axis != Vec3::ZERO {
        // A rotating mover requires an origin brush, so its compiled
        // geometry is already centred on the point it turns about.
        return (axis, degrees, Vec3::ZERO);
    }
    match track_train_transform(registry, entity).1 {
        // Yaw is a rotation about the world up axis, matching
        // `crate::registry::movedir_from_angles`'s convention.
        Some(yaw) => (Vec3::Z, yaw, track_train_pivot(registry, entity)),
        None => (Vec3::ZERO, 0.0, Vec3::ZERO),
    }
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
    let (axis, degrees, pivot_local) = brush_pose_rotation(registry, entity);
    let offset = brush_offset(registry, entity);
    if axis == Vec3::ZERO {
        return Some(center + offset);
    }
    let authored = registry
        .world
        .get::<&Transform>(entity)
        .map_or(Vec3::ZERO, |transform| transform.origin);
    // `BrushCenter` is the *placed* centre (compiled midpoint plus the
    // `origin` keyvalue), so `center - authored` takes it back into the
    // compiled frame `pivot_local` is expressed in; rotate there, then put
    // it back and add however far a translating mover has travelled. A
    // rotating mover has `pivot_local == Vec3::ZERO` and no translation, so
    // this reduces exactly to the swing-about-the-origin-keyvalue rule it
    // has always used.
    let rotation = Quat::from_axis_angle(axis.normalize(), degrees.to_radians());
    let local = center - authored;
    Some(authored + offset + pivot_local + rotation * (local - pivot_local))
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

    /// A `func_tracktrain` authored *without* an origin brush has
    /// world-baked geometry compiled wherever the map editor happened to
    /// place it — unrelated to the path it rides — and a `0 0 0` `origin`
    /// keyvalue (see `track_train_transform`'s own doc comment). Render
    /// and collision get this right: they add the train's path
    /// displacement to that same zero `origin`. But `brush_center` is
    /// still wrong for *this* fixture's shape: its `BrushCenter` is the
    /// compiled bounds midpoint (the unrelated editor location) plus that
    /// zero `origin`, and `brush_offset` then adds the path displacement
    /// on top of it, rather than replacing it — so the proximity point
    /// drifts away from the train as soon as it leaves its spawn node.
    ///
    /// Note what this fixture deliberately is *not*: a world-baked train
    /// whose first node sits inside the car it drew, the minority shape
    /// (well under half of the 93 cited maps' world-baked trains, per
    /// `docs/FORMAT_SOURCES.md` item 31) against which
    /// `track_train_transform`'s world-baked rule stays self-consistent. For
    /// that shape the displacement is zero at spawn and `brush_center`
    /// agrees with the car. This fixture's node is 500 units from its
    /// compiled geometry, so it keeps pinning the documented remaining gap
    /// (`docs/FORMAT_SOURCES.md` item 31); a future fix must update this
    /// test deliberately rather than leave it silently passing on the old
    /// sum.
    #[test]
    fn a_tracktrain_without_an_origin_brush_gives_a_wrong_brush_center() {
        let train_kv = [
            ("classname", "func_tracktrain"),
            ("targetname", "tram"),
            ("target", "node1"),
            ("model", "*1"),
            ("height", "0"),
            // No `origin` keyvalue: this is the no-origin-brush shape.
        ];
        let defs = parse_entities(
            &[
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
                    ("origin", "100 0 0"),
                ]),
            ],
            &Limits::default(),
        );
        let mut bounds = BTreeMap::new();
        // World-baked geometry compiled far from the path, at the
        // editor's own build location — this project's own synthetic
        // fixture, not derived from any payload.
        bounds.insert(1u32, ([492.0, 292.0, 2.0], [508.0, 308.0, 18.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        crate::track_train::spawn_all(&mut registry);
        let entity = registry.find("tram")[0];

        {
            let mut state = registry
                .world
                .get::<&mut crate::track_train::TrackTrainState>(entity)
                .expect("a resolved path");
            state.turn_on();
            state.advance(0.5);
        }

        let center = brush_center(&registry, entity).expect("a placed centre");
        // Correct placement would be the train's own path position,
        // `(50, 0, 0)` — halfway from `node1` to `node2` at `speed 100`.
        // The documented-wrong sum instead adds that onto the unrelated
        // compiled centre `(500, 300, 10)`, first turned about the pivot
        // this world-baked shape stands in with (its first node, the
        // world origin here) by the car's own pose: the chain runs along
        // `+X`, so the car travels at 0 degrees and is posed a
        // `crate::track_train::COMPILED_FACING_OFFSET_DEGREES` half turn
        // from that, taking `(500, 300)` to `(-500, -300)`. Turning an
        // unrelated compiled midpoint is exactly as wrong as translating
        // it was, and just as deliberately pinned here.
        assert!(
            (center - Vec3::new(-450.0, -300.0, 10.0)).length() < 1e-2,
            "expected the documented-wrong sum, got {center:?}"
        );
    }

    /// Builds a one-train registry: a `func_train` (or `func_tracktrain`)
    /// with an origin brush, on a two-node chain running along `+X`, with
    /// `keys` merged into the train's own keyvalues.
    fn registry_with_train(keys: &[(&str, &str)]) -> Registry {
        let mut train = vec![
            ("targetname", "car"),
            ("target", "node1"),
            ("model", "*1"),
            ("origin", "0 0 0"),
            ("height", "0"),
        ];
        train.extend_from_slice(keys);
        let defs = parse_entities(
            &[
                raw(&train),
                raw(&[
                    ("classname", "path_corner"),
                    ("targetname", "node1"),
                    ("target", "node2"),
                    ("origin", "0 0 0"),
                ]),
                raw(&[
                    ("classname", "path_corner"),
                    ("targetname", "node2"),
                    ("origin", "100 0 0"),
                ]),
            ],
            &Limits::default(),
        );
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([-32.0, -4.0, 0.0], [32.0, 4.0, 64.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        crate::track_train::spawn_all(&mut registry);
        registry
    }

    /// A `func_train` is posed at the `angles` its own map spawned it at:
    /// the published pages describe `func_tracktrain` as turning to face
    /// its next node and `func_train` without that behaviour, so a
    /// `func_train`'s `angles` is an orientation and nothing overrides it
    /// (`docs/FORMAT_SOURCES.md`, "Track trains and paths"). Reporting no
    /// rotation instead posed every turned `func_train` at zero — a door
    /// leaf built across its own doorway rather than in it.
    #[test]
    fn a_func_train_keeps_the_angles_its_map_spawned_it_at() {
        let registry = registry_with_train(&[("classname", "func_train"), ("angles", "0 90 0")]);
        let entity = registry.find("car")[0];
        let (_, yaw) = super::track_train_transform(&registry, entity);
        assert_eq!(yaw, Some(90.0));
        let (axis, degrees, _) = super::brush_pose_rotation(&registry, entity);
        assert_eq!(axis, Vec3::Z);
        assert!((degrees - 90.0).abs() < 1e-3);
    }

    /// The same train with no `angles` of its own is posed unrotated, as
    /// before: an absent keyvalue is not an orientation.
    #[test]
    fn a_func_train_without_angles_is_still_posed_unrotated() {
        let registry = registry_with_train(&[("classname", "func_train")]);
        let entity = registry.find("car")[0];
        assert_eq!(
            super::track_train_transform(&registry, entity).1,
            None,
            "no keyvalue, no rotation"
        );
    }

    /// A `func_tracktrain` still faces its active segment, whatever
    /// `angles` its map spawned it at: the published behaviour this
    /// project already implemented, which the `func_train` rule above must
    /// not disturb.
    #[test]
    fn a_func_tracktrain_still_faces_its_segment_over_its_own_angles() {
        let registry =
            registry_with_train(&[("classname", "func_tracktrain"), ("angles", "0 90 0")]);
        let entity = registry.find("car")[0];
        assert_eq!(
            super::track_train_transform(&registry, entity).1,
            Some(180.0),
            "the chain runs along +X, so the car travels at 0 degrees and is posed a \
             `ohl_game::track_train::COMPILED_FACING_OFFSET_DEGREES` half turn from that"
        );
    }
}
