//! Ammunition types and bounded ammo pools.
//!
//! [`AmmoType`] is Half-Life's published ammunition vocabulary — the classes
//! a weapon draws from and a HUD counts separately. Each variant's published
//! carry cap (Combine OverWiki weapon pages, `docs/FORMAT_SOURCES.md`,
//! "Combat and damage") is exposed by [`AmmoType::published_max_carry`]; a
//! cap that is not reliably published on a usable source is `None` there,
//! and [`AmmoType::default_capacity`] falls back to a documented, neutral
//! **black-box** placeholder for it instead of inventing a number.

use ohl_core::SanitizedError;

/// One ammunition class, as selected by a weapon's clip and by pickups.
///
/// All twelve variants and their carry caps below are **[CO]** (Combine
/// OverWiki), reviewed 2026-09-05; see `docs/FORMAT_SOURCES.md`, "Combat and
/// damage".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmmoType {
    /// 9mm parabellum (glock, MP5).
    NineMillimeter,
    /// .357 Magnum.
    ThreeFiveSeven,
    /// 12-gauge buckshot.
    Buckshot,
    /// Crossbow bolts.
    Bolts,
    /// RPG rockets.
    Rockets,
    /// Gauss gun uranium cells.
    Uranium,
    /// Hornets, fired by the hornet gun and regenerated over time.
    Hornets,
    /// Hand grenades.
    HandGrenades,
    /// Satchel charges.
    Satchels,
    /// Tripmines.
    Tripmines,
    /// Snarks.
    Snarks,
    /// MP5 grenades (the secondary-fire underslung launcher).
    Mp5Grenades,
}

impl AmmoType {
    /// Every variant, in declaration order.
    pub const ALL: [Self; 12] = [
        Self::NineMillimeter,
        Self::ThreeFiveSeven,
        Self::Buckshot,
        Self::Bolts,
        Self::Rockets,
        Self::Uranium,
        Self::Hornets,
        Self::HandGrenades,
        Self::Satchels,
        Self::Tripmines,
        Self::Snarks,
        Self::Mp5Grenades,
    ];

    /// The published maximum carried amount, when a usable source states one.
    ///
    /// `Snarks` returns `None`: no source this project may use states a
    /// carry cap for it (only "5 per pickup", which is not the same thing),
    /// so [`default_capacity`](Self::default_capacity) supplies a black-box
    /// placeholder instead.
    #[must_use]
    pub const fn published_max_carry(self) -> Option<u32> {
        match self {
            Self::NineMillimeter => Some(250),
            Self::ThreeFiveSeven => Some(36),
            Self::Buckshot => Some(125),
            Self::Bolts => Some(50),
            Self::Rockets | Self::Satchels | Self::Tripmines => Some(5),
            Self::Uranium => Some(100),
            Self::Hornets => Some(8),
            Self::HandGrenades | Self::Mp5Grenades => Some(10),
            Self::Snarks => None,
        }
    }

    /// The capacity a fresh [`AmmoPool`] uses: the published cap, or a
    /// neutral black-box placeholder when there is none.
    ///
    /// **To be black-box observed:** `Snarks`' real carry cap is not
    /// published on a usable source; `15` is a neutral placeholder, not a
    /// measurement, pending a black-box observation of retail software.
    // TODO(black-box): replace with the measured snark carry cap.
    #[must_use]
    pub const fn default_capacity(self) -> u32 {
        match self.published_max_carry() {
            Some(cap) => cap,
            None => 15,
        }
    }

    /// Whether [`published_max_carry`](Self::published_max_carry) has a
    /// value, i.e. whether [`default_capacity`](Self::default_capacity) is a
    /// cited number rather than a black-box placeholder.
    #[must_use]
    pub const fn is_published(self) -> bool {
        self.published_max_carry().is_some()
    }
}

/// A bounded amount of one [`AmmoType`] carried by an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmmoPool {
    kind: AmmoType,
    current: u32,
    capacity: u32,
}

impl AmmoPool {
    /// An empty pool for `kind`, capped at [`AmmoType::default_capacity`].
    #[must_use]
    pub fn new(kind: AmmoType) -> Self {
        Self {
            kind,
            current: 0,
            capacity: kind.default_capacity(),
        }
    }

    /// An empty pool for `kind` with an explicit capacity (at least 1).
    #[must_use]
    pub fn with_capacity(kind: AmmoType, capacity: u32) -> Self {
        Self {
            kind,
            current: 0,
            capacity: capacity.max(1),
        }
    }

    /// The ammo type this pool holds.
    #[must_use]
    pub fn kind(self) -> AmmoType {
        self.kind
    }

    /// How much is currently carried.
    #[must_use]
    pub fn current(self) -> u32 {
        self.current
    }

    /// The pool's capacity.
    #[must_use]
    pub fn capacity(self) -> u32 {
        self.capacity
    }

    /// Whether the pool has nothing left.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.current == 0
    }

    /// Adds `amount`, clamped to [`capacity`](Self::capacity); returns how
    /// much was actually added.
    pub fn add(&mut self, amount: u32) -> u32 {
        let added = amount.min(self.capacity - self.current);
        self.current += added;
        added
    }

    /// Removes `amount`, failing rather than going negative.
    ///
    /// # Errors
    ///
    /// Returns [`SanitizedError::ArithmeticUnderflow`] when `amount` exceeds
    /// [`current`](Self::current); the pool is left unchanged.
    pub fn take(&mut self, amount: u32) -> Result<(), SanitizedError> {
        self.current = self
            .current
            .checked_sub(amount)
            .ok_or(SanitizedError::ArithmeticUnderflow)?;
        Ok(())
    }

    /// Removes up to `amount`, taking whatever is available instead of
    /// failing; returns how much was actually removed.
    pub fn take_up_to(&mut self, amount: u32) -> u32 {
        let taken = amount.min(self.current);
        self.current -= taken;
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_cap_matches_the_design_table() {
        assert_eq!(AmmoType::NineMillimeter.published_max_carry(), Some(250));
        assert_eq!(AmmoType::ThreeFiveSeven.published_max_carry(), Some(36));
        assert_eq!(AmmoType::Buckshot.published_max_carry(), Some(125));
        assert_eq!(AmmoType::Bolts.published_max_carry(), Some(50));
        assert_eq!(AmmoType::Rockets.published_max_carry(), Some(5));
        assert_eq!(AmmoType::Uranium.published_max_carry(), Some(100));
        assert_eq!(AmmoType::Hornets.published_max_carry(), Some(8));
        assert_eq!(AmmoType::HandGrenades.published_max_carry(), Some(10));
        assert_eq!(AmmoType::Satchels.published_max_carry(), Some(5));
        assert_eq!(AmmoType::Tripmines.published_max_carry(), Some(5));
        assert_eq!(AmmoType::Mp5Grenades.published_max_carry(), Some(10));
    }

    #[test]
    fn snarks_are_an_explicit_black_box() {
        assert_eq!(AmmoType::Snarks.published_max_carry(), None);
        assert!(!AmmoType::Snarks.is_published());
        assert_eq!(AmmoType::Snarks.default_capacity(), 15);
    }

    #[test]
    fn every_variant_is_covered_and_positive() {
        for kind in AmmoType::ALL {
            assert!(kind.default_capacity() > 0, "{kind:?}");
        }
    }

    #[test]
    fn a_pool_never_exceeds_capacity_or_goes_negative() {
        let mut pool = AmmoPool::new(AmmoType::NineMillimeter);
        assert_eq!(pool.add(300), 250);
        assert_eq!(pool.current(), 250);
        assert_eq!(pool.take_up_to(1_000), 250);
        assert_eq!(pool.current(), 0);
        assert!(pool.take(1).is_err());
    }

    #[test]
    fn take_fails_without_mutating_on_underflow() {
        let mut pool = AmmoPool::with_capacity(AmmoType::Rockets, 5);
        pool.add(2);
        assert!(pool.take(3).is_err());
        assert_eq!(pool.current(), 2);
        assert!(pool.take(2).is_ok());
        assert_eq!(pool.current(), 0);
    }
}
