//! Radius damage: what an explosion does to everything around it.
//!
//! [`radius_damage`] is a pure function. It takes the blast's centre, its
//! radius and its maximum damage, an iterator of candidate targets, and the
//! world to test line of sight against, and returns one [`BlastHit`] per
//! target that was actually hurt — a [`DamageInfo`] plus the pushback vector
//! the caller adds to that target's velocity.
//!
//! # Behaviour this crate defines
//!
//! - **Falloff is linear and monotonic.** A target at the centre takes
//!   `max_damage`; a target at exactly `radius` takes nothing; in between,
//!   damage scales with `1 - distance / radius`. Moving a target further from
//!   the centre therefore never increases its damage, which the crate's
//!   tests assert directly. Half-Life's real falloff curve is **not
//!   published**, so the linear rule is this project's own documented choice
//!   rather than a claim about the original engine.
//! - **Distance is measured to the nearest point of the target's hitbox**
//!   when the caller supplies one, and to its origin otherwise, so a large
//!   target standing at the edge of a blast is not saved by where its origin
//!   happens to be.
//! - **Occlusion is all-or-nothing.** A world trace runs from the centre to
//!   the point damage is measured at; if solid geometry blocks it the target
//!   takes nothing at all. The original engine is widely described as
//!   attenuating rather than nullifying blast damage through cover, but no
//!   usable source publishes how, so the attenuated case is **to be
//!   black-box observed** and the conservative "cover protects completely"
//!   rule ships instead. [`ExplosionRule::occlusion`] turns the check off.
//! - **Self damage is a hook.** [`ExplosionRule::self_damage_scale`] scales
//!   the damage a blast does to its own attacker (`0.0` for a blast that
//!   cannot hurt its owner, `1.0` for one that hurts it in full); the value
//!   Half-Life uses per weapon and per skill level is **BBO**.
//!
//! No blast radius is published for any Half-Life explosive, so this module
//! never supplies one: the radius is a parameter, and the placeholders the
//! rest of the crate passes live in [`crate::projectile::ProjectileTuning`]
//! and [`crate::deployables::DeployableTuning`], marked `// TODO(black-box)`.

use glam::Vec3;
use ohl_physics::CollisionModel;

use crate::damage::{DamageInfo, DamageType};
use crate::trace::{EntityId, TraceMask, trace_attack};
use crate::weapons::BlackBox;
use crate::{HitboxIndex, HitboxLimits};

/// One candidate for an explosion's damage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlastTarget {
    /// The caller's handle for the target.
    pub id: EntityId,
    /// The target's world-space origin.
    pub position: Vec3,
    /// The target's world-space bounding box, when the caller has one. Used
    /// to measure distance and to aim the line-of-sight trace; `None` falls
    /// back to [`position`](Self::position).
    pub hitbox: Option<(Vec3, Vec3)>,
}

impl BlastTarget {
    /// A target described only by its origin.
    #[must_use]
    pub const fn new(id: EntityId, position: Vec3) -> Self {
        Self {
            id,
            position,
            hitbox: None,
        }
    }

    /// The same target with a world-space bounding box, corners normalised.
    #[must_use]
    pub fn with_hitbox(mut self, min: Vec3, max: Vec3) -> Self {
        self.hitbox = Some((min.min(max), min.max(max)));
        self
    }

    /// The point damage is measured at: the nearest point of the hitbox to
    /// `center`, or the origin when there is no hitbox.
    #[must_use]
    pub fn damage_point(&self, center: Vec3) -> Vec3 {
        match self.hitbox {
            Some((min, max)) => center.clamp(min, max),
            None => self.position,
        }
    }
}

/// The parts of an explosion this project could not confirm on a usable
/// source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosionRule {
    /// Whether world geometry between the centre and a target protects it.
    pub occlusion: bool,
    /// How much of the blast reaches the attacker who set it off. **BBO**;
    /// the default hurts the attacker in full, which is the conservative
    /// choice for a game where every explosive can kill its user.
    pub self_damage_scale: BlackBox<f32>,
    /// Units per second of pushback per point of damage dealt. **BBO**; the
    /// caller may ignore [`BlastHit::pushback`] entirely.
    pub pushback_per_damage: BlackBox<f32>,
}

impl Default for ExplosionRule {
    fn default() -> Self {
        Self {
            occlusion: true,
            // TODO(black-box): the per-weapon self-damage rule is not
            // published; 1.0 is the neutral "no special case".
            self_damage_scale: BlackBox::new(1.0),
            // TODO(black-box): blast pushback strength is not published.
            pushback_per_damage: BlackBox::new(6.0),
        }
    }
}

/// What an explosion did to one target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlastHit {
    /// Who was hurt. [`DamageInfo`] records the attacker, not the target, so
    /// the target is named here.
    pub target: EntityId,
    /// The damage record to hand to [`crate::apply_damage`].
    pub damage: DamageInfo,
    /// The impulse direction and magnitude to add to the target's velocity,
    /// in units per second. Zero for a target standing exactly at the
    /// centre, where the blast has no direction.
    pub pushback: Vec3,
}

/// The fraction of `max_damage` a target at `distance` takes.
///
/// Linear falloff, clamped into `0.0..=1.0`: `1.0` at the centre, `0.0` at
/// and beyond `radius`. Monotonically non-increasing in `distance`, which is
/// the property the crate's tests pin.
#[must_use]
pub fn falloff(distance: f32, radius: f32) -> f32 {
    if !distance.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return 0.0;
    }
    (1.0 - distance.max(0.0) / radius).clamp(0.0, 1.0)
}

/// Applies an explosion at `center` to every target within `radius`.
///
/// Targets outside the radius, targets the world hides (when
/// [`ExplosionRule::occlusion`] is set) and targets whose scaled damage
/// rounds to nothing are skipped, so the returned vector holds only real
/// hits, in the order the iterator produced them. `attacker` is credited on
/// every record and, when it is itself a target, its damage is scaled by
/// [`ExplosionRule::self_damage_scale`].
///
/// Total: a non-finite centre, a non-positive radius or a non-positive
/// `max_damage` yields an empty result rather than a panic.
#[must_use]
// The blast's geometry, its damage, its attacker, its candidates, the world
// and the rule are six independent inputs; bundling them would only move the
// argument list into a struct literal at every call site.
#[allow(clippy::too_many_arguments)]
pub fn radius_damage(
    center: Vec3,
    radius: f32,
    max_damage: f32,
    kind: DamageType,
    attacker: Option<EntityId>,
    targets: impl Iterator<Item = BlastTarget>,
    occluder: &CollisionModel,
    rule: &ExplosionRule,
) -> Vec<BlastHit> {
    let mut hits = Vec::new();
    if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return hits;
    }
    if !max_damage.is_finite() || max_damage <= 0.0 {
        return hits;
    }
    // Line of sight is a world-only question, so the index stays empty:
    // another monster between the blast and its victim does not shield it.
    let empty = HitboxIndex::new(HitboxLimits::default());

    for target in targets {
        let point = target.damage_point(center);
        if !point.is_finite() {
            continue;
        }
        let distance = point.distance(center);
        let scale = falloff(distance, radius);
        if scale <= 0.0 {
            continue;
        }
        if rule.occlusion {
            let trace = trace_attack(occluder, &empty, center, point, TraceMask::WORLD_ONLY);
            if trace.hit() {
                continue;
            }
        }
        let mut amount = max_damage * scale;
        if attacker == Some(target.id) {
            amount *= rule.self_damage_scale.value.max(0.0);
        }
        if !amount.is_finite() || amount <= 0.0 {
            continue;
        }
        let direction = (point - center).normalize_or_zero();
        hits.push(BlastHit {
            target: target.id,
            damage: DamageInfo {
                attacker,
                inflictor: attacker,
                amount,
                kind,
                origin: center,
                direction,
            },
            pushback: direction * amount * rule.pushback_per_damage.value.max(0.0),
        });
    }
    hits
}
