//! The game state and its two verbs: [`Game::tick`] and
//! [`Game::render`](crate::Game::render).

use glam::Vec3;
use ohl_game::{Event, find_usable_within};
use ohl_physics::{ControllerInput, PlayerController};
use ohl_render::{FreeFlyCamera, GpuContext, LightStyles, MoveInput, wgpu};

use crate::assets::AssetSource;
use crate::error::{EngineError, Result};
use crate::input::Input;
use crate::level::Level;
use crate::render::{RenderTarget, Renderers};
use crate::{MAX_TICK_SECONDS, MOUSE_SENSITIVITY, USE_RADIUS};

/// Something the simulation produced that only the host can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// A `trigger_changelevel` fired. The host decides whether and when to
    /// call [`Game::change_level`].
    LevelChange {
        /// The destination map's bare name.
        map: String,
        /// The landmark both maps place, used to carry the player's
        /// relative position across.
        landmark: String,
    },
}

/// One loaded level plus everything that acts on it.
pub struct Game {
    level: Level,
    camera: FreeFlyCamera,
    controller: PlayerController,
    light_styles: LightStyles,
    renderers: Option<Renderers>,
    /// The colour format the renderers were built for, kept so a level
    /// change can rebuild them without the host re-declaring it.
    format: Option<wgpu::TextureFormat>,
    elapsed: f32,
}

impl Game {
    /// Loads `map` through `source` and places the player at its
    /// `info_player_start`.
    ///
    /// # Errors
    /// As [`crate::level::Level::load`].
    pub fn load(source: &dyn AssetSource, map: &str) -> Result<Self> {
        Ok(Self::from_level(Level::load(source, map)?))
    }

    /// Loads a level from map bytes the caller already holds.
    ///
    /// # Errors
    /// As [`crate::level::Level::from_bytes`].
    pub fn from_map_bytes(source: &dyn AssetSource, map: &str, bytes: &[u8]) -> Result<Self> {
        Ok(Self::from_level(Level::from_bytes(source, map, bytes)?))
    }

    fn from_level(level: Level) -> Self {
        let camera = level
            .spawn
            .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
        let controller = level.spawn.map_or_else(PlayerController::default, |spawn| {
            PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
        });
        Self {
            level,
            camera,
            controller,
            light_styles: LightStyles::new(),
            renderers: None,
            format: None,
            elapsed: 0.0,
        }
    }

    /// The map name this game is currently running.
    #[must_use]
    pub fn map(&self) -> &str {
        &self.level.name
    }

    /// How many studio models this level references that the payload does
    /// not publish. Media-derived: report it as data, never in a log line.
    #[must_use]
    pub fn missing_model_count(&self) -> usize {
        self.level.missing_models
    }

    /// How many brush-entity submodels this level references that could not
    /// be built, and so are not drawn. Media-derived: report it as data,
    /// never in a log line.
    #[must_use]
    pub fn unbuildable_submodel_count(&self) -> usize {
        self.level.unbuildable_submodels
    }

    /// Whether this level has usable collision hulls, i.e. whether the
    /// player walks rather than flies.
    #[must_use]
    pub fn has_collision(&self) -> bool {
        self.level.collision.is_some()
    }

    /// Whether the payload published this map's skybox.
    #[must_use]
    pub fn has_skybox(&self) -> bool {
        self.level.skybox.is_some()
    }

    /// How many brush-entity submodels this level draws.
    #[must_use]
    pub fn submodel_count(&self) -> usize {
        self.level.submodels.len()
    }

    /// The entity registry this level is running, for a host that needs to
    /// read entity state (a HUD, a debug overlay, a test).
    #[must_use]
    pub fn registry(&self) -> &ohl_game::Registry {
        &self.level.registry
    }

    /// This level's `env_sprite`/`env_glow`/`cycler_sprite` placements,
    /// drawn each frame by [`Self::render`].
    #[must_use]
    pub fn sprites(&self) -> &[crate::level::SpritePlacement] {
        &self.level.sprites
    }

    /// How many referenced sprites this level references that the payload
    /// does not publish. Media-derived: report it as data, never in a log
    /// line.
    #[must_use]
    pub fn missing_sprite_count(&self) -> usize {
        self.level.missing_sprites
    }

    /// The camera the next [`Self::render`] draws from.
    #[must_use]
    pub fn camera(&self) -> &FreeFlyCamera {
        &self.camera
    }

    /// The player's eye position in world space.
    #[must_use]
    pub fn eye_position(&self) -> [f32; 3] {
        self.camera.position
    }

    /// Seconds of simulated time since this level was loaded.
    #[must_use]
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Moves the camera (and the walking player) to an explicit viewpoint,
    /// for headless captures at a chosen position.
    pub fn set_viewpoint(&mut self, position: [f32; 3], pitch: f32, yaw: f32) {
        self.camera.position = position;
        self.camera.pitch = pitch;
        self.camera.yaw = yaw;
        self.controller.yaw = yaw;
        self.controller.pitch = pitch;
        self.controller.state.origin = Vec3::from_array(position);
        // A caller-chosen viewpoint is a free camera, not a spawn: keep the
        // physics controller from immediately dragging it back to the floor.
        self.controller.set_noclip(true);
    }

    /// Advances the frame by `dt` seconds and returns the events the host
    /// must act on.
    pub fn tick(&mut self, dt: f32, input: &Input) -> Vec<GameEvent> {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_TICK_SECONDS)
        } else {
            0.0
        };
        self.elapsed += dt;

        let (delta_x, delta_y) = input.mouse_delta;
        if delta_x != 0.0 || delta_y != 0.0 {
            self.camera.apply_mouse_delta(delta_x, delta_y);
            self.controller
                .apply_mouse_delta(delta_x, delta_y, MOUSE_SENSITIVITY);
        }

        if let Some(collision) = self.level.collision.as_ref() {
            self.controller.yaw = self.camera.yaw;
            self.controller.pitch = self.camera.pitch;
            let controller_input = ControllerInput {
                forward: input.forward,
                right: input.right,
                up: input.up,
                jump: input.jump,
                duck: input.duck,
            };
            self.controller.advance(collision, &controller_input, dt);
            self.camera.position = self.controller.eye_position().to_array();
        } else {
            self.camera.update(
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

        let mut events = Vec::new();
        if input.use_pressed {
            let position = Vec3::from_array(self.camera.position);
            if let Some(entity) = find_usable_within(&self.level.registry, position, USE_RADIUS) {
                self.level.simulation.use_entity(
                    &mut self.level.registry,
                    entity,
                    None,
                    &mut events,
                );
            }
        }
        events.extend(self.level.simulation.tick(&mut self.level.registry, dt));

        events
            .into_iter()
            .map(|event| match event {
                Event::LevelChange(change) => GameEvent::LevelChange {
                    map: change.map,
                    landmark: change.landmark,
                },
            })
            .collect()
    }

    /// Loads `map` and places the player relative to `landmark`.
    ///
    /// The basic transition this milestone implements: the player's offset
    /// from the landmark in the *current* level is preserved against the
    /// same-named landmark in the destination. When either level lacks the
    /// landmark, the destination's own `info_player_start` is used instead.
    /// Nothing else (inventory, entity state, global variables) carries
    /// across yet.
    ///
    /// # Errors
    /// As [`Game::load`]; the current level is left untouched on failure.
    pub fn change_level(
        &mut self,
        source: &dyn AssetSource,
        map: &str,
        landmark: &str,
    ) -> Result<()> {
        let offset = self
            .level
            .landmark_origin(landmark)
            .map(|origin| Vec3::from_array(self.camera.position) - origin);

        let next = Level::load(source, map)?;
        let placement = offset
            .zip(next.landmark_origin(landmark))
            .map(|(offset, origin)| origin + offset);

        let format = self.format;
        let (yaw, pitch) = (self.camera.yaw, self.camera.pitch);
        *self = Self::from_level(next);
        if let Some(position) = placement {
            self.camera.position = position.to_array();
            self.camera.yaw = yaw;
            self.camera.pitch = pitch;
            self.controller = PlayerController::spawn_at(position, yaw, pitch);
        }
        // The previous level's uploaded geometry is gone with it; only the
        // target format carries over, and the next `render` rebuilds.
        self.format = format;
        Ok(())
    }

    /// Draws the current frame into `target`, creating the GPU resources
    /// on first use.
    ///
    /// # Errors
    /// [`crate::EngineError::Renderer`] when a GPU resource for this level
    /// could not be created.
    pub fn render(&mut self, context: &GpuContext, target: RenderTarget<'_>) -> Result<()> {
        self.format = Some(target.format);
        if self.renderers.is_none() {
            self.renderers = Some(Renderers::new(context, &self.level, target.format)?);
        }
        let Some(renderers) = self.renderers.as_mut() else {
            return Err(EngineError::Renderer);
        };
        renderers.draw(
            context,
            &self.level,
            &self.camera,
            &self.light_styles,
            self.elapsed,
            target,
        );
        Ok(())
    }
}
