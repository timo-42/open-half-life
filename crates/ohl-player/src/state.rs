//! The player's own state and its tunables.

use serde::{Deserialize, Serialize};

use crate::damage::DamageFlags;

/// The HEV flashlight: on/off plus a `0.0..=1.0` charge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Flashlight {
    /// Whether the beam is currently on.
    pub on: bool,
    /// Remaining charge, `1.0` full and `0.0` empty.
    pub charge: f32,
}

impl Default for Flashlight {
    fn default() -> Self {
        Self {
            on: false,
            charge: 1.0,
        }
    }
}

/// Everything the player systems own about the player.
///
/// Position and velocity are *not* here: those belong to
/// `ohl_physics::PlayerState`, and this crate reads them through
/// [`crate::PhysicsOutput`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    /// Current health. Never negative and never above
    /// [`PlayerConfig::max_health`].
    pub health: f32,
    /// Current HEV armor charge, `0.0..=`[`PlayerConfig::max_armor`].
    pub armor: f32,
    /// Whether the HEV suit has been picked up. Without it there is no
    /// armor, no suit voice and no flashlight.
    pub suit_equipped: bool,
    /// The flashlight.
    pub flashlight: Flashlight,
    /// Whether the long jump module has been installed.
    pub longjump_owned: bool,
    /// The documented `0..=3` waterlevel, mirrored from the physics step so
    /// the HUD and the save file do not have to reach into it.
    pub waterlevel: u8,
    /// Seconds of air left before drowning starts. Counts down while the
    /// player's head is under a liquid and refills when it is not.
    pub air_time: f32,
    /// The damage kinds taken since the HUD last consumed them.
    pub damage_flags: DamageFlags,
    /// Whether the player is dead (health reached zero).
    pub dead: bool,
}

impl PlayerState {
    /// A fresh player for `config`: full health, no suit, no armor.
    #[must_use]
    pub fn new(config: &PlayerConfig) -> Self {
        Self {
            health: config.max_health,
            armor: 0.0,
            suit_equipped: false,
            flashlight: Flashlight::default(),
            longjump_owned: false,
            waterlevel: 0,
            air_time: config.air_capacity_seconds,
            damage_flags: DamageFlags::default(),
            dead: false,
        }
    }

    /// Health rounded for display, clamped to `0`.
    #[must_use]
    pub fn display_health(&self) -> i32 {
        clamp_display(self.health)
    }

    /// Armor rounded for display, clamped to `0`.
    #[must_use]
    pub fn display_armor(&self) -> i32 {
        clamp_display(self.armor)
    }
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::new(&PlayerConfig::default())
    }
}

// The one place this crate converts a float to an integer. Every path into
// it clamps the value into `0..=1_000_000` first, so the conversion is
// exact and cannot truncate or lose a sign.
#[allow(clippy::cast_possible_truncation)]
fn clamp_display(value: f32) -> i32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    // `ceil` so a player on 0.4 health still shows 1: they are alive.
    let rounded = value.ceil();
    if rounded >= 1_000_000.0 {
        1_000_000
    } else {
        // The value is finite, positive and below a million here.
        rounded as i32
    }
}

/// The tunables of the player systems.
///
/// Values with a published source are marked as such; everything else is a
/// neutral placeholder marked `TODO(black-box)` that has to be measured
/// against the retail game before this project may claim parity. See
/// `docs/FORMAT_SOURCES.md`, "Player systems".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerConfig {
    /// Maximum health. Published: 100.
    pub max_health: f32,
    /// Maximum HEV armor. Published: 100.
    pub max_armor: f32,
    /// Seconds of air a full breath lasts.
    ///
    /// `TODO(black-box)`.
    pub air_capacity_seconds: f32,
    /// Seconds between two drowning hits once the air runs out.
    ///
    /// `TODO(black-box)`.
    pub drown_interval_seconds: f32,
    /// Damage per drowning hit.
    ///
    /// `TODO(black-box)`.
    pub drown_damage: f32,
    /// How many seconds of air one second above water restores.
    ///
    /// `TODO(black-box)`.
    pub air_recovery_rate: f32,
    /// Damage per second while in slime.
    ///
    /// `TODO(black-box)`.
    pub slime_damage_per_second: f32,
    /// Damage per second while in lava.
    ///
    /// `TODO(black-box)`.
    pub lava_damage_per_second: f32,
    /// Seconds between two hits from a damaging volume. Published for
    /// `trigger_hurt` ("a hit every 0.5 seconds, and the amount is 0.5x
    /// `dmg` per hit"); reused for slime and lava contact.
    pub hurt_interval_seconds: f32,
    /// Fraction of [`Self::max_health`] below which the suit calls health
    /// critical.
    ///
    /// `TODO(black-box)`.
    pub health_critical_fraction: f32,
    /// Fraction of [`Self::max_health`] below which the suit calls near
    /// death.
    ///
    /// `TODO(black-box)`.
    pub near_death_fraction: f32,
    /// Fraction of flashlight charge spent per second while it is on.
    ///
    /// `TODO(black-box)`: the flashlight is only documented as draining
    /// slowly while on and recharging while off.
    pub flashlight_drain_per_second: f32,
    /// Fraction of flashlight charge recovered per second while it is off.
    ///
    /// `TODO(black-box)`.
    pub flashlight_recharge_per_second: f32,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            max_health: 100.0,
            max_armor: 100.0,
            air_capacity_seconds: 12.0,
            drown_interval_seconds: 1.0,
            drown_damage: 5.0,
            air_recovery_rate: 4.0,
            slime_damage_per_second: 10.0,
            lava_damage_per_second: 50.0,
            hurt_interval_seconds: 0.5,
            health_critical_fraction: 0.25,
            near_death_fraction: 0.1,
            flashlight_drain_per_second: 1.0 / 100.0,
            flashlight_recharge_per_second: 1.0 / 200.0,
        }
    }
}
