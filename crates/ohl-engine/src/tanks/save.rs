//! Optional tag 44: bounded turret continuation, separate from frozen 26/42/43.
//! Registry indices identify physical sources; player identity is explicit.

use std::collections::BTreeSet;

use ohl_combat::ProjectileId;
use ohl_game::tanks::TankMemory;
use serde::{Deserialize, Serialize};

use super::runtime::{MAX_TANKS, control_candidates, sync_tank_collision, tank_entities};
use super::{
    ControlInput, Entity, MountedTank, TankControlIntent, TankDef, TankState, TankSystem, Vec3,
};
use crate::level::Level;
use crate::map_effects::SavedUseType;
use crate::projectiles::{ProjectileSystem, TankProjectileAttribution};
use crate::save_state::{MAX_SNAPSHOT_PROJECTILES, entity_at_spawn_index, spawn_index_of};

/// A save-stable identity. Registry index zero never means the local player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TankEntityRef {
    Player,
    Registry(u32),
}

/// Last visible target point and remaining simulation-time persistence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TankMemorySnapshot {
    pub point: [f32; 3],
    pub remaining: f32,
}

/// One bounded tank's mutable component, keyed by registry spawn index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TankStateSnapshot {
    pub tank: u32,
    pub active: bool,
    pub relative_pitch: f32,
    pub relative_yaw: f32,
    pub shot_wait: f64,
    pub memory: Option<TankMemorySnapshot>,
    pub rng: u64,
}

impl TankStateSnapshot {
    fn valid(&self) -> bool {
        self.relative_pitch.is_finite()
            && self.relative_pitch.abs() <= 180.0
            && self.relative_yaw.is_finite()
            && self.relative_yaw.abs() <= 180.0
            && self.shot_wait.is_finite()
            && self.shot_wait >= 0.0
            && self.rng != 0
            && self.memory.is_none_or(|memory| {
                Vec3::from_array(memory.point).is_finite()
                    && memory.remaining.is_finite()
                    && memory.remaining > 0.0
                    && memory.remaining <= 60.0
            })
    }

    fn capture(tank: u32, state: &TankState) -> Self {
        Self {
            tank,
            active: state.active,
            relative_pitch: state.relative_pitch,
            relative_yaw: state.relative_yaw,
            shot_wait: state.shot_wait,
            memory: state.memory.map(|memory| TankMemorySnapshot {
                point: memory.point.to_array(),
                remaining: memory.remaining,
            }),
            rng: state.rng,
        }
    }

    fn state(&self, def: &TankDef) -> TankState {
        let mut state = TankState {
            active: self.active,
            relative_pitch: self.relative_pitch,
            relative_yaw: self.relative_yaw,
            shot_wait: self.shot_wait,
            memory: self.memory.map(|memory| TankMemory {
                point: Vec3::from_array(memory.point),
                remaining: memory.remaining,
            }),
            rng: self.rng,
        };
        state.sanitize(def);
        state
    }
}

/// A claim is revalidated against current player, controls bounds and masters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MountedTankSnapshot {
    pub tank: u32,
    pub controls: Option<u32>,
    pub player: TankEntityRef,
    pub anchor: [f32; 3],
}

/// One deferred remote input; both runtime and phase-12 queues are preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TankIntentSnapshot {
    pub tank: u32,
    pub player: TankEntityRef,
    pub use_type: SavedUseType,
}

/// Physical source is a registry tank; credit may name the player or a registry
/// entity. A dead credited actor retains identity for already launched shots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TankProjectileSnapshot {
    pub projectile: u32,
    pub source: u32,
    pub attacker: TankEntityRef,
}

/// New optional section. No field is appended to an earlier wire shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TanksSnapshot {
    #[serde(deserialize_with = "bounded_tanks")]
    pub states: Vec<TankStateSnapshot>,
    pub mounted: Option<MountedTankSnapshot>,
    pub pending_remote: Option<TankIntentSnapshot>,
    pub pending_use: Option<TankIntentSnapshot>,
    #[serde(deserialize_with = "bounded_projectiles")]
    pub projectiles: Vec<TankProjectileSnapshot>,
}

impl TanksSnapshot {
    pub(crate) fn within_limits(&self) -> bool {
        self.states.len() <= MAX_TANKS
            && self.projectiles.len() <= MAX_SNAPSHOT_PROJECTILES
            && self.states.iter().all(TankStateSnapshot::valid)
            && self
                .mounted
                .is_none_or(|mount| Vec3::from_array(mount.anchor).is_finite())
    }
}

impl TankSystem {
    pub(crate) fn snapshot(
        &self,
        level: &Level,
        projectiles: &ProjectileSystem,
    ) -> Option<TanksSnapshot> {
        let snapshot = TanksSnapshot {
            states: tank_entities(level)
                .into_iter()
                .filter_map(|entity| {
                    let state = level.registry.world.get::<&TankState>(entity).ok()?;
                    Some(TankStateSnapshot::capture(
                        spawn_index_of(level, entity)?,
                        &state,
                    ))
                })
                .collect(),
            mounted: self.mounted().and_then(|mount| {
                Some(MountedTankSnapshot {
                    tank: spawn_index_of(level, mount.tank)?,
                    controls: match mount.controls {
                        Some(entity) => Some(spawn_index_of(level, entity)?),
                        None => None,
                    },
                    player: reference(level, mount.player)?,
                    anchor: mount.anchor.to_array(),
                })
            }),
            pending_remote: self
                .pending_remote
                .and_then(|intent| capture_intent(level, intent)),
            pending_use: level
                .simulation
                .tank_control_intent()
                .and_then(|intent| capture_intent(level, intent)),
            projectiles: projectiles
                .tank_attributions()
                .filter_map(|(id, attribution)| {
                    Some(TankProjectileSnapshot {
                        projectile: id.0,
                        source: spawn_index_of(level, attribution.source)?,
                        attacker: reference(level, attribution.attacker)?,
                    })
                })
                .take(MAX_SNAPSHOT_PROJECTILES)
                .collect(),
        };
        (snapshot != TanksSnapshot::default()).then_some(snapshot)
    }

    /// Apply after physical projectiles, tag42 and all entity/master overlays.
    /// Revalidation does not tick cadence, consume remote input or emit a shot.
    /// Returns whether a valid claim requires handheld cancellation now.
    pub(crate) fn restore(
        &mut self,
        level: &mut Level,
        projectiles: &mut ProjectileSystem,
        snapshot: Option<&TanksSnapshot>,
        input: ControlInput,
    ) -> bool {
        self.configure_level(level);
        level.simulation.restore_tank_control_intent(None);
        let eligible = tank_entities(level);
        for &entity in &eligible {
            if let Ok((def, state)) = level
                .registry
                .world
                .query_one_mut::<(&TankDef, &mut TankState)>(entity)
            {
                *state = TankState::spawn(def, entity.to_bits().get());
            }
        }
        let Some(snapshot) = snapshot else {
            projectiles.restore_tank_attributions(std::iter::empty());
            sync_tank_collision(level);
            return false;
        };
        restore_states(level, &eligible, &snapshot.states);
        restore_projectiles(level, projectiles, &snapshot.projectiles);
        self.mounted = snapshot
            .mounted
            .and_then(|mount| restore_mount(level, mount));
        let candidates = control_candidates(level, self.mounted, None, None);
        // No pending request is installed until validation finishes, so a save
        // taken after phase12 still mounts at its original next-tick boundary.
        self.resolve_controls(
            ControlInput {
                use_pressed: false,
                attack: false,
                ..input
            },
            &candidates,
            None,
        );
        self.pending_remote = snapshot
            .pending_remote
            .and_then(|intent| restore_intent(level, intent));
        let pending_use = snapshot
            .pending_use
            .and_then(|intent| restore_intent(level, intent));
        level.simulation.restore_tank_control_intent(pending_use);
        sync_tank_collision(level);
        self.mounted.is_some()
    }
}

fn restore_states(level: &mut Level, eligible: &[Entity], rows: &[TankStateSnapshot]) {
    let mut seen = BTreeSet::new();
    for row in rows.iter().take(MAX_TANKS) {
        if !seen.insert(row.tank) || !row.valid() {
            continue;
        }
        let Some(entity) =
            entity_at_spawn_index(level, row.tank).filter(|entity| eligible.contains(entity))
        else {
            continue;
        };
        if let Ok((def, state)) = level
            .registry
            .world
            .query_one_mut::<(&TankDef, &mut TankState)>(entity)
        {
            *state = row.state(def);
        }
    }
}

fn restore_projectiles(
    level: &Level,
    projectiles: &mut ProjectileSystem,
    rows: &[TankProjectileSnapshot],
) {
    let mut seen = BTreeSet::new();
    let entries = rows
        .iter()
        .take(MAX_SNAPSHOT_PROJECTILES)
        .filter_map(|row| {
            // First occurrence reserves the id even if malformed; a duplicate
            // cannot upgrade a rejected reference into a fabricated owner.
            if !seen.insert(row.projectile) {
                return None;
            }
            let source = tank(level, row.source)?;
            let attacker = resolve(level, row.attacker)?;
            Some((
                ProjectileId(row.projectile),
                TankProjectileAttribution { source, attacker },
            ))
        })
        .collect::<Vec<_>>();
    projectiles.restore_tank_attributions(entries);
}

fn restore_mount(level: &Level, mount: MountedTankSnapshot) -> Option<MountedTank> {
    if mount.player != TankEntityRef::Player || !Vec3::from_array(mount.anchor).is_finite() {
        return None;
    }
    Some(MountedTank {
        tank: tank(level, mount.tank)?,
        controls: match mount.controls {
            Some(index) => Some(resolve(level, TankEntityRef::Registry(index))?),
            None => None,
        },
        player: resolve(level, mount.player)?,
        anchor: Vec3::from_array(mount.anchor),
    })
}

fn capture_intent(level: &Level, intent: TankControlIntent) -> Option<TankIntentSnapshot> {
    Some(TankIntentSnapshot {
        tank: spawn_index_of(level, intent.tank)?,
        player: reference(level, intent.player)?,
        use_type: intent.use_type.into(),
    })
}

fn restore_intent(level: &Level, intent: TankIntentSnapshot) -> Option<TankControlIntent> {
    if intent.player != TankEntityRef::Player {
        return None;
    }
    Some(TankControlIntent {
        tank: tank(level, intent.tank)?,
        player: resolve(level, intent.player)?,
        use_type: intent.use_type.into(),
    })
}

fn tank(level: &Level, index: u32) -> Option<Entity> {
    let entity = resolve(level, TankEntityRef::Registry(index))?;
    level.registry.world.get::<&TankDef>(entity).ok()?;
    Some(entity)
}

fn reference(level: &Level, entity: Entity) -> Option<TankEntityRef> {
    if !level.registry.world.contains(entity) {
        return None;
    }
    if entity == level.player {
        Some(TankEntityRef::Player)
    } else {
        spawn_index_of(level, entity).map(TankEntityRef::Registry)
    }
}

fn resolve(level: &Level, reference: TankEntityRef) -> Option<Entity> {
    let entity = match reference {
        TankEntityRef::Player => level.player,
        TankEntityRef::Registry(index) => entity_at_spawn_index(level, index)?,
    };
    level.registry.world.contains(entity).then_some(entity)
}

fn bounded_tanks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<TankStateSnapshot>, D::Error> {
    bounded_vec(deserializer, MAX_TANKS)
}

fn bounded_projectiles<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<TankProjectileSnapshot>, D::Error> {
    bounded_vec(deserializer, MAX_SNAPSHOT_PROJECTILES)
}

fn bounded_vec<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
    max: usize,
) -> Result<Vec<T>, D::Error> {
    struct Bounded<T>(usize, std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bounded turret state")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|count| count > self.0) {
                return Err(serde::de::Error::custom("turret capacity exceeded"));
            }
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == self.0 {
                    return Err(serde::de::Error::custom("turret capacity exceeded"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded(max, std::marker::PhantomData))
}
