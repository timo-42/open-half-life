//! Saving and restoring the player systems.
//!
//! The state goes into one `ohl-save` section, tagged
//! [`PLAYER_STATE_TAG`]. The tag is above `ohl_save::MIN_APPLICATION_TAG`,
//! so the container treats it as application data and never interprets it
//! itself.

use serde::{Deserialize, Serialize};

use ohl_save::{Result, SaveReader, SaveWriter};

use crate::state::PlayerState;
use crate::suit::SuitVoice;
use crate::systems::Player;

/// The `ohl-save` section tag carrying the player systems' state.
///
/// `.plan/m7-design.md` section 4 reserves `0x20` for player state, `0x21`
/// for per-entity health/ammo/inventory and `0x22` for AI state.
pub const PLAYER_STATE_TAG: u32 = 0x20;

/// The version of [`PlayerSnapshot`]'s layout. Bumped whenever a field is
/// removed or changes meaning; adding an optional field does not need a
/// bump.
pub const PLAYER_SNAPSHOT_VERSION: u32 = 1;

/// Everything the player systems restore from a save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    /// [`PLAYER_SNAPSHOT_VERSION`] at the time of writing.
    pub version: u32,
    /// The saved player state.
    pub state: PlayerState,
    /// The HEV voice cooldowns, so reloading does not make the suit repeat
    /// everything it just said.
    pub voice: SuitVoice,
}

impl Player {
    /// Captures this player's state.
    #[must_use]
    pub fn snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            version: PLAYER_SNAPSHOT_VERSION,
            state: self.state.clone(),
            voice: self.voice.clone(),
        }
    }

    /// Restores a snapshot, clamping every restored value into the range
    /// the current [`crate::PlayerConfig`] allows, so a hand-edited or
    /// corrupted save cannot produce an out-of-range player.
    pub fn restore(&mut self, snapshot: &PlayerSnapshot) {
        self.state = snapshot.state.clone();
        self.voice = snapshot.voice.clone();
        self.state.health = clamp(self.state.health, self.config.max_health);
        self.state.armor = clamp(self.state.armor, self.config.max_armor);
        self.state.air_time = clamp(self.state.air_time, self.config.air_capacity_seconds);
        self.state.flashlight.charge = clamp(self.state.flashlight.charge, 1.0);
        self.state.waterlevel = self.state.waterlevel.min(3);
        self.state.dead = self.state.dead || self.state.health <= 0.0;
    }

    /// Writes this player's state into `writer` as section
    /// [`PLAYER_STATE_TAG`].
    pub fn write_section(&self, writer: &mut SaveWriter) -> Result<()> {
        writer.add_section_serde(PLAYER_STATE_TAG, &self.snapshot())?;
        Ok(())
    }

    /// Reads section [`PLAYER_STATE_TAG`] out of `reader` and restores it.
    pub fn read_section(&mut self, reader: &SaveReader<'_>) -> Result<()> {
        let snapshot: PlayerSnapshot = reader.deserialize(PLAYER_STATE_TAG)?;
        self.restore(&snapshot);
        Ok(())
    }
}

fn clamp(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max.max(0.0))
    } else {
        0.0
    }
}
