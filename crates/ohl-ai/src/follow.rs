//! Talk monsters: the `monster_scientist`/`monster_barney` follow layer.
//!
//! The published behaviour is small and precise: the player `use`s a
//! security guard or a scientist to bring it into their group and `use`s it
//! again to send it away; at most two allies follow at once, and recruiting
//! a third makes one of the existing two leave; a talk monster spawned with
//! the `Pre-Disaster` spawnflag refuses to follow at all. That is what this
//! module models, and nothing more — see `docs/FORMAT_SOURCES.md`,
//! "Scripted sequences and talk monsters", for the citations and for the
//! `TODO(black-box)` list of everything the wikis leave open (follow
//! distance, catch-up speed, repath interval, which of the two allies is the
//! one that leaves).
//!
//! Following itself reuses what already exists: a [`Follower`] that is
//! following raises [`crate::Conditions::SPECIAL2`] and points the
//! monster's move target at the player, which is exactly what
//! [`crate::monsters::brains::FOLLOW_PLAYER`] — the schedule Barney and the
//! scientist already select — consumes. No second movement system, no
//! second brain.

use hecs::Entity;

/// The published `Pre-Disaster` spawnflag bit of a talk monster: it "thinks
/// the Black Mesa incident has not happened" and will not follow the player.
pub const SPAWNFLAG_TALK_PRE_DISASTER: u32 = 256;

/// The most allies that follow the player at once.
///
/// Published: "Only two friendly NPCs can follow the player at one time,
/// the player can bring along two security guards, two scientists, or one
/// of each."
pub const MAX_FOLLOWERS: usize = 2;

/// How close a follower tries to get to the player, in world units.
///
/// **`TODO(black-box)`**: no public page states a follow distance. This is
/// the same value [`crate::monsters::brains::FOLLOW_PLAYER`] already uses
/// for its `MoveToTarget` stand-off, kept in one place so observing the real
/// one later changes a single number.
pub const FOLLOW_DISTANCE: f32 = 96.0;

/// A talk monster's follow state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Follower {
    /// Whether this monster is currently in the player's group.
    pub following: bool,
    /// Whether this monster may ever follow: `false` for a `Pre-Disaster`
    /// talk monster.
    pub can_follow: bool,
}

impl Follower {
    /// A talk monster that is not following yet, reading the published
    /// `Pre-Disaster` bit out of `spawnflags`.
    #[must_use]
    pub const fn from_spawnflags(spawnflags: u32) -> Self {
        Self {
            following: false,
            can_follow: spawnflags & SPAWNFLAG_TALK_PRE_DISASTER == 0,
        }
    }
}

/// What one [`FollowRoster::toggle`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowChange {
    /// Nothing: this monster refuses to follow (`Pre-Disaster`).
    Refused,
    /// The monster joined the player's group.
    Started {
        /// The ally the published two-follower limit made leave, if any.
        evicted: Option<Entity>,
    },
    /// The monster left the player's group.
    Stopped,
}

/// Who is following the player, in the order they joined.
///
/// Bounded by [`MAX_FOLLOWERS`] and kept as a list rather than a set so the
/// eviction order — and therefore the whole simulation — is deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowRoster {
    members: Vec<Entity>,
}

impl FollowRoster {
    /// An empty roster.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            members: Vec::new(),
        }
    }

    /// The current group, oldest member first.
    #[must_use]
    pub fn members(&self) -> &[Entity] {
        &self.members
    }

    /// Whether `entity` is in the group.
    #[must_use]
    pub fn is_following(&self, entity: Entity) -> bool {
        self.members.contains(&entity)
    }

    /// Forgets everyone, for a level change.
    pub fn clear(&mut self) {
        self.members.clear();
    }

    /// Drops `entity` from the group if it was in it, reporting whether it
    /// was. Used when a follower dies or is despawned.
    pub fn remove(&mut self, entity: Entity) -> bool {
        let before = self.members.len();
        self.members.retain(|member| *member != entity);
        self.members.len() != before
    }

    /// Applies one player `use` on `follower`.
    ///
    /// **`TODO(black-box)`**: the published rule says "another will leave"
    /// without saying which, so the longest-serving ally is the one that
    /// leaves — the only choice that keeps the roster deterministic.
    pub fn toggle(&mut self, entity: Entity, follower: &mut Follower) -> FollowChange {
        if self.is_following(entity) {
            self.members.retain(|member| *member != entity);
            follower.following = false;
            return FollowChange::Stopped;
        }
        if !follower.can_follow {
            follower.following = false;
            return FollowChange::Refused;
        }
        let evicted = (self.members.len() >= MAX_FOLLOWERS).then(|| self.members.remove(0));
        self.members.push(entity);
        follower.following = true;
        FollowChange::Started { evicted }
    }
}

#[cfg(test)]
mod tests {
    use super::{FollowChange, FollowRoster, Follower, MAX_FOLLOWERS, SPAWNFLAG_TALK_PRE_DISASTER};
    use hecs::World;

    fn entities(count: usize) -> Vec<hecs::Entity> {
        let mut world = World::new();
        (0..count).map(|_| world.spawn((0u8,))).collect()
    }

    #[test]
    fn a_use_starts_following_and_a_second_use_stops_it() {
        let ids = entities(1);
        let mut roster = FollowRoster::new();
        let mut follower = Follower::from_spawnflags(0);
        assert_eq!(
            roster.toggle(ids[0], &mut follower),
            FollowChange::Started { evicted: None }
        );
        assert!(follower.following && roster.is_following(ids[0]));
        assert_eq!(roster.toggle(ids[0], &mut follower), FollowChange::Stopped);
        assert!(!follower.following && !roster.is_following(ids[0]));
    }

    #[test]
    fn a_pre_disaster_talk_monster_refuses_to_follow() {
        let ids = entities(1);
        let mut roster = FollowRoster::new();
        let mut follower = Follower::from_spawnflags(SPAWNFLAG_TALK_PRE_DISASTER);
        assert!(!follower.can_follow);
        assert_eq!(roster.toggle(ids[0], &mut follower), FollowChange::Refused);
        assert!(!follower.following);
        assert!(roster.members().is_empty());
    }

    #[test]
    fn recruiting_a_third_ally_makes_the_longest_serving_one_leave() {
        let ids = entities(3);
        let mut roster = FollowRoster::new();
        let mut followers = [Follower::from_spawnflags(0); 3];
        for (index, id) in ids.iter().take(MAX_FOLLOWERS).enumerate() {
            assert_eq!(
                roster.toggle(*id, &mut followers[index]),
                FollowChange::Started { evicted: None }
            );
        }
        assert_eq!(roster.members().len(), MAX_FOLLOWERS);
        assert_eq!(
            roster.toggle(ids[2], &mut followers[2]),
            FollowChange::Started {
                evicted: Some(ids[0])
            }
        );
        assert_eq!(roster.members(), &ids[1..3]);
    }

    #[test]
    fn a_removed_follower_is_gone_and_removing_twice_is_harmless() {
        let ids = entities(1);
        let mut roster = FollowRoster::new();
        let mut follower = Follower::from_spawnflags(0);
        roster.toggle(ids[0], &mut follower);
        assert!(roster.remove(ids[0]));
        assert!(!roster.remove(ids[0]));
        roster.clear();
        assert!(roster.members().is_empty());
    }
}
