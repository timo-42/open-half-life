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

use crate::damage::{DamageQueue, DamageResponse};
use crate::senses::SoundKind;
use crate::world::{Actor, AiEvent, AiEventKind, Impervious};

use super::bigmomma::{DamageVerdict, GonarchTrail};
use super::nihilanth::{NihilanthShield, ShieldVerdict};
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
///
/// Every target answers every damage type at face value (save an
/// [`Impervious`] one, which nothing hurts); see [`apply_damage_effective`]
/// for the per-species form.
#[must_use]
pub fn apply_damage_with_corpses(
    world: &mut World,
    queue: &DamageQueue,
    gib_overkill_multiplier: f32,
) -> (Vec<AiEvent>, Vec<(Entity, CorpseDecision)>) {
    apply_damage_effective(world, queue, gib_overkill_multiplier, &|_| {
        DamageResponse::ORDINARY
    })
}

/// [`apply_damage_with_corpses`] with a per-target [`DamageResponse`]:
/// `response(target)` says which damage types take health off that entity
/// and which take double (`crate::monsters::table::damage_response_for`
/// for its species). A queued hit it shrugs off costs nothing — though the
/// monster still *notices* it, since `AiWorld::tick` reads the same queue
/// at face value for its conditions. An [`Impervious`] target answers with
/// [`DamageResponse::IMPERVIOUS`] whatever `response` says, so the marker
/// means "no damage can hurt it" here exactly as it does to the engine,
/// which drops every hit aimed at one before it is ever queued.
///
/// Two boss components are consulted here as well, because this is the
/// one place health moves: a [`GonarchTrail`] shields the Gonarch on its
/// trail and turns a depleting hit into a departure rather than a death,
/// and a [`NihilanthShield`] drains its reserve before, and blocks health
/// loss until, the boss is exposed. Neither can emit
/// [`AiEventKind::Died`] except through the same crossing check every
/// other monster uses.
#[must_use]
pub fn apply_damage_effective(
    world: &mut World,
    queue: &DamageQueue,
    gib_overkill_multiplier: f32,
    response: &dyn Fn(Entity) -> DamageResponse,
) -> (Vec<AiEvent>, Vec<(Entity, CorpseDecision)>) {
    let mut corpses = Vec::new();
    let mut seen: Vec<Entity> = Vec::new();
    let mut events = Vec::new();
    for event in queue.events() {
        if seen.contains(&event.target) {
            continue;
        }
        seen.push(event.target);
        let answer = if world.get::<&Impervious>(event.target).is_ok() {
            DamageResponse::IMPERVIOUS
        } else {
            response(event.target)
        };
        let Some((total, _attacker, _position, _provoked)) =
            crate::damage::summarize_effective(queue, event.target, answer)
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
        if !boss_intake(world, event.target, &mut actor, total) {
            continue;
        }
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

/// Runs a hit of `total` past `target`'s boss components, if it has any.
/// Returns whether the hit goes on to cost health. A Gonarch whose node
/// health this hit depletes has its health restored to its spawn value
/// here and leaves for the next node instead.
fn boss_intake(world: &World, target: Entity, actor: &mut Actor, total: f32) -> bool {
    if let Ok(mut shield) = world.get::<&mut NihilanthShield>(target) {
        match shield.absorb(total) {
            ShieldVerdict::Applied => {}
            ShieldVerdict::Absorbed | ShieldVerdict::Blocked => return false,
        }
    }
    if let Ok(mut trail) = world.get::<&mut GonarchTrail>(target) {
        match trail.absorb_damage(actor.health, total) {
            DamageVerdict::Applied => {}
            DamageVerdict::Shielded => return false,
            DamageVerdict::Depleted { .. } => {
                actor.health = trail.base_health();
                return false;
            }
        }
    }
    true
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
        TriggerContext, apply_damage, apply_damage_effective, apply_damage_with_corpses,
    };
    use crate::damage::{DamageEvent, DamageKinds, DamageQueue, DamageResponse, DamageSink};
    use crate::monsters::bigmomma::{GonarchTrail, Trail, TrailNode, TrailPhase};
    use crate::monsters::nihilanth::{HEAD_OPEN_SECONDS, NihilanthShield};
    use crate::monsters::table::{GARGANTUA_VULNERABILITY, MonsterKind, damage_response_for};
    use crate::state::Classification;
    use crate::world::{Actor, AiEventKind, Impervious};
    use glam::Vec3;
    use hecs::World;

    /// The gargantua's published immunity, through the same intake every
    /// other monster uses: bullets and untyped hits cost nothing, a blast
    /// costs health, and a blast can kill.
    #[test]
    fn a_gargantua_loses_health_only_to_its_published_damage_types() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let garg =
            world.spawn((Actor::new(Classification::AlienMonster, Vec3::ZERO).with_health(800.0),));
        let vulnerability = |entity: hecs::Entity| {
            if entity == garg {
                damage_response_for(&MonsterKind::Gargantua)
            } else {
                DamageResponse::ORDINARY
            }
        };
        assert_eq!(vulnerability(garg).vulnerable_to, GARGANTUA_VULNERABILITY);

        let mut small_arms = DamageQueue::new();
        small_arms.push_damage(
            DamageEvent::new(garg, attacker, 300.0, Vec3::ZERO).with_kinds(DamageKinds::BULLET),
        );
        small_arms.push_damage(
            DamageEvent::new(garg, attacker, 300.0, Vec3::ZERO).with_kinds(DamageKinds::SLASH),
        );
        small_arms.push_damage(DamageEvent::new(garg, attacker, 300.0, Vec3::ZERO));
        let (events, _) = apply_damage_effective(
            &mut world,
            &small_arms,
            DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &vulnerability,
        );
        assert!(events.is_empty());
        let actor = *world.get::<&Actor>(garg).expect("actor");
        assert!((actor.health - 800.0).abs() < 1e-4, "nothing got through");
        assert!(actor.alive);

        let mut explosives = DamageQueue::new();
        explosives.push_damage(
            DamageEvent::new(garg, attacker, 100.0, Vec3::ZERO).with_kinds(DamageKinds::BLAST),
        );
        explosives.push_damage(
            DamageEvent::new(garg, attacker, 100.0, Vec3::ZERO).with_kinds(DamageKinds::BULLET),
        );
        let (events, _) = apply_damage_effective(
            &mut world,
            &explosives,
            DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &vulnerability,
        );
        assert!(events.is_empty());
        let actor = *world.get::<&Actor>(garg).expect("actor");
        assert!(
            (actor.health - 700.0).abs() < 1e-4,
            "only the blast counted"
        );

        let mut beam = DamageQueue::new();
        beam.push_damage(
            DamageEvent::new(garg, attacker, 700.0, Vec3::ZERO).with_kinds(DamageKinds::ENERGYBEAM),
        );
        let (events, corpses) = apply_damage_effective(
            &mut world,
            &beam,
            DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &vulnerability,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, AiEventKind::Died));
        assert_eq!(corpses.len(), 1);
        assert!(!world.get::<&Actor>(garg).expect("actor").alive);
    }

    /// The Apache's published "blast damage doubles damage", through the
    /// same intake: a blast costs twice its amount, a bullet its own.
    #[test]
    fn an_apache_takes_double_from_a_blast_and_face_value_from_a_bullet() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let apache = world
            .spawn((Actor::new(Classification::HumanMilitary, Vec3::ZERO).with_health(250.0),));
        let response = |_: hecs::Entity| damage_response_for(&MonsterKind::Apache);
        let hit = |amount: f32, kinds: DamageKinds| {
            let mut queue = DamageQueue::new();
            queue.push_damage(
                DamageEvent::new(apache, attacker, amount, Vec3::ZERO).with_kinds(kinds),
            );
            queue
        };
        let _ = apply_damage_effective(
            &mut world,
            &hit(10.0, DamageKinds::BULLET),
            DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &response,
        );
        assert!((world.get::<&Actor>(apache).expect("actor").health - 240.0).abs() < 1e-4);
        let _ = apply_damage_effective(
            &mut world,
            &hit(100.0, DamageKinds::BLAST),
            DEFAULT_GIB_OVERKILL_MULTIPLIER,
            &response,
        );
        assert!(
            (world.get::<&Actor>(apache).expect("actor").health - 40.0).abs() < 1e-4,
            "the blast counted twice"
        );
    }

    /// `Impervious` is the empty response here too, whatever the caller's
    /// own lookup says: no hit of any type costs such a target anything.
    #[test]
    fn an_impervious_target_loses_nothing_whatever_the_response_says() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let prop = world.spawn((
            Actor::new(Classification::PlayerAlly, Vec3::ZERO).with_health(8.0),
            Impervious,
        ));
        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::new(prop, attacker, 500.0, Vec3::ZERO));
        queue.push_damage(
            DamageEvent::new(prop, attacker, 500.0, Vec3::ZERO).with_kinds(DamageKinds::BLAST),
        );
        let (events, corpses) =
            apply_damage_with_corpses(&mut world, &queue, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!(corpses.is_empty());
        let actor = *world.get::<&Actor>(prop).expect("actor");
        assert!(actor.alive);
        assert!((actor.health - 8.0).abs() < 1e-6);
    }

    /// The default entry point stays fully vulnerable, so every existing
    /// caller keeps its behaviour.
    #[test]
    fn the_untyped_entry_point_treats_every_target_as_fully_vulnerable() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let victim =
            world.spawn((Actor::new(Classification::AlienMonster, Vec3::ZERO).with_health(10.0),));
        let mut queue = DamageQueue::new();
        queue.push_damage(
            DamageEvent::new(victim, attacker, 10.0, Vec3::ZERO).with_kinds(DamageKinds::BULLET),
        );
        let events = apply_damage(&mut world, &queue, DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert_eq!(events.len(), 1);
    }

    /// The Nihilanth's reserve and crystals gate its health: hits drain the
    /// reserve, then are blocked while a crystal stands, and only an
    /// exposed boss can die — and then exactly once.
    #[test]
    fn a_nihilanth_dies_only_once_its_reserve_is_gone_and_its_crystals_are_down() {
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let mut shield = NihilanthShield::for_health(800.0, 20, 1);
        shield.activate();
        let boss = world.spawn((
            Actor::new(Classification::AlienMilitary, Vec3::ZERO).with_health(800.0),
            shield,
        ));
        let hit = |amount: f32| {
            let mut queue = DamageQueue::new();
            queue.push_damage(DamageEvent::new(boss, attacker, amount, Vec3::ZERO));
            queue
        };

        // Drains the reserve, health untouched.
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(500.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!((world.get::<&Actor>(boss).expect("actor").health - 800.0).abs() < 1e-4);
        assert!(
            (world
                .get::<&NihilanthShield>(boss)
                .expect("shield")
                .reserve()
                - 300.0)
                .abs()
                < 1e-4
        );

        // Reserve gone, a crystal standing: blocked.
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(5_000.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(5_000.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!((world.get::<&Actor>(boss).expect("actor").health - 800.0).abs() < 1e-4);

        // The crystal falls and the head opens: hits cost health, and the
        // killing one reports exactly one death.
        {
            let mut shield = world.get::<&mut NihilanthShield>(boss).expect("shield");
            shield.sync_crystals(0);
            assert!(shield.tick(HEAD_OPEN_SECONDS + 1.0));
        }
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(300.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!((world.get::<&Actor>(boss).expect("actor").health - 500.0).abs() < 1e-4);
        let (events, corpses) =
            apply_damage_with_corpses(&mut world, &hit(500.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, AiEventKind::Died));
        assert_eq!(corpses, vec![(boss, CorpseDecision::Corpse)]);
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(500.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty(), "dead is dead");
    }

    /// The Gonarch's trail through the same intake: shielded on the way,
    /// fought down at a node with health, sent on (not killed) by the hit
    /// that depletes it, and killed only at the trail's end.
    #[test]
    fn a_gonarch_is_sent_on_by_a_depleting_hit_and_dies_only_at_the_end() {
        let node = |name: &str, next: Option<&str>, health: Option<f32>| TrailNode {
            name: name.to_string(),
            position: Vec3::ZERO,
            next: next.map(str::to_string),
            health,
            wait: 0.0,
            fire_on_reach: None,
            kill_on_reach: None,
            sequence_on_reach: None,
            run: false,
            wait_indefinitely: false,
        };
        let trail = Trail::new(vec![
            node("a", Some("b"), Some(100.0)),
            node("b", None, Some(60.0)),
        ]);
        let mut world = World::new();
        let attacker = world.spawn((0u8,));
        let gonarch = world.spawn((
            Actor::new(Classification::AlienMonster, Vec3::ZERO).with_health(225.0),
            GonarchTrail::new(trail, 1.5, 225.0),
        ));
        let hit = |amount: f32| {
            let mut queue = DamageQueue::new();
            queue.push_damage(DamageEvent::new(gonarch, attacker, amount, Vec3::ZERO));
            queue
        };

        // Travelling: shielded.
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(10_000.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!((world.get::<&Actor>(gonarch).expect("actor").health - 225.0).abs() < 1e-4);

        // At node a with its (scaled) health: fought down, then sent on with
        // its spawn health restored rather than killed.
        {
            let mut trail = world.get::<&mut GonarchTrail>(gonarch).expect("trail");
            let effects = trail.arrive().expect("at a");
            world.get::<&mut Actor>(gonarch).expect("actor").health =
                effects.set_health.expect("health");
        }
        assert!((world.get::<&Actor>(gonarch).expect("actor").health - 150.0).abs() < 1e-4);
        let (events, _) =
            apply_damage_with_corpses(&mut world, &hit(100.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty());
        assert!((world.get::<&Actor>(gonarch).expect("actor").health - 50.0).abs() < 1e-4);
        let (events, corpses) =
            apply_damage_with_corpses(&mut world, &hit(1_000.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert!(events.is_empty(), "depletion is a departure, not a death");
        assert!(corpses.is_empty());
        let actor = *world.get::<&Actor>(gonarch).expect("actor");
        assert!(actor.alive);
        assert!((actor.health - 225.0).abs() < 1e-4, "spawn health restored");
        assert_eq!(
            world.get::<&GonarchTrail>(gonarch).expect("trail").phase(),
            TrailPhase::Traveling { to: 1 }
        );

        // At the last node: the depleting hit kills, exactly once.
        {
            let mut trail = world.get::<&mut GonarchTrail>(gonarch).expect("trail");
            let effects = trail.arrive().expect("at b");
            world.get::<&mut Actor>(gonarch).expect("actor").health =
                effects.set_health.expect("health");
        }
        let (events, corpses) =
            apply_damage_with_corpses(&mut world, &hit(90.0), DEFAULT_GIB_OVERKILL_MULTIPLIER);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, AiEventKind::Died));
        assert_eq!(corpses.len(), 1);
        assert!(!world.get::<&Actor>(gonarch).expect("actor").alive);
    }

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
