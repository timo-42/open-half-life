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
//!
//! # The full tag map
//!
//! | tag | section | payload |
//! | --- | --- | --- |
//! | 16 | [`SECTION_ENGINE_HEADER`] | [`EngineHeader`]: map, chapter title, difficulty, elapsed time |
//! | 17 | [`SECTION_PLAYER_CARRY`] | [`PlayerCarryState`]: health, armor, the `crate::combat::CombatState::capture_carry` opaque blob |
//! | 18 | [`SECTION_ENTITY_REGISTRY`] | `Vec<`[`EntitySnapshot`]`>`, one per registry entity, in spawn order |
//! | 19 | [`SECTION_SIMULATION`] | [`SimulationState`]: the map-logic simulation's scheduled events and trigger cooldowns |
//! | 20 | [`SECTION_GLOBAL_STATE`] | [`GlobalStateTable`]: the `globalname`/`env_global` state table |
//! | 21 | [`SECTION_LIGHT_STYLE_TIME`] | `f32`: the time the light-style animation is evaluated at |
//! | 22 | [`SECTION_VIEW`] | [`ViewState`]: the camera/player pose |
//! | 23 | [`SECTION_INVENTORY`] | [`InventorySnapshot`]: owned weapons, clips, ammo reserves, selection, the drawn weapon's firing summary (M7.9 P4b) |
//! | 24 | [`SECTION_ENTITY_COMBAT`] | `Vec<`[`EntityCombatSnapshot`]`>`, one per registry entity, in spawn order (M7.9 P4b) |
//! | 25 | [`SECTION_AI`] | `Vec<Option<`[`AiSnapshot`]`>>`, one per registry entity, in spawn order (M7.9 P4b) |
//! | 26 | [`SECTION_PROJECTILES`] | [`ProjectilesSnapshot`]: live projectiles and placed deployables (M7.9 P4b) |
//! | 27 | [`SECTION_RNG`] | [`RngSnapshot`]: the shared random stream and the substep counter (M7.9 P4b) |
//! | 32 | *(reserved, `ohl-player`)* | `PlayerSnapshot`, written through `Player::snapshot()` when a later package wires it |
//!
//! Tags 23-27 are read as `None`/a default when absent, so a save written
//! before M7.9 P4b still loads (`.plan/m79-design.md` §6); a section that is
//! present but fails to decode fails the whole read closed
//! ([`crate::EngineError::SaveUnreadable`]), same as every other section.

use ohl_campaign::Difficulty;
use ohl_game::SimulationState;
use serde::{Deserialize, Serialize};

use crate::save_state::{AiSnapshot, EntityCombatSnapshot, InventorySnapshot, ProjectilesSnapshot, RngSnapshot};
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

/// The typed weapon/ammo/firing inventory (M7.9 P4b).
pub const SECTION_INVENTORY: u32 = 23;

/// Per-entity health/armor, in spawn order (M7.9 P4b).
pub const SECTION_ENTITY_COMBAT: u32 = 24;

/// Per-entity AI state, in spawn order (M7.9 P4b).
pub const SECTION_AI: u32 = 25;

/// Live projectiles and placed deployables (M7.9 P4b).
pub const SECTION_PROJECTILES: u32 = 26;

/// The shared random stream and the substep counter (M7.9 P4b).
pub const SECTION_RNG: u32 = 27;

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
    /// The typed inventory snapshot (M7.9 P4b). Always populated by a
    /// save this package writes; `None` only for a save read back whose
    /// container has no tag 23 at all (a pre-M7.9-P4b file), in which case
    /// `player.extra`'s legacy blob is what actually restores the
    /// inventory.
    pub inventory: Option<InventorySnapshot>,
    /// Per-entity health/armor, in spawn order, zipped against `entities`
    /// (M7.9 P4b). `None` (rather than an empty `Vec`) for a save missing
    /// tag 24.
    pub entity_combat: Option<Vec<EntityCombatSnapshot>>,
    /// Per-entity AI state, in spawn order, zipped against `entities`
    /// (M7.9 P4b). `None` (rather than an empty `Vec`) for a save missing
    /// tag 25.
    pub ai: Option<Vec<Option<AiSnapshot>>>,
    /// Live projectiles and placed deployables (M7.9 P4b). `None` for a
    /// save missing tag 26.
    pub projectiles: Option<ProjectilesSnapshot>,
    /// The shared random stream and the substep counter (M7.9 P4b). `None`
    /// for a save missing tag 27.
    pub rng: Option<RngSnapshot>,
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
            if let Some(inventory) = &self.inventory {
                writer.add_section_serde(SECTION_INVENTORY, inventory)?;
            }
            if let Some(entity_combat) = &self.entity_combat {
                writer.add_section_serde(SECTION_ENTITY_COMBAT, entity_combat)?;
            }
            if let Some(ai) = &self.ai {
                writer.add_section_serde(SECTION_AI, ai)?;
            }
            if let Some(projectiles) = &self.projectiles {
                writer.add_section_serde(SECTION_PROJECTILES, projectiles)?;
            }
            if let Some(rng) = &self.rng {
                writer.add_section_serde(SECTION_RNG, rng)?;
            }
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
            inventory: optional_section(&reader, SECTION_INVENTORY)?,
            entity_combat: optional_section(&reader, SECTION_ENTITY_COMBAT)?,
            ai: optional_section(&reader, SECTION_AI)?,
            projectiles: optional_section(&reader, SECTION_PROJECTILES)?,
            rng: optional_section(&reader, SECTION_RNG)?,
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

/// Deserializes one optional section (tags 23-27, M7.9 P4b): `Ok(None)`
/// when the tag is simply absent (a save written before this package
/// existed), [`crate::EngineError::SaveUnreadable`] when it is present but
/// fails to decode. This is the one place this module distinguishes
/// "missing" from "corrupt" — `.plan/m79-design.md` §8 P4b's rule that a
/// missing section loads as a default while a present-but-broken one fails
/// closed.
fn optional_section<T: serde::de::DeserializeOwned>(
    reader: &ohl_save::SaveReader<'_>,
    tag: u32,
) -> crate::Result<Option<T>> {
    match reader.section(tag) {
        Ok(bytes) => postcard::from_bytes(bytes)
            .map(Some)
            .map_err(|_| crate::EngineError::SaveUnreadable),
        Err(ohl_save::SaveError::SectionNotFound) => Ok(None),
        Err(_) => Err(crate::EngineError::SaveUnreadable),
    }
}
