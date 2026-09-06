//! The first-person view model: a separate [`ohl_render::StudioRenderer`]
//! instance drawn after everything else, into its own cleared depth, so it
//! is never occluded by (and never occludes) world or entity geometry.
//!
//! # What this package can and cannot wire up yet
//!
//! The design this package implements (`docs/MILESTONES.md`, "M7.9 (Rust):
//! engine integration") calls for the sequence to be driven by
//! `ohl_gameplay::ViewModelAction` and the model to be chosen by the
//! player's current `ohl_combat::WeaponId` through the inventory. Neither
//! `ohl-gameplay` (the sound/HUD/viewmodel-action bridge) nor an `Inventory`
//! is wired into this crate yet — that lands with the sibling package that
//! adds `ohl-gameplay` as a dependency (`xtask/src/graph.rs`'s authorized
//! `ohl-engine` row). Rather than add that dependency edge from this
//! package (out of its authorized scope), [`ViewModelAction`] is
//! reproduced here as a small, closed, project-authored enum with the same
//! five variants; it is written to be a drop-in match for
//! `ohl_gameplay::ViewModelAction` so unifying the two later is a type
//! substitution, not a redesign.
//!
//! Likewise, no new asset-loading path is added: this package does not
//! touch `crates/ohl-engine/src/level.rs`, so a view model can only be one
//! of the studio models the level already loaded for its own props
//! (`Level::studio_models`). [`ViewModel::set_model`] is the seam a later
//! package (or a test) uses to point the view model at one of them;
//! resolving a `WeaponId` to a *loaded* model index by path is deferred to
//! whichever package adds the inventory and its asset lookup. The path
//! itself, wherever it ends up read from, must never reach a log line (see
//! `docs/CLEAN_ROOM.md`).

use ohl_render::math::{self, Mat4};
use ohl_render::{FreeFlyCamera, ModelInstance};
use ohl_world::StudioPose;

use crate::level::Level;
use crate::systems::SystemsConfig;

/// Which viewmodel animation should play next. See the module doc: this is
/// this project's own vocabulary, shaped to match
/// `ohl_gameplay::ViewModelAction` exactly so the two can be unified later.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewModelAction {
    /// The weapon was just drawn.
    Draw,
    /// No shot fired or reload in progress; the idle loop.
    Idle,
    /// A shot was just fired.
    Fire,
    /// A reload just started.
    Reload,
    /// The weapon is being holstered.
    Holster,
}

/// Maps an action to a model-local sequence index.
///
/// TODO(black-box): per-model sequence names (`fire1`, `reload`, ...) are QC
/// data this crate does not load; every action plays sequence 0 until a
/// later package supplies the name-to-index lookup
/// `docs/m79-design.md` §4 describes for `ohl_ai::Activity`.
#[allow(dead_code)]
const fn sequence_for(_action: ViewModelAction) -> usize {
    0
}

/// The view model's current state: which loaded model it draws (if any),
/// and where its animation cursor stands.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct ViewModel {
    /// Index into `Level::studio_models`, or `None` when no view model is
    /// configured (the common case until a later package wires up the
    /// inventory).
    model: Option<usize>,
    sequence: usize,
    cycle: f32,
}

impl ViewModel {
    /// An empty view model: nothing drawn, [`crate::Game::viewmodel_visible`]
    /// reports `false`.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Points the view model at one of the level's already-loaded studio
    /// models. `None` hides it.
    ///
    /// Not yet called from production code: nothing in this tree selects a
    /// weapon (that lands with the inventory package). Kept as the seam
    /// that package uses.
    #[allow(dead_code)]
    pub(crate) fn set_model(&mut self, model: Option<usize>) {
        self.model = model;
        self.cycle = 0.0;
    }

    /// Selects the sequence `action` implies, restarting the cursor only
    /// when the sequence actually changes (a repeated action does not
    /// stutter).
    #[allow(dead_code)]
    pub(crate) fn queue_action(&mut self, action: ViewModelAction) {
        let sequence = sequence_for(action);
        if self.sequence != sequence {
            self.sequence = sequence;
            self.cycle = 0.0;
        }
    }

    /// Advances the animation cursor by `dt` seconds. Non-finite `dt`
    /// leaves the cursor where it was.
    pub(crate) fn tick(&mut self, dt: f32) {
        if !dt.is_finite() {
            return;
        }
        let next = self.cycle + dt;
        if next.is_finite() {
            self.cycle = next;
        }
    }

    /// Whether a model is configured to draw.
    pub(crate) fn is_visible(&self) -> bool {
        self.model.is_some()
    }

    /// The model slot this view model draws, when configured.
    pub(crate) fn model(&self) -> Option<usize> {
        self.model
    }

    /// The sequence and cycle a pose should be sampled at.
    pub(crate) fn pose_cursor(&self) -> (usize, f32) {
        (self.sequence, self.cycle)
    }
}

/// The world-space placement of the view model this frame: a basis built
/// from the camera's forward/left/up (so it turns and looks with the
/// player, pitch included, unlike [`ohl_render::placement`]'s yaw-only
/// static-prop placement) plus [`SystemsConfig::view_model_offset`].
///
/// `view_model_offset`'s three components are read as `(forward, left, up)`
/// camera-relative units, matching this project's GoldSrc-derived axis
/// naming (`+Y` is left; see `ohl_render::camera`).
#[must_use]
pub(crate) fn placement(camera: &FreeFlyCamera, offset: [f32; 3]) -> Mat4 {
    let forward = camera.direction();
    let left = math::normalize(math::cross([0.0, 0.0, 1.0], forward));
    let up = math::cross(forward, left);

    let mut origin = camera.position;
    for axis in 0..3 {
        origin[axis] += forward[axis] * offset[0] + left[axis] * offset[1] + up[axis] * offset[2];
    }

    let mut m = math::identity();
    m[0] = forward[0];
    m[1] = forward[1];
    m[2] = forward[2];
    m[4] = left[0];
    m[5] = left[1];
    m[6] = left[2];
    m[8] = up[0];
    m[9] = up[1];
    m[10] = up[2];
    m[12] = origin[0];
    m[13] = origin[1];
    m[14] = origin[2];
    m
}

/// A camera clone with `fov_y_degrees` overridden to
/// [`SystemsConfig::view_model_fov`], the eye position and orientation
/// otherwise unchanged: the view model's own projection, not its own
/// vantage point.
#[must_use]
pub(crate) fn view_camera(camera: &FreeFlyCamera, config: SystemsConfig) -> FreeFlyCamera {
    FreeFlyCamera {
        fov_y_degrees: config.view_model_fov,
        ..*camera
    }
}

/// Builds the [`ModelInstance`] for one frame's view model draw, from an
/// already-sampled `pose` and `transform` (see [`placement`]).
#[must_use]
pub(crate) fn instance(transform: Mat4, pose: &StudioPose, ambient: [f32; 3]) -> ModelInstance<'_> {
    ModelInstance {
        transform,
        pose,
        body: &[],
        skin: 0,
        ambient,
        light_direction: ModelInstance::default_light_direction(),
        light_color: [0.9, 0.9, 0.9],
    }
}

/// Everything [`crate::render::Renderers`] needs to draw one frame's view
/// model, once [`build_frame`] has confirmed there is one.
pub(crate) struct ViewModelFrame {
    /// Index into `Level::studio_models`, and into the render side's
    /// index-aligned `StudioRenderer` list.
    pub model_slot: usize,
    pub pose: StudioPose,
    pub transform: Mat4,
    /// The camera to draw the view model's own pass with (same eye and
    /// orientation as the world camera, [`SystemsConfig::view_model_fov`]
    /// substituted for its field of view).
    pub camera: FreeFlyCamera,
}

/// Builds this frame's [`ViewModelFrame`], or `None` when the view model is
/// not configured (no model set), the configured slot has since gone out of
/// range, or sampling its pose fails.
#[must_use]
pub(crate) fn build_frame(
    level: &Level,
    camera: &FreeFlyCamera,
    config: SystemsConfig,
    state: &ViewModel,
) -> Option<ViewModelFrame> {
    let slot = state.model()?;
    let model = level.studio_models.get(slot)?;
    let (sequence, cycle) = state.pose_cursor();
    let pose = StudioPose::sample(model, sequence, cycle).ok()?;
    Some(ViewModelFrame {
        model_slot: slot,
        transform: placement(camera, config.view_model_offset),
        pose,
        camera: view_camera(camera, config),
    })
}

#[cfg(test)]
mod tests {
    use ohl_render::FreeFlyCamera;

    use super::{ViewModel, ViewModelAction, placement, view_camera};
    use crate::systems::SystemsConfig;

    #[test]
    fn an_unconfigured_view_model_is_not_visible() {
        let view_model = ViewModel::new();
        assert!(!view_model.is_visible());
        assert_eq!(view_model.model(), None);
    }

    #[test]
    fn setting_a_model_makes_it_visible_and_resets_the_cycle() {
        let mut view_model = ViewModel::new();
        view_model.tick(1.0);
        view_model.set_model(Some(2));
        assert!(view_model.is_visible());
        assert_eq!(view_model.model(), Some(2));
        assert_eq!(view_model.pose_cursor(), (0, 0.0));
    }

    #[test]
    fn queuing_the_same_action_twice_does_not_restart_the_cycle() {
        let mut view_model = ViewModel::new();
        view_model.set_model(Some(0));
        view_model.queue_action(ViewModelAction::Fire);
        view_model.tick(0.5);
        view_model.queue_action(ViewModelAction::Fire);
        assert!((view_model.pose_cursor().1 - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn placement_at_identity_camera_sits_in_front_of_the_eye() {
        let camera = FreeFlyCamera::default();
        let m = placement(&camera, [10.0, 0.0, 0.0]);
        // Forward at yaw 0 is +X, so a forward-only offset moves +X.
        assert!((m[12] - (camera.position[0] + 10.0)).abs() < 1e-4);
        assert!((m[13] - camera.position[1]).abs() < 1e-4);
        assert!((m[14] - camera.position[2]).abs() < 1e-4);
    }

    #[test]
    fn the_view_camera_overrides_only_the_field_of_view() {
        let camera = FreeFlyCamera {
            yaw: 45.0,
            pitch: 10.0,
            ..FreeFlyCamera::default()
        };
        let config = SystemsConfig {
            view_model_fov: 90.0,
            ..SystemsConfig::default()
        };
        let view = view_camera(&camera, config);
        assert!((view.fov_y_degrees - 90.0).abs() < f32::EPSILON);
        assert!((view.yaw - camera.yaw).abs() < f32::EPSILON);
        assert!((view.pitch - camera.pitch).abs() < f32::EPSILON);
        for axis in 0..3 {
            assert!((view.position[axis] - camera.position[axis]).abs() < f32::EPSILON);
        }
    }
}
