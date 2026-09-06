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
        }
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
        self.lifecycle(level, dt); // 10
        self.pickups(level, input, dt); // 11
        Self::triggers_and_movers(level, camera, input, dt, events); // 12
        self.presentation(level, dt); // 13
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
    /// Model-backed projectiles/deployables (a flying rocket, a placed
    /// tripmine) are excluded, so a projectile's own drawn model cannot
    /// stop its own sweep in phase 7.
    fn rebuild_hitbox_index(&mut self, level: &mut Level) {
        let exclude: Vec<Entity> = self.projectiles.model_entities().collect();
        crate::combat::rebuild_hitbox_index(&mut self.hitboxes, level, &exclude);
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
        );
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
    /// the host reads after the step; also ages the transient-sprite list
    /// and the view model's own animation cursor (phase 7 no longer does,
    /// now that this phase has landed).
    fn presentation(&mut self, level: &mut Level, dt: f32) {
        crate::combat::sync_player_components(level, &self.player);
        let events = std::mem::take(&mut self.player_events);
        self.presentation
            .tick(dt, &mut self.hud, &self.player, events, &mut self.view_model);
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
}
