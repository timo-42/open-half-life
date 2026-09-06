//! Applying an active `trigger_camera` sequence to the world camera the
//! host renders from and reads through [`crate::Game::eye_position`], and
//! gating player input while its documented "Freeze Player" spawnflag is
//! set.
//!
//! The state machine itself (path traversal, the `wait` hold timer,
//! completion firing `target`) lives in `ohl_game::camera`, ticked by
//! `ohl_game::logic::Simulation` exactly like a door or a track train; see
//! that module and `docs/FORMAT_SOURCES.md` ("Camera sequences") for the
//! public documentation and the `TODO(black-box)` choices this behaviour is
//! built on. This module only reads the resolved state each frame and
//! decides what the camera and the input latch do with it — the same
//! division `crate::render`'s `track_train_transform` already draws between
//! `ohl-game`'s resolved position and `ohl-engine`'s use of it.
//!
//! # Save/load
//!
//! `TriggerCameraState` is not part of any save section (see its own doc
//! comment): a save/load taken mid-sequence loses the active/hold/path
//! progress and resumes with the sequence dormant, the same known gap
//! `ohl_game::track_train::TrackTrainState` already has for a `func_train`
//! mid-route. Adding it to `ohl_game::logic::SimulationState`
//! (`SECTION_SIMULATION`) was investigated for this change and rejected:
//! that section is encoded with `postcard`, which is not a self-describing
//! format, so a struct field added even with `#[serde(default)]` fails to
//! decode any save file written before the field existed (confirmed with a
//! standalone reproduction: `postcard::from_bytes` returns
//! `DeserializeUnexpectedEnd` for exactly this case) rather than filling in
//! the default the attribute names. A real fix needs either a version-
//! tagged sub-encoding for this section or a self-describing replacement,
//! which is out of scope here; `TODO(black-box)` (really
//! `TODO(follow-up)`, tracked for whoever next touches `SECTION_SIMULATION`
//! or `SimulationState`'s own encoding).

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::registry::{Target, Transform, TriggerCamera};
use ohl_game::{Registry, TriggerCameraState};
use ohl_render::FreeFlyCamera;

use crate::level::Level;

/// The steepest pitch this override ever assigns, matching
/// `ohl_render::FreeFlyCamera`'s own clamp so an aim-at-point computation
/// can never push the view past parallel with the up axis.
const MAX_PITCH_DEGREES: f32 = 89.0;

/// The lowest-id entity currently running an active `trigger_camera`
/// sequence, and its two static/runtime halves, or `None` when no sequence
/// is active. Bounded by the map's own entity count; picking the lowest id
/// when more than one sequence is somehow active at once keeps the choice
/// deterministic rather than iteration-order-dependent.
fn active_camera(registry: &Registry) -> Option<(Entity, TriggerCamera, TriggerCameraState)> {
    registry
        .world
        .query::<(Entity, &TriggerCamera, &TriggerCameraState)>()
        .iter()
        .filter(|(_, _, state)| state.is_active())
        .map(|(entity, camera, state)| (entity, camera.clone(), state.clone()))
        .min_by_key(|(entity, _, _)| entity.id())
}

/// Whether an active `trigger_camera` sequence's documented "Freeze Player"
/// spawnflag is set, as the state stood at the *start* of this step (before
/// this step's own `Simulation::tick` may activate or complete a sequence) —
/// the same one-tick lag `ohl_game::scripts::ScriptActivation` already has
/// between a trigger firing and its effect landing, since both ride the
/// same `Simulation::tick`-then-consumed-next-phase shape.
#[must_use]
pub(crate) fn freeze_active(level: &Level) -> bool {
    active_camera(&level.registry).is_some_and(|(_, camera, _)| camera.freeze_player)
}

/// Overrides `camera`'s position/yaw/pitch with the active `trigger_camera`
/// sequence's resolved view, when one is active; a no-op otherwise, leaving
/// whatever this step's ordinary player-move phase already computed. Called
/// after `Simulation::tick` so a sequence that activates or completes this
/// very step is already reflected in the frame this produces.
pub(crate) fn apply_override(level: &Level, camera: &mut FreeFlyCamera) {
    let Some((entity, trigger_camera, state)) = active_camera(&level.registry) else {
        return;
    };
    // The player's own live view, exactly as this step's ordinary
    // player-move phase left it: still correct even under "Freeze Player",
    // since that flag gates movement axes, not mouse look.
    let player_view = (Vec3::from_array(camera.position), camera.yaw, camera.pitch);

    let position = state.path_position().unwrap_or_else(|| {
        if trigger_camera.start_at_player {
            player_view.0
        } else {
            level
                .registry
                .world
                .get::<&Transform>(entity)
                .map_or(Vec3::ZERO, |transform| transform.origin)
        }
    });

    let player_origin = level
        .registry
        .world
        .get::<&Transform>(level.player)
        .map_or(player_view.0, |transform| transform.origin);
    let aim_point = if trigger_camera.follow_player {
        Some(player_view.0)
    } else {
        look_at_target(&level.registry, entity, player_origin)
    };

    let (yaw, pitch) = match aim_point {
        Some(aim) => look_at(position, aim).unwrap_or((camera.yaw, camera.pitch)),
        None => state.path_yaw_degrees().map_or_else(
            || authored_angles(&level.registry, entity),
            |yaw| (yaw, authored_angles(&level.registry, entity).1),
        ),
    };

    if position.is_finite() {
        camera.position = position.to_array();
    }
    if yaw.is_finite() {
        camera.yaw = yaw;
    }
    if pitch.is_finite() {
        camera.pitch = pitch.clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
    }
}

/// `entity`'s own placed `(yaw, pitch)`, in the `FreeFlyCamera` convention
/// (`Transform::angles` is `pitch yaw roll`), or `(0.0, 0.0)` when it has no
/// `Transform` (never true for a map entity, but this is a rendering
/// fallback, not a panic path).
fn authored_angles(registry: &Registry, entity: Entity) -> (f32, f32) {
    registry
        .world
        .get::<&Transform>(entity)
        .map_or((0.0, 0.0), |transform| {
            (transform.angles.y, transform.angles.x)
        })
}

/// The world-space position of `camera_entity`'s completion/look-at
/// `target`, when it names an entity this registry can resolve, else
/// `None`. `player_origin` is unused directly here (the generic `target`
/// keyvalue never resolves to the player, which carries no `targetname`),
/// kept as a parameter so a future public source pinning a documented
/// player-targeting alias has one obvious place to add it.
fn look_at_target(registry: &Registry, camera_entity: Entity, player_origin: Vec3) -> Option<Vec3> {
    let _ = player_origin;
    let name = registry.world.get::<&Target>(camera_entity).ok()?.0.clone();
    let target_entity = *registry.find(&name).first()?;
    registry
        .world
        .get::<&Transform>(target_entity)
        .ok()
        .map(|transform| transform.origin)
}

/// `(yaw, pitch)` degrees, in `ohl_render::FreeFlyCamera`'s convention (yaw
/// counter-clockwise around `+Z` from `+X`; positive pitch looks down —
/// see that struct's own doc comment), so `position` looks directly at
/// `aim`. `None` when the two points coincide (no defined direction),
/// leaving the caller's existing orientation alone rather than snapping to
/// an arbitrary angle.
fn look_at(position: Vec3, aim: Vec3) -> Option<(f32, f32)> {
    let delta = aim - position;
    if !delta.is_finite() || delta.length_squared() < 1e-6 {
        return None;
    }
    let horizontal = (delta.x * delta.x + delta.y * delta.y).sqrt();
    let yaw = delta.y.atan2(delta.x).to_degrees();
    let pitch = (-delta.z).atan2(horizontal).to_degrees();
    Some((yaw, pitch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_straight_ahead_has_zero_pitch() {
        let (yaw, pitch) = look_at(Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0)).expect("finite delta");
        assert!(yaw.abs() < 1e-3);
        assert!(pitch.abs() < 1e-3);
    }

    #[test]
    fn looking_down_at_the_player_gives_a_positive_pitch() {
        let (_, pitch) = look_at(Vec3::new(0.0, 0.0, 100.0), Vec3::ZERO).expect("finite delta");
        assert!(
            pitch > 0.0,
            "positive pitch looks down, per FreeFlyCamera's convention"
        );
    }

    #[test]
    fn looking_up_gives_a_negative_pitch() {
        let (_, pitch) = look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, 100.0)).expect("finite delta");
        assert!(pitch < 0.0);
    }

    #[test]
    fn coincident_points_have_no_defined_direction() {
        assert_eq!(look_at(Vec3::ZERO, Vec3::ZERO), None);
    }

    #[test]
    fn yaw_matches_the_movedir_convention_facing_plus_y() {
        let (yaw, _) = look_at(Vec3::ZERO, Vec3::new(0.0, 100.0, 0.0)).expect("finite delta");
        assert!((yaw - 90.0).abs() < 1e-3);
    }
}
