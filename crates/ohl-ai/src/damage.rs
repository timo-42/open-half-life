//! The minimal damage input this crate needs.
//!
//! **Deliberately minimal.** Package 7.1 (`ohl-combat`) owns the real
//! `DamageInfo`, with an inflictor, a hit group and a hit position. `ohl-ai`
//! must not depend on `ohl-combat` (`xtask/src/graph.rs` lists no such
//! edge), so it defines the smallest shape its senses and its lifecycle need
//! — who hurt me, how much, from where, and **what kind of hurt it was** —
//! plus the [`DamageSink`] trait `ohl-combat`'s host can implement so the two
//! are unified by simply pointing `ohl-ai` at the richer type.
//!
//! [`DamageKinds`] is the one piece of published vocabulary here: the
//! damage-type names a mapper selects from on `trigger_hurt` (generic,
//! crush, bullet, slash, burn, freeze, fall, blast, club, shock, sonic,
//! energy beam, drown, paralyze, nerve gas, poison, radiation, acid,
//! slowburn, slowfreeze; see `docs/FORMAT_SOURCES.md`, "Combat and damage").
//! It is the same set, in the same declaration order and with the same
//! dense bit assignment, as `ohl_combat::DamageType`, so the host can
//! convert one to the other by bits; `ohl-engine` asserts that parity in a
//! test where both crates are visible. Nothing else here is a published
//! GoldSrc structure; it is a project-owned interface between two of our own
//! crates.

use core::fmt;

use glam::Vec3;
use hecs::Entity;

/// The largest number of damage events queued between ticks, so a runaway
/// emitter cannot grow the queue without bound.
pub const MAX_DAMAGE_EVENTS: usize = 256;

/// A set of damage types, as a bitmask (see the module doc comment).
///
/// Constants combine with `|` and are tested with [`Self::contains`] /
/// [`Self::intersects`]. [`Self::ALL`] is what an ordinary monster is
/// vulnerable to; a species with a published rule (the gargantua's
/// immunity, see `crate::monsters::table::damage_response_for`) narrows it
/// through a [`DamageResponse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DamageKinds(u32);

macro_rules! damage_kinds {
    ($($(#[$meta:meta])* $name:ident = $bit:expr, $label:literal;)*) => {
        impl DamageKinds {
            $($(#[$meta])* pub const $name: Self = Self(1 << $bit);)*

            /// Every named damage type.
            pub const ALL: Self = Self($((1u32 << $bit) |)* 0);

            /// The named types in declaration order, with their published
            /// labels (the same labels `ohl_combat::DamageType::NAMED`
            /// carries, in the same order).
            pub const NAMED: &'static [(Self, &'static str)] =
                &[$((Self(1 << $bit), $label),)*];
        }
    };
}

damage_kinds! {
    /// Untyped damage; the default when nothing more specific applies.
    GENERIC = 0, "generic";
    /// Being squashed by a mover.
    CRUSH = 1, "crush";
    /// Hitscan firearms.
    BULLET = 2, "bullet";
    /// Cutting melee attacks.
    SLASH = 3, "slash";
    /// Fire.
    BURN = 4, "burn";
    /// Cold.
    FREEZE = 5, "freeze";
    /// Falling too far.
    FALL = 6, "fall";
    /// Explosions.
    BLAST = 7, "blast";
    /// Blunt melee attacks.
    CLUB = 8, "club";
    /// Electricity.
    SHOCK = 9, "shock";
    /// Sonic attacks.
    SONIC = 10, "sonic";
    /// Energy beams.
    ENERGYBEAM = 11, "energybeam";
    /// Running out of air underwater.
    DROWN = 12, "drown";
    /// Paralysis.
    PARALYZE = 13, "paralyze";
    /// Nerve gas.
    NERVEGAS = 14, "nervegas";
    /// Poison.
    POISON = 15, "poison";
    /// Radiation.
    RADIATION = 16, "radiation";
    /// Acid.
    ACID = 17, "acid";
    /// A lingering burn.
    SLOWBURN = 18, "slowburn";
    /// A lingering freeze.
    SLOWFREEZE = 19, "slowfreeze";
}

impl DamageKinds {
    /// The empty set: hurts nothing, and (as a vulnerability) is hurt by
    /// nothing.
    pub const NONE: Self = Self(0);

    /// The raw bitmask.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// A set from a raw bitmask, keeping only named bits.
    #[must_use]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// Whether every type in `other` is present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any type in `other` is present.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether no type is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The union of two sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOr for DamageKinds {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitOrAssign for DamageKinds {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for DamageKinds {
    /// Lists the set types by their labels, `+`-separated, or `none`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut first = true;
        for (flag, label) in Self::NAMED {
            if self.contains(*flag) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(label)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// One monster being hurt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    /// Who was hurt.
    pub target: Entity,
    /// Who did it, when that is known.
    pub attacker: Option<Entity>,
    /// How much, in health points.
    pub amount: f32,
    /// Where it came from, used to face the attacker.
    pub source_position: Vec3,
    /// Whether the attack should make an otherwise indifferent monster
    /// hostile (sets [`crate::Conditions::PROVOKED`]).
    pub provokes: bool,
    /// What kind of damage this is. [`DamageKinds::GENERIC`] when the
    /// producer did not say; a species' [`DamageResponse`]
    /// (`crate::monsters::table::damage_response_for`) is matched against
    /// this, see [`Self::hurts`] and [`DamageResponse::scale`].
    pub kinds: DamageKinds,
}

impl DamageEvent {
    /// A provoking, untyped hit from a known attacker.
    #[must_use]
    pub fn new(target: Entity, attacker: Entity, amount: f32, source_position: Vec3) -> Self {
        Self {
            target,
            attacker: Some(attacker),
            amount,
            source_position,
            provokes: true,
            kinds: DamageKinds::GENERIC,
        }
    }

    /// An untyped hit with no attacker — a fall, a crusher, drowning.
    #[must_use]
    pub fn environmental(target: Entity, amount: f32, source_position: Vec3) -> Self {
        Self {
            target,
            attacker: None,
            amount,
            source_position,
            provokes: false,
            kinds: DamageKinds::GENERIC,
        }
    }

    /// The same event typed as `kinds`.
    #[must_use]
    pub fn with_kinds(mut self, kinds: DamageKinds) -> Self {
        self.kinds = kinds;
        self
    }

    /// Whether the event carries usable numbers.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.amount.is_finite() && self.amount > 0.0 && self.source_position.is_finite()
    }

    /// Whether a target vulnerable to exactly `vulnerable_to` loses health
    /// to this event.
    ///
    /// A fully vulnerable target ([`DamageKinds::ALL`]) is hurt by every
    /// event, including an untyped one; a narrowed vulnerability is hurt
    /// only by an event sharing at least one of its types, so an untyped
    /// or empty-typed event never gets past a published immunity.
    #[must_use]
    pub fn hurts(&self, vulnerable_to: DamageKinds) -> bool {
        vulnerable_to.contains(DamageKinds::ALL) || self.kinds.intersects(vulnerable_to)
    }
}

/// How a target's health answers each damage type: which types take
/// health off it at all, and which take twice their amount.
///
/// [`Self::ORDINARY`] for every monster without a published rule; a
/// species with one (`crate::monsters::table::damage_response_for`: the
/// gargantua's immunity, the Apache's doubled blast damage) narrows or
/// doubles it. [`Self::IMPERVIOUS`] is the empty response, which
/// `crate::world::Impervious` reads as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageResponse {
    /// The types that take health off the target; see
    /// [`DamageEvent::hurts`].
    pub vulnerable_to: DamageKinds,
    /// The types that take twice their amount.
    pub doubled_by: DamageKinds,
}

impl DamageResponse {
    /// Hurt by everything, at face value.
    pub const ORDINARY: Self = Self {
        vulnerable_to: DamageKinds::ALL,
        doubled_by: DamageKinds::NONE,
    };

    /// Hurt by nothing.
    pub const IMPERVIOUS: Self = Self {
        vulnerable_to: DamageKinds::NONE,
        doubled_by: DamageKinds::NONE,
    };

    /// What one unit of an `event` costs the target: `0.0` when the event
    /// does not get past [`Self::vulnerable_to`], `2.0` when it shares a
    /// type with [`Self::doubled_by`], `1.0` otherwise.
    #[must_use]
    pub fn scale(&self, event: &DamageEvent) -> f32 {
        if !event.hurts(self.vulnerable_to) {
            0.0
        } else if event.kinds.intersects(self.doubled_by) {
            2.0
        } else {
            1.0
        }
    }
}

impl Default for DamageResponse {
    fn default() -> Self {
        Self::ORDINARY
    }
}

/// Accepts damage events for the AI to consume on the next tick.
///
/// Implemented by [`DamageQueue`] here and, later, by whatever `ohl-combat`
/// hands the AI; the trait is the seam along which the two are unified.
pub trait DamageSink {
    /// Queues one event. Returns whether it was kept.
    fn push_damage(&mut self, event: DamageEvent) -> bool;
}

/// A bounded, order-preserving queue of damage events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DamageQueue {
    events: Vec<DamageEvent>,
}

impl DamageQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The queued events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[DamageEvent] {
        &self.events
    }

    /// The queued events aimed at `target`.
    pub fn for_target(&self, target: Entity) -> impl Iterator<Item = &DamageEvent> {
        self.events
            .iter()
            .filter(move |event| event.target == target)
    }

    /// The number of queued events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Empties the queue.
    pub fn clear(&mut self) {
        self.events.clear();
    }
}

impl DamageSink for DamageQueue {
    fn push_damage(&mut self, event: DamageEvent) -> bool {
        if !event.is_usable() || self.events.len() >= MAX_DAMAGE_EVENTS {
            return false;
        }
        self.events.push(event);
        true
    }
}

/// The total damage aimed at `target`, and the attacker of the hardest hit.
///
/// Every queued hit counts, at face value, whatever its type: this is what
/// a monster *notices* (it turns toward and acquires whoever shot it even
/// when its species shrugs the hit off). What it actually *loses* is
/// [`summarize_effective`]'s job.
#[must_use]
pub fn summarize(queue: &DamageQueue, target: Entity) -> Option<(f32, Option<Entity>, Vec3, bool)> {
    summarize_effective(queue, target, DamageResponse::ORDINARY)
}

/// As [`summarize`], each hit weighed by `response`
/// ([`DamageResponse::scale`]): the hits it shrugs off drop out, and the
/// ones it takes double count twice.
#[must_use]
pub fn summarize_effective(
    queue: &DamageQueue,
    target: Entity,
    response: DamageResponse,
) -> Option<(f32, Option<Entity>, Vec3, bool)> {
    let mut total = 0.0f32;
    let mut worst = f32::NEG_INFINITY;
    let mut attacker = None;
    let mut position = Vec3::ZERO;
    let mut provoked = false;
    for event in queue.for_target(target) {
        let amount = event.amount * response.scale(event);
        if amount <= 0.0 {
            continue;
        }
        total += amount;
        provoked |= event.provokes;
        if amount > worst {
            worst = amount;
            attacker = event.attacker;
            position = event.source_position;
        }
    }
    if total > 0.0 {
        Some((total, attacker, position, provoked))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DamageEvent, DamageKinds, DamageQueue, DamageResponse, DamageSink, MAX_DAMAGE_EVENTS,
        summarize, summarize_effective,
    };
    use glam::Vec3;
    use hecs::World;

    #[test]
    fn the_queue_is_bounded_and_rejects_unusable_events() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let attacker = world.spawn((0u8,));
        let mut queue = DamageQueue::new();
        assert!(queue.is_empty());
        assert!(!queue.push_damage(DamageEvent::new(target, attacker, 0.0, Vec3::ZERO)));
        assert!(!queue.push_damage(DamageEvent::new(target, attacker, f32::NAN, Vec3::ZERO)));
        assert!(!queue.push_damage(DamageEvent::new(target, attacker, 1.0, Vec3::NAN)));
        for _ in 0..(MAX_DAMAGE_EVENTS + 4) {
            queue.push_damage(DamageEvent::new(target, attacker, 1.0, Vec3::ZERO));
        }
        assert_eq!(queue.len(), MAX_DAMAGE_EVENTS);
        queue.clear();
        assert!(queue.events().is_empty());
    }

    #[test]
    fn a_summary_totals_the_hits_and_names_the_hardest_attacker() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let other = world.spawn((0u8,));
        let weak = world.spawn((0u8,));
        let strong = world.spawn((0u8,));
        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::new(target, weak, 3.0, Vec3::X));
        queue.push_damage(DamageEvent::new(target, strong, 30.0, Vec3::Y));
        queue.push_damage(DamageEvent::environmental(target, 5.0, Vec3::Z));
        queue.push_damage(DamageEvent::new(other, weak, 100.0, Vec3::X));

        let (total, attacker, position, provoked) =
            summarize(&queue, target).expect("the target was hit");
        assert!((total - 38.0).abs() < 1e-4);
        assert_eq!(attacker, Some(strong));
        assert_eq!(position, Vec3::Y);
        assert!(provoked);
        assert_eq!(queue.for_target(target).count(), 3);

        let (_, attacker, _, provoked) = summarize(&queue, other).expect("hit too");
        assert_eq!(attacker, Some(weak));
        assert!(provoked);

        let untouched = world.spawn((0u8,));
        assert!(summarize(&queue, untouched).is_none());
    }

    #[test]
    fn environmental_damage_does_not_provoke() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let mut queue = DamageQueue::new();
        queue.push_damage(DamageEvent::environmental(target, 10.0, Vec3::ZERO));
        let (_, attacker, _, provoked) = summarize(&queue, target).expect("hit");
        assert!(attacker.is_none());
        assert!(!provoked);
    }

    #[test]
    fn every_damage_kind_is_a_distinct_single_bit_inside_all() {
        let mut seen = 0u32;
        for (kind, label) in DamageKinds::NAMED {
            assert_eq!(kind.bits().count_ones(), 1, "{label} is not a single bit");
            assert_eq!(seen & kind.bits(), 0, "{label} reuses an assigned bit");
            assert!(DamageKinds::ALL.contains(*kind));
            seen |= kind.bits();
        }
        assert_eq!(seen, DamageKinds::ALL.bits());
        assert_eq!(
            DamageKinds::from_bits_truncate(u32::MAX),
            DamageKinds::ALL,
            "unnamed bits are dropped"
        );
        assert_eq!(DamageKinds::NONE.to_string(), "none");
        assert_eq!(
            (DamageKinds::BLAST | DamageKinds::CRUSH).to_string(),
            "crush+blast",
            "labels print in declaration order"
        );
    }

    #[test]
    fn an_untyped_event_is_generic_and_a_typed_one_keeps_its_kinds() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let attacker = world.spawn((0u8,));
        let plain = DamageEvent::new(target, attacker, 1.0, Vec3::ZERO);
        assert_eq!(plain.kinds, DamageKinds::GENERIC);
        assert_eq!(
            DamageEvent::environmental(target, 1.0, Vec3::ZERO).kinds,
            DamageKinds::GENERIC
        );
        let typed = plain.with_kinds(DamageKinds::BLAST | DamageKinds::BURN);
        assert!(typed.kinds.contains(DamageKinds::BLAST));
        assert!(typed.kinds.contains(DamageKinds::BURN));
        assert!(!typed.kinds.contains(DamageKinds::BULLET));
    }

    /// The vulnerability rule: everything hurts a fully vulnerable target,
    /// only a matching type hurts a narrowed one, and an untyped or
    /// empty-typed hit never gets past an immunity.
    #[test]
    fn hurts_matches_types_against_a_vulnerability() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let attacker = world.spawn((0u8,));
        let bullet =
            DamageEvent::new(target, attacker, 1.0, Vec3::ZERO).with_kinds(DamageKinds::BULLET);
        let blast =
            DamageEvent::new(target, attacker, 1.0, Vec3::ZERO).with_kinds(DamageKinds::BLAST);
        let untyped = DamageEvent::new(target, attacker, 1.0, Vec3::ZERO);
        let empty = untyped.with_kinds(DamageKinds::NONE);
        let blast_only = DamageKinds::BLAST | DamageKinds::CRUSH;

        assert!(bullet.hurts(DamageKinds::ALL));
        assert!(untyped.hurts(DamageKinds::ALL));
        assert!(empty.hurts(DamageKinds::ALL));
        assert!(blast.hurts(blast_only));
        assert!(!bullet.hurts(blast_only));
        assert!(!untyped.hurts(blast_only));
        assert!(!empty.hurts(blast_only));
        assert!(!blast.hurts(DamageKinds::NONE));
    }

    #[test]
    fn a_vulnerable_summary_drops_the_hits_that_do_not_get_through() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let gunner = world.spawn((0u8,));
        let bomber = world.spawn((0u8,));
        let mut queue = DamageQueue::new();
        queue.push_damage(
            DamageEvent::new(target, gunner, 90.0, Vec3::X).with_kinds(DamageKinds::BULLET),
        );
        queue.push_damage(
            DamageEvent::new(target, bomber, 30.0, Vec3::Y).with_kinds(DamageKinds::BLAST),
        );

        // Noticing counts everything: the gunner hit hardest.
        let (total, attacker, _, _) = summarize(&queue, target).expect("hit");
        assert!((total - 120.0).abs() < 1e-4);
        assert_eq!(attacker, Some(gunner));

        // Losing health counts only what gets through.
        let blast_only = DamageResponse {
            vulnerable_to: DamageKinds::BLAST,
            doubled_by: DamageKinds::NONE,
        };
        let (total, attacker, position, _) =
            summarize_effective(&queue, target, blast_only).expect("the blast counts");
        assert!((total - 30.0).abs() < 1e-4);
        assert_eq!(attacker, Some(bomber));
        assert_eq!(position, Vec3::Y);
        let burn_only = DamageResponse {
            vulnerable_to: DamageKinds::BURN,
            doubled_by: DamageKinds::NONE,
        };
        assert!(summarize_effective(&queue, target, burn_only).is_none());
        assert!(summarize_effective(&queue, target, DamageResponse::IMPERVIOUS).is_none());
    }

    /// A doubled type counts twice, and can make its hit the hardest one.
    #[test]
    fn a_doubled_type_counts_twice_and_can_become_the_hardest_hit() {
        let mut world = World::new();
        let target = world.spawn((0u8,));
        let gunner = world.spawn((0u8,));
        let bomber = world.spawn((0u8,));
        let mut queue = DamageQueue::new();
        queue.push_damage(
            DamageEvent::new(target, gunner, 40.0, Vec3::X).with_kinds(DamageKinds::BULLET),
        );
        queue.push_damage(
            DamageEvent::new(target, bomber, 30.0, Vec3::Y)
                .with_kinds(DamageKinds::BLAST | DamageKinds::BURN),
        );
        let doubles_blast = DamageResponse {
            vulnerable_to: DamageKinds::ALL,
            doubled_by: DamageKinds::BLAST,
        };
        let (total, attacker, _, _) =
            summarize_effective(&queue, target, doubles_blast).expect("hit");
        assert!((total - 100.0).abs() < 1e-4, "{total}");
        assert_eq!(attacker, Some(bomber), "60 beats 40");
        let (total, attacker, _, _) =
            summarize_effective(&queue, target, DamageResponse::ORDINARY).expect("hit");
        assert!((total - 70.0).abs() < 1e-4);
        assert_eq!(attacker, Some(gunner));
        assert_eq!(DamageResponse::default(), DamageResponse::ORDINARY);
    }
}
