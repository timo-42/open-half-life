//! Engine-owned entity components.
//!
//! `ohl_game::Registry::world` is the project's only entity world: map logic,
//! combat and AI all read and write the same [`ohl_game::hecs::World`]. The
//! components here are the ones no other crate owns — everything else reuses
//! the crate that already defines it ([`ohl_combat::Health`],
//! [`ohl_combat::Armor`], `ohl_ai::Actor`), so nothing is duplicated.
//!
//! Nothing in this module logs. Every field is either project-authored or
//! read out of a map, and map-derived data is returned to the caller rather
//! than written to a diagnostic (see `docs/CLEAN_ROOM.md`).

use ohl_game::hecs::Entity;

/// Which loaded studio model an entity draws, and where in its animation.
///
/// [`crate::Game::render`] sources one draw call per entity carrying this,
/// so a monster walking is a [`ohl_game::registry::Transform`] something
/// else wrote plus a [`StudioAnim::sequence`] the engine picked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioAnim {
    /// Index into the level's loaded studio models.
    pub model: usize,
    /// Which of the model's sequences is playing.
    pub sequence: usize,
    /// How far into the sequence playback stands, in seconds. This is the
    /// `time` argument [`ohl_world::StudioPose::sample`] takes, so a
    /// sequence's own frame rate still decides which frames it lands on.
    pub cycle: f32,
    /// Playback rate multiplier; `1.0` plays the sequence at its authored
    /// speed.
    pub frame_rate: f32,
    /// The `body` keyvalue: which submodel of each body part to draw.
    pub body: u32,
    /// The `skin` keyvalue: which skin family to texture with.
    pub skin: usize,
}

impl StudioAnim {
    /// A cursor at the start of `sequence` of `model`, playing at the
    /// sequence's authored rate.
    #[must_use]
    pub fn new(model: usize, sequence: usize) -> Self {
        Self {
            model,
            sequence,
            cycle: 0.0,
            frame_rate: 1.0,
            body: 0,
            skin: 0,
        }
    }

    /// Advances the cursor by `dt` seconds of simulated time.
    ///
    /// A non-finite `dt` or rate leaves the cursor where it was, so corrupt
    /// state cannot poison the pose sampler.
    pub fn advance(&mut self, dt: f32) {
        let step = dt * self.frame_rate;
        if !step.is_finite() {
            return;
        }
        let next = self.cycle + step;
        if next.is_finite() {
            self.cycle = next;
        }
    }

    /// Restarts `sequence` from its first frame. Selecting the sequence that
    /// is already playing leaves the cursor alone, so a repeated activity
    /// does not stutter.
    pub fn play(&mut self, sequence: usize) {
        if self.sequence != sequence {
            self.sequence = sequence;
            self.cycle = 0.0;
        }
    }

    /// Samples the same complete pose for drawing, attachments and hitboxes.
    pub fn sample(
        &self,
        model: &ohl_world::StudioModel,
        gait: Option<&StudioGait>,
    ) -> ohl_world::Result<ohl_world::StudioPose> {
        if let Some(gait) = gait
            && let Some(sequence) = gait.sequence
        {
            return ohl_world::StudioPose::sample_layered(
                model,
                self.sequence,
                self.cycle,
                sequence,
                gait.cycle,
                &gait.bones,
            );
        }
        ohl_world::StudioPose::sample(model, self.sequence, self.cycle)
    }
}

/// Independent lower-body playback for a skirmish bot. Weapon actions use
/// [`StudioAnim`] while this cursor keeps the legs moving.
#[derive(Debug, Clone, PartialEq)]
pub struct StudioGait {
    /// Locomotion sequence, or `None` while dead or without a matching pose.
    pub sequence: Option<usize>,
    /// Seconds into the locomotion sequence.
    pub cycle: f32,
    /// Playback multiplier derived from horizontal movement speed.
    pub frame_rate: f32,
    /// Model-authored leg hitboxes identify the lower skeleton, without
    /// relying on any particular bone label or table order.
    bones: Vec<bool>,
}

impl StudioGait {
    pub(crate) fn new(model: &ohl_world::StudioModel) -> Option<Self> {
        let mut legs = vec![false; model.bones.len()];
        for hitbox in &model.hitboxes {
            if matches!(
                ohl_combat::HitGroup::from_index(hitbox.group),
                ohl_combat::HitGroup::LeftLeg | ohl_combat::HitGroup::RightLeg
            ) && let Some(leg) = legs.get_mut(hitbox.bone)
            {
                *leg = true;
            }
        }
        // Feet and other descendants follow their leg even without a hitbox.
        for (index, bone) in model.bones.iter().enumerate() {
            if bone.parent.is_some_and(|parent| legs[parent]) {
                legs[index] = true;
            }
        }
        let mut bones = legs.clone();
        // Include the shared hips and root, so the upper body's local pose
        // inherits the gait's sway rather than separating at the waist.
        for (index, leg) in legs.into_iter().enumerate() {
            if !leg {
                continue;
            }
            let mut parent = model.bones[index].parent;
            while let Some(index) = parent {
                bones[index] = true;
                parent = model.bones[index].parent;
            }
        }
        // Models without a separable lower skeleton keep full-body playback.
        (bones.iter().any(|bone| *bone) && bones.iter().any(|bone| !bone)).then_some(Self {
            sequence: None,
            cycle: 0.0,
            frame_rate: 1.0,
            bones,
        })
    }

    pub(crate) fn play(&mut self, sequence: Option<usize>, frame_rate: f32) {
        if self.sequence != sequence {
            self.sequence = sequence;
            self.cycle = 0.0;
        }
        self.frame_rate = frame_rate;
    }

    pub(crate) fn advance(&mut self, dt: f32) {
        let next = self.cycle + dt * self.frame_rate;
        if self.sequence.is_some() && next.is_finite() {
            self.cycle = next;
        }
    }
}

/// A separate studio weapon drawn against this entity's animated skeleton.
/// It shares the owner's transform and lighting, and has no hitboxes of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeldWeapon {
    /// Loaded studio model slot, or `None` while unarmed or dead.
    pub model: Option<usize>,
}

/// Marks the single client entity.
///
/// Exactly one entity per level carries this: the one
/// [`crate::Game::player_entity`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerTag;

/// Attributes a projectile, deployable or spawned monster back to whoever
/// made it, so an attack can ignore its own owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner(pub Entity);

/// Marks a `hecs` entity as `crate::projectiles::ProjectileSystem`'s own
/// model-backed rendering of one placed satchel charge or tripmine, so a
/// generic hit against it (the player's hitscan, another explosive's blast)
/// can be routed back to the [`ohl_combat::DeployableId`] that actually
/// tracks its state. Carries an [`ohl_combat::Health`] too (see
/// `ProjectileSystem::place_satchel`/`place_tripmine`), so it drains
/// through the same `resolve_damage` path as anything else with a health
/// bar; reaching zero is what makes it damageable at all, per the
/// documented behaviour cited in `docs/FORMAT_SOURCES.md` — a tripmine is
/// shootable and a satchel can be killed by another explosion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeployableRef {
    /// The placed charge this entity stands in for.
    pub id: ohl_combat::DeployableId,
    /// Which kind, so the caller need not guess which of
    /// [`ohl_combat::DeployableSet`]'s two lists to search.
    pub kind: ohl_combat::DeployableKind,
}

/// A `weapon_*` / `ammo_*` / `item_*` entity that has not been taken yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pickup {
    /// What taking it gives.
    pub kind: ohl_combat::PickupKind,
    /// Whether it has already been taken; a taken pickup stays in the world
    /// so a respawn rule can bring it back without re-spawning an entity.
    pub taken: bool,
}

impl Pickup {
    /// An untaken pickup of `kind`.
    #[must_use]
    pub fn new(kind: ohl_combat::PickupKind) -> Self {
        Self { kind, taken: false }
    }
}

/// What one `weaponbox` entity is stocked with: a bounded, already-parsed
/// list of (ammo type, units) pairs read off the entity's own keyvalues.
///
/// A `weaponbox` is the one pickup whose contents are per-entity map data
/// rather than a fixed published grant, so [`Pickup`]'s `kind` alone
/// ([`ohl_combat::PickupKind::WeaponBox`]) cannot say what taking it gives.
/// The key names and their case sensitivity live with the rest of the
/// pickup vocabulary in `ohl_combat::weaponbox_ammo_key`; this component
/// only carries the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponBox {
    /// Ammo type and how many units of it, at most one entry per published
    /// key, in the keyvalue table's sorted key order.
    pub contents: Vec<(ohl_combat::AmmoType, u32)>,
}

/// A `func_healthcharger` / `func_recharge` and its remaining charge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Charger(pub ohl_combat::ChargerState);

/// A dead entity kept in the world so it can be drawn, and how long is left
/// before its `Fade Corpse` flag removes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corpse {
    /// Seconds until the corpse is despawned; `f32::INFINITY` for a corpse
    /// that never fades.
    pub seconds_left: f32,
}

/// A `monstermaker` and its spawn bookkeeping.
///
/// The keyvalue semantics (`monstertype`, `monstercount`, `delay`,
/// `m_imaxlivechildren`, the `Start On` and `Cyclic` spawnflags) all live in
/// [`ohl_ai::Spawner`]; the engine only reads the definition, ticks the
/// spawner once per step in [`crate::ai::AiState::lifecycle`] and creates
/// the child entity the spawner asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterMaker(pub ohl_ai::Spawner);
