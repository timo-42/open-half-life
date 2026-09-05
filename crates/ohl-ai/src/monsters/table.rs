//! Per-monster identity, classification and stat table.
//!
//! ## Clean room
//!
//! The monster *roster* — which classnames exist, their published
//! classification/faction, their hull size class and their broad behaviour
//! category — is public modding knowledge (TWHL wiki monster pages; see
//! `docs/FORMAT_SOURCES.md`, "Monster AI behaviour" and "Monster
//! definitions"). **The per-monster health and attack-damage numbers in
//! [`spec_for`] could not be independently verified from a reachable
//! public source in this pass** — TWHL's `skill.cfg` pages return HTTP 403 to
//! automated fetches here, and every mirror of a "Half-Life `skill.cfg`"
//! found during research (Sven Co-op, other total-conversion mods, and one
//! GameBanana upload with internally inconsistent duplicate sections) is a
//! *modified* config, not vanilla retail Half-Life, and several disagreed
//! with each other for the same cvar. Rather than commit a number attributed
//! to a source that does not actually support it, every health and damage
//! value below is a **black-box placeholder** (`TODO(black-box)`) still to
//! be observed against legally obtained retail software and corrected in
//! place; see `docs/FORMAT_SOURCES.md`, "Monster definitions" for the
//! research trail. The *shape* of the table — three difficulty columns per
//! stat, keyed by the `sk_<subject>_<property><1|2|3>` cvar convention
//! `ohl_formats::skill_cfg` and `ohl_campaign::SkillTable` already document
//! and implement — is real and citable; only the numbers inside it are
//! placeholders.

use crate::state::Classification;
use ohl_physics::Hull;

/// A `skill.cfg`-style override lookup: given a cvar name (e.g.
/// `"sk_headcrab_health3"`), returns the overriding value, or `None` to fall
/// back to this table's placeholder. A type alias purely to keep the
/// `Option<&dyn Fn(..) -> ..>` signatures below readable.
pub type SkillLookup<'a> = dyn Fn(&str) -> Option<f32> + 'a;

/// A difficulty level, matching the `1`/`2`/`3` = easy/medium/hard
/// `sk_<subject>_<property><N>` convention documented in
/// `docs/FORMAT_SOURCES.md` ("Game text formats", `skill.cfg`) and already
/// implemented by `ohl_campaign::Difficulty`. Defined again here rather than
/// depending on `ohl-campaign`, which is not an allowed edge for `ohl-ai`
/// (`xtask/src/graph.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Difficulty {
    /// `skill 1`.
    #[default]
    Easy,
    /// `skill 2`.
    Medium,
    /// `skill 3`.
    Hard,
}

impl Difficulty {
    /// Every difficulty, in cvar-suffix order.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// This difficulty's index into a `[easy, medium, hard]` array.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Easy => 0,
            Self::Medium => 1,
            Self::Hard => 2,
        }
    }

    /// The `sk_<subject>_<property><N>` cvar suffix digit.
    #[must_use]
    pub const fn skill_suffix(self) -> u8 {
        match self {
            Self::Easy => 1,
            Self::Medium => 2,
            Self::Hard => 3,
        }
    }
}

/// How much blood a monster's death/gib effects should use.
///
/// Published lore fact, not a numeric skill.cfg value: humans and player
/// allies bleed red, most aliens bleed green, headcrab-family and
/// zombie-family bleed yellow (widely documented modding/mapping
/// convention, e.g. the `BloodColor` FGD field on monster entities).
/// `MachineNone` covers turrets, sentries and other hardware, which do not
/// bleed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum BloodKind {
    /// Humans, human allies.
    Red,
    /// Headcrabs and zombies.
    Yellow,
    /// Most other aliens.
    Green,
    /// Machines: no blood at all.
    #[default]
    None,
}

/// A coarse size bucket, used for hull selection and gib-threshold scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SizeClass {
    /// Headcrab, leech.
    Small,
    /// Human-sized and mid-sized aliens.
    #[default]
    Medium,
    /// Gargantua, ichthyosaur.
    Large,
}

/// Per-monster boolean behaviour flags.
///
/// A small project-owned bitset, in the same style as
/// [`crate::state::Conditions`], so a new flag only ever takes the next free
/// bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MonsterFlags(u32);

impl MonsterFlags {
    /// No flags set.
    pub const EMPTY: Self = Self(0);
    /// The monster can open doors ahead of it (published `CanOpenDoors`
    /// mapping-documentation vocabulary; which concrete monsters do so is
    /// this table's own black-box-observed assignment).
    pub const OPENS_DOORS: Self = Self(1 << 0);
    /// The corpse fades out rather than staying on the ground
    /// (`monster_generic`'s documented `Fade Corpse` spawnflag concept,
    /// applied here as a per-species default rather than a per-instance
    /// spawnflag override, which is out of this crate's scope).
    pub const FADES_CORPSE: Self = Self(1 << 1);
    /// Squad behaviour applies (recruits/joins a [`crate::squad::SquadRoster`]).
    pub const SQUAD_MONSTER: Self = Self(1 << 2);
    /// Never flees regardless of `SEE_FEAR`/low health (turrets, gargantua).
    pub const NEVER_FLEES: Self = Self(1 << 3);

    /// The union of two flag sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for MonsterFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

/// One attack's placeholder damage table and reach.
///
/// **Every field is `TODO(black-box)`**: no per-monster damage or range
/// number survived independent verification (see the module doc comment).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackSpec {
    /// Damage dealt on a hit, `[easy, medium, hard]`.
    pub damage: [f32; 3],
    /// Reach, in world units.
    pub range: f32,
}

impl AttackSpec {
    /// A same-damage-every-difficulty placeholder attack, marked
    /// black-box.
    #[must_use]
    pub const fn placeholder(damage: f32, range: f32) -> Self {
        Self {
            damage: [damage, damage, damage],
            range,
        }
    }

    /// The damage at `difficulty`, with a `skill`-table override applied
    /// first if it returns one.
    #[must_use]
    pub fn resolve_damage(
        &self,
        difficulty: Difficulty,
        cvar: &str,
        skill: Option<&SkillLookup<'_>>,
    ) -> f32 {
        resolve_stat(self.damage, difficulty, cvar, skill)
    }
}

/// The monster kinds package 7.7 defines, plus a fallback for any other
/// `monster_*` classname `ohl-game` hands the AI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MonsterKind {
    /// `monster_headcrab`.
    Headcrab,
    /// `monster_zombie`.
    Zombie,
    /// `monster_houndeye`.
    Houndeye,
    /// `monster_bullsquid`.
    Bullsquid,
    /// `monster_alien_slave`.
    AlienSlave,
    /// `monster_alien_grunt`.
    AlienGrunt,
    /// `monster_human_grunt`.
    HumanGrunt,
    /// `monster_barney`.
    Barney,
    /// `monster_scientist`.
    Scientist,
    /// `monster_turret`.
    Turret,
    /// `monster_miniturret`.
    MiniTurret,
    /// `monster_sentry`.
    Sentry,
    /// `monster_ichthyosaur`.
    Ichthyosaur,
    /// `monster_leech`.
    Leech,
    /// `monster_gargantua`.
    Gargantua,
    /// `monster_tentacle`.
    Tentacle,
    /// Any classname this table does not (yet) know, carried verbatim so it
    /// can still be logged, spawned as an inert actor, or rejected.
    Unknown(String),
}

impl MonsterKind {
    /// The `monster_*` classname this kind spawns from, matching
    /// [`Self::from_classname`].
    #[must_use]
    pub fn classname(&self) -> &str {
        match self {
            Self::Headcrab => "monster_headcrab",
            Self::Zombie => "monster_zombie",
            Self::Houndeye => "monster_houndeye",
            Self::Bullsquid => "monster_bullsquid",
            Self::AlienSlave => "monster_alien_slave",
            Self::AlienGrunt => "monster_alien_grunt",
            Self::HumanGrunt => "monster_human_grunt",
            Self::Barney => "monster_barney",
            Self::Scientist => "monster_scientist",
            Self::Turret => "monster_turret",
            Self::MiniTurret => "monster_miniturret",
            Self::Sentry => "monster_sentry",
            Self::Ichthyosaur => "monster_ichthyosaur",
            Self::Leech => "monster_leech",
            Self::Gargantua => "monster_gargantua",
            Self::Tentacle => "monster_tentacle",
            Self::Unknown(classname) => classname,
        }
    }

    /// The kind for a map entity's `classname`, or [`Self::Unknown`] when it
    /// is not one of the sixteen this table defines.
    #[must_use]
    pub fn from_classname(classname: &str) -> Self {
        match classname {
            "monster_headcrab" => Self::Headcrab,
            "monster_zombie" => Self::Zombie,
            "monster_houndeye" => Self::Houndeye,
            "monster_bullsquid" => Self::Bullsquid,
            "monster_alien_slave" => Self::AlienSlave,
            "monster_alien_grunt" => Self::AlienGrunt,
            "monster_human_grunt" => Self::HumanGrunt,
            "monster_barney" => Self::Barney,
            "monster_scientist" => Self::Scientist,
            "monster_turret" => Self::Turret,
            "monster_miniturret" => Self::MiniTurret,
            "monster_sentry" => Self::Sentry,
            "monster_ichthyosaur" => Self::Ichthyosaur,
            "monster_leech" => Self::Leech,
            "monster_gargantua" => Self::Gargantua,
            "monster_tentacle" => Self::Tentacle,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// The `sk_<subject>` cvar stem this kind's skill.cfg entries use.
    /// Public, widely mirrored skill.cfg subject names (independent of the
    /// unverifiable *values* discussed in the module doc comment — the
    /// naming convention itself is corroborated by multiple sources and by
    /// this project's own `ohl_formats::skill_cfg` reader).
    #[must_use]
    pub fn skill_subject(&self) -> &str {
        match self {
            Self::Headcrab => "headcrab",
            Self::Zombie => "zombie",
            Self::Houndeye => "houndeye",
            Self::Bullsquid => "bullsquid",
            Self::AlienSlave => "islave",
            Self::AlienGrunt => "agrunt",
            Self::HumanGrunt => "hgrunt",
            Self::Barney => "barney",
            Self::Scientist => "scientist",
            Self::Turret => "turret",
            Self::MiniTurret => "miniturret",
            Self::Sentry => "sentry",
            Self::Ichthyosaur => "ichthyosaur",
            Self::Leech => "leech",
            Self::Gargantua => "garg",
            Self::Tentacle => "tentacle",
            Self::Unknown(classname) => classname,
        }
    }

    /// Every defined kind (not [`Self::Unknown`]), in table order.
    #[must_use]
    pub fn defined() -> &'static [MonsterKind] {
        const KINDS: [MonsterKind; 16] = [
            MonsterKind::Headcrab,
            MonsterKind::Zombie,
            MonsterKind::Houndeye,
            MonsterKind::Bullsquid,
            MonsterKind::AlienSlave,
            MonsterKind::AlienGrunt,
            MonsterKind::HumanGrunt,
            MonsterKind::Barney,
            MonsterKind::Scientist,
            MonsterKind::Turret,
            MonsterKind::MiniTurret,
            MonsterKind::Sentry,
            MonsterKind::Ichthyosaur,
            MonsterKind::Leech,
            MonsterKind::Gargantua,
            MonsterKind::Tentacle,
        ];
        &KINDS
    }
}

/// A monster kind's fixed identity data: classification, hull, blood, size,
/// door-opening and the black-box health/attack tables.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterSpec {
    /// The faction, for [`crate::state::RelationshipTable`] lookups.
    pub classification: Classification,
    /// Health, `[easy, medium, hard]`. **`TODO(black-box)`**, see the
    /// module doc comment.
    pub health: [f32; 3],
    /// The primary melee attack, if any. **`TODO(black-box)`**.
    pub melee: Option<AttackSpec>,
    /// The primary ranged attack, if any. **`TODO(black-box)`**.
    pub ranged: Option<AttackSpec>,
    /// The collision hull this monster moves with.
    pub hull: Hull,
    /// Death/gib blood color.
    pub blood: BloodKind,
    /// The size bucket.
    pub size: SizeClass,
    /// Whether it opens doors ahead of it.
    pub can_open_doors: bool,
    /// Behaviour flags.
    pub flags: MonsterFlags,
}

impl MonsterSpec {
    /// The health at `difficulty`, with a `skill`-table override applied
    /// first (looked up as `sk_<subject>_health<N>`).
    #[must_use]
    pub fn resolve_health(
        &self,
        kind: &MonsterKind,
        difficulty: Difficulty,
        skill: Option<&SkillLookup<'_>>,
    ) -> f32 {
        let cvar = format!(
            "sk_{}_health{}",
            kind.skill_subject(),
            difficulty.skill_suffix()
        );
        resolve_stat(self.health, difficulty, &cvar, skill)
    }
}

fn resolve_stat(
    table: [f32; 3],
    difficulty: Difficulty,
    cvar: &str,
    skill: Option<&SkillLookup<'_>>,
) -> f32 {
    if let Some(lookup) = skill
        && let Some(value) = lookup(cvar)
        && value.is_finite()
    {
        return value;
    }
    table[difficulty.index()]
}

/// The spec for a defined [`MonsterKind`], or `None` for
/// [`MonsterKind::Unknown`].
///
/// **Every numeric field is a black-box placeholder**; see the module doc
/// comment and `docs/FORMAT_SOURCES.md`, "Monster definitions", for exactly
/// what could and could not be independently verified.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one row per defined monster kind, each stating its own black-box placeholders explicitly"
)]
pub fn spec_for(kind: &MonsterKind) -> Option<&'static MonsterSpec> {
    use Classification as C;

    // A tiny helper so every row below states its own damage/range/health
    // placeholders explicitly rather than relying on a shared default that
    // could quietly drift. Returning `Option` (rather than `AttackSpec`
    // directly) matches the `Option<AttackSpec>` field it is always used to
    // fill in below.
    #[allow(clippy::unnecessary_wraps)]
    const fn melee(damage: f32, range: f32) -> Option<AttackSpec> {
        Some(AttackSpec::placeholder(damage, range))
    }
    #[allow(clippy::unnecessary_wraps)]
    const fn ranged(damage: f32, range: f32) -> Option<AttackSpec> {
        Some(AttackSpec::placeholder(damage, range))
    }

    static HEADCRAB: MonsterSpec = MonsterSpec {
        classification: C::AlienPrey,
        health: [10.0, 10.0, 10.0],
        melee: melee(5.0, 48.0),
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Yellow,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::FADES_CORPSE,
    };
    static ZOMBIE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [50.0, 50.0, 50.0],
        melee: melee(20.0, 64.0),
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::Yellow,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    static HOUNDEYE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [20.0, 20.0, 20.0],
        melee: melee(10.0, 48.0),
        ranged: ranged(15.0, 384.0),
        hull: Hull::Standing,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::SQUAD_MONSTER,
    };
    static BULLSQUID: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [40.0, 40.0, 40.0],
        melee: melee(15.0, 64.0),
        ranged: ranged(12.0, 512.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    static ALIEN_SLAVE: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [30.0, 30.0, 30.0],
        melee: melee(15.0, 48.0),
        ranged: ranged(15.0, 640.0),
        hull: Hull::Standing,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    static ALIEN_GRUNT: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [80.0, 80.0, 80.0],
        melee: melee(25.0, 64.0),
        ranged: ranged(20.0, 768.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    static HUMAN_GRUNT: MonsterSpec = MonsterSpec {
        classification: C::HumanMilitary,
        health: [50.0, 50.0, 50.0],
        melee: melee(10.0, 48.0),
        ranged: ranged(8.0, 1_024.0),
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::SQUAD_MONSTER.union(MonsterFlags::OPENS_DOORS),
    };
    static BARNEY: MonsterSpec = MonsterSpec {
        classification: C::PlayerAlly,
        health: [50.0, 50.0, 50.0],
        melee: melee(10.0, 48.0),
        ranged: ranged(8.0, 1_024.0),
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::OPENS_DOORS,
    };
    static SCIENTIST: MonsterSpec = MonsterSpec {
        classification: C::HumanPassive,
        health: [20.0, 20.0, 20.0],
        melee: None,
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::OPENS_DOORS,
    };
    static TURRET: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [30.0, 30.0, 30.0],
        melee: None,
        ranged: ranged(8.0, 1_024.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    static MINITURRET: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [20.0, 20.0, 20.0],
        melee: None,
        ranged: ranged(6.0, 1_024.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    static SENTRY: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [10.0, 10.0, 10.0],
        melee: None,
        ranged: ranged(6.0, 768.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    static ICHTHYOSAUR: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [50.0, 50.0, 50.0],
        melee: melee(25.0, 96.0),
        ranged: None,
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    static LEECH: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [10.0, 10.0, 10.0],
        melee: melee(2.0, 32.0),
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Green,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    static GARGANTUA: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [400.0, 400.0, 400.0],
        melee: melee(40.0, 96.0),
        ranged: ranged(15.0, 512.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: true,
        flags: MonsterFlags::NEVER_FLEES,
    };
    static TENTACLE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [75.0, 75.0, 75.0],
        melee: melee(50.0, 128.0),
        ranged: None,
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };

    match kind {
        MonsterKind::Headcrab => Some(&HEADCRAB),
        MonsterKind::Zombie => Some(&ZOMBIE),
        MonsterKind::Houndeye => Some(&HOUNDEYE),
        MonsterKind::Bullsquid => Some(&BULLSQUID),
        MonsterKind::AlienSlave => Some(&ALIEN_SLAVE),
        MonsterKind::AlienGrunt => Some(&ALIEN_GRUNT),
        MonsterKind::HumanGrunt => Some(&HUMAN_GRUNT),
        MonsterKind::Barney => Some(&BARNEY),
        MonsterKind::Scientist => Some(&SCIENTIST),
        MonsterKind::Turret => Some(&TURRET),
        MonsterKind::MiniTurret => Some(&MINITURRET),
        MonsterKind::Sentry => Some(&SENTRY),
        MonsterKind::Ichthyosaur => Some(&ICHTHYOSAUR),
        MonsterKind::Leech => Some(&LEECH),
        MonsterKind::Gargantua => Some(&GARGANTUA),
        MonsterKind::Tentacle => Some(&TENTACLE),
        MonsterKind::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AttackSpec, Difficulty, MonsterKind, spec_for};

    /// Invented, project-authored expectation list (not derived from any
    /// SDK source), asserting the table is internally consistent and that
    /// every published health/damage cell is exercised. These values are
    /// this test's own fixture, checked in so a change to the black-box
    /// placeholders above is a deliberate, reviewed diff.
    #[test]
    fn every_defined_kind_resolves_to_the_documented_placeholder_health() {
        let expected: &[(MonsterKind, [f32; 3])] = &[
            (MonsterKind::Headcrab, [10.0, 10.0, 10.0]),
            (MonsterKind::Zombie, [50.0, 50.0, 50.0]),
            (MonsterKind::Houndeye, [20.0, 20.0, 20.0]),
            (MonsterKind::Bullsquid, [40.0, 40.0, 40.0]),
            (MonsterKind::AlienSlave, [30.0, 30.0, 30.0]),
            (MonsterKind::AlienGrunt, [80.0, 80.0, 80.0]),
            (MonsterKind::HumanGrunt, [50.0, 50.0, 50.0]),
            (MonsterKind::Barney, [50.0, 50.0, 50.0]),
            (MonsterKind::Scientist, [20.0, 20.0, 20.0]),
            (MonsterKind::Turret, [30.0, 30.0, 30.0]),
            (MonsterKind::MiniTurret, [20.0, 20.0, 20.0]),
            (MonsterKind::Sentry, [10.0, 10.0, 10.0]),
            (MonsterKind::Ichthyosaur, [50.0, 50.0, 50.0]),
            (MonsterKind::Leech, [10.0, 10.0, 10.0]),
            (MonsterKind::Gargantua, [400.0, 400.0, 400.0]),
            (MonsterKind::Tentacle, [75.0, 75.0, 75.0]),
        ];
        assert_eq!(expected.len(), MonsterKind::defined().len());
        for (kind, health) in expected {
            let spec = spec_for(kind).unwrap_or_else(|| panic!("{kind:?} is defined"));
            for difficulty in Difficulty::ALL {
                assert!(
                    (spec.resolve_health(kind, difficulty, None) - health[difficulty.index()])
                        .abs()
                        < 1e-6
                );
            }
        }
    }

    #[test]
    fn an_unknown_classname_has_no_spec() {
        let kind = MonsterKind::from_classname("monster_totally_made_up");
        assert!(
            matches!(kind, MonsterKind::Unknown(ref name) if name == "monster_totally_made_up")
        );
        assert!(spec_for(&kind).is_none());
    }

    #[test]
    fn every_defined_classname_round_trips() {
        for kind in MonsterKind::defined() {
            let round_tripped = MonsterKind::from_classname(kind.classname());
            assert_eq!(round_tripped.classname(), kind.classname());
            assert!(spec_for(kind).is_some());
        }
    }

    #[test]
    fn a_skill_table_override_wins_over_the_placeholder() {
        let spec = spec_for(&MonsterKind::Headcrab).expect("defined");
        let lookup: &dyn Fn(&str) -> Option<f32> =
            &|cvar: &str| (cvar == "sk_headcrab_health3").then_some(123.0);
        assert!(
            (spec.resolve_health(&MonsterKind::Headcrab, Difficulty::Hard, Some(lookup)) - 123.0)
                .abs()
                < 1e-6
        );
        // A different difficulty falls back to the placeholder.
        assert!(
            (spec.resolve_health(&MonsterKind::Headcrab, Difficulty::Easy, Some(lookup)) - 10.0)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn a_non_finite_override_is_ignored() {
        let spec = spec_for(&MonsterKind::Zombie).expect("defined");
        let lookup: &dyn Fn(&str) -> Option<f32> = &|_: &str| Some(f32::NAN);
        let resolved = spec.resolve_health(&MonsterKind::Zombie, Difficulty::Medium, Some(lookup));
        assert!((resolved - 50.0).abs() < 1e-6);
    }

    #[test]
    fn attack_damage_resolves_with_the_same_override_rule() {
        let attack = AttackSpec::placeholder(9.0, 48.0);
        assert!((attack.resolve_damage(Difficulty::Easy, "sk_x_dmg1", None) - 9.0).abs() < 1e-6);
        let lookup: &dyn Fn(&str) -> Option<f32> =
            &|cvar: &str| (cvar == "sk_x_dmg1").then_some(5.0);
        assert!(
            (attack.resolve_damage(Difficulty::Easy, "sk_x_dmg1", Some(lookup)) - 5.0).abs() < 1e-6
        );
    }

    #[test]
    fn difficulty_suffixes_match_the_skill_cfg_convention() {
        assert_eq!(Difficulty::Easy.skill_suffix(), 1);
        assert_eq!(Difficulty::Medium.skill_suffix(), 2);
        assert_eq!(Difficulty::Hard.skill_suffix(), 3);
    }
}
