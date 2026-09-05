//! Property test for M7.4's `Inventory`: an arbitrary sequence of
//! give-weapon/give-ammo/select/drop operations never lets any ammo pool
//! exceed its capacity or go negative, and every clip stays within its
//! weapon's clip size.

use ohl_combat::{AmmoType, Inventory, WeaponId, spec};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    GiveWeapon(WeaponId),
    GiveAmmo(AmmoType, u32),
    SelectNext,
    SelectPrev,
    SelectSlot(u8),
    Drop(WeaponId),
    Holster,
}

fn weapon_id() -> impl Strategy<Value = WeaponId> {
    prop::sample::select(WeaponId::ALL.to_vec())
}

fn ammo_type() -> impl Strategy<Value = AmmoType> {
    prop::sample::select(AmmoType::ALL.to_vec())
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        weapon_id().prop_map(Op::GiveWeapon),
        (ammo_type(), 0u32..1_000).prop_map(|(kind, amount)| Op::GiveAmmo(kind, amount)),
        Just(Op::SelectNext),
        Just(Op::SelectPrev),
        (1u8..=6).prop_map(Op::SelectSlot),
        weapon_id().prop_map(Op::Drop),
        Just(Op::Holster),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever sequence of operations arrives, every ammo pool stays within
    /// `0..=capacity`, every clip stays within its weapon's clip size, and
    /// the selected weapon (if any) is always actually owned.
    #[test]
    fn inventory_never_exceeds_caps_or_goes_negative(ops in prop::collection::vec(op(), 0..128)) {
        let mut inventory = Inventory::new();

        for op in ops {
            match op {
                Op::GiveWeapon(id) => {
                    inventory.give_weapon(id);
                }
                Op::GiveAmmo(kind, amount) => {
                    let added = inventory.give_ammo(kind, amount);
                    prop_assert!(added <= amount);
                }
                Op::SelectNext => {
                    inventory.select_next();
                }
                Op::SelectPrev => {
                    inventory.select_prev();
                }
                Op::SelectSlot(slot) => {
                    inventory.select_slot(slot);
                }
                Op::Drop(id) => {
                    inventory.drop(id);
                }
                Op::Holster => {
                    inventory.holster();
                }
            }

            for kind in AmmoType::ALL {
                let pool = inventory.ammo(kind);
                prop_assert!(pool.current() <= pool.capacity());
            }
            for id in WeaponId::ALL {
                let cap = spec(id).clip_size.unwrap_or(0);
                prop_assert!(inventory.clip(id) <= cap);
            }
            if let Some(selected) = inventory.selected() {
                prop_assert!(inventory.has_weapon(selected));
            }
        }
    }
}
