//! Per-monster identity, classification and stat table.
//!
//! ## Clean room
//!
//! The monster *roster* — which classnames exist, their published
//! classification/faction, their hull size class and their broad behaviour
//! category — is public modding knowledge (TWHL wiki monster pages; see
//! `docs/FORMAT_SOURCES.md`, "Monster AI behaviour" and "Monster
//! definitions"). **Every health and primary melee/ranged attack-damage
//! number in [`spec_for`] is cited to that monster's own TWHL wiki page**
//! (`https://twhl.info/wiki/page/<entity>`; see `docs/FORMAT_SOURCES.md`,
//! "Monster definitions" for the exact page per row), reached via
//! search-result snippets since twhl.info returns HTTP 403 to automated
//! fetches from this environment (the same limitation already recorded for
//! "Monster AI behaviour" and "Entity keyvalues and map logic"). An earlier
//! pass had instead searched for a vanilla retail `skill.cfg` mirror and
//! found only mutually-disagreeing *modified* mod configs (Sven Co-op, an
//! unidentified other mod, a GameBanana upload with internally inconsistent
//! duplicate sections); none of those numbers were used. What TWHL's pages
//! do not give — attack reach/range for most monsters, view cones, look
//! distances, movement speeds, turn rates (turret family excepted, see
//! `TURRET_TURN_RATE_DEGREES_PER_SECOND`), and every schedule's own timing —
//! remains a **black-box placeholder** (`TODO(black-box)`), marked as such
//! at each use. The `sk_<subject>_<property><1|2|3>` cvar-naming convention
//! `ohl_formats::skill_cfg` and `ohl_campaign::SkillTable` already document
//! and implement is unrelated to (and not needed to cite) the numbers below;
//! [`SkillLookup`] exists only so a caller's own parsed `skill.cfg` can
//! override them per map.

use crate::state::Classification;
use ohl_physics::Hull;

/// A `skill.cfg`-style override lookup: given a cvar name (e.g.
/// `"sk_headcrab_health3"`), returns the overriding value, or `None` to fall
/// back to this table's placeholder. A type alias purely to keep the
/// `Option<&dyn Fn(..) -> ..>` signatures below readable.
pub type SkillLookup<'a> = dyn Fn(&str) -> Option<f32> + 'a;

// --- Secondary/published attack numbers that don't fit `MonsterSpec`'s ----
// --- single primary melee/ranged pair -------------------------------------
//
// `MonsterSpec` carries one melee and one ranged `AttackSpec`, matching
// `crate::schedule::Task::MeleeAttack1`/`RangeAttack1`. Several monsters
// have a second published attack (a heavier melee swing, a thrown weapon, a
// flat non-skill-scaled hit); those numbers are cited here as named
// constants instead of widening `MonsterSpec`'s shape. Each is cited to the
// same TWHL page as its monster's row in `spec_for`; see
// `docs/FORMAT_SOURCES.md`, "Monster definitions".

/// `TWHL:Zombie`'s second, two-handed swing, `[easy, medium, hard]`.
pub const ZOMBIE_BOTH_HANDS_DAMAGE: [f32; 3] = [25.0, 40.0, 40.0];

/// `TWHL:Houndeye`'s published blast radius, in world units.
pub const HOUNDEYE_BLAST_RADIUS: f32 = 192.0;

/// `TWHL:Houndeye`'s published damage multiplier when the blast has no line
/// of sight to its target.
pub const HOUNDEYE_BLAST_NO_LOS_MULTIPLIER: f32 = 0.5;

/// `TWHL:Bullsquid`'s second, tail-whip melee attack, `[easy, medium, hard]`.
pub const BULLSQUID_WHIP_DAMAGE: [f32; 3] = [25.0, 35.0, 35.0];

/// `TWHL:Alien_Slave`'s heavier "rake" claw hit; one published value, not
/// skill-scaled.
pub const ALIEN_SLAVE_RAKE_DAMAGE: f32 = 25.0;

/// `TWHL:Alien_Grunt`'s published flat armour damage absorption per hit.
pub const AGRUNT_ARMOR_ABSORPTION: f32 = 20.0;

/// `TWHL:Human_Grunt`'s shotgun, per-pellet damage, `[easy, medium, hard]`.
pub const HGRUNT_SHOTGUN_PELLET_DAMAGE: [f32; 3] = [3.0, 5.0, 6.0];

/// `TWHL:Human_Grunt`'s shotgun pellet count (published as `x5` at every
/// difficulty).
pub const HGRUNT_SHOTGUN_PELLET_COUNT: u32 = 5;

/// `TWHL:Human_Grunt`'s thrown grenade; one published value, not
/// skill-scaled.
pub const HGRUNT_GRENADE_DAMAGE: f32 = 100.0;

/// `TWHL:Turret`'s (and the miniturret/sentry family's) published maximum
/// time asleep before it re-checks for a target, in seconds. Not yet wired
/// into a timer.
pub const TURRET_MAX_SLEEP_SECONDS: f32 = 15.0;

/// `TWHL:Turret`'s published turn rate: 30 degrees per 0.1 second, recorded
/// here already converted to degrees per second.
pub const TURRET_TURN_RATE_DEGREES_PER_SECOND: f32 = 300.0;

/// `TWHL:Gargantua`'s ground-stomp shockwave (`brains::GARG_STOMP`),
/// `[easy, medium, hard]`.
pub const GARG_STOMP_DAMAGE: [f32; 3] = [50.0, 100.0, 100.0];

/// `TWHL:Tentacle`'s second, heavier touch-strike level; one published
/// value, not skill-scaled.
pub const TENTACLE_TOUCH2_DAMAGE: f32 = 25.0;

/// `TWHL:Tentacle`'s "beak" strike; one published value, not skill-scaled.
pub const TENTACLE_BEAK_DAMAGE: f32 = 200.0;

/// `TWHL:Tentacle`'s published beak-strike heights above its base, in world
/// units, for the four map-placement variants. Not yet wired into
/// height-based hit detection.
pub const TENTACLE_BEAK_HEIGHTS: [f32; 4] = [0.0, 256.0, 448.0, 640.0];

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

/// One attack's damage table and reach.
///
/// `damage` is cited per-row in [`spec_for`] (see the module doc comment);
/// `range` is cited where TWHL publishes a reach or radius (the houndeye's
/// blast and the bullsquid's unlimited-range spit) and is otherwise a
/// `TODO(black-box)` placeholder, marked as such at each use.
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
/// Health and primary melee/ranged attack numbers are cited from TWHL's
/// per-monster wiki pages (`https://twhl.info/wiki/page/<entity>`; see
/// `docs/FORMAT_SOURCES.md`, "Monster definitions", for the exact page per
/// row). Reach/range figures TWHL does not give, and every timing/turn-rate/
/// FOV number, remain **`TODO(black-box)`**.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one row per defined monster kind, each citing its own published numbers explicitly"
)]
pub fn spec_for(kind: &MonsterKind) -> Option<&'static MonsterSpec> {
    use Classification as C;

    // A tiny helper so every row below states its own damage/range numbers
    // explicitly rather than relying on a shared default that could quietly
    // drift. Returning `Option` (rather than `AttackSpec` directly) matches
    // the `Option<AttackSpec>` field it is always used to fill in below.
    #[allow(clippy::unnecessary_wraps)]
    const fn atk(damage: [f32; 3], range: f32) -> Option<AttackSpec> {
        Some(AttackSpec { damage, range })
    }

    // `TWHL:Headcrab` — health 10/10/20; bite 5/10/10. Melee reach: not
    // published, `TODO(black-box)`.
    static HEADCRAB: MonsterSpec = MonsterSpec {
        classification: C::AlienPrey,
        health: [10.0, 10.0, 20.0],
        melee: atk([5.0, 10.0, 10.0], 48.0),
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Yellow,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::FADES_CORPSE,
    };
    // `TWHL:Zombie` — health 50/50/100; one-hand slash 10/20/20 (the second,
    // both-hands swing at 25/40/40 is `ZOMBIE_BOTH_HANDS_DAMAGE` below).
    // Melee reach: not published, `TODO(black-box)`.
    static ZOMBIE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [50.0, 50.0, 100.0],
        melee: atk([10.0, 20.0, 20.0], 64.0),
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::Yellow,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Houndeye` — health 20/20/30; blast 10/15/15 in a published
    // 192-unit radius (`HOUNDEYE_BLAST_RADIUS`), halved without line of
    // sight (`HOUNDEYE_BLAST_NO_LOS_MULTIPLIER`) and boosted per packmate up
    // to a published cap (`brains::HOUNDEYE_PACK_BONUS_PER_MEMBER`/`_CAP`).
    // No melee attack is published.
    static HOUNDEYE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [20.0, 20.0, 30.0],
        melee: None,
        ranged: atk([10.0, 15.0, 15.0], HOUNDEYE_BLAST_RADIUS),
        hull: Hull::Standing,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::SQUAD_MONSTER,
    };
    // `TWHL:Bullsquid` — health 40/40/120; bite 15/25/25 (the secondary tail
    // whip at 25/35/35 is `BULLSQUID_WHIP_DAMAGE` below); spit 10/10/15,
    // published as unlimited range and unaffected by gravity, represented
    // here as `f32::INFINITY` rather than a finite `TODO(black-box)` guess.
    static BULLSQUID: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [40.0, 40.0, 120.0],
        melee: atk([15.0, 25.0, 25.0], 64.0),
        ranged: atk([10.0, 10.0, 15.0], f32::INFINITY),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Alien_Slave` (vortigaunt) — health 30/30/60; claw 8/10/10 (a
    // secondary "rake" hit at a flat 25 is `ALIEN_SLAVE_RAKE_DAMAGE` below);
    // zap 10/10/15. Published to flee when wounded and alone (not modeled as
    // dedicated logic; the crate's general `SEE_FEAR`/low-health flee path
    // already applies). Zap range: not published, `TODO(black-box)`.
    static ALIEN_SLAVE: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [30.0, 30.0, 60.0],
        melee: atk([8.0, 10.0, 10.0], 48.0),
        ranged: atk([10.0, 10.0, 15.0], 640.0),
        hull: Hull::Standing,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Alien_Grunt` — health 60/90/120; punch 10/20/20; hornet-gun
    // 4/5/8. Armour absorbs a flat 20 points per hit
    // (`AGRUNT_ARMOR_ABSORPTION` below). Reach/range: not published,
    // `TODO(black-box)`.
    static ALIEN_GRUNT: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [60.0, 90.0, 120.0],
        melee: atk([10.0, 20.0, 20.0], 64.0),
        ranged: atk([4.0, 5.0, 8.0], 768.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: true,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Human_Grunt` — health 50/50/80; kick 5/10/10; MP5 3/4/5 (the
    // shotgun pellet/count and fixed-100 grenade values are
    // `HGRUNT_SHOTGUN_PELLET_DAMAGE`/`_PELLET_COUNT`/`HGRUNT_GRENADE_DAMAGE`
    // below). Published squads with flanking, modeled by
    // `brains::GRUNT_FLANK`. Reach/range: not published, `TODO(black-box)`.
    static HUMAN_GRUNT: MonsterSpec = MonsterSpec {
        classification: C::HumanMilitary,
        health: [50.0, 50.0, 80.0],
        melee: atk([5.0, 10.0, 10.0], 48.0),
        ranged: atk([3.0, 4.0, 5.0], 1_024.0),
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::SQUAD_MONSTER.union(MonsterFlags::OPENS_DOORS),
    };
    // `TWHL:Barney` — health 35 (one published value, not skill-scaled);
    // pistol 5/5/8. No melee attack is published. Range: not published,
    // `TODO(black-box)`.
    static BARNEY: MonsterSpec = MonsterSpec {
        classification: C::PlayerAlly,
        health: [35.0, 35.0, 35.0],
        melee: None,
        ranged: atk([5.0, 5.0, 8.0], 1_024.0),
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::OPENS_DOORS,
    };
    // `TWHL:Scientist` — health 20 (one published value, not skill-scaled).
    // No attack of its own; it heals a hurt ally instead (`brains::
    // SCIENTIST_HEAL_AMOUNT`/`_COOLDOWN`/`_RANGE`/`_THRESHOLD_FRACTION`,
    // all cited to the same page).
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
    // `TWHL:Turret` — health 50/50/60; dmg 8/10/10. Published 360-degree
    // vision (`Senses::omnidirectional`), a 15 s max sleep and a 30-degree-
    // per-0.1-s turn rate (`TURRET_MAX_SLEEP_SECONDS`/
    // `TURRET_TURN_RATE_DEGREES_PER_SECOND` below; not wired into a timer
    // yet). Range: not published, `TODO(black-box)`.
    static TURRET: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [50.0, 50.0, 60.0],
        melee: None,
        ranged: atk([8.0, 10.0, 10.0], 1_024.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Miniturret` — health 40/40/50; dmg 5/5/8. Range: not published,
    // `TODO(black-box)`.
    static MINITURRET: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [40.0, 40.0, 50.0],
        melee: None,
        ranged: atk([5.0, 5.0, 8.0], 1_024.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Sentry` — health 40/40/50; dmg 3/4/5. Range: not published,
    // `TODO(black-box)`.
    static SENTRY: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [40.0, 40.0, 50.0],
        melee: None,
        ranged: atk([3.0, 4.0, 5.0], 768.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Ichthyosaur` — health 200/200/400; bite 20/35/50. No ranged
    // attack is published. Reach: not published, `TODO(black-box)`.
    static ICHTHYOSAUR: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [200.0, 200.0, 400.0],
        melee: atk([20.0, 35.0, 50.0], 96.0),
        ranged: None,
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Leech` — health 2 and bite 2 (both single published values, not
    // skill-scaled). Reach: not published, `TODO(black-box)`.
    static LEECH: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [2.0, 2.0, 2.0],
        melee: atk([2.0, 2.0, 2.0], 32.0),
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Green,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Gargantua` — health 800/800/1000; melee 10/30/30; flame
    // 3/5/5 (the ground-stomp shockwave at 50/100/100 is
    // `GARG_STOMP_DAMAGE` in `brains`). Published immune to everything
    // except energy/crush/mortar/blast damage types — `ohl-ai`'s minimal
    // `DamageEvent` carries no damage-type bitflags yet (see `crate::damage`
    // module docs), so that immunity is not modeled here; it is
    // `ohl-combat`'s `DamageInfo` unification's job. Reach/range: not
    // published, `TODO(black-box)`.
    static GARGANTUA: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [800.0, 800.0, 1_000.0],
        melee: atk([10.0, 30.0, 30.0], 96.0),
        ranged: atk([3.0, 5.0, 5.0], 512.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: true,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Tentacle` — health 75 (retreats rather than dying; see
    // `brains::ROOTED_LISTEN`/`TENTACLE_STRIKE`); touch 20 (a second touch
    // level at 25 and the heavier "beak" strike at a flat 200 are
    // `TENTACLE_TOUCH2_DAMAGE`/`TENTACLE_BEAK_DAMAGE` below); reach
    // published as "~336" units. Beak strike heights (+0/+256/+448/+640) are
    // `TENTACLE_BEAK_HEIGHTS`, not yet wired into height-based hit
    // detection.
    static TENTACLE: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [75.0, 75.0, 75.0],
        melee: atk([20.0, 20.0, 20.0], 336.0),
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

    /// A checked-in expectation list, transcribed from the same TWHL pages
    /// cited per-row in [`spec_for`] (see the module doc comment and
    /// `docs/FORMAT_SOURCES.md`, "Monster definitions"); the *test name* is
    /// this project's own, but the health values below are the published
    /// ones, not an invented fixture. Also asserts the table is internally
    /// consistent and every kind is exercised.
    #[test]
    fn every_defined_kind_resolves_to_its_cited_health() {
        let expected: &[(MonsterKind, [f32; 3])] = &[
            (MonsterKind::Headcrab, [10.0, 10.0, 20.0]),
            (MonsterKind::Zombie, [50.0, 50.0, 100.0]),
            (MonsterKind::Houndeye, [20.0, 20.0, 30.0]),
            (MonsterKind::Bullsquid, [40.0, 40.0, 120.0]),
            (MonsterKind::AlienSlave, [30.0, 30.0, 60.0]),
            (MonsterKind::AlienGrunt, [60.0, 90.0, 120.0]),
            (MonsterKind::HumanGrunt, [50.0, 50.0, 80.0]),
            (MonsterKind::Barney, [35.0, 35.0, 35.0]),
            (MonsterKind::Scientist, [20.0, 20.0, 20.0]),
            (MonsterKind::Turret, [50.0, 50.0, 60.0]),
            (MonsterKind::MiniTurret, [40.0, 40.0, 50.0]),
            (MonsterKind::Sentry, [40.0, 40.0, 50.0]),
            (MonsterKind::Ichthyosaur, [200.0, 200.0, 400.0]),
            (MonsterKind::Leech, [2.0, 2.0, 2.0]),
            (MonsterKind::Gargantua, [800.0, 800.0, 1_000.0]),
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

    /// The published primary melee/ranged attack-damage tables, transcribed
    /// from the same TWHL pages cited in [`spec_for`].
    #[test]
    fn every_defined_kind_resolves_to_its_cited_attack_damage() {
        let melee_expected: &[(MonsterKind, [f32; 3])] = &[
            (MonsterKind::Headcrab, [5.0, 10.0, 10.0]),
            (MonsterKind::Zombie, [10.0, 20.0, 20.0]),
            (MonsterKind::Bullsquid, [15.0, 25.0, 25.0]),
            (MonsterKind::AlienSlave, [8.0, 10.0, 10.0]),
            (MonsterKind::AlienGrunt, [10.0, 20.0, 20.0]),
            (MonsterKind::HumanGrunt, [5.0, 10.0, 10.0]),
            (MonsterKind::Ichthyosaur, [20.0, 35.0, 50.0]),
            (MonsterKind::Leech, [2.0, 2.0, 2.0]),
            (MonsterKind::Gargantua, [10.0, 30.0, 30.0]),
            (MonsterKind::Tentacle, [20.0, 20.0, 20.0]),
        ];
        for (kind, damage) in melee_expected {
            let spec = spec_for(kind).unwrap_or_else(|| panic!("{kind:?} is defined"));
            let melee = spec
                .melee
                .unwrap_or_else(|| panic!("{kind:?} has a published melee attack"));
            for difficulty in Difficulty::ALL {
                assert!(
                    (melee.resolve_damage(difficulty, "sk_x_dmg", None)
                        - damage[difficulty.index()])
                    .abs()
                        < 1e-6
                );
            }
        }

        let ranged_expected: &[(MonsterKind, [f32; 3])] = &[
            (MonsterKind::Houndeye, [10.0, 15.0, 15.0]),
            (MonsterKind::Bullsquid, [10.0, 10.0, 15.0]),
            (MonsterKind::AlienSlave, [10.0, 10.0, 15.0]),
            (MonsterKind::AlienGrunt, [4.0, 5.0, 8.0]),
            (MonsterKind::HumanGrunt, [3.0, 4.0, 5.0]),
            (MonsterKind::Barney, [5.0, 5.0, 8.0]),
            (MonsterKind::Turret, [8.0, 10.0, 10.0]),
            (MonsterKind::MiniTurret, [5.0, 5.0, 8.0]),
            (MonsterKind::Sentry, [3.0, 4.0, 5.0]),
            (MonsterKind::Gargantua, [3.0, 5.0, 5.0]),
        ];
        for (kind, damage) in ranged_expected {
            let spec = spec_for(kind).unwrap_or_else(|| panic!("{kind:?} is defined"));
            let ranged = spec
                .ranged
                .unwrap_or_else(|| panic!("{kind:?} has a published ranged attack"));
            for difficulty in Difficulty::ALL {
                assert!(
                    (ranged.resolve_damage(difficulty, "sk_x_dmg", None)
                        - damage[difficulty.index()])
                    .abs()
                        < 1e-6
                );
            }
        }
    }

    /// The bullsquid's spit is published as unlimited range; represented as
    /// `f32::INFINITY` rather than a finite guess.
    #[test]
    fn the_bullsquids_spit_range_is_unlimited() {
        let spec = spec_for(&MonsterKind::Bullsquid).expect("defined");
        assert!(
            spec.ranged
                .expect("published spit attack")
                .range
                .is_infinite()
        );
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
