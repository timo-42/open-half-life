//! Combat events: the push-only interface to presentation.
//!
//! Simulation code appends events to a [`CombatEventQueue`]; the host
//! application drains it each frame and turns entries into sounds, HUD
//! updates and effects. `ohl-combat` therefore has no dependency on
//! `ohl-render`, `ohl-audio` or `ohl-ui`.
//!
//! The queue is bounded: a tick that somehow produces more events than the
//! capacity drops the excess and counts it, instead of growing without limit.

use glam::Vec3;

use crate::damage::DamageType;
use crate::trace::{EntityId, HitGroup};

/// What an attack hit, for choosing an impact effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SurfaceKind {
    /// Solid world geometry.
    #[default]
    World,
    /// An entity's hitbox.
    Entity,
    /// A liquid surface.
    Liquid,
    /// The sky, which absorbs the shot without an effect.
    Sky,
}

/// One thing that happened in combat this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum CombatEvent {
    /// Damage reached a target.
    DamageDealt {
        /// Who was hurt.
        target: EntityId,
        /// Who is credited, when known.
        attacker: Option<EntityId>,
        /// Hit points removed.
        health_lost: f32,
        /// Armour points removed.
        armor_lost: f32,
        /// The kinds of damage applied.
        kind: DamageType,
    },
    /// A target's health reached zero. Emitted once per target.
    Killed {
        /// Who died.
        target: EntityId,
        /// Who is credited, when known.
        attacker: Option<EntityId>,
    },
    /// An attack struck a surface.
    Impact {
        /// What kind of surface was struck.
        surface: SurfaceKind,
        /// The world-space impact point.
        position: Vec3,
        /// The surface normal at the impact point.
        normal: Vec3,
        /// The hit group struck, for an entity impact.
        hitgroup: HitGroup,
    },
}

/// A bounded FIFO of [`CombatEvent`]s.
#[derive(Debug, Clone)]
pub struct CombatEventQueue {
    events: Vec<CombatEvent>,
    capacity: usize,
    dropped: usize,
}

impl Default for CombatEventQueue {
    /// A queue holding [`CombatEventQueue::DEFAULT_CAPACITY`] events.
    fn default() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }
}

impl CombatEventQueue {
    /// The default capacity: enough for a busy tick (a shotgun blast, a
    /// squad reacting and an explosion) with room to spare.
    pub const DEFAULT_CAPACITY: usize = 256;

    /// An empty queue holding at most `capacity` events (at least one).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            events: Vec::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    /// Appends `event`, returning `false` when the queue is full and the
    /// event was dropped.
    pub fn push(&mut self, event: CombatEvent) -> bool {
        if self.events.len() >= self.capacity {
            self.dropped += 1;
            return false;
        }
        self.events.push(event);
        true
    }

    /// The queued events, oldest first.
    #[must_use]
    pub fn events(&self) -> &[CombatEvent] {
        &self.events
    }

    /// How many events are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// How many events have been dropped since the last [`clear`](Self::clear).
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// The queue's capacity.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Removes every event and resets the dropped counter, keeping the
    /// allocation for the next tick.
    pub fn clear(&mut self) {
        self.events.clear();
        self.dropped = 0;
    }

    /// Drains the queue into an iterator, resetting the dropped counter.
    pub fn drain(&mut self) -> impl Iterator<Item = CombatEvent> + '_ {
        self.dropped = 0;
        self.events.drain(..)
    }
}
