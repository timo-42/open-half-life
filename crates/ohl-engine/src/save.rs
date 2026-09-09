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
//! | 22 | [`SECTION_VIEW`] | [`ViewState`]: the camera/player pose (`position` is the physics origin as of M7.9 P4b, not the eye position earlier builds wrote — see that struct's own doc comment) |
//! | 23 | [`SECTION_INVENTORY`] | [`InventorySnapshot`]: owned weapons, clips, ammo reserves, selection, the drawn weapon's firing summary (M7.9 P4b) |
//! | 24 | [`SECTION_ENTITY_COMBAT`] | `Vec<`[`EntityCombatSnapshot`]`>`, one per registry entity, in spawn order (M7.9 P4b) |
//! | 25 | [`SECTION_AI`] | `Vec<Option<`[`AiSnapshot`]`>>`, one per registry entity, in spawn order (M7.9 P4b) |
//! | 26 | [`SECTION_PROJECTILES`] | [`ProjectilesSnapshot`]: live projectiles and placed deployables (M7.9 P4b) |
//! | 27 | [`SECTION_RNG`] | [`RngSnapshot`]: the shared random stream and the substep counter (M7.9 P4b) |
//! | 28 | [`SECTION_MOVER_STATE`] | `Vec<Option<`[`MoverSnapshot`]`>>`, one per registry entity, in spawn order: `func_train`/`func_tracktrain` position, `trigger_camera` sequence progress, running-script phase and `monstermaker` counters (M7.13) |
//! | 29 | [`SECTION_MAKER_CHILDREN`] | `Vec<Option<`[`MonsterMakerChildSnapshot`]`>>`, one per registry entity, in spawn order: which `monstermaker` spawned this entity, and its classname (M9.5) |
//! | 30 | [`SECTION_ROTATING_MOVER_STATE`] | [`RotatingMoverStateSnapshot`]: `func_rot_button`/`momentary_rot_button`/`func_pendulum` runtime state, one optional entry per registry entity, plus the `func_rot_button` touch-edge bookkeeping (M9.6) |
//! | 31 | [`SECTION_MOMENTARY_DOOR_STATE`] | `Vec<Option<`[`MomentaryDoorSnapshot`]`>>`, one per registry entity, in spawn order: `momentary_door` runtime position (M9.8) |
//! | 32 | *(reserved, `ohl-player`)* | `PlayerSnapshot`, written through `Player::snapshot()` when a later package wires it |
//! | 33 | [`SECTION_BREAKABLE_STATE`] | `Vec<Option<`[`BreakableSnapshot`]`>>`, one per registry entity, in spawn order: `func_breakable`/`func_pushable` remaining hit points, broken flag and push offset (M9.10) |
//! | 34 | [`SECTION_TELEPORT_STATE`] | [`TeleportStateSnapshot`]: the `trigger_teleport` touch-edge bookkeeping and the `multisource` master fire counts (M9) |
//! | 35 | [`SECTION_TRAIN_HANDOVER_YAW`] | `Vec<Option<f32>>`, one per registry entity, in spawn order: the heading a `func_tracktrain` was handed across a level change with, for a chain that defines none of its own (M9.25) |
//!
//! Tags 23-31 and 33-35 are read as `None`/a default when absent, so a
//! save written before M7.9 P4b (tags 23-27), M7.13 (tag 28), M9.5 (tag 29),
//! M9.6 (tag 30), M9.8 (tag 31), M9.10 (tag 33), the teleport/master
//! package (tag 34) or M9.25 (tag 35) still loads (`.plan/m79-design.md` §6); a
//! section that is present but fails to decode fails the whole read closed
//! ([`crate::EngineError::SaveUnreadable`]), same as every other section.
//!
//! # Frozen section shapes, the compatibility floor, and the rule
//!
//! `postcard` is not self-describing: a section is decoded as one fixed
//! wire shape, field for field. *Adding or removing a field* on a type any
//! existing tag serializes therefore does not extend the format — it
//! invalidates every save file already written whose section for that tag
//! is non-empty. An optional tag's "missing loads as a default" rule does
//! not help: the tag is present, it just no longer matches.
//!
//! **Every tag listed above is frozen at the shape this build writes**,
//! and that includes the types each one reaches transitively — tag 18's
//! [`EntitySnapshot`] and every component it holds (a field added to
//! `ohl_game::registry::Door` moves tag 18), tag 19's [`SimulationState`],
//! tag 28's [`MoverSnapshot`] and the five snapshot structs it holds.
//! Tags 16-22 are additionally *required*, so for them a shape change is
//! unconditionally fatal; the optional tags are no safer once a save that
//! carries them exists.
//!
//! ## The compatibility floor
//!
//! **A save file written by a build at commit `6090676` or later opens;
//! anything older does not.** Tag 18 is what sets that, not tag 28:
//! [`EntitySnapshot`] gained `rotator` at `6090676` (inserted between
//! `platform` and `light`), and the `ohl_game::registry::Door` it reaches
//! gained `rotation_axis` at `9ea7029` (between `travel_distance` and
//! `state`). Tag 18 is required, so either change alone is enough to
//! reject every older file. Tag 28's own history — it gained `rotator` at
//! `83f968c` — is inside that window and so changes nothing about the
//! floor; a `MoverSnapshot` without that field would rescue no file that
//! is not already lost, while breaking every file written since. Tag 19 is
//! genuinely unmoved: [`SimulationState`] and the types it reaches are the
//! same shape they were well before the floor.
//!
//! From `f64ccfc` on, that floor is meant to hold. Moving it again means
//! abandoning every existing save file, which is a decision to take
//! deliberately and write down here — not a side effect of adding a field.
//!
//! ## A mismatch must fail closed
//!
//! It is not enough for a stale save to be *wrong*; it has to be
//! *rejected*. Neither `postcard::from_bytes` nor a hand-driven
//! `postcard::Deserializer` checks that its input was consumed in full, so
//! a reader one field shorter than the writer used to decode such a
//! section happily — misassigning every field after the missing one and
//! ignoring the surplus bytes. On tag 28 that reads a fired `trigger_auto`
//! back as unfired, replaying it on load: precisely the bug
//! [`MoverSnapshot::auto_trigger_fired`] exists to prevent. A review of
//! this module found that hole; all three decode paths
//! ([`ohl_save::SaveReader::deserialize`] for a required section, this
//! module's own `optional_section`, and `optional_bounded_vec_section`)
//! now require their bytes to be consumed exactly.
//!
//! ## The rule
//!
//! **New persisted state gets a new optional tag**, written only when
//! populated and read through `optional_section` (or
//! `optional_bounded_vec_section`), so an older save simply reports it
//! absent. Tag 30 exists for exactly this reason: its state was first
//! added as fields on tags 18/19/28 and had to be moved out before it
//! shipped (`docs/FORMAT_SOURCES.md` `TODO(black-box)` item 27; item 28
//! records the rest of this section's own history). Tag 31 follows the same
//! rule from the start: `momentary_door`'s own `fraction` never touches
//! tags 18/19/28/30, even though `RotatingMoverSnapshot` (tag 30) already
//! covers the button that drives it (`docs/FORMAT_SOURCES.md`, item 29).
//!
//! `crates/ohl-engine/tests/save_format_frozen.rs` pins tags 16, 17, 18,
//! 19, 20, 21, 22 and 28 with committed golden bytes, decoded by the
//! current reader, and proves the fail-closed behaviour above: a field
//! added to any of them fails those tests rather than shipping a
//! save-breaking build.

use ohl_campaign::Difficulty;
use ohl_game::SimulationState;
use serde::{Deserialize, Serialize};

use crate::save_state::{
    AiSnapshot, BreakableSnapshot, EntityCombatSnapshot, InventorySnapshot, MomentaryDoorSnapshot,
    MonsterMakerChildSnapshot, MoverSnapshot, ProjectilesSnapshot, RngSnapshot,
    RotatingMoverSnapshot,
};
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

/// Mover/camera/script/`monstermaker` runtime state, in spawn order
/// (M7.13).
pub const SECTION_MOVER_STATE: u32 = 28;

/// Which `monstermaker` spawned each registry entity, and its classname, in
/// spawn order (M9.5).
pub const SECTION_MAKER_CHILDREN: u32 = 29;

/// `func_rot_button`/`momentary_rot_button`/`func_pendulum` runtime state,
/// one optional entry per registry entity in spawn order, plus the
/// `func_rot_button` touch-edge bookkeeping `ohl_game::logic::
/// SimulationState` (tag 19) deliberately does not carry (M9.6). A new tag
/// rather than an addition to [`SECTION_MOVER_STATE`] (tag 28) or
/// [`SECTION_ENTITY_REGISTRY`]/[`SECTION_SIMULATION`] (tags 18/19): all
/// three are required sections whose `postcard` encoding is not
/// self-describing, so a field added to a type any of them serializes
/// makes every save written before that field existed fail to decode —
/// see this module's own doc comment on the tag map above. This section
/// stays optional and self-contained instead, so an older save (missing
/// tag 30 entirely) still loads with these three entities defaulting to
/// their spawnflag/keyvalue resting state, exactly like tags 23-29 already
/// do for what they each cover.
pub const SECTION_ROTATING_MOVER_STATE: u32 = 30;

/// A `momentary_door`'s own runtime position, one optional entry per
/// registry entity in spawn order (M9.8, `docs/FORMAT_SOURCES.md` item 29).
/// A new tag rather than an addition to [`SECTION_ROTATING_MOVER_STATE`]
/// (tag 30): that section is already shipped and frozen at its own shape
/// (see this module's own "Frozen section shapes" doc above), so even
/// though a `momentary_door` and the `momentary_rot_button` driving it are
/// closely related, its `fraction` gets its own optional tag rather than
/// reopening tag 30's wire shape — the same reasoning tag 30 itself
/// recorded for staying out of tags 18/19/28.
pub const SECTION_MOMENTARY_DOOR_STATE: u32 = 31;

/// A `func_breakable`/`func_pushable`'s runtime state — remaining hit
/// points, whether it has broken, and how far it has been pushed — one
/// optional entry per registry entity in spawn order (M9.10,
/// `docs/FORMAT_SOURCES.md` item 32).
///
/// **33, not 32**: tag 32 is reserved for `ohl-player`'s own
/// `PlayerSnapshot` in the tag map above, so this section takes the next
/// number after it rather than claiming a tag another package has already
/// been promised. A new tag rather than a field on any existing one, for
/// the reason this module's "Frozen section shapes" doc gives: every
/// shipped section's `postcard` shape is frozen, so new persisted state
/// always gets its own optional tag.
pub const SECTION_BREAKABLE_STATE: u32 = 33;

/// The `trigger_teleport` touch-edge bookkeeping and the `multisource`
/// master fire counts (see [`TeleportStateSnapshot`]).
///
/// Tag 34: tag 32 is reserved for `ohl-player`'s own snapshot and tag 33 is
/// M9.10's `func_breakable`/`func_pushable` state (see this module's tag
/// map). A new tag rather than an addition to
/// [`SECTION_SIMULATION`] (19), which is where the rest of
/// `ohl_game::logic::Simulation`'s bookkeeping lives: that section is
/// shipped and frozen at its own wire shape, so widening
/// `ohl_game::logic::SimulationState` would invalidate every save already
/// written — exactly the reasoning tags 30 and 31 each recorded for
/// staying out of tags 18/19/28.
pub const SECTION_TELEPORT_STATE: u32 = 34;

/// The heading a `func_tracktrain` was handed across a level change with,
/// one optional entry per registry entity in spawn order (M9.25,
/// `docs/FORMAT_SOURCES.md`, "Riding movers").
///
/// `ohl_game::track_train::TrackTrainState::yaw_degrees` normally derives a
/// train's heading from the segment it is on, so nothing about it is state
/// worth saving. A map that *ends* a shared ride parks its own copy of the
/// car on a chain of exactly one `path_track`, though, and such a chain has
/// no segment anywhere in it: the heading carried across the boundary is
/// then the only heading that car has, and it is load-bearing **player**
/// placement — a passenger stands on the car's floor, and a car posed
/// unrotated puts its floor somewhere else entirely.
///
/// A new tag rather than a field on [`SECTION_MOVER_STATE`] (28), which is
/// where the rest of a train's runtime state lives: that section is shipped
/// and frozen at its own wire shape (see this module's "Frozen section
/// shapes"), so widening
/// `crate::save_state::TrackTrainSnapshot` would invalidate every save
/// already written — the same reasoning tags 30, 31, 33 and 34 each
/// recorded for staying out of tags 18/19/28. Tag 35 is the next free
/// number: 32 is reserved for `ohl-player`'s own snapshot.
pub const SECTION_TRAIN_HANDOVER_YAW: u32 = 35;

/// [`SECTION_TELEPORT_STATE`] (34)'s whole payload.
///
/// Both halves are keyed by `hecs` bit pattern rather than by spawn order,
/// matching `ohl_game::logic::TriggerSnapshot`'s own existing convention on
/// tag 19 and [`RotatingMoverStateSnapshot::rot_button_touch`]'s on tag 30.
///
/// Neither half can ride an existing tag, and both fail in the *unsafe*
/// direction if they are simply dropped: a lost teleport touch edge makes
/// the first step after a load a rising edge on the volume the player is
/// standing in (advancing a scripted chain by one scene the player never
/// walked into), and a lost master fire count re-locks a master that had
/// already gone active (stalling a sequence gated on it). See
/// `ohl_game::logic::Simulation::teleport_touching`/`master_fires` for
/// both.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TeleportStateSnapshot {
    /// `ohl_game::logic::Simulation`'s `trigger_teleport` touch-edge
    /// bookkeeping: `(entity bit pattern, touching)` pairs.
    pub teleport_touch: Vec<(u64, bool)>,
    /// `ohl_game::logic::Simulation`'s `multisource` fire counts:
    /// `(entity bit pattern, fires)` pairs.
    pub master_fires: Vec<(u64, u32)>,
}

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
///
/// `position`'s meaning changed with M7.9 P4b: earlier builds wrote and read
/// [`crate::game::Game`]'s own *eye* position here directly, which
/// `Game::restore` then handed to [`ohl_physics::PlayerController::spawn_at`]
/// as the player's *feet* origin — a save/load round trip that displaced the
/// player upward by the stance's eye offset (28 world units, standing)
/// before the next physics step settled it back down. This build writes and
/// expects the feet-level physics origin instead (see the comment at
/// [`crate::game::Game::to_save`]'s own construction of this field), so a
/// save written by an older build still loads (the field's *shape* never
/// changed), but the player spawns slightly high — by that same eye
/// offset — until the first tick's gravity resolves it. Accepted and
/// documented rather than fixed further: an older save already carries no
/// way to tell which convention wrote it, and the visible effect is one
/// frame of settling, not a lost or corrupted position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// The player's feet-level physics origin when a `PlayerController` is
    /// in use (the map has collision hulls), else the free-fly camera's own
    /// position. See this struct's own doc comment for the M7.9 P4b change
    /// in what this field holds.
    pub position: [f32; 3],
    /// Yaw in degrees.
    pub yaw: f32,
    /// Pitch in degrees.
    pub pitch: f32,
}

/// [`SECTION_ROTATING_MOVER_STATE`] (30)'s whole payload: the entity-indexed
/// `func_rot_button`/`momentary_rot_button`/`func_pendulum` state plus the
/// `func_rot_button` touch-edge bookkeeping, which has no entity-indexed
/// shape to share `movers`' slots with (it is keyed by `hecs` bit pattern,
/// not spawn order, matching `ohl_game::logic::TriggerSnapshot`'s own
/// existing convention on tag 19).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RotatingMoverStateSnapshot {
    /// One optional entry per registry entity, in spawn order.
    pub movers: Vec<Option<RotatingMoverSnapshot>>,
    /// `ohl_game::logic::Simulation::rot_button_touch_snapshot`'s own
    /// `(entity bit pattern, touching)` pairs.
    pub rot_button_touch: Vec<(u64, bool)>,
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
    /// (M7.9 P4b). The outer `Option` (`None` for a save missing tag 24
    /// entirely) is this field's own presence; each inner slot's `None`
    /// means the entity itself no longer existed in the world when the
    /// save was taken (a gibbed monster, `world.despawn`ed outright) —
    /// which restore acts on by despawning that slot's entity again, since
    /// a fresh level load always re-spawns every map-declared monster
    /// first. `Some` with both inner fields `None` means the entity
    /// existed but carried neither a health nor an armor component.
    pub entity_combat: Option<Vec<Option<EntityCombatSnapshot>>>,
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
    /// Mover/camera/script/`monstermaker` runtime state, one optional entry
    /// per registry entity, in spawn order, zipped against `entities`
    /// (M7.13). `None` (rather than an empty `Vec`) for a save missing tag
    /// 28.
    pub mover_state: Option<Vec<Option<MoverSnapshot>>>,
    /// Which `monstermaker` spawned each registry entity, and its
    /// classname, one optional entry per registry entity, in spawn order
    /// (M9.5). `None` (rather than an empty `Vec`) for a save missing tag
    /// 29 — an older save simply has no maker children to recreate, the
    /// pre-existing documented gap this section fixes.
    pub maker_children: Option<Vec<Option<MonsterMakerChildSnapshot>>>,
    /// `func_rot_button`/`momentary_rot_button`/`func_pendulum` runtime
    /// state and the `func_rot_button` touch-edge bookkeeping (M9.6).
    /// `None` for a save missing tag 30 — an older save simply has these
    /// three entities default to their spawnflag/keyvalue resting state.
    pub rotating_movers: Option<RotatingMoverStateSnapshot>,
    /// `momentary_door` runtime position, one optional entry per registry
    /// entity, in spawn order (M9.8). `None` for a save missing tag 31 — an
    /// older save simply has every `momentary_door` default to `fraction =
    /// 0.0`, its spawn-time resting position.
    pub momentary_doors: Option<Vec<Option<MomentaryDoorSnapshot>>>,
    /// `func_breakable`/`func_pushable` runtime state, one optional entry
    /// per registry entity, in spawn order (M9.10). `None` for a save
    /// missing tag 33 — an older save simply has every breakable back at
    /// its authored `health`, unbroken, and every pushable back where it
    /// was compiled.
    pub breakables: Option<Vec<Option<BreakableSnapshot>>>,
    /// The teleport touch edges and master fire counts, when present.
    pub teleport_state: Option<TeleportStateSnapshot>,
    /// The heading a `func_tracktrain` was handed across a level change
    /// with, one optional entry per registry entity, in spawn order
    /// (M9.25). `None` for a save missing tag 35 — an older save simply has
    /// every train back on whatever heading its own chain defines, which is
    /// every train except one parked on a single-node chain. See
    /// [`SECTION_TRAIN_HANDOVER_YAW`].
    pub train_handover_yaw: Option<Vec<Option<f32>>>,
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
            if let Some(mover_state) = &self.mover_state {
                writer.add_section_serde(SECTION_MOVER_STATE, mover_state)?;
            }
            if let Some(maker_children) = &self.maker_children {
                writer.add_section_serde(SECTION_MAKER_CHILDREN, maker_children)?;
            }
            if let Some(rotating_movers) = &self.rotating_movers {
                writer.add_section_serde(SECTION_ROTATING_MOVER_STATE, rotating_movers)?;
            }
            if let Some(momentary_doors) = &self.momentary_doors {
                writer.add_section_serde(SECTION_MOMENTARY_DOOR_STATE, momentary_doors)?;
            }
            if let Some(breakables) = &self.breakables {
                writer.add_section_serde(SECTION_BREAKABLE_STATE, breakables)?;
            }
            if let Some(teleport_state) = &self.teleport_state {
                writer.add_section_serde(SECTION_TELEPORT_STATE, teleport_state)?;
            }
            if let Some(train_handover_yaw) = &self.train_handover_yaw {
                writer.add_section_serde(SECTION_TRAIN_HANDOVER_YAW, train_handover_yaw)?;
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
            mover_state: optional_bounded_vec_section(
                &reader,
                SECTION_MOVER_STATE,
                crate::save_state::MAX_SNAPSHOT_MOVERS,
            )?,
            maker_children: optional_bounded_vec_section(
                &reader,
                SECTION_MAKER_CHILDREN,
                crate::save_state::MAX_SNAPSHOT_MAKER_CHILDREN,
            )?,
            rotating_movers: optional_section(&reader, SECTION_ROTATING_MOVER_STATE)?,
            momentary_doors: optional_bounded_vec_section(
                &reader,
                SECTION_MOMENTARY_DOOR_STATE,
                crate::save_state::MAX_SNAPSHOT_MOMENTARY_DOORS,
            )?,
            breakables: optional_bounded_vec_section(
                &reader,
                SECTION_BREAKABLE_STATE,
                crate::save_state::MAX_SNAPSHOT_BREAKABLES,
            )?,
            teleport_state: optional_section(&reader, SECTION_TELEPORT_STATE)?,
            train_handover_yaw: optional_bounded_vec_section(
                &reader,
                SECTION_TRAIN_HANDOVER_YAW,
                crate::save_state::MAX_SNAPSHOT_TRAIN_HANDOVER_YAW,
            )?,
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

/// Deserializes one optional section (tags 23-27, M7.9 P4b, and tag 28,
/// M7.13): `Ok(None)` when the tag is simply absent (a save written before
/// this package existed), [`crate::EngineError::SaveUnreadable`] when it is
/// present but fails to decode. This is the one place this module
/// distinguishes "missing" from "corrupt" — `.plan/m79-design.md` §8 P4b's
/// rule (extended unchanged to tag 28) that a missing section loads as a
/// default while a present-but-broken one fails closed.
fn optional_section<T: serde::de::DeserializeOwned>(
    reader: &ohl_save::SaveReader<'_>,
    tag: u32,
) -> crate::Result<Option<T>> {
    match reader.section(tag) {
        // `take_from_bytes` plus an empty-remainder check, not
        // `postcard::from_bytes`, which discards an unused tail: see
        // `ohl_save::SaveReader::deserialize`'s own doc comment for why a
        // partially consumed section must fail rather than decode.
        Ok(bytes) => match postcard::take_from_bytes(bytes) {
            Ok((value, [])) => Ok(Some(value)),
            _ => Err(crate::EngineError::SaveUnreadable),
        },
        Err(ohl_save::SaveError::SectionNotFound) => Ok(None),
        Err(_) => Err(crate::EngineError::SaveUnreadable),
    }
}

/// Like [`optional_section`], but for a section whose payload is a bare
/// `Vec<Option<T>>` (`SECTION_MOVER_STATE`/tag 28 today), decoded through
/// [`bounded_vec::deserialize_bounded_vec`] instead of a plain
/// `postcard::from_bytes::<Vec<Option<T>>>` so a corrupt or adversarial
/// section's own length prefix — a handful of bytes, wire-cheap to write —
/// cannot itself request a `Vec::with_capacity` far past `max_len` before a
/// single element has actually been validated. `ohl_save::Limits`' own
/// section-byte-count cap already bounds the section as a whole; this
/// additionally bounds the *allocation* the decode attempts, independent of
/// how large a length prefix the bytes claim.
fn optional_bounded_vec_section<T: serde::de::DeserializeOwned>(
    reader: &ohl_save::SaveReader<'_>,
    tag: u32,
    max_len: usize,
) -> crate::Result<Option<Vec<Option<T>>>> {
    match reader.section(tag) {
        Ok(bytes) => bounded_vec::deserialize_bounded_vec(bytes, max_len)
            .map(Some)
            .map_err(|_| crate::EngineError::SaveUnreadable),
        Err(ohl_save::SaveError::SectionNotFound) => Ok(None),
        Err(_) => Err(crate::EngineError::SaveUnreadable),
    }
}

/// A `postcard`-backed `Vec<Option<T>>` decoder that never allocates more
/// than a caller-chosen element cap, regardless of what the wire's own
/// sequence-length prefix claims.
mod bounded_vec {
    use serde::de::{Deserializer as _, SeqAccess, Visitor};

    /// Decodes `bytes` as `Vec<Option<T>>`, capping both the up-front
    /// allocation and the total element count at `max_len`: a claimed
    /// length longer than `max_len` is rejected as soon as the
    /// `(max_len + 1)`th element would be read, before any element beyond
    /// that point is ever decoded or pushed.
    ///
    /// Every byte of `bytes` must be consumed. Driving a
    /// [`postcard::Deserializer`] by hand, unlike calling
    /// [`postcard::from_bytes`], does not check that on its own — it simply
    /// stops once the sequence it was asked for is complete — so this
    /// finalizes the deserializer and rejects a non-empty remainder. That
    /// check is what makes a *shape* change on one of these sections fail
    /// the way [`super`]'s "Frozen section shapes" doc says every section
    /// fails: loudly. Without it, a reader whose element type is one field
    /// *shorter* than the bytes were written with decodes the section
    /// happily, misassigning every field after the missing one and leaving
    /// the surplus bytes unread — silent corruption instead of
    /// [`crate::EngineError::SaveUnreadable`]. A review of this module found
    /// exactly that hole; `crates/ohl-engine/tests/save_format_frozen.rs`'s
    /// `a_bounded_vec_section_with_trailing_bytes_fails_closed` pins the
    /// fix.
    pub(super) fn deserialize_bounded_vec<'de, T: serde::de::Deserialize<'de>>(
        bytes: &'de [u8],
        max_len: usize,
    ) -> postcard::Result<Vec<Option<T>>> {
        let mut deserializer = postcard::Deserializer::from_bytes(bytes);
        let values = deserializer.deserialize_seq(BoundedVecVisitor {
            max_len,
            marker: std::marker::PhantomData,
        })?;
        if deserializer.finalize()?.is_empty() {
            Ok(values)
        } else {
            // `postcard` has no "trailing bytes" error of its own; the
            // caller collapses every failure onto
            // `crate::EngineError::SaveUnreadable` regardless.
            Err(postcard::Error::SerdeDeCustom)
        }
    }

    struct BoundedVecVisitor<T> {
        max_len: usize,
        marker: std::marker::PhantomData<T>,
    }

    impl<'de, T: serde::de::Deserialize<'de>> Visitor<'de> for BoundedVecVisitor<T> {
        type Value = Vec<Option<T>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a sequence of at most {} elements", self.max_len)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            // The claimed size hint is untrusted wire data; only ever use
            // it to cap (never to exceed) `self.max_len`'s own allocation.
            let capacity = seq.size_hint().unwrap_or(0).min(self.max_len);
            let mut values = Vec::with_capacity(capacity);
            while let Some(value) = seq.next_element::<Option<T>>()? {
                if values.len() >= self.max_len {
                    return Err(serde::de::Error::invalid_length(values.len() + 1, &self));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
}
