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

use crate::damage::{DamageKinds, DamageResponse};
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

// Wave 1 batch A: barnacle, alien controller, human assassin, babycrab,
// generic, furniture, rat, cockroach.

/// `TWHL:Monster_barnacle`'s published tongue reach: "its real range goes
/// 2048 units below his origin position", in world units. This is the
/// length of the vertical tongue trace [`crate::monsters::MonsterBrain`]
/// uses as the barnacle's melee reach (`brains::MonsterBrain::
/// melee_in_reach`), and the barnacle's look distance.
pub const BARNACLE_TONGUE_LENGTH: f32 = 2_048.0;

/// How far off the vertical line below a barnacle a victim may stand and
/// still be counted as touching the tongue, in world units.
/// **`TODO(black-box)`**: the tongue's width is not published; this is a
/// project placeholder, roughly one humanoid hull width.
pub const BARNACLE_TONGUE_RADIUS: f32 = 32.0;

/// `TWHL:Monster_barnacle`'s published feeding time: "It takes 10 seconds
/// for a barnacle to kill its prey." Recorded as a fact; the bite cadence
/// `brains::BARNACLE_BITE_INTERVAL` derives from it is this project's own
/// reading of that sentence.
pub const BARNACLE_KILL_SECONDS: f32 = 10.0;

/// `TWHL:Monster_alien_controller`'s published head-launched homing ball
/// damage, `[easy, medium, hard]`. Not yet wired: `MonsterSpec` carries one
/// ranged attack, and the hand-launched energy-ball volley (the row's
/// `ranged`) is the one a controller uses at range.
pub const CONTROLLER_HEAD_BALL_DAMAGE: [f32; 3] = [15.0, 25.0, 35.0];

/// `TWHL:Monster_alien_controller`'s published homing ball speed,
/// `[easy, medium, hard]`, in units per second. Not yet wired (see
/// [`CONTROLLER_HEAD_BALL_DAMAGE`]).
pub const CONTROLLER_HEAD_BALL_SPEED: [f32; 3] = [650.0, 800.0, 1_000.0];

/// `TWHL:Monster_human_assassin`'s thrown grenade; one published value, not
/// skill-scaled. Not yet wired, for the same reason as
/// [`HGRUNT_GRENADE_DAMAGE`].
pub const ASSASSIN_GRENADE_DAMAGE: f32 = 100.0;

/// `TWHL:Monster_babycrab`'s published health as a fraction of a
/// headcrab's: "only 25% as much health as a normal headcrab".
/// [`MonsterSpec::resolve_health`] applies it to the headcrab row so a
/// `skill.cfg` override of `sk_headcrab_health<N>` scales the babycrab
/// too, rather than the babycrab needing a cvar of its own that no
/// published convention names.
pub const BABYCRAB_HEALTH_FRACTION: f32 = 0.25;

/// `TWHL:Monster_babycrab`'s published bite as a fraction of a
/// headcrab's: "only 30% as much damage".
pub const BABYCRAB_DAMAGE_FRACTION: f32 = 0.3;

/// `TWHL:Monster_generic`'s published health: "Spawns with only 8 HP."
pub const GENERIC_HEALTH: f32 = 8.0;

/// `TWHL:Monster_gargantua`'s published vulnerability: "Gargantuas are only
/// vulnerable to energy-beam, crush, mortar, and blast damage". A mortar
/// strike is delivered as blast damage in this project's vocabulary
/// (`docs/FORMAT_SOURCES.md`, "Combat and damage": mortar is not one of
/// the published damage *types*), so the set is the remaining three.
pub const GARGANTUA_VULNERABILITY: DamageKinds = DamageKinds::ENERGYBEAM
    .union(DamageKinds::CRUSH)
    .union(DamageKinds::BLAST);

// Wave 1 batch B — bosses and aircraft. Every number below is cited to the
// monster's own TWHL page (`TWHL:Monster_bigmomma`, `TWHL:Info_bigmomma`,
// `TWHL:Monster_nihilanth`, `TWHL:Monster_apache`, `TWHL:Monster_osprey`)
// or, where noted, to Combine OverWiki; see `docs/FORMAT_SOURCES.md`,
// "Monster definitions", "Wave 1 batch B".

/// `TWHL:Monster_bigmomma`: the Gonarch "has 150 health by default"; its
/// per-difficulty health is that base times [`BIGMOMMA_HEALTH_FACTOR`].
pub const BIGMOMMA_BASE_HEALTH: f32 = 150.0;

/// `TWHL:Monster_bigmomma`'s published health multiplier per difficulty,
/// `[easy, medium, hard]` = 1x / 1.5x / 2x. Also what an `info_bigmomma`
/// node's own `health` keyvalue is scaled by on approach
/// (`crate::monsters::bigmomma`).
pub const BIGMOMMA_HEALTH_FACTOR: [f32; 3] = [1.0, 1.5, 2.0];

/// `TWHL:Monster_bigmomma`'s acid-mortar blast radius, `[easy, medium,
/// hard]`, in world units.
pub const BIGMOMMA_BLAST_RADIUS: [f32; 3] = [250.0, 250.0, 275.0];

/// `TWHL:Monster_bigmomma`: "At most 20 spawned baby headcrabs can be alive
/// at any given time." Recorded, not applied: the Gonarch does not yet
/// spawn the babies (`monster_babycrab` has a row of its own, but nothing
/// here births one).
pub const BIGMOMMA_MAX_LIVE_BABYCRABS: u32 = 20;

/// `TWHL:Monster_nihilanth`: the number of health-reserve sprites floating
/// about it, "each holding 1/20th of its health".
pub const NIHILANTH_SPHERE_COUNT: u32 = 20;

/// `TWHL:Monster_nihilanth`'s published "reserve per sprite", `[easy,
/// medium, hard]` — exactly its health table divided by
/// [`NIHILANTH_SPHERE_COUNT`].
pub const NIHILANTH_SPHERE_RESERVE: [f32; 3] = [40.0, 40.0, 50.0];

/// `TWHL:Monster_apache`'s rocket damage, published as a single 150 at
/// every difficulty (the machine gun is the `ranged` row). Not yet wired:
/// `MonsterSpec` carries one ranged attack, and the engine resolves a
/// second one at the first one's damage, the same reason
/// [`CONTROLLER_HEAD_BALL_DAMAGE`] and [`ASSASSIN_GRENADE_DAMAGE`] are not
/// wired. The Apache fires its machine gun only.
pub const APACHE_ROCKET_DAMAGE: f32 = 150.0;

/// `TWHL:Monster_apache`: "will also start emitting smoke when its health
/// falls below 100". Recorded, not applied (no smoke is drawn).
pub const APACHE_SMOKE_HEALTH: f32 = 100.0;

/// `TWHL:Monster_apache`: "Normally when killed, falls ... for 12 seconds
/// or until hitting ground before disintegrating." Recorded, not applied:
/// a dead aircraft stops where it died.
pub const APACHE_WRECK_FALL_SECONDS: f32 = 12.0;

/// `TWHL:Monster_apache`: "The NoWreckage flag reduces the time to 4
/// seconds." Recorded, not applied (see [`APACHE_WRECK_FALL_SECONDS`]).
pub const APACHE_NO_WRECKAGE_FALL_SECONDS: f32 = 4.0;

/// `TWHL:Monster_apache`'s published `NoWreckage` spawnflag bit
/// ("Explodes in mid-air"). Recorded, not read.
pub const SPAWNFLAG_APACHE_NO_WRECKAGE: u32 = 8;

/// The published `Start Inactive` spawnflag bit shared by `monster_apache`
/// ("Must be triggered to start") and `monster_osprey` ("Requires the
/// Osprey to be triggered to start"); read by
/// `crate::monsters::bosses::attach`.
pub const SPAWNFLAG_AIRCRAFT_START_INACTIVE: u32 = 64;

/// `TWHL:Monster_apache`: "blast damage doubles damage". Applied through
/// [`damage_response_for`]; the same sentence's "the cockpit and engines
/// are more easily damaged" needs per-hitbox routing and is not.
pub const APACHE_DOUBLED_BY: DamageKinds = DamageKinds::BLAST;

/// `TWHL:Monster_osprey`: "An Osprey can track and replace up to 24
/// soldiers (these soldiers will be replaced indefinitely)". Recorded, not
/// applied: the Osprey's soldier drops are not modelled.
pub const OSPREY_MAX_SOLDIERS: u32 = 24;

/// `TWHL:Monster_osprey`: "Shots that deal less than 50 damage are only
/// effective when they hit the cockpit or one of the engines". Recorded,
/// not applied: per-hitbox routing is not modelled.
pub const OSPREY_WEAK_HIT_THRESHOLD: f32 = 50.0;

/// `TWHL:Monster_osprey`: "each engine only takes up to 200 damage before
/// shots become ineffective". Recorded, not yet modelled (same seam).
pub const OSPREY_ENGINE_DAMAGE_CAP: f32 = 200.0;

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
    // Wave 1 batch A.
    /// Never moves at all (the barnacle): no schedule its brain selects
    /// carries a movement task, and it takes no cover. (There is no
    /// matching "flies" flag: whether a monster flies is decided by its
    /// hull alone, `crate::movement::flies`.)
    pub const ROOTED: Self = Self(1 << 4);

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
    /// `monster_bullsquid` (also spawned by the published alias
    /// `monster_bullchicken`; see [`Self::from_classname`]).
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
    // Wave 1 batch A.
    /// `monster_barnacle`.
    Barnacle,
    /// `monster_alien_controller`.
    AlienController,
    /// `monster_human_assassin`.
    HumanAssassin,
    /// `monster_babycrab`.
    Babycrab,
    /// `monster_generic`.
    Generic,
    /// `monster_furniture`.
    Furniture,
    /// `monster_rat`.
    Rat,
    /// `monster_cockroach`.
    Cockroach,
    // Wave 1 batch B.
    /// `monster_bigmomma` (the Gonarch).
    BigMomma,
    /// `monster_nihilanth` (the final boss).
    Nihilanth,
    /// `monster_apache` (attack helicopter).
    Apache,
    /// `monster_osprey` (troop transport).
    Osprey,
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
            // Wave 1 batch A.
            Self::Barnacle => "monster_barnacle",
            Self::AlienController => "monster_alien_controller",
            Self::HumanAssassin => "monster_human_assassin",
            Self::Babycrab => "monster_babycrab",
            Self::Generic => "monster_generic",
            Self::Furniture => "monster_furniture",
            Self::Rat => "monster_rat",
            Self::Cockroach => "monster_cockroach",
            // Wave 1 batch B.
            Self::BigMomma => "monster_bigmomma",
            Self::Nihilanth => "monster_nihilanth",
            Self::Apache => "monster_apache",
            Self::Osprey => "monster_osprey",
            Self::Unknown(classname) => classname,
        }
    }

    /// The kind for a map entity's `classname`, or [`Self::Unknown`] when it
    /// is not one of the kinds this table defines.
    ///
    /// `monster_bullchicken` is accepted as an alias for [`Self::Bullsquid`]:
    /// TWHL's "Reference: Entities and their models"
    /// (<https://twhl.info/wiki/page/Reference:_Entities_and_their_models>;
    /// see `docs/FORMAT_SOURCES.md`, "Monster definitions") lists this
    /// monster's row under the classname `monster_bullchicken`, GoldSrc's
    /// own internal name for the asset, rather than `monster_bullsquid`;
    /// both classnames are accepted here so either published spelling
    /// resolves to the same kind.
    #[must_use]
    pub fn from_classname(classname: &str) -> Self {
        match classname {
            "monster_headcrab" => Self::Headcrab,
            "monster_zombie" => Self::Zombie,
            "monster_houndeye" => Self::Houndeye,
            "monster_bullsquid" | "monster_bullchicken" => Self::Bullsquid,
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
            // Wave 1 batch A.
            "monster_barnacle" => Self::Barnacle,
            "monster_alien_controller" => Self::AlienController,
            "monster_human_assassin" => Self::HumanAssassin,
            "monster_babycrab" => Self::Babycrab,
            "monster_generic" => Self::Generic,
            "monster_furniture" => Self::Furniture,
            "monster_rat" => Self::Rat,
            "monster_cockroach" => Self::Cockroach,
            // Wave 1 batch B.
            "monster_bigmomma" => Self::BigMomma,
            "monster_nihilanth" => Self::Nihilanth,
            "monster_apache" => Self::Apache,
            "monster_osprey" => Self::Osprey,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// The `sk_<subject>` cvar stem this kind's skill.cfg entries use.
    /// Public, widely mirrored skill.cfg subject names (independent of the
    /// unverifiable *values* discussed in the module doc comment — the
    /// naming convention itself is corroborated by multiple sources and by
    /// this project's own `ohl_formats::skill_cfg` reader).
    ///
    /// The Wave 1 batch A stems (`barnacle`, `controller`, `hassassin`)
    /// follow the same `sk_<subject>` convention but were **not** verified
    /// against a vanilla `skill.cfg` (none is reachable from this
    /// environment; see the module doc comment): they are this project's
    /// own choice of stem, so a caller's override lookup that happens to
    /// use another spelling simply falls back to the table. The babycrab
    /// deliberately reuses the headcrab's stem, since its published numbers
    /// are fractions of the headcrab's ([`BABYCRAB_HEALTH_FRACTION`]);
    /// `monster_generic`, `monster_furniture`, `monster_rat` and
    /// `monster_cockroach` have no skill entries at all, so their stems
    /// are the classnames themselves, which no `skill.cfg` will match.
    /// The four batch-B stems (`bigmomma`, `nihilanth`, `apache`,
    /// `osprey`) are this project's own application of that convention to
    /// the classname stem, since none of the cited pages lists the cvars
    /// themselves; a `skill.cfg` that spells them differently simply leaves
    /// the table value in force. `TODO(black-box)`.
    #[must_use]
    pub fn skill_subject(&self) -> &str {
        match self {
            // Wave 1 batch A.
            Self::Barnacle => "barnacle",
            Self::AlienController => "controller",
            Self::HumanAssassin => "hassassin",
            Self::Generic => "monster_generic",
            Self::Furniture => "monster_furniture",
            Self::Rat => "monster_rat",
            Self::Cockroach => "monster_cockroach",
            Self::Headcrab | Self::Babycrab => "headcrab",
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
            // Wave 1 batch B.
            Self::BigMomma => "bigmomma",
            Self::Nihilanth => "nihilanth",
            Self::Apache => "apache",
            Self::Osprey => "osprey",
            Self::Unknown(classname) => classname,
        }
    }

    /// The `models/<name>.mdl` path GoldSrc's own monster class hardcodes
    /// as its default studio model (`None` for [`Self::Unknown`]).
    ///
    /// Unlike every other keyvalue this project loads, a `monster_*`
    /// entity's model is **not** authored in the map's entity lump for the
    /// defined kinds below (`monster_generic`/`monster_furniture`
    /// are the documented exception: the cited page lists their model as
    /// "specified by mapper", they carry an explicit `model` keyvalue the
    /// caller reads directly — see `ohl_engine::level::wants_studio_model`'s
    /// doc comment — and so they return `None` here like `Unknown`). The
    /// monster's own `Spawn`/`Precache` code picks the model instead, so a
    /// clean-room engine that only reads the map's `model` keyvalue never
    /// finds one for these classnames and never draws them, even though
    /// they are simulated normally. This table is this project's
    /// replacement for that hardcoded choice: one path per kind. Every path
    /// is transcribed from a single public page, TWHL's "Reference:
    /// Entities and their models"; see `docs/FORMAT_SOURCES.md`, "Monster
    /// definitions" for the full per-kind citation table, the exact URL,
    /// and an explicit statement that none of these names came from a
    /// payload listing.
    #[must_use]
    pub fn default_model_path(&self) -> Option<&'static str> {
        Some(match self {
            Self::Headcrab => "models/headcrab.mdl",
            Self::Zombie => "models/zombie.mdl",
            Self::Houndeye => "models/houndeye.mdl",
            Self::Bullsquid => "models/bullsquid.mdl",
            Self::AlienSlave => "models/islave.mdl",
            Self::AlienGrunt => "models/agrunt.mdl",
            Self::HumanGrunt => "models/hgrunt.mdl",
            Self::Barney => "models/barney.mdl",
            Self::Scientist => "models/scientist.mdl",
            Self::Turret => "models/turret.mdl",
            Self::MiniTurret => "models/miniturret.mdl",
            Self::Sentry => "models/sentry.mdl",
            // `skill_subject`'s stem for this kind is `"ichthyosaur"`,
            // which does not double as this filename; see the cited table.
            Self::Ichthyosaur => "models/icky.mdl",
            Self::Leech => "models/leech.mdl",
            Self::Gargantua => "models/garg.mdl",
            // `skill_subject`'s stem for this kind is `"tentacle"`, which
            // does not double as this filename; see the cited table.
            Self::Tentacle => "models/tentacle2.mdl",
            // Wave 1 batch A. `skill_subject`'s stems for the controller
            // and the assassin (`controller`, `hassassin`) happen to equal
            // these filenames' stems, but neither was derived from the
            // other: the paths come from the cited model table, the cvar
            // stems are this project's own unverified choice (see
            // `skill_subject`).
            Self::Barnacle => "models/barnacle.mdl",
            Self::AlienController => "models/controller.mdl",
            Self::HumanAssassin => "models/hassassin.mdl",
            Self::Babycrab => "models/baby_headcrab.mdl",
            Self::Rat => "models/bigrat.mdl",
            Self::Cockroach => "models/roach.mdl",
            // Wave 1 batch B: the same cited page's rows.
            Self::BigMomma => "models/big_mom.mdl",
            Self::Nihilanth => "models/nihilanth.mdl",
            Self::Apache => "models/apache.mdl",
            Self::Osprey => "models/osprey.mdl",
            Self::Generic | Self::Furniture | Self::Unknown(_) => return None,
        })
    }

    /// Whether a map's `TriggerCondition`/`TriggerTarget` pair works on
    /// this kind. Every TWHL `monster_*` page lists the pair, but three say
    /// it does not work: `TWHL:Monster_nihilanth` ("TriggerCondition is not
    /// working for this monster"), `TWHL:Monster_apache` and
    /// `TWHL:Monster_osprey` ("trigger condition" and "trigger target" do
    /// not function / "will not work"). `ohl-engine` collects no trigger
    /// for those three.
    #[must_use]
    pub fn honours_trigger_condition(&self) -> bool {
        !matches!(self, Self::Nihilanth | Self::Apache | Self::Osprey)
    }

    /// Whether this kind's model is authored by the map (its `model`
    /// keyvalue) rather than hardcoded per species: `true` exactly for the
    /// two kinds the cited model table lists as "specified by mapper",
    /// which is why [`Self::default_model_path`] has nothing for them.
    #[must_use]
    pub fn model_from_map(&self) -> bool {
        matches!(self, Self::Generic | Self::Furniture)
    }

    /// The model-local fallback eye before runtime metadata is available.
    /// Uses the same derived body policy as map, maker and restored actors.
    /// Exact anatomy remains `TODO(black-box)`; see [`crate::BodyFrame`].
    #[must_use]
    pub fn view_offset(&self) -> glam::Vec3 {
        let hull = spec_for(self).map_or(ohl_physics::Hull::Standing, |spec| spec.hull);
        crate::BodyFrame::for_model(self, hull, None).eye_offset(hull, None)
    }

    /// Every defined kind (not [`Self::Unknown`]), in table order.
    #[must_use]
    pub fn defined() -> &'static [MonsterKind] {
        const KINDS: [MonsterKind; 28] = [
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
            // Wave 1 batch A.
            MonsterKind::Barnacle,
            MonsterKind::AlienController,
            MonsterKind::HumanAssassin,
            MonsterKind::Babycrab,
            MonsterKind::Generic,
            MonsterKind::Furniture,
            MonsterKind::Rat,
            MonsterKind::Cockroach,
            // Wave 1 batch B.
            MonsterKind::BigMomma,
            MonsterKind::Nihilanth,
            MonsterKind::Apache,
            MonsterKind::Osprey,
        ];
        &KINDS
    }
}

/// How `kind`'s health answers each damage type.
///
/// [`DamageResponse::ORDINARY`] for every monster without a published
/// rule; the gargantua is hurt only by [`GARGANTUA_VULNERABILITY`], and the
/// Apache takes double from [`APACHE_DOUBLED_BY`]. Kept as a function
/// beside [`spec_for`] rather than a `MonsterSpec` field so each rule is an
/// explicit, cited exception and every other row stays untouched. A hit a
/// species shrugs off still *registers* — the monster turns toward and
/// acquires its attacker — it just costs no health; see
/// `crate::damage::summarize_effective` and `crate::monsters::lifecycle`.
#[must_use]
pub fn damage_response_for(kind: &MonsterKind) -> DamageResponse {
    match kind {
        MonsterKind::Gargantua => DamageResponse {
            vulnerable_to: GARGANTUA_VULNERABILITY,
            doubled_by: DamageKinds::NONE,
        },
        MonsterKind::Apache => DamageResponse {
            vulnerable_to: DamageKinds::ALL,
            doubled_by: APACHE_DOUBLED_BY,
        },
        _ => DamageResponse::ORDINARY,
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
    ///
    /// The babycrab is published as a fraction of the headcrab
    /// ([`BABYCRAB_HEALTH_FRACTION`]), so its override is the *headcrab's*
    /// `sk_headcrab_health<N>` scaled by that fraction: an override of the
    /// parent scales the child, and the babycrab's own table row is only
    /// ever what that product already equals.
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
        if matches!(kind, MonsterKind::Babycrab)
            && let Some(headcrab) = spec_for(&MonsterKind::Headcrab)
        {
            return resolve_stat(headcrab.health, difficulty, &cvar, skill)
                * BABYCRAB_HEALTH_FRACTION;
        }
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
    // `GARG_STOMP_DAMAGE` in `brains`). Published "only vulnerable to
    // energy-beam, crush, mortar, and blast damage": modelled by
    // `damage_response_for` narrowing it to `GARGANTUA_VULNERABILITY`,
    // matched against `DamageEvent::kinds`. Reach/range: not published,
    // `TODO(black-box)`.
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

    // --- Wave 1 batch A ---------------------------------------------------

    // `TWHL:Monster_barnacle` — health 25 (one published value, not
    // skill-scaled); "Barnacles can be killed with a single hit from the
    // crowbar". Its "attack" is the tongue: published as reaching 2048
    // units straight down (`BARNACLE_TONGUE_LENGTH`, the melee reach here;
    // `brains::MonsterBrain::melee_in_reach` turns that into a vertical
    // test rather than a sphere), and as killing its prey in 10 seconds
    // (`BARNACLE_KILL_SECONDS`), with a player "killed in 1 to 3 bites
    // depending on how much armor they have". Bite damage per hit is
    // `TODO(black-box)`: the flat 40 below is a placeholder chosen to sit
    // inside that published one-to-three-bites bound for a 100-health
    // player, not a cited number. Published to "ignore other monsters":
    // the relationship table's barnacle row only hates the player's side.
    static BARNACLE: MonsterSpec = MonsterSpec {
        classification: C::Barnacle,
        health: [25.0, 25.0, 25.0],
        melee: atk([40.0, 40.0, 40.0], BARNACLE_TONGUE_LENGTH),
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES.union(MonsterFlags::ROOTED),
    };
    // `TWHL:Monster_alien_controller` — health 60/60/100; hand-launched
    // energy-ball volley ("zap") 3/4/5 (the head-launched homing ball at
    // 15/25/35 and its 650/800/1000 speed are `CONTROLLER_HEAD_BALL_DAMAGE`
    // /`_SPEED` above, not yet wired). Published as flying ("Can't move
    // unless an `info_node_air` is nearby"), evasive and keeping its
    // distance; the point hull is the flight seam (`crate::movement::flies`,
    // and `ohl-nav`'s steering), `brains::CONTROLLER_VOLLEY` the
    // fire-then-reposition. No melee
    // attack is published. Range: not published, `TODO(black-box)`.
    static ALIEN_CONTROLLER: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [60.0, 60.0, 100.0],
        melee: None,
        ranged: atk([3.0, 4.0, 5.0], 1_024.0),
        hull: Hull::Point,
        blood: BloodKind::Green,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::EMPTY,
    };
    // `TWHL:Monster_human_assassin` — health 30/50/50; silenced pistol
    // 5/5/8 (grenade 100 flat is `ASSASSIN_GRENADE_DAMAGE` above, not yet
    // wired). Published as fast and agile, running and jumping to avoid
    // fire and attacking from multiple directions, in small teams
    // (`SQUAD_MONSTER`); `brains::ASSASSIN_HIT_AND_RUN` is the fire-then-
    // relocate reading of that, and `brains::ASSASSIN_RUN_SPEED` the "fast".
    // No melee attack is published. Range: not published, `TODO(black-box)`.
    static HUMAN_ASSASSIN: MonsterSpec = MonsterSpec {
        classification: C::HumanMilitary,
        health: [30.0, 50.0, 50.0],
        melee: None,
        ranged: atk([5.0, 5.0, 8.0], 1_024.0),
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: true,
        flags: MonsterFlags::SQUAD_MONSTER
            .union(MonsterFlags::OPENS_DOORS)
            .union(MonsterFlags::FADES_CORPSE),
    };
    // `TWHL:Monster_babycrab` — health 2.5/2.5/5 and bite 1.5/3/3, both
    // published as fractions of the headcrab's ("only 25% as much health",
    // "only 30% as much damage": `BABYCRAB_HEALTH_FRACTION`/
    // `BABYCRAB_DAMAGE_FRACTION`), which is exactly what these rows are.
    // Everything else — hull, blood, brain — is the headcrab's. Melee
    // reach: not published, `TODO(black-box)` (the headcrab's placeholder).
    static BABYCRAB: MonsterSpec = MonsterSpec {
        classification: C::AlienPrey,
        health: [2.5, 2.5, 5.0],
        melee: atk([1.5, 3.0, 3.0], 48.0),
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Yellow,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::FADES_CORPSE,
    };
    // `TWHL:Monster_generic` — "Spawns with only 8 HP" (`GENERIC_HEALTH`);
    // "Classified as a player ally"; "Used to spawn models for use with
    // scripted sequences", its model coming from the map's own `model`
    // keyvalue (`MonsterKind::model_from_map`). No attack of its own is
    // modeled (the page's "will try to attack ... if any animation with the
    // appropriate ACTs exists" depends on the mapper's model, which this
    // table cannot know). `NEVER_FLEES`: a prop for a script stands where
    // it was put.
    static GENERIC: MonsterSpec = MonsterSpec {
        classification: C::PlayerAlly,
        health: [GENERIC_HEALTH, GENERIC_HEALTH, GENERIC_HEALTH],
        melee: None,
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::Red,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Monster_furniture` — "a furniture model used in scripted
    // sequences", model from the map's `model` keyvalue; "still bleeds
    // like a cycler when hit with explosion damage", so it is damageable,
    // but no health value is published: the 8 below mirrors
    // `monster_generic`'s and is `TODO(black-box)`. No classification is
    // published either; `Classification::None` (takes part in no
    // relationship) is the conservative reading of a piece of furniture.
    // Published as not turning to face its path, hence `ROOTED` (its brain
    // never turns or walks it; a script driving it is unaffected).
    static FURNITURE: MonsterSpec = MonsterSpec {
        classification: C::None,
        health: [GENERIC_HEALTH, GENERIC_HEALTH, GENERIC_HEALTH],
        melee: None,
        ranged: None,
        hull: Hull::Standing,
        blood: BloodKind::None,
        size: SizeClass::Medium,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES.union(MonsterFlags::ROOTED),
    };
    // `TWHL:Monster_rat` — "it doesn't do much"; can wander erratically
    // along a patrol path. No health, damage or classification is
    // published: health 1 (dies to any hit) and `Classification::None` are
    // `TODO(black-box)` placeholders. Harmless: no attack.
    static RAT: MonsterSpec = MonsterSpec {
        classification: C::None,
        health: [1.0, 1.0, 1.0],
        melee: None,
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::Red,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::FADES_CORPSE,
    };
    // `TWHL:Monster_cockroach` — "scurry around in the dark ... They are
    // easily scared"; "Stepping on them will kill them". The published
    // classification vocabulary has an `insect` class, used here
    // (`TODO(black-box)`: the page does not say so outright). Health 1 is
    // the "any hit kills" reading of the stepping sentence, not a cited
    // number. Harmless: no attack.
    static COCKROACH: MonsterSpec = MonsterSpec {
        classification: C::Insect,
        health: [1.0, 1.0, 1.0],
        melee: None,
        ranged: None,
        hull: Hull::Crouched,
        blood: BloodKind::None,
        size: SizeClass::Small,
        can_open_doors: false,
        flags: MonsterFlags::FADES_CORPSE,
    };
    // Wave 1 batch B — bosses and aircraft.
    //
    // `TWHL:Monster_bigmomma` (the Gonarch) — 150 base health times the
    // published 1x/1.5x/2x per-difficulty factor (`BIGMOMMA_BASE_HEALTH`,
    // `BIGMOMMA_HEALTH_FACTOR`), so 150/225/300; claw slash 50/60/70; acid
    // mortar 100/120/160 in a 250/250/275 radius (`BIGMOMMA_BLAST_RADIUS`).
    // Classified "Alien Monster" by `TWHL:Reference:_Monster_classifications`.
    // Headcrab family, so yellow blood by the documented convention. Its
    // `info_bigmomma` trail (invulnerable on the trail, killable only at
    // its end) is `crate::monsters::bigmomma`. Reach/mortar range: not
    // published, `TODO(black-box)`.
    static BIGMOMMA: MonsterSpec = MonsterSpec {
        classification: C::AlienMonster,
        health: [150.0, 225.0, 300.0],
        melee: atk([50.0, 60.0, 70.0], 128.0),
        ranged: atk([100.0, 120.0, 160.0], 1_024.0),
        hull: Hull::Large,
        blood: BloodKind::Yellow,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Monster_nihilanth` — health 800/800/1000; zap 30 (one
    // published value, not skill-scaled); a reserve of 20 sprites each
    // holding 1/20th of its health, 40/40/50 (`NIHILANTH_SPHERE_COUNT`,
    // `NIHILANTH_SPHERE_RESERVE`), refilled from its chamber's recharger
    // crystals — `crate::monsters::nihilanth`. Classified "Alien Military"
    // by the same reference page. It never moves: no schedule its brain
    // selects carries a movement task, so it hangs where the map put it,
    // and its hull is never traced (the large hull is the nearest box to
    // its size). No melee attack is published. Zap range: not published,
    // `TODO(black-box)`.
    static NIHILANTH: MonsterSpec = MonsterSpec {
        classification: C::AlienMilitary,
        health: [800.0, 800.0, 1_000.0],
        melee: None,
        ranged: atk([30.0, 30.0, 30.0], 2_048.0),
        hull: Hull::Large,
        blood: BloodKind::Green,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Monster_apache` — health 150/250/400; machine gun 8/10/10
    // (the rocket's 150 is `APACHE_ROCKET_DAMAGE`, not wired); blast
    // damage doubled (`APACHE_DOUBLED_BY`). Classified "Human Military". A
    // machine: no blood. Flies a cyclic `path_corner` route from its
    // `target` (`crate::monsters::aircraft`) on the point hull, the flight
    // seam (`crate::movement::flies`). Gun range: not published,
    // `TODO(black-box)`.
    static APACHE: MonsterSpec = MonsterSpec {
        classification: C::HumanMilitary,
        health: [150.0, 250.0, 400.0],
        melee: None,
        ranged: atk([8.0, 10.0, 10.0], 2_048.0),
        hull: Hull::Point,
        blood: BloodKind::None,
        size: SizeClass::Large,
        can_open_doors: false,
        flags: MonsterFlags::NEVER_FLEES,
    };
    // `TWHL:Monster_osprey` — health 400 (one published value, not
    // skill-scaled); no attack of its own. Classified "Machine". Flies a
    // cyclic `path_corner` route on the point hull, like the Apache. Its
    // soldier drops (`OSPREY_MAX_SOLDIERS`) are not modelled.
    static OSPREY: MonsterSpec = MonsterSpec {
        classification: C::Machine,
        health: [400.0, 400.0, 400.0],
        melee: None,
        ranged: None,
        hull: Hull::Point,
        blood: BloodKind::None,
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
        // Wave 1 batch A.
        MonsterKind::Barnacle => Some(&BARNACLE),
        MonsterKind::AlienController => Some(&ALIEN_CONTROLLER),
        MonsterKind::HumanAssassin => Some(&HUMAN_ASSASSIN),
        MonsterKind::Babycrab => Some(&BABYCRAB),
        MonsterKind::Generic => Some(&GENERIC),
        MonsterKind::Furniture => Some(&FURNITURE),
        MonsterKind::Rat => Some(&RAT),
        MonsterKind::Cockroach => Some(&COCKROACH),
        // Wave 1 batch B.
        MonsterKind::BigMomma => Some(&BIGMOMMA),
        MonsterKind::Nihilanth => Some(&NIHILANTH),
        MonsterKind::Apache => Some(&APACHE),
        MonsterKind::Osprey => Some(&OSPREY),
        MonsterKind::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttackSpec, BIGMOMMA_BASE_HEALTH, BIGMOMMA_HEALTH_FACTOR, Difficulty,
        GARGANTUA_VULNERABILITY, MonsterFlags, MonsterKind, NIHILANTH_SPHERE_COUNT,
        NIHILANTH_SPHERE_RESERVE, damage_response_for, spec_for,
    };
    use crate::damage::{DamageKinds, DamageResponse};

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
            // Wave 1 batch A.
            (MonsterKind::Barnacle, [25.0, 25.0, 25.0]),
            (MonsterKind::AlienController, [60.0, 60.0, 100.0]),
            (MonsterKind::HumanAssassin, [30.0, 50.0, 50.0]),
            (MonsterKind::Babycrab, [2.5, 2.5, 5.0]),
            (MonsterKind::Generic, [8.0, 8.0, 8.0]),
            (MonsterKind::Furniture, [8.0, 8.0, 8.0]),
            (MonsterKind::Rat, [1.0, 1.0, 1.0]),
            (MonsterKind::Cockroach, [1.0, 1.0, 1.0]),
            // Wave 1 batch B.
            (MonsterKind::BigMomma, [150.0, 225.0, 300.0]),
            (MonsterKind::Nihilanth, [800.0, 800.0, 1_000.0]),
            (MonsterKind::Apache, [150.0, 250.0, 400.0]),
            (MonsterKind::Osprey, [400.0, 400.0, 400.0]),
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

    /// Every defined kind must publish a `models/*.mdl` default path so a
    /// map's own `monster_*` entity (which usually carries no `model`
    /// keyvalue at all) can still be drawn; [`MonsterKind::Unknown`] must
    /// not, since this project has no table row to guess a path from, and
    /// neither must the two kinds whose model the map itself authors
    /// ([`MonsterKind::model_from_map`]).
    #[test]
    fn every_defined_kind_has_a_default_model_and_unknown_does_not() {
        for kind in MonsterKind::defined() {
            if kind.model_from_map() {
                assert_eq!(kind.default_model_path(), None, "{kind:?}");
                continue;
            }
            let path = kind
                .default_model_path()
                .unwrap_or_else(|| panic!("{kind:?} has a default model path"));
            assert!(path.starts_with("models/"));
            assert!(
                std::path::Path::new(path)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("mdl"))
            );
        }
        assert_eq!(
            MonsterKind::Unknown("monster_not_in_the_table".to_string()).default_model_path(),
            None
        );
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
            // Wave 1 batch A (the barnacle's bite is a placeholder, not a
            // cited number; see its row).
            (MonsterKind::Babycrab, [1.5, 3.0, 3.0]),
            // Wave 1 batch B.
            (MonsterKind::BigMomma, [50.0, 60.0, 70.0]),
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
            // Wave 1 batch A.
            (MonsterKind::AlienController, [3.0, 4.0, 5.0]),
            (MonsterKind::HumanAssassin, [5.0, 5.0, 8.0]),
            // Wave 1 batch B.
            (MonsterKind::BigMomma, [100.0, 120.0, 160.0]),
            (MonsterKind::Nihilanth, [30.0, 30.0, 30.0]),
            (MonsterKind::Apache, [8.0, 10.0, 10.0]),
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

    // Wave 1 batch B.
    /// The four boss/aircraft classnames all resolve to a defined kind
    /// with a spec, a model and a brain row — none of them is `Unknown`.
    #[test]
    fn the_batch_b_classnames_are_defined_rather_than_unknown() {
        for (classname, kind) in [
            ("monster_bigmomma", MonsterKind::BigMomma),
            ("monster_nihilanth", MonsterKind::Nihilanth),
            ("monster_apache", MonsterKind::Apache),
            ("monster_osprey", MonsterKind::Osprey),
        ] {
            let resolved = MonsterKind::from_classname(classname);
            assert!(
                !matches!(resolved, MonsterKind::Unknown(_)),
                "{classname} must not be Unknown"
            );
            assert_eq!(resolved, kind);
            assert_eq!(resolved.classname(), classname);
            assert!(spec_for(&resolved).is_some());
            assert!(resolved.default_model_path().is_some());
            assert!(MonsterKind::defined().contains(&resolved));
        }
    }

    /// The Gonarch's health row is exactly the published base times the
    /// published per-difficulty factor.
    #[test]
    fn the_gonarchs_health_is_the_base_times_the_published_factor() {
        let spec = spec_for(&MonsterKind::BigMomma).expect("defined");
        for difficulty in Difficulty::ALL {
            let expected = BIGMOMMA_BASE_HEALTH * BIGMOMMA_HEALTH_FACTOR[difficulty.index()];
            assert!((spec.health[difficulty.index()] - expected).abs() < 1e-6);
        }
    }

    /// The Nihilanth's published per-sprite reserve is its health over the
    /// published sprite count, at every difficulty.
    #[test]
    fn the_nihilanths_sphere_reserve_is_a_twentieth_of_its_health() {
        let spec = spec_for(&MonsterKind::Nihilanth).expect("defined");
        #[allow(clippy::cast_precision_loss)]
        let count = NIHILANTH_SPHERE_COUNT as f32;
        for difficulty in Difficulty::ALL {
            let reserve = spec.health[difficulty.index()] / count;
            assert!((reserve - NIHILANTH_SPHERE_RESERVE[difficulty.index()]).abs() < 1e-6);
        }
    }

    /// The two aircraft fly (the point hull, `crate::movement::flies`); the
    /// two bosses do not. Whether a monster flies is its hull alone; there
    /// is no flag for it.
    #[test]
    fn the_aircraft_are_on_the_point_hull_and_the_bosses_are_not() {
        for kind in [MonsterKind::Apache, MonsterKind::Osprey] {
            let spec = spec_for(&kind).expect("defined");
            assert!(crate::movement::flies(spec.hull), "{kind:?}");
        }
        for kind in [MonsterKind::BigMomma, MonsterKind::Nihilanth] {
            let spec = spec_for(&kind).expect("defined");
            assert!(!crate::movement::flies(spec.hull), "{kind:?}");
            assert!(!spec.flags.contains(MonsterFlags::ROOTED), "{kind:?}");
        }
    }

    /// The cited pages say `TriggerCondition` does not work on the
    /// Nihilanth, the Apache or the Osprey; it works on everything else.
    #[test]
    fn only_the_final_boss_and_the_aircraft_ignore_trigger_condition() {
        for kind in MonsterKind::defined() {
            assert_eq!(
                kind.honours_trigger_condition(),
                !matches!(
                    kind,
                    MonsterKind::Nihilanth | MonsterKind::Apache | MonsterKind::Osprey
                ),
                "{kind:?}"
            );
        }
        assert!(MonsterKind::Unknown("monster_x".into()).honours_trigger_condition());
    }

    /// The gargantua's immunity and the Apache's doubled blast are the only
    /// published damage rules in the table; every other kind answers every
    /// type at face value.
    #[test]
    fn only_the_gargantua_and_the_apache_answer_damage_types_specially() {
        for kind in MonsterKind::defined() {
            let response = damage_response_for(kind);
            match kind {
                MonsterKind::Gargantua => {
                    let vulnerable = response.vulnerable_to;
                    assert_eq!(vulnerable, GARGANTUA_VULNERABILITY);
                    assert!(vulnerable.contains(DamageKinds::BLAST));
                    assert!(vulnerable.contains(DamageKinds::CRUSH));
                    assert!(vulnerable.contains(DamageKinds::ENERGYBEAM));
                    assert!(!vulnerable.intersects(DamageKinds::BULLET));
                    assert!(!vulnerable.intersects(DamageKinds::SLASH));
                    assert!(!vulnerable.intersects(DamageKinds::BURN));
                    assert!(!vulnerable.intersects(DamageKinds::GENERIC));
                    assert!(response.doubled_by.is_empty());
                }
                MonsterKind::Apache => {
                    assert_eq!(response.vulnerable_to, DamageKinds::ALL);
                    assert_eq!(response.doubled_by, DamageKinds::BLAST);
                }
                _ => assert_eq!(response, DamageResponse::ORDINARY, "{kind:?}"),
            }
        }
        assert_eq!(
            damage_response_for(&MonsterKind::Unknown("monster_x".into())),
            DamageResponse::ORDINARY
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
    fn monster_bullchicken_is_an_alias_for_bullsquid() {
        // TWHL's "Reference: Entities and their models" lists this
        // monster's row under `monster_bullchicken` (GoldSrc's own
        // internal asset name), not `monster_bullsquid`; both must
        // resolve to the same kind. See docs/FORMAT_SOURCES.md, "Monster
        // definitions".
        assert_eq!(
            MonsterKind::from_classname("monster_bullchicken"),
            MonsterKind::Bullsquid
        );
        assert_eq!(
            MonsterKind::from_classname("monster_bullsquid"),
            MonsterKind::Bullsquid
        );
    }

    /// Wave 1 batch A: none of the eight classnames added is
    /// [`MonsterKind::Unknown`] any more, each has a spec, and each is in
    /// [`MonsterKind::defined`].
    #[test]
    fn wave_1_batch_a_classnames_are_no_longer_unknown() {
        let classnames = [
            "monster_barnacle",
            "monster_alien_controller",
            "monster_human_assassin",
            "monster_babycrab",
            "monster_generic",
            "monster_furniture",
            "monster_rat",
            "monster_cockroach",
        ];
        for classname in classnames {
            let kind = MonsterKind::from_classname(classname);
            assert!(
                !matches!(kind, MonsterKind::Unknown(_)),
                "{classname} is still Unknown"
            );
            assert_eq!(kind.classname(), classname);
            assert!(spec_for(&kind).is_some(), "{classname} has no spec");
            assert!(MonsterKind::defined().contains(&kind));
        }
    }

    /// The babycrab's published numbers are fractions of the headcrab's:
    /// its table row equals the headcrab's scaled by the cited fractions,
    /// and an override of the *headcrab's* health cvar scales it too.
    #[test]
    fn the_babycrab_is_a_scaled_headcrab() {
        use super::{BABYCRAB_DAMAGE_FRACTION, BABYCRAB_HEALTH_FRACTION};
        let headcrab = spec_for(&MonsterKind::Headcrab).expect("defined");
        let babycrab = spec_for(&MonsterKind::Babycrab).expect("defined");
        for difficulty in Difficulty::ALL {
            let index = difficulty.index();
            assert!(
                (babycrab.health[index] - headcrab.health[index] * BABYCRAB_HEALTH_FRACTION).abs()
                    < 1e-6
            );
            let (Some(parent), Some(child)) = (headcrab.melee, babycrab.melee) else {
                panic!("both crabs bite");
            };
            assert!(
                (child.damage[index] - parent.damage[index] * BABYCRAB_DAMAGE_FRACTION).abs()
                    < 1e-6
            );
        }
        let lookup: &dyn Fn(&str) -> Option<f32> =
            &|cvar: &str| (cvar == "sk_headcrab_health2").then_some(40.0);
        let resolved =
            babycrab.resolve_health(&MonsterKind::Babycrab, Difficulty::Medium, Some(lookup));
        assert!((resolved - 40.0 * BABYCRAB_HEALTH_FRACTION).abs() < 1e-6);
        assert!(
            (babycrab.resolve_health(&MonsterKind::Babycrab, Difficulty::Hard, Some(lookup)) - 5.0)
                .abs()
                < 1e-6
        );
    }

    /// A rooted kind is also one that never flees, and the controller —
    /// the one kind that is meant to fly — is on the point hull, the hull
    /// every mover flies (`crate::movement::flies`; `brains`' own test
    /// checks no other point-hull kind ever moves).
    #[test]
    fn rooted_kinds_never_flee_and_the_controller_is_on_the_point_hull() {
        use super::MonsterFlags;
        for kind in MonsterKind::defined() {
            let spec = spec_for(kind).expect("defined");
            if spec.flags.contains(MonsterFlags::ROOTED) {
                assert!(
                    spec.flags.contains(MonsterFlags::NEVER_FLEES),
                    "{kind:?} is rooted but may flee"
                );
            }
        }
        assert!(crate::movement::flies(
            spec_for(&MonsterKind::AlienController)
                .expect("defined")
                .hull
        ));
        assert!(
            spec_for(&MonsterKind::Barnacle)
                .expect("defined")
                .flags
                .contains(MonsterFlags::ROOTED)
        );
    }

    /// The barnacle hangs from a ceiling, so its eye is below its origin;
    /// every other kind keeps the default eye height above it.
    #[test]
    fn only_the_barnacle_looks_down_from_its_origin() {
        for kind in MonsterKind::defined() {
            let offset = kind.view_offset();
            if *kind == MonsterKind::Barnacle {
                assert!(offset.z < 0.0);
            } else {
                assert!(offset.z > 0.0, "{kind:?}");
            }
        }
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
