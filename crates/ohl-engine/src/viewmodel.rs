//! The first-person view model: a separate [`ohl_render::StudioRenderer`]
//! instance drawn after everything else, into its own cleared depth, so it
//! is never occluded by (and never occludes) world or entity geometry.
//!
//! # Models and animation
//!
//! [`Level::preload_view_models`] loads each weapon's optional `v_` model
//! ([`view_model_path`]; provenance in `docs/FORMAT_SOURCES.md`,
//! "First-person view models") into `Level::studio_models`, and
//! [`ViewModel::configure`] binds whichever of them the payload published.
//! [`ViewModel::sync`] follows the player's selected weapon once per step,
//! and the `ohl_gameplay::ViewModelAction`s the gameplay bridge queues pick
//! the sequence. Which sequence plays an action is resolved against the
//! model's own sequence labels by project-authored intent words
//! ([`intent_words`]); labels are media-derived data and never logged. A
//! weapon with several idles rotates through those that start in the same
//! pose ([`idle_pool`]). A weapon whose model is missing simply shows
//! nothing. The path, wherever it is read from, must never reach a log line
//! (see `docs/CLEAN_ROOM.md`).
//!
//! The model is drawn with the world camera's own field of view: the
//! original game has no separate weapon FOV, and its weapons scale with
//! the FOV setting (`docs/FORMAT_SOURCES.md`, "First-person view models").

use ohl_combat::WeaponId;
use ohl_gameplay::ViewModelAction;
use ohl_render::math::{self, Mat4};
use ohl_render::{FreeFlyCamera, ModelInstance};
use ohl_world::{StudioModel, StudioPose};

use crate::level::Level;
use crate::systems::SystemsConfig;

/// A weapon's optional first-person model. Missing assets stay optional.
pub(crate) const fn view_model_path(weapon: WeaponId) -> &'static str {
    match weapon {
        WeaponId::Crowbar => "models/v_crowbar.mdl",
        WeaponId::Glock => "models/v_9mmhandgun.mdl",
        WeaponId::Python => "models/v_357.mdl",
        WeaponId::Mp5 => "models/v_9mmar.mdl",
        WeaponId::Shotgun => "models/v_shotgun.mdl",
        WeaponId::Crossbow => "models/v_crossbow.mdl",
        WeaponId::Rpg => "models/v_rpg.mdl",
        WeaponId::Gauss => "models/v_gauss.mdl",
        WeaponId::Egon => "models/v_egon.mdl",
        WeaponId::HornetGun => "models/v_hgun.mdl",
        WeaponId::HandGrenade => "models/v_grenade.mdl",
        WeaponId::Satchel => "models/v_satchel.mdl",
        WeaponId::Tripmine => "models/v_tripmine.mdl",
        WeaponId::Snark => "models/v_squeak.mdl",
    }
}

/// Every action, in the order [`WeaponViewModel::sequences`] stores them.
const ACTIONS: [ViewModelAction; 5] = [
    ViewModelAction::Idle,
    ViewModelAction::Draw,
    ViewModelAction::Fire,
    ViewModelAction::Reload,
    ViewModelAction::Holster,
];

const fn action_slot(action: ViewModelAction) -> usize {
    match action {
        ViewModelAction::Idle => 0,
        ViewModelAction::Draw => 1,
        ViewModelAction::Fire => 2,
        ViewModelAction::Reload => 3,
        ViewModelAction::Holster => 4,
    }
}

/// Project-authored words a sequence label's `_`-separated token must
/// start with to play `action`. The draw words follow the weapon-template
/// vocabulary cited in `docs/FORMAT_SOURCES.md`; no label is baked in.
const fn intent_words(action: ViewModelAction) -> &'static [&'static str] {
    match action {
        ViewModelAction::Idle => &["idle"],
        ViewModelAction::Draw => &["draw", "deploy"],
        ViewModelAction::Fire => &["shoot", "fire", "attack", "throw"],
        ViewModelAction::Reload => &["reload"],
        ViewModelAction::Holster => &["holster"],
    }
}

/// How far (model units, on any bone) a later idle's first frame may sit
/// from the first idle's and still join the rotation. Project-authored:
/// alternative idles start where the first one does, while a variant for
/// another state (an empty clip, say) starts tens of units away.
const IDLE_POSE_TOLERANCE: f32 = 2.0;

/// One resolved sequence: its index and how it plays.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActionSequence {
    index: usize,
    duration: f32,
    looping: bool,
}

impl ActionSequence {
    fn of(model: &StudioModel, index: usize) -> Option<Self> {
        let sequence = model.sequences.get(index)?;
        Some(Self {
            index,
            duration: sequence.duration(),
            looping: sequence.is_looping(),
        })
    }
}

/// Every one of `model`'s sequences whose label carries one of `action`'s
/// intent words, in file order.
fn matching(model: &StudioModel, action: ViewModelAction) -> impl Iterator<Item = usize> + '_ {
    let words = intent_words(action);
    model
        .sequence_names
        .iter()
        .enumerate()
        .filter(move |(_, label)| {
            label
                .split('_')
                .any(|token| words.iter().any(|word| token.starts_with(word)))
        })
        .map(|(index, _)| index)
}

/// The first of `model`'s sequences matching `action`.
fn resolve(model: &StudioModel, action: ViewModelAction) -> Option<ActionSequence> {
    matching(model, action)
        .next()
        .and_then(|index| ActionSequence::of(model, index))
}

/// Whether two poses put every bone within [`IDLE_POSE_TOLERANCE`] of each
/// other.
fn starts_alike(a: &StudioPose, b: &StudioPose) -> bool {
    a.matrices.len() == b.matrices.len()
        && a.matrices.iter().zip(&b.matrices).all(|(a, b)| {
            let distance = (0..3)
                .map(|axis| (a[12 + axis] - b[12 + axis]).powi(2))
                .sum::<f32>()
                .sqrt();
            distance <= IDLE_POSE_TOLERANCE
        })
}

/// The idles a weapon rotates through: the first idle-labelled sequence,
/// then every later one whose first frame [`starts_alike`] it.
fn idle_pool(model: &StudioModel) -> Vec<ActionSequence> {
    let mut indices = matching(model, ViewModelAction::Idle);
    let Some(first) = indices.next() else {
        return Vec::new();
    };
    let base = StudioPose::sample(model, first, 0.0).ok();
    std::iter::once(first)
        .chain(indices.filter(|&index| {
            base.as_ref().is_some_and(|base| {
                StudioPose::sample(model, index, 0.0).is_ok_and(|pose| starts_alike(base, &pose))
            })
        }))
        .filter_map(|index| ActionSequence::of(model, index))
        .collect()
}

/// A presentation-only pseudo-random number for the `count`th pick (the
/// public SplitMix64 finaliser), so choosing an idle never draws from the
/// simulation's own random stream.
fn mix(count: u64) -> u64 {
    let mut z = count.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A loaded weapon model and the sequence each action resolved to.
#[derive(Debug, Clone, PartialEq)]
struct WeaponViewModel {
    weapon: WeaponId,
    model: usize,
    sequences: [Option<ActionSequence>; 5],
    /// The idle rotation ([`idle_pool`]); its first entry is
    /// `sequences[Idle]`.
    idles: Vec<ActionSequence>,
}

impl WeaponViewModel {
    /// The sequence `action` plays: its own, else the idle (an unresolved
    /// holster keeps whatever is showing).
    fn sequence(&self, action: ViewModelAction) -> Option<ActionSequence> {
        let own = self.sequences[action_slot(action)];
        match action {
            ViewModelAction::Holster => own,
            _ => own.or(self.sequences[action_slot(ViewModelAction::Idle)]),
        }
    }
}

/// The view model's current state: which loaded model it draws (if any),
/// and where its animation cursor stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ViewModel {
    /// This level's loaded weapon models ([`Self::configure`]).
    weapons: Vec<WeaponViewModel>,
    /// The selection [`Self::sync`] last saw.
    weapon: Option<WeaponId>,
    /// Index into [`Self::weapons`] for the model showing, when it is one.
    entry: Option<usize>,
    /// Index into `Level::studio_models`, or `None` when no view model is
    /// configured.
    model: Option<usize>,
    /// Set while the player cannot see their own weapon (dead, or another
    /// camera owns the view).
    hidden: bool,
    action: ViewModelAction,
    playing: Option<ActionSequence>,
    sequence: usize,
    cycle: f32,
    /// How many idles have been picked, the input to [`mix`].
    idle_picks: u64,
}

impl Default for ViewModel {
    fn default() -> Self {
        Self {
            weapons: Vec::new(),
            weapon: None,
            entry: None,
            model: None,
            hidden: false,
            action: ViewModelAction::Idle,
            playing: None,
            sequence: 0,
            cycle: 0.0,
            idle_picks: 0,
        }
    }
}

impl ViewModel {
    /// An empty view model: nothing drawn, [`crate::Game::viewmodel_visible`]
    /// reports `false`.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Binds every weapon model `level` has loaded under its
    /// [`view_model_path`] and resolves each one's action sequences. Forgets
    /// the previous selection, so the next [`Self::sync`] draws afresh.
    pub(crate) fn configure(&mut self, level: &Level) {
        self.weapons = WeaponId::ALL
            .into_iter()
            .filter_map(|weapon| {
                let model = crate::projectiles::find_model_path(level, view_model_path(weapon))?;
                let studio = level.studio_models.get(model)?;
                Some(WeaponViewModel {
                    weapon,
                    model,
                    sequences: ACTIONS.map(|action| resolve(studio, action)),
                    idles: idle_pool(studio),
                })
            })
            .collect();
        self.weapon = None;
        self.entry = None;
        self.model = None;
        self.playing = None;
        self.sequence = 0;
        self.cycle = 0.0;
    }

    /// Follows the player's selected weapon: a new selection swaps the model
    /// and plays its draw. `shown` is whether the player can see their
    /// weapon at all this step.
    pub(crate) fn sync(&mut self, weapon: Option<WeaponId>, shown: bool) {
        self.hidden = !shown;
        if weapon == self.weapon {
            return;
        }
        self.weapon = weapon;
        self.entry =
            weapon.and_then(|weapon| self.weapons.iter().position(|entry| entry.weapon == weapon));
        self.model = self.entry.map(|entry| self.weapons[entry].model);
        self.playing = None;
        self.sequence = 0;
        self.cycle = 0.0;
        self.queue_action(ViewModelAction::Draw);
    }

    /// Points the view model at one of the level's already-loaded studio
    /// models, outside any weapon. `None` hides it. Only the test hook
    /// [`crate::Game::debug_show_viewmodel_and_sprite`] calls it.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_model(&mut self, model: Option<usize>) {
        self.entry = None;
        self.model = model;
        self.playing = None;
        self.sequence = 0;
        self.cycle = 0.0;
    }

    /// Plays the sequence `action` resolves to. A one-shot sequence
    /// restarts every time (each shot replays the fire); a looping one only
    /// restarts when the sequence changes, so a repeated action does not
    /// stutter. An idle asked for while one is already playing lets that
    /// one finish. Called from `crate::presentation::Presentation::tick`
    /// (phase 13) for every viewmodel action the gameplay bridge queues.
    pub(crate) fn queue_action(&mut self, action: ViewModelAction) {
        if action == ViewModelAction::Idle
            && self.action == ViewModelAction::Idle
            && self.playing.is_some()
        {
            return;
        }
        let next = if action == ViewModelAction::Idle {
            self.pick_idle()
        } else {
            self.entry
                .and_then(|entry| self.weapons.get(entry))
                .and_then(|entry| entry.sequence(action))
        };
        let Some(next) = next else {
            return;
        };
        if self.sequence != next.index || !next.looping {
            self.sequence = next.index;
            self.cycle = 0.0;
        }
        self.action = action;
        self.playing = Some(next);
    }

    /// One of the showing weapon's idles, chosen with equal chances.
    fn pick_idle(&mut self) -> Option<ActionSequence> {
        let entry = self.entry.and_then(|entry| self.weapons.get(entry))?;
        let count = entry.idles.len();
        if count < 2 {
            return entry.sequence(ViewModelAction::Idle);
        }
        let pick = mix(self.idle_picks);
        self.idle_picks = self.idle_picks.wrapping_add(1);
        #[allow(clippy::cast_possible_truncation)]
        entry.idles.get((pick % count as u64) as usize).copied()
    }

    /// Advances the animation cursor by `dt` seconds. A finished idle (a
    /// looping one after one pass) makes way for another from the rotation,
    /// a finished holster holds its last frame, and anything else returns
    /// to an idle. Non-finite `dt` leaves the cursor where it was.
    pub(crate) fn tick(&mut self, dt: f32) {
        if !dt.is_finite() {
            return;
        }
        let next = self.cycle + dt;
        if next.is_finite() {
            self.cycle = next;
        }
        let Some(playing) = self.playing else {
            return;
        };
        if playing.duration <= 0.0
            || self.cycle < playing.duration
            || (playing.looping && self.action != ViewModelAction::Idle)
        {
            return;
        }
        match self.action {
            ViewModelAction::Holster => {}
            ViewModelAction::Idle => {
                if let Some(next) = self.pick_idle() {
                    self.sequence = next.index;
                    self.playing = Some(next);
                }
                self.cycle = 0.0;
            }
            ViewModelAction::Draw | ViewModelAction::Fire | ViewModelAction::Reload => {
                self.queue_action(ViewModelAction::Idle);
            }
        }
    }

    /// Whether a model is configured and shown this frame.
    pub(crate) fn is_visible(&self) -> bool {
        self.model.is_some() && !self.hidden
    }

    /// The model slot this view model draws, when shown.
    pub(crate) fn model(&self) -> Option<usize> {
        self.model.filter(|_| !self.hidden)
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
    })
}

#[cfg(test)]
mod tests {
    use ohl_combat::WeaponId;
    use ohl_gameplay::ViewModelAction;
    use ohl_render::FreeFlyCamera;
    use ohl_world::{StudioLimits, StudioModel, StudioPose};

    use super::{ACTIONS, ViewModel, WeaponViewModel, idle_pool, placement, resolve, starts_alike};

    /// Project-authored labels: intent words with a neutral prefix, not
    /// copied from any model. Every synthetic sequence is a 0.2 s one-shot.
    const LABELS: [&str; 5] = [
        "ohl_idle",
        "ohl_deploy",
        "ohl_shoot",
        "ohl_reload",
        "ohl_holster",
    ];

    fn studio(labels: &[&str]) -> StudioModel {
        let (bytes, _) = ohl_formats::test_support::build_minimal_mdl10_with_sequences(labels);
        StudioModel::parse(&bytes, &StudioLimits::default()).unwrap()
    }

    /// A view model with the pistol (model slot 4) and the crowbar (slot 7)
    /// loaded from `labels`.
    fn armed(labels: &[&str]) -> ViewModel {
        let model = studio(labels);
        let mut view_model = ViewModel::new();
        view_model.weapons = [(WeaponId::Glock, 4), (WeaponId::Crowbar, 7)]
            .into_iter()
            .map(|(weapon, slot)| WeaponViewModel {
                weapon,
                model: slot,
                sequences: ACTIONS.map(|action| resolve(&model, action)),
                idles: idle_pool(&model),
            })
            .collect();
        view_model
    }

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
    fn each_action_resolves_to_the_sequence_naming_its_intent() {
        let model = studio(&LABELS);
        let resolved = ACTIONS.map(|action| resolve(&model, action).map(|found| found.index));
        assert_eq!(resolved, [Some(0), Some(1), Some(2), Some(3), Some(4)]);
        let only_idle = studio(&["ohl_idle"]);
        assert_eq!(resolve(&only_idle, ViewModelAction::Fire), None);
        let one_shot = resolve(&model, ViewModelAction::Fire).unwrap();
        assert!(!one_shot.looping);
        assert!((one_shot.duration - 0.2).abs() < 1e-6);
    }

    #[test]
    fn selecting_a_weapon_shows_its_model_and_plays_the_draw() {
        let mut view_model = armed(&LABELS);
        view_model.sync(None, true);
        assert!(!view_model.is_visible(), "nothing selected, nothing drawn");
        view_model.sync(Some(WeaponId::Glock), true);
        assert_eq!(view_model.model(), Some(4));
        assert_eq!(view_model.pose_cursor(), (1, 0.0));
        view_model.tick(0.05);
        view_model.sync(Some(WeaponId::Crowbar), true);
        assert_eq!(view_model.model(), Some(7));
        assert_eq!(view_model.pose_cursor(), (1, 0.0), "a switch draws afresh");
        view_model.sync(Some(WeaponId::Python), true);
        assert!(
            !view_model.is_visible(),
            "an unpublished model shows nothing"
        );
    }

    #[test]
    fn a_dead_player_or_another_camera_hides_the_weapon() {
        let mut view_model = armed(&LABELS);
        view_model.sync(Some(WeaponId::Glock), true);
        view_model.sync(Some(WeaponId::Glock), false);
        assert!(!view_model.is_visible());
        assert_eq!(view_model.model(), None);
        view_model.sync(Some(WeaponId::Glock), true);
        assert_eq!(view_model.model(), Some(4));
    }

    #[test]
    fn every_shot_restarts_the_fire_and_a_finished_one_returns_to_idle() {
        let mut view_model = armed(&LABELS);
        view_model.sync(Some(WeaponId::Glock), true);
        view_model.queue_action(ViewModelAction::Fire);
        view_model.tick(0.1);
        view_model.queue_action(ViewModelAction::Fire);
        assert_eq!(view_model.pose_cursor(), (2, 0.0));
        view_model.tick(0.15);
        assert_eq!(view_model.pose_cursor().0, 2, "still mid-shot");
        view_model.tick(0.1);
        assert_eq!(view_model.pose_cursor(), (0, 0.0), "back to the idle");
        view_model.tick(0.25);
        assert_eq!(
            view_model.pose_cursor(),
            (0, 0.0),
            "a one-shot idle replays"
        );
    }

    #[test]
    fn a_missing_sequence_falls_back_to_idle_and_holster_holds() {
        let mut view_model = armed(&["ohl_idle", "ohl_holster"]);
        view_model.sync(Some(WeaponId::Glock), true);
        assert_eq!(view_model.pose_cursor().0, 0, "no draw: the idle");
        view_model.queue_action(ViewModelAction::Holster);
        view_model.tick(1.0);
        assert_eq!(view_model.pose_cursor().0, 1, "holstered stays holstered");
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
    fn idles_that_start_alike_rotate_and_a_displaced_one_stays_out() {
        // Every synthetic sequence shares one animation, so both idles
        // start in the same pose and join the rotation.
        let model = studio(&["ohl_idle", "ohl_deploy", "ohl_idle2"]);
        let pool: Vec<usize> = idle_pool(&model).iter().map(|idle| idle.index).collect();
        assert_eq!(pool, [0, 2]);

        let at = |x: f32| {
            let mut matrix = [0.0; 16];
            matrix[12] = x;
            StudioPose {
                matrices: vec![[0.0; 16], matrix],
            }
        };
        assert!(starts_alike(&at(0.0), &at(1.5)));
        assert!(!starts_alike(&at(0.0), &at(40.0)), "another state's idle");
    }

    #[test]
    fn a_finished_idle_hands_over_to_others_in_the_rotation() {
        let mut view_model = armed(&["ohl_idle", "ohl_deploy", "ohl_idle2"]);
        view_model.sync(Some(WeaponId::Glock), true);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            view_model.tick(0.25);
            let (sequence, _) = view_model.pose_cursor();
            assert_ne!(sequence, 1, "the draw is long over");
            seen.insert(sequence);
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [0, 2]);
    }

    #[test]
    fn an_idle_asked_for_mid_idle_lets_it_finish() {
        let mut view_model = armed(&LABELS);
        view_model.sync(Some(WeaponId::Glock), true);
        view_model.tick(0.25);
        assert_eq!(
            view_model.pose_cursor().0,
            0,
            "the draw gave way to the idle"
        );
        view_model.tick(0.1);
        view_model.queue_action(ViewModelAction::Idle);
        assert!((view_model.pose_cursor().1 - 0.1).abs() < 1e-6);
    }
}
