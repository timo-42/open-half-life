//! Monster states, classifications, relationships and the condition bitset.
//!
//! The vocabulary — the state names, the relationship values and the
//! condition names — is the public behavioural vocabulary documented on the
//! TWHL wiki's "Monsters Programming" pages (see `docs/FORMAT_SOURCES.md`,
//! "Monster AI behaviour"). The bit layout, the table representation and
//! every default value below are this project's own choices; no SDK header,
//! table or decompilation was consulted.

use core::fmt;

/// The coarse behavioural mode a monster is in.
///
/// Documented state vocabulary; the transitions between them are authored in
/// [`crate::brain`] and by each [`crate::Brain`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MonsterState {
    /// Not yet initialised, or deliberately inert.
    #[default]
    None,
    /// Nothing of interest has been perceived.
    Idle,
    /// Something was perceived but no enemy is currently acquired.
    Alert,
    /// An enemy is acquired and visible.
    Combat,
    /// An enemy is acquired but not visible; move to its last known place.
    Hunt,
    /// Lying down / suppressed (barnacle-style hold, cower).
    Prone,
    /// Under the control of a scripted sequence.
    Script,
    /// Feigning death.
    PlayDead,
    /// Dead; no further scheduling.
    Dead,
}

impl MonsterState {
    /// Every state, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Idle,
        Self::Alert,
        Self::Combat,
        Self::Hunt,
        Self::Prone,
        Self::Script,
        Self::PlayDead,
        Self::Dead,
    ];

    /// A stable byte tag, used by determinism hashes and save files. The
    /// values never change once assigned, so a new state can only be added
    /// at the end.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Idle => 1,
            Self::Alert => 2,
            Self::Combat => 3,
            Self::Hunt => 4,
            Self::Prone => 5,
            Self::Script => 6,
            Self::PlayDead => 7,
            Self::Dead => 8,
        }
    }

    /// Whether the state still takes part in scheduling.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::None | Self::Dead)
    }

    /// The state a previous [`Self::tag`] named, or `None` for a tag this
    /// build does not recognise (a save file written by a newer build).
    /// Additive, for save-file restore: `.plan/m79-design.md` §6/§8 P4b.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            0 => Self::None,
            1 => Self::Idle,
            2 => Self::Alert,
            3 => Self::Combat,
            4 => Self::Hunt,
            5 => Self::Prone,
            6 => Self::Script,
            7 => Self::PlayDead,
            8 => Self::Dead,
            _ => return None,
        })
    }
}

impl fmt::Display for MonsterState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::None => "none",
            Self::Idle => "idle",
            Self::Alert => "alert",
            Self::Combat => "combat",
            Self::Hunt => "hunt",
            Self::Prone => "prone",
            Self::Script => "script",
            Self::PlayDead => "playdead",
            Self::Dead => "dead",
        };
        f.write_str(name)
    }
}

/// The number of [`Classification`] variants; the relationship table is
/// `CLASSIFICATION_COUNT` squared.
pub const CLASSIFICATION_COUNT: usize = 15;

/// The faction an entity belongs to.
///
/// The names are the published classification vocabulary; which concrete
/// monster maps to which class is package 7.7's job, not this crate's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Classification {
    /// Takes part in no relationship at all.
    #[default]
    None,
    /// Turrets, sentries and other automated hardware.
    Machine,
    /// The player.
    Player,
    /// Non-combatant humans (scientists).
    HumanPassive,
    /// Human soldiers.
    HumanMilitary,
    /// Alien soldiers.
    AlienMilitary,
    /// Non-combatant aliens.
    AlienPassive,
    /// Large hostile aliens.
    AlienMonster,
    /// Small aliens that are hunted by predators.
    AlienPrey,
    /// Aliens that hunt prey.
    AlienPredator,
    /// Insects.
    Insect,
    /// Humans that fight alongside the player (security guards).
    PlayerAlly,
    /// Weapons the player deploys that act on their own.
    PlayerBioweapon,
    /// Weapons an alien deploys that act on their own.
    AlienBioweapon,
    /// Barnacles.
    Barnacle,
}

impl Classification {
    /// Every classification, in declaration order.
    pub const ALL: [Self; CLASSIFICATION_COUNT] = [
        Self::None,
        Self::Machine,
        Self::Player,
        Self::HumanPassive,
        Self::HumanMilitary,
        Self::AlienMilitary,
        Self::AlienPassive,
        Self::AlienMonster,
        Self::AlienPrey,
        Self::AlienPredator,
        Self::Insect,
        Self::PlayerAlly,
        Self::PlayerBioweapon,
        Self::AlienBioweapon,
        Self::Barnacle,
    ];

    /// This classification's row/column in a [`RelationshipTable`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The classification at `index`, or `None` when out of range.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// How one classification regards another.
///
/// Ordered from most friendly to most hostile so [`Ord`] is the enemy
/// selection priority: a `Nemesis` outranks a `Hate` target, which outranks a
/// `Dislike` target, exactly as the published description of enemy
/// acquisition requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Relationship {
    /// Runs away from.
    Fear,
    /// Fights alongside.
    Ally,
    /// Indifferent.
    #[default]
    NoRelationship,
    /// Will attack.
    Dislike,
    /// Will attack in preference to a visible `Dislike` target.
    Hate,
    /// Will always attack, in preference to everything else.
    Nemesis,
}

impl Relationship {
    /// Whether this relationship makes the other entity a valid enemy.
    #[must_use]
    pub const fn is_hostile(self) -> bool {
        matches!(self, Self::Dislike | Self::Hate | Self::Nemesis)
    }

    /// A stable byte tag for determinism hashes and save files.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Fear => 0,
            Self::Ally => 1,
            Self::NoRelationship => 2,
            Self::Dislike => 3,
            Self::Hate => 4,
            Self::Nemesis => 5,
        }
    }

    /// The condition bit a sighting of an entity in this relationship sets.
    #[must_use]
    pub const fn sighting_condition(self) -> Conditions {
        match self {
            Self::Fear => Conditions::SEE_FEAR,
            Self::Dislike => Conditions::SEE_DISLIKE,
            Self::Hate => Conditions::SEE_HATE,
            Self::Nemesis => Conditions::SEE_NEMESIS,
            Self::Ally | Self::NoRelationship => Conditions::EMPTY,
        }
    }
}

/// A class-versus-class relationship matrix.
///
/// Data-driven on purpose: [`Self::provisional`] ships a small default so the
/// crate is usable today, and package 7.7 replaces individual entries from
/// per-monster public documentation without touching any code here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipTable {
    rows: [[Relationship; CLASSIFICATION_COUNT]; CLASSIFICATION_COUNT],
}

impl RelationshipTable {
    /// A table in which nobody has any relationship to anybody.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            rows: [[Relationship::NoRelationship; CLASSIFICATION_COUNT]; CLASSIFICATION_COUNT],
        }
    }

    /// **Provisional.** A small default matrix.
    ///
    /// Only one row is published outright — human soldiers hate the player,
    /// player allies and alien soldiers (`docs/FORMAT_SOURCES.md`, "Monster
    /// AI behaviour"). Everything else below is the obvious reciprocal or
    /// the plainly observable in-game grouping (aliens and humans fight,
    /// predators hunt prey, prey fears predators) and is marked to be
    /// black-box observed and corrected in package 7.7. Nothing here was
    /// taken from a relationship table in any SDK source.
    #[must_use]
    pub fn provisional() -> Self {
        use Classification as C;
        use Relationship as R;

        let mut table = Self::empty();
        let player_side = [C::Player, C::PlayerAlly, C::PlayerBioweapon];
        let alien_side = [
            C::AlienMilitary,
            C::AlienMonster,
            C::AlienPredator,
            C::AlienBioweapon,
            C::Barnacle,
        ];

        for &human in &player_side {
            for &alien in &alien_side {
                table.set(human, alien, R::Hate);
                table.set(alien, human, R::Hate);
            }
            table.set(human, C::HumanMilitary, R::Hate);
            table.set(human, C::Machine, R::Hate);
            table.set(C::Machine, human, R::Hate);
        }

        // The published row: human soldiers hate the player, player allies
        // and alien soldiers.
        table.set(C::HumanMilitary, C::Player, R::Hate);
        table.set(C::HumanMilitary, C::PlayerAlly, R::Hate);
        table.set(C::HumanMilitary, C::AlienMilitary, R::Hate);
        table.set(C::AlienMilitary, C::HumanMilitary, R::Hate);
        table.set(C::HumanMilitary, C::AlienMonster, R::Dislike);
        table.set(C::HumanMilitary, C::AlienPrey, R::Dislike);
        table.set(C::HumanMilitary, C::HumanPassive, R::NoRelationship);

        // Passive humans fear anything armed and hostile to them.
        for &hostile in &[C::HumanMilitary, C::AlienMilitary, C::AlienMonster] {
            table.set(C::HumanPassive, hostile, R::Fear);
        }
        table.set(C::HumanPassive, C::Player, R::Ally);
        table.set(C::PlayerAlly, C::Player, R::Ally);
        table.set(C::Player, C::PlayerAlly, R::Ally);
        table.set(C::Player, C::HumanPassive, R::Ally);

        // Predator/prey, and prey that fears its predator.
        table.set(C::AlienPredator, C::AlienPrey, R::Dislike);
        table.set(C::AlienPrey, C::AlienPredator, R::Fear);
        table.set(C::AlienPassive, C::AlienPredator, R::Fear);
        table.set(C::AlienPrey, C::Player, R::Dislike);
        table.set(C::AlienPredator, C::Player, R::Hate);

        // Insects and unclassified entities take part in nothing.
        for &other in &C::ALL {
            table.set(C::Insect, other, R::NoRelationship);
            table.set(other, C::Insect, R::NoRelationship);
            table.set(C::None, other, R::NoRelationship);
            table.set(other, C::None, R::NoRelationship);
        }

        table
    }

    /// How `observer` regards `subject`.
    #[must_use]
    pub fn get(&self, observer: Classification, subject: Classification) -> Relationship {
        self.rows[observer.index()][subject.index()]
    }

    /// Overrides how `observer` regards `subject`.
    pub fn set(
        &mut self,
        observer: Classification,
        subject: Classification,
        relationship: Relationship,
    ) {
        self.rows[observer.index()][subject.index()] = relationship;
    }

    /// The table as a flat row-major byte string, for determinism hashes.
    #[must_use]
    pub fn to_tags(&self) -> [u8; CLASSIFICATION_COUNT * CLASSIFICATION_COUNT] {
        let mut out = [0u8; CLASSIFICATION_COUNT * CLASSIFICATION_COUNT];
        for (row_index, row) in self.rows.iter().enumerate() {
            for (column_index, relationship) in row.iter().enumerate() {
                out[row_index * CLASSIFICATION_COUNT + column_index] = relationship.tag();
            }
        }
        out
    }
}

impl Default for RelationshipTable {
    fn default() -> Self {
        Self::provisional()
    }
}

/// The per-tick condition bitset a monster's senses and damage produce and
/// its schedule selection and interrupt masks consume.
///
/// The condition *names* are the published vocabulary; the bit assignment is
/// this project's own and is stable, so a new condition may only take the
/// next free bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Conditions(u32);

macro_rules! conditions {
    ($($(#[$doc:meta])* $name:ident = $bit:expr;)*) => {
        impl Conditions {
            $($(#[$doc])* pub const $name: Self = Self(1 << $bit);)*

            /// Every named condition, with its name, for diagnostics.
            pub const NAMED: &'static [(&'static str, Self)] = &[
                $((stringify!($name), Self(1 << $bit)),)*
            ];
        }
    };
}

conditions! {
    /// An acquired enemy is currently visible.
    SEE_ENEMY = 0;
    /// A `Hate` target is visible.
    SEE_HATE = 1;
    /// A `Fear` target is visible.
    SEE_FEAR = 2;
    /// A `Dislike` target is visible.
    SEE_DISLIKE = 3;
    /// The player is visible.
    SEE_CLIENT = 4;
    /// A `Nemesis` target is visible.
    SEE_NEMESIS = 5;
    /// The acquired enemy exists but line of sight is blocked.
    ENEMY_OCCLUDED = 6;
    /// The acquired enemy is beyond the look distance.
    ENEMY_TOOFAR = 7;
    /// The acquired enemy changed this tick.
    NEW_ENEMY = 8;
    /// The acquired enemy is looking at us.
    ENEMY_FACING_ME = 9;
    /// The acquired enemy is dead.
    ENEMY_DEAD = 10;
    /// Any audible sound was heard this tick.
    HEAR_SOUND = 11;
    /// A sound classified as dangerous was heard this tick.
    HEAR_DANGER = 12;
    /// A sound classified as combat was heard this tick.
    HEAR_COMBAT = 13;
    /// A scent (carcass / meat / garbage) was smelled this tick.
    SMELL = 14;
    /// Damage below the heavy threshold was taken this tick.
    LIGHT_DAMAGE = 15;
    /// Damage at or above the heavy threshold was taken this tick.
    HEAVY_DAMAGE = 16;
    /// A melee attack is in range and unobstructed.
    CAN_MELEE_ATTACK1 = 17;
    /// A secondary melee attack is in range and unobstructed.
    CAN_MELEE_ATTACK2 = 18;
    /// A ranged attack is in range and unobstructed.
    CAN_RANGE_ATTACK1 = 19;
    /// A secondary ranged attack is in range and unobstructed.
    CAN_RANGE_ATTACK2 = 20;
    /// The running task reported failure.
    TASK_FAILED = 21;
    /// The running schedule ran out of tasks.
    SCHEDULE_DONE = 22;
    /// Attacked by something normally not an enemy, so it became one.
    PROVOKED = 23;
    /// The loaded weapon is empty.
    NO_AMMO_LOADED = 24;
    /// Movement made no progress for several ticks.
    BLOCKED = 25;
    /// The monster is standing in or beside something harmful.
    IN_DANGER = 26;
    /// A per-monster condition, meaning whatever that monster's brain says.
    SPECIAL1 = 27;
    /// A second per-monster condition.
    SPECIAL2 = 28;
}

impl Conditions {
    /// No conditions at all.
    pub const EMPTY: Self = Self(0);

    /// Every condition that reports having seen something.
    pub const ALL_SIGHT: Self = Self(
        Self::SEE_ENEMY.0
            | Self::SEE_HATE.0
            | Self::SEE_FEAR.0
            | Self::SEE_DISLIKE.0
            | Self::SEE_CLIENT.0
            | Self::SEE_NEMESIS.0,
    );

    /// Every condition that reports having heard or smelled something.
    pub const ALL_SOUND: Self =
        Self(Self::HEAR_SOUND.0 | Self::HEAR_DANGER.0 | Self::HEAR_COMBAT.0 | Self::SMELL.0);

    /// Every condition that reports having been hurt.
    pub const ALL_DAMAGE: Self = Self(Self::LIGHT_DAMAGE.0 | Self::HEAVY_DAMAGE.0);

    /// Every condition that reports an attack opportunity.
    pub const ALL_ATTACK: Self = Self(
        Self::CAN_MELEE_ATTACK1.0
            | Self::CAN_MELEE_ATTACK2.0
            | Self::CAN_RANGE_ATTACK1.0
            | Self::CAN_RANGE_ATTACK2.0,
    );

    /// The conditions that end essentially any schedule: the running task
    /// failed, or movement is going nowhere.
    pub const GENERAL_INTERRUPTS: Self = Self(Self::TASK_FAILED.0 | Self::BLOCKED.0);

    /// A bitset from its raw bits.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Whether no condition is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether every condition in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any condition in `other` is set.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The union of two bitsets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The intersection of two bitsets.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// `self` with every condition in `other` removed.
    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// Sets every condition in `other`.
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Clears every condition in `other`.
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// Sets or clears `other` according to `value`.
    pub fn set(&mut self, other: Self, value: bool) {
        if value {
            self.insert(other);
        } else {
            self.remove(other);
        }
    }
}

impl core::ops::BitOr for Conditions {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl core::ops::BitOrAssign for Conditions {
    fn bitor_assign(&mut self, rhs: Self) {
        self.insert(rhs);
    }
}

impl fmt::Display for Conditions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("(none)");
        }
        let mut first = true;
        for (name, bit) in Self::NAMED {
            if self.contains(*bit) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Classification, Conditions, MonsterState, Relationship, RelationshipTable};

    #[test]
    fn every_monster_state_round_trips_through_its_tag() {
        for state in MonsterState::ALL {
            assert_eq!(MonsterState::from_tag(state.tag()), Some(state));
        }
    }

    #[test]
    fn an_unrecognised_monster_state_tag_is_rejected() {
        assert_eq!(MonsterState::from_tag(255), None);
    }

    #[test]
    fn every_condition_bit_is_distinct() {
        let mut seen = 0u32;
        for (name, bit) in Conditions::NAMED {
            assert_eq!(bit.bits().count_ones(), 1, "{name} is not a single bit");
            assert_eq!(seen & bit.bits(), 0, "{name} reuses an assigned bit");
            seen |= bit.bits();
        }
    }

    #[test]
    fn bitset_operations_behave() {
        let a = Conditions::SEE_ENEMY | Conditions::HEAR_DANGER;
        assert!(a.contains(Conditions::SEE_ENEMY));
        assert!(!a.contains(Conditions::SEE_ENEMY | Conditions::SEE_HATE));
        assert!(a.intersects(Conditions::ALL_SOUND));
        assert!(
            a.difference(Conditions::ALL_SOUND)
                .contains(Conditions::SEE_ENEMY)
        );
        assert!(
            !a.difference(Conditions::ALL_SOUND)
                .intersects(Conditions::HEAR_DANGER)
        );
        let mut b = Conditions::EMPTY;
        b.set(Conditions::BLOCKED, true);
        assert!(b.contains(Conditions::BLOCKED));
        b.set(Conditions::BLOCKED, false);
        assert!(b.is_empty());
    }

    #[test]
    fn hostility_ordering_prefers_the_worse_relationship() {
        assert!(Relationship::Nemesis > Relationship::Hate);
        assert!(Relationship::Hate > Relationship::Dislike);
        assert!(Relationship::Dislike > Relationship::NoRelationship);
        assert!(Relationship::Nemesis.is_hostile());
        assert!(!Relationship::Fear.is_hostile());
        assert!(!Relationship::Ally.is_hostile());
    }

    #[test]
    fn the_provisional_table_carries_the_published_row() {
        let table = RelationshipTable::provisional();
        assert_eq!(
            table.get(Classification::HumanMilitary, Classification::Player),
            Relationship::Hate
        );
        assert_eq!(
            table.get(Classification::HumanMilitary, Classification::PlayerAlly),
            Relationship::Hate
        );
        assert_eq!(
            table.get(Classification::HumanMilitary, Classification::AlienMilitary),
            Relationship::Hate
        );
        assert_eq!(
            table.get(Classification::None, Classification::Player),
            Relationship::NoRelationship
        );
        assert_eq!(
            table.get(Classification::AlienPrey, Classification::AlienPredator),
            Relationship::Fear
        );
    }

    #[test]
    fn an_overridden_entry_wins() {
        let mut table = RelationshipTable::empty();
        table.set(
            Classification::Insect,
            Classification::Player,
            Relationship::Nemesis,
        );
        assert_eq!(
            table.get(Classification::Insect, Classification::Player),
            Relationship::Nemesis
        );
        assert_eq!(
            table.get(Classification::Player, Classification::Insect),
            Relationship::NoRelationship
        );
        assert_eq!(
            table.to_tags().len(),
            super::CLASSIFICATION_COUNT * super::CLASSIFICATION_COUNT
        );
    }

    #[test]
    fn state_tags_are_unique_and_stable() {
        let mut tags: Vec<u8> = super::MonsterState::ALL.iter().map(|s| s.tag()).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), super::MonsterState::ALL.len());
        assert_eq!(super::MonsterState::Idle.tag(), 1);
        assert!(super::MonsterState::Combat.is_active());
        assert!(!super::MonsterState::Dead.is_active());
    }
}
