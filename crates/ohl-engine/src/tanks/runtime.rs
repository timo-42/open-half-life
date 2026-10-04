//! Registry, collision and combat adapters for the pure turret state machine.
//! All presentation records are simulation-aged and damage-free.

use std::collections::BTreeMap;

use ohl_combat::{AttackTrace, DamageInfo, HitboxIndex, ProjectileKind, TraceFilter, TraceMask};
use ohl_game::effects::{ExplosionVisuals, MapBlastProfile, effective_render_props};
use ohl_game::keyvalues::RenderProps;
use ohl_game::registry::{BrushBounds, Transform};
use ohl_game::tanks::TankControls;
use ohl_physics::BrushId;

use super::{
    ControlBounds, ControlCandidate, ControlDecision, ControlInput, ControlledAim, Entity,
    MAX_STEP_SECONDS, MountedTank, PlayerTarget, TankBullet, TankControlIntent, TankDef,
    TankQueries, TankShot, TankShotKind, TankState, TankSystem, TankTick, TriggerUse, Vec3,
    finish_tank, prepare_tank,
};
use crate::ids::{entity_id, entity_of};
use crate::level::Level;
use crate::map_effects::{BlastRequest, MapEffectsRuntime};
use crate::projectiles::ProjectileSystem;
use crate::systems::QueuedDamage;

/// Project bounds on runtime work and transient appearance; not map-format limits.
pub(crate) const MAX_TANKS: usize = 256;
const MAX_LASER_PULSES: usize = 128;
const LASER_PULSE_SECONDS: f32 = 0.1;

/// Captured at an accepted shot, never retraced or aged by rendering. Cosmetic
/// pulses clear on restore; they are deliberately absent from tags 43 and 44.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LaserPulse {
    pub start: Vec3,
    pub end: Vec3,
    pub width: f32,
    pub color: [f32; 4],
    pub age: f32,
}

#[derive(Debug)]
struct LaserStyle {
    entity: Entity,
    width: f32,
    render: RenderProps,
}

#[derive(Debug, Default)]
pub(super) struct TankPresentation {
    styles: BTreeMap<String, LaserStyle>,
    pulses: Vec<LaserPulse>,
}

/// Actual unshaken input sampled by Systems before ordinary weapons.
#[derive(Clone, Copy)]
pub(crate) struct TankFrame {
    pub input: ControlInput,
    pub eye: Vec3,
    pub dt: f32,
}

/// The existing damage/projectile/effect owners, borrowed for one dispatch.
pub(crate) struct TankDispatch<'a> {
    pub projectiles: &'a mut ProjectileSystem,
    pub effects: &'a mut MapEffectsRuntime,
    pub damage: &'a mut Vec<QueuedDamage>,
}

impl TankSystem {
    pub(crate) fn configure_level(&mut self, level: &Level) {
        self.clear();
        self.presentation = TankPresentation::default();
        for entity in tank_entities(level) {
            let Ok(def) = level.registry.world.get::<&TankDef>(entity) else {
                continue;
            };
            let Some(name) = &def.laser_entity else {
                continue;
            };
            if let Some(style) = laser_style(level, name) {
                self.presentation.styles.insert(name.clone(), style);
            }
        }
    }

    pub(crate) fn laser_pulses(&self) -> &[LaserPulse] {
        &self.presentation.pulses
    }

    /// Controls first; aim all tanks, synchronize BOTH exact brush instances,
    /// then decide actual-barrel eligibility and produce outgoing shots.
    pub(crate) fn advance(
        &mut self,
        level: &mut Level,
        hitboxes: &HitboxIndex,
        frame: TankFrame,
    ) -> (ControlDecision, Vec<TankShot>) {
        self.presentation.advance(frame.dt);
        if let Some(intent) = level.simulation.take_tank_control_intent() {
            self.queue_remote(intent);
        }
        let selected = frame
            .input
            .use_pressed
            .then(|| ohl_game::find_usable_within(&level.registry, frame.eye, crate::USE_RADIUS))
            .flatten();
        let candidates = control_candidates(level, self.mounted, self.pending_remote, selected);
        let decision = self.resolve_controls(frame.input, &candidates, selected);
        let mut prepared = Vec::new();
        for entity in tank_entities(level) {
            let Some(mut work) = TankWork::read(level, entity, frame, decision.controlled) else {
                continue;
            };
            let aim = prepare_tank(
                &work.def,
                &mut work.state,
                work.tick,
                &Queries { level, hitboxes },
            );
            work.publish(level);
            if let Some(aim) = aim {
                prepared.push((work, aim));
            }
        }
        sync_tank_collision(level);
        let mut shots = Vec::new();
        for (mut work, aim) in prepared {
            if let Some(shot) = finish_tank(
                &work.def,
                &mut work.state,
                work.tick,
                aim,
                &Queries { level, hitboxes },
            ) {
                shots.push(shot);
            }
            work.publish(level);
        }
        (decision, shots)
    }

    /// Fire the authored target only after a real scheduled shot is admitted.
    /// Type-zero bullets and zero-damage shots remain admitted output edges.
    pub(crate) fn dispatch(
        &mut self,
        level: &mut Level,
        hitboxes: &HitboxIndex,
        shot: &TankShot,
        destination: &mut TankDispatch<'_>,
    ) {
        let admitted = match &shot.kind {
            TankShotKind::Rocket {
                damage,
                speed,
                radius,
            } => destination
                .projectiles
                .spawn_tank_request(
                    level,
                    &crate::ai::ProjectileRequest {
                        kind: ProjectileKind::Rocket,
                        owner: shot.source,
                        origin: shot.muzzle,
                        velocity: shot.direction * *speed,
                        damage: *damage,
                        damage_type: shot.kind.damage_type(),
                        blast_radius: Some(*radius),
                        target: None,
                    },
                    shot.attacker,
                )
                .is_some(),
            TankShotKind::Bullet { bullet, damage } => {
                if *bullet != TankBullet::None {
                    let trace = shot_trace(level, hitboxes, shot);
                    queue_hit(destination.damage, shot, trace, *damage);
                }
                true
            }
            TankShotKind::Laser { damage, appearance } => {
                let trace = shot_trace(level, hitboxes, shot);
                queue_hit(destination.damage, shot, trace, *damage);
                self.presentation
                    .laser(level, appearance.as_deref(), shot.muzzle, trace.end);
                true
            }
            TankShotKind::Mortar { magnitude } => {
                let trace = shot_trace(level, hitboxes, shot);
                if trace.hit() {
                    let profile = MapBlastProfile::from_magnitude(*magnitude);
                    destination.projectiles.resolve_map_blast(
                        level,
                        BlastRequest {
                            origin: trace.end,
                            profile,
                            kind: shot.kind.damage_type(),
                            attacker: shot.attacker,
                            inflictor: shot.source,
                        },
                        destination.damage,
                    );
                    destination.effects.emit_blast_visual(
                        trace.end,
                        profile,
                        ExplosionVisuals::default(),
                    );
                }
                true
            }
        };
        if admitted && let Some(target) = &shot.target {
            level.simulation.fire_typed(
                target.clone(),
                Some(shot.attacker),
                0.0,
                TriggerUse::Toggle,
            );
        }
    }
}

struct TankWork {
    def: TankDef,
    state: TankState,
    tick: TankTick,
}

impl TankWork {
    fn read(
        level: &Level,
        entity: Entity,
        frame: TankFrame,
        controlled: Option<ControlledAim>,
    ) -> Option<Self> {
        Some(Self {
            def: (*level.registry.world.get::<&TankDef>(entity).ok()?).clone(),
            state: (*level.registry.world.get::<&TankState>(entity).ok()?).clone(),
            tick: TankTick {
                tank: entity,
                placement_origin: level.registry.world.get::<&Transform>(entity).ok()?.origin,
                // Turrets use the documented origin-brush convention. Their
                // compiled model pivot is zero, matching the shared pose hook.
                pivot_local: Vec3::ZERO,
                dt: frame.dt,
                master_open: level.simulation.master_is_active(&level.registry, entity),
                player: Some(PlayerTarget {
                    entity: frame.input.player,
                    eye: frame.eye,
                    alive: frame.input.alive,
                }),
                controlled,
            },
        })
    }

    fn publish(&self, level: &Level) {
        if let Ok(mut state) = level.registry.world.get::<&mut TankState>(self.tick.tank) {
            *state = self.state.clone();
        }
    }
}

pub(super) fn tank_entities(level: &Level) -> Vec<Entity> {
    level
        .registry
        .entities
        .iter()
        .copied()
        .filter(|&entity| level.registry.world.get::<&TankDef>(entity).is_ok())
        .take(MAX_TANKS)
        .collect()
}

/// Touch only turret poses: a second full mover sync would overwrite the
/// velocity/carry observations produced by Systems phase 2a for other movers.
pub(super) fn sync_tank_collision(level: &mut Level) {
    for entity in tank_entities(level) {
        let Some(pose) = ohl_game::tanks::tank_pose(&level.registry, entity) else {
            continue;
        };
        let (axis, degrees, pivot) = pose.axis_angle();
        for (model, attached) in [
            (&mut level.collision, &level.brush_collision),
            (&mut level.monster_collision, &level.monster_brush_collision),
        ] {
            let Some(model) = model else {
                continue;
            };
            if let Some((_, brush)) = attached.iter().find(|(source, _)| *source == entity) {
                model.set_brush_pose(*brush, pose.placement_origin, pivot, axis, degrees);
            }
        }
    }
}

pub(super) fn control_candidates(
    level: &Level,
    mounted: Option<MountedTank>,
    pending: Option<TankControlIntent>,
    selected: Option<Entity>,
) -> Vec<ControlCandidate> {
    // Examine the actual selected/owned/deferred candidates, never truncate a
    // global controls list ahead of the player's currently owned station.
    let mut candidates = Vec::new();
    for controls in [selected, mounted.and_then(|mount| mount.controls)]
        .into_iter()
        .flatten()
    {
        if let Some(candidate) = controls_candidate(level, controls)
            && !candidates.contains(&candidate)
        {
            candidates.push(candidate);
        }
    }
    for tank in [
        mounted
            .filter(|mount| mount.controls.is_none())
            .map(|mount| mount.tank),
        pending.map(|intent| intent.tank),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(candidate) = candidate(level, tank, None, None)
            && !candidates.contains(&candidate)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn controls_candidate(level: &Level, entity: Entity) -> Option<ControlCandidate> {
    if !level.simulation.master_is_active(&level.registry, entity) {
        return None;
    }
    let controls = level.registry.world.get::<&TankControls>(entity).ok()?;
    let tank = level
        .registry
        .find(&controls.target)
        .iter()
        .copied()
        .find(|&target| {
            level
                .registry
                .world
                .get::<&TankDef>(target)
                .is_ok_and(|def| def.controllable)
        })?;
    let bounds = level.registry.world.get::<&BrushBounds>(entity).ok()?;
    candidate(
        level,
        tank,
        Some(entity),
        Some(ControlBounds {
            min: bounds.mins,
            max: bounds.maxs,
        }),
    )
}

fn candidate(
    level: &Level,
    tank: Entity,
    controls: Option<Entity>,
    bounds: Option<ControlBounds>,
) -> Option<ControlCandidate> {
    let def = level.registry.world.get::<&TankDef>(tank).ok()?;
    // A definition beyond the runtime work cap cannot claim input while never
    // receiving a simulation update.
    if !tank_entities(level).contains(&tank) {
        return None;
    }
    Some(ControlCandidate {
        tank,
        controls,
        bounds,
        controllable: def.controllable && def.is_valid(),
        master_open: level.simulation.master_is_active(&level.registry, tank),
    })
}

struct Queries<'a> {
    level: &'a Level,
    hitboxes: &'a HitboxIndex,
}

impl TankQueries for Queries<'_> {
    fn sees_player(&self, turret: Entity, from: Vec3, player: PlayerTarget) -> bool {
        let trace = trace(self.level, self.hitboxes, turret, from, player.eye);
        trace.entity == Some(entity_id(player.entity)) || !trace.hit()
    }

    fn barrel_reaches_player(
        &self,
        turret: Entity,
        muzzle: Vec3,
        direction: Vec3,
        range: f32,
        player: PlayerTarget,
    ) -> bool {
        trace(
            self.level,
            self.hitboxes,
            turret,
            muzzle,
            muzzle + direction * range,
        )
        .entity
            == Some(entity_id(player.entity))
    }
}

fn source_brush(level: &Level, source: Entity) -> Option<BrushId> {
    level
        .brush_collision
        .iter()
        .find_map(|(entity, brush)| (*entity == source).then_some(*brush))
}

fn trace(
    level: &Level,
    hitboxes: &HitboxIndex,
    source: Entity,
    start: Vec3,
    end: Vec3,
) -> AttackTrace {
    level.collision.as_ref().map_or_else(
        || AttackTrace::miss(end),
        |world| {
            ohl_combat::trace_attack_filtered_ignoring_brush(
                world,
                hitboxes,
                start,
                end,
                TraceFilter::ignoring(TraceMask::SHOT, entity_id(source)),
                source_brush(level, source),
            )
        },
    )
}

fn shot_trace(level: &Level, hitboxes: &HitboxIndex, shot: &TankShot) -> AttackTrace {
    trace(
        level,
        hitboxes,
        shot.source,
        shot.muzzle,
        shot.muzzle + shot.direction * shot.range,
    )
}

fn queue_hit(queue: &mut Vec<QueuedDamage>, shot: &TankShot, trace: AttackTrace, amount: f32) {
    if amount <= 0.0 || !amount.is_finite() {
        return;
    }
    if let Some(target) = trace.entity.and_then(entity_of) {
        queue.push(QueuedDamage {
            target,
            info: DamageInfo::new(amount, shot.kind.damage_type())
                .from_entities(entity_id(shot.attacker), entity_id(shot.source))
                .from_point(shot.muzzle, shot.direction),
        });
    }
}

fn laser_style(level: &Level, name: &str) -> Option<LaserStyle> {
    let index = level
        .defs
        .iter()
        .position(|def| def.classname == "env_laser" && def.targetname.as_deref() == Some(name))?;
    let def = &level.defs[index];
    let number = |key: &str| {
        def.keyvalues
            .get(key)
            .and_then(|value| value.trim().parse::<f32>().ok())
            .filter(|value| value.is_finite())
    };
    Some(LaserStyle {
        entity: *level.registry.entities.get(index)?,
        width: number("width")
            .or_else(|| number("BoltWidth"))
            .unwrap_or(4.0)
            .clamp(0.0, 4096.0),
        render: def.render,
    })
}

impl TankPresentation {
    fn advance(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        self.pulses.retain_mut(|pulse| {
            pulse.age += dt.min(MAX_STEP_SECONDS);
            pulse.age < LASER_PULSE_SECONDS
        });
    }

    fn laser(&mut self, level: &Level, name: Option<&str>, start: Vec3, end: Vec3) {
        if self.pulses.len() >= MAX_LASER_PULSES || !start.is_finite() || !end.is_finite() {
            return;
        }
        let Some(style) = name.and_then(|name| self.styles.get(name)) else {
            return;
        };
        if !level.registry.world.contains(style.entity) {
            return;
        }
        let (render, _) = effective_render_props(&level.registry, style.entity, style.render);
        self.pulses.push(LaserPulse {
            start,
            end,
            width: style.width,
            age: 0.0,
            color: [
                f32::from(render.color[0]) / 255.0,
                f32::from(render.color[1]) / 255.0,
                f32::from(render.color[2]) / 255.0,
                f32::from(u8::try_from(render.amt.clamp(0, 255)).unwrap_or(0)) / 255.0,
            ],
        });
    }
}
