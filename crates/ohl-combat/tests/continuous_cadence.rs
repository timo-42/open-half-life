//! Project-authored input sequences, with no installed game data.
use ohl_combat::{AmmoPool, AmmoType, FiringState, WeaponAction, WeaponId, WeaponInput, spec};

fn drawn(id: WeaponId, ammo: AmmoType, rounds: u32) -> (FiringState, AmmoPool) {
    let mut pool = AmmoPool::new(ammo);
    pool.add(rounds);
    let mut state = FiringState::new(spec(id));
    state.tick(
        0.0,
        WeaponInput {
            select: true,
            ..WeaponInput::default()
        },
        &mut pool,
    );
    (state, pool)
}

fn primary() -> WeaponInput {
    WeaponInput {
        primary: true,
        ..WeaponInput::default()
    }
}

#[test]
fn egon_damage_pulses_and_cells_share_elapsed_time_at_multiple_step_sizes() {
    // Half-open one-second windows: immediate pulse then 0.1-second cadence.
    for (dt, steps) in [(0.01, 100), (0.005, 200), (0.02, 50)] {
        let (mut state, mut pool) = drawn(WeaponId::Egon, AmmoType::Uranium, 100);
        let pulses = (0..steps)
            .filter(|_| state.tick(dt, primary(), &mut pool) == WeaponAction::BeamTick)
            .count();
        assert_eq!(pulses, 10, "one second at {dt}s steps");
        assert_eq!(pool.current(), 90, "one cell per damage pulse");
    }
}

#[test]
fn egon_release_and_repress_cannot_bypass_the_damage_cooldown() {
    let (mut state, mut pool) = drawn(WeaponId::Egon, AmmoType::Uranium, 100);
    assert_eq!(
        state.tick(0.01, primary(), &mut pool),
        WeaponAction::BeamTick
    );
    let mut pulses = 1;
    for step in 1..100 {
        let input = if step % 2 == 0 {
            primary()
        } else {
            WeaponInput::default()
        };
        pulses += usize::from(state.tick(0.01, input, &mut pool) == WeaponAction::BeamTick);
    }
    assert_eq!(pulses, 10);
    assert_eq!(pool.current(), 90);
    for _ in 0..20 {
        state.tick(0.0, WeaponInput::default(), &mut pool);
        assert_ne!(
            state.tick(0.0, primary(), &mut pool),
            WeaponAction::BeamTick
        );
    }
    assert_eq!(pool.current(), 90, "zero time cannot buy another pulse");
}

#[test]
fn egon_last_cell_buys_exactly_one_pulse_then_stops() {
    let (mut state, mut pool) = drawn(WeaponId::Egon, AmmoType::Uranium, 1);
    assert_eq!(
        state.tick(0.01, primary(), &mut pool),
        WeaponAction::BeamTick
    );
    assert_eq!(pool.current(), 0);
    for _ in 0..100 {
        assert_ne!(
            state.tick(0.01, primary(), &mut pool),
            WeaponAction::BeamTick
        );
    }
}

#[test]
fn egon_large_steps_do_not_queue_free_or_delayed_burst_damage() {
    let (mut state, mut pool) = drawn(WeaponId::Egon, AmmoType::Uranium, 10);
    assert_eq!(
        state.tick(0.01, primary(), &mut pool),
        WeaponAction::BeamTick
    );
    assert_eq!(
        state.tick(1.05, primary(), &mut pool),
        WeaponAction::BeamTick
    );
    assert_eq!(pool.current(), 8);
    assert_eq!(state.tick(0.0, primary(), &mut pool), WeaponAction::Empty);
    state.tick(2.0, WeaponInput::default(), &mut pool);
    assert_eq!(
        state.tick(0.0, primary(), &mut pool),
        WeaponAction::BeamTick
    );
    assert_eq!(pool.current(), 7, "idle time primes only one pulse");
}

#[test]
fn egon_exact_multiple_and_changing_steps_leave_no_zero_time_backlog() {
    for oversized in [0.3, 0.5, 0.7, 1.0] {
        let (mut state, mut pool) = drawn(WeaponId::Egon, AmmoType::Uranium, 100);
        assert_eq!(
            state.tick(0.01, primary(), &mut pool),
            WeaponAction::BeamTick
        );
        assert_eq!(
            state.tick(oversized, primary(), &mut pool),
            WeaponAction::BeamTick
        );
        assert_eq!(
            state.tick(0.0, primary(), &mut pool),
            WeaponAction::Empty,
            "an exact {oversized}s multiple leaves no float-rounding backlog"
        );
        for (dt, expected) in [
            (0.03, WeaponAction::Empty),
            (0.07, WeaponAction::BeamTick),
            (0.06, WeaponAction::Empty),
            (0.04, WeaponAction::BeamTick),
        ] {
            assert_eq!(state.tick(dt, primary(), &mut pool), expected);
            assert_eq!(state.tick(0.0, primary(), &mut pool), WeaponAction::Empty);
        }
        assert_eq!(pool.current(), 96, "four pulses each buy one cell");
    }
}

#[test]
fn gauss_charge_depends_on_elapsed_seconds_not_step_count() {
    for (dt, steps) in [(0.01, 500), (0.02, 250)] {
        let (mut state, mut pool) = drawn(WeaponId::Gauss, AmmoType::Uranium, 100);
        let secondary = WeaponInput {
            secondary: true,
            ..WeaponInput::default()
        };
        state.tick(0.0, secondary, &mut pool);
        for _ in 0..steps {
            assert_eq!(state.tick(dt, secondary, &mut pool), WeaponAction::Empty);
            assert_eq!(
                pool.current(),
                100,
                "charge drain remains an unresolved placeholder"
            );
        }
        assert!(matches!(
            state.tick(0.0, WeaponInput::default(), &mut pool),
            WeaponAction::Hitscan { count: 1, .. }
        ));
        let damage = state.take_charge_damage().expect("one charged release");
        assert!(
            (damage - 112.5).abs() < 0.01,
            "the project-authored linear curve at five seconds"
        );
        assert_eq!(pool.current(), 99);
        assert!(state.take_charge_damage().is_none());
    }
}

#[test]
fn hornet_regeneration_is_unimplemented_rather_than_awarded_per_step() {
    for (dt, steps) in [(0.01, 1_000), (0.1, 100)] {
        let (mut state, mut pool) = drawn(WeaponId::HornetGun, AmmoType::Hornets, 0);
        state.set_clip(1);
        assert!(matches!(
            state.tick(dt, primary(), &mut pool),
            WeaponAction::SpawnProjectile { .. }
        ));
        for _ in 0..steps {
            state.tick(dt, WeaponInput::default(), &mut pool);
        }
        assert_eq!(state.clip(), 0, "TODO(black-box): regeneration interval");
        assert_eq!(pool.current(), 0);
        assert!(!matches!(
            state.tick(dt, primary(), &mut pool),
            WeaponAction::SpawnProjectile { .. }
        ));
    }
}
