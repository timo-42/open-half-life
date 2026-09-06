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
//! No phase reads a wall clock, and no phase iterates a `HashMap`. There is
//! exactly one root random stream, seeded from [`SystemsConfig::rng_seed`]
//! and never from the environment: [`Systems::rng`] *is* that stream, and
//! every other generator in the simulation — the AI world's included — is
//! seeded from a draw off it rather than from the configured seed a second
//! time. Two generators seeded with the same number would otherwise produce
//! the same sequence, which is a correlation nothing here wants.
//!
//! # Logging
//!
//! Nothing here logs; see the crate-level note.

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_game::{Event, find_usable_within};
use ohl_physics::{ControllerInput, PlayerController};
use ohl_player::PlayerSystems;
use ohl_render::{FreeFlyCamera, MoveInput};

use crate::USE_RADIUS;
use crate::ai::AiState;
use crate::combat::CombatState;
use crate::components::StudioAnim;
use crate::input::Input;
use crate::level::Level;
use crate::pickups::PickupsState;
use crate::presentation::{Presentation, PresentationEvent};
use crate::projectiles::ProjectileSystem;
use crate::sprites::TransientSprites;
use crate::viewmodel::ViewModel;
use ohl_combat::HitboxIndex;

/// The project's default random seed.
///
/// A constant, not a clock or an environment read: two games built from the
/// same map bytes with the same config must step identically. It is saved
/// and restored with the rest of the simulation state.
pub const DEFAULT_RNG_SEED: u64 = 0x4F48_4C5F_5039_0001;

/// One hit waiting to be resolved: who it lands on, and what it does.
///
/// [`ohl_combat::DamageInfo`] describes a hit without naming its target
/// (it is handed straight to `ohl_combat::apply_damage` against a chosen
/// health component), so the engine's queue pairs the two. Drained once per
/// step, in insertion order, so the result never depends on iteration
/// order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueuedDamage {
    /// The entity the hit lands on.
    pub target: ohl_game::hecs::Entity,
    /// What the hit does.
    pub info: ohl_combat::DamageInfo,
}

/// How close the player's origin must be to a `trigger_hurt` volume's own
/// origin for it to apply this step. **To be black-box observed**: the real
/// test is whether the player is inside the volume's brush, not within a
/// radius of a point; see `crate::pickups::PICKUP_TOUCH_RADIUS` for the same
/// simplification applied to pickups.
// TODO(black-box): replace with a real volume-overlap test.
const TRIGGER_HURT_RADIUS: f32 = 128.0;

/// Everything about the step list a host chooses rather than the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemsConfig {
    /// Seeds the single random stream the simulation phases share.
    pub rng_seed: u64,
    /// The view model's own vertical field of view, in degrees, independent
    /// of the world camera's.
    ///
    /// TODO(black-box): Half-Life's viewmodel FOV (and whether/how it
    /// differs from the world FOV) is not published; see
    /// `crate::viewmodel`'s module doc.
    pub view_model_fov: f32,
    /// The view model's placement offset from the camera's eye, read as
    /// `(forward, left, up)` camera-relative world units (`crate::viewmodel::placement`).
    ///
    /// TODO(black-box): the offset (and any weapon bob) is not published.
    pub view_model_offset: [f32; 3],
}

impl Default for SystemsConfig {
    fn default() -> Self {
        Self {
            rng_seed: DEFAULT_RNG_SEED,
            // TODO(black-box): matches the world camera's own default FOV
            // (`ohl_render::FreeFlyCamera::default`) until observed
            // otherwise.
            view_model_fov: 75.0,
            // TODO(black-box): a small forward-and-down offset, low enough
            // to sit in the lower part of the frame without covering it.
            view_model_offset: [8.0, 0.0, -6.0],
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
    /// Whether "use" is currently held, mirroring [`Input::use_held`]. A
    /// held axis, not an edge: it applies to every step of the frame, the
    /// same as `attack`/`attack2`.
    pub(crate) use_held: bool,
    pub(crate) reload_pressed: bool,
    pub(crate) flashlight_pressed: bool,
    pub(crate) select_slot: Option<u8>,
}

/// The edges (presses, not holds) a frame delivered that no step has taken
/// yet.
///
/// Edges are sticky: a frame too short to release a step still records its
/// presses, and the next step that runs consumes them. Without this a press
/// delivered on a sub-step frame — every frame, on a host rendering faster
/// than the tick rate — would be dropped entirely.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PendingEdges {
    use_pressed: bool,
    reload: bool,
    flashlight: bool,
    /// The most recent slot selection; a later press supersedes an earlier
    /// one, since only one weapon can be selected.
    select_slot: Option<u8>,
}

impl PendingEdges {
    /// Records `input`'s edges alongside anything not yet consumed.
    fn accumulate(&mut self, input: &Input) {
        self.use_pressed |= input.use_pressed;
        self.reload |= input.reload;
        self.flashlight |= input.flashlight_pressed;
        if input.select_slot.is_some() {
            self.select_slot = input.select_slot;
        }
    }
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
            use_held: input.use_held,
            reload_pressed: false,
            flashlight_pressed: false,
            select_slot: None,
        }
    }

    /// The held state of `input` plus the edges no step has taken yet.
    fn with_edges(input: &Input, edges: PendingEdges) -> Self {
        Self {
            use_pressed: edges.use_pressed,
            reload_pressed: edges.reload,
            flashlight_pressed: edges.flashlight,
            select_slot: edges.select_slot,
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
    /// The frame's held input, latched by [`Systems::begin_frame`].
    frame_input: Input,
    /// Edges delivered but not yet handed to a step. Sticky across frames
    /// that release no step, so a press on a short frame is not lost.
    pending_edges: PendingEdges,
    /// The HUD the host draws. Written by the presentation phase.
    hud: ohl_ui::hud::HudState,
    /// The one random stream every simulation phase shares, seeded from
    /// [`SystemsConfig::rng_seed`] and never from the environment.
    rng: ohl_ai::Pcg32,
    /// How many fixed steps have run since [`Self::attach_level`] (a level
    /// load or a save restore) last reset it. Saved as part of
    /// `SECTION_RNG` (27) alongside [`Self::rng`]'s own state, purely as a
    /// determinism cross-check a test can compare across a save/load
    /// boundary; nothing in the step list reads it back.
    substep_counter: u64,
    /// Every hit a weapon (phase 6), a projectile or blast (phase 7) or an
    /// AI attack (phase 8) produced this step, paired with its target.
    /// Phase 9 drains everything aimed at the player or at a non-monster
    /// entity; whatever targets a monster is left for phase 10's
    /// `ohl_ai::apply_monster_damage` drain, which is the one place a
    /// monster's health moves. This is the shared field `docs/m79-design.md`
    /// §8 asks P1 and P3 to agree on by name and type.
    pub(crate) damage_queue: Vec<QueuedDamage>,
    /// The AI world, its brains and this map's navigator.
    pub(crate) ai: AiState,
    /// Phase 7's own state: live projectiles and placed deployables.
    projectiles: ProjectileSystem,
    /// The bounded transient-sprite list phase 7 (and phase 6's muzzle
    /// flashes, in a later package) fills; phase 13 ages it.
    transient_sprites: TransientSprites,
    /// The first-person view model's animation state.
    view_model: ViewModel,
    /// The player's own health/armor/suit/flashlight/long-jump systems
    /// (M7.9 P1). Motion lives in `ohl-physics`/`PlayerController`; this is
    /// everything that reacts to it.
    player: ohl_player::Player,
    /// The hitbox index, weapon firing and damage queue (M7.9 P1).
    combat: CombatState,
    /// Pickup touch tests and chargers (M7.9 P1).
    pickups: PickupsState,
    /// The HUD/audio presentation bridge (M7.9 P1).
    presentation: Presentation,
    /// This step's player-systems events, produced in phase 3 and by phase
    /// 9's damage routing, consumed by phase 13.
    player_events: Vec<ohl_player::PlayerEvent>,
    /// The posed hitbox index, rebuilt by phase 5 and read by phase 6 (and
    /// phase 7's projectiles/deployables). Owned directly by `Systems`,
    /// cleared and refilled each step rather than reallocated, so every
    /// attack this step traces against the exact same index.
    hitboxes: HitboxIndex,
    /// The last player-move step's physics report, produced in phase 2 and
    /// consumed by phase 3.
    physics_output: ohl_player::PhysicsOutput,
    /// How many times damage aimed at the player has actually been applied
    /// since this level was attached. Media-derived: data, never a log
    /// line from this crate.
    player_damage_events: u64,
}

impl Systems {
    /// The step list with `config`'s seed and an empty HUD.
    #[must_use]
    pub fn new(config: SystemsConfig) -> Self {
        let mut rng = ohl_ai::Pcg32::new(config.rng_seed);
        // One root stream: the AI world's generator is seeded from a draw
        // off this one, so the two never run in lockstep.
        let ai_seed = (u64::from(rng.next_u32()) << 32) | u64::from(rng.next_u32());
        Self {
            config,
            frame_input: Input::default(),
            pending_edges: PendingEdges::default(),
            hud: ohl_ui::hud::HudState::default(),
            rng,
            substep_counter: 0,
            damage_queue: Vec::new(),
            ai: AiState::new(ai_seed),
            projectiles: ProjectileSystem::new(config.rng_seed),
            transient_sprites: TransientSprites::new(),
            view_model: ViewModel::new(),
            player: ohl_player::Player::default(),
            combat: CombatState::new(),
            pickups: PickupsState::new(),
            presentation: Presentation::new(),
            player_events: Vec::new(),
            physics_output: ohl_player::PhysicsOutput::default(),
            hitboxes: HitboxIndex::new(ohl_combat::HitboxLimits::default()),
            player_damage_events: 0,
        }
    }

    /// How many times the player has actually fired a weapon since this
    /// level was attached. Media-derived: data, never a log line from this
    /// crate.
    #[must_use]
    pub fn weapon_fired_count(&self) -> u64 {
        self.combat.fired_count()
    }

    /// How many of those shots have landed on an entity. Media-derived:
    /// data, never a log line from this crate.
    #[must_use]
    pub fn shot_hit_count(&self) -> u64 {
        self.combat.hit_count()
    }

    /// How many pickups have actually been taken since this level was
    /// attached. Media-derived: data, never a log line from this crate.
    #[must_use]
    pub fn pickup_count(&self) -> u64 {
        self.pickups.taken_count()
    }

    /// How many times damage aimed at the player has actually been applied
    /// since this level was attached. Media-derived: data, never a log
    /// line from this crate.
    #[must_use]
    pub fn player_damage_event_count(&self) -> u64 {
        self.player_damage_events
    }

    /// The AI world this step list drives.
    #[must_use]
    pub fn ai(&self) -> &AiState {
        &self.ai
    }

    /// The AI world, mutably, so a host can install a projectile spawner or
    /// queue damage against a monster.
    pub fn ai_mut(&mut self) -> &mut AiState {
        &mut self.ai
    }

    /// The one random stream the simulation phases share.
    ///
    /// Public so a later package's phase can draw from the same seeded
    /// stream rather than starting one of its own, which is what keeps two
    /// games from the same seed identical.
    pub fn rng(&mut self) -> &mut ohl_ai::Pcg32 {
        &mut self.rng
    }

    /// The player's weapons, ammo, HEV suit and long-jump ownership.
    #[must_use]
    pub(crate) fn inventory(&self) -> ohl_combat::Inventory {
        self.combat.display_inventory()
    }

    /// The player's current health, from `ohl_player::Player`'s own state.
    #[must_use]
    pub(crate) fn player_health(&self) -> f32 {
        self.player.state.health
    }

    /// The player's current HEV armor, from `ohl_player::Player`'s own
    /// state.
    #[must_use]
    pub(crate) fn player_armor(&self) -> f32 {
        self.player.state.armor
    }

    /// This step's player health/armor/weapons/ammo/suit/long-jump, as a
    /// [`crate::transition::PlayerCarryState`] for a level change or a save.
    /// See `crate::combat::CombatState::capture_carry`'s doc comment for
    /// what `extra` holds, which already round-trips through a save file
    /// unchanged (it is plain data inside `PlayerCarryState`).
    #[must_use]
    pub(crate) fn capture_carry(&self) -> crate::transition::PlayerCarryState {
        crate::transition::PlayerCarryState {
            health: self.player.state.health,
            armor: self.player.state.armor,
            extra: self.combat.capture_carry(),
        }
    }

    /// Applies a previously captured [`crate::transition::PlayerCarryState`]
    /// onto a freshly reset `Systems` (called right after
    /// [`Self::reset`], so there is nothing else to clobber).
    pub(crate) fn restore_carry(&mut self, state: &crate::transition::PlayerCarryState) {
        self.player.state.health = state.health.clamp(0.0, self.player.config.max_health);
        self.player.state.armor = state.armor.clamp(0.0, self.player.config.max_armor);
        self.combat.restore_carry(&state.extra, &mut self.player);
    }

    /// Takes every presentation event collected since the last call.
    pub(crate) fn drain_presentation_events(&mut self) -> Vec<PresentationEvent> {
        self.presentation.drain_events()
    }

    /// `SECTION_INVENTORY` (23): the typed inventory/firing snapshot.
    #[must_use]
    pub(crate) fn snapshot_inventory(&self) -> crate::save_state::InventorySnapshot {
        self.combat.snapshot()
    }

    /// Restores `SECTION_INVENTORY` (23), overlaying whatever
    /// [`Self::restore_carry`]'s legacy blob already applied with the typed
    /// fields (which a save written by this package always agrees with).
    pub(crate) fn restore_inventory(&mut self, snapshot: &crate::save_state::InventorySnapshot) {
        self.combat.restore_snapshot(snapshot, &mut self.player);
    }

    /// `SECTION_ENTITY_COMBAT` (24): one entry per `level.registry.entities`
    /// slot, in spawn order; `None` for an entity no longer present in the
    /// world at all (see `crate::save_state::snapshot_entity_combat`).
    #[must_use]
    pub(crate) fn snapshot_entity_combat(
        level: &Level,
    ) -> Vec<Option<crate::save_state::EntityCombatSnapshot>> {
        level
            .registry
            .entities
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_ENTITIES)
            .map(|entity| crate::save_state::snapshot_entity_combat(level, *entity))
            .collect()
    }

    /// Restores `SECTION_ENTITY_COMBAT` (24), zipped against
    /// `level.registry.entities` in spawn order; a save with more or fewer
    /// entries than the reloaded level currently has simply stops applying
    /// once either list runs out. A `None` entry despawns the freshly
    /// attach-level-spawned entity at that slot, so a monster the save
    /// recorded as gone (gibbed, `world.despawn`ed before the save) stays
    /// gone after the load instead of a fresh level load's own
    /// `attach_monsters` silently resurrecting it.
    pub(crate) fn restore_entity_combat(
        level: &mut Level,
        snapshots: &[Option<crate::save_state::EntityCombatSnapshot>],
    ) {
        let entities = level.registry.entities.clone();
        for (entity, snapshot) in entities.iter().zip(snapshots) {
            crate::save_state::restore_entity_combat(level, *entity, snapshot.as_ref());
        }
    }

    /// `SECTION_AI` (25): one entry per `level.registry.entities` slot, in
    /// spawn order; `None` for an entity with no [`ohl_ai::MonsterAi`].
    #[must_use]
    pub(crate) fn snapshot_ai(level: &Level) -> Vec<Option<crate::save_state::AiSnapshot>> {
        level
            .registry
            .entities
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_ENTITIES)
            .map(|entity| crate::ai::AiState::snapshot_entity(level, *entity))
            .collect()
    }

    /// Restores `SECTION_AI` (25), zipped against `level.registry.entities`
    /// the same way [`Self::restore_entity_combat`] is.
    pub(crate) fn restore_ai(
        level: &mut Level,
        snapshots: &[Option<crate::save_state::AiSnapshot>],
    ) {
        let entities = level.registry.entities.clone();
        for (entity, snapshot) in entities.iter().zip(snapshots) {
            if let Some(snapshot) = snapshot {
                crate::ai::AiState::restore_entity(level, *entity, snapshot);
            }
        }
    }

    /// `SECTION_PROJECTILES` (26): live projectiles and placed deployables.
    #[must_use]
    pub(crate) fn snapshot_projectiles(
        &self,
        level: &Level,
    ) -> crate::save_state::ProjectilesSnapshot {
        self.projectiles.snapshot(level)
    }

    /// Restores `SECTION_PROJECTILES` (26).
    pub(crate) fn restore_projectiles(
        &mut self,
        level: &Level,
        snapshot: &crate::save_state::ProjectilesSnapshot,
    ) {
        self.projectiles.restore_snapshot(level, snapshot);
    }

    /// `SECTION_RNG` (27): the shared random stream's state and the substep
    /// counter.
    #[must_use]
    pub(crate) fn snapshot_rng(&self) -> crate::save_state::RngSnapshot {
        let (state, increment) = self.rng.snapshot();
        let (ai_state, ai_increment) = self.ai.rng_snapshot();
        crate::save_state::RngSnapshot {
            state,
            increment,
            substep_counter: self.substep_counter,
            ai_state,
            ai_increment,
            ai_tick_count: self.ai.tick_count(),
        }
    }

    /// Restores `SECTION_RNG` (27).
    pub(crate) fn restore_rng(&mut self, snapshot: crate::save_state::RngSnapshot) {
        self.rng = ohl_ai::Pcg32::from_snapshot((snapshot.state, snapshot.increment));
        self.substep_counter = snapshot.substep_counter;
        self.ai.restore_rng(
            (snapshot.ai_state, snapshot.ai_increment),
            snapshot.ai_tick_count,
        );
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

    /// How many projectiles and placed deployables are currently live.
    #[must_use]
    pub fn projectile_count(&self) -> usize {
        self.projectiles.count()
    }

    /// Whether this frame draws a view model.
    #[must_use]
    pub fn viewmodel_visible(&self) -> bool {
        self.view_model.is_visible()
    }

    /// Test-only hook backing [`crate::Game::debug_show_viewmodel_and_sprite`].
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn debug_show_viewmodel_and_sprite(&mut self, model_slot: usize) {
        self.view_model.set_model(Some(model_slot));
        self.transient_sprites
            .push(crate::sprites::TransientSprite {
                asset: 0,
                origin: [0.0, 0.0, 40.0],
                scale: 4.0,
                render: ohl_render::RenderProps::from_entity(5, 255, [255, 255, 255], 0),
                seconds_left: 5.0,
                age: 0.0,
            });
    }

    /// The transient sprites [`crate::render`] appends to the sprite pass.
    pub(crate) fn transient_sprites(&self) -> &TransientSprites {
        &self.transient_sprites
    }

    /// The view model's current animation state, for [`crate::render`].
    pub(crate) fn view_model(&self) -> &ViewModel {
        &self.view_model
    }

    /// Spawns a projectile owned by `owner` (or unowned, for e.g. a scripted
    /// hazard). This is the seam `docs/m79-design.md` §8 P3 documents so a
    /// later package's monster ranged attacks (`ohl-ai`'s
    /// `AiEventKind::Attack`) can spawn one without this crate depending on
    /// `ohl-ai`: that package calls this from its own phase-8 hook.
    #[allow(dead_code)]
    pub(crate) fn spawn_projectile(
        &mut self,
        level: &mut Level,
        kind: ohl_combat::ProjectileKind,
        owner: Option<Entity>,
        origin: Vec3,
        velocity: Vec3,
    ) -> Option<ohl_combat::ProjectileId> {
        self.projectiles.spawn(level, kind, owner, origin, velocity)
    }

    /// Clears everything a level change or a save load invalidates, keeping
    /// the configuration. The caller re-attaches the new level with
    /// [`Self::attach_level`].
    pub(crate) fn reset(&mut self) {
        let config = self.config;
        *self = Self::new(config);
    }

    /// Builds the AI state for a freshly loaded level: its monsters,
    /// `monstermaker`s, declared triggers, navigation graph and the
    /// client's own actor.
    pub(crate) fn attach_level(
        &mut self,
        level: &mut Level,
        difficulty: ohl_campaign::Difficulty,
        skill: &ohl_campaign::SkillTable,
    ) {
        self.damage_queue.clear();
        self.ai.attach_level(level, difficulty, skill);
        // Auto-wires a model-backed projectile/deployable kind to whichever
        // of this map's own loaded studio models matches its conventional
        // path (`crate::projectiles::ProjectileSystem::configure_models`'s
        // doc); harmless when none match, which is the ordinary case.
        self.projectiles.configure_models(level);
    }

    /// Latches one frame's input. Called once per [`crate::Game::tick`],
    /// before any step runs.
    ///
    /// Held axes are replaced (they describe *now*); edges are accumulated
    /// (they describe something that happened, and must survive until a
    /// step can act on it).
    pub(crate) fn begin_frame(&mut self, input: &Input) {
        self.frame_input = *input;
        self.pending_edges.accumulate(input);
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
        self.physics_output = ohl_player::PhysicsOutput::from_move(
            &controller.state,
            &controller.config,
            &controller.last_move_events(),
        );
        self.player_systems(level, input, dt); // 3
        Self::actor_sync(level, camera, controller, dt); // 4
        self.rebuild_hitbox_index(level); // 5
        self.weapons(level, controller, dt, input); // 6
        self.projectiles(level, dt); // 7
        self.ai_think(level, dt); // 8
        self.resolve_damage(level); // 9
        self.reap_deployables(level); // 9b
        self.lifecycle(level, dt); // 10
        self.pickups(level, input, dt); // 11
        self.triggers_and_movers(level, camera, input, dt, events); // 12
        self.presentation(level, dt); // 13
        self.substep_counter = self.substep_counter.wrapping_add(1);
    }

    /// Phase 1 — input latch. Axes hold for every step of the frame; edges
    /// are handed to the first step that runs after they arrived, and only
    /// to that one, so a single press cannot fire a phase ten times in one
    /// long frame nor be dropped by a frame too short to step at all.
    fn latch_input(&mut self) -> LatchedInput {
        LatchedInput::with_edges(&self.frame_input, std::mem::take(&mut self.pending_edges))
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
    fn player_systems(&mut self, level: &Level, input: LatchedInput, dt: f32) {
        let mut player_input = ohl_player::PlayerInput {
            flashlight_pressed: input.flashlight_pressed,
            use_held: input.use_held,
            jump: input.jump,
            duck: input.duck,
            hurt: Vec::new(),
        };
        let player_origin = self.physics_output.origin;
        for (hurt, transform) in &mut level
            .registry
            .world
            .query::<(&ohl_game::TriggerHurt, &Transform)>()
        {
            if transform.origin.distance(player_origin) <= TRIGGER_HURT_RADIUS {
                player_input.push_hurt(ohl_player::HurtInput::from_trigger_hurt(hurt));
            }
        }
        let events = match level.collision.as_ref() {
            Some(collision) => self
                .player
                .tick(dt, &player_input, &self.physics_output, collision),
            None => self.player.tick(
                dt,
                &player_input,
                &self.physics_output,
                &ohl_player::EmptyWorld,
            ),
        };
        self.player_events.extend(events);
    }

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
        // The client's `Actor` is what lets a monster see, target and shoot
        // the player through the components it uses for anything else.
        let health = level
            .registry
            .world
            .get::<&ohl_combat::Health>(player)
            .ok()
            .map(|health| *health);
        if let Ok(mut actor) = level.registry.world.get::<&mut ohl_ai::Actor>(player) {
            actor.origin = origin;
            actor.view_ofs = controller.eye_position() - origin;
            actor.yaw = camera.yaw;
            if let Some(health) = health {
                actor.health = health.current;
                actor.alive = !health.is_dead();
            }
        }
        for anim in &mut level.registry.world.query::<&mut StudioAnim>() {
            anim.advance(dt);
        }
    }

    /// Phase 5 — hitbox index: rebuilt each step from every entity carrying
    /// a pose and an actor, so a trace hits where the model is drawn.
    /// Model-backed projectiles and deployables (a flying rocket, a placed
    /// tripmine) are kept in this index like anything else — a tripmine or
    /// satchel must stay shootable — and instead ignored per trace by
    /// whichever trace must not hit itself (`crate::projectiles`' module
    /// doc; `ohl_combat::Projectile::self_id`/`owner`).
    fn rebuild_hitbox_index(&mut self, level: &mut Level) {
        crate::combat::rebuild_hitbox_index(&mut self.hitboxes, level);
    }

    /// Phase 6 — weapons: the firing state machine, its hitscan traces and
    /// the damage they queue.
    fn weapons(
        &mut self,
        level: &Level,
        controller: &PlayerController,
        dt: f32,
        input: LatchedInput,
    ) {
        self.combat.weapons(
            level,
            controller,
            dt,
            input,
            level.player,
            &self.hitboxes,
            &mut self.damage_queue,
            &mut self.hud,
            &mut self.presentation,
        );
    }

    /// Phase 7 — projectiles and deployables, and the radius damage they
    /// queue. See `crate::projectiles`. Traces against [`Self::hitboxes`]
    /// (phase 5's rebuild), the same index phase 6's weapons used, rather
    /// than rebuilding its own.
    fn projectiles(&mut self, level: &mut Level, dt: f32) {
        self.projectiles.tick(
            level,
            &self.hitboxes,
            dt,
            &mut self.damage_queue,
            &mut self.transient_sprites,
        );
    }

    /// Phase 8 — AI think and navigation. Runs before damage resolution on
    /// purpose: see the module note.
    fn ai_think(&mut self, level: &mut Level, dt: f32) {
        self.ai.think(level, dt, &mut self.damage_queue);
    }

    /// Phase 9 — damage resolution: the queue is drained once, in insertion
    /// order.
    fn resolve_damage(&mut self, level: &mut Level) {
        let player_id = level.player;
        crate::combat::resolve_damage(
            &mut self.damage_queue,
            level,
            &mut self.player,
            player_id,
            &mut self.hud,
            &mut self.presentation,
            &mut self.player_events,
            &mut self.player_damage_events,
        );
    }

    /// Phase 9b — a placed satchel or tripmine that phase 9 just brought to
    /// zero health (the player's hitscan, or another explosive's blast)
    /// detonates this same step; see
    /// `crate::projectiles::ProjectileSystem::resolve_deployable_damage`.
    ///
    /// A detonation's own blast can kill *another* deployable's stand-in
    /// (`resolve_deployable_damage`'s doc): a satchel chain reaction. That
    /// freshly queued damage must not reach phase 10 — its `lifecycle`
    /// drain (`ohl_ai`'s `drain_engine_damage`) only ever looks at
    /// `MonsterAi` targets and silently discards everything else, which
    /// would otherwise throw away the player's own share of the blast too.
    /// So this loops: resolve whatever `resolve_deployable_damage` just
    /// queued through the exact same `resolve_damage` phase 9 already ran,
    /// then check again for anything that blast just killed, until nothing
    /// detonates. `DeployableSet` only shrinks (`detonate` removes by
    /// handle), so this is bounded by the number of deployables placed and
    /// always terminates. The result: a whole chain resolves within this
    /// one step, with no per-link tick of latency — not because it was
    /// deferred to next step's phase 9, but because this loop *is* that
    /// resolution, run early.
    fn reap_deployables(&mut self, level: &mut Level) {
        loop {
            let detonated = self.projectiles.resolve_deployable_damage(
                level,
                &mut self.damage_queue,
                &mut self.transient_sprites,
            );
            if detonated == 0 {
                break;
            }
            self.resolve_damage(level);
        }
    }

    /// Phase 10 — lifecycle: deaths, corpses, gibs and `monstermaker`.
    fn lifecycle(&mut self, level: &mut Level, dt: f32) {
        self.ai.lifecycle(level, dt, &mut self.damage_queue);
    }

    /// Phase 11 — pickups: touch tests and the use-and-hold chargers.
    fn pickups(&mut self, level: &mut Level, input: LatchedInput, dt: f32) {
        let player_origin = self.physics_output.origin;
        #[allow(clippy::cast_possible_truncation)]
        let player_tag = crate::ids::entity_id(level.player).0 as u32;
        let (inventory, ammo) = self.combat.inventory_and_ammo_mut();
        self.pickups.run(
            level,
            player_origin,
            player_tag,
            input,
            dt,
            inventory,
            ammo,
            &mut self.player,
            &mut self.hud,
            &mut self.presentation,
        );
    }

    /// Phase 12 — triggers and movers, last so everything else has already
    /// settled.
    fn triggers_and_movers(
        &mut self,
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
            } else {
                // Nothing usable in reach: the press is offered to a talk
                // monster instead, by the next step's phase 8.
                self.ai.queue_use(position);
            }
        }
        events.extend(level.simulation.tick(&mut level.registry, dt));
    }

    /// Phase 13 — presentation: the HUD, sound cues and view-model actions
    /// the host reads after the step; also ages the transient-sprite list
    /// and the view model's own animation cursor (phase 7 no longer does,
    /// now that this phase has landed).
    fn presentation(&mut self, level: &mut Level, dt: f32) {
        crate::combat::sync_player_components(level, &self.player);
        let events = std::mem::take(&mut self.player_events);
        self.presentation.tick(
            dt,
            &mut self.hud,
            &self.player,
            events,
            &mut self.view_model,
        );
        self.transient_sprites.tick(dt);
        self.view_model.tick(dt);
    }
}

impl Default for Systems {
    fn default() -> Self {
        Self::new(SystemsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, PendingEdges, Systems};

    fn systems() -> Systems {
        Systems::default()
    }

    fn pressed() -> Input {
        Input {
            use_pressed: true,
            reload: true,
            flashlight_pressed: true,
            select_slot: Some(3),
            ..Input::default()
        }
    }

    /// An edge is handed to exactly one step. A second latch inside the same
    /// frame — which is what a frame long enough to release several steps
    /// does — sees nothing, so one press cannot fire a phase ten times.
    #[test]
    fn an_edge_is_consumed_by_exactly_one_latch() {
        let mut systems = systems();
        systems.begin_frame(&pressed());

        let first = systems.latch_input();
        assert!(first.use_pressed);
        assert!(first.reload_pressed);
        assert!(first.flashlight_pressed);
        assert_eq!(first.select_slot, Some(3));

        let second = systems.latch_input();
        assert!(!second.use_pressed);
        assert!(!second.reload_pressed);
        assert!(!second.flashlight_pressed);
        assert_eq!(second.select_slot, None);
    }

    /// Edges survive frames that release no step. A host rendering faster
    /// than the tick rate delivers most presses on such frames; dropping
    /// them would silently break "use" above 100 fps.
    #[test]
    fn edges_accumulate_across_frames_that_never_latch() {
        let mut systems = systems();
        systems.begin_frame(&pressed());
        // Three more frames, none of them carrying any press, and none of
        // them releasing a step.
        for _ in 0..3 {
            systems.begin_frame(&Input::default());
        }

        let latched = systems.latch_input();
        assert!(latched.use_pressed, "the press was not dropped");
        assert!(latched.reload_pressed);
        assert!(latched.flashlight_pressed);
        assert_eq!(latched.select_slot, Some(3));
    }

    /// Two presses that land before a step runs are still one activation:
    /// the set is a set, not a counter.
    #[test]
    fn repeated_edges_before_a_latch_collapse_to_one() {
        let mut systems = systems();
        systems.begin_frame(&pressed());
        systems.begin_frame(&pressed());

        assert!(systems.latch_input().use_pressed);
        assert!(
            !systems.latch_input().use_pressed,
            "two presses before a step are one activation, not two"
        );
    }

    /// Only one weapon can be selected, so a later slot press supersedes an
    /// earlier one — but a frame that selects nothing must not erase a
    /// selection still waiting for a step.
    #[test]
    fn the_most_recent_slot_wins_and_no_selection_erases_nothing() {
        let mut edges = PendingEdges::default();
        edges.accumulate(&Input {
            select_slot: Some(1),
            ..Input::default()
        });
        edges.accumulate(&Input {
            select_slot: None,
            ..Input::default()
        });
        assert_eq!(
            edges.select_slot,
            Some(1),
            "a frame with no selection leaves the pending one alone"
        );

        edges.accumulate(&Input {
            select_slot: Some(4),
            ..Input::default()
        });
        assert_eq!(edges.select_slot, Some(4), "a later press supersedes");
    }

    /// Held axes describe *now*, so they are replaced rather than
    /// accumulated: releasing a key stops the player on the next frame.
    #[test]
    fn held_axes_are_replaced_rather_than_accumulated() {
        let mut systems = systems();
        systems.begin_frame(&Input {
            forward: 1,
            jump: true,
            ..Input::default()
        });
        systems.begin_frame(&Input::default());

        let latched = systems.latch_input();
        assert_eq!(latched.forward, 0, "a released axis does not linger");
        assert!(!latched.jump);
    }

    /// A level change or a save load must not carry a press across.
    #[test]
    fn a_reset_drops_pending_edges() {
        let mut systems = systems();
        systems.begin_frame(&pressed());
        systems.reset();
        assert!(!systems.latch_input().use_pressed);
    }

    /// The PR #84 review's blocking finding: `resolve_deployable_damage`
    /// (phase 9b) used to run once, after phase 9, and leave whatever fresh
    /// blast damage it queued (against the player, or against another
    /// deployable's stand-in) for phase 10 to see — but phase 10's
    /// `lifecycle` drain only resolves `MonsterAi` targets and silently
    /// discards everything else, so a satchel chain reaction's second hit
    /// never landed. This test drives the actual `Game::tick` (and so the
    /// real `Systems::step` phase order), not hand-called phase functions
    /// in isolation, specifically so a regression of that ordering bug
    /// would fail it — the version of this test that predates the fix
    /// hand-called `resolve_damage`/`resolve_deployable_damage` once each
    /// and passed regardless of the bug.
    #[test]
    fn a_satchel_chain_reaction_resolves_through_the_real_step_order() {
        // A remote-detonated satchel, plus two placed tripmines standing in
        // for "any other deployable a blast might reach" — tripmines,
        // specifically, because `ohl_combat::DeployableSet::detonate_all_satchels`
        // (deliberately) detonates *every* placed satchel at once, which
        // would have set all three off together and defeated the whole
        // point of a chain: only the satchel is remote-detonated here, so
        // both tripmines start this step alive and can only die from blast
        // damage, one hop at a time.
        //
        // Spacing: the satchel's own blast (radius 200,
        // `DeployableTuning::satchel_radius`'s default) reaches the first
        // tripmine (100 units away) but not the second (280 away, outside
        // the radius); only the first tripmine's *own* detonation, in
        // turn, reaches the second (180 away from it). A single,
        // non-looping call to `resolve_deployable_damage` after phase 9
        // would kill the first tripmine (phase 9 itself already resolves
        // the satchel's blast, queued before this tick even starts) but
        // its own freshly queued blast against the second tripmine would
        // then sit in the queue for phase 10 to silently discard — exactly
        // the regression this test is written to catch.
        let bytes = crate::test_support::synthetic_map_bsp();
        let mut assets = crate::assets::MemoryAssets::new();
        assets.insert("maps/ohlsynth.bsp", bytes.clone());
        let mut game = crate::game::Game::from_map_bytes(&assets, "ohlsynth", &bytes)
            .expect("the synthetic map loads");

        let (level, systems) = game.level_and_systems_mut();
        systems
            .projectiles
            .set_model_for_deployable(ohl_combat::DeployableKind::Satchel, Some(0));
        systems
            .projectiles
            .set_model_for_deployable(ohl_combat::DeployableKind::Tripmine, Some(0));
        systems
            .projectiles
            .place_satchel(level, None, glam::Vec3::new(-140.0, 100.0, 40.0))
            .expect("the set has room");
        // Both mines are placed by a straight-down trace from just above
        // the floor, well within `DeployableTuning::place_range`'s default
        // 64-unit reach, so each ends up sitting on the floor at `z = 0`
        // directly below where it was aimed.
        let first_mine = systems
            .projectiles
            .place_tripmine(
                level,
                None,
                glam::Vec3::new(-40.0, 100.0, 40.0),
                glam::Vec3::new(0.0, 0.0, -1.0),
            )
            .expect("the placement trace finds the floor");
        let second_mine = systems
            .projectiles
            .place_tripmine(
                level,
                None,
                glam::Vec3::new(140.0, 100.0, 40.0),
                glam::Vec3::new(0.0, 0.0, -1.0),
            )
            .expect("the placement trace finds the floor");
        let first_mine_entity = systems
            .projectiles
            .deployable_entity(first_mine)
            .expect("a configured model spawns the stand-in entity");
        let second_mine_entity = systems
            .projectiles
            .deployable_entity(second_mine)
            .expect("a configured model spawns the stand-in entity");
        // The remote detonator itself is outside the per-step phase list
        // (`docs/FORMAT_SOURCES.md`'s deployables section: satchels are
        // player-triggered, not phase-driven), so it is still called
        // directly here; everything downstream of it — the two-hop chain
        // this test is actually about — runs through `Game::tick` only.
        systems.projectiles.detonate_all_satchels(
            level,
            &mut systems.damage_queue,
            &mut systems.transient_sprites,
        );

        // One real tick is enough: `Systems::reap_deployables` resolves a
        // whole chain to a fixpoint within a single step (see its doc), so
        // this does not need to wait several ticks for the second mine to
        // notice it was killed.
        let input = Input::default();
        game.tick(crate::tick::TICK_SECONDS, &input);

        assert!(
            game.registry()
                .world
                .get::<&ohl_combat::Health>(first_mine_entity)
                .is_err(),
            "the first tripmine's stand-in must be despawned: it is well \
             within the satchel's own blast radius"
        );
        assert!(
            game.registry()
                .world
                .get::<&ohl_combat::Health>(second_mine_entity)
                .is_err(),
            "the second tripmine's stand-in must be despawned too: its \
             blast damage comes from the *first* tripmine's detonation, \
             queued inside phase 9b itself, and must reach it through the \
             real step order rather than being silently discarded by \
             phase 10's monster-only drain"
        );
        assert_eq!(
            game.projectile_count(),
            0,
            "the satchel and both tripmines must all be gone within the \
             same step: the satchel by its own detonation, the two \
             tripmines each killed by the previous explosive's blast"
        );
    }
}
