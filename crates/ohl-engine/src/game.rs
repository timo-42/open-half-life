//! The game state and its two verbs: [`Game::tick`] and
//! [`Game::render`](crate::Game::render), plus the campaign flow layered on
//! them: level transitions, save/load, chapter titles and difficulty.

use glam::Vec3;
use ohl_campaign::{Difficulty, SkillTable};
use ohl_game::Event;
use ohl_physics::PlayerController;
use ohl_render::{FreeFlyCamera, GpuContext, LightStyles};

use crate::assets::AssetSource;
use crate::error::{EngineError, Result};
use crate::input::Input;
use crate::level::Level;
use crate::render::{RenderTarget, Renderers};
use crate::save::{EngineHeader, GameSave, ViewState};
use crate::systems::{Systems, SystemsConfig};
use crate::text::{MessageBlock, SentenceLookup, TitleLibrary, load_skill_table};
use crate::tick::{TICK_SECONDS, TickClock};
use crate::transition::{
    DefaultPlayerCarry, EntitySnapshot, GlobalStateTable, PlayerCarry, TransitionState,
};
use crate::{MAX_TICK_SECONDS, MOUSE_SENSITIVITY};

/// Something the simulation produced that only the host can act on.
#[derive(Debug, Clone, PartialEq)]
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
    /// A map was loaded whose chapter `ohl-campaign` knows a title for.
    /// The host shows it in the HUD's message area.
    ChapterTitle(String),
    /// An `env_message`/`game_text` fired and was resolved against
    /// `titles.txt`.
    Message {
        /// The resolved text and its fade/hold timings.
        block: MessageBlock,
    },
    /// A cue the host should play. `ohl_gameplay::SoundCue::path` is always
    /// `None` until a clean-room provenance review admits a sound asset
    /// path; see `crate::presentation`'s module docs.
    Sound(ohl_gameplay::SoundCue),
    /// An HEV suit voice occasion, which the host maps to a voice line.
    Suit(ohl_player::SuitEvent),
    /// A viewmodel animation the host should play next.
    ViewModel(ohl_gameplay::ViewModelAction),
    /// The player's health reached zero.
    PlayerDied,
}

/// How a [`Game`] is started: everything the host chooses rather than the
/// map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameConfig {
    /// The campaign difficulty, selecting which `skill.cfg` cvar suffix
    /// [`Game::skill_table`] lookups read.
    pub difficulty: Difficulty,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Medium,
        }
    }
}

/// One loaded level plus everything that acts on it.
pub struct Game {
    level: Level,
    camera: FreeFlyCamera,
    controller: PlayerController,
    light_styles: LightStyles,
    renderers: Option<Renderers>,
    elapsed: f32,
    difficulty: Difficulty,
    skill: SkillTable,
    titles: TitleLibrary,
    sentences: SentenceLookup,
    globals: GlobalStateTable,
    carry: Box<dyn PlayerCarry>,
    /// The per-step system list; see [`crate::systems`].
    systems: Systems,
    /// Turns a variable frame time into whole fixed steps.
    clock: TickClock,
    /// Events produced outside [`Self::tick`] (a chapter title on load),
    /// drained by the next tick so a host has exactly one event path.
    pending: Vec<GameEvent>,
}

impl Game {
    /// Loads `map` through `source` and places the player at its
    /// `info_player_start`, on the default difficulty.
    ///
    /// # Errors
    /// As [`crate::level::Level::load`].
    pub fn load(source: &dyn AssetSource, map: &str) -> Result<Self> {
        Self::load_with(source, map, &GameConfig::default())
    }

    /// Loads `map` with a host-chosen [`GameConfig`].
    ///
    /// # Errors
    /// As [`crate::level::Level::load`].
    pub fn load_with(source: &dyn AssetSource, map: &str, config: &GameConfig) -> Result<Self> {
        Ok(Self::from_level(Level::load(source, map)?, source, *config))
    }

    /// Loads a level from map bytes the caller already holds.
    ///
    /// # Errors
    /// As [`crate::level::Level::from_bytes`].
    pub fn from_map_bytes(source: &dyn AssetSource, map: &str, bytes: &[u8]) -> Result<Self> {
        Ok(Self::from_level(
            Level::from_bytes(source, map, bytes)?,
            source,
            GameConfig::default(),
        ))
    }

    fn from_level(mut level: Level, source: &dyn AssetSource, config: GameConfig) -> Self {
        let camera = level
            .spawn
            .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
        let controller = level.spawn.map_or_else(PlayerController::default, |spawn| {
            PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
        });
        let mut globals = GlobalStateTable::new();
        globals.seed_from(&level.registry);
        let pending = chapter_title_event(&level.name).into_iter().collect();
        let skill = load_skill_table(source);
        let mut systems = Systems::new(SystemsConfig::default());
        systems.attach_level(&mut level, config.difficulty, &skill);
        Self {
            level,
            camera,
            controller,
            light_styles: LightStyles::new(),
            renderers: None,
            elapsed: 0.0,
            difficulty: config.difficulty,
            skill,
            titles: TitleLibrary::load(source),
            sentences: SentenceLookup::load(source),
            globals,
            carry: Box::new(DefaultPlayerCarry::default()),
            systems,
            clock: TickClock::new(),
            pending,
        }
    }

    /// The map name this game is currently running.
    #[must_use]
    pub fn map(&self) -> &str {
        &self.level.name
    }

    /// The chapter title `ohl-campaign` resolves for the current map, when
    /// it knows one.
    #[must_use]
    pub fn chapter_title(&self) -> Option<&'static str> {
        ohl_campaign::chapter_of(&self.level.name).map(|chapter| chapter.title)
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

    /// How many individual world/submodel faces this level dropped while
    /// building (see [`ohl_world::WorldModel::dropped_faces`]), summed
    /// across every model the level built. Media-derived: report it as
    /// data, never in a log line. A non-zero count here means some faces —
    /// potentially including geometry that should occlude something else,
    /// e.g. a sky face — did not make it into the drawn mesh.
    #[must_use]
    pub fn dropped_face_count(&self) -> usize {
        self.level.dropped_faces
    }

    /// How many `info_player_start` entities this level declares. Media-
    /// derived: report it as data, never in a log line. A value greater
    /// than `1` is the leading suspect for a level whose spawn faces the
    /// wrong direction, since [`ohl_world::find_player_start`] always picks
    /// the first one in entity-lump order (see its doc comment).
    #[must_use]
    pub fn player_start_count(&self) -> usize {
        self.level.player_start_count
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

    /// How many studio-model placements this level published at load. Every
    /// one of them is an entity carrying a
    /// [`crate::components::StudioAnim`], which is what
    /// [`Self::render`] draws.
    #[must_use]
    pub fn prop_count(&self) -> usize {
        self.level.props.len()
    }

    /// This map's parsed entity definitions, in the order the entity lump
    /// declared them and index-aligned with
    /// [`ohl_game::Registry::entities`].
    ///
    /// Media-derived: the values here are map-authored, so they are handed
    /// back as data and never written to a log line.
    #[must_use]
    pub fn entity_defs(&self) -> &[ohl_game::keyvalues::EntityDef] {
        &self.level.defs
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

    /// Seconds of simulated time since this level was loaded. Also the time
    /// the light-style animation is evaluated at.
    #[must_use]
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// The HUD the host draws this frame.
    ///
    /// It is state, not a stream: the presentation phase rewrites it every
    /// step, so a save/load or a headless replay reproduces it without
    /// having to replay an event log.
    #[must_use]
    pub fn hud(&self) -> &ohl_ui::hud::HudState {
        self.systems.hud()
    }

    /// How many projectiles and placed deployables (satchels, tripmines)
    /// are currently live. See `crate::projectiles`.
    #[must_use]
    pub fn projectile_count(&self) -> usize {
        self.systems.projectile_count()
    }

    /// Whether this frame draws a first-person view model. See
    /// `crate::viewmodel`.
    #[must_use]
    pub fn viewmodel_visible(&self) -> bool {
        self.systems.viewmodel_visible()
    }

    /// Test-only hook: points the view model at `model_slot` (an index into
    /// this level's already-loaded `Level::studio_models`) and pushes one
    /// transient sprite, so a headless render test can compare a frame that
    /// draws them against one that does not, without a weapon-selection
    /// package (P1) landed in this tree to drive either normally.
    #[cfg(any(test, feature = "test-support"))]
    pub fn debug_show_viewmodel_and_sprite(&mut self, model_slot: usize) {
        self.systems.debug_show_viewmodel_and_sprite(model_slot);
    }

    /// The player's weapons, ammo, HEV suit and long-jump ownership.
    ///
    /// A freshly built value each call (see `crate::combat`'s module docs
    /// for why): weapon ownership, clips and selection mirror this game's
    /// long-lived inventory exactly, and every ammo pool is stamped from
    /// the engine's own reserve ledger, which is the only ammo count this
    /// crate ever treats as authoritative.
    #[must_use]
    pub fn inventory(&self) -> ohl_combat::Inventory {
        self.systems.inventory()
    }

    /// The player's current health, from `ohl_player::Player`'s own state
    /// (not the world entity's `ohl_combat::Health` component, which is a
    /// mirror written after damage resolution; see `crate::damage_map`'s
    /// module docs for why the two exist side by side).
    #[must_use]
    pub fn player_health(&self) -> f32 {
        self.systems.player_health()
    }

    /// The player's current HEV armor, from `ohl_player::Player`'s own
    /// state (not the world entity's `ohl_combat::Armor` component; see
    /// [`Self::player_health`]).
    #[must_use]
    pub fn player_armor(&self) -> f32 {
        self.systems.player_armor()
    }

    /// The single client entity, carrying [`crate::components::PlayerTag`].
    ///
    /// It is a real entity in the same world every other entity lives in,
    /// which is what lets a monster target the player through the code path
    /// it uses for anything else, and what gives an attack trace something
    /// to ignore.
    #[must_use]
    pub fn player_entity(&self) -> ohl_game::hecs::Entity {
        self.level.player
    }

    /// How many entities in this level are currently thinking monsters.
    ///
    /// Media-derived: the count comes from the map's own entity list, so it
    /// is returned as data and never written to a log line.
    #[must_use]
    pub fn monster_count(&self) -> usize {
        self.systems.ai().monster_count(&self.level)
    }

    /// A digest of the whole AI simulation — every actor's pose, health and
    /// faction, every monster's state, schedule and route, the sound list
    /// and the random stream.
    ///
    /// Two games built from the same map bytes with the same seed and
    /// ticked with the same input produce the same digest, which is what a
    /// determinism test asserts. Data, never a log line.
    #[must_use]
    pub fn ai_state_hash(&self) -> [u8; 32] {
        self.systems.ai().state_hash(&self.level)
    }

    /// The step list this game runs, mutably, so an in-crate caller can
    /// reach a phase's own state (the AI world, the damage queue).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn systems_mut(&mut self) -> &mut Systems {
        &mut self.systems
    }

    /// How many monsters have died since this level was loaded. Data,
    /// never a log line.
    #[must_use]
    pub fn monster_death_count(&self) -> u64 {
        self.systems.ai().death_count()
    }

    /// How many monster damage events have been applied since this level
    /// was loaded. Counts events, not monsters. Data, never a log line.
    #[must_use]
    pub fn monster_damage_event_count(&self) -> u64 {
        self.systems.ai().damage_event_count()
    }

    /// The per-step configuration this game simulates with.
    #[must_use]
    pub fn systems_config(&self) -> SystemsConfig {
        self.systems.config()
    }

    /// Replaces the per-step configuration, e.g. to run a determinism test
    /// from a chosen seed. Takes effect on the next step.
    pub fn set_systems_config(&mut self, config: SystemsConfig) {
        self.systems.set_config(config);
    }

    /// The campaign difficulty this game runs at.
    #[must_use]
    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// Selects a difficulty; `skill.cfg` lookups follow immediately.
    pub fn set_difficulty(&mut self, difficulty: Difficulty) {
        self.difficulty = difficulty;
    }

    /// The `skill.cfg` table the combat and AI crates read their tuned
    /// values from, keyed by the current [`Self::difficulty`].
    #[must_use]
    pub fn skill_table(&self) -> &SkillTable {
        &self.skill
    }

    /// One `skill.cfg` value at the current difficulty, e.g.
    /// `skill("sk_headcrab_health")`.
    #[must_use]
    pub fn skill(&self, subject_property: &str) -> Option<&str> {
        self.skill.lookup(subject_property, self.difficulty)
    }

    /// The `titles.txt` library backing `env_message` and chapter titles.
    #[must_use]
    pub fn titles(&self) -> &TitleLibrary {
        &self.titles
    }

    /// The `sentences.txt` lookup for HEV/scientist voice lines.
    #[must_use]
    pub fn sentences(&self) -> &SentenceLookup {
        &self.sentences
    }

    /// The `globalname`/`env_global` state table.
    #[must_use]
    pub fn global_state(&self) -> &GlobalStateTable {
        &self.globals
    }

    /// The player-carry hook's current state (health/armor placeholders
    /// until `ohl-player` supplies its own implementation).
    #[must_use]
    pub fn player_carry(&self) -> crate::transition::PlayerCarryState {
        self.carry.capture()
    }

    /// Replaces the player-carry hook, so `ohl-player` can own the player's
    /// state without this crate depending on it.
    pub fn set_player_carry(&mut self, carry: Box<dyn PlayerCarry>) {
        self.carry = carry;
    }

    /// Moves the camera (and the walking player) to an explicit viewpoint,
    /// for headless captures at a chosen position.
    ///
    /// This intentionally enables noclip and leaves it on: a caller-chosen
    /// viewpoint is arbitrary map-relative debug coordinates, not a
    /// documented spawn point, so it is not guaranteed to sit in open
    /// space the way an `info_player_start` is. Ordinary spawn placement
    /// keeps collision active and lets the walking player's normal
    /// resolution push them clear of an accidental overlap on the first
    /// tick; this path has no such recovery by design, since a free-fly
    /// debug/capture camera must be able to move to a coordinate the
    /// mapper never intended a player to occupy (looking from outside a
    /// wall, from inside machinery, etc.) without collision fighting it.
    /// A caller that lands here at a coordinate that turns out to be
    /// inside solid geometry (see [`Self::eye_is_in_solid`]) should treat
    /// that as a sign the requested viewpoint needs adjusting, not as a
    /// bug in this method.
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

    /// Whether the camera's current eye position sits inside solid
    /// collision geometry, checked with the same point-in-hull query the
    /// walking player's [`ohl_physics::PlayerController`] already uses.
    /// `false` when this level has no usable collision hulls (see
    /// [`Self::has_collision`]), since there is then nothing to check the
    /// point against.
    ///
    /// [`Self::set_viewpoint`] runs with noclip on and so cannot recover
    /// from landing inside solid geometry the way ordinary spawn placement
    /// does; a host driving a capture/debug viewpoint should call this
    /// after [`Self::set_viewpoint`] and warn when it returns `true`,
    /// since the resulting frame is not a meaningful capture.
    #[must_use]
    pub fn eye_is_in_solid(&self) -> bool {
        self.level.collision.as_ref().is_some_and(|collision| {
            ohl_physics::contents::is_solid(ohl_physics::point_contents(
                collision,
                Vec3::from_array(self.camera.position),
            ))
        })
    }

    /// Advances the frame by `dt` seconds and returns the events the host
    /// must act on.
    ///
    /// This is a frame loop, not a simulation step: `dt` is clamped, the
    /// view is turned once (aiming is a frame-rate concern, not a
    /// simulation one), and the simulation is then advanced by however many
    /// whole [`crate::tick::TICK_SECONDS`] steps the clock releases. A frame
    /// that releases no step still returns the events queued outside the
    /// loop, and banks its time for the next one.
    pub fn tick(&mut self, dt: f32, input: &Input) -> Vec<GameEvent> {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, MAX_TICK_SECONDS)
        } else {
            0.0
        };

        let (delta_x, delta_y) = input.mouse_delta;
        if delta_x.is_finite() && delta_y.is_finite() && (delta_x != 0.0 || delta_y != 0.0) {
            self.camera.apply_mouse_delta(delta_x, delta_y);
            self.controller
                .apply_mouse_delta(delta_x, delta_y, MOUSE_SENSITIVITY);
        }

        self.systems.begin_frame(input);
        let mut events = Vec::new();
        for _ in 0..self.clock.steps(dt) {
            self.step(&mut events);
        }

        let mut out = std::mem::take(&mut self.pending);
        out.extend(events.into_iter().map(|event| match event {
            Event::LevelChange(change) => GameEvent::LevelChange {
                map: change.map,
                landmark: change.landmark,
            },
            Event::Message(message) => GameEvent::Message {
                block: self.titles.resolve(&message),
            },
        }));
        out.extend(
            self.systems
                .drain_presentation_events()
                .into_iter()
                .map(|event| match event {
                    crate::presentation::PresentationEvent::Sound(cue) => GameEvent::Sound(cue),
                    crate::presentation::PresentationEvent::Suit(suit) => GameEvent::Suit(suit),
                    crate::presentation::PresentationEvent::ViewModel(action) => {
                        GameEvent::ViewModel(action)
                    }
                    crate::presentation::PresentationEvent::PlayerDied => GameEvent::PlayerDied,
                }),
        );
        out
    }

    /// One fixed simulation step. Every subsystem sees the same
    /// [`crate::tick::TICK_SECONDS`], so what the simulation does never
    /// depends on how fast the host renders it.
    fn step(&mut self, events: &mut Vec<Event>) {
        self.systems.step(
            &mut self.level,
            &mut self.camera,
            &mut self.controller,
            TICK_SECONDS,
            events,
        );
        self.elapsed += TICK_SECONDS;
    }

    /// Captures everything that travels through `landmark` out of the
    /// current level: the player's landmark-relative pose and carried
    /// state, the entities inside the landmark's `trigger_transition`
    /// volumes (or within [`crate::DEFAULT_CARRY_RADIUS`] of it when the
    /// map declares none), the global state table, and this level's
    /// modified door/button states.
    #[must_use]
    pub fn capture_transition(&self, landmark: &str) -> TransitionState {
        TransitionState::capture(
            &self.level,
            landmark,
            Vec3::from_array(self.camera.position),
            self.camera.yaw,
            self.camera.pitch,
            // M7.9 P1: `self.systems` is now the real source of the
            // player's health/armor/weapons/ammo/suit/long-jump, not the
            // `self.carry` placeholder `#62` wrote before `ohl-player`
            // existed; see `Systems::capture_carry`'s doc comment.
            self.systems.capture_carry(),
            &self.globals,
        )
    }

    /// Loads `map` and applies `transition` to it, placing the player (and
    /// everything that travelled with them) relative to the destination's
    /// landmark.
    ///
    /// When either map declares no `info_landmark` with this name the
    /// player stays at the destination's own `info_player_start`, and a
    /// carried entity the destination does not already declare is dropped:
    /// neither has a position that means anything in the destination's
    /// coordinates.
    ///
    /// A destination whose `worldspawn` sets `newunit` keeps only the
    /// player's placement and carried state; every carried entity, mover
    /// state and global is dropped, per the documented meaning of that key.
    ///
    /// # Errors
    /// As [`Game::load`]; the current level is left untouched on failure.
    pub fn apply_transition(
        &mut self,
        source: &dyn AssetSource,
        map: &str,
        transition: &TransitionState,
    ) -> Result<()> {
        let mut next = Level::load(source, map)?;
        let newunit = next
            .registry
            .worldspawn
            .as_ref()
            .is_some_and(|worldspawn| worldspawn.newunit);
        let placement = transition.apply(&mut next);

        let mut globals = if newunit {
            GlobalStateTable::new()
        } else {
            transition.globals.clone()
        };
        globals.seed_from(&next.registry);

        self.level = next;
        self.camera = self
            .level
            .spawn
            .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
        self.controller = self
            .level
            .spawn
            .map_or_else(PlayerController::default, |spawn| {
                PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
            });
        self.light_styles = LightStyles::new();
        // The previous level's uploaded geometry is gone with it; the next
        // `render` rebuilds against whatever target it is handed.
        self.renderers = None;
        self.elapsed = 0.0;
        self.clock = TickClock::new();
        self.systems.reset();
        self.systems
            .attach_level(&mut self.level, self.difficulty, &self.skill);
        self.systems.restore_carry(&transition.player);
        self.globals = globals;
        self.carry.restore(&transition.player);

        if let Some(position) = placement {
            self.camera.position = position.to_array();
            self.camera.yaw = transition.yaw;
            self.camera.pitch = transition.pitch;
            self.controller =
                PlayerController::spawn_at(position, transition.yaw, transition.pitch);
        }
        self.pending.extend(chapter_title_event(&self.level.name));
        Ok(())
    }

    /// Loads `map` and places the player relative to `landmark`, carrying
    /// everything [`Self::capture_transition`] finds.
    ///
    /// When either map lacks the landmark the destination's own
    /// `info_player_start` is used instead.
    ///
    /// # Errors
    /// As [`Game::load`]; the current level is left untouched on failure.
    pub fn change_level(
        &mut self,
        source: &dyn AssetSource,
        map: &str,
        landmark: &str,
    ) -> Result<()> {
        let transition = self.capture_transition(landmark);
        self.apply_transition(source, map, &transition)
    }

    /// This game's state as a save payload, stamped with a host-supplied
    /// timestamp (this crate reads no clock).
    #[must_use]
    pub fn to_save(&self, created_at_unix_secs: u64) -> GameSave {
        GameSave {
            created_at_unix_secs,
            header: EngineHeader {
                map: self.level.name.clone(),
                chapter_title: self.chapter_title().map(str::to_string),
                difficulty: self.difficulty.skill_cvar_value(),
                elapsed: self.elapsed,
            },
            view: ViewState {
                position: self.camera.position,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            },
            player: self.systems.capture_carry(),
            entities: self
                .level
                .registry
                .entities
                .iter()
                .map(|entity| EntitySnapshot::capture(&self.level.registry, *entity))
                .collect(),
            simulation: self.level.simulation.snapshot(),
            globals: self.globals.clone(),
            light_style_time: self.elapsed,
        }
    }

    /// Serializes this game into an [`ohl_save`] container.
    ///
    /// # Errors
    /// [`EngineError::SaveUnwritable`] when the container rejects a section.
    pub fn save_bytes(&self, created_at_unix_secs: u64) -> Result<Vec<u8>> {
        self.to_save(created_at_unix_secs).to_bytes()
    }

    /// Writes this game into `slot`'s save directory under `name`
    /// (`ohl_save::AUTOSAVE_SLOT_NAME`, `ohl_save::QUICKSAVE_SLOT_NAME`, or
    /// any name `ohl_save::validate_slot_name` accepts).
    ///
    /// # Errors
    /// [`EngineError::SaveUnwritable`] when the payload could not be built
    /// or the slot could not be written.
    pub fn save_slot(
        &self,
        slot: &ohl_save::SaveSlot,
        name: &str,
        created_at_unix_secs: u64,
    ) -> Result<()> {
        let bytes = self.save_bytes(created_at_unix_secs)?;
        slot.write(name, &bytes)
            .map_err(|_| EngineError::SaveUnwritable)
    }

    /// Rebuilds a game from a save payload: the map named in the save is
    /// loaded through `source`, then every stored section is applied to it.
    ///
    /// # Errors
    /// [`EngineError::MapNotFound`] when the payload no longer publishes
    /// the saved map, else as [`Game::load`].
    pub fn from_save(source: &dyn AssetSource, save: &GameSave) -> Result<Self> {
        let config = GameConfig {
            difficulty: save.difficulty(),
        };
        let mut game = Self::load_with(source, &save.header.map, &config)?;
        game.restore(save);
        Ok(game)
    }

    /// Reads a save container and rebuilds the game it describes.
    ///
    /// # Errors
    /// [`EngineError::SaveUnreadable`] when the container does not open or
    /// a section is missing, else as [`Game::from_save`].
    pub fn load_bytes(source: &dyn AssetSource, bytes: &[u8]) -> Result<Self> {
        Self::from_save(source, &GameSave::from_bytes(bytes)?)
    }

    /// Reads `name` out of `slot`'s save directory and rebuilds the game.
    ///
    /// # Errors
    /// [`EngineError::SaveUnreadable`] when the slot could not be read,
    /// else as [`Game::load_bytes`].
    pub fn load_slot(
        source: &dyn AssetSource,
        slot: &ohl_save::SaveSlot,
        name: &str,
    ) -> Result<Self> {
        let bytes = slot.read(name).map_err(|_| EngineError::SaveUnreadable)?;
        Self::load_bytes(source, &bytes)
    }

    /// Applies a save payload onto this (already map-matched) game.
    fn restore(&mut self, save: &GameSave) {
        for (entity, snapshot) in self
            .level
            .registry
            .entities
            .clone()
            .iter()
            .zip(&save.entities)
        {
            snapshot.apply(&mut self.level.registry, *entity);
        }
        self.level.simulation.restore(&save.simulation);
        self.globals = save.globals.clone();
        self.carry.restore(&save.player);
        self.elapsed = save.header.elapsed;
        self.camera.position = save.view.position;
        self.camera.yaw = save.view.yaw;
        self.camera.pitch = save.view.pitch;
        self.controller = PlayerController::spawn_at(
            Vec3::from_array(save.view.position),
            save.view.yaw,
            save.view.pitch,
        );
        self.difficulty = save.difficulty();
        self.clock = TickClock::new();
        self.systems.reset();
        self.systems
            .attach_level(&mut self.level, self.difficulty, &self.skill);
        // `save.player` genuinely round-trips health, armor, owned
        // weapons, per-weapon clips, reserve ammo, the HEV suit and the
        // long jump module: `GameSave` (and so `PlayerCarryState`,
        // `extra` blob included) is serialized whole into the `ohl-save`
        // container, so a save written by `Self::to_save` already carries
        // `Systems::capture_carry`'s encoding through a real file, not
        // just an in-memory transition. `TODO(P4)`: fold this ad hoc byte
        // encoding into its own `SECTION_INVENTORY` (§6) instead, so a
        // save's inventory section is self-describing independent of
        // `PlayerCarryState`'s shape.
        self.systems.restore_carry(&save.player);
        // A load is a map load: the chapter title is announced again.
        self.pending.clear();
        self.pending.extend(chapter_title_event(&self.level.name));
    }

    /// Draws the current frame into `target`, creating the GPU resources
    /// on first use.
    ///
    /// # Errors
    /// [`crate::EngineError::Renderer`] when a GPU resource for this level
    /// could not be created.
    pub fn render(&mut self, context: &GpuContext, target: RenderTarget<'_>) -> Result<()> {
        if self.renderers.is_none() {
            self.renderers = Some(Renderers::new(context, &self.level, target.format)?);
        }
        let Some(renderers) = self.renderers.as_mut() else {
            return Err(EngineError::Renderer);
        };
        let view_model = crate::viewmodel::build_frame(
            &self.level,
            &self.camera,
            self.systems.config(),
            self.systems.view_model(),
        );
        renderers.draw(
            context,
            &self.level,
            &self.camera,
            &self.light_styles,
            self.elapsed,
            target,
            view_model.as_ref(),
            self.systems.transient_sprites().as_slice(),
        );
        Ok(())
    }
}

/// The chapter-title event for `map`, when `ohl-campaign` knows the chapter
/// that map belongs to.
fn chapter_title_event(map: &str) -> Option<GameEvent> {
    ohl_campaign::chapter_of(map).map(|chapter| GameEvent::ChapterTitle(chapter.title.to_string()))
}
