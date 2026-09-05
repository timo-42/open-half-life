//! Property tests for the M7.2 firing state machine: whatever input
//! sequence arrives, `FiringState::tick` never panics and never lets an
//! `AmmoPool` go negative (pools are `u32`, so "negative" here means
//! `AmmoPool::take` ever being asked to remove more than is carried without
//! being rejected first, and `current()` never exceeding `capacity()`).

use ohl_combat::{AmmoPool, AmmoType, FiringState, WeaponId, WeaponInput, spec};
use proptest::prelude::*;

fn weapon_id() -> impl Strategy<Value = WeaponId> {
    prop::sample::select(WeaponId::ALL.to_vec())
}

fn ammo_for(id: WeaponId) -> AmmoType {
    spec(id).ammo.unwrap_or(AmmoType::NineMillimeter)
}

fn input() -> impl Strategy<Value = WeaponInput> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(primary, secondary, reload, select)| WeaponInput {
            primary,
            secondary,
            reload,
            select,
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// An arbitrary sequence of ticks on an arbitrary weapon never panics,
    /// and the ammo pool it draws from stays within `0..=capacity` and
    /// never desyncs from the clip it feeds.
    #[test]
    fn firing_never_panics_and_ammo_stays_in_bounds(
        id in weapon_id(),
        starting_ammo in 0u32..300,
        dts in prop::collection::vec(0.0f32..2.0, 1..64),
        inputs in prop::collection::vec(input(), 1..64),
    ) {
        let entry = spec(id);
        let mut pool = AmmoPool::new(ammo_for(id));
        pool.add(starting_ammo);
        let mut state = FiringState::new(entry);
        state.tick(0.016, WeaponInput { select: true, ..Default::default() }, &mut pool);

        let clip_cap = entry.clip_size.unwrap_or(0);
        let steps = dts.len().min(inputs.len());
        for index in 0..steps {
            let _ = state.tick(dts[index], inputs[index], &mut pool);

            // The pool never exceeds its capacity (checked by construction,
            // but `AmmoPool` is `u32`-backed, so "never negative" is
            // vacuous unless every subtraction is checked; `add`/`take_up_to`
            // are, and this asserts the invariant holds after every tick).
            prop_assert!(pool.current() <= pool.capacity());
            if entry.clip_size.is_some() {
                prop_assert!(state.clip() <= clip_cap);
            } else {
                prop_assert_eq!(state.clip(), 0);
            }
        }
    }

    /// Charging and releasing a gauss-style weapon never yields a charge
    /// damage outside its published range, and never both a charge damage
    /// and a self-damage on the same tick.
    #[test]
    fn gauss_charge_release_stays_in_its_published_range(
        hold_seconds in 0.0f32..30.0,
        step in 0.01f32..1.0,
    ) {
        let entry = spec(WeaponId::Gauss);
        let mut pool = AmmoPool::new(AmmoType::Uranium);
        pool.add(100);
        let mut state = FiringState::new(entry);
        state.tick(0.016, WeaponInput { select: true, ..Default::default() }, &mut pool);

        let charge_input = WeaponInput { secondary: true, ..Default::default() };
        let mut remaining = hold_seconds;
        state.tick(0.001, charge_input, &mut pool);
        // Stop feeding `secondary` the moment the charge stops (a forced
        // overcharge release ends it without the caller letting go), so
        // this single hold-then-release never runs a second charge cycle.
        while remaining > 0.0 && state.is_charging() {
            let dt = step.min(remaining.max(0.001));
            state.tick(dt, charge_input, &mut pool);
            remaining -= dt;
        }
        if state.is_charging() {
            state.tick(0.001, WeaponInput::default(), &mut pool);
        }

        let charge = state.take_charge_damage();
        let self_damage = state.take_self_damage();
        prop_assert!(charge.is_none() || self_damage.is_none());
        if let Some(damage) = charge {
            prop_assert!((25.0..=200.0).contains(&damage), "damage {damage}");
        }
        if let Some(damage) = self_damage {
            prop_assert!(
                (damage - ohl_combat::GAUSS_OVERCHARGE_SELF_DAMAGE).abs() < f32::EPSILON
            );
        }
        prop_assert!(pool.current() <= pool.capacity());
    }
}
