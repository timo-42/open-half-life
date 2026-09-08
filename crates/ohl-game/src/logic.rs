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
    AutoTrigger, Breakable, BrushBounds, Button, ChangeLevel, Door, DoorPassable, DoorUseOnly,
    Message, MomentaryDoor, MomentaryRotButton, MoverState, MultiManager, Pendulum, Platform,
    Registry, RotButton, RotatingDoorSwing, Rotator, Target, TargetName, Transform, Trigger,
    TriggerHurt,
};
use crate::track_train::TrackTrainState;

/// Finds the closest `func_door`, `func_button`, or `use`-activated
/// `func_rot_button` within `radius` units of `position`, measured against
/// a brush entity's own currently-placed bounding-box centre
/// ([`crate::pose::brush_center`]) rather than its `Transform::origin`, and
/// falling back to `Transform::origin` only for an entity that has no
/// brush submodel to measure at all (a point entity, or one whose submodel
/// bounds were unavailable at load).
///
/// Going through [`crate::pose::brush_center`] is what makes this agree
/// with where the entity is drawn and collided: it is the same placed pose
/// — compiled geometry, rotated about its pivot, translated by the `origin`
/// keyvalue, plus however far its state machine has moved it — that
/// `ohl-engine` builds its render transform and collision brush from. A
/// brush entity built around an "origin brush" (a rotating door, a train)
/// compiles its geometry relative to that brush rather than in absolute
/// world space, so measuring against the raw compiled bounds instead would
/// search near the map's `(0, 0, 0)` and never find it.
///
/// A `func_rot_button` with the documented "Touch activates" spawnflag
/// ([`crate::registry::SPAWNFLAG_ROT_BUTTON_TOUCH`]) is excluded: TWHL wiki
/// `func_rot_button` (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map
/// logic") documents that flag as making the button respond only to the
/// player's hull touching its brush (see [`Simulation::touch_rot_buttons`]),
/// not to a proximity `use` press. A `momentary_rot_button` is never
/// returned here at all — it is driven every tick `use` is *held*
/// ([`Simulation::drive_momentary_rot_button`]), not by a single press.
///
/// Intended for a "use the nearest usable thing" input binding.
#[must_use]
pub fn find_usable_within(registry: &Registry, position: Vec3, radius: f32) -> Option<Entity> {
    let mut best: Option<(Entity, f32)> = None;
    let mut consider = |entity: Entity, transform: &Transform| {
        let center = crate::pose::brush_center(registry, entity).unwrap_or(transform.origin);
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
    for (entity, transform, button) in
        &mut registry.world.query::<(Entity, &Transform, &RotButton)>()
    {
        if !button.touch {
            consider(entity, transform);
        }
    }
    best.map(|(entity, _)| entity)
}

/// Finds the closest `momentary_rot_button` within `radius` units of
/// `position` whose documented "Door Hack" spawnflag
/// ([`crate::registry::SPAWNFLAG_MOMENTARY_DOOR_HACK`]) is *not* set — see
/// [`crate::registry::MomentaryRotButton::door_hack`]'s own doc comment for
/// why such a button is excluded from proximity `use`. Intended to be
/// called every tick `use` is held (not only on the press edge, unlike
/// [`find_usable_within`]) so [`Simulation::drive_momentary_rot_button`] has
/// something to drive.
#[must_use]
pub fn find_momentary_rot_button_within(
    registry: &Registry,
    position: Vec3,
    radius: f32,
) -> Option<Entity> {
    let mut best: Option<(Entity, f32)> = None;
    for (entity, transform, button) in &mut registry
        .world
        .query::<(Entity, &Transform, &MomentaryRotButton)>()
    {
        if button.door_hack {
            continue;
        }
        let center = crate::pose::brush_center(registry, entity).unwrap_or(transform.origin);
        let distance = center.distance(position);
        if distance <= radius && best.is_none_or(|(_, best_distance)| distance < best_distance) {
            best = Some((entity, distance));
        }
    }
    best.map(|(entity, _)| entity)
}

/// The documented cap on how many `Fire` events one tick will drain from
/// the queue, so a pathological chain of zero-delay `multi_manager`s cannot
/// spin forever.
const MAX_EVENTS_PER_TICK: usize = 4096;

/// The largest number of scheduled events kept at once.
const MAX_PENDING_EVENTS: usize = 4096;

/// How far a closed door's own placed [`BrushBounds`] are inflated in every
/// direction before [`Simulation::touch_doors`] tests them against the
/// player's hull box. A player who has walked forward into a closed door's
/// solid brush is stopped by ordinary collision *before* the two boxes
/// exactly touch — `ohl_physics::hull::DIST_EPSILON` (1/32 of a unit)
/// backs the resolved position off the contact plane by design — so an
/// un-inflated overlap test would never see a rising edge for a player
/// standing flush against a door they just walked into. No public source
/// states this slop's exact size (or that a real client-side "touch" check
/// uses one at all, as opposed to whatever server-side proximity a
/// `MOVETYPE_TOUCH` walk in the original engine used); this project's own
/// bounded choice, comfortably larger than that epsilon while still small
/// next to a standing hull's own footprint, recorded here as project
/// behaviour (`docs/FORMAT_SOURCES.md`, item 30).
///
/// **Measured thin-wall boundary** (a synthetic probe, not a payload
/// scan): a door placed directly behind a plain wall, with the player
/// stopped flush against the wall's own near face (backed off by
/// `DIST_EPSILON`, the same as against a door directly), opens through
/// the wall at `wall_thickness = 3` but not at `wall_thickness = 4` or
/// thicker. At exactly this margin's own value the miss is decided by
/// `DIST_EPSILON` alone (`aabb_overlaps` counts an exact face touch as
/// overlapping, so a wall exactly `DOOR_TOUCH_MARGIN` units thick, with no
/// epsilon gap at all, would still false-positive) — defensible only
/// because real map geometry does not build door frames into walls
/// thinner than this margin. Raising `DOOR_TOUCH_MARGIN` narrows how thin
/// a wall has to be to trigger this false positive; lowering it risks
/// missing a legitimate flush-contact touch again, the failure this
/// margin exists to prevent in the first place.
const DOOR_TOUCH_MARGIN: f32 = 4.0;

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
    /// Whether a `trigger_changelevel`'s touch volume was overlapping the
    /// player on the last check, per [`Simulation::touch_changelevel_triggers`];
    /// `None` means "never checked yet" so the first observation can be
    /// told apart from a genuine rising edge.
    changelevel_touching: Option<bool>,
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
    /// A `trigger_changelevel`'s last-observed touch state, mirroring
    /// [`TriggerState::changelevel_touching`].
    pub changelevel_touching: Option<bool>,
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

/// GoldSrc's documented special-`targetname` convention: any entity (of
/// any classname) whose own `targetname` equals this string is activated
/// directly — the same way a player's `use` key or another entity's fire
/// chain would activate it by name — the first time a player spawns into
/// the map, rather than being looked up as *another* entity's `target`.
/// See `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic",
/// "game_playerspawn") for the public source and its caveats.
const GAME_PLAYER_SPAWN_TARGETNAME: &str = "game_playerspawn";

/// The map logic simulation: an event queue plus per-tick state-machine
/// advancement.
#[derive(Debug, Default)]
pub struct Simulation {
    pending: Vec<Fire>,
    trigger_state: std::collections::BTreeMap<Entity, TriggerState>,
    /// Per-`func_rot_button` last-observed touch state, for
    /// [`Self::touch_rot_buttons`]'s edge trigger — the same shape as
    /// [`TriggerState::changelevel_touching`], kept in its own map since a
    /// `RotButton` is not a [`Trigger`].
    rot_button_touch: std::collections::BTreeMap<Entity, bool>,
    /// Per-`func_door`/`func_door_rotating` last-observed touch state, for
    /// [`Self::touch_doors`]'s own edge trigger — the same shape as
    /// [`Self::rot_button_touch`], kept in its own map for the same
    /// reason: a [`Door`] is not a [`Trigger`] either. Not part of
    /// [`SimulationState`]: like [`Self::rot_button_touch`], a stale
    /// `true` surviving a save/load only ever suppresses one spurious
    /// re-open on the load's first tick if the player happens to still be
    /// standing in the same door's touch volume, never opens one that
    /// should stay shut.
    door_touch: std::collections::BTreeMap<Entity, bool>,
    /// Whether [`Self::fire_player_spawn`] has already run. Not carried in
    /// [`SimulationState`]: matches this project's existing convention for
    /// `trigger_auto`'s own one-shot `fired` flag (an ECS component field,
    /// also not persisted across a save/restore today), so a loaded save
    /// does not re-fire either one.
    player_spawn_fired: bool,
    /// Where the player is standing, refreshed by the host every tick
    /// through [`Self::set_activator_origin`] and read only by
    /// [`Self::activate`] to decide which way a `func_door_rotating`
    /// swings ("away from the player"; see
    /// [`crate::registry::RotatingDoorSwing`]). The player is not a `hecs`
    /// entity in this project, so a `use` press or touch trigger has no
    /// activator [`Entity`] to read a [`Transform`] off; this scratch
    /// field stands in for one. Not persisted in [`SimulationState`]: it
    /// is overwritten before it is read on every tick a host drives, and a
    /// host that never sets it simply leaves every rotating door swinging
    /// its spawnflag-chosen way, exactly as before this rule existed.
    activator_origin: Option<Vec3>,
    /// Remaining hit points for a [`Button`]/[`RotButton`] whose `health`
    /// keyvalue is non-zero, keyed by entity, populated lazily the first
    /// time [`Self::damage_button`] sees that entity and reset to the
    /// configured `health` again once it reaches zero and presses. **Not
    /// carried in [`SimulationState`], and no new field was added to
    /// [`crate::registry::Button`]/[`crate::registry::RotButton`]
    /// themselves**: both structs round-trip whole through
    /// `ohl_engine::transition::EntitySnapshot` (save section 18, a
    /// *required* section — see `docs/FORMAT_SOURCES.md` item 28's own
    /// "floor" finding), so widening either would reject every save
    /// written before this field existed the same way item 28 already
    /// documents for section 18's `rotator` addition. This project's
    /// documented behaviour instead: a damageable button's accumulated
    /// damage resets on save/load (every button starts back at its full
    /// configured `health`), the same "documented gap" shape
    /// [`Self::rot_button_touch`] already accepts for its own edge state.
    button_health: std::collections::BTreeMap<Entity, f32>,
}

impl Simulation {
    /// An empty simulation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records where the activating player currently stands, for the one
    /// decision that needs it: which way a `func_door_rotating` opens (see
    /// [`Self::activator_origin`] and
    /// [`crate::registry::RotatingDoorSwing`]). A non-finite origin is
    /// stored as `None` rather than propagated into the dot product below.
    pub fn set_activator_origin(&mut self, origin: Option<Vec3>) {
        self.activator_origin = origin.filter(|origin| origin.is_finite());
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

    /// Fires every `trigger_once`/`trigger_multiple` whose brush volume
    /// overlaps the box `[player_mins, player_maxs]` (world space), the way
    /// a real touch trigger tests brush-against-brush rather than a single
    /// point — see `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map
    /// logic", touch triggers). `trigger_hurt` shares the same [`Trigger`]
    /// component but is excluded here: it is polled on its own radius-based
    /// path (`ohl-engine`'s player-systems phase) and firing its `target`
    /// through this path too is unimplemented/out of scope. `trigger_changelevel`
    /// also shares the same component (when it is not "USE Only"; see
    /// [`crate::registry::SPAWNFLAG_CHANGELEVEL_USE_ONLY`]) but is excluded
    /// here too: it needs its own edge-triggered one-shot bookkeeping
    /// rather than the plain-`Trigger` `used`/`cooldown` path, and is
    /// handled by [`Self::touch_changelevel_triggers`] below.
    ///
    /// Entities are visited in ascending id order so which trigger fires
    /// first (when several volumes overlap on the same step) is
    /// deterministic.
    pub fn touch_triggers(
        &mut self,
        registry: &mut Registry,
        player_mins: Vec3,
        player_maxs: Vec3,
        activator: Option<Entity>,
        events: &mut Vec<Event>,
    ) {
        let mut touched: Vec<Entity> = registry
            .world
            .query::<(Entity, &Trigger, &BrushBounds)>()
            .without::<&TriggerHurt>()
            .without::<&ChangeLevel>()
            .iter()
            .filter(|(_, _, bounds)| {
                aabb_overlaps(player_mins, player_maxs, bounds.mins, bounds.maxs)
            })
            .map(|(entity, _, _)| entity)
            .collect();
        touched.sort_unstable_by_key(|entity| entity.id());
        for entity in touched {
            self.activate_trigger(registry, entity, activator);
        }
        self.touch_changelevel_triggers(registry, player_mins, player_maxs, events);
        self.touch_rot_buttons(registry, player_mins, player_maxs);
        self.touch_breakables(registry, player_mins, player_maxs);
    }

    /// Fires a `trigger_changelevel` (not "USE Only"; see
    /// [`Self::touch_triggers`]'s doc comment) the moment the player's AABB
    /// transitions from not overlapping to overlapping its volume — an
    /// edge-triggered touch, unlike the plain [`Trigger`] path above, so a
    /// player standing in the volume across many fixed steps (e.g. while
    /// the destination map streams in) only ever produces one
    /// [`Event::LevelChange`] for that entrance.
    ///
    /// TODO(black-box): no public source states whether the destination
    /// map's own return `trigger_changelevel` can fire again on the very
    /// frame the player arrives already standing inside it. This project
    /// takes the conservative reading: the *first* observation of any
    /// `trigger_changelevel` volume never fires by itself, even when
    /// already overlapping, so a freshly loaded map requires the player to
    /// leave and re-enter the volume before it can fire — see
    /// `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic").
    fn touch_changelevel_triggers(
        &mut self,
        registry: &mut Registry,
        player_mins: Vec3,
        player_maxs: Vec3,
        events: &mut Vec<Event>,
    ) {
        let mut candidates: Vec<(Entity, bool)> = registry
            .world
            .query::<(Entity, &Trigger, &ChangeLevel, &BrushBounds)>()
            .iter()
            .map(|(entity, _, _, bounds)| {
                (
                    entity,
                    aabb_overlaps(player_mins, player_maxs, bounds.mins, bounds.maxs),
                )
            })
            .collect();
        candidates.sort_unstable_by_key(|(entity, _)| entity.id());
        for (entity, overlapping) in candidates {
            let state = self.trigger_state.entry(entity).or_default();
            let previously_touching = state.changelevel_touching;
            state.changelevel_touching = Some(overlapping);
            let rising_edge = overlapping && previously_touching == Some(false);
            if !rising_edge {
                continue;
            }
            if let Ok(change) = registry.world.get::<&ChangeLevel>(entity) {
                events.push(Event::LevelChange(LevelChange {
                    map: change.map.clone(),
                    landmark: change.landmark.clone(),
                }));
            }
        }
    }

    /// Advances every scheduled event and state machine by `dt` seconds
    /// (call once per fixed timestep). Returns the events the caller must
    /// act on (currently only [`Event::LevelChange`]).
    pub fn tick(&mut self, registry: &mut Registry, dt: f32) -> Vec<Event> {
        let mut events = Vec::new();
        self.fire_player_spawn(registry, &mut events);
        self.fire_auto_triggers(registry);
        self.advance_queue(registry, dt, &mut events);
        Self::advance_doors(registry, dt);
        self.advance_buttons(registry, dt, &mut events);
        self.advance_rot_buttons(registry, dt);
        Self::advance_platforms(registry, dt);
        Self::advance_rotators(registry, dt);
        Self::advance_pendulums(registry, dt);
        Self::advance_trains(registry, dt);
        self.advance_cameras(registry, dt);
        for state in self.trigger_state.values_mut() {
            state.cooldown = (state.cooldown - dt).max(0.0);
        }
        events
    }

    /// Activates every entity named [`GAME_PLAYER_SPAWN_TARGETNAME`], once,
    /// the first time this simulation ticks (this project has no separate
    /// "player enters the world" moment from "the level starts ticking":
    /// the player's own state is constructed before the first
    /// [`Self::tick`] call, so that first tick *is* the spawn). Unlike
    /// [`Self::fire_auto_triggers`] (which fires the *target* a
    /// `trigger_auto` names), this activates the matching entity itself
    /// directly — `game_playerspawn` is documented as a special
    /// `targetname` the engine recognizes on whatever entity carries it
    /// (any classname, commonly a `multi_manager` or `trigger_relay`), not
    /// a classname of its own. Before this method existed, a map whose
    /// intro sequence relied on this convention rather than on
    /// `trigger_auto` never activated at all: nothing in this crate ever
    /// looked up that name.
    ///
    /// TODO(black-box): the exact fire order relative to
    /// [`Self::fire_auto_triggers`] within the same tick is not confirmed
    /// by a fetchable primary source (see the citation in
    /// `docs/FORMAT_SOURCES.md`); both still fire on the same tick either
    /// way, so this only matters for a map that names the same entity from
    /// both a `trigger_auto` and a `game_playerspawn`-named relay, which is
    /// not the shape this fix targets.
    fn fire_player_spawn(&mut self, registry: &mut Registry, events: &mut Vec<Event>) {
        if self.player_spawn_fired {
            return;
        }
        self.player_spawn_fired = true;
        let mut targets: Vec<Entity> = registry.find(GAME_PLAYER_SPAWN_TARGETNAME).to_vec();
        targets.sort_unstable_by_key(|entity| entity.id());
        for entity in targets {
            self.activate(registry, entity, None, events);
        }
    }

    /// Fires every `trigger_auto` that has not fired yet, in ascending
    /// entity order, and removes the ones whose `Remove On fire` spawnflag
    /// is set. Runs before the queue, so a zero-delay auto trigger is
    /// dispatched by the same tick that armed it.
    fn fire_auto_triggers(&mut self, registry: &mut Registry) {
        let mut ready: Vec<(Entity, String, f32, bool)> = registry
            .world
            .query::<(Entity, &AutoTrigger, &Target)>()
            .iter()
            .filter(|(_, auto, _)| !auto.fired)
            .map(|(entity, auto, target)| {
                (
                    entity,
                    target.0.clone(),
                    auto.delay.max(0.0),
                    auto.remove_on_fire,
                )
            })
            .collect();
        if ready.is_empty() {
            return;
        }
        ready.sort_unstable_by_key(|(entity, _, _, _)| entity.id());
        for (entity, target, delay, remove) in ready {
            if let Ok(auto) = registry.world.query_one_mut::<&mut AutoTrigger>(entity) {
                auto.fired = true;
            }
            self.fire(target, Some(entity), delay);
            if remove {
                registry.world.despawn(entity).ok();
            }
        }
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

    #[allow(clippy::too_many_lines)]
    fn activate(
        &mut self,
        registry: &mut Registry,
        entity: Entity,
        activator: Option<Entity>,
        events: &mut Vec<Event>,
    ) {
        // Decided before the `&mut Door` borrow below, since it reads two
        // other components off the same registry: which way a
        // `func_door_rotating` should swing for *this* activator. `None`
        // for every other kind of door, and for the cases
        // `RotatingDoorSwing::opening_axis` itself declines to decide.
        let swing_axis = self.rotating_door_open_axis(registry, entity, activator);
        if let Ok(door) = registry.world.query_one_mut::<&mut Door>(entity) {
            if door.state == MoverState::Closed {
                // Only ever applied on the closed -> opening edge, where
                // the door's own rendered/collided angle is zero: flipping
                // the axis at any other point in the cycle would teleport
                // a part-open leaf to the mirrored pose. Writing it into
                // `Door::rotation_axis` is also what makes the choice
                // survive a save/load and a level transition — that field
                // already round-trips through the entity snapshot every
                // save section and `crate::transition` carry — with no new
                // state to persist.
                if let Some(axis) = swing_axis {
                    door.rotation_axis = Some(axis);
                }
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
        if let Ok(button) = registry.world.query_one_mut::<&mut RotButton>(entity) {
            match button.state {
                MoverState::Closed => {
                    button.state = MoverState::Opening;
                    button.timer = button.delay + travel_time(button.distance, button.speed);
                }
                // The documented "Toggle" spawnflag: using an already-open
                // button rotates it back, firing `target` again — see
                // `advance_rot_buttons`'s own `Closing -> Closed` arm for
                // where that second fire happens.
                MoverState::Open if button.toggle => {
                    button.state = MoverState::Closing;
                    button.timer = travel_time(button.distance, button.speed);
                }
                _ => {}
            }
            return;
        }
        // A `func_breakable`/`func_pushable` breaks when triggered, whether
        // or not it has any `health` left: "When the `func_breakable` is
        // broken **or triggered**, it will activate this entity"
        // (`docs/FORMAT_SOURCES.md`, item 32). This arm sits before the
        // generic `Trigger` fallthrough at the end for the same reason
        // every other component arm does — a breakable brush is not a
        // `trigger_*` volume — and returns whether or not the brush was
        // already broken, so a second trigger cannot fire `target` twice.
        if registry.world.get::<&Breakable>(entity).is_ok() {
            self.break_entity(registry, entity);
            return;
        }
        if let Ok(pendulum) = registry.world.query_one_mut::<&mut Pendulum>(entity) {
            pendulum.swinging = !pendulum.swinging;
            if pendulum.swinging {
                pendulum.returning = false;
            } else if pendulum.auto_return {
                pendulum.returning = true;
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
        if let Ok(rotator) = registry.world.query_one_mut::<&mut Rotator>(entity) {
            rotator.spinning = !rotator.spinning;
            return;
        }
        if let Ok((state, camera)) = registry.world.query_one_mut::<(
            &mut crate::camera::TriggerCameraState,
            &crate::registry::TriggerCamera,
        )>(entity)
        {
            state.trigger(camera);
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
        if let Ok(activation) = registry
            .world
            .query_one_mut::<&mut crate::scripts::ScriptActivation>(entity)
        {
            // `scripted_sequence`/`aiscripted_sequence`/`scripted_sentence`
            // ride the same activation path everything else does; the
            // engine's AI phase drains the counter.
            activation.activate();
            return;
        }
        if let Ok(activation) = registry
            .world
            .query_one_mut::<&mut crate::registry::MakerActivation>(entity)
        {
            // `monstermaker` rides the same activation path too; the
            // engine's AI phase drains the counter and calls
            // `ohl_ai::Spawner::trigger`, since the `Spawner` itself lives
            // in an `ohl-engine` component this crate cannot see.
            activation.activate();
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
                    changelevel_touching: state.changelevel_touching,
                })
                .collect(),
        }
    }

    /// Per-`func_rot_button` touch-edge state
    /// ([`Self::rot_button_touch`]), as `(entity bit pattern, touching)`
    /// pairs — kept out of [`SimulationState`]/[`Self::snapshot`] on
    /// purpose: `SimulationState` backs `SECTION_SIMULATION`
    /// (`ohl_engine`'s save tag 19), a *required* section written for
    /// every save, and — per that crate's own `save.rs` module doc — every
    /// section is `postcard`-encoded, which is not self-describing, so
    /// adding a field to a required section's type makes every save
    /// written before that field existed fail to decode. A caller that
    /// wants this state to survive a save/load restores it separately,
    /// through [`Self::restore_rot_button_touch`], into whatever optional
    /// section it chooses to carry it in.
    #[must_use]
    pub fn rot_button_touch_snapshot(&self) -> Vec<(u64, bool)> {
        self.rot_button_touch
            .iter()
            .map(|(entity, touching)| (entity.to_bits().get(), *touching))
            .collect()
    }

    /// Replaces [`Self::rot_button_touch`] with `entries`, the mirror of
    /// [`Self::rot_button_touch_snapshot`]. See that method's own doc
    /// comment for why this is separate from [`Self::restore`].
    pub fn restore_rot_button_touch(&mut self, entries: &[(u64, bool)]) {
        self.rot_button_touch = entries
            .iter()
            .filter_map(|(bits, touching)| {
                Entity::from_bits(*bits).map(|entity| (entity, *touching))
            })
            .collect();
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
                            changelevel_touching: trigger.changelevel_touching,
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

    /// The signed rotation axis `entity` — when it is a
    /// `func_door_rotating` with a [`RotatingDoorSwing`] — should open
    /// about so that its leaf swings away from `activator` (an entity with
    /// a [`Transform`], else the host-supplied
    /// [`Self::activator_origin`]). `None` whenever the direction is not
    /// this rule's to decide; see
    /// [`RotatingDoorSwing::opening_axis`].
    fn rotating_door_open_axis(
        &self,
        registry: &Registry,
        entity: Entity,
        activator: Option<Entity>,
    ) -> Option<Vec3> {
        let swing = *registry.world.get::<&RotatingDoorSwing>(entity).ok()?;
        let pivot = registry.world.get::<&Transform>(entity).ok()?.origin;
        let activator_origin = activator
            .and_then(|activator| {
                registry
                    .world
                    .get::<&Transform>(activator)
                    .ok()
                    .map(|transform| transform.origin)
            })
            .or(self.activator_origin);
        swing.opening_axis(pivot, activator_origin)
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

    /// Advances every `func_rot_button`'s press/return timer, mirroring
    /// [`Self::advance_buttons`]'s shape but firing `target` on *both*
    /// directions when the documented "Toggle" spawnflag
    /// ([`crate::registry::SPAWNFLAG_ROT_BUTTON_TOGGLE`]) is set (each
    /// press-or-release "retriggering its target", per TWHL wiki
    /// `func_rot_button`), and only auto-returning (`Open -> Closing`) for
    /// a non-toggle button, honouring `wait < 0` ("stays set") exactly like
    /// [`Door`]'s own `Open` arm.
    fn advance_rot_buttons(&mut self, registry: &mut Registry, dt: f32) {
        let mut to_fire = Vec::new();
        for (entity, button) in registry.world.query_mut::<(Entity, &mut RotButton)>() {
            match button.state {
                MoverState::Closed => {}
                MoverState::Opening => {
                    // `timer` here counts down the shared `delay +
                    // travel_time` window `Simulation::activate` armed it
                    // with, exactly like `Door`'s own `Opening` arm — so
                    // `ohl-engine`'s `render::mover_fraction` (which reads
                    // the same `speed`/`distance`/`state`/`timer` shape)
                    // animates the press instead of snapping instantly.
                    if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        to_fire.push(entity);
                        button.state = MoverState::Open;
                        button.timer = button.wait;
                    }
                }
                MoverState::Open => {
                    if button.toggle || button.wait < 0.0 {
                        // Toggle buttons wait indefinitely for a re-press
                        // (`Simulation::activate`'s own `Open`-with-toggle
                        // arm); a non-toggle button with `wait < 0` "stays
                        // set" the same way a `Door` does.
                    } else if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        button.state = MoverState::Closing;
                        button.timer = travel_time(button.distance, button.speed);
                    }
                }
                MoverState::Closing => {
                    if button.timer > 0.0 {
                        button.timer -= dt;
                    } else {
                        if button.toggle {
                            to_fire.push(entity);
                        }
                        button.state = MoverState::Closed;
                    }
                }
            }
        }
        for entity in to_fire {
            if let Ok(target) = registry.world.get::<&Target>(entity) {
                let target = target.0.clone();
                self.fire(target, Some(entity), 0.0);
            }
        }
    }

    /// Applies `amount` of damage to `entity`'s [`Button`]/[`RotButton`]
    /// `health`, when it is non-zero — the documented "(or by being shot, if
    /// Health is > 0)" alternative to `use`/touch activation TWHL wiki
    /// `func_rot_button` states for both entities (`docs/FORMAT_SOURCES.md`
    /// item 27, which previously left this unimplemented for both
    /// `func_rot_button` and `func_button`; `func_button`'s own `health` doc
    /// comment already cited the same wording). An entity with `health ==
    /// 0` (the documented "responds only to `use`" default) or with neither
    /// component returns `false` immediately, doing nothing.
    ///
    /// Non-finite or non-positive `amount` is ignored (mirrors
    /// [`ohl_combat::apply_damage`]'s own guard, kept here too since this
    /// path does not go through that function). Once accumulated damage
    /// reaches the configured `health`, the button presses through
    /// [`Self::activate`] — the exact same press path a proximity `use`
    /// takes ([`Self::use_entity`]), so a `func_button`'s `delay`/`wait` and
    /// a `func_rot_button`'s `distance`/`speed`/`toggle` shape the press
    /// identically regardless of what triggered it — and the counter resets
    /// to the full configured `health`, so the button can be shot down
    /// again after it returns (or, for a `Toggle` `func_rot_button`, after a
    /// second press closes it again). Events `Self::activate` would
    /// otherwise report (a `Message`/`LevelChange` reached through this
    /// button's `target`) are discarded here, the same accepted limitation
    /// [`Self::touch_rot_buttons`] already has for its own `Self::activate`
    /// call. Returns whether this call was the one that pressed the button.
    pub fn damage_button(&mut self, registry: &mut Registry, entity: Entity, amount: f32) -> bool {
        if !amount.is_finite() || amount <= 0.0 {
            return false;
        }
        let configured = registry
            .world
            .get::<&Button>(entity)
            .ok()
            .map(|button| button.health)
            .or_else(|| {
                registry
                    .world
                    .get::<&RotButton>(entity)
                    .ok()
                    .map(|button| button.health)
            });
        let Some(configured) = configured.filter(|health| *health > 0.0) else {
            return false;
        };
        let remaining = *self.button_health.get(&entity).unwrap_or(&configured) - amount;
        if remaining <= 0.0 {
            self.button_health.insert(entity, configured);
            self.activate(registry, entity, None, &mut Vec::new());
            true
        } else {
            self.button_health.insert(entity, remaining);
            false
        }
    }

    /// Fires every `func_rot_button` whose documented "Touch activates"
    /// spawnflag ([`crate::registry::SPAWNFLAG_ROT_BUTTON_TOUCH`]) is set
    /// and whose brush volume overlaps `[player_mins, player_maxs]`,
    /// mirroring [`Self::touch_triggers`]'s brush-vs-brush overlap test
    /// (not a single point). Edge-triggered like
    /// [`Self::touch_changelevel_triggers`], so a player standing on the
    /// button across many fixed steps only presses it once per approach —
    /// otherwise a toggle button standing in reach of its own touch volume
    /// would immediately re-trigger itself back closed the moment it
    /// reopened.
    pub fn touch_rot_buttons(
        &mut self,
        registry: &mut Registry,
        player_mins: Vec3,
        player_maxs: Vec3,
    ) {
        let mut candidates: Vec<(Entity, bool)> = registry
            .world
            .query::<(Entity, &RotButton, &BrushBounds)>()
            .iter()
            .filter(|(_, button, _)| button.touch)
            .map(|(entity, _, bounds)| {
                (
                    entity,
                    aabb_overlaps(player_mins, player_maxs, bounds.mins, bounds.maxs),
                )
            })
            .collect();
        candidates.sort_unstable_by_key(|(entity, _)| entity.id());
        for (entity, overlapping) in candidates {
            let state = self.rot_button_touch.entry(entity).or_default();
            let rising_edge = overlapping && !*state;
            *state = overlapping;
            if rising_edge {
                self.activate(registry, entity, None, &mut Vec::new());
            }
        }
    }

    /// Opens every eligible `func_door`/`func_door_rotating` whose brush
    /// volume, inflated by [`DOOR_TOUCH_MARGIN`], overlaps
    /// `[player_mins, player_maxs]` — mirroring
    /// [`Self::touch_triggers`]/[`Self::touch_rot_buttons`]'s own
    /// brush-vs-brush overlap test, not a single point. Edge-triggered the
    /// same way [`Self::touch_rot_buttons`] is (PR #111's pattern), so a
    /// player standing in a door's own reach across many fixed steps only
    /// opens it once per approach.
    ///
    /// The Sven Co-op wiki's `Func_door` page (`docs/FORMAT_SOURCES.md`,
    /// item 30) states the touch rule as one sentence: "Func_doors are
    /// triggered on touch, unless they have a name, in which's case they
    /// require to be triggered manually." A door is excluded from this
    /// method — but still reachable by a `use` press
    /// ([`crate::logic::find_usable_within`]/`Self::use_entity`) or by
    /// another entity's fire chain (`Self::activate_trigger`/fan-out) —
    /// when either half of that sentence applies: it carries a
    /// [`TargetName`] (the "unless they have a name" clause; a named door
    /// is presumed gated behind whatever fires that name, which touch must
    /// not bypass), or it has the "Use Only" spawnflag ([`DoorUseOnly`]).
    /// A door with the "Passable" spawnflag ([`DoorPassable`]) is also
    /// excluded: the same page documents it as entirely non-solid *and*
    /// "cannot be triggered on-touch anymore then" — without this
    /// exclusion, a passable door's own bounds would still register a
    /// touch overlap (nothing stops the player's hull from passing through
    /// them) even though the door has no collision to walk flush against
    /// in the first place, unlike every other door this method opens.
    ///
    /// `activate` is called with `activator = None`, exactly like
    /// [`Self::touch_rot_buttons`] above: [`Self::rotating_door_open_axis`]
    /// falls back to [`Self::activator_origin`] (refreshed every tick from
    /// the player's own position by the host) when no activator [`Entity`]
    /// is given, so a `func_door_rotating` opened this way still swings
    /// *away* from the player, the same rule item 26/PR #110 gives a
    /// `use`-opened one.
    ///
    /// A door already `Open`/`Opening`/`Closing` is still visited (so its
    /// touch state stays current for the next `Closed` edge), but
    /// [`Self::activate`]'s own `Door` arm only actually starts a move from
    /// `Closed`, exactly as it already does for a proximity `use` press —
    /// this method adds a *second way in* to the same state machine, not a
    /// second one.
    ///
    /// Returns how many doors this call actually started opening (a rising
    /// touch edge landing on a door still `Closed`), the same "did the
    /// state machine actually move" count [`Self::activate`]'s callers
    /// already compute by hand for a `use` press — mirrored here since a
    /// touch edge on a door that is already open, or one still animating,
    /// reaches [`Self::activate`] too but is a no-op there.
    ///
    /// `TODO(black-box)`: whether a monster (as opposed to the player) can
    /// open an eligible door by walking into it is not implemented — this
    /// method is called only with the player's own hull box, the same
    /// scope [`Self::touch_triggers`]/[`Self::touch_rot_buttons`] already
    /// have. A door with a nonzero `wait` that auto-closes while the
    /// player never leaves its touch volume also does not re-open: the
    /// touch-edge state stays high across the whole
    /// `Opening -> Open -> Closing -> Closed` cycle, so the closed-to-open
    /// edge this method looks for never re-fires, and the player is left
    /// standing against a closed door until they step out of its touch
    /// volume and back in. No public source states whether a real
    /// touch-triggered door re-opens under a standing player instead (the
    /// real engine reverses a *blocked* door, a different case from one
    /// that finished closing cleanly underneath a stationary player); this
    /// project's edge-triggered choice leaves that case unhandled rather
    /// than guessing at unpublished behaviour.
    pub fn touch_doors(
        &mut self,
        registry: &mut Registry,
        player_mins: Vec3,
        player_maxs: Vec3,
    ) -> usize {
        let mut candidates: Vec<(Entity, bool, bool)> = registry
            .world
            .query::<(Entity, &Door, &BrushBounds)>()
            .without::<&DoorUseOnly>()
            .without::<&DoorPassable>()
            .without::<&TargetName>()
            .iter()
            .map(|(entity, door, bounds)| {
                (
                    entity,
                    aabb_overlaps(
                        player_mins,
                        player_maxs,
                        bounds.mins - Vec3::splat(DOOR_TOUCH_MARGIN),
                        bounds.maxs + Vec3::splat(DOOR_TOUCH_MARGIN),
                    ),
                    door.state == MoverState::Closed,
                )
            })
            .collect();
        candidates.sort_unstable_by_key(|(entity, _, _)| entity.id());
        let mut opened = 0;
        for (entity, overlapping, was_closed) in candidates {
            let state = self.door_touch.entry(entity).or_default();
            let rising_edge = overlapping && !*state;
            *state = overlapping;
            if rising_edge {
                self.activate(registry, entity, None, &mut Vec::new());
                if was_closed {
                    opened += 1;
                }
            }
        }
        opened
    }

    /// Breaks `entity`'s [`Breakable`], when it has one that is not
    /// already broken: the brush is marked broken (which removes it from
    /// the drawn and solid brush lists, and detaches its collision hull —
    /// see [`crate::brush::solid_model_instances`] and
    /// `ohl_engine::Level::sync_brush_collision`) and its `target` is fired
    /// after the documented `delay` ("Target on Break... When the
    /// `func_breakable` is broken or triggered, it will activate this
    /// entity"; "Delay before fire... Delay before Target on Break is
    /// triggered after being broken" — `docs/FORMAT_SOURCES.md`, item 32).
    ///
    /// Returns whether this call was the one that broke it. Breaking is
    /// one-way: this project never restores a broken brush, and a second
    /// call is a no-op, so a `target` fires at most once per break.
    ///
    /// **Documented gaps** (`docs/FORMAT_SOURCES.md`, item 32): no gib is
    /// spawned, no per-[`Breakable::material`] break sound is played, and
    /// the documented `spawnobject` item is not spawned.
    pub fn break_entity(&mut self, registry: &mut Registry, entity: Entity) -> bool {
        let Ok(mut breakable) = registry.world.get::<&mut Breakable>(entity) else {
            return false;
        };
        if breakable.broken {
            return false;
        }
        breakable.broken = true;
        breakable.health = 0.0;
        let delay = breakable.delay;
        drop(breakable);
        let target = registry
            .world
            .get::<&Target>(entity)
            .ok()
            .map(|target| target.0.clone());
        if let Some(target) = target {
            self.fire(target, Some(entity), delay);
        }
        true
    }

    /// Applies `amount` damage to `entity`'s [`Breakable`], breaking it
    /// once its remaining `health` ("Strength - The amount of damage the
    /// entity will take before breaking") reaches zero.
    ///
    /// `club` reports whether the damage carried a club/melee damage type —
    /// this project's own stand-in for "whacked with a crowbar", since a
    /// resolved hit carries a damage *type*, not a weapon id: with the documented "Instant crowbar (256)" flag set,
    /// such a hit breaks the brush outright "regardless of strength"
    /// (`docs/FORMAT_SOURCES.md`, item 32, which records the melee-vs-
    /// crowbar approximation as project behaviour).
    ///
    /// Returns whether this call broke the brush. A brush with
    /// `health == 0` (a `func_pushable` without the documented "Breakable"
    /// flag, or a `func_breakable` a mapper left at the documented default)
    /// takes no damage at all and waits to be triggered, as does one with
    /// the documented "Only Trigger" flag. Non-finite or non-positive
    /// `amount` is ignored, the same guard `ohl_combat::apply_damage`
    /// applies on its own path.
    pub fn damage_breakable(
        &mut self,
        registry: &mut Registry,
        entity: Entity,
        amount: f32,
        club: bool,
    ) -> bool {
        let Ok(mut breakable) = registry.world.get::<&mut Breakable>(entity) else {
            return false;
        };
        if breakable.broken || breakable.trigger_only || breakable.health <= 0.0 {
            return false;
        }
        if !amount.is_finite() || amount <= 0.0 {
            return false;
        }
        let instant = club && breakable.instant_crowbar;
        let remaining = breakable.health - amount;
        breakable.health = remaining.max(0.0);
        let broke = instant || remaining <= 0.0;
        drop(breakable);
        if broke {
            return self.break_entity(registry, entity);
        }
        false
    }

    /// Breaks every `func_breakable` carrying the documented "Touch (2)"
    /// flag ("Brush will break on touch") whose own brush volume overlaps
    /// the box `[player_mins, player_maxs]`, the same brush-against-box
    /// overlap [`Self::touch_rot_buttons`] already uses for a touch-
    /// activated `func_rot_button`. Visited in ascending id order so a
    /// step touching several is deterministic.
    pub fn touch_breakables(
        &mut self,
        registry: &mut Registry,
        player_mins: Vec3,
        player_maxs: Vec3,
    ) {
        let mut touched: Vec<Entity> = registry
            .world
            .query::<(Entity, &Breakable, &BrushBounds)>()
            .iter()
            .filter(|(_, breakable, bounds)| {
                breakable.break_on_touch
                    && !breakable.broken
                    && aabb_overlaps(player_mins, player_maxs, bounds.mins, bounds.maxs)
            })
            .map(|(entity, _, _)| entity)
            .collect();
        touched.sort_unstable_by_key(|entity| entity.id());
        for entity in touched {
            self.break_entity(registry, entity);
        }
    }

    /// Drives the `momentary_rot_button` currently found by proximity while
    /// `use` is held (`held_entity`, from a caller-run
    /// [`find_usable_within`]-style search that also matches this
    /// component — this crate does not do that search itself, since
    /// "currently held" is a per-frame input state a host owns, not
    /// simulation-scheduled activation like every other entity
    /// [`Self::activate`] handles) toward `fraction = 1.0`
    /// (or back toward `0.0`, flipping at either endpoint — the documented
    /// "flip-flops between opening and closing when it reaches its
    /// endpoints" behaviour). Every *other* `momentary_rot_button` with the
    /// documented "Auto return" spawnflag continues animating back toward
    /// `fraction = 0.0` while not held, matching TWHL wiki
    /// `momentary_rot_button`'s documented auto-return behaviour.
    ///
    /// Every button whose own `fraction` changed this tick (held, or mid
    /// "Auto return") also pushes that value as a commanded fraction to
    /// every [`MomentaryDoor`] sharing its `target` keyvalue
    /// (`docs/FORMAT_SOURCES.md`, item 29): a second pass then moves each
    /// such door's own [`MomentaryDoor::fraction`] toward the commanded
    /// value at the door's own `speed`, so a door with no button currently
    /// driving it (nothing pushed to it this tick) simply holds wherever it
    /// last stopped — the documented "stays where you left it" shape a
    /// non-auto-return button already gives its own `fraction`.
    pub fn drive_momentary_rot_button(
        registry: &mut Registry,
        held_entity: Option<Entity>,
        dt: f32,
    ) {
        let mut commanded: Vec<(String, f32)> = Vec::new();
        for (entity, button, target) in
            registry
                .world
                .query_mut::<(Entity, &mut MomentaryRotButton, Option<&Target>)>()
        {
            let mut active = false;
            if Some(entity) == held_entity && !button.door_hack {
                active = true;
                button.returning = false;
                let step = if button.distance > 0.0 {
                    button.speed * dt / button.distance
                } else {
                    0.0
                };
                if button.moving_forward {
                    button.fraction += step;
                    if button.fraction >= 1.0 {
                        button.fraction = 1.0;
                        button.moving_forward = false;
                    }
                } else {
                    button.fraction -= step;
                    if button.fraction <= 0.0 {
                        button.fraction = 0.0;
                        button.moving_forward = true;
                    }
                }
            } else if button.fraction > 0.0
                && (button.returning || (button.auto_return && Some(entity) != held_entity))
            {
                active = true;
                button.returning = true;
                let step = if button.distance > 0.0 {
                    button.return_speed * dt / button.distance
                } else {
                    0.0
                };
                button.fraction = (button.fraction - step).max(0.0);
                if button.fraction <= 0.0 {
                    button.returning = false;
                    button.moving_forward = true;
                }
            }
            if active && let Some(target) = target {
                commanded.push((target.0.clone(), button.fraction));
            }
        }
        for (target_name, fraction) in commanded {
            let doors: Vec<Entity> = registry.find(&target_name).to_vec();
            for door_entity in doors {
                let Ok(mut door) = registry.world.get::<&mut MomentaryDoor>(door_entity) else {
                    continue;
                };
                let step = if door.travel_distance > 0.0 && door.speed > 0.0 {
                    door.speed * dt / door.travel_distance
                } else {
                    1.0
                };
                if door.fraction < fraction {
                    door.fraction = (door.fraction + step).min(fraction);
                } else if door.fraction > fraction {
                    door.fraction = (door.fraction - step).max(fraction);
                }
            }
        }
    }

    /// Advances every `func_rotating`'s accumulated angle while it is
    /// spinning. Unlike [`Self::advance_doors`]/[`Self::advance_platforms`]
    /// there is no open/close cycle to time: TWHL wiki `func_rotating`
    /// (`docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic")
    /// documents `speed` as a continuous degrees-per-second rate while the
    /// brush is on, so this simply integrates it and wraps the result into
    /// `0.0..360.0` so the stored angle never grows without bound over a
    /// long play session (the wrap is arithmetic, not a documented
    /// constant: a wrapped angle and its unwrapped equivalent describe the
    /// identical pose).
    fn advance_rotators(registry: &mut Registry, dt: f32) {
        for rotator in registry.world.query_mut::<&mut Rotator>() {
            if !rotator.spinning {
                continue;
            }
            rotator.angle_deg = (rotator.angle_deg + rotator.speed * dt).rem_euclid(360.0);
        }
    }

    /// The project-chosen decay rate a `damping` of `1000` (the documented
    /// maximum) applies, in `1/second`; see [`Self::advance_pendulums`]'s
    /// own doc comment for why no public source gives an exact formula.
    const PENDULUM_MAX_DAMPING_RATE: f32 = 1.0;

    /// The amplitude, in degrees, below which a damped `func_pendulum` is
    /// considered to have settled — TWHL wiki `func_pendulum` (`docs/
    /// FORMAT_SOURCES.md`, "Entity keyvalues and map logic") documents
    /// damping as narrowing the swing "until it stops moving ... in the
    /// middle of its swing".
    const PENDULUM_SETTLE_EPSILON_DEGREES: f32 = 0.05;

    /// Advances every `func_pendulum`'s swing.
    ///
    /// No public source states GoldSrc's exact per-step trigonometric
    /// integration; this project implements a plain damped sinusoid instead
    /// of guessing at an uncited formula (this milestone's own instruction):
    /// `angle_deg = amplitude(elapsed) * sin(omega * elapsed)`, with
    /// `angle_deg`/`amplitude`/`distance` all in the same unit ("degrees",
    /// the keyvalue's own documented unit) and `omega` a plain `1/second`
    /// scalar — **not itself a degrees- or radians-denominated rate** —
    /// chosen so `distance` (the documented swing amplitude) and `speed`
    /// (the documented "Speed of movement", in degrees/second) combine into
    /// the peak angular speed at the rest crossing:
    /// `d(angle_deg)/dt` at `elapsed = 0` is `amplitude * omega`, so setting
    /// `omega = speed / distance.max(1.0)` (both operands left in degrees,
    /// so they cancel to the dimensionless-but-1/second `omega` that
    /// `f32::sin`'s own always-radians argument convention needs) makes
    /// that peak rate come out to exactly `speed` degrees/second, matching
    /// this doc comment's own stated intent. **Corrected in this revision**:
    /// an earlier version of this formula wrote
    /// `speed.to_radians() / distance.max(1.0)`, converting only `speed`
    /// (not `distance`) to radians before the divide; since the two
    /// operands no longer shared a unit, the peak rate came out to
    /// `speed.to_radians()` degrees/second instead of `speed` degrees/
    /// second — about `180/π` (≈57.3x) slower than intended for every
    /// `func_pendulum` this project simulates (see
    /// `docs/FORMAT_SOURCES.md` item 27's append for the discovery and
    /// `pendulum_reaches_near_amplitude_within_the_documented_quarter_period`
    /// below for the regression test). — and the damped
    /// amplitude an exponential decay whose rate scales linearly with the
    /// documented `damping` `0..1000` keyvalue up to
    /// [`Self::PENDULUM_MAX_DAMPING_RATE`] per second, both project-chosen
    /// constants recorded here rather than in `docs/FORMAT_SOURCES.md`
    /// prose, per that document's own citation policy for an uncited
    /// numeric law. Once the damped amplitude falls under
    /// [`Self::PENDULUM_SETTLE_EPSILON_DEGREES`] the pendulum settles at
    /// the rest pose and stops spending CPU integrating an imperceptible
    /// wobble, matching the cited "stops ... in the middle" behaviour.
    ///
    /// The documented "Auto Return" spawnflag instead animates linearly back
    /// to the rest pose at `speed` degrees/second once toggled off
    /// ([`Simulation::activate`]'s `Pendulum` arm sets
    /// [`crate::registry::Pendulum::returning`]); a plain toggle-off with no
    /// "Auto Return" simply freezes [`crate::registry::Pendulum::angle_deg`]
    /// wherever it was (`elapsed` stays frozen too, so resuming continues
    /// the same sinusoid instead of restarting it).
    fn advance_pendulums(registry: &mut Registry, dt: f32) {
        for pendulum in registry.world.query_mut::<&mut Pendulum>() {
            if pendulum.returning {
                let step = pendulum.speed.max(1.0) * dt;
                if pendulum.angle_deg.abs() <= step {
                    pendulum.angle_deg = 0.0;
                    pendulum.returning = false;
                    pendulum.elapsed = 0.0;
                } else {
                    pendulum.angle_deg -= step * pendulum.angle_deg.signum();
                }
                continue;
            }
            if !pendulum.swinging {
                continue;
            }
            pendulum.elapsed += dt;
            let rate =
                (pendulum.damping / 1000.0).clamp(0.0, 1.0) * Self::PENDULUM_MAX_DAMPING_RATE;
            let amplitude = pendulum.distance * (-rate * pendulum.elapsed).exp();
            if amplitude < Self::PENDULUM_SETTLE_EPSILON_DEGREES {
                pendulum.angle_deg = 0.0;
                pendulum.swinging = false;
                pendulum.elapsed = 0.0;
                continue;
            }
            let omega = pendulum.speed / pendulum.distance.max(1.0);
            pendulum.angle_deg = amplitude * (omega * pendulum.elapsed).sin();
        }
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

    /// Advances every `trigger_camera` sequence, firing its completion
    /// `target` (see [`crate::camera::completion_target`]) exactly once, the
    /// tick [`crate::camera::TriggerCameraState::advance`] reports it
    /// finished. Entities are drained in ascending id order so which
    /// completion fires first is deterministic when more than one finishes
    /// on the same tick.
    fn advance_cameras(&mut self, registry: &mut Registry, dt: f32) {
        let mut completed: Vec<Entity> = registry
            .world
            .query_mut::<(Entity, &mut crate::camera::TriggerCameraState)>()
            .into_iter()
            .filter_map(|(entity, state)| state.advance(dt).then_some(entity))
            .collect();
        completed.sort_unstable_by_key(|entity| entity.id());
        for entity in completed {
            if let Some(target) = crate::camera::completion_target(registry, entity) {
                self.fire(target, Some(entity), 0.0);
            }
        }
    }
}

/// Whether the axis-aligned boxes `[a_mins, a_maxs]` and `[b_mins, b_maxs]`
/// overlap (touching exactly at a face counts, matching
/// [`BrushBounds::contains`]'s inclusive edges).
fn aabb_overlaps(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
    a_mins.cmple(b_maxs).all() && a_maxs.cmpge(b_mins).all()
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
    fn func_door_rotating_opens_and_closes_through_the_shared_door_timer() {
        let entities = vec![raw(&[
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("speed", "90"),
            ("distance", "90"),
            ("wait", "1"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let door_entity = registry.find("door1")[0];
        assert_eq!(
            registry
                .world
                .get::<&Door>(door_entity)
                .unwrap()
                .rotation_axis,
            Some(Vec3::Z)
        );
        let mut events = Vec::new();
        sim.use_entity(&mut registry, door_entity, None, &mut events);
        // 90 degrees at 90 degrees/second takes exactly one second to
        // finish opening.
        tick_for(&mut sim, &mut registry, 1.1, 0.05);
        {
            let door = registry.world.get::<&Door>(door_entity).unwrap();
            assert_eq!(door.state, MoverState::Open);
        }
        tick_for(&mut sim, &mut registry, 2.5, 0.05);
        let door = registry.world.get::<&Door>(door_entity).unwrap();
        assert_eq!(door.state, MoverState::Closed);
    }

    /// A `func_door_rotating` with a leaf compiled along `+y` from its
    /// pivot, spawning with the default `+Z` axis: a positive rotation
    /// sweeps that leaf toward `-x` (`Z x (+y) = -x`), so the door must
    /// keep `+Z` for an activator standing at `+x` and flip to `-Z` for
    /// one standing at `-x`. TWHL wiki `func_door_rotating`, "the door
    /// will always open away from the player" (`docs/FORMAT_SOURCES.md`
    /// items 24 and 26).
    fn rotating_door_registry(extra: &[(&str, &str)]) -> (Registry, Entity) {
        let mut pairs = vec![
            ("classname", "func_door_rotating"),
            ("targetname", "door1"),
            ("model", "*1"),
            ("origin", "0 0 0"),
            ("speed", "90"),
            ("distance", "90"),
            ("wait", "1"),
        ];
        pairs.extend_from_slice(extra);
        let defs = parse_entities(&[raw(&pairs)], &Limits::default());
        // The leaf's compiled bounds sit on the `+y` side of the pivot,
        // the convention `docs/FORMAT_SOURCES.md` item 24 recorded for an
        // origin-brush entity (geometry relative to the origin brush).
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([-4.0, 0.0, -32.0], [4.0, 64.0, 32.0]));
        let registry = Registry::build(&defs, &bounds, &Limits::default());
        let entity = registry.find("door1")[0];
        (registry, entity)
    }

    fn open_and_read_axis(registry: &mut Registry, entity: Entity, activator: Vec3) -> Vec3 {
        let mut sim = Simulation::new();
        sim.set_activator_origin(Some(activator));
        let mut events = Vec::new();
        sim.use_entity(registry, entity, None, &mut events);
        registry
            .world
            .get::<&Door>(entity)
            .unwrap()
            .rotation_axis
            .expect("a func_door_rotating always has a rotation axis")
    }

    #[test]
    fn a_rotating_door_swings_away_from_an_activator_on_either_side() {
        let (mut registry, entity) = rotating_door_registry(&[]);
        assert_eq!(
            open_and_read_axis(&mut registry, entity, Vec3::new(64.0, 32.0, 0.0)),
            Vec3::Z,
            "an activator on the +x side must not be swept into"
        );

        let (mut registry, entity) = rotating_door_registry(&[]);
        assert_eq!(
            open_and_read_axis(&mut registry, entity, Vec3::new(-64.0, 32.0, 0.0)),
            -Vec3::Z,
            "an activator on the -x side must not be swept into"
        );
    }

    #[test]
    fn a_one_way_rotating_door_ignores_the_activator() {
        let (mut registry, entity) = rotating_door_registry(&[("spawnflags", "16")]);
        assert!(
            registry
                .world
                .get::<&RotatingDoorSwing>(entity)
                .unwrap()
                .one_way
        );
        // The same activator position that flipped the axis above leaves
        // a "One Way" door on its spawnflag-chosen direction.
        assert_eq!(
            open_and_read_axis(&mut registry, entity, Vec3::new(-64.0, 32.0, 0.0)),
            Vec3::Z
        );
    }

    #[test]
    fn a_rotating_door_keeps_its_chosen_side_for_the_rest_of_the_cycle() {
        let (mut registry, entity) = rotating_door_registry(&[]);
        let mut sim = Simulation::new();
        sim.set_activator_origin(Some(Vec3::new(-64.0, 32.0, 0.0)));
        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);
        // Mid-swing the activator walks around to the other side; the
        // door must not mirror itself part-way through its own motion.
        sim.set_activator_origin(Some(Vec3::new(64.0, 32.0, 0.0)));
        tick_for(&mut sim, &mut registry, 0.5, 0.05);
        sim.use_entity(&mut registry, entity, None, &mut events);
        let door = registry.world.get::<&Door>(entity).unwrap();
        assert_eq!(door.rotation_axis, Some(-Vec3::Z));
        assert_eq!(door.state, MoverState::Opening);
    }

    #[test]
    fn func_rotating_use_toggles_spinning_and_advance_accumulates_angle() {
        let entities = vec![raw(&[
            ("classname", "func_rotating"),
            ("targetname", "fan1"),
            ("speed", "180"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry.find("fan1")[0];
        assert!(!registry.world.get::<&Rotator>(entity).unwrap().spinning);

        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);
        assert!(registry.world.get::<&Rotator>(entity).unwrap().spinning);

        sim.tick(&mut registry, 0.5);
        // 180 degrees/second for half a second is 90 degrees.
        let angle = registry.world.get::<&Rotator>(entity).unwrap().angle_deg;
        assert!((angle - 90.0).abs() < 1e-3, "angle was {angle}");

        // A second `use` stops it; the angle stays where it was.
        sim.use_entity(&mut registry, entity, None, &mut events);
        sim.tick(&mut registry, 0.5);
        let rotator = registry.world.get::<&Rotator>(entity).unwrap();
        assert!(!rotator.spinning);
        assert!((rotator.angle_deg - 90.0).abs() < 1e-3);
    }

    #[test]
    fn func_rot_button_presses_fires_target_and_auto_returns() {
        let entities = vec![
            raw(&[
                ("classname", "func_rot_button"),
                ("targetname", "btn1"),
                ("target", "door1"),
                ("speed", "90"),
                ("distance", "45"),
                ("wait", "1"),
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
        let button_entity = registry.find("btn1")[0];
        let door_entity = registry.find("door1")[0];
        let mut events = Vec::new();
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Closed
        );
        sim.use_entity(&mut registry, button_entity, None, &mut events);
        // 45 degrees at 90 degrees/second takes half a second to reach Open,
        // firing the target on the same tick it gets there.
        tick_for(&mut sim, &mut registry, 0.6, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Open
        );
        assert_eq!(
            registry.world.get::<&Door>(door_entity).unwrap().state,
            MoverState::Open
        );
        // `wait = 1`: the button auto-returns and reports Closed again.
        tick_for(&mut sim, &mut registry, 2.5, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Closed
        );
    }

    #[test]
    fn func_rot_button_toggle_retriggers_target_both_ways() {
        let entities = vec![
            raw(&[
                ("classname", "func_rot_button"),
                ("targetname", "btn1"),
                ("target", "counter1"),
                ("speed", "360"),
                ("distance", "90"),
                (
                    "spawnflags",
                    &crate::registry::SPAWNFLAG_ROT_BUTTON_TOGGLE.to_string(),
                ),
            ]),
            raw(&[("classname", "trigger_relay"), ("targetname", "counter1")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let button_entity = registry.find("btn1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, button_entity, None, &mut events);
        tick_for(&mut sim, &mut registry, 0.5, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Open
        );
        // Toggle buttons stay pressed until used again, unlike a plain
        // rot_button's auto-return.
        tick_for(&mut sim, &mut registry, 5.0, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Open
        );
        sim.use_entity(&mut registry, button_entity, None, &mut events);
        tick_for(&mut sim, &mut registry, 0.5, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Closed
        );
    }

    #[test]
    fn func_rot_button_touch_activates_from_player_bounding_box() {
        let entities = vec![raw(&[
            ("classname", "func_rot_button"),
            ("targetname", "btn1"),
            ("speed", "360"),
            ("distance", "90"),
            ("model", "*1"),
            (
                "spawnflags",
                &crate::registry::SPAWNFLAG_ROT_BUTTON_TOUCH.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([-8.0, -8.0, -8.0], [8.0, 8.0, 8.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let button_entity = registry.find("btn1")[0];
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Closed
        );
        sim.touch_rot_buttons(
            &mut registry,
            Vec3::new(-4.0, -4.0, -4.0),
            Vec3::new(4.0, 4.0, 4.0),
        );
        sim.tick(&mut registry, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Opening
        );
        // Standing on it does not re-fire every step (edge-triggered).
        for _ in 0..10 {
            sim.touch_rot_buttons(
                &mut registry,
                Vec3::new(-4.0, -4.0, -4.0),
                Vec3::new(4.0, 4.0, 4.0),
            );
        }
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        assert_eq!(
            registry
                .world
                .get::<&RotButton>(button_entity)
                .unwrap()
                .state,
            MoverState::Open
        );
    }

    #[test]
    fn momentary_rot_button_turns_while_held_and_flips_at_endpoints() {
        let entities = vec![raw(&[
            ("classname", "momentary_rot_button"),
            ("targetname", "valve1"),
            ("speed", "90"),
            ("distance", "90"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("valve1")[0];
        // Held for a full second: 90 degrees/second over 90 degrees reaches
        // fraction 1.0 in exactly one second and flips direction.
        for _ in 0..20 {
            Simulation::drive_momentary_rot_button(&mut registry, Some(entity), 0.05);
        }
        {
            let button = registry.world.get::<&MomentaryRotButton>(entity).unwrap();
            assert!((button.fraction - 1.0).abs() < 1e-3, "{}", button.fraction);
            assert!(!button.moving_forward);
        }
        // Released: with no Auto Return, it stays at fraction 1.0.
        Simulation::drive_momentary_rot_button(&mut registry, None, 0.05);
        let button = registry.world.get::<&MomentaryRotButton>(entity).unwrap();
        assert!((button.fraction - 1.0).abs() < 1e-3);
    }

    #[test]
    fn momentary_rot_button_auto_return_animates_back_to_zero_once_released() {
        let entities = vec![raw(&[
            ("classname", "momentary_rot_button"),
            ("targetname", "valve1"),
            ("speed", "90"),
            ("distance", "90"),
            ("returnspeed", "180"),
            (
                "spawnflags",
                &crate::registry::SPAWNFLAG_MOMENTARY_AUTO_RETURN.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let entity = registry.find("valve1")[0];
        Simulation::drive_momentary_rot_button(&mut registry, Some(entity), 0.5);
        let fraction = registry
            .world
            .get::<&MomentaryRotButton>(entity)
            .unwrap()
            .fraction;
        assert!((fraction - 0.5).abs() < 1e-3, "{fraction}");
        for _ in 0..10 {
            Simulation::drive_momentary_rot_button(&mut registry, None, 0.05);
        }
        let button = registry.world.get::<&MomentaryRotButton>(entity).unwrap();
        assert!((button.fraction - 0.0).abs() < 1e-3, "{}", button.fraction);
        assert!(!button.returning);
    }

    /// A `func_breakable` with hit points takes damage until it breaks, and
    /// breaking fires its documented "Target on Break".
    #[test]
    fn a_breakable_breaks_once_its_health_is_spent_and_fires_its_target() {
        let entities = vec![
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "crate1"),
                ("target", "door1"),
                ("health", "30"),
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
        let entity = registry.find("crate1")[0];
        let door = registry.find("door1")[0];

        assert!(!sim.damage_breakable(&mut registry, entity, 10.0, false));
        assert!(!registry.world.get::<&Breakable>(entity).unwrap().broken);
        assert!(
            (registry.world.get::<&Breakable>(entity).unwrap().health - 20.0).abs() < f32::EPSILON
        );

        assert!(sim.damage_breakable(&mut registry, entity, 25.0, false));
        assert!(registry.world.get::<&Breakable>(entity).unwrap().broken);
        // The `target` fire is scheduled, not immediate; one tick drains it.
        tick_for(&mut sim, &mut registry, 0.1, 0.05);
        assert_ne!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed,
            "breaking must fire the documented Target on Break"
        );

        // Breaking is one-way: further damage changes nothing.
        assert!(!sim.damage_breakable(&mut registry, entity, 100.0, false));
    }

    /// The documented "Only Trigger (1)" flag makes damage a no-op, while a
    /// trigger still breaks it; a `health` of `0` behaves the same way.
    #[test]
    fn an_only_trigger_or_zero_health_breakable_only_breaks_when_triggered() {
        let entities = vec![
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "only_trigger"),
                ("health", "30"),
                ("spawnflags", "1"),
            ]),
            raw(&[("classname", "func_breakable"), ("targetname", "no_health")]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        for name in ["only_trigger", "no_health"] {
            let entity = registry.find(name)[0];
            assert!(!sim.damage_breakable(&mut registry, entity, 1000.0, false));
            assert!(!registry.world.get::<&Breakable>(entity).unwrap().broken);
            sim.use_entity(&mut registry, entity, None, &mut Vec::new());
            assert!(
                registry.world.get::<&Breakable>(entity).unwrap().broken,
                "{name} must break when triggered"
            );
        }
    }

    /// The documented "Instant crowbar (256)" flag breaks the brush on a
    /// single club hit "regardless of strength"; the same hit without the
    /// flag only takes its own damage off.
    #[test]
    fn the_instant_crowbar_flag_breaks_on_one_club_hit() {
        let entities = vec![
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "instant"),
                ("health", "500"),
                ("spawnflags", "256"),
            ]),
            raw(&[
                ("classname", "func_breakable"),
                ("targetname", "ordinary"),
                ("health", "500"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let instant = registry.find("instant")[0];
        let ordinary = registry.find("ordinary")[0];

        assert!(sim.damage_breakable(&mut registry, instant, 10.0, true));
        assert!(registry.world.get::<&Breakable>(instant).unwrap().broken);

        assert!(!sim.damage_breakable(&mut registry, ordinary, 10.0, true));
        assert!(!registry.world.get::<&Breakable>(ordinary).unwrap().broken);
    }

    /// A `func_pushable` is only damageable with the documented
    /// "Breakable (128)" flag; without it, its own `health` is ignored
    /// entirely (the cited page states `health` applies only "If
    /// breakable"), and its documented `friction` is carried through.
    #[test]
    fn a_pushable_is_only_breakable_with_its_documented_flag() {
        use crate::registry::Pushable;
        let entities = vec![
            raw(&[
                ("classname", "func_pushable"),
                ("targetname", "plain"),
                ("health", "20"),
                ("friction", "100"),
            ]),
            raw(&[
                ("classname", "func_pushable"),
                ("targetname", "breakable"),
                ("health", "20"),
                ("spawnflags", "128"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let plain = registry.find("plain")[0];
        let breakable = registry.find("breakable")[0];

        assert!(
            (registry.world.get::<&Pushable>(plain).unwrap().friction - 100.0).abs() < f32::EPSILON
        );
        assert!(!sim.damage_breakable(&mut registry, plain, 1000.0, false));
        assert!(!registry.world.get::<&Breakable>(plain).unwrap().broken);

        assert!(sim.damage_breakable(&mut registry, breakable, 1000.0, false));
        assert!(registry.world.get::<&Breakable>(breakable).unwrap().broken);
    }

    #[test]
    fn func_pendulum_swings_and_toggles_off_freezing_the_pose() {
        let entities = vec![raw(&[
            ("classname", "func_pendulum"),
            ("targetname", "swing1"),
            ("distance", "30"),
            ("speed", "180"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry.find("swing1")[0];
        assert!(!registry.world.get::<&Pendulum>(entity).unwrap().swinging);

        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);
        assert!(registry.world.get::<&Pendulum>(entity).unwrap().swinging);
        sim.tick(&mut registry, 0.1);
        let angle_while_swinging = registry.world.get::<&Pendulum>(entity).unwrap().angle_deg;
        assert!(
            angle_while_swinging.abs() > 0.0,
            "the pendulum should have moved off rest"
        );

        // Toggling off with no Auto Return freezes the pose in place.
        sim.use_entity(&mut registry, entity, None, &mut events);
        assert!(!registry.world.get::<&Pendulum>(entity).unwrap().swinging);
        sim.tick(&mut registry, 1.0);
        let angle_after_stop = registry.world.get::<&Pendulum>(entity).unwrap().angle_deg;
        assert!((angle_after_stop - angle_while_swinging).abs() < 1e-6);
    }

    #[test]
    fn func_pendulum_auto_return_animates_back_to_rest_when_toggled_off() {
        let entities = vec![raw(&[
            ("classname", "func_pendulum"),
            ("targetname", "swing1"),
            ("distance", "30"),
            ("speed", "180"),
            (
                "spawnflags",
                &crate::registry::SPAWNFLAG_PENDULUM_AUTO_RETURN.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry.find("swing1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);
        sim.tick(&mut registry, 0.1);
        assert!(
            registry
                .world
                .get::<&Pendulum>(entity)
                .unwrap()
                .angle_deg
                .abs()
                > 0.0
        );

        sim.use_entity(&mut registry, entity, None, &mut events);
        assert!(registry.world.get::<&Pendulum>(entity).unwrap().returning);
        tick_for(&mut sim, &mut registry, 2.0, 0.05);
        let pendulum = registry.world.get::<&Pendulum>(entity).unwrap();
        assert!(
            (pendulum.angle_deg - 0.0).abs() < 1e-3,
            "{}",
            pendulum.angle_deg
        );
        assert!(!pendulum.returning);
    }

    /// Regression for the `omega` unit-conversion bug this milestone found
    /// via real-map fidelity testing (see `docs/FORMAT_SOURCES.md` item
    /// 27's append and `Simulation::advance_pendulums`'s own doc comment):
    /// an earlier revision computed
    /// `omega = pendulum.speed.to_radians() / pendulum.distance.max(1.0)`,
    /// converting only `speed` to radians while leaving `distance`
    /// unconverted, which made the true peak angular rate come out to
    /// `speed.to_radians()` degrees/second instead of `speed`
    /// degrees/second — about `180/π` (\u{2248}57.3x) slower than the
    /// doc comment's own stated intent. Undamped (`damping = 0`), the doc
    /// comment's formula predicts the swing reaches its first peak
    /// (`amplitude`, i.e. `distance`, since nothing has decayed yet) at
    /// `elapsed = (pi/2) / omega = (pi/2) * distance / speed`; for this
    /// test's `speed = 180`\u{b0}/s, `distance = 30`\u{b0}, that is
    /// `elapsed \u{2248} 0.2618s`. This test discriminates the bug: run
    /// against the buggy formula above, the same elapsed time produces an
    /// angle of well under 1\u{b0} (confirmed locally by temporarily
    /// reverting the fix and re-running this test, which then fails), not
    /// the near-`distance` value asserted below.
    #[test]
    fn pendulum_reaches_near_amplitude_within_the_documented_quarter_period() {
        let speed = 180.0_f32;
        let distance = 30.0_f32;
        let entities = vec![raw(&[
            ("classname", "func_pendulum"),
            ("targetname", "swing1"),
            ("distance", "30"),
            ("speed", "180"),
            ("damping", "0"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry.find("swing1")[0];
        let mut events = Vec::new();
        sim.use_entity(&mut registry, entity, None, &mut events);

        let quarter_period = (std::f32::consts::FRAC_PI_2) * distance / speed;
        tick_for(&mut sim, &mut registry, quarter_period, 0.001);

        let angle = registry.world.get::<&Pendulum>(entity).unwrap().angle_deg;
        assert!(
            (angle - distance).abs() < 0.5,
            "expected the pendulum within 0.5 degrees of its {distance} degree amplitude \
             after {quarter_period}s (a quarter of its documented swing period), got {angle}"
        );
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

    /// `Simulation::damage_button`'s documented press: a `func_button` with
    /// `health = 50` takes two 30-point hits (60 total, past the 50
    /// threshold) and presses exactly once, firing `target` — the same
    /// place a `use` press would — rather than pressing on the first,
    /// insufficient hit or pressing twice for the overshoot.
    #[test]
    fn damage_button_presses_once_and_fires_target_once_health_is_exhausted() {
        let entities = vec![
            raw(&[
                ("classname", "func_button"),
                ("targetname", "btn1"),
                ("target", "door1"),
                ("health", "50"),
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

        assert!(
            !sim.damage_button(&mut registry, button, 30.0),
            "30 of 50 health must not press the button yet"
        );
        assert_eq!(
            registry.world.get::<&Button>(button).unwrap().state,
            MoverState::Closed
        );
        assert!(
            sim.damage_button(&mut registry, button, 30.0),
            "a second 30-point hit (60 total) must exhaust the button's 50 health and press it"
        );
        assert_eq!(
            registry.world.get::<&Button>(button).unwrap().state,
            MoverState::Opening
        );
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        let door_component = registry.world.get::<&Door>(door).unwrap();
        assert_eq!(
            door_component.state,
            MoverState::Open,
            "the damage-triggered press must fire target through the same path a use press does"
        );
    }

    /// [`Simulation::damage_button`] on a `func_button`/`func_rot_button`
    /// with `health = 0` (the documented "responds only to `use`/touch"
    /// default) must never press, regardless of how much damage arrives.
    #[test]
    fn damage_button_does_nothing_when_health_is_zero() {
        let entities = vec![raw(&[
            ("classname", "func_button"),
            ("targetname", "btn1"),
            ("target", "door1"),
            ("wait", "1"),
            ("delay", "0"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let button = registry.find("btn1")[0];
        assert!(!sim.damage_button(&mut registry, button, 1_000_000.0));
        assert_eq!(
            registry.world.get::<&Button>(button).unwrap().state,
            MoverState::Closed
        );
    }

    /// The same exhaustion/press shape as
    /// [`damage_button_presses_once_and_fires_target_once_health_is_exhausted`],
    /// for a `func_rot_button` instead of a `func_button` — the two share
    /// [`Simulation::damage_button`]'s implementation, but the rotating
    /// button's own press path (`Simulation::advance_rot_buttons`) is a
    /// distinct state machine worth its own regression.
    #[test]
    fn damage_rot_button_presses_once_and_fires_target_once_health_is_exhausted() {
        let entities = vec![
            raw(&[
                ("classname", "func_rot_button"),
                ("targetname", "btn1"),
                ("target", "door1"),
                ("speed", "90"),
                ("distance", "45"),
                ("wait", "1"),
                ("health", "10"),
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

        assert!(!sim.damage_button(&mut registry, button, 9.0));
        assert_eq!(
            registry.world.get::<&RotButton>(button).unwrap().state,
            MoverState::Closed
        );
        assert!(sim.damage_button(&mut registry, button, 1.0));
        tick_for(&mut sim, &mut registry, 0.6, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Open
        );
    }

    /// Regression for the campaign start map's intro tram: a
    /// `func_tracktrain` with `startspeed 0` (so it does not move on its
    /// own) whose only activation path is a `multi_manager` reached
    /// through the documented `game_playerspawn` special `targetname` —
    /// no `trigger_auto` anywhere in the map. Before [`Simulation::fire_player_spawn`]
    /// existed, nothing in this crate ever looked up that name, so this
    /// train never moved and the scenario in
    /// `docs/FORMAT_SOURCES.md` ("Entity keyvalues and map logic",
    /// "game_playerspawn") reproduced exactly: idling for many ticks left
    /// the train parked at its first node.
    #[test]
    fn game_playerspawn_activates_a_relay_that_starts_a_parked_tram() {
        let entities = vec![
            raw(&[
                ("classname", "multi_manager"),
                ("targetname", "game_playerspawn"),
                ("tram", "0.0"),
            ]),
            raw(&[
                ("classname", "func_tracktrain"),
                ("targetname", "tram"),
                ("target", "node1"),
                ("speed", "50"),
                ("startspeed", "0"),
                ("height", "0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node1"),
                ("target", "node2"),
                ("origin", "0 0 0"),
            ]),
            raw(&[
                ("classname", "path_track"),
                ("targetname", "node2"),
                ("origin", "100 0 0"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let mut sim = Simulation::new();
        let tram = registry.find("tram")[0];
        {
            let state = registry.world.get::<&TrackTrainState>(tram).unwrap();
            assert_eq!(state.position(), Vec3::ZERO, "parked at its first node");
        }
        // A 40-second idle run at a typical fixed step: matches the review
        // window this regression is drawn from. No `use_entity` call and
        // no `trigger_auto` anywhere in this fixture — only ordinary
        // `tick`s, exactly like a player standing still after map load.
        tick_for(&mut sim, &mut registry, 40.0, 0.05);
        let state = registry.world.get::<&TrackTrainState>(tram).unwrap();
        assert!(
            state.position().x > 0.0,
            "game_playerspawn must have started the tram moving toward node2, got {:?}",
            state.position()
        );
    }

    /// A second call to [`Simulation::tick`] must not re-activate a
    /// `game_playerspawn`-named relay: it is a one-shot "player entered
    /// the world" event, not a per-tick poll (mirroring `trigger_auto`'s
    /// own already-established one-shot `fired` behaviour).
    #[test]
    fn game_playerspawn_fires_only_once() {
        let entities = vec![
            raw(&[
                ("classname", "func_button"),
                ("targetname", "game_playerspawn"),
                ("target", "door1"),
                ("wait", "0"),
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
        let door = registry.find("door1")[0];
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        {
            let door_component = registry.world.get::<&Door>(door).unwrap();
            assert_eq!(door_component.state, MoverState::Open);
        }
        // Close it back down by hand and confirm further ticks (i.e. more
        // simulated time passing, not a second spawn) never reopen it.
        {
            let mut door_component = registry.world.get::<&mut Door>(door).unwrap();
            door_component.state = MoverState::Closed;
            door_component.timer = 0.0;
        }
        tick_for(&mut sim, &mut registry, 1.0, 0.05);
        let door_component = registry.world.get::<&Door>(door).unwrap();
        assert_eq!(door_component.state, MoverState::Closed);
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

    /// The entity carrying the fixture's only `Door` component — used
    /// throughout the `touch_doors` tests below in place of
    /// `Registry::find`, since an *unnamed* door (the case this method
    /// actually opens — see [`Simulation::touch_doors`]'s own doc comment
    /// for the cited "unless they have a name" clause) has no `targetname`
    /// to look it up by.
    fn only_door(registry: &Registry) -> Entity {
        registry
            .world
            .query::<(Entity, &Door)>()
            .iter()
            .next()
            .expect("the fixture declares exactly one door")
            .0
    }

    /// A plain, *unnamed* `func_door` (no "Use Only"/"Passable" spawnflag,
    /// no `targetname`) opens the moment the player's own hull box
    /// overlaps its brush — no `use` press, no separate `trigger_*` volume
    /// — the gap this milestone closes. The Sven Co-op wiki's `Func_door`
    /// page cites this as the unnamed half of its touch rule (see
    /// `Simulation::touch_doors`'s own doc comment).
    #[test]
    fn touch_opens_a_plain_unnamed_door() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("model", "*1"),
            ("speed", "100"),
            ("wait", "-1"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([100.0, -32.0, 0.0], [116.0, 32.0, 72.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = only_door(&registry);

        // Short of the door: no overlap even with the touch margin, so it
        // stays closed.
        sim.touch_doors(
            &mut registry,
            Vec3::new(-16.0, -16.0, -36.0),
            Vec3::new(16.0, 16.0, 36.0),
        );
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );

        // Walking forward, the player's own hull now overlaps the door's
        // brush.
        let opened = sim.touch_doors(
            &mut registry,
            Vec3::new(84.0, -16.0, -36.0),
            Vec3::new(116.0, 16.0, 36.0),
        );
        assert_eq!(opened, 1);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Opening
        );
    }

    /// A `func_door` with a `targetname` never opens from
    /// [`Simulation::touch_doors`], even standing squarely inside its
    /// brush and carrying neither "Use Only" nor "Passable" — the "unless
    /// they have a name, in which's case they require to be triggered
    /// manually" half of the cited touch rule. A direct `Simulation::
    /// activate` call (standing in for another entity's fire chain) still
    /// opens it.
    #[test]
    fn touch_does_nothing_for_a_named_door() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("targetname", "door1"),
            ("model", "*1"),
            ("speed", "100"),
            ("wait", "-1"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([100.0, -32.0, 0.0], [116.0, 32.0, 72.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = registry.find("door1")[0];

        let opened = sim.touch_doors(
            &mut registry,
            Vec3::new(100.0, -16.0, -36.0),
            Vec3::new(116.0, 16.0, 36.0),
        );
        assert_eq!(opened, 0);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed,
            "a named door must not open from touch"
        );

        // Another entity's fire chain still reaches it (a `use` press
        // does too, through `use_entity`, exercised elsewhere).
        sim.use_entity(&mut registry, door, None, &mut Vec::new());
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Opening,
            "a named door must still open when triggered"
        );
    }

    /// A `func_door` with the "Use Only" spawnflag set (256;
    /// `SPAWNFLAG_DOOR_USE_ONLY`), unnamed so the assertion isolates this
    /// flag's own effect from the `targetname` exclusion proved above,
    /// never opens from [`Simulation::touch_doors`] — only `use` (proved
    /// in the same test) reaches it.
    #[test]
    fn touch_does_nothing_for_a_use_only_door() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("model", "*1"),
            ("speed", "100"),
            ("wait", "-1"),
            (
                "spawnflags",
                &crate::registry::SPAWNFLAG_DOOR_USE_ONLY.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([100.0, -32.0, 0.0], [116.0, 32.0, 72.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = only_door(&registry);

        let opened = sim.touch_doors(
            &mut registry,
            Vec3::new(100.0, -16.0, -36.0),
            Vec3::new(116.0, 16.0, 36.0),
        );
        assert_eq!(opened, 0);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed,
            "a Use Only door must not open from touch"
        );

        // `use` still reaches it.
        let mut events = Vec::new();
        sim.use_entity(&mut registry, door, None, &mut events);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Opening,
            "a Use Only door must still open from a use press"
        );
    }

    /// A `func_door` with the "Passable" spawnflag set (8;
    /// `SPAWNFLAG_DOOR_PASSABLE`), unnamed, never opens from
    /// [`Simulation::touch_doors`] either: the cited page documents it as
    /// non-solid *and* not touch-triggerable.
    #[test]
    fn touch_does_nothing_for_a_passable_door() {
        let entities = vec![raw(&[
            ("classname", "func_door"),
            ("model", "*1"),
            ("speed", "100"),
            ("wait", "-1"),
            (
                "spawnflags",
                &crate::registry::SPAWNFLAG_DOOR_PASSABLE.to_string(),
            ),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([100.0, -32.0, 0.0], [116.0, 32.0, 72.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = only_door(&registry);

        let opened = sim.touch_doors(
            &mut registry,
            Vec3::new(100.0, -16.0, -36.0),
            Vec3::new(116.0, 16.0, 36.0),
        );
        assert_eq!(opened, 0);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed,
            "a Passable door must not open from touch"
        );
    }

    /// Reproduces the training-map fixture: a closed `func_door` gated by an
    /// adjoining `trigger_multiple` touch volume, wired only through
    /// `target`/`targetname` the way a real map does — no direct `use` on
    /// the door itself. Walking the player's bounding box into the volume
    /// (not merely moving its origin near the door) must open the door;
    /// before `touch_triggers` existed nothing ever activated a touch
    /// trigger from player movement at all.
    #[test]
    fn touch_trigger_opens_gated_door_from_player_bounding_box() {
        let entities = vec![
            raw(&[
                ("classname", "trigger_multiple"),
                ("targetname", "startdoor_trigger"),
                ("target", "startdoor"),
                ("model", "*1"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "startdoor"),
                ("speed", "100"),
                ("wait", "-1"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        // The trigger volume sits well ahead of the origin, the way a
        // touch trigger in front of a door does.
        bounds.insert(1u32, ([100.0, -32.0, 0.0], [164.0, 32.0, 72.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = registry.find("startdoor")[0];
        let mut events = Vec::new();

        // The player's origin (a standing-hull-sized box) has not yet
        // reached the volume: no touch, door stays closed.
        sim.touch_triggers(
            &mut registry,
            Vec3::new(-16.0, -16.0, -36.0),
            Vec3::new(16.0, 16.0, 36.0),
            None,
            &mut events,
        );
        sim.tick(&mut registry, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );

        // Walking forward, the player's bounding box now overlaps the
        // trigger volume even though its origin (0,0,0 + 120 = still short
        // of centre) has not reached the volume's own centre.
        sim.touch_triggers(
            &mut registry,
            Vec3::new(84.0, -16.0, -36.0),
            Vec3::new(116.0, 16.0, 36.0),
            None,
            &mut events,
        );
        sim.tick(&mut registry, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Open
        );
    }

    #[test]
    fn touch_triggers_ignore_trigger_hurt_targets() {
        // `trigger_hurt` shares the `Trigger` component but its touch path
        // is handled elsewhere (a radius test against the player's
        // origin in `ohl-engine`'s player-systems phase); it must not also
        // fire its `target` through the bounding-box touch path.
        let entities = vec![
            raw(&[
                ("classname", "trigger_hurt"),
                ("targetname", "hurt1"),
                ("target", "door1"),
                ("model", "*1"),
                ("dmg", "5"),
            ]),
            raw(&[
                ("classname", "func_door"),
                ("targetname", "door1"),
                ("wait", "-1"),
            ]),
        ];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let door = registry.find("door1")[0];
        let mut events = Vec::new();
        sim.touch_triggers(
            &mut registry,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            None,
            &mut events,
        );
        sim.tick(&mut registry, 0.05);
        assert_eq!(
            registry.world.get::<&Door>(door).unwrap().state,
            MoverState::Closed
        );
    }

    /// A plain (non-"USE Only") `trigger_changelevel` fires from the
    /// player's own bounding box overlapping its volume, the same way a
    /// `trigger_multiple` does — per the TWHL `trigger_changelevel` page
    /// (see `docs/FORMAT_SOURCES.md`, "Entity keyvalues and map logic").
    #[test]
    fn trigger_changelevel_fires_on_touch() {
        let entities = vec![raw(&[
            ("classname", "trigger_changelevel"),
            ("model", "*1"),
            ("map", "next_map"),
            ("landmark", "next_landmark"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();

        // Not yet overlapping: the volume has never been observed, so per
        // this project's conservative reading it must not fire even though
        // this frame is the first check.
        let mut events = Vec::new();
        sim.touch_triggers(
            &mut registry,
            Vec3::new(200.0, 200.0, 200.0),
            Vec3::new(210.0, 210.0, 210.0),
            None,
            &mut events,
        );
        assert!(events.is_empty());

        // Walks into the volume: a genuine rising edge, so it fires.
        sim.touch_triggers(
            &mut registry,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            None,
            &mut events,
        );
        assert_eq!(
            events,
            vec![Event::LevelChange(LevelChange {
                map: "next_map".to_string(),
                landmark: "next_landmark".to_string(),
            })]
        );

        // Still overlapping on the next check: must not fire again.
        events.clear();
        sim.touch_triggers(
            &mut registry,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            None,
            &mut events,
        );
        assert!(events.is_empty());
    }

    /// The published "USE Only" spawnflag (`2`) on `trigger_changelevel`:
    /// per the TWHL wiki page, "Entity can only be triggered by another
    /// event", so a player merely touching the volume must not fire it
    /// (only `Simulation::use_entity`/being targeted may).
    #[test]
    fn trigger_changelevel_use_only_ignores_touch() {
        let entities = vec![raw(&[
            ("classname", "trigger_changelevel"),
            ("model", "*1"),
            ("map", "next_map"),
            ("landmark", "next_landmark"),
            ("spawnflags", "2"),
        ])];
        let defs = parse_entities(&entities, &Limits::default());
        let mut bounds = BTreeMap::new();
        bounds.insert(1u32, ([0.0, 0.0, 0.0], [64.0, 64.0, 64.0]));
        let mut registry = Registry::build(&defs, &bounds, &Limits::default());
        let mut sim = Simulation::new();
        let entity = registry
            .world
            .query::<(Entity, &ChangeLevel)>()
            .iter()
            .next()
            .expect("the fixture declares one trigger_changelevel")
            .0;

        let mut events = Vec::new();
        sim.touch_triggers(
            &mut registry,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            None,
            &mut events,
        );
        sim.touch_triggers(
            &mut registry,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            None,
            &mut events,
        );
        assert!(events.is_empty(), "\"USE Only\" must never fire from touch");

        // `use` still works.
        sim.use_entity(&mut registry, entity, None, &mut events);
        assert_eq!(
            events,
            vec![Event::LevelChange(LevelChange {
                map: "next_map".to_string(),
                landmark: "next_landmark".to_string(),
            })]
        );
    }
}
