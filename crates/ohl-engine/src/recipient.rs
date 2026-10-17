//! Owned selected-player presentation, independent of transport and simulation.
//!
//! A host explicitly selects a connected human by full [`PlayerId`]. Only that
//! player's numeric HUD accompanies the public roster and owned scene. The
//! replica binds a catalog, epoch and recipient; no implicit reconnection or
//! resource remapping occurs. Drop it on disconnect or a binding change.
//!
//! First-person poses come from the selected actor's own fixed-step playback.
//! This project-defined slice has no audio, transient effects, runtime
//! light-style changes, prediction or wire mapping.
//! Drawing uses the supplied camera/time, project-default lighting/projection,
//! and owned sampled poses. It never ticks authority or consumes input/events.

use std::fmt;

use ohl_combat::{WeaponId, spec};
use ohl_render::{FreeFlyCamera, LightStyles};
use ohl_ui::{hud::HudState, scoreboard::ScoreboardState};
use ohl_world::StudioPose;

use crate::deathmatch::{MAX_PLAYERS, PlayerId};
use crate::scene::{
    SceneCatalog, SceneEpoch, SceneError, SceneFrame, SceneIdentity, SceneLimits, SceneReplica,
    StudioHandle,
};

/// Project player-label bound, matching deathmatch admission (UTF-8 bytes).
pub const MAX_LABEL_BYTES: usize = 32;

#[derive(Clone, Copy, PartialEq)]
pub struct RecipientCamera {
    pub eye: [f32; 3],
    pub pitch: f32,
    /// Canonical project degrees in `[0, 360)`.
    pub yaw: f32,
}

impl RecipientCamera {
    pub(crate) fn camera(self, fov: f32, aspect: f32) -> Result<FreeFlyCamera, RecipientError> {
        if self.eye.iter().any(|value| !value.is_finite())
            || !self.pitch.is_finite()
            || self.pitch.abs() > ohl_physics::controller::MAX_PITCH_DEGREES
            || !self.yaw.is_finite()
            || !(0.0..360.0).contains(&self.yaw)
            || !fov.is_finite()
            || !crate::FOV_RANGE.contains(&fov)
            || !aspect.is_finite()
            || aspect <= 0.0
        {
            return Err(RecipientError::InvalidCamera);
        }
        // Near/far are the existing project defaults, never the spectator's
        // mutable camera. No controller or stored camera is changed.
        let camera = FreeFlyCamera {
            position: self.eye,
            pitch: self.pitch,
            yaw: self.yaw,
            fov_y_degrees: fov,
            ..FreeFlyCamera::default()
        };
        if camera
            .view()
            .iter()
            .chain(camera.view_projection(aspect).iter())
            .any(|v| !v.is_finite())
        {
            return Err(RecipientError::InvalidCamera);
        }
        Ok(camera)
    }
}

#[derive(Clone, PartialEq)]
pub struct RecipientHud {
    pub health: i32,
    pub armor: i32,
    pub weapon: Option<WeaponId>,
    pub clip_ammo: Option<i32>,
    pub reserve_ammo: Option<i32>,
}

/// One selected human's already sampled first-person pose and placement.
/// Its resource belongs to the scene catalog; it has no world entity ID.
#[derive(Clone, PartialEq)]
pub struct RecipientViewModel {
    pub model: StudioHandle,
    pub pose: StudioPose,
    pub transform: [f32; 16],
}

impl RecipientViewModel {
    pub(crate) fn draw(&self) -> crate::viewmodel::ViewModelDraw<'_> {
        crate::viewmodel::ViewModelDraw {
            model_slot: self.model.local_index(),
            pose: &self.pose,
            transform: self.transform,
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct PublicPlayerRow {
    pub id: PlayerId,
    pub name: String,
    pub frags: i32,
    pub deaths: u32,
    pub alive: bool,
}

#[derive(Clone, PartialEq)]
pub struct RecipientScoreboard {
    /// Existing tally order: frags descending, deaths ascending, then slot.
    pub rows: Vec<PublicPlayerRow>,
    pub frag_limit: Option<u32>,
    pub seconds_left: Option<f32>,
    /// Public display label only; it is never used to resolve a player ID.
    pub winner_label: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct RecipientState {
    pub recipient: PlayerId,
    /// Actual generational actor identity, not an interpretation of player slot.
    pub body: SceneIdentity,
    pub camera: RecipientCamera,
    pub hud: RecipientHud,
    /// Missing optional assets or a dead actor show no first-person geometry.
    pub view_model: Option<RecipientViewModel>,
    pub scoreboard: RecipientScoreboard,
}

/// Owned candidate. Only replica apply admits it for rendering/UI projection.
#[derive(Clone, PartialEq)]
pub struct RecipientFrame {
    scene: SceneFrame,
    state: RecipientState,
}

impl RecipientFrame {
    pub fn new(scene: SceneFrame, state: RecipientState) -> Self {
        Self { scene, state }
    }

    pub fn scene(&self) -> &SceneFrame {
        &self.scene
    }

    pub fn state(&self) -> &RecipientState {
        &self.state
    }
}

/// An immutable view into a fully owned accepted replica, with no Game borrow.
#[derive(Clone, Copy, PartialEq)]
pub struct RecipientFrameRef<'a> {
    scene: &'a SceneFrame,
    state: &'a RecipientState,
}

impl<'a> RecipientFrameRef<'a> {
    pub const fn scene(self) -> &'a SceneFrame {
        self.scene
    }

    pub const fn state(self) -> &'a RecipientState {
        self.state
    }
}

pub struct RecipientReplica {
    epoch: SceneEpoch,
    recipient: PlayerId,
    scene: SceneReplica,
    state: Option<RecipientState>,
    default_light_styles: LightStyles,
}

impl RecipientReplica {
    /// Bind one resource revision/host epoch and complete admitted player ID.
    ///
    /// # Errors
    /// Invalid player identity or limits above the existing scene hard bounds.
    pub fn new(
        catalog: SceneCatalog,
        epoch: SceneEpoch,
        recipient: PlayerId,
        limits: SceneLimits,
    ) -> Result<Self, RecipientError> {
        if !valid_player(recipient) {
            return Err(RecipientError::InvalidIdentity);
        }
        Ok(Self {
            epoch,
            recipient,
            scene: SceneReplica::with_limits(catalog, epoch, limits)?,
            state: None,
            default_light_styles: LightStyles::new(),
        })
    }

    /// Full replacement. Every rejection preserves both scene and private state.
    ///
    /// # Errors
    /// Invalid/mismatched recipient data, scene binding, stamp, pose or budget.
    pub fn apply(&mut self, frame: RecipientFrame) -> Result<(), RecipientError> {
        validate_state(&frame.state, self.epoch, self.recipient)?;
        // This is the final fallible operation. SceneReplica itself commits
        // only after all its validations; state replacement cannot fail.
        self.scene.apply_with_extra_studio(
            frame.scene,
            frame
                .state
                .view_model
                .as_ref()
                .map(|view_model| (&view_model.model, &view_model.pose)),
        )?;
        self.state = Some(frame.state);
        Ok(())
    }

    pub fn frame(&self) -> Option<RecipientFrameRef<'_>> {
        Some(RecipientFrameRef {
            scene: self.scene.frame()?,
            state: self.state.as_ref()?,
        })
    }

    pub(crate) fn light_styles(&self) -> &LightStyles {
        &self.default_light_styles
    }
}

/// Fixed errors contain no labels, private values, resource names or addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecipientError {
    Scene(SceneError),
    WrongRecipient,
    InvalidIdentity,
    InvalidCamera,
    InvalidHud,
    InvalidViewModel,
    InvalidRoster,
    InvalidLabel,
    NoFrame,
    Renderer,
}

impl From<SceneError> for RecipientError {
    fn from(error: SceneError) -> Self {
        Self::Scene(error)
    }
}

impl fmt::Display for RecipientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Scene(error) => return error.fmt(f),
            Self::WrongRecipient => "mismatched recipient identity",
            Self::InvalidIdentity => "invalid recipient identity",
            Self::InvalidCamera => "invalid recipient camera",
            Self::InvalidHud => "invalid recipient HUD",
            Self::InvalidViewModel => "invalid recipient viewmodel",
            Self::InvalidRoster => "invalid recipient roster",
            Self::InvalidLabel => "invalid recipient label",
            Self::NoFrame => "no accepted recipient frame",
            Self::Renderer => "recipient renderer unavailable",
        })
    }
}

impl std::error::Error for RecipientError {}

macro_rules! redacted_debug {
    ($($ty:ty),+ $(,)?) => { $(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($ty)).finish_non_exhaustive()
            }
        }
    )+ };
}

redacted_debug!(
    RecipientCamera,
    RecipientHud,
    RecipientViewModel,
    PublicPlayerRow,
    RecipientScoreboard,
    RecipientState,
    RecipientFrame,
    RecipientFrameRef<'_>,
    RecipientReplica
);

fn valid_player(id: PlayerId) -> bool {
    id.session.0 != 0 && id.slot < MAX_PLAYERS && id.generation != 0
}

fn valid_label(name: &str) -> bool {
    name.len() <= MAX_LABEL_BYTES
        && !name.trim().is_empty()
        && !name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '"'))
}

fn validate_state(
    state: &RecipientState,
    epoch: SceneEpoch,
    recipient: PlayerId,
) -> Result<(), RecipientError> {
    if state.recipient != recipient {
        return Err(RecipientError::WrongRecipient);
    }
    if state.body.epoch() != epoch || crate::entity_of(state.body.local_id()).is_none() {
        return Err(RecipientError::InvalidIdentity);
    }
    state.camera.camera(75.0, 1.0)?;
    let hud = &state.hud;
    let weapon = hud.weapon.map(spec);
    if hud.armor < 0
        || hud.clip_ammo.is_some_and(|value| value < 0)
        || hud.reserve_ammo.is_some_and(|value| value < 0)
        || hud.clip_ammo.is_some() != weapon.is_some_and(|weapon| weapon.clip_size.is_some())
        || hud.reserve_ammo.is_some() != weapon.is_some_and(|weapon| weapon.ammo.is_some())
    {
        return Err(RecipientError::InvalidHud);
    }
    let board = &state.scoreboard;
    if board.rows.len() > usize::from(MAX_PLAYERS)
        || board
            .seconds_left
            .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        return Err(RecipientError::InvalidRoster);
    }
    if board
        .winner_label
        .as_deref()
        .is_some_and(|name| !valid_label(name))
    {
        return Err(RecipientError::InvalidLabel);
    }
    let mut slots = [false; MAX_PLAYERS as usize];
    let mut selected = false;
    let mut selected_alive = false;
    for row in &board.rows {
        if !valid_player(row.id) || row.id.session != recipient.session {
            return Err(RecipientError::InvalidIdentity);
        }
        if slots[usize::from(row.id.slot)] {
            return Err(RecipientError::InvalidRoster);
        }
        slots[usize::from(row.id.slot)] = true;
        selected |= row.id == recipient;
        selected_alive |= row.id == recipient && row.alive;
        if !valid_label(&row.name) {
            return Err(RecipientError::InvalidLabel);
        }
    }
    if !selected
        || board.rows.windows(2).any(|pair| {
            let (a, b) = (&pair[0], &pair[1]);
            b.frags
                .cmp(&a.frags)
                .then(a.deaths.cmp(&b.deaths))
                .then(a.id.slot.cmp(&b.id.slot))
                .is_gt()
        })
    {
        return Err(RecipientError::InvalidRoster);
    }
    if let Some(view_model) = &state.view_model
        && (!selected_alive
            || hud.weapon.is_none()
            || view_model.transform.iter().any(|value| !value.is_finite()))
    {
        return Err(RecipientError::InvalidViewModel);
    }
    Ok(())
}

/// Pure numeric HUD projection; transient text, flashes and history stay empty.
pub fn hud(frame: RecipientFrameRef<'_>, crosshair: bool) -> HudState {
    let state = frame.state;
    let selected = state
        .scoreboard
        .rows
        .iter()
        .find(|row| row.id == state.recipient)
        .expect("accepted recipient retains its public row");
    HudState {
        health: state.hud.health,
        armor: state.hud.armor,
        clip_ammo: state.hud.clip_ammo,
        reserve_ammo: state.hud.reserve_ammo,
        show_crosshair: crosshair && selected.alive && state.hud.weapon.is_some(),
        frags: Some(selected.frags),
        match_clock: state.scoreboard.seconds_left,
        ..HudState::default()
    }
}

/// Existing scoreboard UI with recipient highlight based solely on full ID.
pub fn scoreboard(frame: RecipientFrameRef<'_>) -> ScoreboardState {
    let state = frame.state;
    ScoreboardState {
        rows: state
            .scoreboard
            .rows
            .iter()
            .map(|row| ohl_ui::scoreboard::ScoreRow {
                name: row.name.clone(),
                frags: row.frags,
                deaths: row.deaths,
                alive: row.alive,
                is_local: row.id == state.recipient,
            })
            .collect(),
        frag_limit: state.scoreboard.frag_limit,
        seconds_left: state.scoreboard.seconds_left,
        winner: state.scoreboard.winner_label.clone(),
    }
}
