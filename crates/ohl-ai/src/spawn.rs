//! Turning `ohl-game`'s parsed map entities into AI components.
//!
//! `ohl-game` already owns the entity registry: it parses the BSP entities
//! lump into [`EntityDef`]s and spawns one `hecs` entity per map entity, in
//! order, into [`Registry::entities`]. This module attaches the AI's own
//! components ([`Actor`], [`MonsterAi`], [`SquadTag`]) to those same
//! entities, so there is one entity world rather than two.
//!
//! The keyvalues read here are the published ones recorded in
//! `docs/FORMAT_SOURCES.md`: `origin`, `angles`/`angle`, `netname` (the
//! squad name) and the `SquadLeader` spawnflag, bit 32. Which classname is
//! which [`Classification`], and which brain each gets, is package 7.7's
//! job — hence the caller-supplied [`MonsterSpawnRules`] rather than a table
//! here.

use glam::Vec3;
use hecs::Entity;
use ohl_game::{EntityDef, Registry};

use crate::state::Classification;
use crate::world::{Actor, BrainId, MonsterAi, SquadTag};

/// The published `SquadLeader` spawnflag bit.
pub const SPAWNFLAG_SQUAD_LEADER: u32 = 32;

/// The published `netname` key that names a monster's squad.
pub const SQUAD_NAME_KEY: &str = "netname";

/// What the caller wants done with one map entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonsterSpawn {
    /// The faction to give it.
    pub classification: Classification,
    /// Which registered brain drives it.
    pub brain: BrainId,
    /// Its starting health.
    pub health: f32,
    /// Its eye offset above the origin.
    pub view_ofs: Vec3,
}

impl MonsterSpawn {
    /// A monster of the given faction and brain, with 100 health and the
    /// default eye height.
    #[must_use]
    pub fn new(classification: Classification, brain: BrainId) -> Self {
        Self {
            classification,
            brain,
            health: 100.0,
            view_ofs: Vec3::new(0.0, 0.0, 28.0),
        }
    }

    /// The same spawn with a different starting health.
    #[must_use]
    pub fn with_health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }
}

/// Decides which map entities become monsters.
///
/// Implemented by package 7.7's per-monster table; a closure works too.
pub trait MonsterSpawnRules {
    /// Returns the spawn for `def`, or `None` when it is not a monster.
    fn spawn_for(&self, def: &EntityDef) -> Option<MonsterSpawn>;
}

impl<F: Fn(&EntityDef) -> Option<MonsterSpawn>> MonsterSpawnRules for F {
    fn spawn_for(&self, def: &EntityDef) -> Option<MonsterSpawn> {
        self(def)
    }
}

/// Attaches AI components to every entity `rules` claims.
///
/// `defs` must be the same slice, in the same order, that
/// [`Registry::build`] was given, because `ohl-game` records the spawn order
/// in [`Registry::entities`]. Entities the registry did not spawn (a
/// truncated `defs`, say) are skipped rather than panicking.
///
/// Returns the entities that gained AI components, in map order.
pub fn attach_monsters(
    registry: &mut Registry,
    defs: &[EntityDef],
    rules: &impl MonsterSpawnRules,
) -> Vec<Entity> {
    let mut spawned = Vec::new();
    for (index, def) in defs.iter().enumerate() {
        let Some(&entity) = registry.entities.get(index) else {
            break;
        };
        let Some(spawn) = rules.spawn_for(def) else {
            continue;
        };
        let actor = Actor {
            classification: spawn.classification,
            origin: Vec3::from_array(def.origin),
            view_ofs: spawn.view_ofs,
            yaw: crate::movement::normalize_yaw(def.angles[1]),
            health: spawn.health,
            alive: spawn.health > 0.0,
            is_client: false,
            hull: ohl_physics::Hull::Standing,
        };
        if registry
            .world
            .insert(entity, (actor, MonsterAi::new(spawn.brain)))
            .is_err()
        {
            continue;
        }
        if let Some(tag) = squad_tag(def)
            && registry.world.insert_one(entity, tag).is_err()
        {
            continue;
        }
        spawned.push(entity);
    }
    spawned
}

/// The squad membership `def` declares, if any.
#[must_use]
pub fn squad_tag(def: &EntityDef) -> Option<SquadTag> {
    let name = def.keyvalues.get(SQUAD_NAME_KEY)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(SquadTag {
        name: name.to_string(),
        leader: def.spawnflags & SPAWNFLAG_SQUAD_LEADER != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{MonsterSpawn, attach_monsters, squad_tag};
    use crate::state::Classification;
    use crate::world::{Actor, BrainId, MonsterAi, SquadTag};
    use ohl_game::keyvalues::{Limits, RenderProps};
    use ohl_game::{EntityDef, Registry};
    use std::collections::BTreeMap;

    fn def(classname: &str, origin: [f32; 3], keys: &[(&str, &str)], spawnflags: u32) -> EntityDef {
        EntityDef {
            classname: classname.to_string(),
            keyvalues: keys
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            origin,
            angles: [0.0, 90.0, 0.0],
            targetname: None,
            target: None,
            spawnflags,
            model: None,
            render: RenderProps::default(),
        }
    }

    fn defs() -> Vec<EntityDef> {
        vec![
            def("worldspawn", [0.0; 3], &[], 0),
            def(
                "monster_human_grunt",
                [64.0, 0.0, 36.0],
                &[("netname", "squad_a")],
                super::SPAWNFLAG_SQUAD_LEADER,
            ),
            def(
                "monster_human_grunt",
                [96.0, 0.0, 36.0],
                &[("netname", "squad_a")],
                0,
            ),
            def("info_player_start", [0.0, 0.0, 36.0], &[], 0),
        ]
    }

    #[test]
    fn only_the_claimed_entities_gain_ai_components() {
        let defs = defs();
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let spawned = attach_monsters(&mut registry, &defs, &|def: &EntityDef| {
            (def.classname == "monster_human_grunt").then(|| {
                MonsterSpawn::new(Classification::HumanMilitary, BrainId(0)).with_health(50.0)
            })
        });

        assert_eq!(spawned.len(), 2);
        let actor = *registry.world.get::<&Actor>(spawned[0]).expect("component");
        assert_eq!(actor.classification, Classification::HumanMilitary);
        assert!((actor.origin.x - 64.0).abs() < 1e-4);
        assert!((actor.yaw - 90.0).abs() < 1e-4);
        assert!((actor.health - 50.0).abs() < 1e-4);
        assert!(actor.alive);
        assert!(registry.world.get::<&MonsterAi>(spawned[1]).is_ok());

        let leader = registry
            .world
            .get::<&SquadTag>(spawned[0])
            .expect("the leader is tagged");
        assert_eq!(leader.name, "squad_a");
        assert!(leader.leader);
        let follower = registry
            .world
            .get::<&SquadTag>(spawned[1])
            .expect("the follower is tagged");
        assert!(!follower.leader);

        // The worldspawn and the player start are untouched.
        assert!(registry.world.get::<&Actor>(registry.entities[0]).is_err());
        assert!(registry.world.get::<&Actor>(registry.entities[3]).is_err());
    }

    #[test]
    fn an_untagged_entity_has_no_squad() {
        let defs = defs();
        assert!(squad_tag(&defs[0]).is_none());
        assert!(squad_tag(&defs[3]).is_none());
        assert!(squad_tag(&defs[1]).is_some_and(|tag| tag.leader));
    }

    #[test]
    fn a_short_registry_stops_rather_than_panicking() {
        let defs = defs();
        let mut registry = Registry::build(&defs[..1], &BTreeMap::new(), &Limits::default());
        let spawned = attach_monsters(&mut registry, &defs, &|_: &EntityDef| {
            Some(MonsterSpawn::new(Classification::HumanMilitary, BrainId(0)))
        });
        assert_eq!(spawned.len(), 1);
    }
}
