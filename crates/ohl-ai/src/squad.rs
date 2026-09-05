//! Squads: a named leader that recruits nearby monsters and shares an enemy.
//!
//! Published behaviour (`docs/FORMAT_SOURCES.md`, "Monster AI behaviour"):
//! monsters are grouped by the `netname` keyvalue, one member is the squad
//! leader (the `SquadLeader` spawnflag), and a leader recruits **up to three**
//! members, so a full squad is four monsters. Recruits beyond that stay
//! unsquadded. The bookkeeping below — the roster type, the deterministic
//! ordering, the enemy-sharing call — is this project's own.

use hecs::Entity;
use std::collections::BTreeMap;

use glam::Vec3;

/// The largest number of members a leader recruits, excluding itself.
pub const MAX_RECRUITS: usize = 3;

/// The largest squad size, leader included.
pub const MAX_SQUAD_SIZE: usize = MAX_RECRUITS + 1;

/// The largest number of distinct squad names a roster tracks, so a
/// pathological map cannot grow the roster without bound.
pub const MAX_SQUADS: usize = 256;

/// One monster offering itself to a squad.
#[derive(Debug, Clone, PartialEq)]
pub struct SquadCandidate {
    /// The monster.
    pub entity: Entity,
    /// Its `netname`.
    pub squad_name: String,
    /// Whether it carries the `SquadLeader` spawnflag.
    pub is_leader: bool,
}

impl SquadCandidate {
    /// A plain member.
    #[must_use]
    pub fn member(entity: Entity, squad_name: impl Into<String>) -> Self {
        Self {
            entity,
            squad_name: squad_name.into(),
            is_leader: false,
        }
    }

    /// A leader.
    #[must_use]
    pub fn leader(entity: Entity, squad_name: impl Into<String>) -> Self {
        Self {
            is_leader: true,
            ..Self::member(entity, squad_name)
        }
    }
}

/// One squad.
#[derive(Debug, Clone, PartialEq)]
pub struct Squad {
    /// The `netname` the squad is keyed by.
    pub name: String,
    /// The leader, always also the first entry of [`Self::members`].
    pub leader: Entity,
    /// Every member, leader first, at most [`MAX_SQUAD_SIZE`] long.
    pub members: Vec<Entity>,
    /// The enemy the squad currently shares, if any.
    pub enemy: Option<Entity>,
    /// Where that enemy was last reported, if any.
    pub enemy_position: Option<Vec3>,
}

impl Squad {
    /// Whether `entity` belongs to this squad.
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.members.contains(&entity)
    }

    /// The number of members the leader has recruited.
    #[must_use]
    pub fn recruit_count(&self) -> usize {
        self.members.len().saturating_sub(1)
    }

    /// Whether the leader can still recruit.
    #[must_use]
    pub fn has_room(&self) -> bool {
        self.members.len() < MAX_SQUAD_SIZE
    }
}

/// Every squad on the map, keyed by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SquadRoster {
    squads: Vec<Squad>,
    by_name: BTreeMap<String, usize>,
    rejected: Vec<Entity>,
}

impl SquadRoster {
    /// An empty roster.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a roster from `candidates`.
    ///
    /// Candidates are considered in the order given, which the caller sorts
    /// by entity id, so the same map always produces the same squads. Within
    /// a name the leader is whichever candidate declares itself one first,
    /// or the first candidate when none does. Candidates a full leader
    /// cannot recruit are recorded by [`Self::rejected`] and belong to no
    /// squad; candidates with an empty name are skipped entirely.
    #[must_use]
    pub fn build(candidates: &[SquadCandidate]) -> Self {
        let mut roster = Self::new();
        // Leaders first, so a squad always forms around a declared leader
        // even when it is not the lowest-numbered entity.
        for candidate in candidates.iter().filter(|c| c.is_leader) {
            roster.enlist(candidate);
        }
        for candidate in candidates.iter().filter(|c| !c.is_leader) {
            roster.enlist(candidate);
        }
        roster
    }

    fn enlist(&mut self, candidate: &SquadCandidate) {
        if candidate.squad_name.is_empty() {
            return;
        }
        if let Some(&index) = self.by_name.get(&candidate.squad_name) {
            let squad = &mut self.squads[index];
            if squad.contains(candidate.entity) {
                return;
            }
            if squad.has_room() {
                squad.members.push(candidate.entity);
            } else {
                self.rejected.push(candidate.entity);
            }
            return;
        }
        if self.squads.len() >= MAX_SQUADS {
            self.rejected.push(candidate.entity);
            return;
        }
        self.by_name
            .insert(candidate.squad_name.clone(), self.squads.len());
        self.squads.push(Squad {
            name: candidate.squad_name.clone(),
            leader: candidate.entity,
            members: vec![candidate.entity],
            enemy: None,
            enemy_position: None,
        });
    }

    /// Every squad, in the order they were first named.
    #[must_use]
    pub fn squads(&self) -> &[Squad] {
        &self.squads
    }

    /// Every squad, mutably, in the order they were first named.
    pub fn squads_mut(&mut self) -> &mut [Squad] {
        &mut self.squads
    }

    /// The squad called `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Squad> {
        self.by_name.get(name).map(|&index| &self.squads[index])
    }

    /// The squad `entity` belongs to.
    #[must_use]
    pub fn squad_of(&self, entity: Entity) -> Option<&Squad> {
        self.squads.iter().find(|squad| squad.contains(entity))
    }

    /// Whether `entity` is a squad leader.
    #[must_use]
    pub fn is_leader(&self, entity: Entity) -> bool {
        self.squads.iter().any(|squad| squad.leader == entity)
    }

    /// Entities that wanted a squad but could not be recruited.
    #[must_use]
    pub fn rejected(&self) -> &[Entity] {
        &self.rejected
    }

    /// Tells `entity`'s squad about an enemy, so members that have not seen
    /// it themselves can adopt it.
    ///
    /// Returns whether a squad was updated.
    pub fn share_enemy(&mut self, entity: Entity, enemy: Entity, position: Vec3) -> bool {
        let Some(index) = self.squads.iter().position(|squad| squad.contains(entity)) else {
            return false;
        };
        let squad = &mut self.squads[index];
        squad.enemy = Some(enemy);
        squad.enemy_position = Some(position);
        true
    }

    /// The enemy `entity`'s squad has reported, if any.
    #[must_use]
    pub fn shared_enemy(&self, entity: Entity) -> Option<(Entity, Vec3)> {
        let squad = self.squad_of(entity)?;
        Some((squad.enemy?, squad.enemy_position.unwrap_or(Vec3::ZERO)))
    }

    /// Forgets every shared enemy, keeping the membership.
    pub fn clear_enemies(&mut self) {
        for squad in &mut self.squads {
            squad.enemy = None;
            squad.enemy_position = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECRUITS, MAX_SQUAD_SIZE, SquadCandidate, SquadRoster};
    use glam::Vec3;
    use hecs::World;

    fn entities(count: usize) -> Vec<hecs::Entity> {
        let mut world = World::new();
        (0..count).map(|_| world.spawn((0u8,))).collect()
    }

    #[test]
    fn a_leader_recruits_at_most_three_members() {
        let ids = entities(8);
        let mut candidates = vec![SquadCandidate::leader(ids[0], "alpha")];
        candidates.extend(ids[1..].iter().map(|&e| SquadCandidate::member(e, "alpha")));

        let roster = SquadRoster::build(&candidates);
        let squad = roster.get("alpha").expect("the squad formed");
        assert_eq!(squad.leader, ids[0]);
        assert_eq!(squad.recruit_count(), MAX_RECRUITS);
        assert_eq!(squad.members.len(), MAX_SQUAD_SIZE);
        assert!(!squad.has_room());
        assert_eq!(roster.rejected().len(), 4);
        assert!(roster.is_leader(ids[0]));
        assert!(!roster.is_leader(ids[1]));
        assert!(roster.squad_of(ids[7]).is_none());
    }

    #[test]
    fn a_declared_leader_wins_over_spawn_order() {
        let ids = entities(3);
        let candidates = vec![
            SquadCandidate::member(ids[0], "bravo"),
            SquadCandidate::member(ids[1], "bravo"),
            SquadCandidate::leader(ids[2], "bravo"),
        ];
        let roster = SquadRoster::build(&candidates);
        let squad = roster.get("bravo").expect("the squad formed");
        assert_eq!(squad.leader, ids[2]);
        assert_eq!(squad.members, vec![ids[2], ids[0], ids[1]]);
    }

    #[test]
    fn a_squad_without_a_declared_leader_takes_the_first_candidate() {
        let ids = entities(2);
        let roster = SquadRoster::build(&[
            SquadCandidate::member(ids[0], "charlie"),
            SquadCandidate::member(ids[1], "charlie"),
        ]);
        assert_eq!(roster.get("charlie").expect("formed").leader, ids[0]);
    }

    #[test]
    fn unnamed_and_duplicate_candidates_are_ignored() {
        let ids = entities(2);
        let roster = SquadRoster::build(&[
            SquadCandidate::member(ids[0], ""),
            SquadCandidate::member(ids[1], "delta"),
            SquadCandidate::member(ids[1], "delta"),
        ]);
        assert_eq!(roster.squads().len(), 1);
        assert_eq!(roster.get("delta").expect("formed").members.len(), 1);
        assert!(roster.squad_of(ids[0]).is_none());
    }

    #[test]
    fn separate_names_make_separate_squads() {
        let ids = entities(4);
        let roster = SquadRoster::build(&[
            SquadCandidate::leader(ids[0], "alpha"),
            SquadCandidate::member(ids[1], "alpha"),
            SquadCandidate::leader(ids[2], "bravo"),
            SquadCandidate::member(ids[3], "bravo"),
        ]);
        assert_eq!(roster.squads().len(), 2);
        assert_eq!(roster.squad_of(ids[1]).expect("in alpha").name, "alpha");
        assert_eq!(roster.squad_of(ids[3]).expect("in bravo").name, "bravo");
    }

    #[test]
    fn an_enemy_is_shared_across_the_squad() {
        let ids = entities(4);
        let mut roster = SquadRoster::build(&[
            SquadCandidate::leader(ids[0], "alpha"),
            SquadCandidate::member(ids[1], "alpha"),
            SquadCandidate::member(ids[2], "alpha"),
        ]);
        assert!(roster.share_enemy(ids[0], ids[3], Vec3::new(1.0, 2.0, 3.0)));
        let (enemy, position) = roster.shared_enemy(ids[2]).expect("shared");
        assert_eq!(enemy, ids[3]);
        assert_eq!(position, Vec3::new(1.0, 2.0, 3.0));
        assert!(!roster.share_enemy(ids[3], ids[0], Vec3::ZERO));
        roster.clear_enemies();
        assert!(roster.shared_enemy(ids[1]).is_none());
    }
}
