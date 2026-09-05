//! The player's weapon and ammo inventory.
//!
//! [`Inventory`] tracks which weapons are carried (a bitset over
//! [`WeaponId`], which has fourteen variants and so fits comfortably in a
//! `u16`), how much ammo of each [`AmmoType`] is held (a bounded
//! [`AmmoPool`] per type, so the published carry caps from
//! `crate::ammo::AmmoType::published_max_carry` are enforced the same way a
//! standalone pool enforces them), each owned weapon's loaded clip, the
//! selected weapon, and a HUD selection slot/position for every weapon.
//!
//! Everything here is a plain, deterministic data structure: no I/O, no
//! wall-clock reads, and no `HashMap` iteration order to depend on. All
//! indexing into the two fixed-size arrays goes through
//! [`weapon_index`]/[`ammo_index`], which are `const fn`s over the two
//! enums' declared variants, so array order never depends on enum
//! discriminant values.

use crate::ammo::{AmmoPool, AmmoType};
use crate::weapons::{WeaponId, spec};

/// How many distinct weapons [`Inventory`] can track — [`WeaponId::ALL`]'s
/// length, kept as a separate constant so the fixed-size arrays below have
/// a name instead of a bare `14`.
const WEAPON_COUNT: usize = WeaponId::ALL.len();

/// [`AmmoType::ALL`]'s length; see [`WEAPON_COUNT`].
const AMMO_COUNT: usize = AmmoType::ALL.len();

/// This weapon's position in [`WeaponId::ALL`], and so its index into
/// [`Inventory`]'s per-weapon arrays and its bit in the owned-weapons
/// bitset. A `const fn` match rather than a linear search, so it can be
/// used in a `const` context and never depends on iteration order.
#[must_use]
const fn weapon_index(id: WeaponId) -> usize {
    match id {
        WeaponId::Crowbar => 0,
        WeaponId::Glock => 1,
        WeaponId::Python => 2,
        WeaponId::Mp5 => 3,
        WeaponId::Shotgun => 4,
        WeaponId::Crossbow => 5,
        WeaponId::Rpg => 6,
        WeaponId::Gauss => 7,
        WeaponId::Egon => 8,
        WeaponId::HornetGun => 9,
        WeaponId::HandGrenade => 10,
        WeaponId::Satchel => 11,
        WeaponId::Tripmine => 12,
        WeaponId::Snark => 13,
    }
}

/// This ammo type's position in [`AmmoType::ALL`]; see [`weapon_index`].
#[must_use]
const fn ammo_index(kind: AmmoType) -> usize {
    match kind {
        AmmoType::NineMillimeter => 0,
        AmmoType::ThreeFiveSeven => 1,
        AmmoType::Buckshot => 2,
        AmmoType::Bolts => 3,
        AmmoType::Rockets => 4,
        AmmoType::Uranium => 5,
        AmmoType::Hornets => 6,
        AmmoType::HandGrenades => 7,
        AmmoType::Satchels => 8,
        AmmoType::Tripmines => 9,
        AmmoType::Snarks => 10,
        AmmoType::Mp5Grenades => 11,
    }
}

/// A weapon's HUD selection slot and position within that slot.
///
/// **Not sourced**: no usable public page documents Half-Life's HUD weapon
/// selection slot table (the per-weapon wiki pages this project cites for
/// damage and ammo describe the weapon, not its inventory position), so
/// this is this project's own layout rather than a transcription of
/// anything. Weapons are grouped by kind in ascending slot order — melee,
/// sidearms, primary automatics/shotgun, heavy ordnance, thrown/placed
/// explosives — and numbered by position within a slot in
/// [`WeaponId::ALL`] order. See [`hud_slot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HudSlot {
    /// The 1-based slot number (a HUD selection group, conventionally bound
    /// to one number key).
    pub slot: u8,
    /// The 1-based position within [`slot`](Self::slot).
    pub position: u8,
}

/// This weapon's [`HudSlot`]. See the type's documentation: this is a
/// project-authored layout, not published data.
#[must_use]
pub const fn hud_slot(id: WeaponId) -> HudSlot {
    match id {
        WeaponId::Crowbar => HudSlot {
            slot: 1,
            position: 1,
        },
        WeaponId::Glock => HudSlot {
            slot: 2,
            position: 1,
        },
        WeaponId::Python => HudSlot {
            slot: 2,
            position: 2,
        },
        WeaponId::Mp5 => HudSlot {
            slot: 3,
            position: 1,
        },
        WeaponId::Shotgun => HudSlot {
            slot: 3,
            position: 2,
        },
        WeaponId::Crossbow => HudSlot {
            slot: 4,
            position: 1,
        },
        WeaponId::Rpg => HudSlot {
            slot: 4,
            position: 2,
        },
        WeaponId::Gauss => HudSlot {
            slot: 4,
            position: 3,
        },
        WeaponId::Egon => HudSlot {
            slot: 4,
            position: 4,
        },
        WeaponId::HornetGun => HudSlot {
            slot: 4,
            position: 5,
        },
        WeaponId::HandGrenade => HudSlot {
            slot: 5,
            position: 1,
        },
        WeaponId::Satchel => HudSlot {
            slot: 5,
            position: 2,
        },
        WeaponId::Tripmine => HudSlot {
            slot: 5,
            position: 3,
        },
        WeaponId::Snark => HudSlot {
            slot: 5,
            position: 4,
        },
    }
}

/// The player's weapons, ammo, loaded clips and current selection.
///
/// `give_weapon` only unlocks a weapon slot; it does not itself grant ammo
/// or load a clip; a weapon pickup's accompanying ammo (see `crate::pickups`)
/// travels through the same [`give_ammo`](Self::give_ammo) path an ammo box
/// pickup uses, so there is exactly one place that enforces the carry cap.
/// A weapon's clip starts (and returns to, on [`drop`](Self::drop)) at zero;
/// it is loaded from the ammo pool by the same reload path
/// `crate::firing::FiringState` already implements, not duplicated here.
#[derive(Debug, Clone, PartialEq)]
pub struct Inventory {
    /// Bit `weapon_index(id)` is set when `id` is owned.
    owned: u16,
    /// Rounds currently loaded per weapon, indexed by `weapon_index`.
    clips: [u32; WEAPON_COUNT],
    /// One bounded pool per ammo type, indexed by `ammo_index`.
    ammo: [AmmoPool; AMMO_COUNT],
    /// The currently selected weapon, if any is drawn.
    selected: Option<WeaponId>,
    /// Whether `item_suit` has been picked up.
    has_suit: bool,
    /// Whether `item_longjump` has been picked up.
    has_long_jump: bool,
}

impl Default for Inventory {
    /// An empty inventory: no weapons, no ammo, nothing selected.
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    /// An empty inventory: no weapons, no ammo, nothing selected.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owned: 0,
            clips: [0; WEAPON_COUNT],
            ammo: AmmoType::ALL.map(AmmoPool::new),
            selected: None,
            has_suit: false,
            has_long_jump: false,
        }
    }

    /// Whether `id` is owned.
    #[must_use]
    pub const fn has_weapon(&self, id: WeaponId) -> bool {
        self.owned & (1 << weapon_index(id)) != 0
    }

    /// Every owned weapon, in [`WeaponId::ALL`] order.
    pub fn owned_weapons(&self) -> impl Iterator<Item = WeaponId> + '_ {
        WeaponId::ALL
            .into_iter()
            .filter(move |id| self.has_weapon(*id))
    }

    /// Unlocks `id`. Returns `true` when this was a new acquisition, `false`
    /// when the weapon was already owned (a re-pickup then only grants
    /// whatever ammo it carries, via [`give_ammo`](Self::give_ammo)).
    pub fn give_weapon(&mut self, id: WeaponId) -> bool {
        let bit = 1 << weapon_index(id);
        let was_new = self.owned & bit == 0;
        self.owned |= bit;
        was_new
    }

    /// Removes `id` from the inventory, clearing its loaded clip and, if it
    /// was selected, deselecting. Returns `true` when `id` had been owned.
    pub fn drop(&mut self, id: WeaponId) -> bool {
        let bit = 1 << weapon_index(id);
        let was_owned = self.owned & bit != 0;
        self.owned &= !bit;
        self.clips[weapon_index(id)] = 0;
        if self.selected == Some(id) {
            self.selected = None;
        }
        was_owned
    }

    /// Deselects the current weapon without dropping it, leaving the
    /// player's hands empty until [`select_slot`](Self::select_slot),
    /// [`select_next`](Self::select_next) or
    /// [`select_prev`](Self::select_prev) is called again.
    pub fn holster(&mut self) {
        self.selected = None;
    }

    /// The currently selected weapon, if any.
    #[must_use]
    pub const fn selected(&self) -> Option<WeaponId> {
        self.selected
    }

    /// Rounds loaded in `id`'s clip; `0` for an unowned weapon or one with
    /// no clip concept.
    #[must_use]
    pub fn clip(&self, id: WeaponId) -> u32 {
        self.clips[weapon_index(id)]
    }

    /// Sets `id`'s loaded clip directly (used by a reload or a save
    /// restore), clamped to the weapon's clip size.
    pub fn set_clip(&mut self, id: WeaponId, rounds: u32) {
        let cap = spec(id).clip_size.unwrap_or(0);
        self.clips[weapon_index(id)] = rounds.min(cap);
    }

    /// The ammo pool for `kind`.
    #[must_use]
    pub const fn ammo(&self, kind: AmmoType) -> &AmmoPool {
        &self.ammo[ammo_index(kind)]
    }

    /// Adds `amount` of `kind`, clamped to its published carry cap (or
    /// black-box placeholder, see [`AmmoType::default_capacity`]). Returns
    /// how much was actually added, so a pickup can tell whether the target
    /// was already at capacity.
    pub fn give_ammo(&mut self, kind: AmmoType, amount: u32) -> u32 {
        self.ammo[ammo_index(kind)].add(amount)
    }

    /// Whether `item_suit` has been picked up.
    #[must_use]
    pub const fn has_suit(&self) -> bool {
        self.has_suit
    }

    /// Marks `item_suit` as picked up. Returns `true` when this was new.
    pub fn give_suit(&mut self) -> bool {
        let was_new = !self.has_suit;
        self.has_suit = true;
        was_new
    }

    /// Whether `item_longjump` has been picked up.
    #[must_use]
    pub const fn has_long_jump(&self) -> bool {
        self.has_long_jump
    }

    /// Marks `item_longjump` as picked up. Returns `true` when this was new.
    pub fn give_long_jump(&mut self) -> bool {
        let was_new = !self.has_long_jump;
        self.has_long_jump = true;
        was_new
    }

    /// Every owned weapon's [`HudSlot`], sorted by slot then position —
    /// the order [`select_next`](Self::select_next)/
    /// [`select_prev`](Self::select_prev) cycle through.
    fn owned_by_hud_order(&self) -> Vec<WeaponId> {
        let mut owned: Vec<WeaponId> = self.owned_weapons().collect();
        owned.sort_by_key(|id| {
            let slot = hud_slot(*id);
            (slot.slot, slot.position)
        });
        owned
    }

    /// Selects the next owned weapon after the current selection in HUD
    /// slot/position order, wrapping around; selects the first owned weapon
    /// if nothing is currently selected. Returns the newly selected weapon,
    /// or `None` if nothing is owned.
    pub fn select_next(&mut self) -> Option<WeaponId> {
        let owned = self.owned_by_hud_order();
        if owned.is_empty() {
            self.selected = None;
            return None;
        }
        let next_index = match self
            .selected
            .and_then(|id| owned.iter().position(|w| *w == id))
        {
            Some(index) => (index + 1) % owned.len(),
            None => 0,
        };
        self.selected = Some(owned[next_index]);
        self.selected
    }

    /// As [`select_next`](Self::select_next), cycling backward.
    pub fn select_prev(&mut self) -> Option<WeaponId> {
        let owned = self.owned_by_hud_order();
        if owned.is_empty() {
            self.selected = None;
            return None;
        }
        let prev_index = match self
            .selected
            .and_then(|id| owned.iter().position(|w| *w == id))
        {
            Some(index) => (index + owned.len() - 1) % owned.len(),
            None => owned.len() - 1,
        };
        self.selected = Some(owned[prev_index]);
        self.selected
    }

    /// Selects a weapon from HUD `slot`. Pressing the same slot again (the
    /// current selection is already in `slot`) cycles to the next owned
    /// position within that slot, wrapping; otherwise the lowest owned
    /// position in `slot` is selected. Returns the newly selected weapon, or
    /// `None` if `slot` has no owned weapon.
    pub fn select_slot(&mut self, slot: u8) -> Option<WeaponId> {
        let mut in_slot: Vec<WeaponId> = self
            .owned_by_hud_order()
            .into_iter()
            .filter(|id| hud_slot(*id).slot == slot)
            .collect();
        in_slot.sort_by_key(|id| hud_slot(*id).position);
        if in_slot.is_empty() {
            return None;
        }
        let next_index = match self
            .selected
            .filter(|id| hud_slot(*id).slot == slot)
            .and_then(|id| in_slot.iter().position(|w| *w == id))
        {
            Some(index) => (index + 1) % in_slot.len(),
            None => 0,
        };
        self.selected = Some(in_slot[next_index]);
        self.selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn giving_a_weapon_reports_whether_it_was_new() {
        let mut inventory = Inventory::new();
        assert!(inventory.give_weapon(WeaponId::Glock));
        assert!(!inventory.give_weapon(WeaponId::Glock));
        assert!(inventory.has_weapon(WeaponId::Glock));
        assert!(!inventory.has_weapon(WeaponId::Python));
    }

    #[test]
    fn ammo_never_exceeds_the_published_cap() {
        let mut inventory = Inventory::new();
        let added = inventory.give_ammo(AmmoType::NineMillimeter, 1_000);
        assert_eq!(added, 250);
        assert_eq!(inventory.ammo(AmmoType::NineMillimeter).current(), 250);
        // A second gift on top of a full pool adds nothing further.
        assert_eq!(inventory.give_ammo(AmmoType::NineMillimeter, 10), 0);
    }

    #[test]
    fn dropping_a_weapon_clears_its_clip_and_deselects_it() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Python);
        inventory.set_clip(WeaponId::Python, 6);
        inventory.select_next();
        assert_eq!(inventory.selected(), Some(WeaponId::Python));

        assert!(inventory.drop(WeaponId::Python));
        assert!(!inventory.has_weapon(WeaponId::Python));
        assert_eq!(inventory.clip(WeaponId::Python), 0);
        assert_eq!(inventory.selected(), None);
        // Dropping something never owned reports false and changes nothing.
        assert!(!inventory.drop(WeaponId::Python));
    }

    #[test]
    fn select_next_and_prev_cycle_in_hud_slot_order() {
        let mut inventory = Inventory::new();
        // Two sidearms in slot 2: Glock (position 1), Python (position 2).
        inventory.give_weapon(WeaponId::Python);
        inventory.give_weapon(WeaponId::Glock);

        assert_eq!(inventory.select_next(), Some(WeaponId::Glock));
        assert_eq!(inventory.select_next(), Some(WeaponId::Python));
        assert_eq!(
            inventory.select_next(),
            Some(WeaponId::Glock),
            "wraps forward"
        );

        assert_eq!(
            inventory.select_prev(),
            Some(WeaponId::Python),
            "wraps backward"
        );
        assert_eq!(inventory.select_prev(), Some(WeaponId::Glock));
    }

    #[test]
    fn select_slot_picks_the_lowest_position_then_cycles_within_the_slot() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Mp5);
        inventory.give_weapon(WeaponId::Shotgun);

        assert_eq!(inventory.select_slot(3), Some(WeaponId::Mp5));
        assert_eq!(inventory.select_slot(3), Some(WeaponId::Shotgun));
        assert_eq!(
            inventory.select_slot(3),
            Some(WeaponId::Mp5),
            "wraps within the slot"
        );

        // A slot with nothing owned selects nothing and does not panic.
        assert_eq!(inventory.select_slot(1), None);
    }

    #[test]
    fn holster_deselects_without_dropping() {
        let mut inventory = Inventory::new();
        inventory.give_weapon(WeaponId::Crowbar);
        inventory.select_next();
        assert_eq!(inventory.selected(), Some(WeaponId::Crowbar));
        inventory.holster();
        assert_eq!(inventory.selected(), None);
        assert!(inventory.has_weapon(WeaponId::Crowbar));
    }

    #[test]
    fn every_weapon_and_ammo_type_has_a_distinct_index() {
        let mut weapon_indices: Vec<usize> = WeaponId::ALL.map(weapon_index).to_vec();
        weapon_indices.sort_unstable();
        weapon_indices.dedup();
        assert_eq!(weapon_indices.len(), WEAPON_COUNT);

        let mut ammo_indices: Vec<usize> = AmmoType::ALL.map(ammo_index).to_vec();
        ammo_indices.sort_unstable();
        ammo_indices.dedup();
        assert_eq!(ammo_indices.len(), AMMO_COUNT);
    }

    #[test]
    fn suit_and_long_jump_are_given_once() {
        let mut inventory = Inventory::new();
        assert!(inventory.give_suit());
        assert!(!inventory.give_suit());
        assert!(inventory.has_suit());

        assert!(inventory.give_long_jump());
        assert!(!inventory.give_long_jump());
        assert!(inventory.has_long_jump());
    }
}
