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
//! squad name — except on a `monster_bigmomma`, whose `netname` names the
//! first node of its trail), the `SquadLeader` spawnflag, bit 32, and the
//! `Prisoner` spawnflag, bit 16. The boss and aircraft kinds read a few
//! more of their own through `crate::monsters::bosses::attach`. Which classname is which [`Classification`], and
//! which brain each gets, is package 7.7's job — hence the caller-supplied
//! [`MonsterSpawnRules`] rather than a table here.

use glam::Vec3;
use hecs::Entity;
use ohl_game::{EntityDef, Registry};

use crate::monsters::bosses;
use crate::monsters::table::{Difficulty, MonsterKind};
use crate::state::Classification;
use crate::world::{Actor, BrainId, MonsterAi, Prisoner, SquadTag};

/// The published `Prisoner` spawnflag bit, shared by every `monster_*`
/// entity page (`docs/FORMAT_SOURCES.md`, "Monster definitions"). See
/// [`Prisoner`] for what it does.
pub const SPAWNFLAG_PRISONER: u32 = 16;

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
    /// The collision hull it moves with. The standing hull by default; a
    /// species table (`crate::monsters::MonsterSpec::hull`) supplies the
    /// real one, which is also what decides whether it walks, swims or
    /// flies (`crate::movement::flies`).
    pub hull: ohl_physics::Hull,
    /// The difficulty the level runs at, for the per-kind components that
    /// scale by it (the Gonarch's node health; see
    /// `crate::monsters::bigmomma`). Defaults to [`Difficulty::Easy`], the
    /// published 1x factor.
    pub difficulty: Difficulty,
}

impl MonsterSpawn {
    /// A monster of the given faction and brain, with 100 health, the
    /// default eye height and the standing hull.
    #[must_use]
    pub fn new(classification: Classification, brain: BrainId) -> Self {
        Self {
            classification,
            brain,
            health: 100.0,
            view_ofs: crate::BodyFrame::Feet.eye_offset(ohl_physics::Hull::Standing, None),
            hull: ohl_physics::Hull::Standing,
            difficulty: Difficulty::Easy,
        }
    }

    /// The same spawn with a different starting health.
    #[must_use]
    pub fn with_health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }

    /// The same spawn with a different collision hull.
    #[must_use]
    pub fn with_hull(mut self, hull: ohl_physics::Hull) -> Self {
        self.hull = hull;
        self.view_ofs = crate::BodyFrame::Feet.eye_offset(hull, None);
        self
    }

    /// The same spawn with a different eye offset.
    #[must_use]
    pub fn with_view_ofs(mut self, view_ofs: Vec3) -> Self {
        self.view_ofs = view_ofs;
        self
    }

    /// The same spawn at a different difficulty.
    #[must_use]
    pub fn with_difficulty(mut self, difficulty: Difficulty) -> Self {
        self.difficulty = difficulty;
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
/// A kind with a boss or aircraft component of its own (see
/// [`crate::monsters::bosses::attach`]) gets it here too, built from the
/// same `defs` and `registry`; for the one kind whose `netname` names a
/// trail node rather than a squad (`monster_bigmomma`), no [`SquadTag`] is
/// attached.
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
            hull: spawn.hull,
            body_frame: crate::BodyFrame::for_model(
                &MonsterKind::from_classname(&def.classname),
                spawn.hull,
                None,
            ),
        };
        if registry
            .world
            .insert(entity, (actor, MonsterAi::new(spawn.brain)))
            .is_err()
        {
            continue;
        }
        let kind = MonsterKind::from_classname(&def.classname);
        if !bosses::netname_is_not_a_squad(&kind)
            && let Some(tag) = squad_tag(def)
            && registry.world.insert_one(entity, tag).is_err()
        {
            continue;
        }
        if is_prisoner(def) && registry.world.insert_one(entity, Prisoner).is_err() {
            continue;
        }
        bosses::attach(registry, entity, def, defs, spawn.difficulty);
        spawned.push(entity);
    }
    spawned
}

/// Whether `def` carries the published `Prisoner` spawnflag
/// ([`SPAWNFLAG_PRISONER`]).
#[must_use]
pub fn is_prisoner(def: &EntityDef) -> bool {
    def.spawnflags & SPAWNFLAG_PRISONER != 0
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
    use crate::world::{Actor, BrainId, MonsterAi, Prisoner, SquadTag};
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

    /// The published `Prisoner` bit becomes the [`Prisoner`] marker, and
    /// only on the entity that carries it.
    #[test]
    fn the_prisoner_spawnflag_marks_the_monster() {
        let mut defs = defs();
        defs[2].spawnflags |= super::SPAWNFLAG_PRISONER;
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let spawned = attach_monsters(&mut registry, &defs, &|def: &EntityDef| {
            (def.classname == "monster_human_grunt")
                .then(|| MonsterSpawn::new(Classification::HumanMilitary, BrainId(0)))
        });
        assert_eq!(spawned.len(), 2);
        assert!(registry.world.get::<&Prisoner>(spawned[0]).is_err());
        assert!(registry.world.get::<&Prisoner>(spawned[1]).is_ok());
        assert!(!super::is_prisoner(&defs[1]));
        assert!(super::is_prisoner(&defs[2]));
    }

    /// The hull and eye offset a spawn asks for reach the actor, so a
    /// species table's hull (the flier's point hull, the barnacle's
    /// downward eye) is what the monster actually moves and looks with.
    #[test]
    fn a_spawn_carries_its_hull_and_eye_offset_onto_the_actor() {
        let defs = defs();
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let spawned = attach_monsters(&mut registry, &defs, &|def: &EntityDef| {
            (def.classname == "monster_human_grunt").then(|| {
                MonsterSpawn::new(Classification::HumanMilitary, BrainId(0))
                    .with_hull(ohl_physics::Hull::Point)
                    .with_view_ofs(glam::Vec3::new(0.0, 0.0, -16.0))
            })
        });
        let actor = *registry.world.get::<&Actor>(spawned[0]).expect("component");
        assert_eq!(actor.hull, ohl_physics::Hull::Point);
        assert!((actor.view_ofs.z + 16.0).abs() < 1e-4);
        // The default is still the standing hull at the default eye height.
        let default = MonsterSpawn::new(Classification::HumanMilitary, BrainId(0));
        assert_eq!(default.hull, ohl_physics::Hull::Standing);
        assert!(default.view_ofs.z > 0.0);
    }

    #[test]
    fn an_untagged_entity_has_no_squad() {
        let defs = defs();
        assert!(squad_tag(&defs[0]).is_none());
        assert!(squad_tag(&defs[3]).is_none());
        assert!(squad_tag(&defs[1]).is_some_and(|tag| tag.leader));
    }

    /// Wave 1 batch B: a Gonarch's `netname` is its trail's first node,
    /// not a squad, and an aircraft's `target` is its flight route.
    #[test]
    fn boss_and_aircraft_kinds_get_their_components_at_spawn() {
        use crate::monsters::bigmomma::TrailPhase;
        use crate::monsters::table::Difficulty;
        use crate::monsters::{FlightPlan, GonarchTrail, NihilanthShield};
        use ohl_game::registry::MonsterActivation;

        let defs = vec![
            def("worldspawn", [0.0; 3], &[], 0),
            def(
                "monster_bigmomma",
                [0.0, 0.0, 0.0],
                &[("netname", "trail_1")],
                0,
            ),
            {
                let mut node = def("info_bigmomma", [256.0, 0.0, 0.0], &[("health", "100")], 0);
                node.targetname = Some("trail_1".to_string());
                node
            },
            {
                let mut aircraft = def(
                    "monster_osprey",
                    [0.0, 0.0, 512.0],
                    &[("target", "route_1")],
                    0,
                );
                aircraft.target = Some("route_1".to_string());
                aircraft
            },
            {
                let mut corner = def(
                    "path_corner",
                    [512.0, 0.0, 512.0],
                    &[("target", "route_1")],
                    0,
                );
                corner.targetname = Some("route_1".to_string());
                corner.target = Some("route_1".to_string());
                corner
            },
            def("monster_nihilanth", [0.0, 0.0, 1_024.0], &[], 0),
        ];
        let mut registry = Registry::build(&defs, &BTreeMap::new(), &Limits::default());
        let spawned = attach_monsters(&mut registry, &defs, &|def: &EntityDef| {
            def.classname.starts_with("monster_").then(|| {
                MonsterSpawn::new(Classification::AlienMonster, BrainId(0))
                    .with_health(300.0)
                    .with_difficulty(Difficulty::Hard)
            })
        });
        assert_eq!(spawned.len(), 3);
        let (gonarch, osprey, boss) = (spawned[0], spawned[1], spawned[2]);

        assert!(
            registry.world.get::<&SquadTag>(gonarch).is_err(),
            "the Gonarch's netname is not a squad"
        );
        let trail = registry.world.get::<&GonarchTrail>(gonarch).expect("trail");
        assert_eq!(trail.trail().len(), 1);
        assert_eq!(trail.phase(), TrailPhase::Traveling { to: 0 });
        drop(trail);

        let plan = registry.world.get::<&FlightPlan>(osprey).expect("plan");
        assert_eq!(plan.waypoints().len(), 1);
        assert!(plan.is_looped());
        drop(plan);
        for waiting in [gonarch, osprey, boss] {
            assert!(
                registry.world.get::<&MonsterActivation>(waiting).is_ok(),
                "a use can reach it"
            );
        }

        let shield = registry
            .world
            .get::<&NihilanthShield>(boss)
            .expect("shield");
        assert!((shield.reserve_capacity() - 300.0).abs() < 1e-4);
        assert!(!shield.is_active());
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
