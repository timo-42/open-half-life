//! Turret input ownership, bounded aiming/cadence and outgoing shot values.
//!
//! Systems resolves controls before handheld weapons, poses both collision
//! models from `TankPose`, then dispatches each returned shot once. Combat uses
//! the shared projectile/blast owners; rendering borrows captured appearance.
//! Physical rocket owner remains the turret; operator attribution is separate.
//! See FORMAT_SOURCES, "Turret definitions and simulation", for published facts
//! versus project-authored policies and original-build TODO(black-box) limits.

#[cfg(test)]
pub(crate) mod live_tests;
mod runtime;
pub(crate) mod save;
#[cfg(test)]
mod save_tests;

pub(crate) use runtime::{LaserPulse, TankDispatch, TankFrame};

use glam::Vec3;
use ohl_combat::{DamageType, WeaponId, spec};
use ohl_game::hecs::Entity;
use ohl_game::registry::TriggerUse;
use ohl_game::tanks::{
    MAX_RANGE, TankBullet, TankControlIntent, TankDef, TankMemory, TankPose, TankState,
    TankVariant, wrap_degrees,
};

/// Project cap on eligible controls examined in one step.
pub(crate) const MAX_CONTROL_CANDIDATES: usize = 256;
/// Project use/release margin around a controls brush or remote mount location.
pub(crate) const CONTROL_MARGIN: f32 = 64.0;
/// Project bound on one simulation update; normal engine ticks are much shorter.
const MAX_STEP_SECONDS: f32 = 1.0;
/// Project rocket tuning, not a sourced turret-specific velocity/radius.
const ROCKET_SPEED: f32 = 1000.0;
const ROCKET_RADIUS: f32 = 250.0;

/// Current world bounds from the shared posed-brush adapter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlBounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl ControlBounds {
    fn permits(self, point: Vec3) -> bool {
        self.min.is_finite()
            && self.max.is_finite()
            && self.min.cmple(self.max).all()
            && point.distance_squared(point.clamp(self.min, self.max))
                <= CONTROL_MARGIN * CONTROL_MARGIN
    }
}

/// The host resolves names/existence/master using current registry state. Missing
/// tanks/controls are absent. Only local controls candidates carry world bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlCandidate {
    pub tank: Entity,
    pub controls: Option<Entity>,
    pub controllable: bool,
    pub master_open: bool,
    pub bounds: Option<ControlBounds>,
}

/// Real local player input, sampled before shake and before weapon phase 6.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlInput {
    pub player: Entity,
    pub position: Vec3,
    pub view_direction: Vec3,
    pub alive: bool,
    pub use_pressed: bool,
    pub attack: bool,
}

/// Mounted state uses runtime handles only. The tag44 adapter remaps them and
/// reruns candidate/master/liveness validation before restoring a claim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MountedTank {
    pub tank: Entity,
    pub controls: Option<Entity>,
    pub player: Entity,
    pub anchor: Vec3,
}

/// A claimed frame's aim/fire request, passed only to its named tank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlledAim {
    pub tank: Entity,
    pub player: Entity,
    pub direction: Vec3,
    pub attack: bool,
}

/// Input arbitration result; the same Use+Attack edge is claimed before weapons.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ControlDecision {
    pub consume_use: bool,
    pub suppress_weapons: bool,
    /// Successful new ownership requires explicit cancellation of charged,
    /// continuous and pending handheld actions BEFORE ordinary weapon ticking.
    /// Clearing attack buttons alone can release a charged Gauss. Cancellation
    /// preserves inventory/resources and already-spent ammo; it never refunds.
    /// Mounted restore must establish the same invariant before its first tick.
    pub cancel_handheld_actions: bool,
    pub controlled: Option<ControlledAim>,
}

/// One local controller and one pending remote request. Latest remote use wins
/// within a tick (project policy); no queue grows with repeated target cycles.
#[derive(Debug, Default)]
pub(crate) struct TankSystem {
    mounted: Option<MountedTank>,
    pending_remote: Option<TankControlIntent>,
    presentation: runtime::TankPresentation,
}

impl TankSystem {
    pub(crate) fn mounted(&self) -> Option<MountedTank> {
        self.mounted
    }

    pub(crate) fn queue_remote(&mut self, intent: TankControlIntent) {
        self.pending_remote = Some(intent);
    }

    /// Level changes clear all ownership; this does not move the player.
    pub(crate) fn clear(&mut self) {
        self.mounted = None;
        self.pending_remote = None;
    }

    /// `selected_controls` is the entity selected by the common use/proximity
    /// query. It never means "pick any tank" or a fabricated player activator.
    pub(crate) fn resolve_controls(
        &mut self,
        input: ControlInput,
        candidates: &[ControlCandidate],
        selected_controls: Option<Entity>,
    ) -> ControlDecision {
        let previously_mounted = self.mounted.is_some();
        let candidates = &candidates[..candidates.len().min(MAX_CONTROL_CANDIDATES)];
        if !input.alive || !input.position.is_finite() {
            self.clear();
            return ControlDecision {
                suppress_weapons: previously_mounted,
                ..ControlDecision::default()
            };
        }
        self.mounted = self.mounted.filter(|mount| {
            mount.player == input.player
                && candidates.iter().any(|candidate| {
                    candidate.tank == mount.tank
                        && candidate.controls == mount.controls
                        && candidate.controllable
                        && candidate.master_open
                        && match candidate.bounds {
                            Some(bounds) if mount.controls.is_some() => {
                                bounds.permits(input.position)
                            }
                            None if mount.controls.is_none() => {
                                input.position.distance_squared(mount.anchor)
                                    <= CONTROL_MARGIN * CONTROL_MARGIN
                            }
                            _ => false,
                        }
                })
        });
        let mut consume_use = input.use_pressed && previously_mounted;
        let mut cancel_handheld_actions = false;
        if input.use_pressed && previously_mounted {
            self.clear();
        } else if input.use_pressed {
            self.pending_remote = None;
            if let Some(candidate) = candidates.iter().find(|candidate| {
                candidate.controls.is_some()
                    && candidate.controls == selected_controls
                    && candidate.controllable
                    && candidate.master_open
                    && candidate
                        .bounds
                        .is_some_and(|bounds| bounds.permits(input.position))
            }) {
                self.mount(*candidate, input);
                consume_use = true;
                cancel_handheld_actions = true;
            }
        } else if let Some(intent) = self.pending_remote.take() {
            cancel_handheld_actions = self.apply_remote(intent, input, candidates);
        }
        let controlled = self.mounted.map(|mount| ControlledAim {
            tank: mount.tank,
            player: mount.player,
            direction: input.view_direction.normalize_or_zero(),
            attack: input.attack,
        });
        ControlDecision {
            consume_use,
            // The release tick also consumes held Attack, preventing an
            // accidental handheld shot while relinquishing the turret.
            suppress_weapons: previously_mounted || controlled.is_some(),
            cancel_handheld_actions,
            controlled,
        }
    }

    fn mount(&mut self, candidate: ControlCandidate, input: ControlInput) {
        self.mounted = Some(MountedTank {
            tank: candidate.tank,
            controls: candidate.controls,
            player: input.player,
            anchor: input.position,
        });
    }

    fn apply_remote(
        &mut self,
        intent: TankControlIntent,
        input: ControlInput,
        candidates: &[ControlCandidate],
    ) -> bool {
        if intent.player != input.player {
            return false;
        }
        let current = self.mounted.is_some_and(|mount| mount.tank == intent.tank);
        if intent.use_type == TriggerUse::Off || (intent.use_type == TriggerUse::Toggle && current)
        {
            if current {
                self.mounted = None;
            }
            return false;
        }
        if current {
            // Repeated On is idempotent: keep the existing controls area and
            // do not restart ownership or passive handheld cooldowns.
            return false;
        }
        if let Some(candidate) = candidates.iter().find(|candidate| {
            candidate.tank == intent.tank
                && candidate.controls.is_none()
                && candidate.controllable
                && candidate.master_open
        }) {
            self.mount(*candidate, input);
            return true;
        }
        false
    }
}

/// The only autonomous target type: the current local player, never an NPC.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayerTarget {
    pub entity: Entity,
    pub eye: Vec3,
    pub alive: bool,
}

/// Two different queries: visibility to the player and whether the actual
/// posed barrel ray reaches them. Each must ignore only the turret's own brush,
/// leaving all intervening world/mover geometry in the query.
pub(crate) trait TankQueries {
    fn sees_player(&self, turret: Entity, from: Vec3, player: PlayerTarget) -> bool;
    fn barrel_reaches_player(
        &self,
        turret: Entity,
        muzzle: Vec3,
        direction: Vec3,
        range: f32,
        player: PlayerTarget,
    ) -> bool;
}

/// Current simulation context; no renderer/camera-shake state is accepted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TankTick {
    pub tank: Entity,
    pub placement_origin: Vec3,
    pub pivot_local: Vec3,
    pub dt: f32,
    pub master_open: bool,
    pub player: Option<PlayerTarget>,
    pub controlled: Option<ControlledAim>,
}

/// A resolved shot, dispatched once by Systems before damage/lifecycle. Rockets
/// use P1 physics; mortar is a trace-to-impact blast, never an airborne surrogate.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TankShotKind {
    Bullet {
        bullet: TankBullet,
        damage: f32,
    },
    Rocket {
        damage: f32,
        speed: f32,
        radius: f32,
    },
    Laser {
        damage: f32,
        appearance: Option<String>,
    },
    Mortar {
        magnitude: f32,
    },
}

impl TankShotKind {
    /// Project damage classifications, not additional source claims.
    pub(crate) fn damage_type(&self) -> DamageType {
        match self {
            Self::Bullet { .. } => DamageType::BULLET,
            Self::Rocket { .. } | Self::Mortar { .. } => DamageType::BLAST,
            Self::Laser { .. } => DamageType::ENERGYBEAM,
        }
    }
}

/// Immutable outgoing command. The source is physical owner/inflictor; attacker
/// is the real operator when controlled, otherwise source. The dispatcher fires
/// `target` exactly once after shot admission (None bullets are admitted visual
/// shots); a failed rocket-capacity admission fires no target and refunds no ammo.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TankShot {
    pub source: Entity,
    pub attacker: Entity,
    pub muzzle: Vec3,
    pub direction: Vec3,
    pub range: f32,
    pub kind: TankShotKind,
    pub target: Option<String>,
}

/// Advance one turret and return at most one admitted-cadence command. Missing
/// laser/sprite assets do not decide whether the combat command exists.
#[cfg(test)]
pub(crate) fn advance_tank(
    def: &TankDef,
    state: &mut TankState,
    tick: TankTick,
    queries: &impl TankQueries,
) -> Option<TankShot> {
    let prepared = prepare_tank(def, state, tick, queries)?;
    finish_tank(def, state, tick, prepared, queries)
}

#[derive(Clone, Copy)]
struct PreparedAim {
    dt: f32,
    pitch: f32,
    yaw: f32,
    controlled: Option<ControlledAim>,
}

/// Aim all turrets first, then publish their poses to both collision models
/// before `finish_tank` performs actual-barrel eligibility and shot tracing.
fn prepare_tank(
    def: &TankDef,
    state: &mut TankState,
    tick: TankTick,
    queries: &impl TankQueries,
) -> Option<PreparedAim> {
    if !def.is_valid() || !tick.dt.is_finite() || tick.dt <= 0.0 {
        return None;
    }
    let dt = tick.dt.min(MAX_STEP_SECONDS);
    state.sanitize(def);
    if let Some(memory) = &mut state.memory {
        memory.remaining -= dt;
        if memory.remaining <= 0.0 {
            state.memory = None;
        }
    }
    let pose = TankPose::new(def, state, tick.placement_origin, tick.pivot_local)?;
    if !pose.muzzle(def).is_finite() {
        return None;
    }
    let controlled = tick.controlled.filter(|aim| {
        aim.tank == tick.tank
            && tick
                .player
                .is_some_and(|player| player.alive && player.entity == aim.player)
    });
    let desired = if tick.master_open {
        desired_direction(def, state, tick, pose, controlled, queries)
    } else {
        state.memory = None;
        None
    };
    let Some(direction) = desired else {
        idle_cadence(state, dt);
        return None;
    };
    let (pitch, yaw) = requested_angles(def, state, direction);
    state.relative_pitch = approach_angle(
        state.relative_pitch,
        pitch.clamp(-def.pitch_range, def.pitch_range),
        def.pitch_rate * dt,
        def.pitch_range,
    );
    state.relative_yaw = approach_angle(
        state.relative_yaw,
        yaw.clamp(-def.yaw_range, def.yaw_range),
        def.yaw_rate * dt,
        def.yaw_range,
    );
    Some(PreparedAim {
        dt,
        pitch,
        yaw,
        controlled,
    })
}

fn finish_tank(
    def: &TankDef,
    state: &mut TankState,
    tick: TankTick,
    prepared: PreparedAim,
    queries: &impl TankQueries,
) -> Option<TankShot> {
    let PreparedAim {
        dt,
        pitch,
        yaw,
        controlled,
    } = prepared;
    let pose = TankPose::new(def, state, tick.placement_origin, tick.pivot_local)?;
    let muzzle = pose.muzzle(def);
    if !muzzle.is_finite() {
        return None;
    }
    let wants_fire = controlled.map_or_else(
        || {
            if def.only_direct {
                tick.player.is_some_and(|player| {
                    let distance = pose.pivot_world().distance(player.eye);
                    player.alive
                        && distance >= def.min_range
                        && distance <= def.max_range
                        && queries.barrel_reaches_player(
                            tick.tank,
                            muzzle,
                            pose.forward(),
                            def.max_range,
                            player,
                        )
                })
            } else {
                wrap_degrees(pitch - state.relative_pitch).abs() <= def.pitch_tolerance
                    && wrap_degrees(yaw - state.relative_yaw).abs() <= def.yaw_tolerance
            }
        },
        |aim| aim.attack,
    );
    if !wants_fire || def.fire_rate <= 0.0 {
        idle_cadence(state, dt);
        return None;
    }
    if !cadence_due(state, def.fire_rate, dt) {
        return None;
    }
    Some(TankShot {
        source: tick.tank,
        attacker: controlled.map_or(tick.tank, |aim| aim.player),
        muzzle,
        direction: spread_direction(state, pose.forward(), def.spread),
        // Published min/max target ranges apply to automatic targeting. A
        // mounted operator retains the project trace ceiling instead.
        range: if controlled.is_some() {
            MAX_RANGE
        } else {
            def.max_range
        },
        kind: shot_kind(def),
        target: def.target.clone(),
    })
}

fn shot_kind(def: &TankDef) -> TankShotKind {
    match def.variant {
        TankVariant::Bullets => TankShotKind::Bullet {
            bullet: def.bullet,
            damage: if def.bullet == TankBullet::None {
                0.0
            } else {
                def.damage
            },
        },
        // Sven documents per-shot damage but no rocket-specific default.
        // Reusing RPG only for an omitted key is project policy; explicit
        // zero remains harmless. TODO(black-box): original 929 default.
        TankVariant::Rocket => TankShotKind::Rocket {
            damage: if def.damage_key_present {
                def.damage
            } else {
                spec(WeaponId::Rpg).damage
            },
            speed: ROCKET_SPEED,
            radius: ROCKET_RADIUS,
        },
        TankVariant::Laser => TankShotKind::Laser {
            damage: def.damage,
            appearance: def.laser_entity.clone(),
        },
        TankVariant::Mortar => TankShotKind::Mortar {
            magnitude: def.magnitude,
        },
    }
}

fn desired_direction(
    def: &TankDef,
    state: &mut TankState,
    tick: TankTick,
    pose: TankPose,
    controlled: Option<ControlledAim>,
    queries: &impl TankQueries,
) -> Option<Vec3> {
    if let Some(aim) = controlled {
        let direction = aim.direction.normalize_or_zero();
        return (direction.is_finite() && direction.length_squared() > 0.5).then_some(direction);
    }
    if !state.active {
        state.memory = None;
        return None;
    }
    let Some(player) = tick.player.filter(|p| p.alive && p.eye.is_finite()) else {
        state.memory = None;
        return None;
    };
    let distance = pose.pivot_world().distance(player.eye);
    let seen = distance >= def.min_range
        && distance <= def.max_range
        && queries.sees_player(tick.tank, pose.pivot_world(), player);
    let point = if seen {
        state.memory = Some(TankMemory {
            point: player.eye,
            remaining: def.persistence,
        });
        player.eye
    } else {
        state.memory?.point
    };
    let direction = (point - pose.muzzle(def)).normalize_or_zero();
    (direction.is_finite() && direction.length_squared() > 0.5).then_some(direction)
}

fn idle_cadence(state: &mut TankState, dt: f32) {
    // Cooldown survives tapping, but unused time cannot accrue a firing burst.
    state.shot_wait = (state.shot_wait - f64::from(dt)).max(0.0);
}

fn cadence_due(state: &mut TankState, rate: f32, dt: f32) -> bool {
    let interval = 1.0 / f64::from(rate);
    let dt = f64::from(dt);
    let epsilon = 1.0e-7_f64.min(interval * 1.0e-5);
    // A deadline exactly at the interval's end belongs to the next call.
    // Snap floating-point boundary noise without manufacturing a second pulse.
    let due = state.shot_wait <= epsilon || state.shot_wait + epsilon < dt;
    if due {
        state.shot_wait += interval;
    }
    state.shot_wait -= dt;
    if state.shot_wait < 0.0 {
        let remainder = (-state.shot_wait) % interval;
        state.shot_wait = if remainder <= epsilon || interval - remainder <= epsilon {
            0.0
        } else {
            // At most one shot in this call. Skip missed whole deadlines and
            // retain only the phase leading to the next future deadline.
            interval - remainder
        };
    }
    due
}

fn requested_angles(def: &TankDef, state: &TankState, direction: Vec3) -> (f32, f32) {
    let horizontal = direction.x.hypot(direction.y);
    let pitch = (-direction.z).atan2(horizontal).to_degrees();
    // Yaw is indeterminate at a vertical pole. Keep the current mechanical
    // yaw instead of introducing an atan2(0, 0) turn or a 180-degree branch flip.
    let yaw = if horizontal <= 1.0e-6 {
        def.authored_angles.y + state.relative_yaw
    } else {
        direction.y.atan2(direction.x).to_degrees()
    };
    let a = (
        wrap_degrees(pitch - def.authored_angles.x),
        wrap_degrees(yaw - def.authored_angles.y),
    );
    let b = (
        wrap_degrees(180.0 - pitch - def.authored_angles.x),
        wrap_degrees(yaw + 180.0 - def.authored_angles.y),
    );
    if aim_score(def, state, direction, a) <= aim_score(def, state, direction, b) {
        a
    } else {
        b
    }
}

/// Prefer a reachable Euler representation, then travel time and continuity
/// from the live pose. If both are unreachable, prefer the clamped endpoint
/// closest to the desired ray. All ranking/epsilon choices are project policy.
fn aim_score(
    def: &TankDef,
    state: &TankState,
    direction: Vec3,
    candidate: (f32, f32),
) -> (bool, f32, f64, f32) {
    const EPSILON_DEGREES: f32 = 1.0e-4;
    let pitch_delta = angle_delta(state.relative_pitch, candidate.0, def.pitch_range).abs();
    let yaw_delta = angle_delta(state.relative_yaw, candidate.1, def.yaw_range).abs();
    let reachable = candidate.0.abs() <= def.pitch_range + EPSILON_DEGREES
        && candidate.1.abs() <= def.yaw_range + EPSILON_DEGREES
        && (def.pitch_rate > 0.0 || pitch_delta <= EPSILON_DEGREES)
        && (def.yaw_rate > 0.0 || yaw_delta <= EPSILON_DEGREES);
    let endpoint_pitch = if def.pitch_rate > 0.0 {
        candidate.0.clamp(-def.pitch_range, def.pitch_range)
    } else {
        state.relative_pitch
    };
    let endpoint_yaw = if def.yaw_rate > 0.0 {
        candidate.1.clamp(-def.yaw_range, def.yaw_range)
    } else {
        state.relative_yaw
    };
    let pitch_travel = angle_delta(state.relative_pitch, endpoint_pitch, def.pitch_range).abs();
    let yaw_travel = angle_delta(state.relative_yaw, endpoint_yaw, def.yaw_range).abs();
    let seconds = |travel, rate| {
        if rate > 0.0 {
            f64::from(travel) / f64::from(rate)
        } else {
            0.0
        }
    };
    let (pitch_sin, pitch_cos) = (def.authored_angles.x + endpoint_pitch)
        .to_radians()
        .sin_cos();
    let (yaw_sin, yaw_cos) = (def.authored_angles.y + endpoint_yaw)
        .to_radians()
        .sin_cos();
    let endpoint_direction = Vec3::new(pitch_cos * yaw_cos, pitch_cos * yaw_sin, -pitch_sin);
    let blocked_error = if reachable {
        0.0
    } else {
        1.0 - endpoint_direction.dot(direction).clamp(-1.0, 1.0)
    };
    (
        !reachable,
        blocked_error,
        seconds(pitch_travel, def.pitch_rate).max(seconds(yaw_travel, def.yaw_rate)),
        pitch_travel + yaw_travel,
    )
}

fn angle_delta(current: f32, desired: f32, range: f32) -> f32 {
    if range >= 180.0 {
        wrap_degrees(desired - current)
    } else {
        desired - current
    }
}

fn approach_angle(current: f32, desired: f32, step: f32, range: f32) -> f32 {
    let next = current + angle_delta(current, desired, range).clamp(-step, step);
    if range >= 180.0 {
        wrap_degrees(next)
    } else {
        next
    }
}

fn spread_direction(state: &mut TankState, forward: Vec3, spread: u8) -> Vec3 {
    // TWHL defines five accuracy ordinals, not cone angles/distribution.
    // This disk distribution and angle table are project policy, TODO(black-box).
    const CONES: [f32; 5] = [0.0, 1.0, 3.0, 6.0, 10.0];
    let cone = CONES[usize::from(spread.min(4))].to_radians();
    if cone <= 0.0 {
        return forward;
    }
    let radius = random_unit(state).sqrt() * cone.tan();
    let angle = random_unit(state) * std::f32::consts::TAU;
    let reference = if forward.z.abs() < 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let right = forward.cross(reference).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    (forward + radius * (angle.cos() * right + angle.sin() * up)).normalize_or_zero()
}

#[allow(clippy::cast_precision_loss)]
fn random_unit(state: &mut TankState) -> f32 {
    let mut value = state.rng.max(1);
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    state.rng = value;
    // Upper 24 bits are exactly representable in f32, always below one.
    (value >> 40) as f32 / 16_777_216.0
}

#[cfg(test)]
mod tests;
