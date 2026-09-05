//! The HUD projection of the player state.
//!
//! `ohl-ui`'s `HudState` is the real HUD model, but it lives behind
//! egui/wgpu/winit. This crate therefore always exposes a dependency-free
//! [`HudSnapshot`] with the same fields, and converts it to `HudState`
//! behind the optional `hud` feature.

use crate::state::PlayerState;

/// The player-owned part of the HUD, with the same fields
/// `ohl_ui::hud::HudState` carries.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HudSnapshot {
    /// Health, clamped to `0`.
    pub health: i32,
    /// Armor, clamped to `0`.
    pub armor: i32,
    /// `0..=3`, the documented waterlevel; the HUD draws an air meter
    /// while it is `3`.
    pub waterlevel: u8,
    /// Air left as a `0.0..=1.0` fraction, for that meter.
    pub air_fraction: f32,
    /// Flashlight charge as a `0.0..=1.0` fraction.
    pub flashlight_charge: f32,
    /// Whether the flashlight is on.
    pub flashlight_on: bool,
    /// Whether the player has the HEV suit; without it the HUD shows no
    /// armor or suit readouts at all.
    pub suit_equipped: bool,
    /// Whether a damage indicator should flash this frame.
    pub damage_flash: bool,
}

impl PlayerState {
    /// This state as a HUD snapshot. `air_capacity` is
    /// `PlayerConfig::air_capacity_seconds`.
    #[must_use]
    pub fn hud_snapshot(&self, air_capacity: f32) -> HudSnapshot {
        let air_fraction = if air_capacity > 0.0 && air_capacity.is_finite() {
            (self.air_time / air_capacity).clamp(0.0, 1.0)
        } else {
            0.0
        };
        HudSnapshot {
            health: self.display_health(),
            armor: self.display_armor(),
            waterlevel: self.waterlevel.min(3),
            air_fraction,
            flashlight_charge: self.flashlight.charge.clamp(0.0, 1.0),
            flashlight_on: self.flashlight.on,
            suit_equipped: self.suit_equipped,
            damage_flash: !self.damage_flags.is_empty(),
        }
    }
}

#[cfg(feature = "hud")]
impl HudSnapshot {
    /// Writes the player-owned fields into an `ohl_ui::hud::HudState`, leaving
    /// the weapon/message fields the combat and map-logic systems own
    /// untouched.
    pub fn apply_to(&self, hud: &mut ohl_ui::hud::HudState) {
        hud.health = self.health;
        hud.armor = self.armor;
        if self.damage_flash {
            hud.trigger_damage_flash();
        }
    }
}
