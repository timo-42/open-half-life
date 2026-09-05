//! Damage classification and the HEV armor absorption model.

use serde::{Deserialize, Serialize};

/// What kind of damage a hit is. The categories are the ones the HEV suit
/// is documented to announce and the ones the player systems in this crate
/// can actually produce; combat weapon damage arrives through the same
/// enum once `ohl-combat` exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DamageKind {
    /// No more specific classification.
    Generic,
    /// Landing too fast.
    Fall,
    /// Running out of air under water.
    Drown,
    /// Fire, heat, lava.
    Burn,
    /// Cold.
    Freeze,
    /// Electricity.
    Shock,
    /// Corrosive liquids (slime).
    Acid,
    /// Radiation.
    Radiation,
    /// Toxins and other chemical exposure.
    Chemical,
    /// Being crushed by a mover.
    Crush,
    /// An explosion.
    Blast,
}

impl DamageKind {
    /// Whether the HEV armor absorbs any of this kind of damage.
    ///
    /// Published behaviour: "if the damage type is `DMG_FALL` or
    /// `DMG_DROWN`, then the armour value will remain the same", and more
    /// loosely "the HEV suit's charge absorbs more than two-thirds of
    /// damage from most sources, with the exception of fall damage, which
    /// is absorbed directly into body-integrity". See
    /// `docs/FORMAT_SOURCES.md`, "Player systems".
    #[must_use]
    pub const fn bypasses_armor(self) -> bool {
        matches!(self, Self::Fall | Self::Drown)
    }

    /// The HEV suit occasion this kind of damage announces, when it has a
    /// dedicated one.
    #[must_use]
    pub const fn suit_occasion(self) -> Option<crate::suit::SuitOccasion> {
        use crate::suit::SuitOccasion;
        match self {
            Self::Burn => Some(SuitOccasion::HeatDamage),
            Self::Shock => Some(SuitOccasion::ShockDamage),
            Self::Acid | Self::Chemical => Some(SuitOccasion::ChemicalDetected),
            Self::Radiation => Some(SuitOccasion::RadiationDetected),
            Self::Blast | Self::Crush => Some(SuitOccasion::MajorFracture),
            Self::Fall => Some(SuitOccasion::MinorFracture),
            Self::Drown => Some(SuitOccasion::SeekMedicalAttention),
            Self::Generic | Self::Freeze => None,
        }
    }
}

/// `trigger_hurt`'s `damagetype` bits this crate can classify.
///
/// The entity's documentation states that the values of `damagetype` add
/// up ("a value of 24 deals both burn (8) and freeze (16) damage"), which
/// is where these two constants come from. The remaining bits of the field
/// are not published anywhere reachable and are handled as
/// [`DamageKind::Generic`].
///
/// `TODO(black-box)`: the rest of the `damagetype` bit table.
pub mod damage_type_bits {
    /// Burn damage.
    pub const BURN: u32 = 8;
    /// Freeze damage.
    pub const FREEZE: u32 = 16;
}

/// Classifies a `trigger_hurt` `damagetype` field. Only the documented bits
/// are recognised; anything else, including `0`, is
/// [`DamageKind::Generic`]. When several documented bits are set the first
/// in the fixed order below wins, so the result never depends on hashing or
/// iteration order.
#[must_use]
pub const fn damage_kind_from_bits(bits: u32) -> DamageKind {
    if bits & damage_type_bits::BURN != 0 {
        DamageKind::Burn
    } else if bits & damage_type_bits::FREEZE != 0 {
        DamageKind::Freeze
    } else {
        DamageKind::Generic
    }
}

/// A bit set of the damage kinds taken since it was last cleared, which is
/// what the HUD's damage-direction/type indicators read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DamageFlags(pub u32);

impl DamageFlags {
    /// The bit [`DamageKind`] `kind` occupies.
    #[must_use]
    pub const fn bit(kind: DamageKind) -> u32 {
        1u32 << (kind as u32)
    }

    /// Records that `kind` was taken.
    pub const fn insert(&mut self, kind: DamageKind) {
        self.0 |= Self::bit(kind);
    }

    /// Whether `kind` has been taken since the last [`Self::clear`].
    #[must_use]
    pub const fn contains(self, kind: DamageKind) -> bool {
        self.0 & Self::bit(kind) != 0
    }

    /// Forgets every recorded kind.
    pub const fn clear(&mut self) {
        self.0 = 0;
    }

    /// Whether nothing has been recorded.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// How one hit was split between the HEV armor and the player's health.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Absorbed {
    /// Health points lost.
    pub health_loss: f32,
    /// The armor value left afterwards.
    pub armor_left: f32,
}

/// Splits `damage` between the HEV armor and health.
///
/// This is the published model (Half-Life Physics Reference, chapter 11
/// "Health and damage", cited in `docs/FORMAT_SOURCES.md` under "Player
/// systems"): the armor drains by `2/5` of the damage and health takes
/// `1/5` of it, until the armor is exhausted, at which point health takes
/// what the remaining armor could not cover. Damage kinds that
/// [`DamageKind::bypasses_armor`] skip the split entirely.
///
/// Like every behavioural constant in this project this is a
/// *community-documented* model, not something measured here; it must be
/// verified against the retail game before parity is claimed.
#[must_use]
pub fn absorb(damage: f32, armor: f32, kind: DamageKind) -> Absorbed {
    if !damage.is_finite() || damage <= 0.0 {
        return Absorbed {
            health_loss: 0.0,
            armor_left: armor.max(0.0),
        };
    }
    let armor = armor.max(0.0);
    if kind.bypasses_armor() || armor <= 0.0 {
        return Absorbed {
            health_loss: damage,
            armor_left: armor,
        };
    }
    let drained = armor - ARMOR_DRAIN_PER_DAMAGE * damage;
    if drained > 0.0 {
        Absorbed {
            health_loss: damage * HEALTH_SHARE_OF_DAMAGE,
            armor_left: drained,
        }
    } else {
        Absorbed {
            // Published: once the armor is exhausted by the hit, health
            // loses `D - 2A`. At the boundary `A = 2D/5` that is exactly
            // `D/5`, so the two branches agree.
            health_loss: (damage - DAMAGE_COVERED_PER_ARMOR * armor).max(0.0),
            armor_left: 0.0,
        }
    }
}

/// Armor points drained per point of incoming damage (`2/5`).
pub const ARMOR_DRAIN_PER_DAMAGE: f32 = 0.4;
/// Share of incoming damage that reaches health while armor lasts (`1/5`).
pub const HEALTH_SHARE_OF_DAMAGE: f32 = 0.2;
/// Damage each remaining armor point covers when the armor is exhausted by
/// the hit (the published `D - 2A` term).
pub const DAMAGE_COVERED_PER_ARMOR: f32 = 2.0;

/// The published maximum safe landing speed, in units per second: below it
/// a landing does no damage at all.
///
/// Source: Half-Life Physics Reference, chapter 11, which names
/// `PLAYER_MAX_SAFE_FALL_SPEED` and gives 580 ups, together with the
/// resulting 210.25-unit safe height.
pub const SAFE_FALL_SPEED: f32 = 580.0;

/// The published constant of proportionality between the excess landing
/// speed and the damage taken (`25/111`), which is what makes a 1024 ups
/// landing (a 655.36-unit fall) exactly lethal to a 100-health player.
pub const DAMAGE_PER_EXCESS_FALL_SPEED: f32 = 25.0 / 111.0;

/// Fall damage for a landing at `speed` units per second downward.
///
/// `D = (25/111) * (v_z - 580)`, zero at or below the safe speed. Both
/// constants are community-documented, not measured here.
#[must_use]
pub fn fall_damage(speed: f32) -> f32 {
    if !speed.is_finite() || speed <= SAFE_FALL_SPEED {
        0.0
    } else {
        (speed - SAFE_FALL_SPEED) * DAMAGE_PER_EXCESS_FALL_SPEED
    }
}
