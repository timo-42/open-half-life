//! The game state and its two verbs: [`Game::tick`] and
//! [`Game::render`](crate::Game::render), plus the campaign flow layered on
//! them: level transitions, save/load, chapter titles and difficulty.

use glam::Vec3;
use ohl_campaign::{Difficulty, SkillTable};
use ohl_game::Event;
use ohl_physics::{MAX_PITCH_DEGREES, PlayerController};
use ohl_render::{FreeFlyCamera, GpuContext, LightStyles};
use ohl_world::LightRamp;

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameConfig {
    /// The campaign difficulty, selecting which `skill.cfg` cvar suffix
    /// [`Game::skill_table`] lookups read.
    pub difficulty: Difficulty,
    /// The lightmap ramp's overbright multiplier (see
    /// `ohl_world::LightRamp::overbright`). Defaults to `1.0`, the
    /// documented `LightRamp` default (no multiplier); a fidelity
    /// investigation (round 4, finding E5) measured public reference
    /// screenshots at roughly 1.7x this project's mean luma at that
    /// default, and found a real, publicly documented GoldSrc/Quake-family
    /// "overbright" lightmap convention that doubles brightness, but no
    /// public source pins a specific default multiplier value for this
    /// project to adopt (GoldSrc's own `gl_overbright` cvar in fact
    /// defaults *off*; see `docs/FORMAT_SOURCES.md`, "Rendering
    /// conventions"). This field exposes the multiplier as a user choice
    /// (`--overbright` in `ohl-app`) instead of hard-coding a fitted value.
    pub overbright: f32,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Medium,
            overbright: LightRamp::default().overbright,
        }
    }
}

impl GameConfig {
    /// The [`LightRamp`] this config selects: the documented defaults with
    /// [`Self::overbright`] substituted for the ramp's own `overbright`.
    #[must_use]
    fn light_ramp(self) -> LightRamp {
        LightRamp {
            overbright: self.overbright,
            ..LightRamp::default()
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
    /// The [`GameConfig::overbright`] this game was loaded with, reapplied
    /// to every subsequent [`Self::apply_transition`] so a level change
    /// does not silently reset it to the ramp default.
    overbright: f32,
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
    /// How many times a `trigger_teleport` has moved the player on this
    /// level; see [`Self::teleport_count`].
    teleports: u64,
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
        Ok(Self::from_level(
            Level::load_with_ramp(source, map, config.light_ramp())?,
            source,
            *config,
        ))
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
        let mut camera = level
            .spawn
            .map_or_else(FreeFlyCamera::default, FreeFlyCamera::at_spawn);
        let mut controller = level.spawn.map_or_else(PlayerController::default, |spawn| {
            PlayerController::spawn_at(Vec3::from_array(spawn.origin), spawn.yaw, spawn.pitch)
        });
        // A map may spawn the player standing *inside* one of its movers —
        // a campaign opening that starts them aboard a `func_tracktrain`
        // is authored exactly that way. Without the settle the passenger
        // is embedded in the car rather than standing in it, which is not
        // merely cosmetic: an embedded hull has no ground brush, so the
        // ride never starts, and no trace out of solid succeeds, so they
        // cannot fall free either. See `ohl_physics::settle_at_spawn`.
        //
        // Only for an `info_player_start` placement, which is why this is
        // gated on the spawn point and not merely on the map having
        // collision at all: a map with no spawn leaves the controller at
        // `PlayerController::default`'s world origin, a placement no map
        // authored and nothing should nudge. Every mover's hull is already
        // where the map logic puts it — `Level`'s own
        // `attach_brush_collision_with` attaches each brush at
        // `origin + ohl_game::pose::brush_offset(..)` while the level
        // loads, and nothing has moved a `Transform` since — so there is
        // nothing to re-pose here first. (The transition path's own
        // `sync_brush_collision(0.0)` *is* load-bearing, for the opposite
        // reason: there the carry has just moved movers after attach.)
        if level.spawn.is_some()
            && let Some(collision) = level.collision.as_ref()
        {
            controller.settle_at_spawn(collision);
            camera.position = controller.eye_position().to_array();
        }
        let mut globals = GlobalStateTable::new();
        globals.seed_from(&level.registry);
        let pending = chapter_title_event(&level.name).into_iter().collect();
        let skill = load_skill_table(source);
        let sentences = SentenceLookup::load(source);
        let mut systems = Systems::new(SystemsConfig::default());
        systems.ai_mut().set_sentence_lookup(sentences.clone());
        systems.attach_level(&mut level, config.difficulty, &skill);
        Self {
            level,
            camera,
            controller,
            light_styles: LightStyles::new(),
            renderers: None,
            elapsed: 0.0,
            difficulty: config.difficulty,
            overbright: config.overbright,
            skill,
            titles: TitleLibrary::load(source),
            sentences,
            globals,
            carry: Box::new(DefaultPlayerCarry::default()),
            systems,
            clock: TickClock::new(),
            pending,
            teleports: 0,
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

    /// Whether this level resolved a player start to spawn at. `false`
    /// means the player was placed at the world origin instead, which is
    /// very likely inside solid geometry: a map that reports `false` here
    /// did not load the entity world it was supposed to.
    #[must_use]
    pub fn has_player_start(&self) -> bool {
        self.level.spawn.is_some()
    }

    /// Whether this level declares an `info_landmark` (see
    /// [`crate::Level::has_landmark`]), i.e. whether it is a map a
    /// transition can arrive into without a player start of its own.
    #[must_use]
    pub fn has_landmark(&self) -> bool {
        self.level.has_landmark()
    }

    /// How many entity definitions this level's entities lump declared. A
    /// zero here means the level is an empty room. Media-derived only in
    /// the aggregate; safe to report as a count.
    #[must_use]
    pub fn entity_def_count(&self) -> usize {
        self.level.defs.len()
    }

    /// How many of this level's entity-lump strings had to be decoded
    /// byte-per-byte because they were not valid UTF-8 (see
    /// [`crate::Level::entity_lump_relaxed_strings`]).
    #[must_use]
    pub fn entity_lump_relaxed_strings(&self) -> usize {
        self.level.entity_lump_relaxed_strings
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

    /// As [`Self::registry`], mutably, so a test can put an entity into a
    /// world state the ordinary spawn path never produces (an inert actor
    /// with no `MonsterAi`, say) without a second, parallel construction
    /// path just for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn registry_mut(&mut self) -> &mut ohl_game::Registry {
        &mut self.level.registry
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

    /// Test-only hook: spawns a live projectile directly, the same call
    /// `crate::ai`'s own monster ranged-attack mapping makes through
    /// `Systems::spawn_projectile` (`docs/m79-design.md` §8 P3's seam).
    /// Used by the `SECTION_PROJECTILES` (M7.9 P4b) save round-trip tests,
    /// since nothing in this tree yet drives a monster's projectile attack
    /// or a weapon's `WeaponAction::SpawnProjectile` end to end (a known
    /// gap between P1/P2/P3's independent packages, out of this section's
    /// scope).
    #[cfg(any(test, feature = "test-support"))]
    pub fn debug_spawn_projectile(
        &mut self,
        kind: ohl_combat::ProjectileKind,
        origin: [f32; 3],
        velocity: [f32; 3],
    ) -> Option<ohl_combat::ProjectileId> {
        self.systems.spawn_projectile(
            &mut self.level,
            kind,
            None,
            Vec3::from_array(origin),
            Vec3::from_array(velocity),
        )
    }

    /// Test-only hook: places a satchel directly, bypassing the weapon
    /// wiring that does not exist yet for it (see
    /// [`Self::debug_spawn_projectile`]'s doc for why this crate needs
    /// these hooks at all). Used by the `SECTION_PROJECTILES` stand-in
    /// restore tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn debug_place_satchel(&mut self, position: [f32; 3]) -> Option<ohl_combat::DeployableId> {
        self.systems
            .debug_place_satchel(&mut self.level, Vec3::from_array(position))
    }

    /// As [`Self::debug_place_satchel`], for a tripmine.
    #[cfg(any(test, feature = "test-support"))]
    pub fn debug_place_tripmine(
        &mut self,
        from: [f32; 3],
        direction: [f32; 3],
    ) -> Option<ohl_combat::DeployableId> {
        self.systems.debug_place_tripmine(
            &mut self.level,
            Vec3::from_array(from),
            Vec3::from_array(direction),
        )
    }

    /// How many placed deployables currently have a model-backed stand-in
    /// entity (drawn, and damageable). See
    /// `crate::projectiles::ProjectileSystem::deployable_stand_in_count`.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn deployable_stand_in_count(&self) -> usize {
        self.systems.debug_deployable_stand_in_count()
    }

    /// How many satchels/tripmines are currently placed, model or not — the
    /// other half of the equality [`Self::deployable_stand_in_count`]
    /// documents.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn deployable_count(&self) -> usize {
        self.systems.debug_deployable_count()
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

    /// Applies a `--start-inventory` list (dev-tools only; see
    /// `crate::start_inventory`), meant to be called exactly once, right
    /// after [`Self::load`]/[`Self::load_with`], so a single-map probe or
    /// scenario can model the inventory a real campaign run would have
    /// carried in from an earlier map.
    ///
    /// Each item is applied through the same grant path a `weapon_*`/
    /// `ammo_*` pickup touch already uses
    /// (`ohl_combat::Inventory::give_weapon`/`give_ammo`,
    /// `ohl_combat::weapon_pickup_ammo`/`ammo_pickup_amount`): a weapon
    /// unlocks the slot and grants its bundled ammo, an ammo entry tops up
    /// one pickup's worth of that ammo type, clamped to its carry cap the
    /// same way a real pickup already clamps. This never touches the save
    /// format (inventory is save tag 23; nothing here changes what that
    /// tag records or how it is restored) — it is ordinary runtime state,
    /// applied through the ordinary inventory API.
    pub fn give_start_inventory(&mut self, items: &[crate::StartInventoryItem]) {
        let (inventory, ammo) = self.systems.inventory_and_ammo_mut();
        for item in items {
            match *item {
                crate::StartInventoryItem::Weapon(id) => {
                    inventory.give_weapon(id);
                    if let Some(ammo_kind) = ohl_combat::spec(id).ammo {
                        ammo.add(ammo_kind, ohl_combat::weapon_pickup_ammo(id).value);
                    }
                }
                crate::StartInventoryItem::Ammo(kind) => {
                    ammo.add(kind, ohl_combat::ammo_pickup_amount(kind).value);
                }
            }
        }
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

    /// The eye position of whichever spawned, living monster sits closest
    /// to `from`, or `None` when this level has none.
    ///
    /// Additive and data-only, like [`Self::monster_count`]: a caller (a
    /// headless capture viewpoint, say) may place a camera from this, but
    /// this method itself never logs a position or classname. See
    /// [`crate::ai::AiState::nearest_monster_position`].
    #[must_use]
    pub fn nearest_monster_position(&self, from: Vec3) -> Option<Vec3> {
        self.systems
            .ai()
            .nearest_monster_position(&self.level, from)
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

    /// As [`Self::systems_mut`], but also hands back the level: a test that
    /// needs to place a projectile or a deployable directly (bypassing the
    /// weapon wiring that does not exist yet) needs both at once, and
    /// `&mut self` cannot be borrowed twice to get them separately. Only
    /// this crate's own unit tests use it today (unlike
    /// [`Self::systems_mut`], nothing in `crate::test_support` calls it),
    /// so it is gated on `cfg(test)` alone rather than also on
    /// `feature = "test-support"`.
    #[cfg(test)]
    pub(crate) fn level_and_systems_mut(&mut self) -> (&mut Level, &mut Systems) {
        (&mut self.level, &mut self.systems)
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

    /// Whether a `trigger_camera` sequence is currently overriding the
    /// player's view (see `crate::camera`). Data, never a log line; used by
    /// `ohl-app`'s `--script-log` milestone lines the same way
    /// [`Self::active_script_count`] already is.
    #[must_use]
    pub fn camera_sequence_active(&self) -> bool {
        self.level
            .registry
            .world
            .query::<&ohl_game::TriggerCameraState>()
            .iter()
            .any(ohl_game::TriggerCameraState::is_active)
    }

    /// How many `scripted_sequence`/`aiscripted_sequence` entities are
    /// currently possessing a monster. Data, never a log line.
    #[must_use]
    pub fn active_script_count(&self) -> usize {
        self.systems.ai().active_script_count()
    }

    /// How many scripted sequences have started since this level was
    /// loaded. Data, never a log line.
    #[must_use]
    pub fn script_start_count(&self) -> u64 {
        self.systems.ai().script_start_count()
    }

    /// How many scripted sequences have finished their action animation
    /// since this level was loaded. Data, never a log line.
    #[must_use]
    pub fn script_completion_count(&self) -> u64 {
        self.systems.ai().script_completion_count()
    }

    /// How many scripted sequences have given up on ever satisfying their
    /// mark condition and released their monster, since this level was
    /// loaded. See `ohl_ai::scripts::SCRIPT_MOVE_TIMEOUT_SECONDS`'s doc
    /// comment. Data, never a log line.
    #[must_use]
    pub fn script_timeout_count(&self) -> u64 {
        self.systems.ai().script_timeout_count()
    }

    /// The allies currently following the player, oldest first. Data, never
    /// a log line.
    #[must_use]
    pub fn followers(&self) -> &[ohl_game::hecs::Entity] {
        self.systems.ai().followers().members()
    }

    /// How many times the player has actually fired a weapon since this
    /// level was loaded. Data, never a log line.
    #[must_use]
    pub fn weapon_fired_count(&self) -> u64 {
        self.systems.weapon_fired_count()
    }

    /// How many of those shots have landed on an entity. Data, never a log
    /// line.
    #[must_use]
    pub fn shot_hit_count(&self) -> u64 {
        self.systems.shot_hit_count()
    }

    /// How many pickups have actually been taken since this level was
    /// loaded. Data, never a log line.
    #[must_use]
    pub fn pickup_count(&self) -> u64 {
        self.systems.pickup_count()
    }

    /// How many `trigger_*` volumes have fired because the player's own
    /// hull walked into them since this level was loaded (a `use` press or
    /// another entity's fire chain reaching the same volume by name does
    /// not count; see `ohl_game::logic::Simulation::touch_triggers`).
    ///
    /// This is the end-to-end evidence that the player is somewhere a
    /// map's own touch volumes can reach at all — which a player sealed
    /// into the wrong place by a misplaced brush mover never is. Data,
    /// never a log line.
    #[must_use]
    pub fn touch_trigger_count(&self) -> u64 {
        self.systems.touch_trigger_count()
    }

    /// How many closed doors have been opened since this level was loaded,
    /// whether by a `use` press (the proximity path —
    /// `ohl_game::pose::brush_center` placing the door,
    /// [`crate::USE_RADIUS`] reaching it, the map logic simulation acting
    /// on it) or by the player's own hull touching one without the "Use
    /// Only" spawnflag (`docs/FORMAT_SOURCES.md` item 30). Data, never a
    /// log line.
    #[must_use]
    pub fn doors_opened_count(&self) -> u64 {
        self.systems.doors_opened_count()
    }

    /// How many times damage aimed at the player has actually been applied
    /// since this level was loaded. Data, never a log line.
    #[must_use]
    pub fn player_damage_event_count(&self) -> u64 {
        self.systems.player_damage_event_count()
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

    /// The lightmap ramp's overbright multiplier this game was loaded with
    /// (see [`GameConfig::overbright`]). A display setting, not save state:
    /// it is not stored in [`GameSave`] and is not restored by
    /// [`Self::from_save`]/[`Self::load_bytes`]/[`Self::load_slot`] unless
    /// their `_with` variant is given an explicit [`GameConfig`].
    #[must_use]
    pub fn overbright(&self) -> f32 {
        self.overbright
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
        self.position_is_in_solid(self.camera.position)
    }

    /// As [`Self::eye_is_in_solid`], but against an arbitrary world-space
    /// point instead of the camera's own tracked position — for a host
    /// that renders from a position it computed itself (see
    /// [`Self::render_from`]) rather than one it handed to
    /// [`Self::set_viewpoint`].
    #[must_use]
    pub fn position_is_in_solid(&self, position: [f32; 3]) -> bool {
        self.level.collision.as_ref().is_some_and(|collision| {
            ohl_physics::contents::is_solid(ohl_physics::point_contents(
                collision,
                Vec3::from_array(position),
            ))
        })
    }

    /// The live collision model, for a host that needs to trace against it
    /// directly (`crate::reachability`'s route-triage walk) rather than
    /// through the player-move step. `None` when this level has no usable
    /// collision hulls (see [`Self::has_collision`]).
    #[must_use]
    pub fn collision(&self) -> Option<&ohl_physics::CollisionModel> {
        self.level.collision.as_ref()
    }

    /// As [`Self::collision`], mutably: `crate::reachability`'s iterative
    /// walk detaches a door's brush (`CollisionModel::detach_brush`) to
    /// simulate it having been opened, between rounds.
    pub fn collision_mut(&mut self) -> Option<&mut ohl_physics::CollisionModel> {
        self.level.collision.as_mut()
    }

    /// Which attached brush hull belongs to which entity, so a caller that
    /// gets a [`ohl_physics::BrushId`] back from a
    /// [`ohl_physics::CollisionModel::trace`] (via
    /// [`ohl_physics::Trace::brush_index`]) can look up the entity — and,
    /// through [`Self::registry`], its classname — that blocked it.
    #[must_use]
    pub fn brush_collision(&self) -> &[(ohl_game::hecs::Entity, ohl_physics::BrushId)] {
        &self.level.brush_collision
    }

    /// The live *monster-side* collision model (M9.11,
    /// `docs/FORMAT_SOURCES.md` item 33): identical to [`Self::collision`]
    /// except `func_monsterclip` is attached as solid, which
    /// [`Self::collision`] never attaches at all. `ai.rs` reads this one
    /// for every AI-side trace; nothing else in this crate does. `None`
    /// exactly when [`Self::collision`] is (see [`Self::has_collision`]).
    #[must_use]
    pub fn monster_collision(&self) -> Option<&ohl_physics::CollisionModel> {
        self.level.monster_collision.as_ref()
    }

    /// As [`Self::monster_collision`], mutably: for a test or dev tool
    /// that needs to mutate the monster model directly (a door detach that
    /// should also apply on the monster side, for example) rather than
    /// through [`crate::Level::sync_monster_brush_collision`].
    pub fn monster_collision_mut(&mut self) -> Option<&mut ohl_physics::CollisionModel> {
        self.level.monster_collision.as_mut()
    }

    /// As [`Self::brush_collision`], but for [`Self::monster_collision`]:
    /// a superset (every `func_monsterclip` entity is attached here and
    /// not in [`Self::brush_collision`]).
    #[must_use]
    pub fn monster_brush_collision(&self) -> &[(ohl_game::hecs::Entity, ohl_physics::BrushId)] {
        &self.level.monster_brush_collision
    }

    /// The walking player's own hull-space origin (36 units above the
    /// floor it stands on for the standing hull), as
    /// [`ohl_physics::PlayerController`] tracks it — distinct from
    /// [`Self::eye_position`], which adds the eye-height offset on top.
    /// `crate::reachability`'s walk starts here, since it traces with the
    /// same [`ohl_physics::Hull::Standing`] this origin is defined
    /// relative to.
    #[must_use]
    pub fn player_origin(&self) -> [f32; 3] {
        self.controller.state.origin.to_array()
    }

    /// The tunables the walking player's own [`ohl_physics::PlayerController`]
    /// moves with — `crate::reachability`'s walk derives its jump-height and
    /// jump-distance bounds from this rather than restating either figure,
    /// so both stay in lock-step with whatever this build's `PlayerController`
    /// actually simulates.
    #[must_use]
    pub fn move_config(&self) -> &ohl_physics::MoveConfig {
        &self.controller.config
    }

    /// The speed of the ride the player is currently getting from whatever
    /// they are standing on (a moving `func_train`/`func_tracktrain`/
    /// `func_plat`/lift `func_door`, or a rotating `func_rotating`/
    /// `func_door_rotating` measured at the player's own feet), or `0.0`
    /// while airborne, standing on worldspawn geometry, or standing on a
    /// mover that is not currently moving.
    ///
    /// This is the brush's whole-body translation ([`crate::level::Level::brush_velocity`])
    /// plus, for a rotating mover, the *actual chord* the rider's own seat
    /// was carried through this tick ([`crate::level::Level::rotational_carry`])
    /// divided by `dt` — not the instantaneous tangential rate `omega x r`
    /// a spin's angle-per-tick would suggest at the limit of a vanishingly
    /// small step. The two agree to within a rounding error for an
    /// ordinarily slow `func_rotating`/`func_door_rotating` turn
    /// (`chord ~= r * dtheta` for small `dtheta`), so this reads the same
    /// either way there; it matters for a `func_tracktrain`, whose hull
    /// used to turn by the *whole* angle between two path segments in the
    /// single tick it reached the node between them
    /// (`ohl_game::track_train::TrackTrainState::yaw_degrees`'s blend, see
    /// its own doc comment, is what actually spreads that turn over
    /// several ticks now). At that size of a heading change the chord runs
    /// meaningfully short of the arc, so this method reading the rigid
    /// step's own chord rather than re-deriving an arc-rate from the same
    /// angle (as it used to, e.g. `docs/MILESTONES.md`'s recorded 1.5k-5.8k
    /// u/s spikes, "the yaw-snap ride-speed spike, resolved") reports what
    /// the rider was actually carried by rather than an overestimate of
    /// it, on top of the blend making that carry small at every tick to
    /// begin with. See `crates/ohl-engine/tests/track_train_bend.rs`'s
    /// `a_riders_reported_speed_never_exceeds_the_cars_own_by_more_than_a_small_bound`
    /// for the regression this pair of changes closed.
    #[must_use]
    pub fn ground_mover_speed(&self) -> f32 {
        self.controller.state.ground_brush.map_or(0.0, |brush| {
            let translation = self
                .level
                .brush_velocity
                .get(&brush)
                .copied()
                .unwrap_or(Vec3::ZERO);
            let origin = self.controller.state.origin;
            let rotational = self
                .level
                .rotational_carry(brush, origin, TICK_SECONDS)
                .map_or(Vec3::ZERO, |carried| (carried - origin) / TICK_SECONDS);
            (translation + rotational).length()
        })
    }

    /// Whether the walking player is currently attached to a ladder
    /// (`ohl_physics::PlayerState::on_ladder`) — the same state
    /// `ohl-physics`'s ladder-climb step reads and sets, and the same one
    /// a real `func_ladder`/world `CONTENTS_LADDER` volume both attach
    /// through (`docs/FORMAT_SOURCES.md`, "Player systems").
    #[must_use]
    pub fn player_on_ladder(&self) -> bool {
        self.controller.state.on_ladder
    }

    /// Whether the walking player is standing on a floor
    /// (`ohl_physics::PlayerState::on_ground`), rather than falling,
    /// jumping or riding a mover's edge in mid-air.
    ///
    /// `crate::route_plan` plans from a *standing* position — its walk
    /// starts by settling onto the floor beneath the point it is given —
    /// so a caller planning one route after another asks this before it
    /// plans again: a plan made while the player is still falling is
    /// planned from a floor they have not reached yet.
    #[must_use]
    pub fn player_on_ground(&self) -> bool {
        self.controller.state.on_ground
    }

    /// Whether the walking player is at all submerged in a liquid: any
    /// [`ohl_physics::WaterLevel`] above [`ohl_physics::WaterLevel::Dry`]
    /// (feet, waist or eyes), the same state a real `func_water`/world
    /// liquid volume both categorise through.
    #[must_use]
    pub fn player_in_water(&self) -> bool {
        self.controller.state.water_level != ohl_physics::WaterLevel::Dry
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
        out.extend(events.into_iter().filter_map(|event| match event {
            Event::LevelChange(change) => Some(GameEvent::LevelChange {
                map: change.map,
                landmark: change.landmark,
            }),
            Event::Message(message) => Some(GameEvent::Message {
                block: self.titles.resolve(&message),
            }),
            // Already applied to the player in `Self::apply_teleports`, and
            // removed from the queue there; this arm only exists for a
            // teleport queued outside a fixed step, which nothing does.
            Event::Teleport(_) => None,
        }));
        out.extend(
            self.systems
                .ai_mut()
                .drain_sound_cues()
                .into_iter()
                .map(GameEvent::Sound),
        );
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
        let before = events.len();
        self.systems.step(
            &mut self.level,
            &mut self.camera,
            &mut self.controller,
            TICK_SECONDS,
            events,
        );
        self.apply_teleports(events, before);
        self.elapsed += TICK_SECONDS;
    }

    /// Applies (and removes) every [`Event::Teleport`] this step produced,
    /// so the next step already simulates the player from their new
    /// position rather than the host seeing a teleport it has no way to
    /// act on.
    ///
    /// Only the *last* teleport of a step actually places the player: two
    /// destinations activated in one fixed step are two positions for one
    /// player, and the later one wins for the same reason the later of two
    /// same-tick moves does. Placement matches this crate's own spawn
    /// convention exactly (`Game::from_level`): the destination's `origin`
    /// becomes the player's entity origin and the camera sits
    /// [`ohl_render::FreeFlyCamera::at_spawn`]'s documented standing view
    /// offset above it. Velocity is cleared — a teleported player arrives,
    /// they do not carry the speed they walked into the volume with — and
    /// collision stays on, unlike [`Self::set_viewpoint`]'s free camera,
    /// because a destination is a mapper-placed point a player is meant to
    /// stand at.
    fn apply_teleports(&mut self, events: &mut Vec<Event>, from: usize) {
        let mut destination = None;
        let mut index = from;
        while index < events.len() {
            if let Event::Teleport(teleport) = events[index] {
                destination = Some(teleport);
                events.remove(index);
            } else {
                index += 1;
            }
        }
        let Some(teleport) = destination else {
            return;
        };
        let origin = Vec3::from_array(teleport.origin);
        if !origin.is_finite() {
            return;
        }
        let (pitch, yaw) = (teleport.angles[0], teleport.angles[1]);
        let (pitch, yaw) = if pitch.is_finite() && yaw.is_finite() {
            (pitch, yaw)
        } else {
            (self.controller.pitch, self.controller.yaw)
        };
        self.controller.state.origin = origin;
        self.controller.state.velocity = Vec3::ZERO;
        self.controller.yaw = yaw;
        self.controller.pitch = pitch.clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
        self.camera.position = self.controller.eye_position().to_array();
        self.camera.yaw = self.controller.yaw;
        self.camera.pitch = self.controller.pitch;
        self.teleports = self.teleports.saturating_add(1);
    }

    /// How many times a `trigger_teleport` volume has moved the player
    /// since this level was loaded (see
    /// [`ohl_game::registry::TeleportDestination`]). Data, never a log
    /// line.
    #[must_use]
    pub fn teleport_count(&self) -> u64 {
        self.teleports
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
            // The player's own hull origin, not the eye: the documented
            // rule places the player in the destination at the offset from
            // the landmark they had in the source map, and
            // `apply_transition` applies that offset to the destination
            // player's *origin*. Measuring it from the eye instead raised
            // the player by the standing view offset on every level change
            // — enough to drop someone arriving on a moving mover out of it
            // before they landed.
            Vec3::from_array(self.player_origin()),
            self.camera.yaw,
            self.camera.pitch,
            // M7.9 P1: `self.systems` is now the real source of the
            // player's health/armor/weapons/ammo/suit/long-jump, not the
            // `self.carry` placeholder `#62` wrote before `ohl-player`
            // existed; see `Systems::capture_carry`'s doc comment.
            self.systems.capture_carry(),
            &self.globals,
            self.ridden_entity(),
        )
    }

    /// The brush entity the walking player is standing on right now, or
    /// `None` while airborne or standing on worldspawn geometry.
    ///
    /// Exactly the lookup [`Self::ground_mover_speed`] keys off — the
    /// physics step's own [`ohl_physics::PlayerState::ground_brush`],
    /// resolved back to its registry entity through
    /// [`crate::level::Level::brush_collision`] — so "the player is riding
    /// this mover" means the same thing here as it does to the ride
    /// velocity they are actually being carried at.
    fn ridden_entity(&self) -> Option<ohl_game::hecs::Entity> {
        let brush = self.controller.state.ground_brush?;
        self.level
            .brush_collision
            .iter()
            .find(|(_, id)| *id == brush)
            .map(|(entity, _)| *entity)
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
        let ramp = LightRamp {
            overbright: self.overbright,
            ..LightRamp::default()
        };
        let mut next = Level::load_with_ramp(source, map, ramp)?;
        let newunit = next
            .registry
            .worldspawn
            .as_ref()
            .is_some_and(|worldspawn| worldspawn.newunit);
        let placement = transition.apply(&mut next);
        // `transition.apply` -> `crate::transition::materialise_carried`
        // just appended whatever it carried past `next.map_defs` (the
        // field never moves once a level is built); give exactly that
        // range the studio-model load/attach pass `Level::load_with_ramp`
        // above already ran for the destination's own defs, or a carried
        // monster stays simulated but invisible — see
        // `Level::attach_studio_models`'s doc comment.
        next.attach_studio_models(source, next.map_defs);
        // Re-baseline the collision model against whatever the transition
        // just moved (a carried `func_tracktrain` is placed where the
        // source map's copy was, thousands of units from where this map
        // spawned it). A zero `dt` records the new positions with no
        // velocity, so the first step does not read that placement as one
        // step's worth of motion and hand a rider a base velocity of
        // thousands of units per second. The save-restore path
        // (`Self::restore`, at the end of this file) makes the
        // identical zero-`dt` call for the identical reason — a restored
        // mover's `Transform` is just as far from where a fresh `Level`
        // attached its hull — and `crates/ohl-engine/tests/
        // train_across_level_change.rs` pins this one so it cannot go
        // missing unnoticed.
        next.sync_brush_collision(0.0);

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
        self.teleports = 0;
        self.clock = TickClock::new();
        self.systems.reset();
        self.systems
            .attach_level(&mut self.level, self.difficulty, &self.skill);
        self.systems.restore_carry(&transition.player);
        self.globals = globals;
        self.carry.restore(&transition.player);

        if let Some(position) = placement {
            // `position` is a player *origin* (see `capture_transition`), so
            // the camera is placed above it the same way an
            // `info_player_start` spawn places it.
            self.camera = FreeFlyCamera::at_spawn(ohl_world::PlayerSpawn {
                origin: position.to_array(),
                yaw: transition.yaw,
                pitch: transition.pitch,
            });
            self.controller =
                PlayerController::spawn_at(position, transition.yaw, transition.pitch);
            // A landmark-relative arrival stays a pure offset — except when
            // the offset put the player *inside* solid, which is not an
            // offset the physics state can carry across at all: no ground
            // brush resolves while `start_solid` holds, and no traced move
            // out of solid succeeds, so they are frozen where they landed.
            // `ohl_physics::settle_if_embedded` runs the same bounded
            // upward nudge a spawn (and a landing) already uses, and only
            // when that test fails, so every boundary whose two maps line
            // up is left bit-for-bit where the offset put it.
            if let Some(collision) = self.level.collision.as_ref()
                && self.controller.settle_if_embedded(collision)
            {
                self.camera.position = self.controller.eye_position().to_array();
            }
        } else if self.level.spawn.is_some()
            && let Some(collision) = self.level.collision.as_ref()
        {
            // No landmark to place against, so this is the destination's
            // own `info_player_start` and nothing else — exactly the
            // placement `Self::from_level` makes, and it gets exactly the
            // same spawn settle (`ohl_physics::settle_at_spawn`), against
            // hulls the `sync_brush_collision(0.0)` above has already
            // re-posed after the carry moved them. Gated on the spawn
            // point for the same reason `from_level` is: with no
            // `info_player_start` the controller is
            // `PlayerController::default`'s world origin, which no map
            // authored. A *landmark-relative* arrival deliberately gets no
            // settle either: that placement is a pure offset from where
            // the player stood in the source map, and nudging it would
            // make a boundary stop being a no-op for the physics state.
            self.controller.settle_at_spawn(collision);
            self.camera.position = self.controller.eye_position().to_array();
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
                // The player's feet-level physics origin, not
                // `self.camera.position` (the *eye* position, `origin +`
                // the stance's eye offset): `Self::restore` reconstructs a
                // fresh `PlayerController` from this value via `spawn_at`,
                // which treats its argument as the origin. Saving the eye
                // position there used to teleport the reloaded player
                // upward by the eye offset, only settling back down (via a
                // real, physics-simulated fall) once ticking resumed —
                // harmless for the byte-identical save round trip (nothing
                // ticks in between), but it meant a save/load boundary was
                // never actually a no-op for the physics state, which
                // broke exact continuation (`Game::ai_state_hash` includes
                // the player's own `Actor`, moved by this same physics).
                // A map with no collision hulls has no `PlayerController`
                // origin/eye distinction at all, so it keeps using the
                // free-fly camera's own position.
                position: if self.level.collision.is_some() {
                    self.controller.state.origin.to_array()
                } else {
                    self.camera.position
                },
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
            inventory: Some(self.systems.snapshot_inventory()),
            entity_combat: Some(Systems::snapshot_entity_combat(&self.level)),
            ai: Some(Systems::snapshot_ai(&self.level)),
            projectiles: Some(self.systems.snapshot_projectiles(&self.level)),
            rng: Some(self.systems.snapshot_rng()),
            mover_state: Some(self.systems.snapshot_mover_state(&self.level)),
            maker_children: Some(crate::save_state::snapshot_maker_children(&self.level)),
            rotating_movers: Some(crate::save::RotatingMoverStateSnapshot {
                movers: crate::save_state::snapshot_rotating_movers(&self.level),
                rot_button_touch: self.level.simulation.rot_button_touch_snapshot(),
            }),
            momentary_doors: Some(crate::save_state::snapshot_momentary_doors(&self.level)),
            breakables: Some(crate::save_state::snapshot_breakables(&self.level)),
            train_handover_yaw: Some(crate::save_state::snapshot_train_handover_yaw(&self.level)),
            // Written only when a level change actually materialised
            // something here, so a cold-loaded map's save carries no
            // section at all rather than an empty one.
            carried_entities: {
                let defs = crate::save_state::snapshot_carried_entities(&self.level);
                (!defs.is_empty()).then_some(defs)
            },
            teleport_state: Some({
                let state = self.level.simulation.teleport_state_snapshot();
                crate::save::TeleportStateSnapshot {
                    teleport_touch: state.teleport_touching,
                    master_fires: state.master_fires,
                }
            }),
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
    /// `difficulty` always comes from `save` (it is save state); the
    /// lightmap ramp's `overbright` multiplier is a display setting instead
    /// (`GameSave` carries no such field), so this keeps it at
    /// [`GameConfig::default`]'s value. Use [`Self::from_save_with`] to
    /// override it, for example to carry a `--overbright` command-line
    /// choice across a save/load.
    ///
    /// # Errors
    /// [`EngineError::MapNotFound`] when the payload no longer publishes
    /// the saved map, else as [`Game::load`].
    pub fn from_save(source: &dyn AssetSource, save: &GameSave) -> Result<Self> {
        Self::from_save_with(source, save, &GameConfig::default())
    }

    /// As [`Self::from_save`], with a caller-chosen [`GameConfig`] for
    /// settings that are not save state. Only [`GameConfig::overbright`] is
    /// read; `config.difficulty` is ignored in favour of `save`'s own
    /// difficulty, which is always save state.
    ///
    /// # Errors
    /// As [`Self::from_save`].
    pub fn from_save_with(
        source: &dyn AssetSource,
        save: &GameSave,
        config: &GameConfig,
    ) -> Result<Self> {
        let config = GameConfig {
            difficulty: save.difficulty(),
            overbright: config.overbright,
        };
        // The level is built, then every entity a level change materialised
        // in it is rebuilt on top (tag 36), and only then does the rest of
        // the game attach: `Systems::attach_level` is what gives a carried
        // monster its brain, its `Actor` and its place in this map's own
        // `scripted_sequence` bookkeeping, and it reads the definition list
        // those entities were just appended to. Doing it after the attach
        // would restore the entity and leave it inert, which is the husk
        // this section exists to stop being.
        let mut level = Level::load_with_ramp(source, &save.header.map, config.light_ramp())?;
        if let Some(carried) = save.carried_entities.as_deref() {
            crate::save_state::restore_carried_entities(&mut level, carried);
            // As `Self::apply_transition` does after a live level change:
            // `restore_carried_entities` -> `materialise_carried` appended
            // past `level.map_defs`, which the initial `load_with_ramp`
            // above never saw. Without this a monster restored from a
            // tag-36 save keeps its `Actor`/brain (`Self::from_level`'s own
            // `attach_level` call, below, re-runs over the whole restored
            // `defs`) but never gets a `StudioAnim` back.
            level.attach_studio_models(source, level.map_defs);
        }
        let mut game = Self::from_level(level, source, config);
        game.restore(save);
        Ok(game)
    }

    /// Reads a save container and rebuilds the game it describes.
    ///
    /// # Errors
    /// [`EngineError::SaveUnreadable`] when the container does not open or
    /// a section is missing, else as [`Game::from_save`].
    pub fn load_bytes(source: &dyn AssetSource, bytes: &[u8]) -> Result<Self> {
        Self::load_bytes_with(source, bytes, &GameConfig::default())
    }

    /// As [`Self::load_bytes`], with a caller-chosen [`GameConfig`]; see
    /// [`Self::from_save_with`].
    ///
    /// # Errors
    /// As [`Self::load_bytes`].
    pub fn load_bytes_with(
        source: &dyn AssetSource,
        bytes: &[u8],
        config: &GameConfig,
    ) -> Result<Self> {
        Self::from_save_with(source, &GameSave::from_bytes(bytes)?, config)
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
        Self::load_slot_with(source, slot, name, &GameConfig::default())
    }

    /// As [`Self::load_slot`], with a caller-chosen [`GameConfig`]; see
    /// [`Self::from_save_with`].
    ///
    /// # Errors
    /// As [`Self::load_slot`].
    pub fn load_slot_with(
        source: &dyn AssetSource,
        slot: &ohl_save::SaveSlot,
        name: &str,
        config: &GameConfig,
    ) -> Result<Self> {
        let bytes = slot.read(name).map_err(|_| EngineError::SaveUnreadable)?;
        Self::load_bytes_with(source, &bytes, config)
    }

    /// Applies a save payload onto this (already map-matched) game.
    fn restore(&mut self, save: &GameSave) {
        // `SECTION_MAKER_CHILDREN` (29, M9.5): recreates every
        // `monstermaker` child the save recorded, before any of the
        // index-keyed sections below (`entities`/18, `entity_combat`/24,
        // `ai`/25) are zipped against `self.level.registry.entities` — a
        // fresh `attach_level` (already run by `Self::load_with`, this
        // method's caller) only ever spawns the map's own declared
        // entities, never a maker's dynamically-created children, so
        // without this step those sections' own zip would simply stop
        // short of every maker child the save described. See
        // `crate::ai::AiState::restore_maker_children`'s own doc comment.
        let maker_children_pairs = self
            .systems
            .ai
            .restore_maker_children(&mut self.level, save.maker_children.as_deref());
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
        self.camera.yaw = save.view.yaw;
        self.camera.pitch = save.view.pitch;
        self.controller = PlayerController::spawn_at(
            Vec3::from_array(save.view.position),
            save.view.yaw,
            save.view.pitch,
        );
        // `save.view.position` is the physics origin (see `Self::to_save`'s
        // doc comment); the camera itself always tracks the *eye* position,
        // derived from that origin the same way a normal tick would.
        self.camera.position = if self.level.collision.is_some() {
            self.controller.eye_position().to_array()
        } else {
            save.view.position
        };
        self.difficulty = save.difficulty();
        self.clock = TickClock::new();
        // `Self::load_with` (this game's own constructor, just above in
        // `from_save`) already built a fresh `Systems` and called
        // `attach_level` exactly once, with this same `save.difficulty()`
        // and this level's own skill table. Calling `reset`/`attach_level`
        // again here was a redundant re-attach, now removed:
        // `ohl_ai::attach_monsters` only inserts components onto the
        // entities `Registry::build` already spawned rather than spawning
        // new ones, so this never produced duplicate monster entities; the
        // second call was simply doing the same brain-registration and
        // component-insertion work twice for no reason.
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
        // `SECTION_INVENTORY` (23, M7.9 P4b): when a newer save's typed
        // section is present it overlays the legacy blob above with the
        // exact same data (a save this build writes always agrees with
        // itself), and additionally restores the drawn weapon's firing
        // summary and the exact selection, neither of which the legacy
        // blob alone could carry.
        if let Some(inventory) = &save.inventory {
            self.systems.restore_inventory(inventory);
        }
        // `SECTION_ENTITY_COMBAT`/`SECTION_AI` (24/25, M7.9 P4b): applied
        // after `attach_level` has rebuilt this level's monsters fresh, so
        // these snapshots overlay the exact health/AI state the save
        // recorded onto entities `attach_level` just spawned in the same
        // deterministic spawn order.
        if let Some(entity_combat) = &save.entity_combat {
            Systems::restore_entity_combat(&mut self.level, entity_combat);
        }
        if let Some(ai) = &save.ai {
            Systems::restore_ai(&mut self.level, ai);
        }
        // The entity snapshots and 24/25 above just restored every
        // monster's `Transform` from the save; `ohl_ai::Actor` (sensing,
        // navigation, attacks) still holds this level's fresh spawn
        // position from `attach_level`'s own `attach_monsters` call.
        // Carrying `Transform` onto `Actor` here, once, is what makes a
        // reloaded monster think from where the save actually left it
        // rather than from the map's spawn point; see
        // `Systems::sync_actor_from_transforms`'s doc.
        Systems::sync_actor_from_transforms(&mut self.level);
        // `SECTION_MAKER_CHILDREN` (29, M9.5), part two: links each child
        // `Self::restore_maker_children` recreated above back onto its
        // maker's own live-child list, now that its restored health/AI
        // state (and so `ohl_ai::Actor::alive`) is actually known — see
        // `crate::ai::AiState::finalize_maker_children`'s own doc comment
        // for why this must run after entity_combat/ai/actor-sync rather
        // than immediately alongside the recreation above.
        crate::ai::AiState::finalize_maker_children(&mut self.level, &maker_children_pairs);
        // `SECTION_PROJECTILES` (26, M7.9 P4b).
        if let Some(projectiles) = &save.projectiles {
            self.systems
                .restore_projectiles(&mut self.level, projectiles);
        }
        // `SECTION_RNG` (27, M7.9 P4b).
        if let Some(rng) = &save.rng {
            self.systems.restore_rng(*rng);
        }
        // `SECTION_MOVER_STATE` (28, M7.13): applied last, after
        // `attach_level` has rebuilt every `func_train`/`func_tracktrain`,
        // `trigger_camera`, `scripted_sequence` and `monstermaker` fresh,
        // so this overlays the exact runtime progress the save recorded
        // (a train's position mid-route, a camera sequence mid-hold, a
        // script mid-`Moving`, a maker's spawn counters) onto entities
        // `attach_level` just spawned in the same deterministic spawn
        // order — the same pattern `SECTION_ENTITY_COMBAT`/`SECTION_AI`
        // already use above.
        if let Some(mover_state) = &save.mover_state {
            self.systems
                .restore_mover_state(&mut self.level, mover_state);
        }
        // `SECTION_ROTATING_MOVER_STATE` (30, M9.6): the same
        // spawn-order-zipped overlay `SECTION_MOVER_STATE` above applies,
        // for the three entities that ride their own tag instead (see
        // `crate::save_state::MoverSnapshot`'s own doc comment for why).
        if let Some(rotating_movers) = &save.rotating_movers {
            crate::save_state::restore_rotating_movers(&mut self.level, &rotating_movers.movers);
            self.level
                .simulation
                .restore_rot_button_touch(&rotating_movers.rot_button_touch);
        }
        // `SECTION_MOMENTARY_DOOR_STATE` (31, M9.8): same spawn-order-zipped
        // overlay, applied after tag 30 restores the button driving it (the
        // two are independent state either way — a door simply holds its
        // own last-restored `fraction` until a button pushes it again).
        if let Some(momentary_doors) = &save.momentary_doors {
            crate::save_state::restore_momentary_doors(&mut self.level, momentary_doors);
        }
        // `SECTION_BREAKABLE_STATE` (33, M9.10): the same spawn-order-zipped
        // overlay. A brush that had already broken before the save stays
        // broken after the load, and a pushed `func_pushable` is restored
        // where the player left it, not where it was compiled.
        if let Some(breakables) = &save.breakables {
            crate::save_state::restore_breakables(&mut self.level, breakables);
        }
        // `SECTION_TELEPORT_STATE` (34): the `trigger_teleport` touch edges
        // and `multisource` fire counts, both keyed by `hecs` bit pattern
        // rather than spawn order (see `crate::save::TeleportStateSnapshot`).
        if let Some(teleport_state) = &save.teleport_state {
            self.level.simulation.restore_teleport_state(
                &teleport_state.teleport_touch,
                &teleport_state.master_fires,
            );
        }
        // `SECTION_TRAIN_HANDOVER_YAW` (35, M9.25): the same
        // spawn-order-zipped overlay, applied after tag 28 has put every
        // train back on its chain — the carried heading is only ever read
        // when that chain defines none, so the two never disagree.
        if let Some(train_handover_yaw) = &save.train_handover_yaw {
            crate::save_state::restore_train_handover_yaw(&mut self.level, train_handover_yaw);
        }
        // A load is a map load: the chapter title is announced again.
        self.pending.clear();
        self.pending.extend(chapter_title_event(&self.level.name));

        // The snapshots just applied above may have moved a mover entity
        // (its `Transform`) far from where this restore's fresh `Level`
        // attached its collision brushes — at each brush's *spawn-time
        // placed* position, in `attach_brush_collision`, which is the
        // entity's `origin` keyvalue plus its map logic's own spawn offset
        // (a train's, for instance, puts it on the first node of its
        // path), not the save's restored one. A cold map load never has
        // this gap, since that same attach is already the position the
        // simulation starts from. Left alone, next step's own
        // `sync_brush_collision` would see that whole restore displacement
        // as one step's motion and divide it by `dt`, synthesizing a large
        // `brush_velocity` out of nothing — safe for the ride path (a
        // restored `PlayerState::ground_brush` is never trusted;
        // `categorize_position` recomputes it fresh on the first tick),
        // but the push path added alongside mover riders
        // reads `brush_velocity` unconditionally and would shove a player
        // who happens to be standing inside a restored mover's hull by the
        // full restore displacement. Syncing once here, with a
        // non-positive `dt`, moves every brush to its restored position
        // and records zero velocity for all of them — exactly the seed a
        // cold load already gets for free. `Self::apply_transition` makes
        // the identical zero-`dt` call, for the identical reason, after a
        // level change re-seats a carried `func_tracktrain`.
        self.level.sync_brush_collision(0.0);
    }

    /// Returns cumulative resource uploads for the current level.
    /// Counts reset when a level's renderer is replaced.
    #[must_use]
    pub fn render_resource_stats(&self) -> crate::RenderResourceStats {
        self.renderers
            .as_ref()
            .map_or_else(Default::default, Renderers::resource_stats)
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

    /// As [`Self::render`], but drawn from `position`/`pitch`/`yaw` instead
    /// of this game's own tracked camera, for exactly this one frame.
    ///
    /// Unlike [`Self::set_viewpoint`] this never touches the physics
    /// controller or noclip: the tracked camera is saved, swapped in for
    /// the draw call, and restored before returning, so the walking
    /// player's own position keeps advancing normally on every following
    /// [`Self::tick`] (riding a mover, gravity, collision, all unaffected).
    /// Intended for a headless capture that wants the render eye to follow
    /// the player at a fixed offset (`--spawn-offset`) rather than freeze
    /// in world space.
    ///
    /// # Errors
    /// As [`Self::render`].
    pub fn render_from(
        &mut self,
        context: &GpuContext,
        target: RenderTarget<'_>,
        position: [f32; 3],
        pitch: f32,
        yaw: f32,
    ) -> Result<()> {
        let saved = self.camera;
        self.camera.position = position;
        self.camera.pitch = pitch;
        self.camera.yaw = yaw;
        let result = self.render(context, target);
        self.camera = saved;
        result
    }
}

/// The chapter-title event for `map`, when `ohl-campaign` knows the chapter
/// that map belongs to.
fn chapter_title_event(map: &str) -> Option<GameEvent> {
    ohl_campaign::chapter_of(map).map(|chapter| GameEvent::ChapterTitle(chapter.title.to_string()))
}
