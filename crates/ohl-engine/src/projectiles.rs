//! Projectiles, explosions and placed deployables, engine-side.
//!
//! [`ProjectileSystem`] is [`crate::systems::Systems`]' phase-7 body: it
//! owns `ohl_combat`'s [`ProjectileSet`] and [`DeployableSet`], builds the
//! [`ProjectileWorld`] they sweep through from the level's collision hulls
//! and phase 5's [`HitboxIndex`] (`crate::combat::rebuild_hitbox_index`,
//! owned by `crate::systems::Systems` and already excluding this module's
//! own model-backed entities — see [`ProjectileSystem::model_entities`] —
//! so a rocket cannot detonate on its own drawn model), and turns their
//! events into either a queued
//! [`DamageInfo`] (direct hits and blast damage, by calling
//! [`ohl_combat::radius_damage`]) or a [`TransientSprite`] (impacts,
//! detonations). Model-backed projectile kinds (a rocket, a bolt, a
//! hornet, a snark) each get a real `hecs` entity carrying [`StudioAnim`]
//! so the existing entity-driven render path
//! (`crates/ohl-engine/src/render.rs`) draws them for free; the
//! [`ProjectileId`] <-> [`Entity`] mapping is this module's own.
//!
//! # Numbers this module invents
//!
//! Nothing here is published: `ohl_combat::explosion` already documents
//! that no Half-Life explosive's blast radius is public, and the same is
//! true of a bolt or hornet's direct-hit damage outside a full weapon-spec
//! table (`ohl_combat::weapons`, not yet wired to a projectile's *impact*
//! rather than its *firing*). [`BlastSpec`] and [`ImpactDamage`] hold this
//! module's own placeholder constants, each marked `// TODO(black-box)`.

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_combat::{
    BlastTarget, DamageInfo, DamageType, DeployableEvent, DeployableId, DeployableSet,
    DeployableTuning, EntityId as CombatEntityId, ExplosionRule, HitboxIndex, ProjectileEvent,
    ProjectileId, ProjectileKind, ProjectileSet, ProjectileTuning, ProjectileWorld, radius_damage,
};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_physics::MoveConfig;

use crate::components::{Owner, StudioAnim};
use crate::ids::{entity_id, entity_of};
use crate::level::Level;
use crate::sprites::{TransientSprite, TransientSprites};
use crate::systems::QueuedDamage;

/// Which studio-model slot (into `Level::studio_models`) draws each
/// projectile kind, when one has been configured. A flat, linearly-scanned
/// list rather than a `HashMap`: it holds at most one entry per
/// [`ProjectileKind`] variant (six, today), so a scan costs nothing and
/// stays deterministic without needing `Hash`/`Ord` on the key.
type ModelTable = Vec<(ProjectileKind, usize)>;

/// This module's own placeholder blast parameters, per detonating kind.
///
/// TODO(black-box): no Half-Life explosive's blast radius or damage total
/// is published (`ohl_combat::explosion`'s module doc); these are a
/// project-chosen, explicitly unverified starting point.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BlastSpec {
    radius: f32,
    damage: f32,
}

/// The blast a detonating projectile or deployable applies, or `None` for a
/// kind that never detonates (a bolt, a hornet, a snark's bite).
const fn projectile_blast(kind: ProjectileKind) -> Option<BlastSpec> {
    match kind {
        // TODO(black-box): placeholder radius/damage; see the module doc.
        ProjectileKind::Rocket => Some(BlastSpec {
            radius: 250.0,
            damage: 120.0,
        }),
        // TODO(black-box): as above; the hand grenade's *fuse* (5 s) is
        // published (`ohl_combat::projectile::HAND_GRENADE_FUSE_SECONDS`),
        // its blast is not.
        ProjectileKind::HandGrenade | ProjectileKind::Mp5Grenade => Some(BlastSpec {
            radius: 200.0,
            damage: 100.0,
        }),
        ProjectileKind::CrossbowBolt | ProjectileKind::Hornet | ProjectileKind::Snark => None,
    }
}

/// Direct-hit damage for a kind that stops on impact instead of exploding.
///
/// TODO(black-box): unpublished; see the module doc.
const fn direct_impact_damage(kind: ProjectileKind) -> Option<(f32, DamageType)> {
    match kind {
        ProjectileKind::CrossbowBolt => Some((40.0, DamageType::BULLET)),
        ProjectileKind::Hornet => Some((15.0, DamageType::BULLET)),
        ProjectileKind::Snark => Some((5.0, DamageType::GENERIC)),
        ProjectileKind::Rocket | ProjectileKind::HandGrenade | ProjectileKind::Mp5Grenade => None,
    }
}

/// TODO(black-box): impact/explosion transient-sprite lifetime and scale;
/// see `crate::sprites`.
const IMPACT_SPRITE_SECONDS: f32 = 0.25;
const EXPLOSION_SPRITE_SECONDS: f32 = 0.5;
const IMPACT_SPRITE_SCALE: f32 = 1.0;
const EXPLOSION_SPRITE_SCALE: f32 = 4.0;

/// Owns the simulated projectiles and placed deployables for one level.
pub(crate) struct ProjectileSystem {
    projectiles: ProjectileSet,
    deployables: DeployableSet,
    tuning: ProjectileTuning,
    deployable_tuning: DeployableTuning,
    explosion_rule: ExplosionRule,
    movement: MoveConfig,
    models: BTreeMap<ProjectileId, Entity>,
    model_table: ModelTable,
}

impl ProjectileSystem {
    /// An empty system seeded for its (currently only) source of
    /// randomness: a wandering snark's hop direction.
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            projectiles: ProjectileSet::new(ohl_combat::ProjectileLimits::default(), seed),
            deployables: DeployableSet::new(),
            tuning: ProjectileTuning::default(),
            deployable_tuning: DeployableTuning::default(),
            explosion_rule: ExplosionRule::default(),
            movement: MoveConfig::default(),
            models: BTreeMap::new(),
            model_table: ModelTable::new(),
        }
    }

    /// How many projectiles and placed deployables are currently live, for
    /// [`crate::Game::projectile_count`].
    pub(crate) fn count(&self) -> usize {
        self.projectiles.len()
            + self.deployables.satchels().len()
            + self.deployables.tripmines().len()
    }

    /// The model-backed entities this system owns (a flying rocket, a
    /// placed tripmine, ...), for phase 5's hitbox rebuild
    /// (`crate::combat::rebuild_hitbox_index`) to exclude: a projectile's
    /// own drawn model must not be able to stop its own sweep in phase 7.
    pub(crate) fn model_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.models.values().copied()
    }

    /// Points `kind`'s model-backed rendering at one of
    /// `Level::studio_models`'s slots. `None` removes any mapping, so the
    /// kind is drawn as a transient sprite (or not at all) instead. See the
    /// module doc: no new asset is loaded here, only an existing slot is
    /// named.
    #[allow(dead_code)]
    pub(crate) fn set_model_for(&mut self, kind: ProjectileKind, model: Option<usize>) {
        self.model_table.retain(|(entry, _)| *entry != kind);
        if let Some(model) = model {
            self.model_table.push((kind, model));
        }
    }

    fn model_for(&self, kind: ProjectileKind) -> Option<usize> {
        self.model_table
            .iter()
            .find(|(entry, _)| *entry == kind)
            .map(|(_, model)| *model)
    }

    /// Spawns one projectile, and — when `kind` has a configured model — a
    /// backing `hecs` entity carrying [`StudioAnim`] so the ordinary
    /// entity-driven render path draws it.
    ///
    /// This is the hook `docs/m79-design.md` §8 P3 asks for so a monster's
    /// ranged attack (a later package) can spawn a projectile without a new
    /// dependency edge: `Systems::spawn_projectile` just forwards here.
    pub(crate) fn spawn(
        &mut self,
        level: &mut Level,
        kind: ProjectileKind,
        owner: Option<Entity>,
        position: Vec3,
        velocity: Vec3,
    ) -> Option<ProjectileId> {
        let owner_id = owner.map(entity_id);
        let id = self
            .projectiles
            .spawn(kind, owner_id, position, velocity, &self.tuning)?;
        if let Some(model) = self.model_for(kind) {
            let yaw = if velocity.length_squared() > f32::EPSILON {
                velocity.y.atan2(velocity.x).to_degrees()
            } else {
                0.0
            };
            let entity = level.registry.world.spawn((
                Transform {
                    origin: position,
                    angles: Vec3::new(0.0, yaw, 0.0),
                },
                StudioAnim::new(model, 0),
            ));
            if let Some(owner) = owner {
                let _ = level.registry.world.insert_one(entity, Owner(owner));
            }
            self.models.insert(id, entity);
        }
        Some(id)
    }

    /// Places a satchel charge; see [`ohl_combat::DeployableSet::place_satchel`].
    #[allow(dead_code)]
    pub(crate) fn place_satchel(
        &mut self,
        owner: Option<Entity>,
        position: Vec3,
    ) -> Option<DeployableId> {
        let mut events = Vec::new();
        self.deployables
            .place_satchel(owner.map(entity_id), position, &mut events)
    }

    /// Sets off every satchel this owner (or, when `owner` is `None`, every
    /// owner-less satchel) has placed, applying radius damage immediately —
    /// `ohl_combat::DeployableSet::detonate_all_satchels` is itself
    /// synchronous, unlike the tripmine beam `tick` resolves per step.
    #[allow(dead_code)]
    pub(crate) fn detonate_all_satchels(
        &mut self,
        level: &Level,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) -> usize {
        let mut events = Vec::new();
        let count = self
            .deployables
            .detonate_all_satchels(&self.deployable_tuning, &mut events);
        for event in events {
            self.apply_deployable_event(level, event, damage_queue, sprites);
        }
        count
    }

    /// Places a tripmine on whatever `from -> from + direction * place_range`
    /// runs into. `None` when the level has no collision hulls or the
    /// underlying placement trace fails.
    #[allow(dead_code)]
    pub(crate) fn place_tripmine(
        &mut self,
        level: &Level,
        owner: Option<Entity>,
        from: Vec3,
        direction: Vec3,
    ) -> Option<DeployableId> {
        let collision = level.collision.as_ref()?;
        let mut events = Vec::new();
        self.deployables.place_tripmine(
            owner.map(entity_id),
            from,
            direction,
            collision,
            &self.deployable_tuning,
            &mut events,
        )
    }

    /// Advances every projectile and deployable by `dt` seconds, queuing
    /// the damage and transient sprites their events imply.
    ///
    /// `hitboxes` is phase 5's rebuild (`crate::combat::rebuild_hitbox_index`,
    /// owned by `crate::systems::Systems`), already excluding this system's
    /// own model-backed entities (see [`Self::model_entities`]) — this
    /// system traces against it rather than rebuilding its own index.
    pub(crate) fn tick(
        &mut self,
        level: &mut Level,
        hitboxes: &HitboxIndex,
        dt: f32,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) {
        if level.collision.is_none() {
            // No usable collision hulls: nothing for a swept trace to hit,
            // and no world for a tripmine to stick to. Ages nothing rather
            // than guessing.
            return;
        }

        let mut events = Vec::new();
        {
            // Scoped so the immutable borrow of `level.collision` ends
            // before `sync_model_entities` needs `level` mutably.
            let world = ProjectileWorld {
                collision: level.collision.as_ref().expect("checked above"),
                entities: hitboxes,
                movement: &self.movement,
                tuning: &self.tuning,
            };
            self.projectiles.tick(dt, &world, &mut events);
        }
        for event in events {
            self.apply_projectile_event(level, event, damage_queue, sprites);
        }
        self.sync_model_entities(level);

        let collision = level.collision.as_ref().expect("checked above");
        let mut deploy_events = Vec::new();
        self.deployables.tick(
            dt,
            collision,
            hitboxes,
            &self.deployable_tuning,
            &mut deploy_events,
        );
        for event in deploy_events {
            self.apply_deployable_event(level, event, damage_queue, sprites);
        }
        // Sprites are aged in phase 13 (`Systems::presentation`), not here:
        // `docs/m79-design.md` §4 assigns transient-sprite aging to
        // presentation, alongside the HUD and sound cues.
    }

    fn apply_projectile_event(
        &mut self,
        level: &Level,
        event: ProjectileEvent,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) {
        match event {
            ProjectileEvent::Impact {
                id,
                kind,
                position,
                entity,
                ..
            } => {
                push_sprite(
                    sprites,
                    level,
                    position,
                    IMPACT_SPRITE_SECONDS,
                    IMPACT_SPRITE_SCALE,
                );
                if let (Some(target), Some((amount, damage_kind))) =
                    (entity.and_then(entity_of), direct_impact_damage(kind))
                {
                    let attacker = self.projectiles.get(id).and_then(|p| p.owner);
                    damage_queue.push(QueuedDamage {
                        target,
                        info: DamageInfo {
                            attacker,
                            inflictor: attacker,
                            amount,
                            kind: damage_kind,
                            origin: position,
                            direction: Vec3::ZERO,
                        },
                    });
                }
            }
            ProjectileEvent::Detonate { id, kind, position } => {
                push_sprite(
                    sprites,
                    level,
                    position,
                    EXPLOSION_SPRITE_SECONDS,
                    EXPLOSION_SPRITE_SCALE,
                );
                if let Some(spec) = projectile_blast(kind) {
                    let attacker = self.projectiles.get(id).and_then(|p| p.owner);
                    resolve_blast(
                        level,
                        position,
                        spec.radius,
                        spec.damage,
                        DamageType::BLAST,
                        attacker,
                        &self.explosion_rule,
                        damage_queue,
                    );
                }
            }
            // `ProjectileEvent` is `#[non_exhaustive]`; `Expired` and any
            // future variant this crate does not yet know about both need
            // nothing more than what `sync_model_entities` already does
            // (removing the entity for a projectile no longer in flight).
            _ => {}
        }
    }

    fn apply_deployable_event(
        &mut self,
        level: &Level,
        event: DeployableEvent,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) {
        if let DeployableEvent::Detonated {
            position,
            owner,
            radius,
            ..
        } = event
        {
            push_sprite(
                sprites,
                level,
                position,
                EXPLOSION_SPRITE_SECONDS,
                EXPLOSION_SPRITE_SCALE,
            );
            // TODO(black-box): the deployables' own published damage
            // (150, `ohl_combat::deployables`' module doc) is a per-weapon
            // spec value this package does not yet look up by kind; a
            // project-chosen 150 stands in until that wiring lands.
            resolve_blast(
                level,
                position,
                radius,
                150.0,
                DamageType::BLAST,
                owner,
                &self.explosion_rule,
                damage_queue,
            );
        }
    }

    /// Keeps each model-backed projectile's entity in step with its
    /// simulated position, and removes the ones that no longer exist.
    fn sync_model_entities(&mut self, level: &mut Level) {
        let mut gone = Vec::new();
        for (&id, &entity) in &self.models {
            let Some(projectile) = self.projectiles.get(id) else {
                gone.push(id);
                continue;
            };
            let yaw = if projectile.velocity.length_squared() > f32::EPSILON {
                projectile
                    .velocity
                    .y
                    .atan2(projectile.velocity.x)
                    .to_degrees()
            } else {
                0.0
            };
            if let Ok(mut transform) = level.registry.world.get::<&mut Transform>(entity) {
                transform.origin = projectile.position;
                transform.angles = Vec3::new(0.0, yaw, 0.0);
            }
        }
        for id in gone {
            if let Some(entity) = self.models.remove(&id) {
                let _ = level.registry.world.despawn(entity);
            }
        }
    }
}

/// Applies `radius_damage` at `position` and queues every hit it reports.
#[allow(clippy::too_many_arguments)]
fn resolve_blast(
    level: &Level,
    position: Vec3,
    radius: f32,
    damage: f32,
    kind: DamageType,
    attacker: Option<CombatEntityId>,
    rule: &ExplosionRule,
    damage_queue: &mut Vec<QueuedDamage>,
) {
    let Some(collision) = level.collision.as_ref() else {
        return;
    };
    let targets = blast_targets(level);
    let hits = radius_damage(
        position,
        radius,
        damage,
        kind,
        attacker,
        targets.into_iter(),
        collision,
        rule,
    );
    damage_queue.extend(hits.into_iter().filter_map(|hit| {
        entity_of(hit.target).map(|target| QueuedDamage {
            target,
            info: hit.damage,
        })
    }));
}

/// Every entity a blast may hurt: everything carrying `Health`, positioned
/// by its `Transform`. `ohl-ai`'s `Actor`-carrying monsters are not a
/// dependency of this package yet (see `crate::components`'s note); the
/// player, spawned in `crate::level`, already qualifies.
fn blast_targets(level: &Level) -> Vec<BlastTarget> {
    let mut targets = Vec::new();
    for (entity, transform, _health) in
        &mut level
            .registry
            .world
            .query::<(ohl_game::hecs::Entity, &Transform, &ohl_combat::Health)>()
    {
        targets.push(BlastTarget::new(entity_id(entity), transform.origin));
    }
    targets
}

/// Pushes a transient sprite at `position`, when the level has published at
/// least one sprite asset to draw it with (see `crate::sprites`'s module
/// doc). A map with none simply draws nothing for this event.
fn push_sprite(
    sprites: &mut TransientSprites,
    level: &Level,
    position: Vec3,
    seconds: f32,
    scale: f32,
) {
    if level.sprite_assets.is_empty() {
        return;
    }
    sprites.push(TransientSprite {
        asset: 0,
        origin: position.to_array(),
        scale,
        render: ohl_render::RenderProps::from_entity(5, 255, [255, 255, 255], 0),
        seconds_left: seconds,
        age: 0.0,
    });
}

#[cfg(test)]
mod tests {
    use ohl_combat::{
        BlastTarget, DamageType, EntityId as CombatEntityId, ExplosionRule,
        HAND_GRENADE_FUSE_SECONDS, ProjectileKind, radius_damage,
    };
    use proptest::prelude::*;

    use super::ProjectileSystem;
    use crate::assets::MemoryAssets;
    use crate::ids::entity_id;
    use crate::level::Level;
    use crate::sprites::TransientSprites;
    use crate::test_support::synthetic_map_bsp;
    use ohl_combat::{HitboxIndex, HitboxLimits};

    fn synthetic_level() -> Level {
        let assets = MemoryAssets::new();
        let bytes = synthetic_map_bsp();
        Level::from_bytes(&assets, "ohlsynth", &bytes).expect("the synthetic map loads")
    }

    /// An empty hitbox index: these tests exercise the swept world-collision
    /// path and the deployable/blast logic, neither of which needs an entity
    /// hitbox to be present.
    fn empty_hitboxes() -> HitboxIndex {
        HitboxIndex::new(HitboxLimits::default())
    }

    #[test]
    fn a_grenade_never_tunnels_through_a_wall() {
        let mut level = synthetic_level();
        let mut system = ProjectileSystem::new(0);
        let mut damage = Vec::new();
        let mut sprites = TransientSprites::new();

        // `ohl_formats::test_support::collision_room_brushes` (which the
        // engine's own synthetic fixture builds its hulls from) walls the
        // room at `x/y = +/-256`. Fire straight at the +X wall fast enough
        // that a naive (non-swept) integration would step clean through it
        // in a single substep.
        let position = glam::Vec3::new(0.0, 0.0, 40.0);
        let velocity = glam::Vec3::new(4000.0, 0.0, 0.0);
        system.spawn(
            &mut level,
            ProjectileKind::HandGrenade,
            None,
            position,
            velocity,
        );

        for _ in 0..30 {
            system.tick(
                &mut level,
                &empty_hitboxes(),
                1.0 / 30.0,
                &mut damage,
                &mut sprites,
            );
        }

        // Whatever is left in flight (a grenade bounces, so it may still be
        // live) must be within the room's bounds; one that detonated leaves
        // nothing to check, which is also a pass.
        for projectile in system.projectiles.projectiles() {
            assert!(
                projectile.position.x.abs() <= 256.0 + 1.0,
                "a projectile ended up outside the room: {:?}",
                projectile.position
            );
        }
    }

    #[test]
    fn radius_damage_falls_off_monotonically_and_respects_line_of_sight() {
        let level = synthetic_level();
        let collision = level
            .collision
            .as_ref()
            .expect("the synthetic fixture has collision hulls");

        let center = glam::Vec3::new(0.0, 0.0, 40.0);
        let radius = 200.0;
        let max_damage = 100.0;
        let near = BlastTarget::new(CombatEntityId(1), glam::Vec3::new(20.0, 0.0, 40.0));
        let mid = BlastTarget::new(CombatEntityId(2), glam::Vec3::new(80.0, 0.0, 40.0));
        // Outside the room (walls sit at `x = +/-256`): the trace from the
        // blast to this point must cross a wall and be occluded.
        let behind_wall = BlastTarget::new(CombatEntityId(3), glam::Vec3::new(400.0, 0.0, 40.0));

        let hits = radius_damage(
            center,
            radius,
            max_damage,
            DamageType::BLAST,
            None,
            [near, mid, behind_wall].into_iter(),
            collision,
            &ExplosionRule::default(),
        );

        let near_hit = hits
            .iter()
            .find(|hit| hit.target == CombatEntityId(1))
            .expect("the near target is hit");
        let mid_hit = hits
            .iter()
            .find(|hit| hit.target == CombatEntityId(2))
            .expect("the mid target is hit");
        assert!(
            near_hit.damage.amount > mid_hit.damage.amount,
            "damage must fall off monotonically with distance"
        );
        assert!(
            hits.iter().all(|hit| hit.target != CombatEntityId(3)),
            "a target behind a wall must take no damage"
        );
    }

    #[test]
    fn a_tripmine_arms_after_the_published_three_seconds() {
        let mut level = synthetic_level();
        let mut system = ProjectileSystem::new(0);
        // Place a mine against the floor, right under the player start.
        let placed = system.place_tripmine(
            &level,
            None,
            glam::Vec3::new(0.0, 0.0, 40.0),
            glam::Vec3::new(0.0, 0.0, -1.0),
        );
        assert!(placed.is_some(), "the placement trace must find the floor");

        let mut damage = Vec::new();
        let mut sprites = TransientSprites::new();
        // Just under the arming delay: the beam must not be live yet.
        for _ in 0..(2 * 30) {
            system.tick(
                &mut level,
                &empty_hitboxes(),
                1.0 / 30.0,
                &mut damage,
                &mut sprites,
            );
        }
        assert_eq!(system.deployables.tripmines().len(), 1);
        assert!(!system.deployables.tripmines()[0].armed);

        // Past the published three seconds: it must be armed now.
        for _ in 0..(2 * 30) {
            system.tick(
                &mut level,
                &empty_hitboxes(),
                1.0 / 30.0,
                &mut damage,
                &mut sprites,
            );
        }
        assert!(system.deployables.tripmines()[0].armed);
    }

    #[test]
    fn a_satchel_set_off_by_its_owner_damages_the_owner() {
        let mut level = synthetic_level();
        let owner = level.registry.world.spawn((
            ohl_game::registry::Transform {
                origin: glam::Vec3::new(10.0, 0.0, 40.0),
                angles: glam::Vec3::ZERO,
            },
            ohl_combat::Health::new(100.0),
        ));

        let mut system = ProjectileSystem::new(0);
        system.place_satchel(Some(owner), glam::Vec3::new(0.0, 0.0, 40.0));

        let mut damage_queue = Vec::new();
        let mut sprites = TransientSprites::new();
        system.detonate_all_satchels(&level, &mut damage_queue, &mut sprites);

        let owner_id = entity_id(owner);
        assert!(
            damage_queue
                .iter()
                .any(|queued| queued.info.attacker == Some(owner_id) && queued.info.amount > 0.0),
            "the owner must be among the blast's own hits"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn arbitrary_spawns_never_panic_and_stay_resolved_or_in_bounds(
            vx in -3000.0f32..3000.0,
            vy in -3000.0f32..3000.0,
            vz in -1000.0f32..1000.0,
            dt in 0.0001f32..0.5,
            ticks in 1u32..20,
        ) {
            let mut level = synthetic_level();
            let mut system = ProjectileSystem::new(1);
            let mut damage = Vec::new();
            let mut sprites = TransientSprites::new();
            system.spawn(
                &mut level,
                ProjectileKind::HandGrenade,
                None,
                glam::Vec3::new(0.0, 0.0, 40.0),
                glam::Vec3::new(vx, vy, vz),
            );
            for _ in 0..ticks {
                system.tick(&mut level, &empty_hitboxes(), dt, &mut damage, &mut sprites);
            }
            for projectile in system.projectiles.projectiles() {
                prop_assert!(projectile.position.is_finite());
                prop_assert!(projectile.position.x.abs() <= 4000.0);
                prop_assert!(projectile.position.y.abs() <= 4000.0);
            }
        }
    }

    #[test]
    fn the_hand_grenade_fuse_constant_is_unchanged_by_this_module() {
        // A guard against accidentally shadowing the published constant
        // with a local placeholder of the same name.
        assert!((HAND_GRENADE_FUSE_SECONDS - 5.0).abs() < f32::EPSILON);
    }
}
