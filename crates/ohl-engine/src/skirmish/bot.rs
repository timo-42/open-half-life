//! One bot: a second walking player with a brain.
//!
//! A [`Bot`] moves through `crate::systems::Systems::player_move` (the
//! human's own move, mover riding and pushing included), takes damage
//! through its own [`ohl_player::Player`] (HEV armour absorption, fall
//! damage, `trigger_hurt`), and fires through its own
//! `crate::combat::CombatState` (the human's own weapon state machine,
//! hitscan resolution and projectile commands). Only the decisions are its
//! own: [`Brain`] turns what the bot can see into the same held input a
//! keyboard would produce.
//!
//! Everything [`Brain`] decides with — reaction times, turn rates, aim
//! error, preferred engagement ranges, item values, strafe timing — is
//! project-authored tuning; no reviewed page describes how a deathmatch
//! opponent should play.

use glam::{Vec2, Vec3};
use ohl_combat::{AmmoType, DamageInfo, PickupKind, WeaponId, spec};
use ohl_game::hecs::Entity;
use ohl_game::registry::{BrushBounds, Target, TargetName, TeleportTrigger, Transform};
use ohl_physics::{CollisionModel, HULL_SIZES, Hull, PlayerController};
use ohl_player::Player;
use ohl_render::FreeFlyCamera;

use super::nav::{EdgeKind, NavGraph};
use super::{BotBody, BotSkill, SpawnPoint, sequence_named};
use crate::combat::{CombatState, PlayerProjectileCommand};
use crate::components::{Pickup, StudioAnim};
use crate::level::Level;
use crate::systems::{LatchedInput, QueuedDamage};

/// One living opponent as a bot sees it this step.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Opponent {
    /// Scoreboard slot.
    pub(crate) slot: usize,
    /// World entity.
    pub(crate) entity: Entity,
    /// Hull origin.
    pub(crate) origin: Vec3,
    /// Eye position.
    pub(crate) eye: Vec3,
    /// Whether it fired last step, which every bot within
    /// [`HEARING_RADIUS`] hears.
    pub(crate) firing: bool,
}

/// What one [`BotSkill`] level selects. Project-authored tuning.
#[derive(Debug, Clone, Copy)]
struct SkillParams {
    /// Seconds an opponent must have been in view before the bot fires.
    reaction: f32,
    /// Fastest the view turns, in degrees per second.
    turn_rate: f32,
    /// Largest random aim error, in degrees.
    aim_error: f32,
    /// How far off target (degrees, beyond the target's own angular size)
    /// the bot still pulls the trigger.
    fire_cone: f32,
    /// Whether it strafes while fighting.
    strafes: bool,
    /// Whether it hops while fighting.
    hops: bool,
}

impl SkillParams {
    fn of(skill: BotSkill) -> Self {
        match skill {
            BotSkill::Easy => Self {
                reaction: 0.75,
                turn_rate: 200.0,
                aim_error: 8.0,
                fire_cone: 10.0,
                strafes: false,
                hops: false,
            },
            BotSkill::Normal => Self {
                reaction: 0.4,
                turn_rate: 400.0,
                aim_error: 4.0,
                fire_cone: 7.0,
                strafes: true,
                hops: false,
            },
            BotSkill::Hard => Self {
                reaction: 0.2,
                turn_rate: 720.0,
                aim_error: 1.75,
                fire_cone: 5.0,
                strafes: true,
                hops: true,
            },
        }
    }
}

/// How often a bot re-checks what it can see, in seconds.
const SENSE_INTERVAL: f32 = 0.1;
/// How long a bot keeps hunting an opponent it lost sight of.
const MEMORY_SECONDS: f32 = 5.0;
/// Half of the bot's field of view, in degrees.
const VIEW_HALF_ANGLE: f32 = 75.0;
/// Inside this distance a bot notices an opponent whichever way it faces.
const AWARENESS_RADIUS: f32 = 192.0;
/// How long a hit keeps a bot aware of whoever landed it.
const ATTACKED_MEMORY_SECONDS: f32 = 2.0;
/// The most nodes one path search may expand.
const MAX_PATH_EXPANSIONS: usize = 30_000;
/// How long a goal is pursued before the bot picks another.
const GOAL_TIMEOUT_SECONDS: f32 = 20.0;
/// How long an item the bot could not reach is left off its list.
const BLACKLIST_SECONDS: f32 = 15.0;
/// Within this horizontal distance a path node counts as reached.
const NODE_REACHED: f32 = 16.0;
/// How long a bot may head for one path node before it hops, and before
/// it gives the path up as blocked.
const NODE_HOP_SECONDS: f32 = 1.5;
const NODE_GIVE_UP_SECONDS: f32 = 4.0;
/// How long a bot leaves an edge it failed to cross out of its searches,
/// and how many such edges it remembers at once.
const FAILED_EDGE_SECONDS: f32 = 60.0;
const MAX_FAILED_EDGES: usize = 64;
/// How far a bot hears gunfire it then goes to investigate.
const HEARING_RADIUS: f32 = 1600.0;
/// How far ahead the ledge probe looks before a combat move.
const LEDGE_PROBE: f32 = 24.0;

/// A target the bot is tracking.
#[derive(Debug, Clone, Copy)]
struct Enemy {
    slot: usize,
    origin: Vec3,
    eye: Vec3,
    visible: bool,
    visible_for: f32,
    unseen_for: f32,
}

/// What the bot is walking toward when it is not fighting.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Goal {
    Item(Entity),
    Roam,
    Chase,
}

/// One step's decided input.
// As `crate::Input`: independent buttons, decided together.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
struct Intent {
    /// Horizontal world direction to move in; zero to stand still.
    wish: Vec3,
    jump: bool,
    /// Held while airborne on a jump: tucking the legs in mid-air lifts
    /// the feet by the standing and crouched hulls' foot difference, the
    /// extra reach a jump edge's height bound assumes.
    duck: bool,
    attack: bool,
    reload: bool,
    select: Option<WeaponId>,
}

/// A bot's decision state. See the module docs.
pub(crate) struct Brain {
    skill: SkillParams,
    rng: ohl_ai::Pcg32,
    enemy: Option<Enemy>,
    sense_timer: f32,
    attacked_by: Option<(Entity, f32)>,
    path: Vec<u32>,
    cursor: usize,
    goal: Option<Goal>,
    goal_age: f32,
    blacklist: Vec<(Entity, f32)>,
    /// Edges this bot failed to cross, and for how much longer its path
    /// searches leave them out.
    failed_edges: Vec<((u32, u32), f32)>,
    progress_origin: Vec3,
    progress_timer: f32,
    stuck_for: f32,
    /// Seconds spent heading for the current path node.
    node_timer: f32,
    /// Seconds before another path search may run, after one failed.
    plan_cooldown: f32,
    strafe: f32,
    strafe_timer: f32,
    aim_offset: Vec2,
    hop_timer: f32,
    weapon_timer: f32,
    reload_timer: f32,
    intent: Intent,
}

impl Brain {
    fn new(skill: BotSkill, seed: u64) -> Self {
        let mut rng = ohl_ai::Pcg32::new(seed);
        // Bots join in the same step; staggered clocks keep their sensing
        // and planning from all landing on one step.
        let stagger = rng.range_f32(0.0, SENSE_INTERVAL);
        Self {
            skill: SkillParams::of(skill),
            rng,
            enemy: None,
            sense_timer: stagger,
            attacked_by: None,
            path: Vec::new(),
            cursor: 0,
            goal: None,
            goal_age: 0.0,
            blacklist: Vec::new(),
            failed_edges: Vec::new(),
            progress_origin: Vec3::ZERO,
            progress_timer: 0.0,
            stuck_for: 0.0,
            node_timer: 0.0,
            plan_cooldown: 0.0,
            strafe: 1.0,
            strafe_timer: 0.0,
            aim_offset: Vec2::ZERO,
            hop_timer: 0.0,
            weapon_timer: 0.0,
            reload_timer: 0.0,
            intent: Intent::default(),
        }
    }

    /// Forgets everything a death or a respawn invalidates; keeps the
    /// random stream and the skill.
    fn reset(&mut self, origin: Vec3) {
        self.enemy = None;
        self.sense_timer = 0.0;
        self.attacked_by = None;
        self.path.clear();
        self.cursor = 0;
        self.goal = None;
        self.goal_age = 0.0;
        self.progress_origin = origin;
        self.progress_timer = 0.0;
        self.stuck_for = 0.0;
        self.intent = Intent::default();
    }

    fn clear_path(&mut self) {
        self.path.clear();
        self.cursor = 0;
        self.node_timer = 0.0;
        self.goal = None;
        self.goal_age = 0.0;
    }
}

/// A bot. See the module docs.
pub(crate) struct Bot {
    pub(crate) entity: Entity,
    pub(crate) name: &'static str,
    controller: PlayerController,
    view: FreeFlyCamera,
    player: Player,
    combat: CombatState,
    /// The weapon code writes a HUD and presentation cues for whoever
    /// fires; a bot's are scratch, drained and dropped every step.
    hud: ohl_ui::hud::HudState,
    presentation: crate::presentation::Presentation,
    brain: Brain,
    /// Seconds since death, while dead.
    dead_for: Option<f32>,
    /// Set when this step's own movement (a fall, a hazard) killed it.
    world_death: bool,
    /// The teleport volumes the bot's hull overlapped last step, so only
    /// stepping *into* one carries it.
    touching_teleports: Vec<Entity>,
}

impl Bot {
    /// Creates bot `index`'s world entity (dead and out of sight until its
    /// first [`Self::respawn`]).
    pub(crate) fn new(
        level: &mut Level,
        index: u8,
        name: &'static str,
        skill: BotSkill,
        seed: u64,
        player_model: Option<usize>,
    ) -> Self {
        let entity = level.registry.world.spawn((
            BotBody {
                index,
                alive: false,
            },
            Transform {
                origin: Vec3::ZERO,
                angles: Vec3::ZERO,
            },
            ohl_combat::Health::new(crate::level::PLAYER_MAX_HEALTH),
            ohl_combat::Armor::empty(crate::level::PLAYER_MAX_ARMOR),
        ));
        if let Some(model) = player_model {
            let _ = level
                .registry
                .world
                .insert_one(entity, StudioAnim::new(model, 0));
        }
        Self {
            entity,
            name,
            controller: PlayerController::default(),
            view: FreeFlyCamera::default(),
            player: Player::default(),
            combat: CombatState::new(),
            hud: ohl_ui::hud::HudState::default(),
            presentation: crate::presentation::Presentation::new(),
            brain: Brain::new(skill, seed),
            dead_for: Some(super::BOT_RESPAWN_SECONDS),
            world_death: false,
            touching_teleports: Vec::new(),
        }
    }

    pub(crate) fn alive(&self) -> bool {
        self.dead_for.is_none()
    }

    pub(crate) fn origin(&self) -> Vec3 {
        self.controller.state.origin
    }

    pub(crate) fn eye(&self) -> Vec3 {
        self.controller.eye_position()
    }

    /// The weapon in hand, for the kill feed.
    pub(crate) fn selected_weapon(&self) -> Option<WeaponId> {
        self.combat.selected_weapon()
    }

    /// Puts the bot back into the match at `spawn`: full health, the
    /// deathmatch starting equipment, a fresh brain.
    pub(crate) fn respawn(&mut self, level: &mut Level, spawn: SpawnPoint) {
        self.controller = PlayerController::spawn_at(spawn.origin, spawn.yaw, 0.0);
        if let Some(collision) = level.collision.as_ref() {
            self.controller.settle_at_spawn(collision);
        }
        self.view = FreeFlyCamera::at_spawn(ohl_world::PlayerSpawn {
            origin: self.controller.state.origin.to_array(),
            yaw: spawn.yaw,
            pitch: 0.0,
        });
        self.view.position = self.controller.eye_position().to_array();
        self.player = Player::default();
        let mut events = Vec::new();
        self.player.equip_suit(&mut events);
        self.combat = CombatState::new();
        give_deathmatch_loadout(&mut self.combat);
        self.brain.reset(self.controller.state.origin);
        self.dead_for = None;
        self.world_death = false;
        self.touching_teleports.clear();
        let world = &mut level.registry.world;
        if let Ok(mut body) = world.get::<&mut BotBody>(self.entity) {
            body.alive = true;
        }
        if let Ok(mut health) = world.get::<&mut ohl_combat::Health>(self.entity) {
            health.current = self.player.state.health;
        }
        if let Ok(mut armor) = world.get::<&mut ohl_combat::Armor>(self.entity) {
            armor.current = self.player.state.armor;
        }
        self.sync_transform(level);
        self.animate(level);
    }

    /// Applies one hit through the bot's own [`Player`] (HEV armour
    /// absorbs it the same way it does the human's). Returns whether it
    /// killed the bot.
    pub(crate) fn take_damage(&mut self, level: &mut Level, info: &DamageInfo) -> bool {
        if !self.alive() {
            return false;
        }
        let mut events = Vec::new();
        self.player.apply_damage(
            info.amount,
            crate::damage_map::damage_kind_of(info.kind),
            &mut events,
        );
        self.mirror_health(level);
        if let Some(attacker) = info.attacker.and_then(crate::ids::entity_of)
            && attacker != self.entity
        {
            self.brain.attacked_by = Some((attacker, ATTACKED_MEMORY_SECONDS));
        }
        self.player.state.dead
    }

    fn mirror_health(&self, level: &mut Level) {
        let world = &mut level.registry.world;
        if let Ok(mut health) = world.get::<&mut ohl_combat::Health>(self.entity) {
            health.current = self.player.state.health.max(0.0);
        }
        if let Ok(mut armor) = world.get::<&mut ohl_combat::Armor>(self.entity) {
            armor.current = self.player.state.armor;
        }
    }

    /// The bot dies: it stops, plays a death sequence where it fell, and
    /// is no longer shootable until it respawns.
    pub(crate) fn die(&mut self, level: &mut Level) {
        if !self.alive() {
            return;
        }
        self.dead_for = Some(0.0);
        self.world_death = false;
        self.player.state.dead = true;
        self.controller.state.velocity = Vec3::ZERO;
        self.brain.reset(self.controller.state.origin);
        self.mirror_health(level);
        if let Ok(mut body) = level.registry.world.get::<&mut BotBody>(self.entity) {
            body.alive = false;
        }
        let sequence = level
            .registry
            .world
            .get::<&StudioAnim>(self.entity)
            .ok()
            .and_then(|anim| sequence_named(level, &anim, "die", true));
        if let Ok(mut anim) = level.registry.world.get::<&mut StudioAnim>(self.entity) {
            anim.sequence = sequence.unwrap_or(anim.sequence);
            anim.cycle = 0.0;
            anim.frame_rate = 1.0;
        }
    }

    /// Whether this step's own movement killed the bot (a fall, a hazard);
    /// the caller then calls [`Self::die`] and records the death.
    pub(crate) fn died_from_world(&self) -> bool {
        self.world_death
    }

    /// Counts a dead bot's wait; `true` once it should respawn.
    pub(crate) fn ready_to_respawn(&mut self, dt: f32) -> bool {
        let Some(dead_for) = self.dead_for.as_mut() else {
            return false;
        };
        *dead_for += dt;
        *dead_for >= super::BOT_RESPAWN_SECONDS
    }

    /// Moves the bot by `push` when the destination is clear (see
    /// `SkirmishState::separate`).
    pub(crate) fn nudge(&mut self, level: &mut Level, push: Vec3) {
        let Some(collision) = level.collision.as_ref() else {
            return;
        };
        let from = self.controller.state.origin;
        let trace = collision.trace(self.controller.state.hull(), from, from + push);
        if !trace.start_solid {
            self.controller.state.origin = trace.end_pos;
            self.sync_transform(level);
        }
    }

    /// A step with no decision: a living bot (the match is over) stands
    /// still under gravity; a dead one's body falls to the floor and stays
    /// there, its death sequence untouched.
    pub(crate) fn idle(&mut self, level: &mut Level, dt: f32) {
        self.brain.intent = Intent::default();
        if self.alive() {
            self.move_step(level, dt);
        } else if !self.controller.state.on_ground {
            let blocked = level.movers_blocked.len();
            crate::systems::Systems::player_move(
                level,
                &mut self.view,
                &mut self.controller,
                LatchedInput::default(),
                dt,
            );
            level.movers_blocked.truncate(blocked);
            self.sync_transform(level);
        }
    }

    /// The bot's own scratch presentation, drained so it never grows.
    fn drain_scratch(&mut self) {
        let _ = self.presentation.drain_events();
        let _ = self.presentation.bridge.drain_sounds().count();
        let _ = self.presentation.bridge.drain_viewmodel_actions().count();
    }

    fn sync_transform(&self, level: &mut Level) {
        if let Ok(mut transform) = level.registry.world.get::<&mut Transform>(self.entity) {
            transform.origin = self.controller.state.origin;
            transform.angles = Vec3::new(0.0, self.view.yaw, 0.0);
        }
    }

    /// Picks the sequence the bot's speed calls for.
    fn animate(&self, level: &mut Level) {
        let speed = self.controller.state.velocity.truncate().length();
        let name = if speed > 150.0 {
            "run"
        } else if speed > 10.0 {
            "walk"
        } else {
            "idle"
        };
        let sequence = level
            .registry
            .world
            .get::<&StudioAnim>(self.entity)
            .ok()
            .and_then(|anim| sequence_named(level, &anim, name, false));
        if let Some(sequence) = sequence
            && let Ok(mut anim) = level.registry.world.get::<&mut StudioAnim>(self.entity)
        {
            anim.play(sequence);
        }
    }

    /// Decides this step's input: who to fight, where to go, where to look.
    pub(crate) fn think(
        &mut self,
        level: &Level,
        nav: &NavGraph,
        opponents: &[Opponent],
        own_slot: usize,
        dt: f32,
    ) {
        let Some(collision) = level.collision.as_ref() else {
            self.brain.intent = Intent::default();
            return;
        };
        self.age_timers(dt);
        self.sense(collision, opponents, own_slot, dt);
        let mut intent = Intent::default();
        self.choose_weapon(&mut intent);
        let origin = self.origin();
        let fighting = self.brain.enemy.filter(|enemy| enemy.visible);
        let look = if let Some(enemy) = fighting {
            self.brain.clear_path();
            (intent.wish, intent.jump) = self.combat_move(collision, enemy);
            self.aim_point(enemy)
        } else {
            let heard = self.heard_gunfire(opponents, own_slot);
            if self.brain.plan_cooldown > 0.0 {
                // A search just failed; keep following whatever path is left.
            } else if let Some(enemy) = self.brain.enemy {
                if self.brain.goal != Some(Goal::Chase)
                    && !self.plan_to(nav, enemy.origin, Goal::Chase)
                {
                    if enemy.origin.distance(origin) < 96.0 {
                        // Standing where they were last seen: they are gone.
                        self.brain.enemy = None;
                    } else {
                        self.brain.plan_cooldown = 1.0;
                    }
                }
            } else if let Some(noise) = heard
                && self.brain.goal != Some(Goal::Chase)
            {
                if !self.plan_to(nav, noise, Goal::Chase) {
                    self.brain.plan_cooldown = 1.0;
                }
            } else if self.brain.goal.is_none() || self.brain.goal_age > GOAL_TIMEOUT_SECONDS {
                self.choose_goal(level, nav);
                if self.brain.goal.is_none() {
                    self.brain.plan_cooldown = 1.0;
                }
            }
            let (wish, jump, duck) = self.follow_path(nav, dt);
            intent.wish = wish;
            intent.jump = jump;
            intent.duck = duck;
            if self.brain.path.is_empty() {
                self.brain.goal = None;
            }
            if wish == Vec3::ZERO {
                self.brain
                    .enemy
                    .map_or(origin + self.forward() * 64.0, |enemy| enemy.eye)
            } else {
                self.eye() + wish * 128.0
            }
        };
        self.turn_toward(look, dt);
        if let Some(enemy) = fighting {
            intent.attack = self.should_fire(enemy);
        }
        self.check_progress(&mut intent, dt);
        self.reload_decision(&mut intent, fighting.is_some());
        self.brain.intent = intent;
    }

    /// The nearest gunfire within [`HEARING_RADIUS`], when the bot is fit
    /// enough to go and see (a hurt bot looks for health instead).
    fn heard_gunfire(&self, opponents: &[Opponent], own_slot: usize) -> Option<Vec3> {
        if self.player.state.health < 50.0 {
            return None;
        }
        let origin = self.origin();
        opponents
            .iter()
            .filter(|opponent| opponent.slot != own_slot && opponent.firing)
            .map(|opponent| (opponent.origin.distance(origin), opponent.origin))
            .filter(|(distance, _)| *distance < HEARING_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, position)| position)
    }

    /// Whether the bot pulled the trigger this step.
    pub(crate) fn firing(&self) -> bool {
        self.alive() && self.brain.intent.attack
    }

    fn forward(&self) -> Vec3 {
        let yaw = self.view.yaw.to_radians();
        Vec3::new(yaw.cos(), yaw.sin(), 0.0)
    }

    fn age_timers(&mut self, dt: f32) {
        let brain = &mut self.brain;
        brain.goal_age += dt;
        brain.plan_cooldown -= dt;
        brain.weapon_timer -= dt;
        brain.strafe_timer -= dt;
        brain.hop_timer -= dt;
        brain.reload_timer -= dt;
        brain.blacklist.retain_mut(|(_, left)| {
            *left -= dt;
            *left > 0.0
        });
        brain.failed_edges.retain_mut(|(_, left)| {
            *left -= dt;
            *left > 0.0
        });
        if let Some((_, left)) = brain.attacked_by.as_mut() {
            *left -= dt;
            if *left <= 0.0 {
                brain.attacked_by = None;
            }
        }
    }

    /// Re-checks every [`SENSE_INTERVAL`] who is in view, and keeps the
    /// tracked enemy's position current every step it stays visible.
    fn sense(
        &mut self,
        collision: &CollisionModel,
        opponents: &[Opponent],
        own_slot: usize,
        dt: f32,
    ) {
        let eye = self.eye();
        // Keep tracking the current enemy's live position between checks.
        if let Some(enemy) = self.brain.enemy.as_mut() {
            match opponents
                .iter()
                .find(|opponent| opponent.slot == enemy.slot)
            {
                Some(opponent) if enemy.visible => {
                    enemy.origin = opponent.origin;
                    enemy.eye = opponent.eye;
                    enemy.visible_for += dt;
                }
                Some(_) => enemy.unseen_for += dt,
                None => self.brain.enemy = None,
            }
        }
        if self
            .brain
            .enemy
            .is_some_and(|enemy| !enemy.visible && enemy.unseen_for > MEMORY_SECONDS)
        {
            self.brain.enemy = None;
        }
        self.brain.sense_timer -= dt;
        if self.brain.sense_timer > 0.0 {
            return;
        }
        self.brain.sense_timer = SENSE_INTERVAL;
        let forward = self.view_direction();
        let attacked_by = self.brain.attacked_by.map(|(entity, _)| entity);
        let mut best: Option<(f32, Opponent)> = None;
        let mut current_visible = false;
        for opponent in opponents {
            if opponent.slot == own_slot {
                continue;
            }
            let offset = opponent.eye - eye;
            let distance = offset.length();
            let noticed = distance < AWARENESS_RADIUS
                || attacked_by == Some(opponent.entity)
                || self
                    .brain
                    .enemy
                    .is_some_and(|enemy| enemy.slot == opponent.slot)
                || forward.angle_between(offset).to_degrees() <= VIEW_HALF_ANGLE;
            if !noticed || !line_of_sight(collision, eye, opponent) {
                continue;
            }
            if self
                .brain
                .enemy
                .is_some_and(|enemy| enemy.slot == opponent.slot)
            {
                current_visible = true;
            }
            // Whoever just shot the bot is preferred, then the nearest.
            let score = if attacked_by == Some(opponent.entity) {
                distance * 0.5
            } else {
                distance
            };
            if best.is_none_or(|(current, _)| score < current) {
                best = Some((score, *opponent));
            }
        }
        match (self.brain.enemy.as_mut(), best) {
            (Some(enemy), Some((score, candidate))) => {
                let current_distance = enemy.eye.distance(eye);
                if !current_visible
                    || (candidate.slot != enemy.slot && score < current_distance * 0.6)
                {
                    self.brain.enemy = Some(Enemy::sighted(candidate));
                } else {
                    enemy.visible = true;
                    enemy.unseen_for = 0.0;
                }
            }
            (Some(enemy), None) => {
                enemy.visible = false;
            }
            (None, Some((_, candidate))) => {
                self.brain.enemy = Some(Enemy::sighted(candidate));
            }
            (None, None) => {}
        }
    }

    fn view_direction(&self) -> Vec3 {
        let (yaw, pitch) = (self.view.yaw.to_radians(), self.view.pitch.to_radians());
        Vec3::new(
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            -pitch.sin(),
        )
    }

    /// Picks the weapon this distance calls for, reconsidered twice a
    /// second.
    fn choose_weapon(&mut self, intent: &mut Intent) {
        if self.brain.weapon_timer > 0.0 && !self.out_of_ammo(self.selected_weapon()) {
            return;
        }
        self.brain.weapon_timer = 0.5;
        let distance = self
            .brain
            .enemy
            .map_or(512.0, |enemy| enemy.origin.distance(self.origin()));
        let best = WeaponId::ALL
            .into_iter()
            .filter(|id| self.combat.owns(*id) && !self.out_of_ammo(Some(*id)))
            .filter_map(|id| weapon_score(id, distance).map(|score| (score, id)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id);
        if let Some(id) = best
            && Some(id) != self.selected_weapon()
        {
            intent.select = Some(id);
        }
    }

    fn out_of_ammo(&self, weapon: Option<WeaponId>) -> bool {
        let Some(weapon) = weapon else {
            return true;
        };
        let weapon_spec = spec(weapon);
        match weapon_spec.ammo {
            None => false,
            Some(kind) => self.combat.clip(weapon) == 0 && self.combat.reserve(kind) == 0,
        }
    }

    /// Where to aim at `enemy`: the chest, or the feet for a rocket (the
    /// blast still lands if the rocket misses), plus this skill's
    /// wandering aim error.
    fn aim_point(&mut self, enemy: Enemy) -> Vec3 {
        let base = match self.selected_weapon() {
            Some(WeaponId::Rpg) => enemy.origin - Vec3::Z * 24.0,
            _ => enemy.origin + Vec3::Z * 12.0,
        };
        let distance = base.distance(self.eye()).max(1.0);
        let error = self.brain.aim_offset * distance * std::f32::consts::PI / 180.0;
        let to = (base - self.eye()).normalize_or_zero();
        let right = Vec3::new(to.y, -to.x, 0.0).normalize_or_zero();
        base + right * error.x + Vec3::Z * error.y
    }

    /// Turns the view toward `point` at this skill's turn rate.
    fn turn_toward(&mut self, point: Vec3, dt: f32) {
        let offset = point - self.eye();
        if offset.length_squared() < 1.0 {
            return;
        }
        let desired_yaw = offset.y.atan2(offset.x).to_degrees();
        let desired_pitch = -offset
            .z
            .atan2(offset.truncate().length())
            .to_degrees()
            .clamp(-89.0, 89.0);
        let max_turn = self.brain.skill.turn_rate * dt;
        let yaw_delta = wrap_degrees(desired_yaw - self.view.yaw).clamp(-max_turn, max_turn);
        let pitch_delta = (desired_pitch - self.view.pitch).clamp(-max_turn, max_turn);
        self.view.yaw = wrap_degrees(self.view.yaw + yaw_delta);
        self.view.pitch = (self.view.pitch + pitch_delta).clamp(-89.0, 89.0);
    }

    /// Whether the view is close enough to `enemy` to pull the trigger.
    fn should_fire(&self, enemy: Enemy) -> bool {
        if enemy.visible_for < self.brain.skill.reaction {
            return false;
        }
        let Some(weapon) = self.selected_weapon() else {
            return false;
        };
        let target = enemy.origin + Vec3::Z * 12.0;
        let offset = target - self.eye();
        let distance = offset.length();
        if distance > max_range(weapon) || (weapon == WeaponId::Rpg && distance < 160.0) {
            return false;
        }
        // The target's own half-width, as an angle, widens the cone.
        let size = 16.0f32.atan2(distance.max(1.0)).to_degrees();
        self.view_direction().angle_between(offset).to_degrees()
            <= self.brain.skill.fire_cone + size
    }

    /// Strafes around `enemy` while keeping this weapon's preferred range,
    /// never stepping off a ledge to do it; also says whether to hop now
    /// (a skill that hops does so every few seconds).
    fn combat_move(&mut self, collision: &CollisionModel, enemy: Enemy) -> (Vec3, bool) {
        let to_enemy = (enemy.origin - self.origin()).truncate();
        let distance = to_enemy.length();
        let toward = if distance > 0.01 {
            (to_enemy / distance).extend(0.0)
        } else {
            Vec3::X
        };
        let (near, far) = preferred_range(self.selected_weapon());
        let approach = if distance > far {
            1.0
        } else if distance < near {
            -1.0
        } else {
            0.0
        };
        let brain = &mut self.brain;
        if brain.strafe_timer <= 0.0 {
            brain.strafe_timer = brain.rng.range_f32(0.4, 1.4);
            if brain.rng.below(3) > 0 {
                brain.strafe = -brain.strafe;
            }
            brain.aim_offset = Vec2::new(
                brain.rng.range_f32(-1.0, 1.0),
                brain.rng.range_f32(-1.0, 1.0),
            ) * brain.skill.aim_error;
        }
        let strafe = if brain.skill.strafes {
            brain.strafe
        } else {
            0.0
        };
        let right = Vec3::new(toward.y, -toward.x, 0.0);
        let mut wish = toward * approach + right * strafe;
        if wish != Vec3::ZERO && !safe_to_step(collision, self.origin(), wish.normalize()) {
            self.brain.strafe = -self.brain.strafe;
            wish = toward * approach;
            if wish != Vec3::ZERO && !safe_to_step(collision, self.origin(), wish.normalize()) {
                wish = Vec3::ZERO;
            }
        }
        let hop = self.brain.skill.hops && self.brain.hop_timer <= 0.0;
        if hop {
            self.brain.hop_timer = self.brain.rng.range_f32(1.5, 4.0);
        }
        (wish.normalize_or_zero(), hop)
    }

    /// Picks something worth walking to — an item this bot needs, or a
    /// random far corner of the graph — and plans a path to it.
    fn choose_goal(&mut self, level: &Level, nav: &NavGraph) {
        self.brain.clear_path();
        let origin = self.origin();
        let mut candidates: Vec<(f32, Entity, Vec3)> = Vec::new();
        for (entity, pickup, transform) in &mut level
            .registry
            .world
            .query::<(Entity, &Pickup, &Transform)>()
        {
            if pickup.taken
                || self
                    .brain
                    .blacklist
                    .iter()
                    .any(|(blocked, _)| *blocked == entity)
            {
                continue;
            }
            let value = self.item_value(pickup.kind);
            if value <= 0.0 {
                continue;
            }
            let score = value / (1.0 + transform.origin.distance(origin) / 600.0);
            candidates.push((score, entity, transform.origin));
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, entity, position) in candidates.into_iter().take(3) {
            if self.plan_to(nav, position + Vec3::Z * 36.0, Goal::Item(entity)) {
                return;
            }
            self.brain.blacklist.push((entity, BLACKLIST_SECONDS));
        }
        for _ in 0..4 {
            let Ok(count) = u32::try_from(nav.len()) else {
                return;
            };
            if count == 0 {
                return;
            }
            let pick = self.brain.rng.below(count);
            let Some(node) = nav.node(pick) else {
                continue;
            };
            if node.position.distance(origin) > 256.0
                && self.plan_to(nav, node.position, Goal::Roam)
            {
                return;
            }
        }
    }

    /// How much this bot wants an item of `kind` right now.
    fn item_value(&self, kind: PickupKind) -> f32 {
        let health = self.player.state.health;
        let armor = self.player.state.armor;
        match kind {
            PickupKind::Weapon(id) if !self.combat.owns(id) => match id {
                WeaponId::Rpg | WeaponId::Gauss | WeaponId::Egon => 4.0,
                WeaponId::Mp5 | WeaponId::Shotgun | WeaponId::Crossbow | WeaponId::Python => 3.0,
                WeaponId::HornetGun => 2.0,
                _ => 0.5,
            },
            PickupKind::Weapon(id) => spec(id).ammo.map_or(0.0, |ammo| self.ammo_need(ammo) * 1.5),
            PickupKind::Ammo(ammo) => {
                let used = WeaponId::ALL
                    .into_iter()
                    .any(|id| self.combat.owns(id) && spec(id).ammo == Some(ammo));
                if used {
                    self.ammo_need(ammo) * 2.0
                } else {
                    0.0
                }
            }
            PickupKind::HealthKit => {
                if health < 80.0 {
                    (100.0 - health) / 20.0
                } else {
                    0.0
                }
            }
            PickupKind::Battery => {
                if armor < 80.0 {
                    1.5
                } else {
                    0.0
                }
            }
            PickupKind::LongJump if !self.player.state.longjump_owned => 0.5,
            _ => 0.0,
        }
    }

    /// How empty `ammo`'s reserve is, `0.0` (full) to `1.0` (empty).
    fn ammo_need(&self, ammo: AmmoType) -> f32 {
        let capacity = ammo.default_capacity().max(1);
        let have = self.combat.reserve(ammo).min(capacity);
        #[allow(clippy::cast_precision_loss)]
        let need = 1.0 - have as f32 / capacity as f32;
        need
    }

    /// Plans a path from where the bot stands to the node nearest `to`.
    fn plan_to(&mut self, nav: &NavGraph, to: Vec3, goal: Goal) -> bool {
        let (Some(start), Some(end)) = (nav.nearest(self.origin()), nav.nearest(to)) else {
            return false;
        };
        if start == end {
            return false;
        }
        let mut avoid: Vec<(u32, u32)> = self
            .brain
            .failed_edges
            .iter()
            .map(|(edge, _)| *edge)
            .collect();
        avoid.sort_unstable();
        let Some(path) = nav.find_path(start, end, MAX_PATH_EXPANSIONS, &avoid) else {
            return false;
        };
        self.brain.path = path;
        self.brain.cursor = 0;
        self.brain.goal = Some(goal);
        self.brain.goal_age = 0.0;
        true
    }

    /// The direction toward the next path node, advancing past every node
    /// already reached, and whether the edge being crossed needs a jump.
    ///
    /// A bot that has slipped off its path — the next node is now well
    /// above a floor it fell to, or it was knocked far from it — or that
    /// has headed for one node for [`NODE_GIVE_UP_SECONDS`] gives the path
    /// up, so the caller plans again from wherever it now stands.
    fn follow_path(&mut self, nav: &NavGraph, dt: f32) -> (Vec3, bool, bool) {
        let origin = self.origin();
        while let Some(index) = self.brain.path.get(self.brain.cursor).copied() {
            let Some(node) = nav.node(index) else {
                self.brain.clear_path();
                return (Vec3::ZERO, false, false);
            };
            let offset = node.position - origin;
            let horizontal = offset.truncate().length();
            if horizontal < NODE_REACHED && offset.z.abs() < 52.0 {
                self.brain.cursor += 1;
                self.brain.node_timer = 0.0;
                continue;
            }
            let edge = self
                .brain
                .cursor
                .checked_sub(1)
                .and_then(|previous| nav.edge_between(self.brain.path[previous], index));
            let climbing = edge.is_some_and(|edge| edge.kind == EdgeKind::Jump);
            let fell_below = self.controller.state.on_ground && !climbing && offset.z > 40.0;
            self.brain.node_timer += dt;
            if fell_below || horizontal > 320.0 || self.brain.node_timer > NODE_GIVE_UP_SECONDS {
                if !fell_below
                    && let Some(previous) = self.brain.cursor.checked_sub(1)
                    && self.brain.failed_edges.len() < MAX_FAILED_EDGES
                {
                    self.brain
                        .failed_edges
                        .push(((self.brain.path[previous], index), FAILED_EDGE_SECONDS));
                }
                self.brain.clear_path();
                return (Vec3::ZERO, false, false);
            }
            let wish = offset.truncate().normalize_or_zero().extend(0.0);
            let takeoff = self.brain.cursor.checked_sub(1).is_some_and(|previous| {
                nav.node(self.brain.path[previous])
                    .is_some_and(|from| from.position.truncate().distance(origin.truncate()) < 48.0)
            });
            let hopping = climbing || self.brain.node_timer > NODE_HOP_SECONDS;
            let on_ground = self.controller.state.on_ground;
            let jump =
                on_ground && ((climbing && takeoff) || self.brain.node_timer > NODE_HOP_SECONDS);
            return (wish, jump, hopping && !on_ground);
        }
        self.brain.clear_path();
        (Vec3::ZERO, false, false)
    }

    /// Notices a bot that has stopped making progress: it hops first, and
    /// gives its goal up if hopping does not free it.
    fn check_progress(&mut self, intent: &mut Intent, dt: f32) {
        let brain = &mut self.brain;
        brain.progress_timer += dt;
        if brain.progress_timer < 0.5 {
            return;
        }
        brain.progress_timer = 0.0;
        let moved = self.controller.state.origin.distance(brain.progress_origin);
        brain.progress_origin = self.controller.state.origin;
        if intent.wish == Vec3::ZERO || moved > 12.0 {
            brain.stuck_for = 0.0;
            return;
        }
        brain.stuck_for += 0.5;
        if brain.stuck_for >= 1.0 {
            intent.jump = true;
            brain.strafe = -brain.strafe;
        }
        if brain.stuck_for >= 2.5 {
            brain.stuck_for = 0.0;
            if let Some(Goal::Item(entity)) = brain.goal {
                brain.blacklist.push((entity, BLACKLIST_SECONDS));
            }
            brain.clear_path();
        }
    }

    /// Reloads an empty weapon at once, and tops a partly empty one up
    /// whenever nothing is in view.
    fn reload_decision(&mut self, intent: &mut Intent, fighting: bool) {
        let Some(weapon) = self.selected_weapon() else {
            return;
        };
        let weapon_spec = spec(weapon);
        let (Some(clip_size), Some(ammo)) = (weapon_spec.clip_size, weapon_spec.ammo) else {
            return;
        };
        let clip = self.combat.clip(weapon);
        let reserve = self.combat.reserve(ammo);
        if reserve == 0 || self.combat.is_reloading() || self.brain.reload_timer > 0.0 {
            return;
        }
        if clip == 0 || (!fighting && clip < clip_size) {
            intent.reload = true;
            intent.attack = false;
            self.brain.reload_timer = 1.0;
        }
    }

    /// Moves the bot one step with its decided input, through the same
    /// move the human makes, then runs its own player systems (fall
    /// damage, hazards).
    pub(crate) fn move_step(&mut self, level: &mut Level, dt: f32) {
        let intent = self.brain.intent;
        let forward = self.forward();
        let right = Vec3::new(forward.y, -forward.x, 0.0);
        let axis = |value: f32| -> i8 {
            if value > 0.38 {
                1
            } else if value < -0.38 {
                -1
            } else {
                0
            }
        };
        let input = LatchedInput {
            forward: axis(intent.wish.dot(forward)),
            right: axis(intent.wish.dot(right)),
            jump: intent.jump,
            duck: intent.duck,
            ..LatchedInput::default()
        };
        // The human's own move: carried by movers, pushed out of doors.
        // Anything that move reports as "blocked by the player" is the
        // human's to report, not a bot's, so it is discarded here.
        let blocked = level.movers_blocked.len();
        crate::systems::Systems::player_move(
            level,
            &mut self.view,
            &mut self.controller,
            input,
            dt,
        );
        level.movers_blocked.truncate(blocked);
        self.player_systems(level, input, dt);
        self.sync_transform(level);
        self.animate(level);
    }

    /// The bot's own fall damage and hazard volumes.
    fn player_systems(&mut self, level: &Level, input: LatchedInput, dt: f32) {
        use ohl_player::PlayerSystems as _;
        let Some(collision) = level.collision.as_ref() else {
            return;
        };
        let physics = ohl_player::PhysicsOutput::from_move(
            &self.controller.state,
            &self.controller.config,
            &self.controller.last_move_events(),
        );
        let mut player_input = ohl_player::PlayerInput {
            flashlight_pressed: false,
            use_held: false,
            jump: input.jump,
            duck: input.duck,
            hurt: Vec::new(),
        };
        for (hurt, transform) in &mut level
            .registry
            .world
            .query::<(&ohl_game::TriggerHurt, &Transform)>()
        {
            if transform.origin.distance(physics.origin) <= crate::systems::TRIGGER_HURT_RADIUS {
                player_input.push_hurt(ohl_player::HurtInput::from_trigger_hurt(hurt));
            }
        }
        let was_dead = self.player.state.dead;
        let _ = self.player.tick(dt, &player_input, &physics, collision);
        if self.player.state.dead && !was_dead {
            self.world_death = true;
        }
    }

    /// Fires (or reloads, or switches) with this step's decision, through
    /// the human's own weapon code.
    pub(crate) fn fire(
        &mut self,
        level: &Level,
        hitboxes: &ohl_combat::HitboxIndex,
        damage_queue: &mut Vec<QueuedDamage>,
        dt: f32,
    ) -> Option<PlayerProjectileCommand> {
        let intent = self.brain.intent;
        if let Some(weapon) = intent.select {
            self.combat.select_weapon(weapon);
        }
        let input = LatchedInput {
            attack: intent.attack,
            reload_pressed: intent.reload,
            ..LatchedInput::default()
        };
        let command = self.combat.weapons(
            level,
            &self.controller,
            dt,
            input,
            self.entity,
            hitboxes,
            damage_queue,
            &mut self.hud,
            &mut self.presentation,
            0,
        );
        self.drain_scratch();
        command
    }

    /// Reports whether this step's projectile command was admitted.
    pub(crate) fn finish_projectile_command(&mut self, success: bool) {
        self.combat.finish_projectile_command(success);
    }

    /// Takes every untaken pickup within reach, the way the human does
    /// (`crate::pickups`' own grant rules and touch radius).
    pub(crate) fn touch_pickups(&mut self, level: &mut Level) {
        let origin = self.origin();
        let touched: Vec<(Entity, PickupKind)> = level
            .registry
            .world
            .query::<(Entity, &Pickup, &Transform)>()
            .iter()
            .filter(|(_, pickup, transform)| {
                !pickup.taken && crate::pickups::touches(origin, transform.origin)
            })
            .map(|(entity, pickup, _)| (entity, pickup.kind))
            .collect();
        for (entity, kind) in touched {
            let (inventory, ammo) = self.combat.inventory_and_ammo_mut();
            let taken = if kind == PickupKind::WeaponBox {
                crate::pickups::take_weapon_box(level, entity, ammo)
            } else {
                crate::pickups::apply_pickup(kind, inventory, ammo, &mut self.player)
            };
            if taken {
                if let Ok(mut pickup) = level.registry.world.get::<&mut Pickup>(entity) {
                    pickup.taken = true;
                }
                if matches!(self.brain.goal, Some(Goal::Item(goal)) if goal == entity) {
                    self.brain.clear_path();
                }
            }
        }
        self.mirror_health(level);
    }

    /// Opens the touch-opened doors the bot walks into, and carries it
    /// through any `trigger_teleport` it steps into.
    pub(crate) fn touch_world(&mut self, level: &mut Level) {
        let (mins, maxs) = HULL_SIZES[self.controller.state.hull().index()];
        let origin = self.origin();
        let (mins, maxs) = (
            origin + Vec3::from_array(mins),
            origin + Vec3::from_array(maxs),
        );
        let _ = level
            .simulation
            .touch_doors_by(&mut level.registry, Some(self.entity), mins, maxs);
        self.teleport(level, mins, maxs);
    }

    /// The bot's own `trigger_teleport` touch: stepping into an active
    /// volume moves it to the destination the volume's `target` names,
    /// facing the destination's own yaw (`docs/FORMAT_SOURCES.md`, the
    /// `trigger_teleport` entry the human's own teleport follows).
    fn teleport(&mut self, level: &mut Level, mins: Vec3, maxs: Vec3) {
        let overlapping: Vec<(Entity, Option<String>)> = level
            .registry
            .world
            .query::<(Entity, &TeleportTrigger, &BrushBounds, Option<&Target>)>()
            .iter()
            .filter(|(_, _, bounds, _)| overlaps(mins, maxs, bounds.mins, bounds.maxs))
            .map(|(entity, _, _, target)| (entity, target.map(|target| target.0.clone())))
            .collect();
        let entered = overlapping
            .iter()
            .find(|(entity, _)| !self.touching_teleports.contains(entity))
            .cloned();
        self.touching_teleports = overlapping.iter().map(|(entity, _)| *entity).collect();
        let Some((trigger, Some(target))) = entered else {
            return;
        };
        if !level.simulation.master_is_active(&level.registry, trigger) {
            return;
        }
        let destination = level
            .registry
            .world
            .query::<(&TargetName, &Transform)>()
            .iter()
            .find(|(name, _)| name.0 == target)
            .map(|(_, transform)| (transform.origin, transform.angles));
        let Some((origin, angles)) = destination else {
            return;
        };
        if !origin.is_finite() {
            return;
        }
        self.controller.state.origin = origin;
        self.controller.state.velocity = Vec3::ZERO;
        if angles.y.is_finite() {
            self.view.yaw = angles.y;
        }
        if let Some(collision) = level.collision.as_ref() {
            self.controller.settle_if_embedded(collision);
        }
        self.brain.clear_path();
        // Whatever volume the bot now stands in is already "touched", so
        // arriving is not itself a new step into one.
        let (hull_mins, hull_maxs) = HULL_SIZES[self.controller.state.hull().index()];
        let here = self.controller.state.origin;
        let (mins, maxs) = (
            here + Vec3::from_array(hull_mins),
            here + Vec3::from_array(hull_maxs),
        );
        self.touching_teleports = level
            .registry
            .world
            .query::<(Entity, &TeleportTrigger, &BrushBounds)>()
            .iter()
            .filter(|(_, _, bounds)| overlaps(mins, maxs, bounds.mins, bounds.maxs))
            .map(|(entity, _, _)| entity)
            .collect();
        self.sync_transform(level);
    }
}

impl Enemy {
    fn sighted(opponent: Opponent) -> Self {
        Self {
            slot: opponent.slot,
            origin: opponent.origin,
            eye: opponent.eye,
            visible: true,
            visible_for: 0.0,
            unseen_for: 0.0,
        }
    }
}

/// The deathmatch starting equipment: the HEV suit (the caller equips the
/// [`Player`]), a crowbar and a 9mm pistol ("giving you the HEV, crowbar
/// and pistol from the beginning", TWHL "Tutorial: Half-Life Deathmatch
/// mapping"). The pistol starts drawn and loaded with one pickup's worth
/// in reserve — project-authored, since no reviewed page gives the count.
pub(crate) fn give_deathmatch_loadout(combat: &mut CombatState) {
    let (inventory, ammo) = combat.inventory_and_ammo_mut();
    let _ = inventory.give_suit();
    inventory.give_weapon(WeaponId::Crowbar);
    inventory.give_weapon(WeaponId::Glock);
    let pistol = spec(WeaponId::Glock);
    if let Some(clip) = pistol.clip_size {
        inventory.set_clip(WeaponId::Glock, clip);
    }
    if let Some(kind) = pistol.ammo {
        ammo.add(kind, ohl_combat::weapon_pickup_ammo(WeaponId::Glock).value);
    }
    combat.select_weapon(WeaponId::Glock);
}

/// Whether nothing solid stands between `eye` and `opponent`'s eye or body.
fn line_of_sight(collision: &CollisionModel, eye: Vec3, opponent: &Opponent) -> bool {
    [opponent.eye, opponent.origin].into_iter().any(|point| {
        let trace = collision.trace(Hull::Point, eye, point);
        !trace.start_solid && trace.fraction >= 0.999
    })
}

/// Whether a step of [`LEDGE_PROBE`] units along `direction` from `origin`
/// keeps the bot on a floor no further than a safe fall below. A wall
/// counts as safe: walking into it hurts nobody.
fn safe_to_step(collision: &CollisionModel, origin: Vec3, direction: Vec3) -> bool {
    use crate::reachability::{EdgeOutcome, STEP_UP, try_edge};
    let safe = crate::route_plan::safe_drop_height(&ohl_physics::MoveConfig::default());
    match try_edge(
        collision,
        Hull::Standing,
        origin,
        direction,
        STEP_UP,
        LEDGE_PROBE,
    ) {
        EdgeOutcome::Landed { drop, .. } => drop <= safe,
        EdgeOutcome::BlockedAcross(_) | EdgeOutcome::BlockedUp => true,
        EdgeOutcome::NoFloor => false,
    }
}

/// How good `weapon` is at `distance`, or `None` when it should not be
/// used there at all. Project-authored tuning.
fn weapon_score(weapon: WeaponId, distance: f32) -> Option<f32> {
    let score = match weapon {
        WeaponId::Crowbar => {
            if distance < 64.0 {
                2.0
            } else {
                0.1
            }
        }
        WeaponId::Glock => 1.0,
        WeaponId::Python => 2.2,
        WeaponId::Mp5 => {
            if distance < 900.0 {
                2.5
            } else {
                1.2
            }
        }
        WeaponId::Shotgun => {
            if distance < 320.0 {
                3.5
            } else {
                0.8
            }
        }
        WeaponId::Crossbow => {
            if distance > 400.0 {
                3.0
            } else {
                1.0
            }
        }
        WeaponId::Rpg => {
            if distance > 220.0 {
                3.8
            } else {
                return None;
            }
        }
        WeaponId::Gauss => 3.2,
        WeaponId::Egon => {
            if distance < 500.0 {
                3.6
            } else {
                1.0
            }
        }
        WeaponId::HornetGun => 2.0,
        // Thrown and placed explosives need a plan a bot does not have.
        WeaponId::HandGrenade | WeaponId::Satchel | WeaponId::Tripmine | WeaponId::Snark => {
            return None;
        }
    };
    Some(score)
}

/// The furthest a bot fires `weapon` from. Project-authored.
fn max_range(weapon: WeaponId) -> f32 {
    match weapon {
        WeaponId::Crowbar => 64.0,
        WeaponId::Shotgun => 900.0,
        WeaponId::Egon => 800.0,
        _ => 3000.0,
    }
}

/// The distance band a bot tries to fight `weapon` from. Project-authored.
fn preferred_range(weapon: Option<WeaponId>) -> (f32, f32) {
    match weapon {
        Some(WeaponId::Crowbar) => (0.0, 40.0),
        Some(WeaponId::Shotgun | WeaponId::Egon) => (96.0, 280.0),
        Some(WeaponId::Rpg) => (320.0, 900.0),
        Some(WeaponId::Crossbow) => (384.0, 1400.0),
        _ => (160.0, 700.0),
    }
}

/// `angle` wrapped into `-180.0..180.0` degrees.
fn wrap_degrees(angle: f32) -> f32 {
    if !angle.is_finite() {
        return 0.0;
    }
    (angle + 180.0).rem_euclid(360.0) - 180.0
}

fn overlaps(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.cmple(b_max).all() && b_min.cmple(a_max).all()
}

#[cfg(test)]
mod tests {
    use super::{preferred_range, weapon_score, wrap_degrees};
    use ohl_combat::WeaponId;

    #[test]
    fn angles_wrap_into_one_turn() {
        assert!((wrap_degrees(190.0) + 170.0).abs() < 1e-4);
        assert!((wrap_degrees(-190.0) - 170.0).abs() < 1e-4);
        assert!(wrap_degrees(45.0).abs() - 45.0 < 1e-4);
        assert!(wrap_degrees(f32::NAN).abs() < f32::EPSILON);
    }

    #[test]
    fn a_rocket_is_never_chosen_at_point_blank_range() {
        assert!(weapon_score(WeaponId::Rpg, 100.0).is_none());
        assert!(weapon_score(WeaponId::Rpg, 600.0).is_some());
        assert!(weapon_score(WeaponId::Satchel, 600.0).is_none());
    }

    #[test]
    fn the_shotgun_beats_the_pistol_up_close_and_loses_far_away() {
        let close = |id| weapon_score(id, 150.0).unwrap();
        let far = |id| weapon_score(id, 1500.0).unwrap();
        assert!(close(WeaponId::Shotgun) > close(WeaponId::Glock));
        assert!(far(WeaponId::Shotgun) < far(WeaponId::Glock));
    }

    #[test]
    fn every_preferred_range_is_a_band() {
        for weapon in WeaponId::ALL {
            let (near, far) = preferred_range(Some(weapon));
            assert!(near < far);
        }
    }
}
