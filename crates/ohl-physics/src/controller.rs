//! The fixed-timestep player controller a host application drives.
//!
//! [`PlayerController`] owns a [`PlayerState`] and a [`MoveConfig`], turns
//! per-frame key states and view angles into a movement wish, and runs
//! [`player_move`] on a fixed tick so movement does not depend on frame
//! rate. The host only has to feed it [`ControllerInput`] and the elapsed
//! frame time, then read [`PlayerController::eye_position`] for its camera.

use glam::Vec3;

use crate::hull::CollisionModel;
use crate::movement::{MoveConfig, MoveEvents, MoveInput, PlayerState, player_move_events};

/// The movement tick length, in seconds. Movement runs at this fixed rate
/// regardless of frame rate; leftover time is carried into the next frame.
pub const TICK_SECONDS: f32 = 1.0 / 100.0;

/// The most ticks one frame may run, so a long stall (a breakpoint, a
/// window drag) cannot turn into an unbounded simulation burst.
pub const MAX_TICKS_PER_FRAME: u32 = 10;

/// The steepest pitch the view may reach, in degrees.
pub const MAX_PITCH_DEGREES: f32 = 89.0;

/// Which way the player is being pushed this frame, in their own frame of
/// reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControllerInput {
    /// Forward (`+1`) and back (`-1`).
    pub forward: i8,
    /// Right (`+1`) and left (`-1`).
    pub right: i8,
    /// Up (`+1`) and down (`-1`); only used in noclip and while swimming.
    pub up: i8,
    /// Whether the jump key is held.
    pub jump: bool,
    /// Whether the duck key is held.
    pub duck: bool,
}

/// A walking player driven from a host application's frame loop.
#[derive(Debug, Clone)]
pub struct PlayerController {
    /// The simulated player.
    pub state: PlayerState,
    /// The movement constants in force.
    pub config: MoveConfig,
    /// View yaw in degrees, counter-clockwise around +Z from +X.
    pub yaw: f32,
    /// View pitch in degrees; positive looks down.
    pub pitch: f32,
    /// The velocity of whatever is carrying the player this frame (a
    /// platform or train underfoot). The host sets it from the game's mover
    /// state; it is applied for the duration of each move and removed
    /// again, so it never accumulates into the player's own velocity.
    pub base_velocity: Vec3,
    /// Whether the player owns the long jump module, enabling the
    /// duck-then-jump long jump.
    pub long_jump_owned: bool,
    last_events: MoveEvents,
    leftover: f32,
}

impl Default for PlayerController {
    fn default() -> Self {
        Self {
            state: PlayerState::default(),
            config: MoveConfig::default(),
            yaw: 0.0,
            pitch: 0.0,
            base_velocity: Vec3::ZERO,
            long_jump_owned: false,
            last_events: MoveEvents::default(),
            leftover: 0.0,
        }
    }
}

impl PlayerController {
    /// Places a player at a spawn point, treating `origin` as the entity
    /// origin an `info_player_start` names.
    #[must_use]
    pub fn spawn_at(origin: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            state: PlayerState::at(origin),
            yaw,
            pitch: pitch.clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES),
            ..Self::default()
        }
    }

    /// The camera position for the current stance: the eye height above the
    /// entity origin (28 units standing, 12 ducked).
    #[must_use]
    pub fn eye_position(&self) -> Vec3 {
        self.state.eye_position(&self.config)
    }

    /// The unit view direction in GoldSrc world space.
    #[must_use]
    pub fn view_direction(&self) -> Vec3 {
        let yaw = self.yaw.to_radians();
        let pitch = self.pitch.to_radians();
        let (yaw_sin, yaw_cos) = (libm::sinf(yaw), libm::cosf(yaw));
        let (pitch_sin, pitch_cos) = (libm::sinf(pitch), libm::cosf(pitch));
        Vec3::new(pitch_cos * yaw_cos, pitch_cos * yaw_sin, -pitch_sin).normalize_or_zero()
    }

    /// Applies relative mouse motion in pixels scaled by `sensitivity`
    /// degrees per pixel.
    pub fn apply_mouse_delta(&mut self, delta_x: f32, delta_y: f32, sensitivity: f32) {
        if !delta_x.is_finite() || !delta_y.is_finite() || !sensitivity.is_finite() {
            return;
        }
        let yaw = libm::fmodf(self.yaw - delta_x * sensitivity, 360.0);
        self.yaw = if yaw < 0.0 { yaw + 360.0 } else { yaw };
        self.pitch =
            (self.pitch + delta_y * sensitivity).clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
    }

    /// Everything the ticks run by the last [`Self::advance`] call
    /// reported, merged: any landing keeps the fastest impact speed and
    /// every flag is the OR of the ticks'. `ohl-player` turns these into
    /// fall damage, drowning and HEV suit events.
    #[must_use]
    pub fn last_move_events(&self) -> MoveEvents {
        self.last_events
    }

    /// Whether collision is currently disabled.
    #[must_use]
    pub fn noclip(&self) -> bool {
        self.state.noclip
    }

    /// Turns noclip on or off. Leaving noclip clears any leftover velocity
    /// so the player drops from a standstill.
    pub fn set_noclip(&mut self, enabled: bool) {
        self.state.noclip = enabled;
        self.state.velocity = Vec3::ZERO;
    }

    /// Toggles noclip and returns its new value.
    pub fn toggle_noclip(&mut self) -> bool {
        self.set_noclip(!self.state.noclip);
        self.state.noclip
    }

    /// Builds the world-space movement wish for `input` from the current
    /// view angles. Only yaw steers walking; pitch additionally steers the
    /// noclip and swimming wish.
    #[must_use]
    pub fn wish_move(&self, input: &ControllerInput) -> Vec3 {
        let forward = if self.state.noclip || self.state.is_swimming() {
            self.view_direction()
        } else {
            let yaw = self.yaw.to_radians();
            let (sin, cos) = (libm::sinf(yaw), libm::cosf(yaw));
            Vec3::new(cos, sin, 0.0)
        };
        // GoldSrc is Z-up with +Y to the left, so "right" is forward turned
        // clockwise about +Z.
        let right = Vec3::new(forward.y, -forward.x, 0.0).normalize_or_zero();
        let mut wish = forward * f32::from(input.forward.clamp(-1, 1))
            + right * f32::from(input.right.clamp(-1, 1));
        if self.state.noclip || self.state.is_swimming() {
            wish += Vec3::Z * f32::from(input.up.clamp(-1, 1));
        }
        wish.normalize_or_zero()
    }

    /// Advances the player by `frame_seconds` of wall-clock time, running
    /// whole [`TICK_SECONDS`] ticks and carrying the remainder forward.
    /// Returns how many ticks ran.
    pub fn advance(
        &mut self,
        model: &CollisionModel,
        input: &ControllerInput,
        frame_seconds: f32,
    ) -> u32 {
        if !frame_seconds.is_finite() || frame_seconds <= 0.0 {
            return 0;
        }
        self.last_events = MoveEvents::default();
        self.leftover += frame_seconds.min(0.25);
        let move_input = MoveInput {
            wish_move: self.wish_move(input),
            jump: input.jump,
            duck: input.duck,
            base_velocity: self.base_velocity,
            long_jump: self.long_jump_owned,
        };
        let mut ticks = 0;
        while self.leftover >= TICK_SECONDS && ticks < MAX_TICKS_PER_FRAME {
            let events = player_move_events(
                model,
                &mut self.state,
                &move_input,
                &self.config,
                TICK_SECONDS,
            );
            self.last_events.merge(events);
            self.leftover -= TICK_SECONDS;
            ticks += 1;
        }
        if ticks == MAX_TICKS_PER_FRAME {
            // Drop the backlog instead of catching up forever.
            self.leftover = 0.0;
        }
        ticks
    }
}
