//! The per-step system list: what one fixed simulation step does, in order.
//!
//! [`crate::Game::tick`] is a frame loop; [`Systems::step`] is the frame
//! loop's body, run once per [`crate::tick::TICK_SECONDS`]. The order below
//! is normative, and two of its rules are worth stating outright:
//!
//! - **Damage is resolved after AI thinks, not before.** A monster shot this
//!   step reacts on the next one, so the think phase never mutates the
//!   health it just read.
//! - **Movers run last.** A door blocked or damaged this step resolves
//!   against positions everything else has already agreed on.
//!
//! Phases this package does not fill yet are present as empty hooks with the
//! signature the phase needs, so a later package adds a body rather than
//! re-cutting the list.
//!
//! # Determinism
//!
//! No phase reads a wall clock, and no phase iterates a `HashMap`. The one
//! random stream the later phases share is seeded from
//! [`SystemsConfig::rng_seed`], never from the environment.
//!
//! # Logging
//!
//! Nothing here logs; see the crate-level note.

use glam::Vec3;
use ohl_game::registry::Transform;
use ohl_game::{Event, find_usable_within};
use ohl_physics::{ControllerInput, PlayerController};
use ohl_render::{FreeFlyCamera, MoveInput};

use crate::USE_RADIUS;
use crate::components::StudioAnim;
use crate::input::Input;
use crate::level::Level;

/// The project's default random seed.
///
/// A constant, not a clock or an environment read: two games built from the
/// same map bytes with the same config must step identically. It is saved
/// and restored with the rest of the simulation state.
pub const DEFAULT_RNG_SEED: u64 = 0x4F48_4C5F_5039_0001;

/// Everything about the step list a host chooses rather than the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemsConfig {
    /// Seeds the single random stream the simulation phases share.
    pub rng_seed: u64,
}

impl Default for SystemsConfig {
    fn default() -> Self {
        Self {
            rng_seed: DEFAULT_RNG_SEED,
        }
    }
}

/// One frame's intent, latched for the steps that frame runs.
///
/// Held axes apply to every step of the frame; edges (a press, not a hold)
/// apply to the first step only, so a single press cannot fire a phase ten
/// times in one long frame.
// As `Input`: independent buttons, latched.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct LatchedInput {
    pub(crate) forward: i8,
    pub(crate) right: i8,
    pub(crate) up: i8,
    pub(crate) jump: bool,
    pub(crate) duck: bool,
    pub(crate) attack: bool,
    pub(crate) attack2: bool,
    pub(crate) use_pressed: bool,
    pub(crate) reload_pressed: bool,
    pub(crate) flashlight_pressed: bool,
    pub(crate) select_slot: Option<u8>,
}

impl LatchedInput {
    /// The held state of `input`, with every edge cleared.
    fn held(input: &Input) -> Self {
        Self {
            forward: input.forward,
            right: input.right,
            up: input.up,
            jump: input.jump,
            duck: input.duck,
            attack: input.attack,
            attack2: input.attack2,
            use_pressed: false,
            reload_pressed: false,
            flashlight_pressed: false,
            select_slot: None,
        }
    }

    /// The held state of `input` plus the edges it carries.
    fn with_edges(input: &Input) -> Self {
        Self {
            use_pressed: input.use_pressed,
            reload_pressed: input.reload,
            flashlight_pressed: input.flashlight_pressed,
            select_slot: input.select_slot,
            ..Self::held(input)
        }
    }

    fn controller_input(self) -> ControllerInput {
        ControllerInput {
            forward: self.forward,
            right: self.right,
            up: self.up,
            jump: self.jump,
            duck: self.duck,
        }
    }
}

/// The systems one [`crate::Game`] steps, and the state they share.
pub struct Systems {
    config: SystemsConfig,
    /// The frame's input, latched by [`Systems::begin_frame`].
    frame_input: Input,
    /// Whether the current frame's edges are still unconsumed.
    edges_pending: bool,
    /// The HUD the host draws. Written by the presentation phase; a default
    /// until the gameplay bridge is wired in.
    hud: ohl_ui::hud::HudState,
}

impl Systems {
    /// The step list with `config`'s seed and an empty HUD.
    #[must_use]
    pub fn new(config: SystemsConfig) -> Self {
        Self {
            config,
            frame_input: Input::default(),
            edges_pending: false,
            hud: ohl_ui::hud::HudState::default(),
        }
    }

    /// The configuration this step list runs with.
    #[must_use]
    pub fn config(&self) -> SystemsConfig {
        self.config
    }

    /// Replaces the configuration. Takes effect on the next step.
    pub fn set_config(&mut self, config: SystemsConfig) {
        self.config = config;
    }

    /// The HUD the host draws this frame.
    #[must_use]
    pub fn hud(&self) -> &ohl_ui::hud::HudState {
        &self.hud
    }

    /// Clears everything a level change or a save load invalidates, keeping
    /// the configuration.
    pub(crate) fn reset(&mut self) {
        let config = self.config;
        *self = Self::new(config);
    }

    /// Latches one frame's input. Called once per [`crate::Game::tick`],
    /// before any step runs.
    pub(crate) fn begin_frame(&mut self, input: &Input) {
        self.frame_input = *input;
        self.edges_pending = true;
    }

    /// Runs one fixed simulation step. The phase numbers match this
    /// module's documented order.
    pub(crate) fn step(
        &mut self,
        level: &mut Level,
        camera: &mut FreeFlyCamera,
        controller: &mut PlayerController,
        dt: f32,
        events: &mut Vec<Event>,
    ) {
        let input = self.latch_input(); // 1
        Self::player_move(level, camera, controller, input, dt); // 2
        self.player_systems(dt); // 3
        Self::actor_sync(level, camera, controller, dt); // 4
        self.rebuild_hitbox_index(level); // 5
        self.weapons(dt); // 6
        self.projectiles(dt); // 7
        self.ai_think(level, dt); // 8
        self.resolve_damage(); // 9
        self.lifecycle(dt); // 10
        self.pickups(level, input); // 11
        Self::triggers_and_movers(level, camera, input, dt, events); // 12
        self.presentation(dt); // 13
    }

    /// Phase 1 — input latch. Axes hold for every step of the frame; edges
    /// are handed to the first step only.
    fn latch_input(&mut self) -> LatchedInput {
        if std::mem::take(&mut self.edges_pending) {
            LatchedInput::with_edges(&self.frame_input)
        } else {
            LatchedInput::held(&self.frame_input)
        }
    }

    /// Phase 2 — player move. The walking path runs the collision
    /// controller; a map with no usable hulls falls back to the free-fly
    /// camera, which is what makes an unbuildable map still inspectable.
    fn player_move(
        level: &mut Level,
        camera: &mut FreeFlyCamera,
        controller: &mut PlayerController,
        input: LatchedInput,
        dt: f32,
    ) {
        if let Some(collision) = level.collision.as_ref() {
            controller.yaw = camera.yaw;
            controller.pitch = camera.pitch;
            controller.advance(collision, &input.controller_input(), dt);
            camera.position = controller.eye_position().to_array();
        } else {
            camera.update(
                MoveInput {
                    forward: input.forward,
                    right: input.right,
                    up: input.up,
                    fast: false,
                }
                .clamped(),
                dt,
            );
        }
    }

    /// Phase 3 — player systems: fall damage, drowning, `trigger_hurt`,
    /// the suit, the flashlight and the long jump.
    #[allow(clippy::unused_self)]
    fn player_systems(&mut self, _dt: f32) {}

    /// Phase 4 — actor sync: the player's world entity follows the player,
    /// and every studio animation cursor advances one step.
    fn actor_sync(
        level: &mut Level,
        camera: &FreeFlyCamera,
        controller: &PlayerController,
        dt: f32,
    ) {
        let player = level.player;
        let origin = if level.collision.is_some() {
            controller.state.origin
        } else {
            Vec3::from_array(camera.position)
        };
        if let Ok(mut transform) = level.registry.world.get::<&mut Transform>(player) {
            transform.origin = origin;
            transform.angles = Vec3::new(camera.pitch, camera.yaw, 0.0);
        }
        for anim in &mut level.registry.world.query::<&mut StudioAnim>() {
            anim.advance(dt);
        }
    }

    /// Phase 5 — hitbox index: rebuilt each step from every entity carrying
    /// a pose and an actor, so a trace hits where the model is drawn.
    #[allow(clippy::unused_self)]
    fn rebuild_hitbox_index(&mut self, _level: &mut Level) {}

    /// Phase 6 — weapons: the firing state machine, its hitscan traces and
    /// the damage they queue.
    #[allow(clippy::unused_self)]
    fn weapons(&mut self, _dt: f32) {}

    /// Phase 7 — projectiles and deployables, and the radius damage they
    /// queue.
    #[allow(clippy::unused_self)]
    fn projectiles(&mut self, _dt: f32) {}

    /// Phase 8 — AI think and navigation. Runs before damage resolution on
    /// purpose: see the module note.
    #[allow(clippy::unused_self)]
    fn ai_think(&mut self, _level: &mut Level, _dt: f32) {}

    /// Phase 9 — damage resolution: the queue is drained once, in insertion
    /// order.
    #[allow(clippy::unused_self)]
    fn resolve_damage(&mut self) {}

    /// Phase 10 — lifecycle: deaths, corpses, gibs and `monstermaker`.
    #[allow(clippy::unused_self)]
    fn lifecycle(&mut self, _dt: f32) {}

    /// Phase 11 — pickups: touch tests and the use-and-hold chargers.
    #[allow(clippy::unused_self)]
    fn pickups(&mut self, _level: &mut Level, _input: LatchedInput) {}

    /// Phase 12 — triggers and movers, last so everything else has already
    /// settled.
    fn triggers_and_movers(
        level: &mut Level,
        camera: &FreeFlyCamera,
        input: LatchedInput,
        dt: f32,
        events: &mut Vec<Event>,
    ) {
        if input.use_pressed {
            let position = Vec3::from_array(camera.position);
            if let Some(entity) = find_usable_within(&level.registry, position, USE_RADIUS) {
                level
                    .simulation
                    .use_entity(&mut level.registry, entity, None, events);
            }
        }
        events.extend(level.simulation.tick(&mut level.registry, dt));
    }

    /// Phase 13 — presentation: the HUD, sound cues and view-model actions
    /// the host reads after the step.
    #[allow(clippy::unused_self)]
    fn presentation(&mut self, _dt: f32) {}
}

impl Default for Systems {
    fn default() -> Self {
        Self::new(SystemsConfig::default())
    }
}
