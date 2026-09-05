//! Simulated projectiles: the things a weapon spawns that then travel.
//!
//! A [`ProjectileSet`] is a bounded, flat list of in-flight projectiles that
//! the caller advances once per fixed tick. Every projectile moves by a
//! *swept* trace — `ohl_physics`' hull 0 (the point hull) against world
//! geometry, refined against the caller's [`HitboxIndex`] by
//! [`crate::trace_attack`] — so a projectile can never step through a wall,
//! however fast it is going. Bouncing projectiles keep sweeping with the
//! time left over after each impact, up to a bounded number of bumps.
//!
//! Nothing here applies damage. The set reports [`ProjectileEvent`]s and the
//! caller turns them into [`crate::DamageInfo`] (directly, for a bolt or a
//! bite) or into a call to [`crate::explosion::radius_damage`] (for a
//! `Detonate`). That keeps the simulation a pure function of its inputs and
//! testable without an ECS world.
//!
//! # Published versus placeholder
//!
//! Only three numbers in this module are published (see
//! `docs/FORMAT_SOURCES.md`, "Projectiles, explosions and deployables
//! (M7.3)"): the hand grenade's 5 second fuse
//! ([`HAND_GRENADE_FUSE_SECONDS`]) and the snark's roughly 20 second
//! self-destruct ([`SNARK_LIFETIME_SECONDS`]); the tripmine's 3 second arming
//! cue lives in [`crate::deployables`]. Everything else a projectile needs —
//! speeds, bounce restitution, guidance turn rates, hornet and bolt
//! lifetimes, the snark's hop cadence — is **to be black-box observed** and
//! is therefore a [`BlackBox`] field of [`ProjectileTuning`] with a neutral,
//! documented placeholder and a `// TODO(black-box)` marker.

use glam::Vec3;
use ohl_physics::{CollisionModel, MoveConfig};

use crate::trace::{EntityId, HitboxIndex, TraceMask, trace_attack};
use crate::weapons::BlackBox;

/// The hand grenade's fuse, in seconds.
///
/// Published: Combine OverWiki, "Hand Grenade" — a five second fuse that
/// starts when the pin is pulled (`docs/FORMAT_SOURCES.md`).
pub const HAND_GRENADE_FUSE_SECONDS: f32 = 5.0;

/// How long a snark lives before destroying itself, in seconds.
///
/// Published: Combine OverWiki, "Snark" — a snark "self-destructs roughly 20
/// seconds after attacking". This crate therefore restarts the timer on every
/// bite and starts it at spawn, so an idle snark also expires after this long
/// (`docs/FORMAT_SOURCES.md`).
pub const SNARK_LIFETIME_SECONDS: f32 = 20.0;

/// What kind of projectile this is.
///
/// One variant per projectile the published weapon table spawns (see
/// [`crate::weapons`]); the behaviour attached to each is described on
/// [`ProjectileSet::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectileKind {
    /// A crossbow bolt: unaffected by gravity, sticks (or, in an entity,
    /// stops) on the first impact.
    CrossbowBolt,
    /// An RPG rocket: unaffected by gravity, optionally steered toward a
    /// laser-designated point, detonates on the first impact.
    Rocket,
    /// The MP5's underslung 40mm grenade: an arcing, bouncing grenade with a
    /// fuse.
    Mp5Grenade,
    /// A thrown hand grenade: arcs, bounces and detonates after
    /// [`HAND_GRENADE_FUSE_SECONDS`].
    HandGrenade,
    /// A hornet: unaffected by gravity, stops on the first impact, expires
    /// after a lifetime. Primary-fire hornets home on a target entity;
    /// secondary-fire hornets fly straight.
    Hornet,
    /// A snark: hops along the ground toward the nearest entity in the
    /// hitbox index, bites what it lands on, and detonates after
    /// [`SNARK_LIFETIME_SECONDS`] without a bite.
    Snark,
}

impl ProjectileKind {
    /// Whether the projectile falls under [`MoveConfig::gravity`].
    ///
    /// This project's own categorisation of the published weapon behaviour
    /// (a bolt and a rocket fly flat, a thrown grenade and a snark arc), not
    /// a cited number: the *amount* of gravity is `MoveConfig`'s, and how
    /// much of it each projectile feels is [`ProjectileTuning::gravity_scale`].
    #[must_use]
    pub const fn falls(self) -> bool {
        matches!(self, Self::Mp5Grenade | Self::HandGrenade | Self::Snark)
    }

    /// Whether an impact detonates the projectile instead of bouncing it.
    #[must_use]
    pub const fn detonates_on_impact(self) -> bool {
        matches!(self, Self::Rocket)
    }

    /// Whether an impact simply stops the projectile (a bolt embedding in a
    /// wall, a hornet striking a target).
    #[must_use]
    pub const fn stops_on_impact(self) -> bool {
        matches!(self, Self::CrossbowBolt | Self::Hornet)
    }

    /// Whether the projectile bounces off what it hits.
    #[must_use]
    pub const fn bounces(self) -> bool {
        matches!(self, Self::Mp5Grenade | Self::HandGrenade | Self::Snark)
    }
}

/// Every unpublished number a projectile needs.
///
/// Each field is a [`BlackBox`] placeholder chosen to be plausible and
/// neutral rather than measured; the values are this project's own and are
/// not claimed to match retail Half-Life. Tests pass explicit values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileTuning {
    /// Fraction of [`MoveConfig::gravity`] a falling projectile feels.
    pub gravity_scale: BlackBox<f32>,
    /// Fraction of the normal-direction speed kept across a bounce.
    pub restitution: BlackBox<f32>,
    /// Fraction of the tangential speed kept across a bounce (surface drag).
    pub bounce_friction: BlackBox<f32>,
    /// Speed below which a bounced projectile is treated as having come to
    /// rest, in units per second.
    pub rest_speed: BlackBox<f32>,
    /// How fast a guided rocket may turn, in radians per second.
    pub rocket_turn_rate: BlackBox<f32>,
    /// How fast a homing hornet may turn, in radians per second.
    pub hornet_turn_rate: BlackBox<f32>,
    /// How long a hornet lives before expiring, in seconds.
    pub hornet_lifetime: BlackBox<f32>,
    /// How long a crossbow bolt lives before expiring, in seconds.
    pub bolt_lifetime: BlackBox<f32>,
    /// The MP5 grenade's fuse, in seconds. Unlike the hand grenade's, no
    /// usable source publishes it.
    pub mp5_grenade_fuse: BlackBox<f32>,
    /// Seconds between a resting snark's hops.
    pub snark_hop_interval: BlackBox<f32>,
    /// Horizontal speed a snark hop imparts, in units per second.
    pub snark_hop_speed: BlackBox<f32>,
    /// Vertical speed a snark hop imparts, in units per second.
    pub snark_hop_rise: BlackBox<f32>,
    /// Seconds a snark waits between bites.
    pub snark_bite_interval: BlackBox<f32>,
    /// How many impacts one projectile resolves within a single substep
    /// before the move is abandoned. A project-chosen bound on work, not an
    /// observed value.
    pub max_bumps: u32,
}

impl Default for ProjectileTuning {
    fn default() -> Self {
        Self {
            // TODO(black-box): no source publishes how much gravity each
            // projectile feels; 1.0 is the neutral "the same as a player".
            gravity_scale: BlackBox::new(1.0),
            // TODO(black-box): bounce restitution is unpublished. 0.45 loses
            // most of the impact speed, which keeps a grenade near where it
            // lands; it is a placeholder, not a measurement.
            restitution: BlackBox::new(0.45),
            // TODO(black-box): surface drag across a bounce is unpublished.
            bounce_friction: BlackBox::new(0.8),
            // TODO(black-box): unpublished.
            rest_speed: BlackBox::new(30.0),
            // TODO(black-box): the RPG's laser guidance is published as a
            // behaviour, its turn rate is not.
            rocket_turn_rate: BlackBox::new(4.0),
            // TODO(black-box): hornet homing is published as a behaviour,
            // its turn rate is not.
            hornet_turn_rate: BlackBox::new(6.0),
            // TODO(black-box): unpublished.
            hornet_lifetime: BlackBox::new(5.0),
            // TODO(black-box): unpublished.
            bolt_lifetime: BlackBox::new(10.0),
            // TODO(black-box): unpublished; only the hand grenade's 5 s fuse
            // is documented.
            mp5_grenade_fuse: BlackBox::new(3.0),
            // TODO(black-box): unpublished.
            snark_hop_interval: BlackBox::new(0.5),
            // TODO(black-box): unpublished.
            snark_hop_speed: BlackBox::new(180.0),
            // TODO(black-box): unpublished.
            snark_hop_rise: BlackBox::new(140.0),
            // TODO(black-box): unpublished.
            snark_bite_interval: BlackBox::new(0.5),
            max_bumps: 4,
        }
    }
}

/// A handle to one projectile in a [`ProjectileSet`], unique within that set
/// for the lifetime of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectileId(pub u32);

/// Something that happened to a projectile this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum ProjectileEvent {
    /// The projectile struck something. A bouncing projectile keeps going
    /// afterwards; every other kind is removed the same tick.
    Impact {
        /// Which projectile.
        id: ProjectileId,
        /// What kind it is.
        kind: ProjectileKind,
        /// Where the impact happened, in world units.
        position: Vec3,
        /// The surface normal at the impact, pointing back along the flight.
        normal: Vec3,
        /// The entity struck, when an entity hitbox was nearer than the
        /// world.
        entity: Option<EntityId>,
    },
    /// The projectile exploded; the caller applies radius damage here.
    Detonate {
        /// Which projectile.
        id: ProjectileId,
        /// What kind it was.
        kind: ProjectileKind,
        /// Where it exploded, in world units.
        position: Vec3,
    },
    /// The projectile was removed without exploding: a bolt that embedded
    /// itself, a hornet that ran out of lifetime.
    Expired {
        /// Which projectile.
        id: ProjectileId,
        /// What kind it was.
        kind: ProjectileKind,
        /// Where it was removed, in world units.
        position: Vec3,
    },
}

impl ProjectileEvent {
    /// The projectile the event is about.
    #[must_use]
    pub const fn id(&self) -> ProjectileId {
        match *self {
            Self::Impact { id, .. } | Self::Detonate { id, .. } | Self::Expired { id, .. } => id,
        }
    }

    /// Where the event happened.
    #[must_use]
    pub const fn position(&self) -> Vec3 {
        match *self {
            Self::Impact { position, .. }
            | Self::Detonate { position, .. }
            | Self::Expired { position, .. } => position,
        }
    }
}

/// One projectile in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projectile {
    /// This projectile's handle.
    pub id: ProjectileId,
    /// What kind it is.
    pub kind: ProjectileKind,
    /// Who fired it, for damage attribution. The caller is responsible for
    /// leaving the owner out of the [`HitboxIndex`] it passes to
    /// [`ProjectileSet::tick`]; this crate never dereferences the handle and
    /// so cannot filter by it.
    pub owner: Option<EntityId>,
    /// World-space position.
    pub position: Vec3,
    /// World-space velocity, units per second.
    pub velocity: Vec3,
    /// Seconds since the projectile was spawned.
    pub age: f32,
    /// Seconds left on the fuse, when the kind has one. Reaching zero
    /// detonates the projectile.
    pub fuse: Option<f32>,
    /// The point a guided rocket steers toward, updated by the caller as the
    /// player's laser moves. `None` flies straight.
    pub guide_point: Option<Vec3>,
    /// The entity a homing hornet or a hopping snark steers toward. `None`
    /// makes a hornet fly straight; a snark with `None` picks the nearest
    /// entity in the index each time it hops.
    pub target: Option<EntityId>,
    /// Seconds until this projectile may bite again (snarks only).
    pub attack_cooldown: f32,
    /// Seconds until this projectile hops again (snarks only).
    pub hop_cooldown: f32,
    /// Whether the projectile has settled on a surface.
    ///
    /// A bouncing projectile whose speed drops below
    /// [`ProjectileTuning::rest_speed`] after an impact is parked: it stops
    /// integrating, so a grenade lying on the floor does not re-report an
    /// impact with that floor every tick. Only a snark leaves this state, by
    /// hopping.
    pub resting: bool,
}

/// Caps on how much a [`ProjectileSet`] may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectileLimits {
    /// Maximum simultaneous projectiles; spawning past this fails.
    pub max_projectiles: usize,
}

impl Default for ProjectileLimits {
    /// A project-chosen cap of 128, comfortably above a busy fight.
    fn default() -> Self {
        Self {
            max_projectiles: 128,
        }
    }
}

/// The world a [`ProjectileSet::tick`] moves through.
#[derive(Debug, Clone, Copy)]
pub struct ProjectileWorld<'a> {
    /// The map's collision hulls; projectiles sweep hull 0 through it.
    pub collision: &'a CollisionModel,
    /// The entities a projectile may hit, rebuilt by the caller each tick.
    /// Callers omit the projectile's own owner.
    pub entities: &'a HitboxIndex,
    /// Where gravity comes from, shared with player movement.
    pub movement: &'a MoveConfig,
    /// The unpublished constants.
    pub tuning: &'a ProjectileTuning,
}

/// The longest slice of a tick one integration substep covers, in seconds.
///
/// Sweeping guarantees a projectile never tunnels regardless of step length;
/// substepping only keeps *guidance* and gravity accurate when a host runs a
/// long frame. A project-chosen bound, not an observed value.
pub const MAX_SUBSTEP_SECONDS: f32 = 0.05;

/// How many substeps one [`ProjectileSet::tick`] may take.
pub const MAX_SUBSTEPS: u32 = 8;

/// A bounded set of in-flight projectiles.
///
/// Projectiles are stored in spawn order and ticked in that order, so a set
/// built from the same spawns and ticked with the same inputs always
/// produces the same event sequence.
#[derive(Debug, Clone)]
pub struct ProjectileSet {
    projectiles: Vec<Projectile>,
    limits: ProjectileLimits,
    next_id: u32,
    rng: Rng,
}

impl Default for ProjectileSet {
    /// An empty set with the default limits and seed `0`.
    fn default() -> Self {
        Self::new(ProjectileLimits::default(), 0)
    }
}

impl ProjectileSet {
    /// An empty set.
    ///
    /// `seed` drives the one place this module needs a random choice: the
    /// direction a snark hops when it has no target. Two sets built with the
    /// same seed and given the same inputs produce identical events.
    #[must_use]
    pub fn new(limits: ProjectileLimits, seed: u64) -> Self {
        Self {
            projectiles: Vec::new(),
            limits,
            next_id: 0,
            rng: Rng::new(seed),
        }
    }

    /// The projectiles currently in flight, in spawn order.
    #[must_use]
    pub fn projectiles(&self) -> &[Projectile] {
        &self.projectiles
    }

    /// How many projectiles are in flight.
    #[must_use]
    pub fn len(&self) -> usize {
        self.projectiles.len()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.projectiles.is_empty()
    }

    /// The projectile with this handle, if it is still in flight.
    #[must_use]
    pub fn get(&self, id: ProjectileId) -> Option<&Projectile> {
        self.projectiles.iter().find(|entry| entry.id == id)
    }

    /// The projectile with this handle, mutably: how a caller updates a
    /// rocket's [`Projectile::guide_point`] or a hornet's target.
    pub fn get_mut(&mut self, id: ProjectileId) -> Option<&mut Projectile> {
        self.projectiles.iter_mut().find(|entry| entry.id == id)
    }

    /// Removes every projectile without emitting events.
    pub fn clear(&mut self) {
        self.projectiles.clear();
    }

    /// Spawns a projectile at `position` travelling at `velocity`.
    ///
    /// Returns `None` when the set is full or either vector is not finite,
    /// so a flood of spawns degrades instead of growing without limit. The
    /// initial fuse comes from the kind: [`HAND_GRENADE_FUSE_SECONDS`] and
    /// [`SNARK_LIFETIME_SECONDS`] are published, the MP5 grenade's is a
    /// [`ProjectileTuning`] placeholder, and the remaining kinds have none.
    pub fn spawn(
        &mut self,
        kind: ProjectileKind,
        owner: Option<EntityId>,
        position: Vec3,
        velocity: Vec3,
        tuning: &ProjectileTuning,
    ) -> Option<ProjectileId> {
        if self.projectiles.len() >= self.limits.max_projectiles
            || !position.is_finite()
            || !velocity.is_finite()
        {
            return None;
        }
        let id = ProjectileId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let fuse = match kind {
            ProjectileKind::HandGrenade => Some(HAND_GRENADE_FUSE_SECONDS),
            ProjectileKind::Snark => Some(SNARK_LIFETIME_SECONDS),
            ProjectileKind::Mp5Grenade => Some(tuning.mp5_grenade_fuse.value),
            _ => None,
        };
        self.projectiles.push(Projectile {
            id,
            kind,
            owner,
            position,
            velocity,
            age: 0.0,
            fuse,
            guide_point: None,
            target: None,
            attack_cooldown: 0.0,
            hop_cooldown: 0.0,
            resting: false,
        });
        Some(id)
    }

    /// Advances every projectile by `dt` seconds.
    ///
    /// The step, per projectile and per substep, is:
    ///
    /// 1. tick the fuse and the lifetime; a fuse that runs out emits
    ///    [`ProjectileEvent::Detonate`] and a lifetime that runs out emits
    ///    [`ProjectileEvent::Expired`];
    /// 2. steer — a rocket with a [`Projectile::guide_point`] and a hornet
    ///    with a [`Projectile::target`] turn toward it by at most their turn
    ///    rate times the substep, keeping their speed; a resting snark hops
    ///    toward the nearest entity in the index (or, with none, in a
    ///    seeded pseudorandom direction);
    /// 3. apply gravity to the kinds that fall;
    /// 4. sweep from the current position to the position the velocity
    ///    implies, through hull 0 and the hitbox index. On an impact,
    ///    [`ProjectileEvent::Impact`] is emitted and the kind decides what
    ///    happens next: a rocket detonates, a bolt or hornet stops and
    ///    expires, and a grenade or snark bounces and keeps sweeping with the
    ///    time it has left.
    ///
    /// Because every move is a swept trace, a projectile never crosses a
    /// solid surface no matter how fast it travels.
    ///
    /// Events are appended to `events` in projectile order; the vector is
    /// not cleared first.
    pub fn tick(
        &mut self,
        dt: f32,
        world: &ProjectileWorld<'_>,
        events: &mut Vec<ProjectileEvent>,
    ) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let substeps = substep_count(dt);
        #[allow(clippy::cast_precision_loss)]
        let step = dt / substeps as f32;

        let mut index = 0;
        while index < self.projectiles.len() {
            let mut removed = false;
            for _ in 0..substeps {
                if self.advance_one(index, step, world, events) {
                    removed = true;
                    break;
                }
            }
            if removed {
                self.projectiles.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Advances the projectile at `index` by one substep. Returns `true`
    /// when it should be removed from the set.
    fn advance_one(
        &mut self,
        index: usize,
        step: f32,
        world: &ProjectileWorld<'_>,
        events: &mut Vec<ProjectileEvent>,
    ) -> bool {
        let hop = self.snark_hop(index, step, world);
        let projectile = &mut self.projectiles[index];
        projectile.age += step;
        projectile.attack_cooldown = (projectile.attack_cooldown - step).max(0.0);

        if let Some(fuse) = projectile.fuse.as_mut() {
            *fuse -= step;
            if *fuse <= 0.0 {
                events.push(ProjectileEvent::Detonate {
                    id: projectile.id,
                    kind: projectile.kind,
                    position: projectile.position,
                });
                return true;
            }
        }
        if let Some(lifetime) = lifetime_of(projectile.kind, world.tuning)
            && projectile.age >= lifetime
        {
            events.push(ProjectileEvent::Expired {
                id: projectile.id,
                kind: projectile.kind,
                position: projectile.position,
            });
            return true;
        }

        if let Some(velocity) = hop {
            projectile.velocity = velocity;
            projectile.resting = false;
        }
        if projectile.resting {
            // Settled on a surface: nothing to integrate, and nothing to
            // report.
            return false;
        }
        steer(projectile, step, world);
        if projectile.kind.falls() {
            projectile.velocity.z -=
                world.movement.gravity * world.tuning.gravity_scale.value * step;
        }

        Self::sweep(projectile, step, world, events)
    }

    /// Moves one projectile through the world for `step` seconds, resolving
    /// impacts. Returns `true` when the projectile should be removed.
    fn sweep(
        projectile: &mut Projectile,
        step: f32,
        world: &ProjectileWorld<'_>,
        events: &mut Vec<ProjectileEvent>,
    ) -> bool {
        let tuning = world.tuning;
        let mut remaining = step;
        for _ in 0..tuning.max_bumps.max(1) {
            if remaining <= 0.0 || !projectile.velocity.is_finite() {
                return false;
            }
            let start = projectile.position;
            let end = start + projectile.velocity * remaining;
            let trace = trace_attack(world.collision, world.entities, start, end, TraceMask::SHOT);
            if !trace.hit() {
                projectile.position = end;
                return false;
            }

            projectile.position = trace.end;
            let entity = trace.entity;
            let normal = trace.surface_normal;
            let biting = projectile.kind == ProjectileKind::Snark && entity.is_some();
            let report = !biting || projectile.attack_cooldown <= 0.0;
            if report {
                events.push(ProjectileEvent::Impact {
                    id: projectile.id,
                    kind: projectile.kind,
                    position: trace.end,
                    normal,
                    entity,
                });
            }
            if biting && report {
                // Published: a snark self-destructs roughly 20 s after
                // attacking, so a bite restarts the timer.
                projectile.fuse = Some(SNARK_LIFETIME_SECONDS);
                projectile.attack_cooldown = tuning.snark_bite_interval.value.max(0.0);
            }

            if projectile.kind.detonates_on_impact() {
                events.push(ProjectileEvent::Detonate {
                    id: projectile.id,
                    kind: projectile.kind,
                    position: trace.end,
                });
                return true;
            }
            if projectile.kind.stops_on_impact() {
                projectile.velocity = Vec3::ZERO;
                events.push(ProjectileEvent::Expired {
                    id: projectile.id,
                    kind: projectile.kind,
                    position: trace.end,
                });
                return true;
            }

            // A bouncer. A degenerate normal (a trace that started inside
            // solid) has no plane to reflect off, so the projectile simply
            // stops rather than being flung somewhere arbitrary.
            if normal.length_squared() <= f32::EPSILON {
                projectile.velocity = Vec3::ZERO;
                projectile.resting = true;
                return false;
            }
            projectile.velocity = bounce(projectile.velocity, normal, tuning);
            let rest_speed = tuning.rest_speed.value.max(0.0);
            if projectile.velocity.length_squared() <= rest_speed * rest_speed {
                projectile.velocity = Vec3::ZERO;
                projectile.resting = true;
                return false;
            }
            remaining *= 1.0 - trace.fraction;
        }
        false
    }

    /// The velocity a resting snark should hop with this substep, if it is
    /// due a hop.
    fn snark_hop(&mut self, index: usize, step: f32, world: &ProjectileWorld<'_>) -> Option<Vec3> {
        let projectile = self.projectiles[index];
        if projectile.kind != ProjectileKind::Snark {
            return None;
        }
        let cooldown = (self.projectiles[index].hop_cooldown - step).max(0.0);
        self.projectiles[index].hop_cooldown = cooldown;
        if cooldown > 0.0 || !projectile.resting {
            return None;
        }
        self.projectiles[index].hop_cooldown = world.tuning.snark_hop_interval.value.max(step);

        let direction = nearest_entity(world.entities, projectile.position, projectile.target)
            .map(|(_, origin)| origin - projectile.position)
            .map(|to_target| Vec3::new(to_target.x, to_target.y, 0.0))
            .filter(|flat| flat.length_squared() > f32::EPSILON)
            .map_or_else(
                || {
                    // No hostile in the index: wander, from the set's own
                    // seeded stream so a replay is identical.
                    let angle = self.rng.next_unit() * core::f32::consts::TAU;
                    Vec3::new(angle.cos(), angle.sin(), 0.0)
                },
                Vec3::normalize,
            );
        Some(
            direction * world.tuning.snark_hop_speed.value
                + Vec3::Z * world.tuning.snark_hop_rise.value,
        )
    }
}

/// How many substeps `dt` needs.
fn substep_count(dt: f32) -> u32 {
    let needed = (dt / MAX_SUBSTEP_SECONDS).ceil();
    if !needed.is_finite() || needed < 1.0 {
        return 1;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = needed as u32;
    count.clamp(1, MAX_SUBSTEPS)
}

/// The lifetime after which a projectile of this kind expires, if any.
fn lifetime_of(kind: ProjectileKind, tuning: &ProjectileTuning) -> Option<f32> {
    match kind {
        ProjectileKind::Hornet => Some(tuning.hornet_lifetime.value),
        ProjectileKind::CrossbowBolt => Some(tuning.bolt_lifetime.value),
        _ => None,
    }
}

/// Turns a guided projectile toward its target, keeping its speed.
fn steer(projectile: &mut Projectile, step: f32, world: &ProjectileWorld<'_>) {
    let (goal, turn_rate) = match projectile.kind {
        ProjectileKind::Rocket => (projectile.guide_point, world.tuning.rocket_turn_rate.value),
        ProjectileKind::Hornet => (
            projectile
                .target
                .and_then(|id| entity_origin(world.entities, id)),
            world.tuning.hornet_turn_rate.value,
        ),
        _ => return,
    };
    let Some(goal) = goal else { return };
    let speed = projectile.velocity.length();
    if speed <= f32::EPSILON || !goal.is_finite() {
        return;
    }
    let desired = (goal - projectile.position).normalize_or_zero();
    if desired == Vec3::ZERO {
        return;
    }
    let current = projectile.velocity / speed;
    projectile.velocity = turn_towards(current, desired, turn_rate.max(0.0) * step) * speed;
}

/// Rotates the unit vector `current` toward the unit vector `desired` by at
/// most `max_angle` radians.
fn turn_towards(current: Vec3, desired: Vec3, max_angle: f32) -> Vec3 {
    let cosine = current.dot(desired).clamp(-1.0, 1.0);
    let angle = cosine.acos();
    if !angle.is_finite() || angle <= max_angle {
        return desired;
    }
    let axis = current.cross(desired);
    let axis = if axis.length_squared() <= f32::EPSILON {
        // Exactly opposed: any perpendicular axis turns the right amount.
        let fallback = if current.x.abs() < 0.9 {
            Vec3::X
        } else {
            Vec3::Y
        };
        current.cross(fallback)
    } else {
        axis
    };
    let axis = axis.normalize_or_zero();
    if axis == Vec3::ZERO {
        return current;
    }
    let (sin, cos) = max_angle.sin_cos();
    // Rodrigues' rotation of `current` about `axis` by `max_angle`.
    (current * cos + axis.cross(current) * sin).normalize_or_zero()
}

/// Reflects `velocity` off a surface with unit normal `normal`.
fn bounce(velocity: Vec3, normal: Vec3, tuning: &ProjectileTuning) -> Vec3 {
    let normal = normal.normalize_or_zero();
    if normal == Vec3::ZERO {
        return Vec3::ZERO;
    }
    let into = velocity.dot(normal);
    let normal_part = normal * into;
    let tangent = velocity - normal_part;
    let restitution = tuning.restitution.value.clamp(0.0, 1.0);
    let friction = tuning.bounce_friction.value.clamp(0.0, 1.0);
    tangent * friction - normal_part * restitution
}

/// The world origin of an entity in the index.
fn entity_origin(entities: &HitboxIndex, id: EntityId) -> Option<Vec3> {
    entities
        .entries()
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.origin)
}

/// The entity in the index nearest `from`, preferring `preferred` when it is
/// present. Ties resolve to the earlier entry, so the choice is stable.
fn nearest_entity(
    entities: &HitboxIndex,
    from: Vec3,
    preferred: Option<EntityId>,
) -> Option<(EntityId, Vec3)> {
    if let Some(id) = preferred
        && let Some(origin) = entity_origin(entities, id)
    {
        return Some((id, origin));
    }
    let mut best: Option<(EntityId, Vec3, f32)> = None;
    for entry in entities.entries() {
        let distance = entry.origin.distance_squared(from);
        if best.is_none_or(|(_, _, current)| distance < current) {
            best = Some((entry.id, entry.origin, distance));
        }
    }
    best.map(|(id, origin, _)| (id, origin))
}

/// A tiny deterministic generator (a PCG-style xorshift-multiply), used only
/// for a wandering snark's hop direction.
///
/// This project's own, so a replay from the same seed is identical without
/// pulling in a dependency; nothing about it mirrors any engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rng {
    state: u64,
}

impl Rng {
    const fn new(seed: u64) -> Self {
        // An odd, non-zero start so the stream never degenerates.
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let xor = ((self.state >> 18) ^ self.state) >> 27;
        let rotate = (self.state >> 59) as u32;
        #[allow(clippy::cast_possible_truncation)]
        let value = xor as u32;
        value.rotate_right(rotate)
    }

    /// A value in `0.0..1.0`.
    fn next_unit(&mut self) -> f32 {
        let scaled = f32::from(u16::try_from(self.next_u32() >> 16).unwrap_or(0));
        scaled / 65_536.0
    }
}
