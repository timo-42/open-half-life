//! The engine's save-game payload, laid into `ohl-save`'s container.
//!
//! [`ohl_save`] owns the container (magic, version, tagged section table,
//! per-section and whole-file SHA-256, atomic slot writes); this module owns
//! what goes in the sections and nothing else. One tag per subsystem, so a
//! later milestone can add a section (weapons, AI) without renumbering, and
//! an older build that does not know a tag simply reports it as unknown
//! rather than failing to open the file.
//!
//! Serialization is `postcard` through [`ohl_save::SaveWriter`], which is
//! deterministic: the same game state and the same header always produce
//! byte-identical files, which is what the save -> load -> save round-trip
//! test asserts.

use ohl_campaign::Difficulty;
use ohl_game::SimulationState;
use serde::{Deserialize, Serialize};

use crate::transition::{EntitySnapshot, GlobalStateTable, PlayerCarryState};

/// Engine header: which map is loaded, its chapter title, the difficulty
/// and the simulated time.
pub const SECTION_ENGINE_HEADER: u32 = 16;

/// The player's carried state, from the [`crate::PlayerCarry`] hook.
pub const SECTION_PLAYER_CARRY: u32 = 17;

/// Entity registry state: one [`EntitySnapshot`] per entity, in spawn
/// order.
pub const SECTION_ENTITY_REGISTRY: u32 = 18;

/// The map-logic simulation's scheduled events and trigger cooldowns.
pub const SECTION_SIMULATION: u32 = 19;

/// The `globalname`/`env_global` state table.
pub const SECTION_GLOBAL_STATE: u32 = 20;

/// The time the light-style animation is evaluated at.
pub const SECTION_LIGHT_STYLE_TIME: u32 = 21;

/// The camera/player pose, so a load resumes exactly where the save was
/// taken rather than at the map's own player start.
pub const SECTION_VIEW: u32 = 22;

/// The engine header section's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineHeader {
    /// The bare map name, as the host asked for it.
    pub map: String,
    /// The chapter title `ohl-campaign` resolved for that map, when it
    /// knows one.
    pub chapter_title: Option<String>,
    /// The difficulty's documented `skill` cvar value (`1`/`2`/`3`).
    pub difficulty: u8,
    /// Seconds of simulated time since the map was loaded.
    pub elapsed: f32,
}

/// The camera/player pose section's contents.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// Eye position in world space.
    pub position: [f32; 3],
    /// Yaw in degrees.
    pub yaw: f32,
    /// Pitch in degrees.
    pub pitch: f32,
}

/// Everything one save file holds, as this crate sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSave {
    /// The container header's creation timestamp, carried on the struct
    /// (not in a section) so a save read back and written again reproduces
    /// the original bytes exactly.
    pub created_at_unix_secs: u64,
    /// The engine header section.
    pub header: EngineHeader,
    /// The camera/player pose.
    pub view: ViewState,
    /// The player carry hook's state.
    pub player: PlayerCarryState,
    /// One snapshot per registry entity, in spawn order.
    pub entities: Vec<EntitySnapshot>,
    /// The map-logic simulation's own bookkeeping.
    pub simulation: SimulationState,
    /// Global variables.
    pub globals: GlobalStateTable,
    /// The light-style animation time, in seconds.
    pub light_style_time: f32,
}

impl GameSave {
    /// The difficulty named by the header, defaulting to
    /// [`Difficulty::Medium`] when the stored value is out of range.
    #[must_use]
    pub fn difficulty(&self) -> Difficulty {
        Difficulty::from_skill_cvar_value(self.header.difficulty).unwrap_or(Difficulty::Medium)
    }

    /// Writes this save into an [`ohl_save`] container.
    ///
    /// The timestamp comes from [`Self::created_at_unix_secs`], which the
    /// host supplies (this crate performs no I/O and reads no clock), so
    /// the output is a pure function of the save's own contents.
    ///
    /// # Errors
    /// [`crate::EngineError::SaveUnwritable`] when a section could not be
    /// serialized or the container's limits reject the result.
    pub fn to_bytes(&self) -> crate::Result<Vec<u8>> {
        let header = ohl_save::Header {
            game_version: ohl_core::VERSION.to_string(),
            created_at_unix_secs: self.created_at_unix_secs,
            // The map name is media-derived, so it stays inside the save
            // file (which the user owns) and is never logged.
            map_identity: self.header.map.clone(),
            title: self
                .header
                .chapter_title
                .clone()
                .unwrap_or_else(|| self.header.map.clone()),
            thumbnail: Vec::new(),
        };
        let mut writer = ohl_save::SaveWriter::begin(header);
        let write = |writer: &mut ohl_save::SaveWriter| -> ohl_save::Result<()> {
            writer.add_section_serde(SECTION_ENGINE_HEADER, &self.header)?;
            writer.add_section_serde(SECTION_PLAYER_CARRY, &self.player)?;
            writer.add_section_serde(SECTION_ENTITY_REGISTRY, &self.entities)?;
            writer.add_section_serde(SECTION_SIMULATION, &self.simulation)?;
            writer.add_section_serde(SECTION_GLOBAL_STATE, &self.globals)?;
            writer.add_section_serde(SECTION_LIGHT_STYLE_TIME, &self.light_style_time)?;
            writer.add_section_serde(SECTION_VIEW, &self.view)?;
            Ok(())
        };
        write(&mut writer).map_err(|_| crate::EngineError::SaveUnwritable)?;
        writer
            .finish(&ohl_save::Limits::default())
            .map_err(|_| crate::EngineError::SaveUnwritable)
    }

    /// Reads a save back out of an [`ohl_save`] container.
    ///
    /// # Errors
    /// [`crate::EngineError::SaveUnreadable`] when the container does not
    /// open, a required section is missing, or a section does not
    /// deserialize.
    pub fn from_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let reader = ohl_save::SaveReader::open(bytes, &ohl_save::Limits::default())
            .map_err(|_| crate::EngineError::SaveUnreadable)?;
        Ok(Self {
            created_at_unix_secs: reader.header().created_at_unix_secs,
            header: section(&reader, SECTION_ENGINE_HEADER)?,
            view: section(&reader, SECTION_VIEW)?,
            player: section(&reader, SECTION_PLAYER_CARRY)?,
            entities: section(&reader, SECTION_ENTITY_REGISTRY)?,
            simulation: section(&reader, SECTION_SIMULATION)?,
            globals: section(&reader, SECTION_GLOBAL_STATE)?,
            light_style_time: section(&reader, SECTION_LIGHT_STYLE_TIME)?,
        })
    }
}

/// Deserializes one section, mapping every failure onto the crate's single
/// opaque save-read reason (a section tag is not media-derived, but the
/// reason a section failed is not worth distinguishing to a caller).
fn section<T: serde::de::DeserializeOwned>(
    reader: &ohl_save::SaveReader<'_>,
    tag: u32,
) -> crate::Result<T> {
    reader
        .deserialize(tag)
        .map_err(|_| crate::EngineError::SaveUnreadable)
}
