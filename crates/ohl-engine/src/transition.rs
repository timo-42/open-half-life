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
use ohl_game::registry::{
    BrushBounds, BrushCenter, Button, ClassName, Door, EnvGlobal, GlobalName, GlobalStateValue,
    Landmark, Light, Message, MoverState, Platform, Registry, RenderPropsComponent, SpawnFlags,
    Target, TargetName, Transform, TransitionVolume, Trigger,
};
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
    }
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

/// An entity's world-space position, preferring a brush entity's bounding
/// box centre over its (conventionally zero) `origin`.
fn entity_position(registry: &Registry, entity: Entity) -> Option<Vec3> {
    if let Ok(center) = registry.world.get::<&BrushCenter>(entity) {
        return Some(center.0);
    }
    registry
        .world
        .get::<&Transform>(entity)
        .ok()
        .map(|transform| transform.origin)
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
    pub(crate) fn capture(
        level: &Level,
        landmark: &str,
        eye: Vec3,
        yaw: f32,
        pitch: f32,
        player: PlayerCarryState,
        globals: &GlobalStateTable,
    ) -> Self {
        let origin = level.landmark_origin(landmark);
        let registry = &level.registry;
        let volumes = transition_volumes(registry, landmark);

        let mut entities = Vec::new();
        let mut movers = Vec::new();
        for entity in &registry.entities {
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
                origin.is_none_or(|origin| position.distance(origin) <= DEFAULT_CARRY_RADIUS)
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
                let mut snapshot = mover.snapshot.clone();
                // A mover keeps the destination map's own placement: only
                // its state travels.
                snapshot.transform = None;
                snapshot.apply(&mut level.registry, entity);
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

        self.player_position(origin)
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
                    let mut snapshot = carried.snapshot.clone();
                    snapshot.transform = None;
                    snapshot.apply(&mut level.registry, entity);
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
                    let mut snapshot = carried.snapshot.clone();
                    snapshot.transform = None;
                    snapshot.apply(&mut level.registry, entity);
                }
                return;
            }
        }
        let (Some(origin), Some(offset)) = (origin, carried.offset) else {
            return;
        };
        let position = origin + Vec3::from_array(offset);
        let angles = carried
            .snapshot
            .transform
            .map_or(Vec3::ZERO, |transform| transform.angles);
        let entity = level.registry.world.spawn((
            ClassName(carried.classname.clone()),
            Transform {
                origin: position,
                angles,
            },
        ));
        if let Some(name) = carried.targetname.clone() {
            level
                .registry
                .world
                .insert_one(entity, TargetName(name))
                .ok();
        }
        if let Some(target) = carried.target.clone() {
            level.registry.world.insert_one(entity, Target(target)).ok();
        }
        if let Some(globalname) = carried.globalname.clone() {
            level
                .registry
                .world
                .insert_one(entity, GlobalName(globalname))
                .ok();
        }
        let mut snapshot = carried.snapshot.clone();
        snapshot.transform = None;
        snapshot.apply(&mut level.registry, entity);
        level.registry.entities.push(entity);
        level.registry.index(entity, carried.targetname.as_deref());
    }
}
