//! Damage types, health and armour, and the damage application rule.
//!
//! The damage-type *names* below are Half-Life's published damage
//! vocabulary, as documented on the public mapping wikis cited in
//! `docs/FORMAT_SOURCES.md` ("Combat and damage"): the set a mapper selects
//! from on `trigger_hurt`'s damage-type field. The bit *values* are this
//! project's own dense assignment (bit 0 upwards in the order the vocabulary
//! is listed), not a transcription of any engine header.
//!
//! Everything numeric that describes how much a hit hurts lives in
//! [`ArmorRule`] or [`DifficultyScale`], both supplied by the caller.

use core::fmt;
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

use glam::Vec3;
use ohl_core::SanitizedError;

use crate::trace::EntityId;

/// A set of damage types, as a bitmask.
///
/// Constants are combined with `|` and tested with
/// [`contains`](Self::contains):
///
/// ```
/// use ohl_combat::DamageType;
///
/// let kind = DamageType::BULLET | DamageType::SHOCK;
/// assert!(kind.contains(DamageType::BULLET));
/// assert!(!kind.contains(DamageType::BURN));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DamageType(u32);

macro_rules! damage_types {
    ($($(#[$meta:meta])* $name:ident = $bit:expr, $label:literal;)*) => {
        impl DamageType {
            $($(#[$meta])* pub const $name: Self = Self(1 << $bit);)*

            /// Every named damage type.
            pub const ALL: Self = Self($((1u32 << $bit) |)* 0);

            /// The named types in declaration order, with their wiki labels.
            pub const NAMED: &'static [(Self, &'static str)] =
                &[$((Self(1 << $bit), $label),)*];
        }
    };
}

damage_types! {
    /// Untyped damage; the default when nothing more specific applies.
    GENERIC = 0, "generic";
    /// Being squashed by a mover.
    CRUSH = 1, "crush";
    /// Hitscan firearms.
    BULLET = 2, "bullet";
    /// Cutting melee attacks.
    SLASH = 3, "slash";
    /// Fire.
    BURN = 4, "burn";
    /// Cold.
    FREEZE = 5, "freeze";
    /// Falling too far.
    FALL = 6, "fall";
    /// Explosions.
    BLAST = 7, "blast";
    /// Blunt melee attacks.
    CLUB = 8, "club";
    /// Electricity.
    SHOCK = 9, "shock";
    /// Sonic attacks.
    SONIC = 10, "sonic";
    /// Energy beams.
    ENERGYBEAM = 11, "energybeam";
    /// Running out of air underwater.
    DROWN = 12, "drown";
    /// Paralysis.
    PARALYZE = 13, "paralyze";
    /// Nerve gas.
    NERVEGAS = 14, "nervegas";
    /// Poison.
    POISON = 15, "poison";
    /// Radiation.
    RADIATION = 16, "radiation";
    /// Acid.
    ACID = 17, "acid";
    /// A lingering burn.
    SLOWBURN = 18, "slowburn";
    /// A lingering freeze.
    SLOWFREEZE = 19, "slowfreeze";
}

impl DamageType {
    /// The empty set.
    pub const NONE: Self = Self(0);

    /// The raw bitmask.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// A set from a raw bitmask, keeping only named bits.
    #[must_use]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// Whether every type in `other` is present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether any type in `other` is present.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether no type is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for DamageType {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for DamageType {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for DamageType {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for DamageType {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for DamageType {
    type Output = Self;
    fn not(self) -> Self {
        Self(!self.0 & Self::ALL.0)
    }
}

impl fmt::Display for DamageType {
    /// Lists the set types by their wiki labels, `+`-separated, or `none`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut first = true;
        for (flag, label) in Self::NAMED {
            if self.contains(*flag) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(label)?;
                first = false;
            }
        }
        Ok(())
    }
}

/// One application of damage to one target.
///
/// `attacker` is who is responsible (a player or monster); `inflictor` is
/// what physically delivered it (a rocket, a trigger volume), which can be
/// the same entity or none at all for world hazards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageInfo {
    /// The entity credited with the damage, if any.
    pub attacker: Option<EntityId>,
    /// The entity that delivered the damage, if any.
    pub inflictor: Option<EntityId>,
    /// Hit points before armour and any multipliers. Must be finite and
    /// greater than zero; [`apply_damage`] rejects anything else.
    pub amount: f32,
    /// What kind of damage this is.
    pub kind: DamageType,
    /// Where the damage came from, in world units (used for knockback and
    /// for the HUD's damage direction indicator).
    pub origin: Vec3,
    /// The unit direction the damage travelled in, when it had one.
    pub direction: Vec3,
}

impl DamageInfo {
    /// A minimal record: an amount and a type, with no attacker or geometry.
    #[must_use]
    pub fn new(amount: f32, kind: DamageType) -> Self {
        Self {
            attacker: None,
            inflictor: None,
            amount,
            kind,
            origin: Vec3::ZERO,
            direction: Vec3::ZERO,
        }
    }

    /// Sets the attacker and inflictor.
    #[must_use]
    pub fn from_entities(mut self, attacker: EntityId, inflictor: EntityId) -> Self {
        self.attacker = Some(attacker);
        self.inflictor = Some(inflictor);
        self
    }

    /// Sets the world-space origin and direction.
    #[must_use]
    pub fn from_point(mut self, origin: Vec3, direction: Vec3) -> Self {
        self.origin = origin;
        self.direction = direction;
        self
    }

    /// The same record with a different amount.
    #[must_use]
    pub fn with_amount(mut self, amount: f32) -> Self {
        self.amount = amount;
        self
    }
}

/// A target's hit points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    /// Current hit points; zero or below means dead.
    pub current: f32,
    /// The cap [`heal`](Self::heal) tops up to.
    ///
    /// The player's published maximum is 100 (`docs/FORMAT_SOURCES.md`,
    /// "Combat and damage"); every other entity's maximum comes from its own
    /// definition, so this type stores rather than assumes it.
    pub max: f32,
}

impl Health {
    /// A full-health component with `max` hit points.
    #[must_use]
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }

    /// Whether the target has run out of hit points. A non-finite current
    /// value counts as dead, so corrupt state cannot make a target immortal.
    #[must_use]
    pub fn is_dead(self) -> bool {
        self.current <= 0.0 || self.current.is_nan()
    }

    /// Adds hit points, clamped to [`max`](Self::max). Returns how many were
    /// actually restored, so a charger can bill for exactly that much.
    pub fn heal(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        let before = self.current;
        self.current = (self.current + amount).min(self.max);
        (self.current - before).max(0.0)
    }
}

/// A target's armour points (the HEV suit, for the player).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Armor {
    /// Current armour points.
    pub current: f32,
    /// The cap [`recharge`](Self::recharge) tops up to. The player's
    /// published maximum is 100 (`docs/FORMAT_SOURCES.md`).
    pub max: f32,
}

impl Armor {
    /// An armour component with `max` points, starting empty.
    #[must_use]
    pub fn empty(max: f32) -> Self {
        Self { current: 0.0, max }
    }

    /// An armour component that starts full.
    #[must_use]
    pub fn full(max: f32) -> Self {
        Self { current: max, max }
    }

    /// Adds armour points, clamped to [`max`](Self::max); returns how many
    /// were actually added.
    pub fn recharge(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        let before = self.current;
        self.current = (self.current + amount).min(self.max);
        (self.current - before).max(0.0)
    }
}

/// How armour splits incoming damage, as a caller-supplied parameter.
///
/// The rule this project implements is:
///
/// 1. `ratio` of the incoming damage always reaches health, whatever the
///    armour level (`ratio = 0.0` means armour can stop everything,
///    `ratio = 1.0` means armour does nothing);
/// 2. the remaining `1 - ratio` is charged against armour at `bonus` armour
///    points per hit point stopped;
/// 3. if armour runs out part-way, the hit points it could not pay for reach
///    health as well.
///
/// **To be black-box observed.** Half-Life's real HEV split is not published
/// on any source this project may use, so [`Default`] is the neutral rule
/// (`ratio = 1.0`, `bonus = 1.0`): armour absorbs nothing and loses nothing.
/// Tests and callers pass explicit values; once the real behaviour has been
/// measured against legally obtained retail software it will be recorded in
/// `docs/FORMAT_SOURCES.md` as a project measurement, not a Valve value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmorRule {
    /// The fraction of damage that bypasses armour, in `0.0..=1.0`.
    pub ratio: f32,
    /// Armour points spent per hit point stopped; must be greater than zero.
    pub bonus: f32,
}

impl Default for ArmorRule {
    fn default() -> Self {
        Self {
            ratio: 1.0,
            bonus: 1.0,
        }
    }
}

impl ArmorRule {
    /// A rule with the given parameters, clamped into their valid ranges
    /// (`ratio` into `0.0..=1.0`, `bonus` to at least [`f32::MIN_POSITIVE`]);
    /// non-finite inputs fall back to the neutral rule's values.
    #[must_use]
    pub fn new(ratio: f32, bonus: f32) -> Self {
        let ratio = if ratio.is_finite() {
            ratio.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let bonus = if bonus.is_finite() {
            bonus.max(f32::MIN_POSITIVE)
        } else {
            1.0
        };
        Self { ratio, bonus }
    }
}

/// What one [`apply_damage`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DamageOutcome {
    /// Hit points removed from [`Health::current`].
    pub health_lost: f32,
    /// Armour points removed from [`Armor::current`].
    pub armor_lost: f32,
    /// Whether this call is the one that brought health to zero or below.
    pub killed: bool,
}

/// Applies `info` to `health`, letting `armor` absorb what `rule` allows.
///
/// `armor` is `None` for a target with no suit, and for damage types the
/// caller has decided armour does not cover — which of Half-Life's damage
/// types those are is itself a **to be black-box observed** question, so this
/// function does not guess and leaves the choice at the call site.
///
/// [`DamageOutcome::killed`] is set only on the transition into death, so a
/// caller can emit exactly one [`crate::CombatEvent::Killed`] per target even
/// if damage keeps arriving afterwards.
///
/// # Errors
///
/// Returns [`SanitizedError::InvalidInput`] when `info.amount` is not finite
/// or is not greater than zero: zero and negative "damage" are rejected
/// rather than silently healing the target.
pub fn apply_damage(
    health: &mut Health,
    armor: Option<&mut Armor>,
    info: &DamageInfo,
    rule: ArmorRule,
) -> Result<DamageOutcome, SanitizedError> {
    if !info.amount.is_finite() || info.amount <= 0.0 {
        return Err(SanitizedError::InvalidInput);
    }
    let rule = ArmorRule::new(rule.ratio, rule.bonus);
    let was_dead = health.is_dead();

    let mut to_health = info.amount * rule.ratio;
    let coverable = info.amount - to_health;
    let mut armor_lost = 0.0;

    if let Some(armor) = armor
        && coverable > 0.0
        && armor.current > 0.0
    {
        let wanted = coverable * rule.bonus;
        if wanted <= armor.current {
            armor_lost = wanted;
        } else {
            armor_lost = armor.current;
            // The hit points the remaining armour could not pay for.
            to_health += coverable - armor_lost / rule.bonus;
        }
        armor.current = (armor.current - armor_lost).max(0.0);
    } else {
        to_health = info.amount;
    }

    let health_lost = to_health.clamp(0.0, info.amount);
    health.current -= health_lost;
    Ok(DamageOutcome {
        health_lost,
        armor_lost,
        killed: !was_dead && health.is_dead(),
    })
}

/// The three skill levels Half-Life's published monster and weapon tables are
/// quoted for.
///
/// Mirrors `ohl_campaign::Difficulty` (same three variants in the same
/// order, so [`index`](Self::index) is that crate's `skill_suffix() - 1`),
/// but is defined here so `ohl-combat` needs no edge to `ohl-campaign`: this
/// crate only ever needs to pick one of three values. The composition root
/// converts between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Difficulty {
    /// Skill 1.
    Easy,
    /// Skill 2.
    #[default]
    Medium,
    /// Skill 3.
    Hard,
}

impl Difficulty {
    /// Picks this level's entry from an easy/medium/hard triple, the shape
    /// the published per-difficulty tables are quoted in.
    #[must_use]
    pub fn pick<T: Copy>(self, values: [T; 3]) -> T {
        values[self.index()]
    }

    /// `0`, `1` or `2`, for indexing difficulty-keyed tables.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Easy => 0,
            Self::Medium => 1,
            Self::Hard => 2,
        }
    }
}

/// A per-difficulty multiplier applied to an attack's damage.
///
/// **To be black-box observed.** Half-Life scales some damage values by skill
/// level, but the multipliers are not published on any usable source, so
/// [`Default`] is `1.0` everywhere — no scaling at all — and callers pass
/// measured values once they exist.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifficultyScale {
    /// Multiplier on [`Difficulty::Easy`].
    pub easy: f32,
    /// Multiplier on [`Difficulty::Medium`].
    pub medium: f32,
    /// Multiplier on [`Difficulty::Hard`].
    pub hard: f32,
}

impl Default for DifficultyScale {
    fn default() -> Self {
        Self {
            easy: 1.0,
            medium: 1.0,
            hard: 1.0,
        }
    }
}

impl DifficultyScale {
    /// The multiplier for `difficulty`; non-finite or negative entries are
    /// treated as `1.0`.
    #[must_use]
    pub fn factor(self, difficulty: Difficulty) -> f32 {
        let raw = difficulty.pick([self.easy, self.medium, self.hard]);
        if raw.is_finite() && raw >= 0.0 {
            raw
        } else {
            1.0
        }
    }

    /// `info` with its amount scaled for `difficulty`.
    #[must_use]
    pub fn scaled(self, info: &DamageInfo, difficulty: Difficulty) -> DamageInfo {
        info.with_amount(info.amount * self.factor(difficulty))
    }
}
