//! Pickup entity classification and resolution.
//!
//! [`classify_classname`] maps a BSP entity's `classname` to a
//! [`PickupKind`]. Every classname matched is Half-Life's published entity
//! vocabulary, not this project's invention: TWHL wiki, `weaponbox`
//! (<https://twhl.info/wiki/page/weaponbox>), "Weapons Programming -
//! Standard Weapons" and "Weapons Programming - Custom Ammo Types"
//! (`CGlockAmmo`/`ammo_9mmclip`, `CPythonAmmo`/`ammo_357`,
//! `CShotgunAmmo`/`ammo_buckshot`, `CCrossbowAmmo`/`ammo_crossbow`,
//! `CRpgAmmo`/`ammo_rpgclip`, `CGaussAmmo`/`ammo_gaussclip`,
//! `CMP5AmmoGrenade`/`ammo_ARgrenades`), the individual `weapon_hornetgun`
//! and `weapon_rpg` pages, `item_healthkit`, `func_healthcharger` and
//! `func_recharge` pages (all reviewed 2026-09-05 through search-engine
//! result summaries, since TWHL and Combine OverWiki front automated
//! requests with a challenge/proof-of-work page); `item_battery`,
//! `item_suit` and `item_longjump` are the corresponding, equally
//! well-documented `CItem` subclasses in the same public entity hierarchy.
//! See `docs/FORMAT_SOURCES.md`, "Pickups and chargers".
//!
//! Pickup *amounts* are cited only where a usable source states one:
//! Combine OverWiki's "Chargers" page publishes a 50 HP health-charger
//! reservoir and a 75/50/35 (easy/medium/hard) suit-charger reservoir
//! (reviewed 2026-09-05 through search-engine result summaries). No usable
//! source states how much ammo one `ammo_*`/`weapon_*` box grants, or how
//! much `item_healthkit`/`item_battery` restore, so those are
//! [`crate::weapons::BlackBox`] placeholders with a `// TODO(black-box)`
//! marker, exactly as `crate::weapons::spec` marks its own unpublished
//! numbers. Respawn (`SF_NORESPAWN` and friends) is a multiplayer-only
//! concern and is not modeled: single-player pickups are simply consumed.
//!
//! No path or name literal derived from user media appears here — every
//! classname above is drawn from public wiki pages, per `docs/CLEAN_ROOM.md`
//! rule 7.

use crate::ammo::AmmoType;
use crate::damage::{Armor, Difficulty, Health};
use crate::inventory::Inventory;
use crate::weapons::{BlackBox, WeaponId, spec};

/// One pickup entity's kind, as classified from its `classname`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PickupKind {
    /// A `weapon_*` entity: unlocks the weapon and grants its bundled ammo.
    Weapon(WeaponId),
    /// An `ammo_*` entity: tops up one ammo type.
    Ammo(AmmoType),
    /// `item_healthkit`.
    HealthKit,
    /// `item_battery`.
    Battery,
    /// `item_suit`.
    Suit,
    /// `item_longjump`.
    LongJump,
    /// `func_healthcharger`: a use-and-hold wall unit restoring health.
    HealthCharger,
    /// `func_recharge`: a use-and-hold wall unit restoring HEV armour.
    SuitCharger,
}

/// Classifies a BSP entity `classname` into a [`PickupKind`], or `None` when
/// `classname` is not one of the published pickup entities this module
/// recognises. Matching is exact and case-sensitive, as GoldSrc classnames
/// are.
#[must_use]
pub fn classify_classname(classname: &str) -> Option<PickupKind> {
    use PickupKind::{
        Ammo, Battery, HealthCharger, HealthKit, LongJump, Suit, SuitCharger, Weapon,
    };
    Some(match classname {
        "weapon_crowbar" => Weapon(WeaponId::Crowbar),
        // TWHL: "weapon_glock and weapon_9mmhandgun are linked to the same
        // weapon"; both classnames spawn the Glock.
        "weapon_9mmhandgun" | "weapon_glock" => Weapon(WeaponId::Glock),
        "weapon_357" => Weapon(WeaponId::Python),
        "weapon_9mmAR" => Weapon(WeaponId::Mp5),
        "weapon_shotgun" => Weapon(WeaponId::Shotgun),
        "weapon_crossbow" => Weapon(WeaponId::Crossbow),
        "weapon_rpg" => Weapon(WeaponId::Rpg),
        "weapon_gauss" => Weapon(WeaponId::Gauss),
        "weapon_egon" => Weapon(WeaponId::Egon),
        "weapon_hornetgun" => Weapon(WeaponId::HornetGun),
        "weapon_handgrenade" => Weapon(WeaponId::HandGrenade),
        "weapon_satchel" => Weapon(WeaponId::Satchel),
        "weapon_tripmine" => Weapon(WeaponId::Tripmine),
        "weapon_snark" => Weapon(WeaponId::Snark),
        // The MP5 belt box (`ammo_9mmAR`) uses a second, distinct classname
        // for the same 9mm ammo type the Glock clip box (`ammo_9mmclip`)
        // grants.
        "ammo_9mmclip" | "ammo_glockclip" | "ammo_9mmAR" => Ammo(AmmoType::NineMillimeter),
        "ammo_ARgrenades" | "ammo_mp5grenades" => Ammo(AmmoType::Mp5Grenades),
        "ammo_357" => Ammo(AmmoType::ThreeFiveSeven),
        "ammo_buckshot" => Ammo(AmmoType::Buckshot),
        "ammo_crossbow" => Ammo(AmmoType::Bolts),
        "ammo_rpgclip" => Ammo(AmmoType::Rockets),
        "ammo_gaussclip" => Ammo(AmmoType::Uranium),
        "item_healthkit" => HealthKit,
        "item_battery" => Battery,
        "item_suit" => Suit,
        "item_longjump" => LongJump,
        "func_healthcharger" => HealthCharger,
        "func_recharge" => SuitCharger,
        _ => return None,
    })
}

/// Combine OverWiki, "Chargers": a health charger's reservoir. **[CO]**.
pub const HEALTH_CHARGER_TOTAL: f32 = 50.0;

/// Combine OverWiki, "Chargers": a suit charger's reservoir, published per
/// difficulty (easy, medium, hard). **[CO]**.
pub const SUIT_CHARGER_TOTAL_BY_DIFFICULTY: [f32; 3] = [75.0, 50.0, 35.0];

/// A charger's per-second drain rate while held. **To be black-box
/// observed**: the published totals above describe the reservoir, not how
/// fast it empties; this placeholder drains a health charger's total over
/// five (simulated) seconds of continuous use.
// TODO(black-box): replace with the measured drain rate.
pub const CHARGER_DRAIN_RATE: BlackBox<f32> = BlackBox::new(HEALTH_CHARGER_TOTAL / 5.0);

/// How much one `item_healthkit` restores. **To be black-box observed**: no
/// usable source publishes this amount.
// TODO(black-box): replace with the measured healthkit amount.
pub const HEALTHKIT_AMOUNT: BlackBox<f32> = BlackBox::new(25.0);

/// How much one `item_battery` restores. **To be black-box observed**: no
/// usable source publishes this amount.
// TODO(black-box): replace with the measured battery amount.
pub const BATTERY_AMOUNT: BlackBox<f32> = BlackBox::new(15.0);

/// How much one ammo box of `kind` grants. **To be black-box observed**: no
/// usable source publishes a per-box amount, only the carry cap
/// (`AmmoType::published_max_carry`); this placeholder is a quarter of the
/// pool's capacity (at least one unit), a neutral fraction rather than a
/// measurement.
// TODO(black-box): replace with the measured per-box amount.
#[must_use]
pub const fn ammo_pickup_amount(kind: AmmoType) -> BlackBox<u32> {
    let cap = kind.default_capacity();
    let quarter = cap.div_ceil(4);
    BlackBox::new(if quarter > 0 { quarter } else { 1 })
}

/// How much ammo one `weapon_*` pickup bundles alongside unlocking the
/// weapon. **To be black-box observed**: no usable source publishes this
/// either; the placeholder is the weapon's clip size (a weapon plausibly
/// arrives loaded), or one unit for a weapon with no clip concept.
// TODO(black-box): replace with the measured bundled-ammo amount.
#[must_use]
pub const fn weapon_pickup_ammo(id: WeaponId) -> BlackBox<u32> {
    match spec(id).clip_size {
        Some(clip_size) => BlackBox::new(clip_size),
        None => BlackBox::new(1),
    }
}

/// What one [`try_pickup`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PickupOutcome {
    /// Whether anything was actually taken (a new weapon, ammo added to a
    /// pool with room, health/armour restored, or a not-yet-owned flag
    /// item). A pickup whose target was already full or already owned
    /// reports `false` here.
    pub taken: bool,
    /// The requested amount that could not be applied (`0.0` when fully
    /// applied). For a flag item (suit, long jump) this is `1.0` when
    /// already owned, `0.0` otherwise.
    pub remaining: f32,
}

/// Resolves a touch pickup of `kind` against `inventory`, `health` and
/// `armor`. `difficulty` only matters for a (future) difficulty-scaled
/// pickup; none of the kinds handled here are currently scaled, but the
/// parameter keeps the signature stable once one is.
///
/// [`PickupKind::HealthCharger`] and [`PickupKind::SuitCharger`] are
/// "use-and-hold" brush entities, not touch pickups: this function reports
/// them as untaken (`taken: false, remaining: 0.0`) and leaves them to
/// [`ChargerState`], which the composition root drives every tick the
/// player holds `+use` against one.
#[must_use]
pub fn try_pickup(
    inventory: &mut Inventory,
    health: &mut Health,
    armor: &mut Armor,
    kind: PickupKind,
    _difficulty: Difficulty,
) -> PickupOutcome {
    match kind {
        PickupKind::Weapon(id) => {
            let is_new = inventory.give_weapon(id);
            let ammo_taken = spec(id).ammo.is_some_and(|ammo_kind| {
                inventory.give_ammo(ammo_kind, weapon_pickup_ammo(id).value) > 0
            });
            let taken = is_new || ammo_taken;
            PickupOutcome {
                taken,
                remaining: if taken { 0.0 } else { 1.0 },
            }
        }
        PickupKind::Ammo(kind) => {
            let amount = ammo_pickup_amount(kind).value;
            let added = inventory.give_ammo(kind, amount);
            PickupOutcome {
                taken: added > 0,
                #[allow(clippy::cast_precision_loss)]
                remaining: (amount - added) as f32,
            }
        }
        PickupKind::HealthKit => {
            let applied = health.heal(HEALTHKIT_AMOUNT.value);
            PickupOutcome {
                taken: applied > 0.0,
                remaining: (HEALTHKIT_AMOUNT.value - applied).max(0.0),
            }
        }
        PickupKind::Battery => {
            let applied = armor.recharge(BATTERY_AMOUNT.value);
            PickupOutcome {
                taken: applied > 0.0,
                remaining: (BATTERY_AMOUNT.value - applied).max(0.0),
            }
        }
        PickupKind::Suit => {
            let taken = inventory.give_suit();
            PickupOutcome {
                taken,
                remaining: if taken { 0.0 } else { 1.0 },
            }
        }
        PickupKind::LongJump => {
            let taken = inventory.give_long_jump();
            PickupOutcome {
                taken,
                remaining: if taken { 0.0 } else { 1.0 },
            }
        }
        PickupKind::HealthCharger | PickupKind::SuitCharger => PickupOutcome {
            taken: false,
            remaining: 0.0,
        },
    }
}

/// A `func_healthcharger`/`func_recharge`'s remaining reservoir.
///
/// Modeled as "use and hold": each tick the player holds `+use` against it,
/// [`drain_health`](Self::drain_health)/[`drain_armor`](Self::drain_armor)
/// pays out up to [`CHARGER_DRAIN_RATE`] `* dt` from the reservoir into the
/// target, stopping early once either the reservoir or the target's
/// capacity runs out. The reservoir never recharges within this type: how
/// long a drained charger takes to refill is **BBO** and left to the
/// composition root's own cooldown timer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChargerState {
    remaining: f32,
}

impl ChargerState {
    /// A full health charger: [`HEALTH_CHARGER_TOTAL`] to give out.
    #[must_use]
    pub const fn health() -> Self {
        Self {
            remaining: HEALTH_CHARGER_TOTAL,
        }
    }

    /// A full suit charger at `difficulty`: one of
    /// [`SUIT_CHARGER_TOTAL_BY_DIFFICULTY`] to give out.
    #[must_use]
    pub fn suit(difficulty: Difficulty) -> Self {
        Self {
            remaining: difficulty.pick(SUIT_CHARGER_TOTAL_BY_DIFFICULTY),
        }
    }

    /// How much is left in the reservoir.
    #[must_use]
    pub const fn remaining(&self) -> f32 {
        self.remaining
    }

    /// Whether the reservoir has nothing left.
    #[must_use]
    pub fn is_depleted(&self) -> bool {
        self.remaining <= 0.0
    }

    /// Pays out up to [`CHARGER_DRAIN_RATE`] `* dt` into `health`, capped by
    /// both the remaining reservoir and `health`'s own headroom. Returns the
    /// amount actually restored; a non-finite or non-positive `dt` restores
    /// nothing.
    pub fn drain_health(&mut self, health: &mut Health, dt: f32) -> f32 {
        let offer = Self::offer(self.remaining, dt);
        if offer <= 0.0 {
            return 0.0;
        }
        let applied = health.heal(offer);
        self.remaining = (self.remaining - applied).max(0.0);
        applied
    }

    /// As [`drain_health`](Self::drain_health), for `armor`.
    pub fn drain_armor(&mut self, armor: &mut Armor, dt: f32) -> f32 {
        let offer = Self::offer(self.remaining, dt);
        if offer <= 0.0 {
            return 0.0;
        }
        let applied = armor.recharge(offer);
        self.remaining = (self.remaining - applied).max(0.0);
        applied
    }

    fn offer(remaining: f32, dt: f32) -> f32 {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        (CHARGER_DRAIN_RATE.value * dt).min(remaining.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_classnames_map_to_the_expected_kind() {
        assert_eq!(
            classify_classname("weapon_357"),
            Some(PickupKind::Weapon(WeaponId::Python))
        );
        assert_eq!(
            classify_classname("ammo_buckshot"),
            Some(PickupKind::Ammo(AmmoType::Buckshot))
        );
        assert_eq!(
            classify_classname("item_healthkit"),
            Some(PickupKind::HealthKit)
        );
        assert_eq!(
            classify_classname("func_healthcharger"),
            Some(PickupKind::HealthCharger)
        );
        assert_eq!(
            classify_classname("func_recharge"),
            Some(PickupKind::SuitCharger)
        );
        assert_eq!(classify_classname("func_door"), None);
    }

    #[test]
    fn a_new_weapon_pickup_is_taken_and_bundles_its_ammo() {
        let mut inventory = Inventory::new();
        let mut health = Health::new(100.0);
        let mut armor = Armor::empty(100.0);
        let outcome = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::Weapon(WeaponId::Python),
            Difficulty::Medium,
        );
        assert!(outcome.taken);
        assert!(inventory.has_weapon(WeaponId::Python));
        assert!(inventory.ammo(AmmoType::ThreeFiveSeven).current() > 0);
    }

    #[test]
    fn a_full_ammo_pool_is_not_taken() {
        let mut inventory = Inventory::new();
        inventory.give_ammo(AmmoType::Buckshot, AmmoType::Buckshot.default_capacity());
        let mut health = Health::new(100.0);
        let mut armor = Armor::empty(100.0);
        let outcome = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::Ammo(AmmoType::Buckshot),
            Difficulty::Medium,
        );
        assert!(!outcome.taken);
        assert!(outcome.remaining > 0.0);
    }

    #[test]
    fn healthkit_and_battery_apply_their_published_placeholder_amounts() {
        let mut inventory = Inventory::new();
        let mut health = Health {
            current: 50.0,
            max: 100.0,
        };
        let mut armor = Armor::empty(100.0);

        let health_outcome = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::HealthKit,
            Difficulty::Medium,
        );
        assert!(health_outcome.taken);
        assert!((health.current - (50.0 + HEALTHKIT_AMOUNT.value)).abs() < f32::EPSILON);

        let battery_outcome = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::Battery,
            Difficulty::Medium,
        );
        assert!(battery_outcome.taken);
        assert!((armor.current - BATTERY_AMOUNT.value).abs() < f32::EPSILON);
    }

    #[test]
    fn suit_and_long_jump_pickups_are_only_taken_once() {
        let mut inventory = Inventory::new();
        let mut health = Health::new(100.0);
        let mut armor = Armor::empty(100.0);

        let first = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::Suit,
            Difficulty::Medium,
        );
        assert!(first.taken);
        let second = try_pickup(
            &mut inventory,
            &mut health,
            &mut armor,
            PickupKind::Suit,
            Difficulty::Medium,
        );
        assert!(!second.taken);
        assert!((second.remaining - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_health_charger_drains_to_exactly_its_published_total() {
        let mut charger = ChargerState::health();
        let mut health = Health {
            current: 0.0,
            max: 200.0,
        };
        let mut total_applied = 0.0;
        for _ in 0..100 {
            total_applied += charger.drain_health(&mut health, 1.0);
        }
        assert!(charger.is_depleted());
        assert!((total_applied - HEALTH_CHARGER_TOTAL).abs() < 1e-3);
        assert!((health.current - HEALTH_CHARGER_TOTAL).abs() < 1e-3);
    }

    #[test]
    fn a_suit_charger_drains_to_its_published_per_difficulty_total() {
        for (difficulty, expected) in [
            (Difficulty::Easy, 75.0),
            (Difficulty::Medium, 50.0),
            (Difficulty::Hard, 35.0),
        ] {
            let mut charger = ChargerState::suit(difficulty);
            let mut armor = Armor::empty(200.0);
            let mut total_applied = 0.0;
            for _ in 0..100 {
                total_applied += charger.drain_armor(&mut armor, 1.0);
            }
            assert!(charger.is_depleted());
            assert!(
                (total_applied - expected).abs() < 1e-3,
                "{difficulty:?} expected {expected}, got {total_applied}"
            );
        }
    }

    #[test]
    fn a_charger_never_overfills_the_target() {
        let mut charger = ChargerState::health();
        let mut health = Health {
            current: 45.0,
            max: 50.0,
        };
        for _ in 0..1_000 {
            charger.drain_health(&mut health, 1.0);
        }
        assert!(health.current <= 50.0);
    }
}
