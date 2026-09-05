//! Property tests for projectile sweeping and radius damage.
//!
//! The anti-tunnelling property is the important one: a grenade thrown at
//! any speed and any angle inside the synthetic room must never end a tick
//! inside solid geometry, and must stay inside the room.

use ohl_combat::explosion::falloff;
use ohl_combat::projectile::{
    ProjectileEvent, ProjectileKind, ProjectileLimits, ProjectileSet, ProjectileTuning,
    ProjectileWorld,
};
use ohl_combat::{HitboxIndex, HitboxLimits, Vec3};
use ohl_formats::bsp30::{Bsp, Limits};
use ohl_formats::test_support::build_collision_room_bsp;
use ohl_physics::{CollisionModel, Hull, MoveConfig, contents};
use proptest::prelude::*;

const TICK: f32 = 0.01;

fn room() -> CollisionModel {
    let bytes = build_collision_room_bsp();
    let limits = Limits::default();
    let bsp = Bsp::parse(&bytes, &limits).expect("fixture parses as BSP v30");
    CollisionModel::from_bsp(&bsp, &limits).expect("fixture has usable collision hulls")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// A grenade thrown at any speed from any point in the room's open
    /// middle never tunnels: every tick leaves it in open space, inside the
    /// room.
    #[test]
    fn a_grenade_never_ends_a_tick_inside_solid(
        speed in 1.0f32..12_000.0,
        yaw in -core::f32::consts::PI..core::f32::consts::PI,
        pitch in -1.5f32..1.5,
        start_x in -50.0f32..50.0,
        start_y in -50.0f32..50.0,
        start_z in 8.0f32..200.0,
    ) {
        let world = room();
        let entities = HitboxIndex::new(HitboxLimits::default());
        let movement = MoveConfig::default();
        let tuning = ProjectileTuning::default();
        let context = ProjectileWorld {
            collision: &world,
            entities: &entities,
            movement: &movement,
            tuning: &tuning,
        };
        let direction = Vec3::new(
            yaw.cos() * pitch.cos(),
            yaw.sin() * pitch.cos(),
            pitch.sin(),
        );
        let mut set = ProjectileSet::new(ProjectileLimits::default(), 0);
        set.spawn(
            ProjectileKind::HandGrenade,
            None,
            Vec3::new(start_x, start_y, start_z),
            direction * speed,
            &tuning,
        );

        let mut events = Vec::new();
        for _ in 0..500 {
            set.tick(TICK, &context, &mut events);
            for projectile in set.projectiles() {
                prop_assert!(projectile.position.is_finite());
                prop_assert!(
                    !contents::is_solid(world.contents_at(Hull::Point, projectile.position)),
                    "ended a tick inside solid at {:?}",
                    projectile.position
                );
                prop_assert!(
                    projectile.position.x.abs() <= 257.0
                        && projectile.position.y.abs() <= 257.0
                        && projectile.position.z >= -1.0
                        && projectile.position.z <= 257.0,
                    "left the room at {:?}",
                    projectile.position
                );
            }
        }
        // The fuse is 5 seconds and the run is 5, so it always goes off.
        let detonated = events
            .iter()
            .any(|event| matches!(event, ProjectileEvent::Detonate { .. }));
        prop_assert!(detonated, "the five second fuse always runs out");
    }

    /// Ticking never panics and never leaves a projectile with a non-finite
    /// state, whatever the step and the launch velocity.
    #[test]
    fn ticking_is_total(
        dt in -1.0f32..1.0,
        vx in -20_000.0f32..20_000.0,
        vy in -20_000.0f32..20_000.0,
        vz in -20_000.0f32..20_000.0,
        kind in 0usize..6,
    ) {
        let kinds = [
            ProjectileKind::CrossbowBolt,
            ProjectileKind::Rocket,
            ProjectileKind::Mp5Grenade,
            ProjectileKind::HandGrenade,
            ProjectileKind::Hornet,
            ProjectileKind::Snark,
        ];
        let world = room();
        let entities = HitboxIndex::new(HitboxLimits::default());
        let movement = MoveConfig::default();
        let tuning = ProjectileTuning::default();
        let context = ProjectileWorld {
            collision: &world,
            entities: &entities,
            movement: &movement,
            tuning: &tuning,
        };
        let mut set = ProjectileSet::new(ProjectileLimits::default(), 3);
        set.spawn(
            kinds[kind],
            None,
            Vec3::new(0.0, 0.0, 128.0),
            Vec3::new(vx, vy, vz),
            &tuning,
        );
        let mut events = Vec::new();
        for _ in 0..64 {
            set.tick(dt, &context, &mut events);
            for projectile in set.projectiles() {
                prop_assert!(projectile.position.is_finite());
                prop_assert!(projectile.velocity.is_finite());
            }
        }
    }

    /// Blast falloff is monotonically non-increasing in distance and always
    /// lands in `0..=1`.
    #[test]
    fn falloff_never_rises_with_distance(
        radius in 1.0f32..4096.0,
        near in 0.0f32..8192.0,
        extra in 0.0f32..8192.0,
    ) {
        let close = falloff(near, radius);
        let far = falloff(near + extra, radius);
        prop_assert!((0.0..=1.0).contains(&close));
        prop_assert!((0.0..=1.0).contains(&far));
        prop_assert!(far <= close + 1e-6, "{far} > {close}");
    }
}
