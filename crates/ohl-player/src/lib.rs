//! Player systems for Open Half-Life: health and HEV armor, fall damage,
//! drowning, contact damage, the HEV suit voice, the flashlight, the long
//! jump module and the HUD/save projection of all of it.
//!
//! This crate owns everything about the player that is *not* motion.
//! Motion — walking, swimming, climbing a `func_ladder`, riding a
//! `func_plat`, the long jump impulse itself — lives in `ohl-physics`,
//! which reports what happened through `ohl_physics::MoveEvents`; this
//! crate turns those reports into damage, suit warnings and HUD state.
//! Nothing here draws or plays anything: [`PlayerEvent`]s are pushed to
//! the host, which maps them to `ohl_audio` and `ohl_ui`.
//!
//! Everything was written from public documentation only; see
//! `docs/FORMAT_SOURCES.md`, "Player systems", for the sources of each
//! behaviour and of every published constant, and `docs/CLEAN_ROOM.md` for
//! the project's clean-room policy. Constants that no reachable public
//! source gives are neutral placeholders marked `TODO(black-box)` in the
//! code, to be measured against legally obtained retail software before
//! this project claims parity.
#![forbid(unsafe_code)]

pub mod damage;
pub mod hud;
pub mod input;
pub mod save;
pub mod state;
pub mod suit;
pub mod systems;

pub use damage::{
    Absorbed, DamageFlags, DamageKind, absorb, damage_kind_from_bits, damage_type_bits, fall_damage,
};
pub use hud::HudSnapshot;
pub use input::{ContentsQuery, EmptyWorld, HurtInput, PhysicsOutput, PlayerInput};
pub use save::{PLAYER_SNAPSHOT_VERSION, PLAYER_STATE_TAG, PlayerSnapshot};
pub use state::{Flashlight, PlayerConfig, PlayerState};
pub use suit::{SuitEvent, SuitOccasion, SuitVoice};
pub use systems::{Player, PlayerEvent, PlayerSystems};
