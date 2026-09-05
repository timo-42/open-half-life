//! HEV suit voice events.
//!
//! The suit announces a fixed set of *occasions*. Which occasions exist is
//! public (the HEV suit quote listings on Combine OverWiki group them by
//! systems startup, injury category, medical treatment, equipment pickup,
//! hazard detection and suit status; see `docs/FORMAT_SOURCES.md`, "Player
//! systems"). The actual sentence text, the `sentences.txt` group names and
//! the WAV files are *game data*: this project never reproduces them, so an
//! occasion carries a project-owned symbolic name and the host resolves it
//! against the user's own installation.
//!
//! Nothing here plays audio. A [`SuitEvent`] is a request the host maps to
//! an `ohl_audio::PlayRequest` on the voice channel.

use serde::{Deserialize, Serialize};

/// One thing the HEV suit has to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SuitOccasion {
    /// The suit was just put on.
    SuitEquipped,
    /// Health has fallen below the critical fraction.
    HealthCritical,
    /// Health is almost gone.
    NearDeath,
    /// The armor charge has just run out.
    ArmorGone,
    /// The armor was recharged from zero.
    ArmorRestored,
    /// Corrosive or toxic exposure (slime, acid, chemical `trigger_hurt`).
    ChemicalDetected,
    /// Radiation exposure.
    RadiationDetected,
    /// Burn/heat damage (lava, fire).
    HeatDamage,
    /// Electrical damage.
    ShockDamage,
    /// Sustained blood loss.
    BloodLoss,
    /// A heavy impact.
    MajorFracture,
    /// A light impact, e.g. a survivable fall.
    MinorFracture,
    /// Internal injury.
    InternalBleeding,
    /// The player is drowning or otherwise needs treatment.
    SeekMedicalAttention,
    /// Morphine was administered.
    MorphineAdministered,
    /// Ammunition was picked up.
    AmmoPickup,
    /// A weapon was picked up.
    WeaponPickup,
    /// A medkit or health charger was used.
    MedkitPickup,
    /// Suit power was restored by a battery or charger.
    PowerRestored,
    /// The active weapon is out of ammunition.
    AmmunitionDepleted,
    /// The long jump module was installed.
    LongJumpActivated,
}

impl SuitOccasion {
    /// This occasion's stable, project-owned symbolic name.
    ///
    /// It is *not* a Half-Life `sentences.txt` group name: mapping a name
    /// here to a sentence group in the user's own installation is the
    /// host's job, so no game data is reproduced in this repository.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SuitEquipped => "suit_equipped",
            Self::HealthCritical => "health_critical",
            Self::NearDeath => "near_death",
            Self::ArmorGone => "armor_gone",
            Self::ArmorRestored => "armor_restored",
            Self::ChemicalDetected => "chemical_detected",
            Self::RadiationDetected => "radiation_detected",
            Self::HeatDamage => "heat_damage",
            Self::ShockDamage => "shock_damage",
            Self::BloodLoss => "blood_loss",
            Self::MajorFracture => "major_fracture",
            Self::MinorFracture => "minor_fracture",
            Self::InternalBleeding => "internal_bleeding",
            Self::SeekMedicalAttention => "seek_medical_attention",
            Self::MorphineAdministered => "morphine_administered",
            Self::AmmoPickup => "ammo_pickup",
            Self::WeaponPickup => "weapon_pickup",
            Self::MedkitPickup => "medkit_pickup",
            Self::PowerRestored => "power_restored",
            Self::AmmunitionDepleted => "ammunition_depleted",
            Self::LongJumpActivated => "long_jump_activated",
        }
    }

    /// How urgent this occasion is: `0` pre-empts everything, higher
    /// numbers wait. Life-threatening warnings come first, hazards next,
    /// then status, then pickups.
    ///
    /// `TODO(black-box)`: the real priority order is not published; this
    /// is a neutral ordering chosen so a death warning is never queued
    /// behind an ammunition pickup.
    #[must_use]
    pub const fn priority(self) -> u8 {
        match self {
            Self::NearDeath | Self::HealthCritical => 0,
            Self::ArmorGone
            | Self::ChemicalDetected
            | Self::RadiationDetected
            | Self::HeatDamage
            | Self::ShockDamage
            | Self::SeekMedicalAttention => 1,
            Self::BloodLoss
            | Self::MajorFracture
            | Self::MinorFracture
            | Self::InternalBleeding
            | Self::MorphineAdministered => 2,
            Self::SuitEquipped | Self::LongJumpActivated | Self::ArmorRestored => 3,
            Self::AmmoPickup
            | Self::WeaponPickup
            | Self::MedkitPickup
            | Self::PowerRestored
            | Self::AmmunitionDepleted => 4,
        }
    }
}

/// A queued suit announcement. Not serialised: the queue's *cooldowns*
/// are saved ([`SuitVoice`]), a half-spoken line is not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuitEvent {
    /// What happened.
    pub occasion: SuitOccasion,
    /// [`SuitOccasion::name`], carried along so a host never has to match
    /// on the enum.
    pub name: &'static str,
    /// [`SuitOccasion::priority`].
    pub priority: u8,
    /// Seconds the host should wait before speaking, so several events
    /// raised in the same tick do not overlap.
    pub delay: f32,
}

/// Seconds between two consecutive announcements, used to space a burst of
/// events out over time.
///
/// `TODO(black-box)`: neutral placeholder.
pub const SUIT_SPACING_SECONDS: f32 = 1.5;

/// Seconds before the *same* occasion may be announced again, so a
/// continuing condition (standing in slime, health staying critical) is
/// reported once rather than every tick.
///
/// `TODO(black-box)`: neutral placeholder.
pub const SUIT_COOLDOWN_SECONDS: f32 = 10.0;

/// The number of distinct occasions whose cooldown is tracked. It is the
/// number of [`SuitOccasion`] variants, so the table never allocates.
const OCCASION_COUNT: usize = 21;

/// Per-occasion cooldown bookkeeping.
///
/// It is a fixed-size table indexed by occasion, so it never allocates,
/// never depends on hash order, and serialises to the same bytes for the
/// same history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuitVoice {
    /// Seconds of cooldown left per occasion, in [`SuitOccasion`] order.
    remaining: [f32; OCCASION_COUNT],
    /// Seconds until the next announcement may be spoken at all.
    spacing: f32,
    /// How many announcements have been raised this tick, used to stagger
    /// their [`SuitEvent::delay`].
    raised_this_tick: u8,
}

impl Default for SuitVoice {
    fn default() -> Self {
        Self {
            remaining: [0.0; OCCASION_COUNT],
            spacing: 0.0,
            raised_this_tick: 0,
        }
    }
}

fn occasion_index(occasion: SuitOccasion) -> usize {
    // The cast is over a fieldless enum with 21 variants, so it is always
    // in range; the modulo keeps that true even if a variant is added
    // without updating `OCCASION_COUNT`.
    (occasion as usize) % OCCASION_COUNT
}

impl SuitVoice {
    /// An empty queue with nothing on cooldown.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances every cooldown by `dt` seconds. Call once per tick, before
    /// raising this tick's events.
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        for remaining in &mut self.remaining {
            *remaining = (*remaining - dt).max(0.0);
        }
        self.spacing = (self.spacing - dt).max(0.0);
        self.raised_this_tick = 0;
    }

    /// Whether `occasion` may be announced right now.
    #[must_use]
    pub fn ready(&self, occasion: SuitOccasion) -> bool {
        self.remaining[occasion_index(occasion)] <= 0.0
    }

    /// Raises `occasion` if it is off cooldown, putting it back on
    /// cooldown and staggering it behind anything else raised this tick.
    pub fn raise(&mut self, occasion: SuitOccasion) -> Option<SuitEvent> {
        if !self.ready(occasion) {
            return None;
        }
        self.remaining[occasion_index(occasion)] = SUIT_COOLDOWN_SECONDS;
        let delay = self.spacing + f32::from(self.raised_this_tick) * SUIT_SPACING_SECONDS;
        self.raised_this_tick = self.raised_this_tick.saturating_add(1);
        self.spacing = delay + SUIT_SPACING_SECONDS;
        Some(SuitEvent {
            occasion,
            name: occasion.name(),
            priority: occasion.priority(),
            delay,
        })
    }

    /// Clears every cooldown, e.g. when a new map starts.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
