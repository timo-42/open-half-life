//! A minimal, deterministic, fixed-timestep map logic simulation.
//!
//! Covers only what a `hecs`-registered entity set can drive without
//! rendering, audio, physics or AI: door/button/platform state machines,
//! `multi_manager` fan-out scheduling, `trigger_once`/`trigger_multiple`
//! dispatch (including their `wait` cooldown), and `trigger_changelevel`
//! signalling a [`LevelChange`] event back to the caller. Semantics are
//! taken only from public mapping documentation; see
//! `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic").

use glam::Vec3;
use hecs::Entity;

use crate::registry::{
    Button, ChangeLevel, Door, Message, MoverState, MultiManager, Platform, Registry, Transform,
    Trigger,
};
use crate::track_train::TrackTrainState;

/// Finds the closest `func_door` or `func_button` within `radius` units of
/// `position`, preferring a brush entity's precomputed bounding-box centre
/// (see [`crate::registry::BrushCenter`]) over its `Transform::origin` when
/// both exist, since brush entities conventionally leave `origin` at
/// `0 0 0`. Intended for a "use the nearest usable thing" input binding.
#[must_use]
pub fn find_usable_within(registry: &Registry, position: Vec3, radius: f32) -> Option<Entity> {
    let mut best: Option<(Entity, f32)> = None;
    let mut consider = |entity: Entity, transform: &Transform| {
        let center = registry
            .world
            .get::<&crate::registry::BrushCenter>(entity)
            .map_or(transform.origin, |c| c.0);
        let distance = center.distance(position);
        if distance <= radius && best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((entity, distance));
        }
    };
    for (entity, transform) in &mut registry
        .world
        .query::<(Entity, &Transform)>()
        .with::<&Door>()
    {
        consider(entity, transform);
    }
    for (entity, transform) in &mut registry
        .world
        .query::<(Entity, &Transform)>()
        .with::<&Button>()
    {
        consider(entity, transform);
    }
    best.map(|(entity, _)| entity)
}

/// The documented cap on how many `Fire` events one tick will drain from
/// the queue, so a pathological chain of zero-delay `multi_manager`s cannot
/// spin forever.
const MAX_EVENTS_PER_TICK: usize = 4096;

/// The largest number of scheduled events kept at once.
const MAX_PENDING_EVENTS: usize = 4096;

/// A scheduled "fire this target" event, counting down `delay` seconds.
#[derive(Debug, Clone)]
pub struct Fire {
    /// The `targetname` to look up and activate.
    pub target: String,
    /// The entity that caused this, if any (propagated so a fired door,
    /// for instance, could in principle attribute damage or a `use`
    /// originator; unused by the state machines below today).
    pub activator: Option<Entity>,
    /// Seconds remaining before this fires.
    pub delay: f32,
}

/// An externally visible outcome of the simulation the caller must act on
/// (there is no in-crate handling for a level change: loading the next map
/// is the caller's job).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A `trigger_changelevel` fired.
    LevelChange(LevelChange),
    /// An `env_message`/`game_text` fired; the caller resolves the
    /// `titles.txt` entry (when [`Message::literal`] is `false`) and shows
    /// it.
    Message(Message),
}

/// The destination of a level transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelChange {
    /// The `map` keyvalue.
    pub map: String,
    /// The `landmark` keyvalue.
    pub landmark: String,
}

/// Per-entity cooldown state for `trigger_once`/`trigger_multiple`, kept
/// outside the `hecs` world since it is simulation bookkeeping rather than
/// map data.
#[derive(Debug, Default)]
struct TriggerState {
    used: bool,
    cooldown: f32,
}

/// One scheduled event, as stored in a save file.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PendingFire {
    /// The `targetname` this event will activate.
    pub target: String,
    /// The activator's `hecs` bit pattern, when it had one.
    pub activator: Option<u64>,
    /// Seconds remaining before it fires.
    pub delay: f32,
}

/// One trigger's cooldown state, as stored in a save file.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TriggerSnapshot {
    /// The trigger entity's `hecs` bit pattern.
    pub entity: u64,
    /// Whether a `trigger_once` has already fired.
    pub used: bool,
    /// Seconds left before the trigger may fire again.
    pub cooldown: f32,
}

/// A [`Simulation`]'s persistable bookkeeping: what is scheduled and which
/// triggers are spent or cooling down.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimulationState {
    /// Scheduled events, in queue order.
    pub pending: Vec<PendingFire>,
    /// Per-trigger cooldown state, ordered by entity.
    pub triggers: Vec<TriggerSnapshot>,
}

/// The map logic simulation: an event queue plus per-tick state-machine
/// advancement.
#[derive(Debug, Default)]
pub struct Simulation {
    pending: Vec<Fire>,
    trigger_state: std::collections::BTreeMap<Entity, TriggerState>,
}

impl Simulation {
    /// An empty simulation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedules `target` to be activated in `delay` seconds (`0` fires on
    /// the next [`Self::tick`]). Bounded: once [`MAX_PENDING_EVENTS`] are
    /// queued, further `fire` calls are dropped rather than growing
    /// unbounded, matching how a real map cannot queue infinite work either.
    pub fn fire(&mut self, target: impl Into<String>, activator: Option<Entity>, delay: f32) {
        if self.pending.len() >= MAX_PENDING_EVENTS {
            return;
        }
        self.pending.push(Fire {
            target: target.into(),
            activator,
            delay: delay.max(0.0),
        });
    }

    /// `use`s `entity` directly (as if the player pressed `E` on it),
    /// bypassing the name index: doors open, buttons press, triggers with a
    /// `target` fire it, `multi_manager`s fan out immediately, and
    /// `trigger_changelevel` emits a [`Event::LevelChange`] straight away.
    pub fn use_entity(
        &mut self,
        registry: &mut Registry,
        entity: Entity,
        activator: Option<Entity>,
        events: &mut Vec<Event>,
    ) {
        self.activate(registry, entity, activator, events);
    }

    /// Advances every scheduled event and state machine by `dt` seconds
    /// (call once per fixed timestep). Returns the events the caller must
    /// act on (currently only [`Event::LevelChange`]).
    pub fn tick(&mut self, registry: &mut Registry, dt: f32) -> Vec<Event> {
        let mut events = Vec::new();
        self.advance_queue(registry, dt, &mut events);
        Self::advance_doors(registry, dt);
        self.advance_buttons(registry, dt, &mut events);
        Self::advance_platforms(registry, dt);
        Self::advance_trains(registry, dt);
        for state in self.trigger_state.values_mut() {
            state.cooldown = (state.cooldown - dt).max(0.0);
        }
        events
    }

    fn advance_queue(&mut self, registry: &mut Registry, dt: f32, events: &mut Vec<Event>) {
        for scheduled in &mut self.pending {
            scheduled.delay -= dt;
        }
        let mut fired = 0;
        while fired < MAX_EVENTS_PER_TICK {
            let Some(index) = self.pending.iter().position(|f| f.delay <= 0.0) else {
                break;
            };
            let fire = self.pending.swap_remove(index);
            let targets: Vec<Entity> = registry.find(&fire.target).to_vec();
            for target in targets {
                self.activate(registry, target, fire.activator, events);
            }
            fired += 1;
        }
    }

    fn activate(
        &mut self,
        registry: &mut Registry,
        entity: Entity,
        activator: Option<Entity>,
        events: &mut Vec<Event>,
    ) {
        if let Ok(door) = registry.world.query_one_mut::<&mut Door>(entity) {
            if door.state == MoverState::Closed {
                door.state = MoverState::Opening;
                door.timer = door.delay + travel_time(door.travel_distance, door.speed);
            }
            return;
        }
        if let Ok(button) = registry.world.query_one_mut::<&mut Button>(entity) {
            if button.state == MoverState::Closed {
                button.state = MoverState::Opening;
                button.timer = button.delay;
            }
            return;
        }
        if let Ok(platform) = registry.world.query_one_mut::<&mut Platform>(entity) {
            if platform.state == MoverState::Closed {
                platform.state = MoverState::Opening;
                platform.timer = 0.0;
            }
            return;
        }
        if let Ok(train) = registry.world.query_one_mut::<&mut TrackTrainState>(entity) {
            train.toggle();
            return;
        }
        if let Some(mm) = registry
            .world
            .get::<&MultiManager>(entity)
            .ok()
            .map(|mm| mm.targets.clone())
        {
            for (target, delay) in mm {
                self.fire(target, activator, delay);
            }
            return;
        }
        if let Some(change) = registry
            .world
            .get::<&ChangeLevel>(entity)
            .ok()
            .map(|c| LevelChange {
                map: c.map.clone(),
                landmark: c.landmark.clone(),
            })
        {
            events.push(Event::LevelChange(change));
            return;
        }
        if let Some(message) = registry
            .world
            .get::<&Message>(entity)
            .ok()
            .map(|message| Message::clone(&message))
        {
            events.push(Event::Message(message));
            return;
        }
        if registry.world.get::<&Trigger>(entity).is_ok() {
            self.activate_trigger(registry, entity, activator);
        }
    }

    /// This simulation's own bookkeeping (scheduled events and per-trigger
    /// cooldowns), in a form a save file can hold.
    ///
    /// Entities are recorded by their `hecs` bit pattern, which is stable
    /// for a registry rebuilt from the same map in the same order; a
    /// snapshot restored onto a different map's registry simply does not
    /// match any entity and is ignored.
    #[must_use]
    pub fn snapshot(&self) -> SimulationState {
        SimulationState {
            pending: self
                .pending
                .iter()
                .map(|fire| PendingFire {
                    target: fire.target.clone(),
                    activator: fire.activator.map(|entity| entity.to_bits().get()),
                    delay: fire.delay,
                })
                .collect(),
            triggers: self
                .trigger_state
                .iter()
                .map(|(entity, state)| TriggerSnapshot {
                    entity: entity.to_bits().get(),
                    used: state.used,
                    cooldown: state.cooldown,
                })
                .collect(),
        }
    }

    /// Replaces this simulation's bookkeeping with `state`, dropping
    /// anything beyond the same bounds [`Self::fire`] enforces.
    pub fn restore(&mut self, state: &SimulationState) {
        self.pending = state
            .pending
            .iter()
            .take(MAX_PENDING_EVENTS)
            .map(|fire| Fire {
                target: fire.target.clone(),
                activator: fire.activator.and_then(Entity::from_bits),
                delay: fire.delay.max(0.0),
            })
            .collect();
        self.trigger_state = state
            .triggers
            .iter()
            .filter_map(|trigger| {
                Entity::from_bits(trigger.entity).map(|entity| {
                    (
                        entity,
                        TriggerState {
                            used: trigger.used,
                            cooldown: trigger.cooldown.max(0.0),
                        },
                    )
                })
            })
            .collect();
    }

    fn activate_trigger(
        &mut self,
        registry: &mut Registry,
        entity: Entity,
        activator: Option<Entity>,
    ) {
        let Ok(trigger) = registry.world.get::<&Trigger>(entity) else {
            return;
        };
        let trigger = *trigger;
        let state = self.trigger_state.entry(entity).or_default();
        if state.used || state.cooldown > 0.0 {
            return;
        }
        state.used = trigger.once;
        state.cooldown = trigger.wait.max(0.0);
        if let Ok(target) = registry.world.get::<&crate::registry::Target>(entity) {
            self.fire(target.0.clone(), activator, trigger.delay);
        }
    }

    fn advance_doors(registry: &mut Registry, dt: f32) {
        for door in registry.world.query_mut::<&mut Door>() {
            match door.state {
                MoverState::Closed => {}
                MoverState::Opening => {
                    door.timer -= dt;
                    if door.timer <= 0.0 {
                        door.state = MoverState::Open;
                        door.timer = door.wait;
                    }
                }
                MoverState::Open => {
                    if door.wait < 0.0 {
                        // Stays open.
                    } else if door.timer > 0.0 {
                        door.timer -= dt;
                    } else {
                        door.state = MoverState::Closing;
                        door.timer = travel_time(door.travel_distance, door.speed);
                    }
                }
                MoverState::Closing => {
                    if door.timer > 0.0 {
                        door.timer -= dt;
                    } else {
                        door.state = MoverState::Closed;
                        door.timer = 0.0;
                    }
                }
            }
        }
    }

    fn advance_buttons(&mut self, registry: &mut Registry, dt: f32, events: &mut Vec<Event>) {
        let mut to_fire = Vec::new();
        for (entity, button) in registry.world.query_mut::<(Entity, &mut Button)>() {
            match button.state {
                MoverState::Closed => {}
                MoverState::Opening => {
                    if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        to_fire.push(entity);
                        button.state = MoverState::Open;
                        button.timer = button.wait;
                    }
                }
                MoverState::Open => {
                    if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        button.state = MoverState::Closing;
                        button.timer = travel_time(4.0, button.speed.max(1.0));
                    }
                }
                MoverState::Closing => {
                    if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        button.state = MoverState::Closed;
                    }
                }
            }
        }
        for entity in to_fire {
            if let Ok(target) = registry.world.get::<&crate::registry::Target>(entity) {
                let target = target.0.clone();
                self.fire(target, Some(entity), 0.0);
            }
        }
        // A button firing its target may itself be a trigger_changelevel
        // reached through a chain that resolves within this same tick;
        // draining that is `advance_queue`'s job on the next tick, so no
        // event is produced directly here.
        let _ = events;
    }

    fn advance_platforms(registry: &mut Registry, dt: f32) {
        for platform in registry.world.query_mut::<&mut Platform>() {
            match platform.state {
                MoverState::Closed => {}
                MoverState::Opening => {
                    if platform.timer <= 0.0 {
                        platform.timer = travel_time(platform.travel_distance, platform.speed);
                    }
                    platform.timer -= dt;
                    if platform.timer <= 0.0 {
                        platform.state = MoverState::Open;
                        platform.timer = platform.wait;
                    }
                }
                MoverState::Open => {
                    if platform.timer > 0.0 {
                        platform.timer -= dt;
                    } else {
                        platform.state = MoverState::Closing;
                        platform.timer = travel_time(platform.travel_distance, platform.speed);
                    }
                }
                MoverState::Closing => {
                    if platform.timer > 0.0 {
                        platform.timer -= dt;
                    } else {
                        platform.state = MoverState::Closed;
                    }
                }
            }
        }
    }

    /// Advances every `func_train`/`func_tracktrain` along its resolved
    /// `path_track`/`path_corner` chain; see
    /// [`crate::track_train::TrackTrainState::advance`].
    fn advance_trains(registry: &mut Registry, dt: f32) {
        for train in registry.world.query_mut::<&mut TrackTrainState>() {
            train.advance(dt);
        }
    }
}

fn travel_time(distance: f32, speed: f32) -> f32 {
    if speed <= 0.0 {
        0.0
    } else {
        (distance / speed).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyvalues::{Limits, parse_entities};
    use crate::registry::Registry;
    use ohl_formats::bsp30::Entity as RawEntity;
    use std::collections::BTreeMap;

    fn raw(pairs: &[(&str, &str)]) -> RawEntity {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn tick_for(sim: &mut Simulation, registry: &mut Registry, seconds: f32, step: f32) {
        let mut elapsed = 0.0;
        while elapsed < seconds {
            sim.tick(registry, step);
            elapsed += step;
        }
    }

    #[test]
    fn door_opens_waits_and_closes() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("targetname", "door1"),
            ("speed", "100"),
            ("wait", "1"),
            ("angle", "0"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        // No brush model attached (point-entity style test), so travel
        // distance is 0 and the door opens instantly once its (zero)
        // delay elapses.
        let _ = &mut bounds;
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door_entity = registry.find("door1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, door_entity, None, &mut events);
        sim.tick(&mut registry, 0.05);
        {
            let door = registry.world.get::<&Door>(door_entity).unwrap();
            assert_eq!(door.state, MoverState::Open);
        }
        tick_for(&mut sim, &mut registry, 2.0, 0.05);
        let door = registry.world.get::<&Door>(door_entity).unwrap();
        assert_eq!(door.state, MoverState::Closed);
    }

    #[test]
    fn button_fires_door_target_after_delay() {
        let entities = vec![
            raw(&[
                ("classname", "func_button"),
                ("targetname", "btn1"),
                ("target", "door1"),
                ("wait", "1"),
                ("delay", "0"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("speed", "100"),
                ("wait", "-1"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let button = registry.find("btn1")[0];
        let door = registry.find("door1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, button, None, &mut events);
        // Button's press animation (default timer 0 -> fires on next tick).
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        let door_component = registry.world.get::<&Door>(door).unwrap();
        assert_eq!(door_component.state, MoverState::Open);
    }

    #[test]
    fn multi_manager_fans_out_in_order() {
        let entities = vec![
            raw(&[
                ("classname", "multi_manager"),
                ("targetname", "mm1"),
                ("door_a", "0.0"),
                ("door_b", "0.5"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door_a"),
                ("wait", "-1"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door_b"),
                ("wait", "-1"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let mm = registry.find("mm1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, mm, None, &mut events);
        sim.tick(&mut registry, 0.05);
        let a = registry.find("door_a")[0];
        let b = registry.find("door_b")[0];
        assert_eq!(
            registry.world.get::<&Door>(a).unwrap().state,
            MoverState::Open
        );
        assert_eq!(
            registry.world.get::<&Door>(b).unwrap().state,
            MoverState::Closed
        );
        tick_for(&mut sim, &mut registry, 0.6, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(b).unwrap().state,
            MoverState::Open
        );
    }

    #[test]
    fn trigger_once_fires_only_once() {
        let entities = vec![
            raw(&[
                ("classname", "trigger_once"),
                ("targetname", "trig1"),
                ("target", "door1"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("wait", "0.2"),
                ("speed", "1000"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let trigger = registry.find("trig1")[0];
        let door = registry.find("door1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, trigger, None, &mut events);
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );
        // Second activation must be ignored (trigger_once already used).
        sim.use_entity(&mut registry, trigger, None, &mut events);
        sim.tick(&mut registry, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );
    }

    #[test]
    fn trigger_multiple_respects_wait_cooldown() {
        let entities = vec![
            raw(&[
                ("classname", "trigger_multiple"),
                ("targetname", "trig1"),
                ("target", "door1"),
                ("wait", "1.0"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("wait", "0.1"),
                ("speed", "1000"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let trigger = registry.find("trig1")[0];
        let door = registry.find("door1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, trigger, None, &mut events);
        // Immediately try again: cooldown should block this second use.
        sim.use_entity(&mut registry, trigger, None, &mut events);
        tick_for(&mut sim, &mut registry, 0.3, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );
    }

    #[test]
    fn changelevel_emits_event() {
        let entities = vec![raw(&[
            ("classname", "trigger_changelevel"),
            ("targetname", "cl1"),
            ("map", "next_map"),
            ("landmark", "lm1"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry.find("cl1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);
        assert_eq!(
            events,
            vec![Event::LevelChange(LevelChange {
                map: "next_map".to_string(),
                landmark: "lm1".to_string(),
            })]
        );
    }

    #[test]
    fn find_usable_within_prefers_brush_center_and_respects_radius() {
        let entities = vec![
            raw(&[
                ("classname", "func_door"),
                ("targetname", "near_door"),
                ("model", "*1"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "far_door"),
                ("model", "*2"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]));
        bounds.insert(2u32, ([1000.0, 0.0, 0.0], [1010.0, 10.0, 10.0]));
        let registry = Registry::build(&defs, &bounds, &Limits::default());
        let found = find_usable_within(&registry, Vec3::new(5.0, 5.0, 5.0), 64.0);
        assert_eq!(found, Some(registry.find("near_door")[0]));
        assert!(find_usable_within(&registry, Vec3::new(500.0, 5.0, 5.0), 10.0).is_none());
    }
}
