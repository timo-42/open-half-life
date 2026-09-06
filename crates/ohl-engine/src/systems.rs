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
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_game::{Event, find_usable_within};
use ohl_physics::{ControllerInput, PlayerController};
use ohl_render::{FreeFlyCamera, MoveInput};

use crate::USE_RADIUS;
use crate::components::StudioAnim;
use crate::input::Input;
use crate::level::Level;
use crate::projectiles::ProjectileSystem;
use crate::sprites::TransientSprites;
use crate::viewmodel::ViewModel;

/// The project's default random seed.
///
/// A constant, not a clock or an environment read: two games built from the
/// same map bytes with the same config must step identically. It is saved
/// and restored with the rest of the simulation state.
pub const DEFAULT_RNG_SEED: u64 = 0x4F48_4C5F_5039_0001;

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
    /// The HUD the host draws. Written by the presentation phase; a default
    /// until the gameplay bridge is wired in.
    hud: ohl_ui::hud::HudState,
    /// Phase 9's queue: every [`ohl_combat::DamageInfo`] a weapon
    /// (phase 6), a projectile or blast (phase 7) or an AI attack
    /// (phase 8) produced this step, drained once damage resolution runs.
    /// This is the shared field `docs/m79-design.md` §8 asks P1 and P3 to
    /// agree on by name and type; P3 (this package) is the first to create
    /// it, since it lands without P1 in this tree.
    pub(crate) damage_queue: Vec<ohl_combat::DamageInfo>,
    /// Phase 7's own state: live projectiles and placed deployables.
    projectiles: ProjectileSystem,
    /// The bounded transient-sprite list phase 7 (and, later, phase 6's
    /// muzzle flashes) fills; phase 13 ages it.
    transient_sprites: TransientSprites,
    /// The first-person view model's animation state.
    view_model: ViewModel,
}

impl Systems {
    /// The step list with `config`'s seed and an empty HUD.
    #[must_use]
    pub fn new(config: SystemsConfig) -> Self {
        Self {
            config,
            frame_input: Input::default(),
            pending_edges: PendingEdges::default(),
            hud: ohl_ui::hud::HudState::default(),
            damage_queue: Vec::new(),
            projectiles: ProjectileSystem::new(config.rng_seed),
            transient_sprites: TransientSprites::new(),
            view_model: ViewModel::new(),
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
    /// the configuration.
    pub(crate) fn reset(&mut self) {
        let config = self.config;
        *self = Self::new(config);
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
        self.player_systems(dt); // 3
        Self::actor_sync(level, camera, controller, dt); // 4
        self.rebuild_hitbox_index(level); // 5
        self.weapons(dt); // 6
        self.projectiles(level, dt); // 7
        self.ai_think(level, dt); // 8
        self.resolve_damage(); // 9
        self.lifecycle(dt); // 10
        self.pickups(level, input); // 11
        Self::triggers_and_movers(level, camera, input, dt, events); // 12
        self.presentation(dt); // 13
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
    /// queue. See `crate::projectiles`. Also ages the transient-sprite list
    /// and the view model's animation cursor: neither phase 6 (weapons,
    /// still an empty hook) nor phase 13 (presentation, likewise) has
    /// landed a body yet in this tree, and both are this package's own
    /// state, so ageing them here rather than leaving them frozen keeps
    /// `Game::viewmodel_visible` and the sprite cap meaningful standalone.
    fn projectiles(&mut self, level: &mut Level, dt: f32) {
        self.projectiles.tick(
            level,
            dt,
            &mut self.damage_queue,
            &mut self.transient_sprites,
        );
        self.transient_sprites.tick(dt);
        self.view_model.tick(dt);
    }

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
