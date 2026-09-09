//! Level transitions: what survives a `trigger_changelevel`, and how it is
//! placed in the destination map.
//!
//! Semantics come only from public mapping documentation (see
//! `docs/FORMAT_SOURCES.md`, "Campaign flow"): a transition is described by
//! a `trigger_changelevel` naming the destination `map` and a `landmark`,
//! plus a matching pair of `info_landmark` entities (one per map) sharing
//! that name. The player is placed in the destination at the *same offset
//! from the landmark* it had in the source map. An optional
//! `trigger_transition` volume named after the same landmark bounds which
//! entities are eligible to travel; when a map declares none, this
//! implementation falls back to a documented radius
//! ([`DEFAULT_CARRY_RADIUS`]) around the landmark. Entities are correlated
//! across the two maps by `globalname` (and, for the previous map's own
//! mover state, by `targetname`), and the worldspawn `newunit` key discards
//! carried state instead of applying it. A landmark either map does not
//! declare leaves every offset unmeasurable, so the player stays at the
//! destination's own `info_player_start` and only state that needs no
//! placement travels.
//!
//! # To verify
//!
//! The exact `env_global` save semantics are modelled here from the
//! documented *behaviour* of a named global that is off/on/dead, not from a
//! retrieved specification of its stored form; see `.plan/m8-research.md`
//! open item 2 and [`ohl_game::registry::GlobalStateValue`].

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_game::hecs::Entity;
use ohl_game::keyvalues::{EntityDef, Limits as KeyvalueLimits};
use ohl_game::registry::{
    BrushBounds, Button, ClassName, Door, EnvGlobal, GlobalName, GlobalStateValue, Landmark, Light,
    Message, MoverState, Platform, Registry, RenderPropsComponent, Rotator, SpawnFlags, Target,
    TargetName, Transform, TransitionVolume, Trigger,
};
use ohl_game::track_train::{PathChain, TrackTrain, TrackTrainState};
use serde::{Deserialize, Serialize};

use crate::level::Level;

/// How far from the landmark an entity may be and still travel to the next
/// map when neither map declares a `trigger_transition` volume for that
/// landmark.
///
/// The public documentation states the eligibility rule in terms of the
/// transition volume (or the landmark's PVS) rather than a distance, so
/// this radius is a project-chosen, documented stand-in for the PVS test
/// this engine does not run at level-change time — not a value read from
/// any specification.
pub const DEFAULT_CARRY_RADIUS: f32 = 512.0;

/// The largest number of entities one transition carries, so a map full of
/// named entities cannot make a transition unbounded.
pub const MAX_CARRIED_ENTITIES: usize = 256;

/// The largest number of keyvalues carried per entity, so a transition
/// stays bounded however many keys a map hangs on one entity. Matches
/// `ohl_game::keyvalues::Limits`' own per-entity default.
pub const MAX_CARRIED_KEYVALUES: usize = 64;

/// The player state a transition (and a save file) carries.
///
/// `health` and `armor` are the player's real values (`ohl_player::Player`'s
/// own state, M7.9 P1); `extra` is an opaque, already-serialized blob —
/// today `crate::combat::CombatState::capture_carry`'s encoding of owned
/// weapons, per-weapon clips, reserve ammo, the HEV suit and the long jump
/// module — built and applied by `crate::systems::Systems::{capture_carry,
/// restore_carry}`, which `Game::capture_transition`/`to_save` and
/// `Game::apply_transition`/`restore` call directly. `extra`'s *shape* is
/// deliberately opaque to this module, so a later encoding change needs no
/// change here.
///
/// **To verify:** that the player's inventory persists across a
/// `changelevel` at all is community knowledge that the M8 research pass
/// could not confirm from a reachable public page; see
/// `.plan/m8-research.md` open item 3.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerCarryState {
    /// Health carried into the next map.
    pub health: f32,
    /// Armor carried into the next map.
    pub armor: f32,
    /// Whatever the [`PlayerCarry`] implementation additionally serialized.
    pub extra: Vec<u8>,
}

impl Default for PlayerCarryState {
    fn default() -> Self {
        Self {
            health: 100.0,
            armor: 0.0,
            extra: Vec::new(),
        }
    }
}

/// A host-replaceable seam over [`PlayerCarryState`], predating M7.9 P1's
/// direct `crate::systems::Systems::{capture_carry, restore_carry}` wiring.
/// `Game` still notifies whatever is installed here on every transition and
/// save/load (so [`Game::player_carry`](crate::game::Game::player_carry)
/// stays readable for a host that wants it), but the *authoritative* health,
/// armor, weapons and ammo capture and restore for a transition or a save
/// goes through `Systems` directly today, not through this trait's own
/// implementation.
pub trait PlayerCarry {
    /// The state to carry across.
    fn capture(&self) -> PlayerCarryState;
    /// Applies previously captured state.
    fn restore(&mut self, state: &PlayerCarryState);
}

/// The default [`PlayerCarry`]: a plain holder for whatever was last
/// [`restore`](PlayerCarry::restore)d, with no capture logic of its own —
/// see the trait's doc comment for why that no longer matters for
/// correctness.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefaultPlayerCarry {
    /// The carried state, readable by a HUD.
    pub state: PlayerCarryState,
}

impl PlayerCarry for DefaultPlayerCarry {
    fn capture(&self) -> PlayerCarryState {
        self.state.clone()
    }

    fn restore(&mut self, state: &PlayerCarryState) {
        self.state = state.clone();
    }
}

/// Every component of one entity that this engine persists, as serialized
/// state. Components the entity does not have stay `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    /// `spawnflags`.
    pub spawnflags: Option<u32>,
    /// `rendermode`/`renderamt`/`rendercolor`.
    pub render: Option<RenderPropsComponent>,
    /// Position and angles.
    pub transform: Option<Transform>,
    /// `func_door` state.
    pub door: Option<Door>,
    /// `func_button` state.
    pub button: Option<Button>,
    /// `func_plat` state.
    pub platform: Option<Platform>,
    /// `func_rotating` state (`spinning`/`angle_deg`), so a rotator the
    /// player switched on keeps spinning, at its accumulated angle, in the
    /// next map rather than reverting to its spawnflag default. Doors,
    /// buttons and platforms above carry their own state machines the same
    /// way.
    pub rotator: Option<Rotator>,
    /// Light brightness/colour/style.
    pub light: Option<Light>,
    /// Trigger keys.
    pub trigger: Option<Trigger>,
    /// `env_message`/`game_text` keys.
    pub message: Option<Message>,
}

impl EntitySnapshot {
    /// Reads every persisted component off `entity`.
    #[must_use]
    pub fn capture(registry: &Registry, entity: Entity) -> Self {
        let world = &registry.world;
        Self {
            spawnflags: world.get::<&SpawnFlags>(entity).ok().map(|c| c.0),
            render: world.get::<&RenderPropsComponent>(entity).ok().map(|c| *c),
            transform: world.get::<&Transform>(entity).ok().map(|c| *c),
            door: world.get::<&Door>(entity).ok().map(|c| *c),
            button: world.get::<&Button>(entity).ok().map(|c| *c),
            platform: world.get::<&Platform>(entity).ok().map(|c| *c),
            rotator: world.get::<&Rotator>(entity).ok().map(|c| *c),
            light: world.get::<&Light>(entity).ok().map(|c| *c),
            trigger: world.get::<&Trigger>(entity).ok().map(|c| *c),
            message: world
                .get::<&Message>(entity)
                .ok()
                .map(|c| Message::clone(&c)),
        }
    }

    /// Writes every present component back onto `entity`, inserting it when
    /// the entity does not already carry that component.
    pub fn apply(&self, registry: &mut Registry, entity: Entity) {
        let world = &mut registry.world;
        if let Some(value) = self.spawnflags {
            world.insert_one(entity, SpawnFlags(value)).ok();
        }
        if let Some(value) = self.render {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.transform {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.door {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.button {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.platform {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.rotator {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.light {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.trigger {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.message.clone() {
            world.insert_one(entity, value).ok();
        }
    }

    /// As [`Self::apply`], but onto an entity the *destination map*
    /// already declares — a `globalname` counterpart, or one the
    /// destination happens to name the same way — rather than onto an
    /// entity this transition is creating from nothing.
    ///
    /// Only a mover's **runtime** fields travel that way: its
    /// open/closed/pressed/spinning state and the timer driving it. Its
    /// `speed`, `wait`, `lip`, move direction, travel distance, damage,
    /// health, delay and sounds do not, for exactly the reason
    /// [`Transform`] does not (see [`TransitionState::apply`]'s own
    /// comment): those are facts about the brush the *destination* map
    /// compiled, not about the state the player left the mover in. Two
    /// maps in one chapter routinely give unrelated doors the same
    /// `targetname`, and one map's leaf sliding "down 172 units" is
    /// nonsense applied to another map's leaf that slides up, left or
    /// right — it parks a door's solid hull across the space its own map
    /// compiled it to clear.
    ///
    /// A destination entity that carries no mover component at all is left
    /// without one: inserting a foreign mover's keyvalues onto it is the
    /// same mistake in a louder form.
    pub fn apply_onto_existing(&self, registry: &mut Registry, entity: Entity) {
        let world = &mut registry.world;
        if let Some(value) = self.spawnflags {
            world.insert_one(entity, SpawnFlags(value)).ok();
        }
        if let Some(value) = self.render {
            world.insert_one(entity, value).ok();
        }
        if let Some(carried) = self.door
            && let Ok(mut door) = world.get::<&mut Door>(entity)
        {
            door.state = carried.state;
            door.timer = carried.timer;
            // `Door::rotation_axis` is the one field in this component
            // that is not purely compiled. Its *axis* is compiled (the
            // spawnflag/`distance` choice `RotatingDoorSwing::base_axis`
            // holds), but its *sign* is runtime: `ohl_game::logic`'s door
            // arm rewrites it on every closed -> opening edge so the leaf
            // swings away from whoever opened it (PR #110). Dropping the
            // whole field would mirror a carried-open leaf onto the wrong
            // side of its frame — the same "a mover's hull is parked where
            // its own map never put it" fault this method exists to stop,
            // reflected instead of translated. So the sign travels and the
            // axis does not: both doors must be rotating ones for the
            // question to mean anything, and a carried sign of zero (the
            // dot product of two perpendicular axes) leaves the
            // destination's own untouched.
            if let (Some(carried_axis), Some(own_axis)) =
                (carried.rotation_axis, door.rotation_axis)
            {
                let sign = carried_axis.dot(own_axis);
                if sign < 0.0 {
                    door.rotation_axis = Some(-own_axis);
                } else if sign > 0.0 {
                    door.rotation_axis = Some(own_axis);
                }
            }
        }
        if let Some(carried) = self.button
            && let Ok(mut button) = world.get::<&mut Button>(entity)
        {
            button.state = carried.state;
            button.timer = carried.timer;
        }
        if let Some(carried) = self.platform
            && let Ok(mut platform) = world.get::<&mut Platform>(entity)
        {
            platform.state = carried.state;
            platform.timer = carried.timer;
        }
        if let Some(carried) = self.rotator
            && let Ok(mut rotator) = world.get::<&mut Rotator>(entity)
        {
            rotator.spinning = carried.spinning;
            rotator.angle_deg = carried.angle_deg;
        }
        if let Some(value) = self.light {
            world.insert_one(entity, value).ok();
        }
        if let Some(value) = self.message.clone() {
            world.insert_one(entity, value).ok();
        }
    }

    /// Whether a mover this snapshot describes has been moved from its
    /// authored resting state, i.e. whether it is worth carrying across.
    #[must_use]
    pub fn is_modified_mover(&self) -> bool {
        let moved = |state: MoverState, timer: f32| state != MoverState::Closed || timer != 0.0;
        self.door.is_some_and(|door| moved(door.state, door.timer))
            || self
                .button
                .is_some_and(|button| moved(button.state, button.timer))
            || self
                .platform
                .is_some_and(|platform| moved(platform.state, platform.timer))
            // A `func_rotating` has no open/closed state machine: "moved"
            // for one means it is spinning, or has accumulated an angle it
            // would otherwise snap back from.
            || self
                .rotator
                .is_some_and(|rotator| rotator.spinning || rotator.angle_deg != 0.0)
    }
}

/// A `func_train`/`func_tracktrain`'s ride state, as it travels to the
/// next map alongside the entity that owns it.
///
/// The documented cross-level rule (see `docs/FORMAT_SOURCES.md`,
/// "Campaign flow": entities persist across a transition when correlated
/// by a shared `globalname`) says *that* such a train persists, and the
/// `path_track`/`path_corner` documentation says a train's route is
/// expressed as a chain of nodes named by `targetname`. So the one part of
/// a chain position that means anything in the destination map is the
/// **name** of the node the train is currently at: a node index belongs to
/// the source map's chain, and the destination's own chain routinely
/// starts somewhere else entirely. This struct therefore carries the node
/// by name plus the train's own motion, and never a node index or a world
/// position.
///
/// Kept out of [`EntitySnapshot`] deliberately: that type is tag 18 of the
/// save container and frozen at its current shape (see `crate::save`), and
/// this state is rebuilt from the destination map's own chain anyway. See
/// [`crate::save_state::TrackTrainSnapshot`] for the *save* path's own,
/// index-based record of the same runtime fields, which stays index-based
/// because a save is always reloaded into the same map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackTrainCarry {
    /// The `targetname` of the `path_track`/`path_corner` the train last
    /// departed from (or rests at).
    pub node: String,
    /// Progress from that node toward the next one in the train's
    /// direction of travel, in `0..=1`.
    pub t: f32,
    /// `1.0` travelling toward the chain's next node, `-1.0` toward its
    /// previous one.
    pub direction: f32,
    /// Speed magnitude, units/second.
    pub speed: f32,
    /// Whether the train is moving.
    pub moving: bool,
    /// Seconds left in a `path_track`'s `wait` pause.
    pub wait_timer: f32,
    /// The heading, in degrees, the source map's copy of the train was
    /// posed at, when it had one.
    ///
    /// A `func_tracktrain` faces along the segment it is on, so this is
    /// normally the destination map's own business — and it is left as
    /// such whenever the destination's chain defines a heading at all. A
    /// map that *ends* a shared ride, though, parks its own copy of the
    /// car on a single `path_track` and never moves it again: that chain
    /// has no segment anywhere in it, so the car has no heading of its own
    /// and would be posed unrotated, across the track its compiled
    /// geometry was authored along, dropping the arriving passenger
    /// through the floor of a car that is no longer under them. The only
    /// place a heading can come from then is the map the ride arrived
    /// from. Applied only as that last fallback; see
    /// `ohl_game::track_train::TrackTrainState::set_handover_yaw`.
    #[serde(default)]
    pub yaw: Option<f32>,
}

/// Where the player was sitting or standing *on a ride* at the instant a
/// level change fired, expressed in that ride's own frame.
///
/// The documented placement rule for a transition is the landmark offset
/// (`docs/FORMAT_SOURCES.md`, "Campaign flow"): the arriving player keeps
/// the offset from the destination's `info_landmark` they had from the
/// source map's own. That rule assumes the thing they were standing on is
/// in the same place relative to the landmark in both maps, which is true
/// of world geometry and false of a `func_tracktrain`: the destination's
/// copy of a shared ride is placed by its *own* `path_track` chain (see
/// [`restore_track_train`]), whose head sits wherever that map's author put
/// it and points along whatever heading that map's first segment has.
/// Applying the landmark offset to a rider therefore moves them by however
/// far the two chains disagree — measured on the campaign's own tram
/// boundaries as tens of units and around a dozen degrees, which is enough
/// to put a passenger through the car's interior wall.
///
/// So when the player's ground brush at the moment of the change *is* such
/// a ride, this project carries their seat relative to it instead, and the
/// arrival is placed from wherever the destination's own copy of it ends
/// up. The seat is recorded in the ride's frame — the offset from its posed
/// centre (`ohl_game::pose::brush_center`), turned back through the ride's
/// own yaw (`ohl_game::pose::track_train_transform`) — so a destination car
/// facing a different way seats the passenger in the same part of the car
/// rather than the same part of the world.
///
/// **Only a ride qualifies**, meaning an entity carrying a
/// [`TrackTrainState`] (`func_train`/`func_tracktrain`). That is not a
/// convenience restriction: a train is the one brush entity whose placement
/// comes from a `path_track` chain rather than from where its geometry was
/// compiled. Every other mover — a `func_door`, `func_plat`, `func_wall` —
/// is placed by its own compiled bounds plus its `origin` keyvalue in the
/// destination map's own coordinates, which is exactly what the landmark
/// offset already agrees with, so measuring against one instead would
/// replace a rule that works with one that merely happens to. A player
/// standing on any of those keeps the documented offset.
///
/// Project-determined, `TODO(black-box)`: no public page states what an
/// engine does with a passenger aboard a mover at a level change. It is a
/// *precedence* rule over the documented offset, not a replacement — a
/// player standing on world geometry, on a non-ride brush entity, or on a
/// ride the destination map does not declare, is placed by the landmark
/// offset exactly as before. Recorded in `docs/FORMAT_SOURCES.md` under
/// "Riding movers".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiderSeat {
    /// The ridden ride's `globalname`, the documented cross-level
    /// correlation key, when it has one.
    pub globalname: Option<String>,
    /// The ridden entity's `targetname`, used when it has no `globalname`.
    pub targetname: Option<String>,
    /// The player's origin minus the mover's posed centre, turned back
    /// through the mover's own yaw so it is expressed in the mover's frame.
    pub seat: [f32; 3],
    /// The heading, in degrees, the ridden mover itself was posed at.
    ///
    /// Needed because the destination's copy of a ride that *ends* there
    /// is parked on a single `path_track` and so has no segment to take a
    /// heading from at all (see
    /// `ohl_game::track_train::TrackTrainState::set_handover_yaw`): it
    /// would be posed unrotated, across the track its geometry was
    /// authored along, and the seat computed against it would be nowhere
    /// near its floor. The car a passenger arrives on therefore faces the
    /// way it faced when they boarded it, and only when its own chain
    /// cannot say otherwise.
    #[serde(default)]
    pub yaw: Option<f32>,
}

/// One entity travelling to the next map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CarriedEntity {
    /// `classname`, so an entity with no counterpart in the destination can
    /// still be re-created there.
    pub classname: String,
    /// `targetname`, when it has one.
    pub targetname: Option<String>,
    /// `globalname`, the documented cross-level correlation key.
    pub globalname: Option<String>,
    /// `target`, when it has one.
    pub target: Option<String>,
    /// Position relative to the landmark in the *source* map, or `None`
    /// when that map declared no such landmark: an entity with no
    /// counterpart in the destination then has no place to be put, and is
    /// dropped rather than materialised at an arbitrary position.
    pub offset: Option<[f32; 3]>,
    /// Component state.
    pub snapshot: EntitySnapshot,
    /// The ride state of a `func_train`/`func_tracktrain`, when this entity
    /// is one that was following a path. Applied only through the
    /// `globalname` correlation; see [`TrackTrainCarry`].
    pub track_train: Option<TrackTrainCarry>,
    /// The source map's own keyvalues for this entity, bounded by
    /// [`MAX_CARRIED_KEYVALUES`].
    ///
    /// Carried for the one case that has no destination data to fall back
    /// on: an entity the destination map declares no counterpart for, which
    /// [`TransitionState::place`] re-creates there. Re-creating it from a
    /// classname and a position alone produced an entity the destination's
    /// own spawn pipeline had never heard of — it is not in
    /// `ohl_engine::level::Level::defs`, so `AiState::attach_level` never
    /// gives a carried monster its brain, its `Actor`, or a place in the
    /// destination's own `scripted_sequence` bookkeeping, and a destination
    /// whose scripts are written around that monster waits for it forever.
    /// With its keyvalues the transition can append a real
    /// `ohl_game::keyvalues::EntityDef` and let the destination build it
    /// exactly like one of its own.
    #[serde(default)]
    pub keyvalues: Vec<(String, String)>,
}

/// One named mover's state, carried so the previous map's doors and buttons
/// are still open/pressed if the player walks back into them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoverSnapshot {
    /// The mover's `targetname`.
    pub targetname: String,
    /// Its component state.
    pub snapshot: EntitySnapshot,
}

/// The `globalname`/`env_global` state table: named variables that are off,
/// on, or dead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalStateTable {
    entries: BTreeMap<String, GlobalStateValue>,
}

impl GlobalStateTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// This variable's value, or [`GlobalStateValue::Off`] when it was
    /// never set.
    #[must_use]
    pub fn get(&self, name: &str) -> GlobalStateValue {
        self.entries.get(name).copied().unwrap_or_default()
    }

    /// Sets one variable.
    pub fn set(&mut self, name: impl Into<String>, value: GlobalStateValue) {
        self.entries.insert(name.into(), value);
    }

    /// Whether `name` has been retired.
    #[must_use]
    pub fn is_dead(&self, name: &str) -> bool {
        self.get(name) == GlobalStateValue::Dead
    }

    /// Every `(name, value)` pair, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, GlobalStateValue)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
    }

    /// How many variables the table holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Seeds every `env_global` in `registry` that asks for its initial
    /// state to be applied, leaving variables this table already knows
    /// untouched (a returning map must not reset a global the player
    /// already changed).
    pub fn seed_from(&mut self, registry: &Registry) {
        for global in &mut registry.world.query::<&EnvGlobal>() {
            if !global.sets_initial_state || global.global_state.is_empty() {
                continue;
            }
            self.entries
                .entry(global.global_state.clone())
                .or_insert(global.initial_state);
        }
    }
}

/// Everything one level transition carries into the destination map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TransitionState {
    /// The `info_landmark` name both maps share.
    pub landmark: String,
    /// The player's position relative to the source map's landmark, or
    /// `None` when that map declared no such landmark. A transition with no
    /// offset leaves the player at the destination's own
    /// `info_player_start`, since an absolute source position means nothing
    /// in the destination's coordinates.
    pub player_offset: Option<[f32; 3]>,
    /// The player's yaw, in degrees.
    pub yaw: f32,
    /// The player's pitch, in degrees.
    pub pitch: f32,
    /// The player's own carried state.
    pub player: PlayerCarryState,
    /// Entities eligible to travel.
    pub entities: Vec<CarriedEntity>,
    /// Global variables.
    pub globals: GlobalStateTable,
    /// The source map's modified door/button/platform states.
    pub movers: Vec<MoverSnapshot>,
    /// Where the player was standing on a carried mover, when they were.
    /// Takes precedence over [`Self::player_offset`]; see [`RiderSeat`].
    #[serde(default)]
    pub rider: Option<RiderSeat>,
}

/// Whether `entity` is one of the entities a transition never carries: the
/// map's own structural markers, which the destination map declares itself.
fn is_structural(registry: &Registry, entity: Entity, classname: &str) -> bool {
    registry.world.get::<&Landmark>(entity).is_ok()
        || registry.world.get::<&TransitionVolume>(entity).is_ok()
        || matches!(
            classname,
            "worldspawn" | "info_player_start" | "trigger_changelevel"
        )
}

/// An entity's world-space position, preferring a brush entity's own
/// placed bounding-box centre (`ohl_game::pose::brush_center`: the
/// compiled bounds' midpoint, plus the `origin` keyvalue, plus however far
/// its state machine has since moved it) over its `Transform::origin`,
/// which for a brush entity compiled in absolute world space is
/// conventionally zero.
fn entity_position(registry: &Registry, entity: Entity) -> Option<Vec3> {
    if let Some(center) = ohl_game::pose::brush_center(registry, entity) {
        return Some(center);
    }
    registry
        .world
        .get::<&Transform>(entity)
        .ok()
        .map(|transform| transform.origin)
}

/// The ride state of `entity`, when it is a `func_train`/`func_tracktrain`
/// that resolved a path chain, with the node it is at recorded by name.
/// `None` for any other entity, and for a train whose current node carries
/// no `targetname` (nothing in the destination could then be correlated
/// with it).
fn capture_track_train(registry: &Registry, entity: Entity) -> Option<TrackTrainCarry> {
    let state = registry.world.get::<&TrackTrainState>(entity).ok()?;
    let yaw = registry
        .world
        .get::<&TrackTrain>(entity)
        .ok()
        .and_then(|train| state.yaw_degrees(&train));
    let (node_index, t, direction, speed, moving, wait_timer) = state.dynamic_state();
    let node_entity = state.chain().nodes.get(node_index)?.entity;
    let node = registry
        .world
        .get::<&TargetName>(node_entity)
        .ok()?
        .0
        .clone();
    Some(TrackTrainCarry {
        node,
        t,
        direction,
        speed,
        moving,
        wait_timer,
        yaw,
    })
}

/// Puts the destination map's copy of a carried train back where the
/// source map's copy was, and moving the same way.
///
/// Two maps that share one ride share the *node names* along it — that is
/// what makes a chain expressible at all (`path_track`'s documented
/// `target`), and it is the only correlation a destination map offers for
/// a position along a track. So:
///
/// - When the destination train's own chain already contains a node of the
///   carried name, the train is simply re-seated on it, and keeps that
///   chain (so a train travelling backward still has the nodes behind it).
/// - Otherwise the chain is rebuilt from the carried node's own name, the
///   same way [`ohl_game::track_train::spawn_all`] builds one from the
///   train's `target`: the destination's copy of the node the train is at
///   leads onward exactly as the source's copy did.
///
/// A train that reaches neither — no `TrackTrain` keyvalues, or a node
/// name the destination map does not declare — is left exactly as the
/// destination map spawned it rather than placed at a guessed position.
///
/// A train travelling backward onto a *rebuilt* chain has no node behind
/// it (a rebuilt chain starts at the train's own node) and so comes to
/// rest there; a map that hands a reversing train across a level change
/// would need the destination to declare the nodes behind it, which is
/// exactly the first case above.
fn restore_track_train(registry: &mut Registry, entity: Entity, carry: &TrackTrainCarry) {
    let Some(train) = registry
        .world
        .get::<&TrackTrain>(entity)
        .ok()
        .map(|train| *train)
    else {
        return;
    };
    let existing = registry
        .world
        .get::<&TrackTrainState>(entity)
        .ok()
        .map(|state| TrackTrainState::clone(&state));
    let seated = existing.clone().and_then(|state| {
        let index = state.chain().nodes.iter().position(|node| {
            registry
                .world
                .get::<&TargetName>(node.entity)
                .is_ok_and(|name| name.0 == carry.node)
        })?;
        Some((state, index))
    });
    // `(state, index, t)`: which chain the destination's train ends up on,
    // and where along it. The third case keeps the destination's *own*
    // placement, so it keeps that placement's own `t` too.
    let placed = if let Some((state, index)) = seated {
        Some((state, index, carry.t))
    } else if let Some(chain) = PathChain::build(registry, &carry.node, train.height) {
        Some((TrackTrainState::spawn(&train, chain), 0, carry.t))
    } else {
        // The destination map declares no node of the carried name at all,
        // so there is nothing to correlate a *position* with — but the
        // train's own motion is not a position, and discarding it left the
        // destination's copy running on its `startspeed` as if the ride
        // that arrived had never happened. Keep the destination's own
        // chain and place along it, and carry the ride: a map that parks
        // its tram exactly at the boundary and has the *next* map start it
        // again (`trigger_auto`'s documented `triggerstate`) then gets the
        // parked train it was authored around instead of one already
        // rolling at a speed no keyvalue in either map asked for.
        existing.map(|state| {
            let (index, t, ..) = state.dynamic_state();
            (state, index, t)
        })
    };
    let Some((mut state, index, t)) = placed else {
        return;
    };
    state.restore_dynamic_state(
        index,
        t,
        carry.direction,
        carry.speed,
        carry.moving,
        carry.wait_timer,
    );
    // Only as a last fallback: `yaw_degrees` uses it exactly when the
    // destination's own chain defines no heading anywhere. See
    // [`TrackTrainCarry::yaw`].
    state.set_handover_yaw(carry.yaw);
    registry.world.insert_one(entity, state).ok();
}

/// The yaw, in degrees, `ohl_game::pose::brush_pose_rotation` poses
/// `entity`'s collision hull and drawn geometry at — a `func_tracktrain`'s
/// *pose*, which is its segment heading turned by
/// `ohl_game::track_train::COMPILED_FACING_OFFSET_DEGREES`, and `0.0` for
/// any mover that carries no yaw of its own.
///
/// The *pose* is the one number a seat has to be expressed against, so that
/// a destination map's copy of a ride, pointing a different way, still
/// seats a passenger in the same part of the car: it is the frame the car's
/// geometry — and so its floor — is actually drawn and collided in.
/// Reading the bare direction of travel here
/// (`TrackTrainState::travel_heading_degrees`) instead would seat a
/// passenger half a car-length out.
fn mover_yaw_degrees(registry: &Registry, entity: Entity) -> f32 {
    ohl_game::pose::track_train_transform(registry, entity)
        .1
        .unwrap_or(0.0)
}

/// `offset` turned about the world up axis by `degrees`, matching
/// `ohl_game::registry::movedir_from_angles`' convention (the same one
/// `TrackTrainState::yaw_degrees` reports in).
fn turn_about_z(offset: Vec3, degrees: f32) -> Vec3 {
    glam::Quat::from_rotation_z(degrees.to_radians()) * offset
}

/// `origin` expressed in `entity`'s own posed frame: the offset from its
/// centre, turned back through its yaw. `None` for an entity with no brush
/// centre (a point entity, or one whose submodel bounds were unavailable).
fn seat_in_mover_frame(registry: &Registry, entity: Entity, origin: Vec3) -> Option<Vec3> {
    let center = ohl_game::pose::brush_center(registry, entity)?;
    Some(turn_about_z(
        origin - center,
        -mover_yaw_degrees(registry, entity),
    ))
}

/// The inverse of [`seat_in_mover_frame`]: where a seat recorded in one
/// map's copy of a mover lands on another map's copy of it.
fn seat_to_world(registry: &Registry, entity: Entity, seat: Vec3) -> Option<Vec3> {
    let center = ohl_game::pose::brush_center(registry, entity)?;
    Some(center + turn_about_z(seat, mover_yaw_degrees(registry, entity)))
}

/// The `trigger_transition` volumes named after `landmark`.
fn transition_volumes(registry: &Registry, landmark: &str) -> Vec<BrushBounds> {
    let mut volumes = Vec::new();
    for (name, bounds) in &mut registry
        .world
        .query::<(&TargetName, &BrushBounds)>()
        .with::<&TransitionVolume>()
    {
        if name.0 == landmark {
            volumes.push(*bounds);
        }
    }
    volumes
}

/// The destination-map entity definition of a carried entity the
/// destination declares no counterpart for, placed at `position` and facing
/// `angles`.
///
/// Built from the source map's own keyvalues (see
/// [`CarriedEntity::keyvalues`]) so every key the destination's spawn
/// pipeline reads — a monster's model, skin, squad, spawnflags, a script's
/// own choices — arrives with it, with only the placement rewritten into
/// the destination's coordinates. A transition captured before those
/// keyvalues travelled (an older save) still yields a usable definition
/// from the classname, names and placement alone.
fn carried_def(carried: &CarriedEntity, position: Vec3, angles: Vec3) -> EntityDef {
    let mut pairs: std::collections::BTreeMap<String, String> =
        carried.keyvalues.iter().cloned().collect();
    pairs.insert("classname".to_string(), carried.classname.clone());
    pairs.insert(
        "origin".to_string(),
        format!("{} {} {}", position.x, position.y, position.z),
    );
    pairs.insert(
        "angles".to_string(),
        format!("{} {} {}", angles.x, angles.y, angles.z),
    );
    // `angle` is the older scalar spelling of the same key and would win a
    // disagreement with the placement just written, so it is dropped.
    pairs.remove("angle");
    // A `model` keyvalue naming a *brush submodel* (`*N`) is an index into
    // the map that compiled it. The destination compiled its own submodels
    // and numbers them its own way, so carrying the index over would hand
    // the re-created entity an unrelated piece of the destination's world
    // to be collided and drawn as. A studio/sprite model path is a plain
    // asset reference and travels.
    if pairs
        .get("model")
        .is_some_and(|model| model.starts_with('*'))
    {
        pairs.remove("model");
    }
    for (key, value) in [
        ("targetname", carried.targetname.as_ref()),
        ("target", carried.target.as_ref()),
        ("globalname", carried.globalname.as_ref()),
    ] {
        match value {
            Some(value) => pairs.insert(key.to_string(), value.clone()),
            None => pairs.remove(key),
        };
    }
    ohl_game::keyvalues::parse_entity(&pairs, &KeyvalueLimits::default())
}

/// Whether `position` lies in the potentially-visible set of the landmark
/// at `origin`, or `None` when this map cannot answer the question.
///
/// The documented eligibility rule for a level change is that an entity
/// must be "inside the volume, or otherwise in the landmark's PVS"
/// (`docs/FORMAT_SOURCES.md`, "Campaign flow"). The PVS half of it is a
/// plain leaf-to-leaf query against the visibility lump this project
/// already decodes for rendering (`ohl_world::VisibilitySet`), so it is
/// answered here rather than approximated.
///
/// `None` — "ask [`DEFAULT_CARRY_RADIUS`] instead" — in the three cases
/// where a leaf query means nothing:
///
/// - the map carries no usable visibility data at all, where the set
///   answers "visible" for every pair and would carry every named entity
///   in the map;
/// - either point falls outside the world's node tree;
/// - either point lands in leaf `0`, the shared outside/solid leaf, which
///   has neither a visibility row nor a bit in anyone else's. A brush
///   entity's own reference point (`entity_position`, the centre of its
///   compiled brush) is normally *inside* that brush, so this is the usual
///   answer for one, and a brush entity keeps exactly the radius rule it
///   had before.
fn in_landmark_pvs(world: &ohl_world::WorldModel, origin: Vec3, position: Vec3) -> Option<bool> {
    landmark_pvs_answer(
        world.visibility(),
        world.leaf_at(origin.to_array()),
        world.leaf_at(position.to_array()),
    )
}

/// [`in_landmark_pvs`]'s decision, with the two leaf lookups already made:
/// the whole rule, and every case it declines to answer, in one pure
/// function so each is unit-testable without a world to trace against.
fn landmark_pvs_answer(
    vis: &ohl_world::VisibilitySet,
    from: Option<usize>,
    to: Option<usize>,
) -> Option<bool> {
    if !vis.is_decoded() {
        return None;
    }
    let (from, to) = (from?, to?);
    if from == 0 || to == 0 {
        return None;
    }
    Some(vis.is_visible(from, to))
}

impl TransitionState {
    /// Captures what travels from `level` through the landmark named
    /// `landmark`.
    ///
    /// `eye` is the player's current position, and `globals` the game's
    /// current global state table (already seeded from this level).
    ///
    /// When `level` declares no `info_landmark` named `landmark` there is
    /// nothing to measure against, so every offset is captured as `None`
    /// and the destination falls back to its own `info_player_start`. State
    /// that needs no placement (mover states, globals, and entities the
    /// destination correlates by `globalname`/`targetname`) still travels.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "every argument is one independent piece of what a level \
                  change carries; bundling them into a struct would only move \
                  the same list to the call site"
    )]
    pub(crate) fn capture(
        level: &Level,
        landmark: &str,
        eye: Vec3,
        yaw: f32,
        pitch: f32,
        player: PlayerCarryState,
        globals: &GlobalStateTable,
        ridden: Option<Entity>,
    ) -> Self {
        let origin = level.landmark_origin(landmark);
        let registry = &level.registry;
        let volumes = transition_volumes(registry, landmark);

        let mut entities = Vec::new();
        let mut movers = Vec::new();
        // The ride the player was standing on at the instant of the
        // change, if they were standing on one; see [`RiderSeat`] for what
        // qualifies and why, and the capture below for why it is recorded
        // ahead of the entity-eligibility rules rather than under them.
        let mut rider = None;
        for (index, entity) in registry.entities.iter().enumerate() {
            let entity = *entity;
            let Some(classname) = registry
                .world
                .get::<&ClassName>(entity)
                .ok()
                .map(|name| name.0.clone())
            else {
                continue;
            };
            if is_structural(registry, entity, &classname) {
                continue;
            }
            let targetname = registry
                .world
                .get::<&TargetName>(entity)
                .ok()
                .map(|name| name.0.clone());
            let globalname = registry
                .world
                .get::<&GlobalName>(entity)
                .ok()
                .map(|name| name.0.clone());
            let snapshot = EntitySnapshot::capture(registry, entity);

            // The player's own seat, recorded *before* the eligibility
            // rules below: those decide which entities travel, and this is
            // not one — it is part of the player's own placement, and the
            // player always travels. All the destination needs to
            // reproduce it is a counterpart of the same name. (The ride
            // this campaign's last boundary hands over sits well outside
            // the landmark's carry radius, so gating the seat on
            // eligibility would lose exactly the case it exists for.)
            //
            // Restricted to a *ride* — an entity with a
            // [`TrackTrainState`], i.e. a `func_train`/`func_tracktrain` —
            // and not to any named brush the player happens to stand on;
            // see [`RiderSeat`] for why that is the whole of the rule's
            // justification.
            if ridden == Some(entity)
                && registry.world.get::<&TrackTrainState>(entity).is_ok()
                && (targetname.is_some() || globalname.is_some())
                && let Some(seat) = seat_in_mover_frame(registry, entity, eye)
            {
                rider = Some(RiderSeat {
                    globalname: globalname.clone(),
                    targetname: targetname.clone(),
                    seat: seat.to_array(),
                    yaw: Some(mover_yaw_degrees(registry, entity)),
                });
            }

            if let Some(name) = targetname.clone()
                && snapshot.is_modified_mover()
            {
                movers.push(MoverSnapshot {
                    targetname: name,
                    snapshot: snapshot.clone(),
                });
            }

            // Only a named or globally correlated entity travels: an
            // anonymous entity has no counterpart to correlate with.
            if targetname.is_none() && globalname.is_none() {
                continue;
            }
            let Some(position) = entity_position(registry, entity) else {
                continue;
            };
            let eligible = if volumes.is_empty() {
                // With no transition volume and no landmark to measure
                // from, the documented eligibility rule cannot be applied
                // at all; only an entity the destination correlates by name
                // travels, so the radius test is skipped rather than
                // measured against an invented origin.
                //
                // With a landmark, the documented rule is "inside the
                // volume, or otherwise in the landmark's PVS"; the radius
                // is only this project's stand-in for the half of it this
                // engine could not run. It can run it now (see
                // [`in_landmark_pvs`]), so the PVS answer *is* the rule
                // whenever there is one, and the stand-in answers only for
                // the entities that test declines to — not as an `or` over
                // the top of it, which would carry an entity the PVS test
                // had already answered "no" for.
                origin.is_none_or(|origin| {
                    in_landmark_pvs(&level.world, origin, position)
                        .unwrap_or_else(|| position.distance(origin) <= DEFAULT_CARRY_RADIUS)
                })
            } else {
                volumes.iter().any(|volume| volume.contains(position))
            };
            if !eligible || entities.len() >= MAX_CARRIED_ENTITIES {
                continue;
            }
            entities.push(CarriedEntity {
                classname,
                targetname,
                globalname,
                target: registry
                    .world
                    .get::<&Target>(entity)
                    .ok()
                    .map(|target| target.0.clone()),
                offset: origin.map(|origin| (position - origin).to_array()),
                snapshot,
                track_train: capture_track_train(registry, entity),
                keyvalues: level.defs.get(index).map_or_else(Vec::new, |def| {
                    def.keyvalues
                        .iter()
                        .take(MAX_CARRIED_KEYVALUES)
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect()
                }),
            });
        }

        Self {
            landmark: landmark.to_string(),
            player_offset: origin.map(|origin| (eye - origin).to_array()),
            yaw,
            pitch,
            player,
            entities,
            globals: globals.clone(),
            movers,
            rider,
        }
    }

    /// Applies this state to a freshly loaded `level`, returning the world
    /// position the player should be placed at when *both* maps declare the
    /// landmark, and `None` when either does not (the caller then leaves
    /// the player at the destination's own `info_player_start`).
    ///
    /// A destination whose `worldspawn` sets `newunit` discards everything
    /// but the player's own placement, per the documented meaning of that
    /// key.
    pub(crate) fn apply(&self, level: &mut Level) -> Option<Vec3> {
        let origin = level.landmark_origin(&self.landmark);
        if level
            .registry
            .worldspawn
            .as_ref()
            .is_some_and(|worldspawn| worldspawn.newunit)
        {
            return self.player_position(origin);
        }

        for mover in &self.movers {
            for entity in level.registry.find(&mover.targetname).to_vec() {
                // A mover keeps the destination map's own placement — and
                // the rest of its own compiled keyvalues with it: only its
                // state travels. See
                // [`EntitySnapshot::apply_onto_existing`].
                mover
                    .snapshot
                    .apply_onto_existing(&mut level.registry, entity);
            }
        }

        for carried in &self.entities {
            Self::place(level, carried, origin);
        }

        // A `globalname` the table has retired is removed from the map, the
        // documented purpose of the "dead" state.
        let mut dead: Vec<Entity> = Vec::new();
        for (entity, name) in &mut level.registry.world.query::<(Entity, &GlobalName)>() {
            if self.globals.is_dead(&name.0) {
                dead.push(entity);
            }
        }
        for entity in dead {
            level.registry.world.despawn(entity).ok();
        }

        // A rider's seat on a mover that travelled with them takes
        // precedence over the raw landmark offset; see [`RiderSeat`]. Only
        // once the destination's copy of that mover has been put where the
        // source's was (the loops above), and only when the landmark
        // placement this replaces exists at all — a boundary with no
        // landmark keeps its documented `info_player_start` fallback.
        self.rider_position(&mut level.registry, origin)
            .or_else(|| self.player_position(origin))
    }

    /// Where the arriving player's seat on a carried mover lands in the
    /// destination map, or `None` when they were not riding one, when the
    /// destination declares no counterpart for it, or when this boundary
    /// has no landmark placement for the rule to take precedence over.
    ///
    /// The counterpart is looked up by `globalname` first — the documented
    /// cross-level correlation key — and by `targetname` only when the
    /// ridden entity carried no `globalname`, which is the same order
    /// [`Self::place`] applies a carried entity's own state in.
    fn rider_position(&self, registry: &mut Registry, origin: Option<Vec3>) -> Option<Vec3> {
        if origin.is_none() || self.player_offset.is_none() {
            return None;
        }
        let rider = self.rider.as_ref()?;
        let entity = if let Some(globalname) = rider.globalname.as_ref() {
            let mut found = None;
            for (entity, name) in &mut registry.world.query::<(Entity, &GlobalName)>() {
                if &name.0 == globalname {
                    found = Some(entity);
                    break;
                }
            }
            found
        } else {
            registry.find(rider.targetname.as_deref()?).first().copied()
        }?;
        // The car a passenger arrives on faces the way it faced when they
        // boarded it, and only when its own chain cannot say otherwise —
        // otherwise the seat below is measured against a car posed across
        // its own track. See [`RiderSeat::yaw`].
        if let Ok(mut state) = registry.world.get::<&mut TrackTrainState>(entity)
            && let Ok(train) = registry.world.get::<&TrackTrain>(entity)
            && state.yaw_degrees(&train).is_none()
        {
            state.set_handover_yaw(rider.yaw);
        }
        seat_to_world(registry, entity, Vec3::from_array(rider.seat))
    }

    /// The player's world position in the destination, which needs both a
    /// captured offset and a destination landmark to exist.
    fn player_position(&self, origin: Option<Vec3>) -> Option<Vec3> {
        let offset = self.player_offset?;
        origin.map(|origin| origin + Vec3::from_array(offset))
    }

    /// Applies one carried entity: onto its `globalname` counterpart when
    /// the destination declares one, else as a new entity placed relative
    /// to the destination's landmark.
    fn place(level: &mut Level, carried: &CarriedEntity, origin: Option<Vec3>) {
        if let Some(globalname) = carried.globalname.as_ref() {
            let mut existing: Vec<Entity> = Vec::new();
            for (entity, name) in &mut level.registry.world.query::<(Entity, &GlobalName)>() {
                if &name.0 == globalname {
                    existing.push(entity);
                }
            }
            if !existing.is_empty() {
                for entity in existing {
                    carried
                        .snapshot
                        .apply_onto_existing(&mut level.registry, entity);
                    // A ride position travels only through the documented
                    // `globalname` correlation, for the same reason
                    // `transform` does not travel at all: it is a
                    // placement, and only a `globalname` says two maps mean
                    // the same entity by it.
                    if let Some(carry) = carried.track_train.as_ref() {
                        restore_track_train(&mut level.registry, entity, carry);
                    }
                }
                return;
            }
        }
        // A `targetname` the destination map already declares is the same
        // entity too: its state travels, its placement does not.
        if let Some(name) = carried.targetname.as_deref() {
            let existing = level.registry.find(name).to_vec();
            if !existing.is_empty() {
                for entity in existing {
                    carried
                        .snapshot
                        .apply_onto_existing(&mut level.registry, entity);
                }
                return;
            }
        }
        // A brush entity is never *created* from nothing in the
        // destination. The cited level-transition pages say a brush entity
        // needs "a unique global name to be able to be carried over"
        // (`docs/FORMAT_SOURCES.md`, "Campaign flow"), and a `globalname`
        // is how the destination's *own* copy of that brush is found — the
        // two branches above. There is nothing left to create here: a
        // brush entity is its submodel, the `*N` index naming it belongs to
        // the map that compiled it, and a destination that never compiled
        // one has no geometry for this entity to be. So an eligible brush
        // entity the destination does not declare applies no state and is
        // not materialised, rather than arriving as a modelless husk.
        if carried
            .keyvalues
            .iter()
            .any(|(key, value)| key == "model" && value.starts_with('*'))
        {
            return;
        }
        let (Some(origin), Some(offset)) = (origin, carried.offset) else {
            return;
        };
        let position = origin + Vec3::from_array(offset);
        let angles = carried
            .snapshot
            .transform
            .map_or(Vec3::ZERO, |transform| transform.angles);
        let Some(entity) = materialise_carried(level, &carried_def(carried, position, angles))
        else {
            return;
        };
        // The carried *state* on top of the definition. `transform` is
        // dropped: `carried_def` already wrote the destination-relative
        // placement into the definition the entity was built from, and the
        // snapshot's own is the source map's.
        let mut snapshot = carried.snapshot.clone();
        snapshot.transform = None;
        snapshot.apply(&mut level.registry, entity);
    }
}

/// Builds one entity the destination map never declared, from the entity
/// *definition* a transition (or a save) carries for it, and returns it.
///
/// A re-created entity is one of the destination map's entities from here
/// on, so it gets an entry in [`Level::defs`] as well as one in
/// `Registry::entities`: the two are index-parallel, and every later stage
/// of a level's build reads a def and writes onto the entity at its index
/// (`ohl_ai::spawn::attach_monsters`, `crate::ai::AiState`'s
/// `register_brains`/`collect_triggers`/`attach_scripts`/`attach_followers`,
/// and the navigation graph). Without one a carried monster arrives as a
/// husk: no brain, no `Actor`, no hull — and a destination map whose own
/// `scripted_sequence`s name that monster waits for an actor that can never
/// exist.
///
/// Appends nothing, and returns `None`, unless the two vectors are still
/// aligned, so this can never be what misaligns them.
///
/// Only the components `Registry::build` derives from the same four
/// keyvalues are inserted here — a re-created entity is not a brush entity
/// (`carried_def` drops a `*N` submodel `model`, whose index belongs to the
/// map that compiled it), so it needs no `BrushModel`/`BrushBounds`, and
/// every other component it should carry comes either from its
/// [`EntitySnapshot`] or from the build stages above.
pub(crate) fn materialise_carried(level: &mut Level, def: &EntityDef) -> Option<Entity> {
    if level.defs.len() != level.registry.entities.len() {
        return None;
    }
    let entity = level.registry.world.spawn((
        ClassName(def.classname.clone()),
        Transform {
            origin: Vec3::from_array(def.origin),
            angles: Vec3::from_array(def.angles),
        },
        SpawnFlags(def.spawnflags),
    ));
    if let Some(name) = def.targetname.clone() {
        level
            .registry
            .world
            .insert_one(entity, TargetName(name))
            .ok();
    }
    if let Some(target) = def.target.clone() {
        level.registry.world.insert_one(entity, Target(target)).ok();
    }
    if let Some(globalname) = def
        .keyvalues
        .get("globalname")
        .filter(|value| !value.is_empty())
    {
        level
            .registry
            .world
            .insert_one(entity, GlobalName(globalname.clone()))
            .ok();
    }
    level.defs.push(def.clone());
    level.registry.entities.push(entity);
    level.registry.index(entity, def.targetname.as_deref());
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::landmark_pvs_answer;
    use ohl_world::VisibilitySet;

    /// A set built from real rows: leaf 1 sees only itself, leaf 2 only
    /// itself. Bit `n` of a row names leaf `n + 1`, the lump's own
    /// leaf-1-based numbering.
    fn split_set() -> VisibilitySet {
        VisibilitySet::build(&[0b0000_0001, 0b0000_0010], &[-1, 0, 1]).expect("the rows decode")
    }

    /// The rule itself: the PVS answers, in both directions.
    #[test]
    fn a_decoded_set_answers_both_ways() {
        let vis = split_set();
        assert_eq!(landmark_pvs_answer(&vis, Some(1), Some(1)), Some(true));
        assert_eq!(landmark_pvs_answer(&vis, Some(1), Some(2)), Some(false));
        assert_eq!(landmark_pvs_answer(&vis, Some(2), Some(2)), Some(true));
    }

    /// The first documented fallback: a map whose visibility data this
    /// project could not materialise at all answers "visible" for every
    /// pair, which is not an answer — the caller must ask
    /// `DEFAULT_CARRY_RADIUS` instead rather than carry every named entity
    /// in the map.
    #[test]
    fn an_undecoded_set_declines_to_answer() {
        let vis = VisibilitySet::all_visible(3);
        assert_eq!(landmark_pvs_answer(&vis, Some(1), Some(2)), None);
    }

    /// The second: either point outside the world's node tree.
    #[test]
    fn a_point_outside_the_node_tree_declines_to_answer() {
        let vis = split_set();
        assert_eq!(landmark_pvs_answer(&vis, None, Some(1)), None);
        assert_eq!(landmark_pvs_answer(&vis, Some(1), None), None);
    }

    /// The third: leaf `0`, the shared outside/solid leaf, which has
    /// neither a visibility row of its own nor a bit in anyone else's — and
    /// is where a brush entity's own compiled centre normally sits.
    #[test]
    fn the_outside_leaf_declines_to_answer() {
        let vis = split_set();
        assert_eq!(landmark_pvs_answer(&vis, Some(0), Some(1)), None);
        assert_eq!(landmark_pvs_answer(&vis, Some(1), Some(0)), None);
    }
}
