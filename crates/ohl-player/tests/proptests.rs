//! Property tests: the player systems never panic, and health, armor, air
//! and flashlight charge never leave their documented ranges, whatever the
//! host feeds them.

use ohl_physics::{LiquidKind, WaterLevel};
use ohl_player::{
    DamageKind, EmptyWorld, HurtInput, PhysicsOutput, Player, PlayerEvent, PlayerInput,
    PlayerSystems,
};
use proptest::prelude::*;

fn any_water_level() -> impl Strategy<Value = WaterLevel> {
    prop_oneof![
        Just(WaterLevel::Dry),
        Just(WaterLevel::Feet),
        Just(WaterLevel::Waist),
        Just(WaterLevel::Eyes),
    ]
}

fn any_liquid() -> impl Strategy<Value = LiquidKind> {
    prop_oneof![
        Just(LiquidKind::None),
        Just(LiquidKind::Water),
        Just(LiquidKind::Slime),
        Just(LiquidKind::Lava),
    ]
}

fn any_damage_kind() -> impl Strategy<Value = DamageKind> {
    prop_oneof![
        Just(DamageKind::Generic),
        Just(DamageKind::Fall),
        Just(DamageKind::Drown),
        Just(DamageKind::Burn),
        Just(DamageKind::Freeze),
        Just(DamageKind::Shock),
        Just(DamageKind::Acid),
        Just(DamageKind::Radiation),
        Just(DamageKind::Chemical),
        Just(DamageKind::Crush),
        Just(DamageKind::Blast),
    ]
}

fn check_invariants(player: &Player) -> Result<(), TestCaseError> {
    let state = &player.state;
    prop_assert!(state.health.is_finite());
    prop_assert!(state.health >= 0.0);
    prop_assert!(state.health <= player.config.max_health);
    prop_assert!(state.armor.is_finite());
    prop_assert!(state.armor >= 0.0);
    prop_assert!(state.armor <= player.config.max_armor);
    prop_assert!(state.air_time.is_finite());
    prop_assert!(state.air_time >= 0.0);
    prop_assert!(state.air_time <= player.config.air_capacity_seconds);
    prop_assert!(state.flashlight.charge.is_finite());
    prop_assert!((0.0..=1.0).contains(&state.flashlight.charge));
    prop_assert!(state.waterlevel <= 3);
    prop_assert_eq!(state.dead, state.health <= 0.0);
    prop_assert!(state.display_health() >= 0);
    prop_assert!(state.display_armor() >= 0);
    Ok(())
}

proptest! {
    #[test]
    fn ticking_with_arbitrary_input_keeps_every_value_in_range(
        dt in prop_oneof![Just(f32::NAN), Just(0.0f32), Just(-1.0f32), 0.001f32..0.5],
        suit in any::<bool>(),
        flashlight in any::<bool>(),
        landed in proptest::option::of(-5000.0f32..5000.0),
        water in any_water_level(),
        liquid in any_liquid(),
        hurt_dmg in -1000.0f32..1000.0,
        hurt_type in 0u32..64,
        ticks in 1usize..60,
    ) {
        let mut player = Player::default();
        let mut setup = Vec::new();
        if suit {
            player.equip_suit(&mut setup);
            player.add_armor(100.0, &mut setup);
        }

        let mut input = PlayerInput { flashlight_pressed: flashlight, ..PlayerInput::default() };
        input.push_hurt(HurtInput { damage_per_second: hurt_dmg, damage_type: hurt_type });
        let physics = PhysicsOutput {
            water_level: water,
            liquid,
            landed_speed: landed,
            ..PhysicsOutput::default()
        };

        for _ in 0..ticks {
            let events = player.tick(dt, &input, &physics, &EmptyWorld);
            prop_assert!(events.len() <= ohl_player::systems::MAX_EVENTS_PER_TICK);
            for event in &events {
                if let PlayerEvent::Damaged { amount, .. } = event {
                    prop_assert!(*amount >= 0);
                }
            }
            check_invariants(&player)?;
        }
    }

    #[test]
    fn arbitrary_damage_and_healing_keeps_health_in_range(
        hits in proptest::collection::vec(
            (-1e9f32..1e9, any_damage_kind()),
            0..32,
        ),
    ) {
        let mut player = Player::default();
        let mut events = Vec::new();
        player.equip_suit(&mut events);
        player.add_armor(100.0, &mut events);

        for (amount, kind) in hits {
            events.clear();
            if amount >= 0.0 {
                player.apply_damage(amount, kind, &mut events);
            } else {
                player.heal(-amount, &mut events);
            }
            check_invariants(&player)?;
        }
    }

    #[test]
    fn a_snapshot_always_restores_to_itself(
        health in 0.0f32..100.0,
        armor in 0.0f32..100.0,
        air in 0.0f32..12.0,
        charge in 0.0f32..1.0,
        waterlevel in 0u8..4,
    ) {
        let mut player = Player::default();
        let mut events = Vec::new();
        player.equip_suit(&mut events);
        player.state.health = health;
        player.state.armor = armor;
        player.state.air_time = air;
        player.state.flashlight.charge = charge;
        player.state.waterlevel = waterlevel;
        player.state.dead = health <= 0.0;

        let snapshot = player.snapshot();
        let mut restored = Player::default();
        restored.restore(&snapshot);
        prop_assert_eq!(restored.snapshot(), snapshot);
        check_invariants(&restored)?;
    }

    #[test]
    fn absorption_never_produces_a_negative_result(
        damage in -1e6f32..1e6,
        armor in -100.0f32..1000.0,
        kind in any_damage_kind(),
    ) {
        let absorbed = ohl_player::absorb(damage, armor, kind);
        prop_assert!(absorbed.health_loss.is_finite());
        prop_assert!(absorbed.health_loss >= 0.0);
        prop_assert!(absorbed.armor_left.is_finite());
        prop_assert!(absorbed.armor_left >= 0.0);
        if damage > 0.0 && armor > 0.0 && !kind.bypasses_armor() {
            // Armor never grows and never absorbs more than it has.
            prop_assert!(absorbed.armor_left <= armor);
        }
    }

    #[test]
    fn fall_damage_is_monotonic_in_the_impact_speed(
        slower in 0.0f32..4000.0,
        extra in 0.0f32..4000.0,
    ) {
        let faster = slower + extra;
        prop_assert!(ohl_player::fall_damage(faster) >= ohl_player::fall_damage(slower));
        prop_assert!(ohl_player::fall_damage(slower) >= 0.0);
    }
}
