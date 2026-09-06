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
