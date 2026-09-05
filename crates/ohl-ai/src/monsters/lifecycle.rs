//! Health intake, death, corpse/gib decisions, `TriggerCondition` firing and
//! `monstermaker` spawner semantics.
//!
//! [`apply_damage`] is deliberately the *only* place [`crate::Actor::health`]
//! is decremented from a [`crate::DamageQueue`] today — `AiWorld::tick`
//! itself only turns damage into [`crate::Conditions`], leaving the actual
//! health subtraction to whichever crate resolves damage, which today is
//! this module and eventually `ohl-combat`'s `DamageInfo` (see
//! `crate::damage`'s module doc comment for the same unification note).
//! Calling it once per tick, before [`crate::AiWorld::tick`], guarantees a
//! monster crossing to zero health emits exactly one
//! [`crate::AiEventKind::Died`], because the crossing check
//! (`previous_health > 0.0 && new_health <= 0.0`) can only be true once:
//! the entity's `Actor::alive` flag is cleared in the same call, and a
//! cleared flag skips every later call for that entity.
//!
//! ## Clean room
//!
//! `TriggerCondition`/`TriggerTarget` is a published `monster_generic`
//! keyvalue pair (see `docs/FORMAT_SOURCES.md`, "Monster definitions"): a
//! monster fires a named target entity when a documented condition occurs.
//! Two of the eleven numbered conditions this module models
//! (`Unconfirmed5`/`Unconfirmed6`) could not be independently verified from
//! a reachable public source and are named accordingly rather than guessed;
//! everything else here — the `Spawner`/`monstermaker` fields, the gib
//! overkill threshold, and the wiring between them — is this project's own,
//! written from the public keyvalue *names* only.

use hecs::{Entity, World};

use crate::damage::DamageQueue;
use crate::senses::SoundKind;
use crate::world::{Actor, AiEvent, AiEventKind};

use super::table::{MonsterFlags, MonsterSpec};

/// How much total damage in the killing tick counts as an overkill that
/// gibs the corpse, expressed as a multiple of the monster's max health.
/// **`TODO(black-box)`**: gibbing on sufficiently large overkill is
/// published behaviour; the exact multiplier is not.
pub const DEFAULT_GIB_OVERKILL_MULTIPLIER: f32 = 2.0;

/// What happens to a monster's remains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpseDecision {
    /// A normal corpse is left behind.
    Corpse,
    /// The kill was hard enough to gib instead.
    Gib,
}

/// Decrements `target`'s health by every queued hit against it, in world
/// units of health, and reports what happened.
///
/// Skips any target that has no [`Actor`] component or is already dead
/// (`actor.alive == false`), so calling this more than once with a stale
/// queue, or after `AiWorld::tick` has already marked an entity dead, cannot
/// double-count a kill or refire [`AiEventKind::Died`].
#[must_use]
pub fn apply_damage(
    world: &mut World,
    queue: &DamageQueue,
    gib_overkill_multiplier: f32,
) -> Vec<AiEvent> {
    apply_damage_with_corpses(world, queue, gib_overkill_multiplier).0
}

/// [`apply_damage`], additionally reporting the corpse decision for every
/// monster that died this call, in the same order as `queue`.
#[must_use]
pub fn apply_damage_with_corpses(
    world: &mut World,
    queue: &DamageQueue,
    gib_overkill_multiplier: f32,
) -> (Vec<AiEvent>, Vec<(Entity, CorpseDecision)>) {
    let mut corpses = Vec::new();
    let mut seen: Vec<Entity> = Vec::new();
    let mut events = Vec::new();
    for event in queue.events() {
        if seen.contains(&event.target) {
            continue;
        }
        seen.push(event.target);
        let Some((total, _attacker, _position, _provoked)) =
            crate::damage::summarize(queue, event.target)
        else {
            continue;
        };
        let Ok(mut actor) = world.get::<&mut Actor>(event.target) else {
            continue;
        };
        if !actor.alive {
            continue;
        }
        let previous_health = actor.health;
        let new_health = previous_health - total;
        actor.health = new_health;
        if previous_health > 0.0 && new_health <= 0.0 {
            actor.alive = false;
            events.push(AiEvent {
                entity: event.target,
                kind: AiEventKind::Died,
            });
            let overkill_threshold = previous_health.max(0.0) * gib_overkill_multiplier.max(0.0);
            let decision = if total > overkill_threshold && overkill_threshold > 0.0 {
                CorpseDecision::Gib
            } else {
                CorpseDecision::Corpse
            };
            corpses.push((event.target, decision));
        }
    }
    (events, corpses)
}

/// Whether `spec`'s corpse should fade rather than persist, per its
/// [`MonsterFlags::FADES_CORPSE`] flag.
#[must_use]
pub fn should_fade_corpse(spec: &MonsterSpec) -> bool {
    spec.flags.contains(MonsterFlags::FADES_CORPSE)
}

/// The published `monster_generic` `TriggerCondition` keyvalue's numbered
/// values.
///
/// Conditions `5` and `6` never appeared in any reachable public source
/// during this pass (see the module doc comment) and are modeled as
/// [`Self::Unconfirmed5`]/[`Self::Unconfirmed6`], evaluated as never firing
/// (they behave like [`Self::None`]) until an independently verified
/// meaning is found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TriggerCondition {
    /// No trigger condition.
    #[default]
    None = 0,
    /// See the player, and become hostile toward them.
    SeePlayerMadAtPlayer = 1,
    /// Took any damage this tick.
    TakeDamage = 2,
    /// Health fell to or below half of its starting value.
    HalfHealthRemaining = 3,
    /// Died.
    Death = 4,
    /// Unconfirmed (see the module doc comment).
    Unconfirmed5 = 5,
    /// Unconfirmed (see the module doc comment).
    Unconfirmed6 = 6,
    /// Heard a world sound.
    HearWorld = 7,
    /// Heard the player.
    HearPlayer = 8,
    /// Heard combat.
    HearCombat = 9,
    /// See the player, unconditionally (even if not otherwise hostile).
    SeePlayerUnconditional = 10,
}

/// Everything [`TriggerCondition::evaluate`] needs to know about a monster's
/// tick, gathered by the caller from [`crate::AiWorld`]/[`Actor`]/
/// [`crate::MonsterAi`] state rather than owned by this module.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each field mirrors one independent published TriggerCondition input, not a state machine"
)]
pub struct TriggerContext {
    /// [`crate::Conditions::SEE_ENEMY`] was set and the seen entity was the
    /// player.
    pub sees_player: bool,
    /// The monster is currently hostile toward the player (acquired them as
    /// an enemy, rather than merely perceiving them).
    pub hostile_to_player: bool,
    /// Damage was taken this tick.
    pub took_damage: bool,
    /// Current health divided by starting health, in `[0, 1]`.
    pub health_fraction: f32,
    /// The monster died this tick.
    pub died: bool,
    /// A world sound was heard this tick.
    pub heard_world: bool,
    /// The player was heard this tick.
    pub heard_player: bool,
    /// Combat was heard this tick.
    pub heard_combat: bool,
}

impl TriggerCondition {
    /// Whether this condition fires, given `context` and whether it already
    /// fired once before (conditions other than repeatable sense/damage
    /// ones — [`Self::Death`], [`Self::HalfHealthRemaining`] — should only
    /// ever fire once; the caller is responsible for not calling
    /// [`Self::evaluate`] again after a one-shot condition has fired,
    /// exactly as it already must for [`crate::AiEventKind::Died`]).
    #[must_use]
    pub fn evaluate(self, context: TriggerContext) -> bool {
        match self {
            Self::None | Self::Unconfirmed5 | Self::Unconfirmed6 => false,
            Self::SeePlayerMadAtPlayer => context.sees_player && context.hostile_to_player,
            Self::TakeDamage => context.took_damage,
            Self::HalfHealthRemaining => context.health_fraction <= 0.5,
            Self::Death => context.died,
            Self::HearWorld => context.heard_world,
            Self::HearPlayer => context.heard_player,
            Self::HearCombat => context.heard_combat,
            Self::SeePlayerUnconditional => context.sees_player,
        }
    }
}

/// A monster's `TriggerCondition`/`TriggerTarget` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonsterTrigger {
    /// The condition that fires [`Self::target`].
    pub condition: TriggerCondition,
    /// The `targetname` to fire. Actually dispatching a fire-on-target
    /// event into the map logic simulation is `ohl-game`'s job (see
    /// `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic"); this
    /// crate only decides *whether* to fire, once, per
    /// [`MonsterTrigger::check`].
    pub target: String,
    fired: bool,
}

impl MonsterTrigger {
    /// A trigger that has not fired yet.
    #[must_use]
    pub fn new(condition: TriggerCondition, target: impl Into<String>) -> Self {
        Self {
            condition,
            target: target.into(),
            fired: false,
        }
    }

    /// Whether this trigger should fire now: the condition evaluates true
    /// and (for a one-shot condition) it has not already fired.
    ///
    /// [`TriggerCondition::TakeDamage`], `HearWorld`, `HearPlayer` and
    /// `HearCombat` are treated as repeatable; every other condition fires
    /// at most once for the trigger's lifetime.
    pub fn check(&mut self, context: TriggerContext) -> bool {
        let repeatable = matches!(
            self.condition,
            TriggerCondition::TakeDamage
                | TriggerCondition::HearWorld
                | TriggerCondition::HearPlayer
                | TriggerCondition::HearCombat
        );
        if self.fired && !repeatable {
            return false;
        }
        let fires = self.condition.evaluate(context);
        if fires {
            self.fired = true;
        }
        fires
    }

    /// Whether a one-shot trigger has already fired.
    #[must_use]
    pub fn has_fired(&self) -> bool {
        self.fired
    }
}

/// A sound (or scent) queued for the *next* tick's `AiWorld::emit_sound`,
/// returned by callers that decide a monster should make noise as part of
/// its lifecycle (a death cry, say) without this crate depending on the
/// audio/render crates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LifecycleSound {
    /// What kind of sound.
    pub kind: SoundKind,
    /// How far it carries, in world units.
    pub radius: f32,
}

#[cfg(test)]
mod tests {
    use super::{
        CorpseDecision, DEFAULT_GIB_OVERKILL_MULTIPLIER, MonsterTrigger, TriggerCondition,
        TriggerContext, apply_damage, apply_damage_with_corpses,
    };
    use crate::damage::{DamageEvent, DamageQueue, DamageSink};
    use crate::state::Classification;
    use crate::world::{Actor, AiEventKind};
    use glam::Vec3;
    use hecs::World;

    #[test]
    fn a_killed_monster_emits_exactly_one_died_event() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let victim =
            world.spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(10.0),));

        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::new(victim, attacker, 30.0, Vec3::ZERO));
        let events = apply_damage(&mut world, &queue, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, AiEventKind::Died));
        assert_eq!(events[0].entity, victim);

        let actor = *world.get::<&Actor>(victim).expect("component");
        assert!(!actor.alive);
        assert!(actor.health <= 0.0);

        // Calling it again with the same (or a fresh) queue must not refire
        // Died for an entity that is already dead.
        let mut queue2 = DamageQueue::new();
        queue2.push_damage(DamageEvent::new(victim, attacker, 5.0, Vec3::ZERO));
        let events2 = apply_damage(&mut world, &queue2, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events2.is_empty());
    }

    #[test]
    fn a_survivor_is_not_reported_as_died() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let victim = world
            .spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(100.0),));
        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::new(victim, attacker, 10.0, Vec3::ZERO));
        let events = apply_damage(&mut world, &queue, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        let actor = *world.get::<&Actor>(victim).expect("component");
        assert!(actor.alive);
        assert!((actor.health - 90.0).abs() < 1e-4);
    }

    #[test]
    fn overkill_gibs_and_a_narrow_kill_leaves_a_corpse() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let barely_killed =
            world.spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(10.0),));
        let overkilled =
            world.spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(10.0),));

        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::new(barely_killed, attacker, 10.0, Vec3::ZERO));
        queue.push_damage(DamageEvent::new(overkilled, attacker, 1_000.0, Vec3::ZERO));

        let (events, corpses) =
            apply_damage_with_corpses(&mut world, &queue, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert_eq!(events.len(), 2);
        assert_eq!(corpses.len(), 2);
        let barely = corpses.iter().find(|(e, _)| *e == barely_killed).unwrap();
        let over = corpses.iter().find(|(e, _)| *e == overkilled).unwrap();
        assert_eq!(barely.1, CorpseDecision::Corpse);
        assert_eq!(over.1, CorpseDecision::Gib);
    }

    #[test]
    fn trigger_condition_4_death_fires_the_target() {
        let mut trigger = MonsterTrigger::new(TriggerCondition::Death, "door_1");
        let context = TriggerContext {
            died: true,
            ..TriggerContext::default()
        };
        assert!(trigger.check(context));
        assert!(trigger.has_fired());
        // A one-shot trigger does not fire twice.
        assert!(!trigger.check(context));
    }

    #[test]
    fn half_health_fires_once_and_take_damage_repeats() {
        let mut half_health = MonsterTrigger::new(TriggerCondition::HalfHealthRemaining, "t");
        let low = TriggerContext {
            health_fraction: 0.4,
            ..TriggerContext::default()
        };
        assert!(half_health.check(low));
        assert!(!half_health.check(low));

        let mut take_damage = MonsterTrigger::new(TriggerCondition::TakeDamage, "t");
        let hurt = TriggerContext {
            took_damage: true,
            ..TriggerContext::default()
        };
        assert!(take_damage.check(hurt));
        assert!(take_damage.check(hurt));
    }

    #[test]
    fn unconfirmed_conditions_never_fire() {
        let context = TriggerContext {
            sees_player: true,
            hostile_to_player: true,
            took_damage: true,
            died: true,
            heard_world: true,
            heard_player: true,
            heard_combat: true,
            health_fraction: 0.0,
        };
        assert!(!TriggerCondition::Unconfirmed5.evaluate(context));
        assert!(!TriggerCondition::Unconfirmed6.evaluate(context));
        assert!(!TriggerCondition::None.evaluate(context));
    }
}
