//! The minimal damage input this crate needs.
//!
//! **Temporary, and deliberately minimal.** Package 7.1 (`ohl-combat`) owns
//! the real `DamageInfo`, with damage-type bitflags, an inflictor, a hit
//! group and a hit position. `ohl-ai` must not depend on `ohl-combat` yet, so
//! it defines the smallest shape its senses need — who hurt me, how much,
//! from where — plus the [`DamageSink`] trait `ohl-combat` can implement so
//! the two are unified by simply pointing `ohl-ai` at the richer type.
//!
//! Nothing here is a published GoldSrc structure; it is a project-owned
//! interface between two of our own crates.

use glam::Vec3;
use hecs::Entity;

/// The largest number of damage events queued between ticks, so a runaway
/// emitter cannot grow the queue without bound.
pub const MAX_DAMAGE_EVENTS: usize = 256;

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
}

impl DamageEvent {
    /// A provoking hit from a known attacker.
    #[must_use]
    pub fn new(target: Entity, attacker: Entity, amount: f32, source_position: Vec3) -> Self {
        Self {
            target,
            attacker: Some(attacker),
            amount,
            source_position,
            provokes: true,
        }
    }

    /// A hit with no attacker — a fall, a crusher, drowning.
    #[must_use]
    pub fn environmental(target: Entity, amount: f32, source_position: Vec3) -> Self {
        Self {
            target,
            attacker: None,
            amount,
            source_position,
            provokes: false,
        }
    }

    /// Whether the event carries usable numbers.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        self.amount.is_finite() && self.amount > 0.0 && self.source_position.is_finite()
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
#[must_use]
pub fn summarize(queue: &DamageQueue, target: Entity) -> Option<(f32, Option<Entity>, Vec3, bool)> {
    let mut total = 0.0f32;
    let mut worst = f32::NEG_INFINITY;
    let mut attacker = None;
    let mut position = Vec3::ZERO;
    let mut provoked = false;
    for event in queue.for_target(target) {
        total += event.amount;
        provoked |= event.provokes;
        if event.amount > worst {
            worst = event.amount;
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
    use super::{DamageEvent, DamageQueue, DamageSink, MAX_DAMAGE_EVENTS, summarize};
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
}
