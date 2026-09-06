//! Projectiles, explosions and placed deployables, engine-side.
//!
//! [`ProjectileSystem`] is [`crate::systems::Systems`]' phase-7 body: it
//! owns `ohl_combat`'s [`ProjectileSet`] and [`DeployableSet`], builds the
//! [`ProjectileWorld`] they sweep through from the level's collision hulls
//! and phase 5's [`HitboxIndex`] (`crate::combat::rebuild_hitbox_index`,
//! owned by `crate::systems::Systems`), and turns their events into either
//! a queued [`DamageInfo`] (direct hits and blast damage, by calling
//! [`ohl_combat::radius_damage`]) or a [`TransientSprite`] (impacts,
//! detonations). Model-backed projectile kinds (a rocket, a bolt, a
//! hornet, a snark) and model-backed deployables (a placed satchel, a
//! placed tripmine) each get a real `hecs` entity carrying [`StudioAnim`]
//! so the existing entity-driven render path
//! (`crates/ohl-engine/src/render.rs`) draws them for free; the
//! [`ProjectileId`]/[`DeployableId`] <-> [`Entity`] mappings are this
//! module's own.
//!
//! # A shared index, never a global exclusion
//!
//! Every model-backed entity this module owns stays *in* phase 5's shared
//! [`HitboxIndex`] — a placed tripmine or satchel must stay damageable by
//! the player's hitscan and by another explosive's blast (see
//! `docs/FORMAT_SOURCES.md`, "Deployable damageability and per-trace hitbox
//! exclusion"), and a global exclusion would hide it from those traces too,
//! not just from its own. What must not happen is a projectile detonating
//! on its own drawn model or hitting its own owner mid-flight; that is
//! handled per trace instead, by [`ohl_combat::Projectile::self_id`] and
//! `owner` (set on spawn, here, from this module's own `models` map) making
//! [`ProjectileSet::tick`] ignore exactly those two entities in its own
//! sweep (`ohl_combat` ticket #57's `TraceFilter::ignoring`), leaving the
//! index itself untouched for everyone else's trace this same tick.
//!
//! # Numbers this module invents
//!
//! Nothing here is published: `ohl_combat::explosion` already documents
//! that no Half-Life explosive's blast radius is public, and the same is
//! true of a bolt or hornet's direct-hit damage outside a full weapon-spec
//! table (`ohl_combat::weapons`, not yet wired to a projectile's *impact*
//! rather than its *firing*). [`BlastSpec`] and [`ImpactDamage`] hold this
//! module's own placeholder constants, each marked `// TODO(black-box)`.
//! [`default_projectile_model_path`] and [`default_deployable_model_path`]
//! name a kind's asset path only when this project found that exact
//! literal published on a specific, linkable page (cited on the function
//! itself and in `docs/FORMAT_SOURCES.md`); a map that has not loaded that
//! exact model (this project loads none by that path itself — see
//! `Level::studio_models`'s doc) simply leaves the kind model-less, drawn
//! as a sprite or not drawn at all, never a missing-asset error.
//!
//! # Not yet persisted
//!
//! Neither [`ohl_combat::Projectile::self_id`] nor a deployable's
//! stand-in `hecs` entity (tracked in [`ProjectileSystem::models`] and
//! [`ProjectileSystem::deployable_models`]) is written by `ohl-engine`'s
//! save/restore path (PR #80's five additive save sections). A restored
//! `DeployableSet` today would come back with no stand-ins at all —
//! simulated but undrawn and, worse, undamageable again, exactly the
//! regression this module exists to fix — and a restored, still-in-flight
//! projectile would come back with `self_id: None`. Whoever picks up a
//! save-format follow-up for projectiles/deployables should re-run
//! [`ProjectileSystem::configure_models`] and re-spawn every stand-in from
//! the restored `ProjectileSet`/`DeployableSet` on load, rather than trying
//! to serialise a `hecs::Entity` handle across a save (`hecs` gives no
//! stability guarantee for those across a process, let alone a save file).

use std::collections::BTreeMap;

use glam::Vec3;
use ohl_combat::{
    BlastTarget, DamageInfo, DamageType, DeployableEvent, DeployableId, DeployableKind,
    DeployableSet, DeployableTuning, EntityId as CombatEntityId, ExplosionRule, Health,
    HitboxIndex, ProjectileEvent, ProjectileId, ProjectileKind, ProjectileSet, ProjectileTuning,
    ProjectileWorld, radius_damage,
};
use ohl_game::hecs::Entity;
use ohl_game::registry::Transform;
use ohl_physics::MoveConfig;

use crate::components::{DeployableRef, Owner, StudioAnim};
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

/// As [`ModelTable`], for [`DeployableKind`] (two variants today).
type DeployableModelTable = Vec<(DeployableKind, usize)>;

/// The asset path `kind`'s in-flight model loads under, when this project
/// found that exact literal published on a specific, linkable page, or
/// `None` for a kind it did not (see [`ProjectileSystem::configure_models`]
/// — a caller may still name any slot directly with
/// [`ProjectileSystem::set_model_for`], this is only the automatic
/// default). Per `docs/CLEAN_ROOM.md`'s per-literal citation rule, no path
/// is named here unless it is cited by URL in `docs/FORMAT_SOURCES.md`,
/// "Deployable damageability and per-trace hitbox exclusion" — none of the
/// six [`ProjectileKind`] variants has a model path this project found
/// published on a page it could re-check by URL (the page this project
/// already trusts for entity-to-model mappings, TWHL's "Reference: Entities
/// and their models", has no row for any of them), so every variant is
/// `None`.
///
/// TODO(black-box): a rocket, a bolt and a hornet plausibly do have a
/// published in-flight model somewhere; this project simply did not find a
/// citable one. Revisit if one turns up.
const fn default_projectile_model_path(_kind: ProjectileKind) -> Option<&'static str> {
    None
}

/// As [`default_projectile_model_path`], for a placed deployable. Unlike
/// that function, both of [`DeployableKind`]'s two variants have a citable
/// path today, so this returns one unconditionally rather than an
/// `Option` — see `docs/FORMAT_SOURCES.md`'s citation table for this
/// function for both.
///
/// `models/v_tripmine.mdl` ([`DeployableKind::Tripmine`]) and
/// `models/w_satchel.mdl` ([`DeployableKind::Satchel`]) are cited, by URL
/// and verbatim table row, in that table — the same TWHL "Reference:
/// Entities and their models" page `MonsterKind::default_model_path`
/// already cites for monster model paths, which also carries a
/// `monster_tripmine` and a `monster_satchel` row.
const fn default_deployable_model_path(kind: DeployableKind) -> &'static str {
    match kind {
        DeployableKind::Tripmine => "models/v_tripmine.mdl",
        DeployableKind::Satchel => "models/w_satchel.mdl",
    }
}

/// The [`Health`] a model-backed deployable's stand-in entity spawns with.
///
/// Published for the tripmine: TWHL, "monster_tripmine"
/// (<https://twhl.info/wiki/page/monster_tripmine>, reached via a
/// text-extraction proxy since `twhl.info` returns HTTP 403 to direct
/// automated fetches — the same limitation recorded elsewhere in this file
/// for other TWHL pages), whose keyvalue/property table publishes
/// "Health | 1". No source publishes a distinct number for a placed
/// satchel; this project uses the same one for consistency (any single hit
/// kills either), an explicit, unverified choice rather than a claim about
/// the satchel specifically.
const DEPLOYABLE_HEALTH: f32 = 1.0;

/// The blast damage a detonating placed deployable applies.
///
/// Published for the tripmine, on the same page and table cited on
/// [`DEPLOYABLE_HEALTH`]: "Explosive damage | 150". This project applies
/// the same number to a satchel's detonation too — `weapons::spec`'s own
/// M7.2 table already cites a separate, matching 150 for the Satchel
/// Charge weapon itself (`docs/FORMAT_SOURCES.md`, "Weapons and firing
/// (M7.2)"), so this is not a guess for the satchel, only reused here
/// rather than re-derived from `weapons::spec` by kind (that lookup is not
/// wired to a deployable's *detonation* yet, only its *firing*).
const DEPLOYABLE_BLAST_DAMAGE: f32 = 150.0;

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
    deployable_models: BTreeMap<DeployableId, Entity>,
    deployable_model_table: DeployableModelTable,
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
            deployable_models: BTreeMap::new(),
            deployable_model_table: DeployableModelTable::new(),
        }
    }

    /// How many projectiles and placed deployables are currently live, for
    /// [`crate::Game::projectile_count`].
    pub(crate) fn count(&self) -> usize {
        self.projectiles.len()
            + self.deployables.satchels().len()
            + self.deployables.tripmines().len()
    }

    /// Points `kind`'s model-backed rendering at one of
    /// `Level::studio_models`'s slots. `None` removes any mapping, so the
    /// kind is drawn as a transient sprite (or not at all) instead. See the
    /// module doc: no new asset is loaded here, only an existing slot is
    /// named.
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

    /// As [`Self::set_model_for`], for a placed deployable kind.
    pub(crate) fn set_model_for_deployable(&mut self, kind: DeployableKind, model: Option<usize>) {
        self.deployable_model_table
            .retain(|(entry, _)| *entry != kind);
        if let Some(model) = model {
            self.deployable_model_table.push((kind, model));
        }
    }

    fn deployable_model_for(&self, kind: DeployableKind) -> Option<usize> {
        self.deployable_model_table
            .iter()
            .find(|(entry, _)| *entry == kind)
            .map(|(_, model)| *model)
    }

    /// Auto-wires [`Self::set_model_for`]/[`Self::set_model_for_deployable`]
    /// for whichever kinds have a conventional model path
    /// ([`default_projectile_model_path`], [`default_deployable_model_path`])
    /// that happens to match one of `level.studio_model_paths` — i.e. that
    /// this map already loaded, for its own map-placed entities, under
    /// that exact path. Leaves an already-configured kind alone, so a
    /// caller's explicit [`Self::set_model_for`] is never overwritten by a
    /// later level attach.
    ///
    /// This is the one place `set_model_for`/`set_model_for_deployable` are
    /// called from today; a later package (a monster's own ranged attack,
    /// say) may call either directly instead.
    pub(crate) fn configure_models(&mut self, level: &Level) {
        for kind in [
            ProjectileKind::Rocket,
            ProjectileKind::CrossbowBolt,
            ProjectileKind::HandGrenade,
            ProjectileKind::Mp5Grenade,
            ProjectileKind::Hornet,
            ProjectileKind::Snark,
        ] {
            if self.model_for(kind).is_some() {
                continue;
            }
            if let Some(path) = default_projectile_model_path(kind)
                && let Some(index) = find_model_path(level, path)
            {
                self.set_model_for(kind, Some(index));
            }
        }
        for kind in [DeployableKind::Satchel, DeployableKind::Tripmine] {
            if self.deployable_model_for(kind).is_some() {
                continue;
            }
            if let Some(index) = find_model_path(level, default_deployable_model_path(kind)) {
                self.set_model_for_deployable(kind, Some(index));
            }
        }
    }

    /// The `hecs` entity standing in for a placed deployable's model, when
    /// it has one, for a caller that needs to key off it directly (tests,
    /// and any later render-side lookup).
    #[allow(dead_code)]
    pub(crate) fn deployable_entity(&self, id: DeployableId) -> Option<Entity> {
        self.deployable_models.get(&id).copied()
    }

    /// Spawns one projectile, and — when `kind` has a configured model — a
    /// backing `hecs` entity carrying [`StudioAnim`] so the ordinary
    /// entity-driven render path draws it. That entity's id becomes the
    /// spawned [`ohl_combat::Projectile::self_id`], so its *own* movement
    /// trace ignores it (see the module doc) without excluding it from
    /// anyone else's.
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
            if let Some(projectile) = self.projectiles.get_mut(id) {
                projectile.self_id = Some(entity_id(entity));
            }
        }
        Some(id)
    }

    /// Places a satchel charge; see [`ohl_combat::DeployableSet::place_satchel`].
    /// When [`DeployableKind::Satchel`] has a configured model, also spawns
    /// the backing `hecs` entity ([`StudioAnim`], [`Health`] and
    /// [`DeployableRef`]) that makes the placed charge damageable — see the
    /// module doc.
    #[allow(dead_code)]
    pub(crate) fn place_satchel(
        &mut self,
        level: &mut Level,
        owner: Option<Entity>,
        position: Vec3,
    ) -> Option<DeployableId> {
        let mut events = Vec::new();
        let id = self
            .deployables
            .place_satchel(owner.map(entity_id), position, &mut events)?;
        self.spawn_deployable_model(level, DeployableKind::Satchel, id, position, owner);
        Some(id)
    }

    /// Sets off every satchel this owner (or, when `owner` is `None`, every
    /// owner-less satchel) has placed, applying radius damage immediately —
    /// `ohl_combat::DeployableSet::detonate_all_satchels` is itself
    /// synchronous, unlike the tripmine beam `tick` resolves per step.
    #[allow(dead_code)]
    pub(crate) fn detonate_all_satchels(
        &mut self,
        level: &mut Level,
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
    /// underlying placement trace fails. As [`Self::place_satchel`], also
    /// spawns the model-backed stand-in entity when
    /// [`DeployableKind::Tripmine`] has a configured model.
    #[allow(dead_code)]
    pub(crate) fn place_tripmine(
        &mut self,
        level: &mut Level,
        owner: Option<Entity>,
        from: Vec3,
        direction: Vec3,
    ) -> Option<DeployableId> {
        let collision = level.collision.as_ref()?;
        let mut events = Vec::new();
        let id = self.deployables.place_tripmine(
            owner.map(entity_id),
            from,
            direction,
            collision,
            &self.deployable_tuning,
            &mut events,
        )?;
        // The placement trace's own hit point (`events`' `Placed` position),
        // not `from`, is where the mine actually stuck.
        let position = events
            .iter()
            .find_map(|event| match event {
                DeployableEvent::Placed { position, .. } => Some(*position),
                _ => None,
            })
            .unwrap_or(from);
        self.spawn_deployable_model(level, DeployableKind::Tripmine, id, position, owner);
        Some(id)
    }

    /// Spawns the `hecs` entity standing in for one placed deployable's
    /// model, when `kind` has a configured slot; a no-op otherwise (the
    /// deployable stays simulated but undrawn and undamageable, as before
    /// this module tracked a model for it at all).
    fn spawn_deployable_model(
        &mut self,
        level: &mut Level,
        kind: DeployableKind,
        id: DeployableId,
        position: Vec3,
        owner: Option<Entity>,
    ) {
        let Some(model) = self.deployable_model_for(kind) else {
            return;
        };
        let entity = level.registry.world.spawn((
            Transform {
                origin: position,
                angles: Vec3::ZERO,
            },
            StudioAnim::new(model, 0),
            Health::new(DEPLOYABLE_HEALTH),
            DeployableRef { id, kind },
        ));
        if let Some(owner) = owner {
            let _ = level.registry.world.insert_one(entity, Owner(owner));
        }
        self.deployable_models.insert(id, entity);
    }

    /// Advances every projectile and deployable by `dt` seconds, queuing
    /// the damage and transient sprites their events imply.
    ///
    /// `hitboxes` is phase 5's rebuild (`crate::combat::rebuild_hitbox_index`,
    /// owned by `crate::systems::Systems`), which keeps this system's own
    /// model-backed entities in it (see the module doc) — this system
    /// traces against it rather than rebuilding its own index, and each
    /// projectile ignores only its own entity and owner per trace.
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
        level: &mut Level,
        event: DeployableEvent,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) {
        if let DeployableEvent::Detonated {
            id,
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
            resolve_blast(
                level,
                position,
                radius,
                DEPLOYABLE_BLAST_DAMAGE,
                DamageType::BLAST,
                owner,
                &self.explosion_rule,
                damage_queue,
            );
            // The charge is gone: whatever model-backed stand-in it had
            // (see `spawn_deployable_model`) goes with it, so a shot or
            // blast that killed it does not leave a dead, undamageable
            // husk still sitting in the hitbox index next step.
            if let Some(entity) = self.deployable_models.remove(&id) {
                let _ = level.registry.world.despawn(entity);
            }
        }
    }

    /// Phase 9's follow-up: a placed satchel or tripmine whose stand-in
    /// entity's [`Health`] was just brought to zero or below (the player's
    /// hitscan, or another explosive's blast, resolved by
    /// `crate::combat::resolve_damage` immediately before this runs) is
    /// killed, per the published health cited on [`DEPLOYABLE_HEALTH`]: it
    /// detonates, via [`ohl_combat::DeployableSet::detonate`] by handle,
    /// exactly like any other detonation.
    ///
    /// Must run after damage resolution, in the same step, so a placed
    /// charge that dies this step detonates this step rather than one step
    /// late. Returns how many detonated, so a caller
    /// (`crate::systems::Systems::reap_deployables`) knows whether this
    /// call just queued fresh blast damage of its own (against the player,
    /// or against another deployable's stand-in) that itself needs
    /// resolving before phase 10 runs — a detonation's blast is not
    /// self-resolving, and phase 10's monster-only drain silently discards
    /// anything else left in the queue.
    pub(crate) fn resolve_deployable_damage(
        &mut self,
        level: &mut Level,
        damage_queue: &mut Vec<QueuedDamage>,
        sprites: &mut TransientSprites,
    ) -> usize {
        let mut dead = Vec::new();
        for (&id, &entity) in &self.deployable_models {
            if let Ok(health) = level.registry.world.get::<&Health>(entity)
                && health.is_dead()
            {
                dead.push(id);
            }
        }
        let mut detonated = 0;
        for id in dead {
            let mut events = Vec::new();
            if self
                .deployables
                .detonate(id, &self.deployable_tuning, &mut events)
            {
                detonated += 1;
                for event in events {
                    self.apply_deployable_event(level, event, damage_queue, sprites);
                }
            }
        }
        detonated
    }

    /// Captures `SECTION_PROJECTILES` (26): every live projectile and
    /// placed deployable, plus the id/rng restart bookkeeping
    /// [`Self::restore_snapshot`] needs to continue exactly where this
    /// snapshot left off.
    #[must_use]
    pub(crate) fn snapshot(&self, level: &Level) -> crate::save_state::ProjectilesSnapshot {
        let (projectile_next_id, projectile_rng_state) = self.projectiles.next_id_and_rng_state();
        let projectiles = self
            .projectiles
            .projectiles()
            .iter()
            .map(|projectile| crate::save_state::ProjectileSnapshot {
                id: projectile.id.0,
                kind_tag: crate::save_state::projectile_kind_tag(projectile.kind),
                owner: projectile
                    .owner
                    .and_then(|owner| crate::save_state::spawn_index_of_combat_id(level, owner)),
                position: crate::save_state::vec3_array(projectile.position),
                velocity: crate::save_state::vec3_array(projectile.velocity),
                age: projectile.age,
                fuse: projectile.fuse,
                guide_point: projectile.guide_point.map(crate::save_state::vec3_array),
                target: projectile
                    .target
                    .and_then(|target| crate::save_state::spawn_index_of_combat_id(level, target)),
                attack_cooldown: projectile.attack_cooldown,
                hop_cooldown: projectile.hop_cooldown,
                resting: projectile.resting,
            })
            .collect();
        let satchels = self
            .deployables
            .satchels()
            .iter()
            .map(|satchel| crate::save_state::SatchelSnapshot {
                id: satchel.id.0,
                owner: satchel
                    .owner
                    .and_then(|owner| crate::save_state::spawn_index_of_combat_id(level, owner)),
                position: crate::save_state::vec3_array(satchel.position),
                age: satchel.age,
            })
            .collect();
        let tripmines = self
            .deployables
            .tripmines()
            .iter()
            .map(|tripmine| crate::save_state::TripmineSnapshot {
                id: tripmine.id.0,
                owner: tripmine
                    .owner
                    .and_then(|owner| crate::save_state::spawn_index_of_combat_id(level, owner)),
                position: crate::save_state::vec3_array(tripmine.position),
                normal: crate::save_state::vec3_array(tripmine.normal),
                age: tripmine.age,
                armed: tripmine.armed,
            })
            .collect();
        crate::save_state::ProjectilesSnapshot {
            projectiles,
            projectile_next_id,
            projectile_rng_state,
            satchels,
            tripmines,
            deployable_next_id: self.deployables.next_id(),
        }
    }

    /// Restores everything [`Self::snapshot`] captured, replacing whatever
    /// this system currently holds. Model-backed rendering entities are not
    /// recreated (see `crate::save_state`'s module doc): the restored
    /// projectiles simulate identically but draw as nothing until they
    /// resolve.
    pub(crate) fn restore_snapshot(
        &mut self,
        level: &Level,
        snapshot: &crate::save_state::ProjectilesSnapshot,
    ) {
        self.models.clear();
        // Bounded before any allocation grows from it, not just by
        // `ProjectileSet`/`DeployableSet::restore_from_parts`'s own
        // truncation afterward: a corrupt or adversarial save naming an
        // enormous section must not make this crate build an enormous
        // `Vec` just to throw most of it away.
        let projectiles = snapshot
            .projectiles
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_PROJECTILES)
            .filter_map(|entry| {
                let kind = crate::save_state::projectile_kind_from_tag(entry.kind_tag)?;
                Some(ohl_combat::Projectile {
                    id: ProjectileId(entry.id),
                    kind,
                    owner: entry.owner.and_then(|index| {
                        crate::save_state::combat_id_at_spawn_index(level, index)
                    }),
                    position: crate::save_state::array_vec3(entry.position),
                    velocity: crate::save_state::array_vec3(entry.velocity),
                    age: crate::save_state::sanitize_f32(entry.age, 0.0).max(0.0),
                    fuse: entry
                        .fuse
                        .map(|fuse| crate::save_state::sanitize_f32(fuse, 0.0).max(0.0)),
                    guide_point: entry.guide_point.map(crate::save_state::array_vec3),
                    target: entry.target.and_then(|index| {
                        crate::save_state::combat_id_at_spawn_index(level, index)
                    }),
                    attack_cooldown: crate::save_state::sanitize_f32(entry.attack_cooldown, 0.0)
                        .max(0.0),
                    hop_cooldown: crate::save_state::sanitize_f32(entry.hop_cooldown, 0.0).max(0.0),
                    resting: entry.resting,
                    // Not persisted (see this method's own doc and
                    // `crate::projectiles`' module doc's "Not yet
                    // persisted" note): a restored projectile has no
                    // model-backed stand-in entity yet, so it has nothing
                    // to name here either. `sync_model_entities` never
                    // populates `self.models` for a restored id, so this
                    // stays `None` until a future save-format revision
                    // re-spawns stand-ins on restore (see that note for
                    // why a *later* respawn must also set this, or a
                    // restored rocket could detonate on its own model).
                    self_id: None,
                })
            })
            .collect();
        self.projectiles = ProjectileSet::restore_from_parts(
            projectiles,
            ohl_combat::ProjectileLimits::default(),
            snapshot.projectile_next_id,
            snapshot.projectile_rng_state,
        );
        let satchels = snapshot
            .satchels
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_DEPLOYABLES)
            .map(|entry| ohl_combat::Satchel {
                id: DeployableId(entry.id),
                owner: entry
                    .owner
                    .and_then(|index| crate::save_state::combat_id_at_spawn_index(level, index)),
                position: crate::save_state::array_vec3(entry.position),
                age: crate::save_state::sanitize_f32(entry.age, 0.0).max(0.0),
            })
            .collect();
        let tripmines = snapshot
            .tripmines
            .iter()
            .take(crate::save_state::MAX_SNAPSHOT_DEPLOYABLES)
            .map(|entry| ohl_combat::Tripmine {
                id: DeployableId(entry.id),
                owner: entry
                    .owner
                    .and_then(|index| crate::save_state::combat_id_at_spawn_index(level, index)),
                position: crate::save_state::array_vec3(entry.position),
                normal: crate::save_state::array_vec3(entry.normal),
                age: crate::save_state::sanitize_f32(entry.age, 0.0).max(0.0),
                armed: entry.armed,
            })
            .collect();
        self.deployables =
            DeployableSet::restore_from_parts(satchels, tripmines, snapshot.deployable_next_id);
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

/// The index of `path` (case-insensitively) in `level.studio_model_paths`,
/// for [`ProjectileSystem::configure_models`]. `None` when the map never
/// loaded that exact model — the ordinary case for every path in
/// [`default_projectile_model_path`]/[`default_deployable_model_path`],
/// since none of them is a path this project's own map loader fetches on
/// its own (see `Level::studio_model_paths`'s doc).
fn find_model_path(level: &Level, path: &str) -> Option<usize> {
    let needle = path.to_ascii_lowercase();
    level
        .studio_model_paths
        .iter()
        .position(|candidate| *candidate == needle)
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
    use crate::systems::QueuedDamage;
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
            &mut level,
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
        system.place_satchel(&mut level, Some(owner), glam::Vec3::new(0.0, 0.0, 40.0));

        let mut damage_queue = Vec::new();
        let mut sprites = TransientSprites::new();
        system.detonate_all_satchels(&mut level, &mut damage_queue, &mut sprites);

        let owner_id = entity_id(owner);
        assert!(
            damage_queue
                .iter()
                .any(|queued| queued.info.attacker == Some(owner_id) && queued.info.amount > 0.0),
            "the owner must be among the blast's own hits"
        );
    }

    /// A placed tripmine given a model stays *in* the shared hitbox index
    /// (never excluded, per the module doc) and is therefore reachable by
    /// the player's own hitscan, which brings its published one point of
    /// health (see [`DEPLOYABLE_HEALTH`]'s doc) to zero and detonates it —
    /// `Systems::resolve_deployable_damage`'s job, exercised here directly
    /// against a hand-built `HitboxIndex` the way
    /// `crate::combat`'s own weapon-wiring tests do, rather than through a
    /// full firing state machine.
    #[test]
    fn the_players_hitscan_can_hit_and_detonate_a_placed_tripmine() {
        let mut level = synthetic_level();
        let mut system = ProjectileSystem::new(0);
        system.set_model_for_deployable(ohl_combat::DeployableKind::Tripmine, Some(0));

        let placed = system
            .place_tripmine(
                &mut level,
                None,
                glam::Vec3::new(0.0, 0.0, 40.0),
                glam::Vec3::new(0.0, 0.0, -1.0),
            )
            .expect("the placement trace finds the floor");
        let mine_entity = system
            .deployable_entity(placed)
            .expect("a configured model spawns the stand-in entity");

        // The stand-in entity's own hitbox, as phase 5's real rebuild would
        // have produced from a posed studio model — built by hand here so
        // the test does not depend on a real `.mdl` asset's hitboxes.
        let mut hitboxes = HitboxIndex::new(HitboxLimits::default());
        let mut entry = ohl_combat::EntityHitboxes::new(
            entity_id(mine_entity),
            glam::Vec3::new(0.0, 0.0, 40.0),
        );
        entry.push_box(
            0,
            glam::Vec3::splat(-4.0),
            glam::Vec3::splat(4.0),
            ohl_combat::HitGroup::Generic,
        );
        hitboxes.push(entry);

        // The trace itself proves the mine is not excluded from the index.
        let collision = level.collision.as_ref().expect("the fixture has hulls");
        let trace = ohl_combat::trace_attack(
            collision,
            &hitboxes,
            glam::Vec3::new(-64.0, 0.0, 40.0),
            glam::Vec3::new(64.0, 0.0, 40.0),
            ohl_combat::TraceMask::SHOT,
        );
        assert_eq!(
            trace.entity,
            Some(entity_id(mine_entity)),
            "the player's hitscan must be able to hit the placed tripmine"
        );

        // Queue and resolve the hit exactly as `Systems::weapons`/
        // `resolve_damage` would, then let the follow-up phase detonate it.
        let mut damage_queue = vec![QueuedDamage {
            target: mine_entity,
            info: ohl_combat::DamageInfo::new(50.0, ohl_combat::DamageType::BULLET),
        }];
        let mut player = ohl_player::Player::new(ohl_player::PlayerConfig::default());
        let mut hud = ohl_ui::hud::HudState::default();
        let mut presentation = crate::presentation::Presentation::new();
        let mut player_events = Vec::new();
        let mut player_damage_events = 0u64;
        let player_id = level.player;
        crate::combat::resolve_damage(
            &mut damage_queue,
            &mut level,
            &mut player,
            player_id,
            &mut hud,
            &mut presentation,
            &mut player_events,
            &mut player_damage_events,
        );

        let mut sprites = TransientSprites::new();
        system.resolve_deployable_damage(&mut level, &mut damage_queue, &mut sprites);

        assert!(
            system.deployables.tripmines().is_empty(),
            "a killed tripmine must detonate and be removed"
        );
        assert!(
            level
                .registry
                .world
                .get::<&ohl_combat::Health>(mine_entity)
                .is_err(),
            "the stand-in entity must be despawned once it detonates"
        );
    }

    /// One satchel's blast damages another's stand-in entity (both are
    /// ordinary [`ohl_combat::Health`]-carrying [`BlastTarget`]s, per
    /// `blast_targets`'s doc), which brings the second one to zero health
    /// and detonates it in turn — a chain reaction, not a special case.
    ///
    /// This hand-calls `resolve_damage`/`resolve_deployable_damage` once
    /// each, at this module's own level, as a fast, isolated check of the
    /// detonation mechanism; it does not drive `Systems::step`, so it
    /// cannot by itself guard the *phase ordering* this mechanism depends
    /// on (`crate::systems::Systems::reap_deployables`'s doc). That guard
    /// is `crate::systems::tests::a_satchel_chain_reaction_resolves_through_the_real_step_order`,
    /// which does drive a real tick.
    #[test]
    fn a_satchel_is_detonated_by_another_satchels_explosion() {
        let mut level = synthetic_level();
        let mut system = ProjectileSystem::new(0);
        system.set_model_for_deployable(ohl_combat::DeployableKind::Satchel, Some(0));

        system
            .place_satchel(&mut level, None, glam::Vec3::new(0.0, 0.0, 40.0))
            .expect("the set has room");
        let second = system
            .place_satchel(&mut level, None, glam::Vec3::new(20.0, 0.0, 40.0))
            .expect("the set has room");
        let second_entity = system
            .deployable_entity(second)
            .expect("a configured model spawns the stand-in entity");

        let mut damage_queue = Vec::new();
        let mut sprites = TransientSprites::new();
        // Sets off every placed satchel, `first` included; `second` sits
        // well inside a satchel's blast radius (`DeployableTuning`'s
        // default 200 units) and has line of sight, so it must take damage
        // from `first`'s detonation queued here.
        system.detonate_all_satchels(&mut level, &mut damage_queue, &mut sprites);
        assert!(
            damage_queue
                .iter()
                .any(|queued| queued.target == second_entity && queued.info.amount > 0.0),
            "the second satchel must be among the first satchel's blast hits"
        );

        let mut player = ohl_player::Player::new(ohl_player::PlayerConfig::default());
        let mut hud = ohl_ui::hud::HudState::default();
        let mut presentation = crate::presentation::Presentation::new();
        let mut player_events = Vec::new();
        let mut player_damage_events = 0u64;
        let player_id = level.player;
        crate::combat::resolve_damage(
            &mut damage_queue,
            &mut level,
            &mut player,
            player_id,
            &mut hud,
            &mut presentation,
            &mut player_events,
            &mut player_damage_events,
        );
        system.resolve_deployable_damage(&mut level, &mut damage_queue, &mut sprites);

        assert!(
            system.deployables.satchels().is_empty(),
            "both satchels must be gone: the first by its own detonation, \
             the second killed by the first's blast"
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
