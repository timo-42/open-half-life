//! Bridges `ohl-combat` M7.4 simulation output — inventory/pickup outcomes,
//! `CombatEvent`s and `WeaponAction`s — into `ohl-ui::HudState` updates,
//! queued sound cues and queued viewmodel actions, without either
//! presentation crate needing an edge back to `ohl-combat`.
//!
//! - [`bridge::GameplayBridge`]: the stateful (beyond its two bounded output
//!   queues) glue. `on_weapon_action`, `on_combat_event` and `on_pickup`
//!   each take the current combat-side state (an `Inventory`, a
//!   `Health`/`Armor` pair, a `PickupOutcome`) and write straight to a
//!   caller-owned `HudState`, so replaying a fixed input sequence from a
//!   fixed starting state always produces the same HUD, the same queued
//!   sound cues and the same queued viewmodel actions.
//! - [`sounds::SoundCue`]: a lightweight "please play this" record — an
//!   entity, an `ohl_audio::ChannelClass`, an optional asset path — rather
//!   than an `ohl_audio::PlayRequest` itself, since this crate never
//!   decodes a sound file and so has no `Arc<SoundBuffer>` to put in one;
//!   see the module docs for the full reasoning. Every asset path this
//!   package ships is `None`: no source this project may use publishes
//!   Half-Life's sound file layout as reusable data, and
//!   `docs/CLEAN_ROOM.md` rule 7 requires a clean-room provenance review
//!   before any such literal enters source.
//! - [`viewmodel::ViewModelAction`][]: `Draw`/`Idle`/`Fire`/`Reload`/`Holster`,
//!   this project's own closed vocabulary for "which animation should the
//!   viewmodel play next" (the actual viewmodel rendering is later M7
//!   work), derived from an `ohl_combat::WeaponAction` by
//!   [`viewmodel::from_weapon_action`].
//! - [`entities::classify_entity`]: the thin seam from an
//!   `ohl_game::EntityDef` (a parsed BSP entity) to
//!   `ohl_combat::classify_classname`'s `PickupKind`.
//!
//! No Valve SDK source, decompiled binary or leaked material was consulted;
//! see `docs/CLEAN_ROOM.md`. Every published number this package's
//! dependencies rely on is cited where it is defined
//! (`ohl_combat::ammo`/`weapons`/`pickups`); this crate itself introduces no
//! new numeric fact, only the mapping from combat output to presentation
//! output.
#![forbid(unsafe_code)]

mod bridge;
mod entities;
mod queue;
mod sounds;
mod viewmodel;

pub use bridge::{DEFAULT_QUEUE_CAPACITY, GameplayBridge, PICKUP_MESSAGE_SECONDS};
pub use entities::classify_entity;
pub use sounds::{SoundCue, charger_sound_path, pickup_sound_path, weapon_sound_path};
pub use viewmodel::{ViewModelAction, WeaponCue, from_weapon_action};
