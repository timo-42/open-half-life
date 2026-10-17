//! Server-authoritative deathmatch actors, independently of any transport.
//!
//! These are project-defined adapter types, not GoldSrc wire structures or
//! compatibility claims. They use the existing, publicly sourced skirmish
//! gameplay; a transport supplies validated intent and receives owned state.
//! Commands consume only the server's fixed-step time. Sending additional or
//! longer commands never advances the world, weapons or respawn clocks.

use std::collections::VecDeque;

use glam::Vec3;
use ohl_combat::WeaponId;

use crate::systems::LatchedInput;
use crate::{
    AssetSource, EngineError, Game, GameConfig, GameEvent, Input, SkirmishConfig, SkirmishStatus,
};

/// Project-defined server capacity, independent of the bot limit.
pub const MAX_PLAYERS: u8 = 32;
/// Project-defined maximum buffered commands per connection.
pub const MAX_QUEUED_COMMANDS: usize = 64;
/// Project-defined maximum duration admitted for one command.
pub const MAX_COMMAND_MILLIS: u16 = 100;

/// A host-chosen session identity; hosts must replace it when starting a new match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// A session-scoped player slot. Its generation changes whenever a slot is reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId {
    pub session: SessionId,
    pub slot: u8,
    pub generation: u32,
}

/// Held controls. Use, reload and flashlight transitions are latched as edges.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerButtons {
    pub attack: bool,
    pub attack2: bool,
    pub jump: bool,
    pub duck: bool,
    pub use_held: bool,
    pub reload: bool,
    pub flashlight: bool,
}

/// Intent in project units: absolute pitch/yaw/roll degrees and normalized
/// forward/right/up movement. A protocol adapter performs its own conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerCommand {
    pub sequence: u32,
    pub duration_millis: u16,
    pub view_angles: [f32; 3],
    pub movement: [f32; 3],
    pub buttons: PlayerButtons,
    pub weapon: Option<WeaponId>,
}

impl Default for PlayerCommand {
    fn default() -> Self {
        Self {
            sequence: 0,
            duration_millis: 10,
            view_angles: [0.0; 3],
            movement: [0.0; 3],
            buttons: PlayerButtons::default(),
            weapon: None,
        }
    }
}

impl PlayerCommand {
    fn validate(self) -> Result<Self, DeathmatchError> {
        if !(1..=MAX_COMMAND_MILLIS).contains(&self.duration_millis)
            || self.view_angles.iter().any(|angle| !angle.is_finite())
            || self
                .movement
                .iter()
                .any(|axis| !axis.is_finite() || axis.abs() > 1.0)
            || self.view_angles[0].abs() > ohl_physics::controller::MAX_PITCH_DEGREES
            || self.view_angles[2].abs() > f32::EPSILON
        {
            return Err(DeathmatchError::InvalidCommand);
        }
        let mut command = self;
        command.view_angles[1] = command.view_angles[1].rem_euclid(360.0);
        Ok(command)
    }
}

/// Match options. `max_players` includes bots; no local player is reserved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeathmatchConfig {
    pub session_id: SessionId,
    pub max_players: u8,
    pub rules: SkirmishConfig,
}

impl Default for DeathmatchConfig {
    fn default() -> Self {
        Self {
            session_id: SessionId(1),
            max_players: 8,
            rules: SkirmishConfig {
                bots: 0,
                ..SkirmishConfig::default()
            },
        }
    }
}

/// A bounded admission or intent failure. No peer-provided text is included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathmatchError {
    Engine(EngineError),
    InvalidConfig,
    InvalidName,
    Full,
    UnknownPlayer,
    InvalidCommand,
    StaleCommand,
    CommandQueueFull,
}

impl std::fmt::Display for DeathmatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Engine(error) => return error.fmt(f),
            Self::InvalidConfig => "invalid deathmatch configuration",
            Self::InvalidName => "invalid player name",
            Self::Full => "the deathmatch is full",
            Self::UnknownPlayer => "unknown or expired player identity",
            Self::InvalidCommand => "invalid player command",
            Self::StaleCommand => "duplicate or stale player command",
            Self::CommandQueueFull => "the player command queue is full",
        })
    }
}

impl std::error::Error for DeathmatchError {}
impl From<EngineError> for DeathmatchError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

/// Owned actor state, safe to inspect without mutating the authority.
/// Ammunition and health are available to the host; a transport should select
/// the recipient-appropriate subset rather than broadcasting private inventory.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "actor snapshots contain independent physical and presentation flags"
)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub name: String,
    pub is_bot: bool,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub view_angles: [f32; 3],
    pub on_ground: bool,
    pub ducked: bool,
    pub health: f32,
    pub armor: f32,
    pub flashlight_on: bool,
    pub alive: bool,
    pub weapon: Option<WeaponId>,
    pub clip: u32,
    pub reserve: u32,
    pub frags: i32,
    pub deaths: u32,
    pub last_processed_sequence: Option<u32>,
}

/// Owned match state; no raw ECS handles or asset bytes cross this API.
#[derive(Debug, Clone, PartialEq)]
pub struct DeathmatchSnapshot {
    pub session_id: SessionId,
    pub tick: u64,
    pub elapsed: f32,
    pub players: Vec<PlayerSnapshot>,
    pub status: SkirmishStatus,
}

/// One dedicated authority. The contained game's spectator is disabled and
/// never takes pickups, occupies a spawn, receives frags or appears as a player.
pub struct Deathmatch {
    game: Game,
    config: DeathmatchConfig,
    generations: [u32; MAX_PLAYERS as usize],
}

impl Deathmatch {
    /// Loads a map and starts a dedicated match.
    ///
    /// # Errors
    /// Invalid bounds, missing map/collision or missing spawn points.
    pub fn load(
        source: &dyn AssetSource,
        map: &str,
        game_config: &GameConfig,
        config: &DeathmatchConfig,
    ) -> Result<Self, DeathmatchError> {
        Self::from_game(Game::load_with(source, map, game_config)?, source, config)
    }

    /// Starts a dedicated match from caller-provided map bytes.
    ///
    /// # Errors
    /// As [`Self::load`].
    pub fn from_map_bytes(
        source: &dyn AssetSource,
        map: &str,
        bytes: &[u8],
        config: &DeathmatchConfig,
    ) -> Result<Self, DeathmatchError> {
        Self::from_game(Game::from_map_bytes(source, map, bytes)?, source, config)
    }

    fn from_game(
        mut game: Game,
        source: &dyn AssetSource,
        config: &DeathmatchConfig,
    ) -> Result<Self, DeathmatchError> {
        if config.session_id.0 == 0
            || !(1..=MAX_PLAYERS).contains(&config.max_players)
            || config.rules.bots > crate::MAX_BOTS
            || config.rules.bots > config.max_players
        {
            return Err(DeathmatchError::InvalidConfig);
        }
        game.start_dedicated_deathmatch(source, &config.rules)?;
        Ok(Self {
            game,
            config: *config,
            generations: [1; MAX_PLAYERS as usize],
        })
    }

    /// Admits one human with a fresh slot generation.
    ///
    /// # Errors
    /// Invalid name, full server or exhausted slot generations.
    pub fn join(&mut self, name: &str) -> Result<PlayerId, DeathmatchError> {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 32
            || name
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\\' | '"'))
        {
            return Err(DeathmatchError::InvalidName);
        }
        let (level, systems) = self.game.deathmatch_parts_mut();
        let state = systems
            .skirmish
            .as_mut()
            .ok_or(DeathmatchError::InvalidConfig)?;
        let index = state.join_remote(level, name, usize::from(self.config.max_players))?;
        Ok(PlayerId {
            session: self.config.session_id,
            slot: u8::try_from(index).map_err(|_| DeathmatchError::Full)?,
            generation: self.generations[index],
        })
    }

    /// Disconnects and expires the player's identity immediately.
    ///
    /// # Errors
    /// The identity is stale, foreign or belongs to a bot.
    pub fn leave(&mut self, id: PlayerId) -> Result<(), DeathmatchError> {
        let index = self.remote_index(id)?;
        let generation = self.generations[index]
            .checked_add(1)
            .ok_or(DeathmatchError::Full)?;
        let (level, systems) = self.game.deathmatch_parts_mut();
        if let Some(state) = systems.skirmish.as_mut() {
            state.leave_remote(level, index);
        }
        self.generations[index] = generation;
        Ok(())
    }

    /// Queues intent without advancing the simulation. Edges survive frames
    /// that produce no fixed step, and packet replay cannot repeat them.
    ///
    /// # Errors
    /// Invalid identity, non-finite/out-of-bounds input, replay or full queue.
    pub fn submit(&mut self, id: PlayerId, command: PlayerCommand) -> Result<(), DeathmatchError> {
        let index = self.remote_index(id)?;
        let command = command.validate()?;
        let (_, systems) = self.game.deathmatch_parts_mut();
        systems
            .skirmish
            .as_mut()
            .ok_or(DeathmatchError::InvalidConfig)?
            .submit_remote(index, command)
    }

    fn remote_index(&self, id: PlayerId) -> Result<usize, DeathmatchError> {
        let index = usize::from(id.slot);
        if id.session != self.config.session_id
            || self.generations.get(index) != Some(&id.generation)
            || !self
                .game
                .deathmatch_state()
                .is_some_and(|state| state.remote_connected(index))
        {
            return Err(DeathmatchError::UnknownPlayer);
        }
        Ok(index)
    }

    /// Advances only by server time, clamped by the ordinary engine clock.
    pub fn tick(&mut self, seconds: f32) -> Vec<GameEvent> {
        self.game.tick(seconds, &Input::default())
    }

    /// Captures all currently connected combatants in stable slot order.
    #[must_use]
    pub fn snapshot(&self) -> DeathmatchSnapshot {
        let state = self
            .game
            .deathmatch_state()
            .expect("a dedicated match retains its rules");
        DeathmatchSnapshot {
            session_id: self.config.session_id,
            tick: self.game.deathmatch_tick_count(),
            elapsed: self.game.elapsed(),
            players: state.player_snapshots(self.config.session_id, &self.generations),
            status: state.status(false),
        }
    }

    /// Read-only world access for host inspection and eventual presentation.
    #[must_use]
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// Own one admitted human's camera/HUD plus public roster and world scene.
    /// This selects no spectator state and never advances or drains authority.
    /// First-person animation, wire mapping and prediction remain outside it.
    ///
    /// # Errors
    /// Expired/foreign/bot identity, unavailable rules or invalid scene data.
    pub fn capture_recipient(
        &self,
        epoch: crate::scene::SceneEpoch,
        id: PlayerId,
    ) -> Result<crate::recipient::RecipientFrame, crate::recipient::RecipientError> {
        use crate::recipient::{RecipientError, RecipientFrame};
        self.remote_index(id)
            .map_err(|_| RecipientError::WrongRecipient)?;
        let state = self
            .game
            .deathmatch_state()
            .ok_or(RecipientError::WrongRecipient)?
            .recipient_state(id, &self.generations, epoch)?;
        Ok(RecipientFrame::new(self.game.capture_scene(epoch)?, state))
    }

    /// Draw this catalog's accepted recipient view; only GPU caches are mutable.
    ///
    /// # Errors
    /// No frame, foreign resource catalog, invalid projection or GPU failure.
    pub fn render_recipient(
        &mut self,
        context: &ohl_render::GpuContext,
        target: crate::RenderTarget<'_>,
        replica: &crate::recipient::RecipientReplica,
        fov_y_degrees: f32,
    ) -> Result<(), crate::recipient::RecipientError> {
        self.game
            .render_recipient(context, target, replica, fov_y_degrees)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RemoteStep {
    pub(crate) input: LatchedInput,
    pub(crate) wish_move: Vec3,
    pub(crate) view_angles: Option<[f32; 3]>,
    pub(crate) weapon: Option<WeaponId>,
}

struct QueuedCommand {
    command: PlayerCommand,
    edges: LatchedInput,
    remaining_millis: f32,
    started: bool,
}

pub(crate) struct RemoteControl {
    queue: VecDeque<QueuedCommand>,
    submitted: Option<u32>,
    previous: PlayerButtons,
    pub(crate) processed: Option<u32>,
    pub(crate) step: RemoteStep,
    pub(crate) respawn_released: bool,
}

impl RemoteControl {
    pub(crate) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            submitted: None,
            previous: PlayerButtons::default(),
            processed: None,
            step: RemoteStep::default(),
            respawn_released: true,
        }
    }

    pub(crate) fn submit(&mut self, command: PlayerCommand) -> Result<(), DeathmatchError> {
        if self.submitted.is_some_and(|previous| {
            let distance = command.sequence.wrapping_sub(previous);
            distance == 0 || distance > u32::MAX / 2
        }) {
            return Err(DeathmatchError::StaleCommand);
        }
        if self.queue.len() >= MAX_QUEUED_COMMANDS {
            return Err(DeathmatchError::CommandQueueFull);
        }
        let buttons = command.buttons;
        let edges = LatchedInput {
            use_pressed: buttons.use_held && !self.previous.use_held,
            reload_pressed: buttons.reload && !self.previous.reload,
            flashlight_pressed: buttons.flashlight && !self.previous.flashlight,
            ..LatchedInput::default()
        };
        self.queue.push_back(QueuedCommand {
            command,
            edges,
            remaining_millis: f32::from(command.duration_millis),
            started: false,
        });
        self.submitted = Some(command.sequence);
        self.previous = buttons;
        Ok(())
    }

    /// Project-defined fixed-window aggregation, not a claim about original
    /// command scheduling: orient and cap each analog wish before averaging
    /// by duration. Keep final aim and any button pulse. No command creates
    /// an additional simulation or weapon step.
    pub(crate) fn prepare(&mut self, dt: f32, swimming: bool) {
        let millis = dt * 1000.0;
        let mut budget = millis;
        let mut step = RemoteStep::default();
        while budget > f32::EPSILON {
            let Some(queued) = self.queue.front_mut() else {
                break;
            };
            let command = queued.command;
            let portion = queued.remaining_millis.min(budget);
            step.wish_move += command_wish(command, swimming) * (portion / millis);
            let buttons = command.buttons;
            step.input.attack |= buttons.attack;
            step.input.attack2 |= buttons.attack2;
            step.input.jump |= buttons.jump;
            step.input.duck = buttons.duck;
            step.input.use_held = buttons.use_held;
            step.view_angles = Some(command.view_angles);
            if command.weapon.is_some() {
                step.weapon = command.weapon;
            }
            if !queued.started {
                step.input.use_pressed |= queued.edges.use_pressed;
                step.input.reload_pressed |= queued.edges.reload_pressed;
                step.input.flashlight_pressed |= queued.edges.flashlight_pressed;
                queued.started = true;
            }
            budget -= portion;
            queued.remaining_millis -= portion;
            if queued.remaining_millis <= f32::EPSILON {
                self.processed = Some(command.sequence);
                self.queue.pop_front();
            }
        }
        self.step = step;
    }
}

fn command_wish(command: PlayerCommand, swimming: bool) -> Vec3 {
    let yaw = command.view_angles[1].to_radians();
    let forward = if swimming {
        let pitch = command.view_angles[0].to_radians();
        Vec3::new(
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            -pitch.sin(),
        )
    } else {
        Vec3::new(yaw.cos(), yaw.sin(), 0.0)
    };
    let right = Vec3::new(forward.y, -forward.x, 0.0).normalize_or_zero();
    let [forward_axis, right_axis, up_axis] = command.movement;
    let wish = forward * forward_axis
        + right * right_axis
        + if swimming {
            Vec3::Z * up_axis
        } else {
            Vec3::ZERO
        };
    wish.clamp_length_max(1.0)
}
