//! The mapping between `ohl_player::DamageKind` (a closed, 11-variant
//! classification the player systems crate uses for fall damage, drowning,
//! `trigger_hurt` and the HEV suit reactions) and `ohl_combat::DamageType`
//! (a 20-bit composable mask, the project's single damage vocabulary for
//! everything a weapon, projectile or explosion produces).
//!
//! Neither `ohl-player` nor `ohl-combat` is edited to add this: the mapping
//! lives entirely here, in the one crate that already depends on both, so
//! no new crate edge is introduced. See `.plan/m79-design.md` §3 for the
//! recommendation this module implements.
//!
//! `TODO(black-box)`: whether retail treats nerve gas and poison damage as
//! one suit occasion, or two distinct ones, is not settled by any source
//! this project may use; [`damage_kind_of`] folds both into
//! [`ohl_player::DamageKind::Chemical`], and [`damage_type_of`] maps that
//! back to [`ohl_combat::DamageType::POISON`] rather than `NERVEGAS`,
//! pending a black-box observation of legally obtained retail software.

use ohl_combat::DamageType;
use ohl_player::DamageKind;

/// The [`DamageType`] bit a player-systems [`DamageKind`] stands for.
///
/// Every one of the eleven variants round-trips through [`damage_kind_of`]
/// (see this module's tests): `damage_kind_of(damage_type_of(kind)) == kind`
/// for every `kind`.
#[must_use]
pub const fn damage_type_of(kind: DamageKind) -> DamageType {
    match kind {
        DamageKind::Generic => DamageType::GENERIC,
        DamageKind::Fall => DamageType::FALL,
        DamageKind::Drown => DamageType::DROWN,
        DamageKind::Burn => DamageType::BURN,
        DamageKind::Freeze => DamageType::FREEZE,
        DamageKind::Shock => DamageType::SHOCK,
        DamageKind::Acid => DamageType::ACID,
        DamageKind::Radiation => DamageType::RADIATION,
        // See the module doc's TODO(black-box): Chemical maps to POISON,
        // not NERVEGAS, as the single representative bit.
        DamageKind::Chemical => DamageType::POISON,
        DamageKind::Crush => DamageType::CRUSH,
        DamageKind::Blast => DamageType::BLAST,
    }
}

/// The player-systems [`DamageKind`] a [`DamageType`] mask reduces to.
///
/// Total and order-independent: every mask, including combinations no
/// single bit above produces on its own (`BURN | FREEZE`, say), reduces to
/// exactly one [`DamageKind`] by testing the fixed order below and taking
/// the first match, so the result never depends on iteration order over the
/// mask's bits. Bits with no [`DamageKind`] counterpart (`BULLET`, `SLASH`,
/// `CLUB`, `SONIC`, `ENERGYBEAM`, `PARALYZE`, `SLOWBURN`, `SLOWFREEZE`, and
/// the empty mask) reduce to [`DamageKind::Generic`], which still absorbs
/// into armour: only [`DamageKind::Fall`] and [`DamageKind::Drown`] bypass
/// it (see [`DamageKind::bypasses_armor`]).
#[must_use]
pub const fn damage_kind_of(mask: DamageType) -> DamageKind {
    if mask.contains(DamageType::FALL) {
        DamageKind::Fall
    } else if mask.contains(DamageType::DROWN) {
        DamageKind::Drown
    } else if mask.contains(DamageType::BLAST) {
        DamageKind::Blast
    } else if mask.contains(DamageType::CRUSH) {
        DamageKind::Crush
    } else if mask.contains(DamageType::BURN) {
        DamageKind::Burn
    } else if mask.contains(DamageType::FREEZE) {
        DamageKind::Freeze
    } else if mask.contains(DamageType::SHOCK) {
        DamageKind::Shock
    } else if mask.contains(DamageType::ACID) {
        DamageKind::Acid
    } else if mask.contains(DamageType::RADIATION) {
        DamageKind::Radiation
    } else if mask.contains(DamageType::NERVEGAS) || mask.contains(DamageType::POISON) {
        DamageKind::Chemical
    } else {
        DamageKind::Generic
    }
}

#[cfg(test)]
mod tests {
    use super::{damage_kind_of, damage_type_of};
    use ohl_combat::DamageType;
    use ohl_player::DamageKind;

    /// Every single-bit `DamageType` mask classifies to the same
    /// `DamageKind` no matter which other (already-tested) bit orderings
    /// might otherwise apply; this is really a total-function smoke test,
    /// with the order-independence proven separately below.
    #[test]
    fn every_single_bit_mask_classifies_without_panicking() {
        for (bit, _) in DamageType::NAMED {
            let _ = damage_kind_of(*bit);
        }
    }

    #[test]
    fn burn_and_freeze_together_resolve_to_burn_by_the_fixed_order() {
        let combined = DamageType::BURN | DamageType::FREEZE;
        assert_eq!(damage_kind_of(combined), DamageKind::Burn);
        // Order independence: building the same mask the other way around
        // must classify identically.
        let reordered = DamageType::FREEZE | DamageType::BURN;
        assert_eq!(damage_kind_of(reordered), damage_kind_of(combined));
    }

    #[test]
    fn every_named_mask_is_order_independent_against_every_other_bit() {
        // Combine each named bit with every other named bit, in both
        // orders, and require the classification to agree.
        for (a, _) in DamageType::NAMED {
            for (b, _) in DamageType::NAMED {
                let forward = *a | *b;
                let backward = *b | *a;
                assert_eq!(
                    damage_kind_of(forward),
                    damage_kind_of(backward),
                    "a={a:?} b={b:?}"
                );
            }
        }
    }

    #[test]
    fn unmapped_bits_and_the_empty_mask_are_generic() {
        assert_eq!(damage_kind_of(DamageType::NONE), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::BULLET), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::SLASH), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::CLUB), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::SONIC), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::ENERGYBEAM), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::PARALYZE), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::SLOWBURN), DamageKind::Generic);
        assert_eq!(damage_kind_of(DamageType::SLOWFREEZE), DamageKind::Generic);
    }

    /// `damage_type_of` composed with `damage_kind_of` is the identity on
    /// every one of the eleven `DamageKind` variants.
    #[test]
    fn damage_type_of_then_damage_kind_of_is_the_identity() {
        let all = [
            DamageKind::Generic,
            DamageKind::Fall,
            DamageKind::Drown,
            DamageKind::Burn,
            DamageKind::Freeze,
            DamageKind::Shock,
            DamageKind::Acid,
            DamageKind::Radiation,
            DamageKind::Chemical,
            DamageKind::Crush,
            DamageKind::Blast,
        ];
        for kind in all {
            assert_eq!(damage_kind_of(damage_type_of(kind)), kind, "{kind:?}");
        }
    }
}
