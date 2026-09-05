//! Sourced Half-Life single-player campaign data: the chapter/map
//! sequence, `startmap`/`trainmap` defaults, chapter navigation, the
//! difficulty enum, and a `skill.cfg`-backed lookup table.
//!
//! `#![no_std]` + `alloc`: this crate carries only data and small bounded
//! lookups, so it compiles into any host (including the freestanding
//! parser worker) exactly like [`ohl_core`], the only crate it depends on
//! (see `xtask/src/graph.rs`'s `ALLOWED_EDGES` table — this crate is
//! deliberately *not* allowed to depend on `ohl-formats`; callers that have
//! already parsed a `liblist.gam`/`skill.cfg` file with that crate adapt
//! the parsed `(key, value)` pairs into [`skill_table::SkillTable`]
//! themselves).
//!
//! # Sources
//!
//! The chapter/map sequence in [`chapters::CHAPTERS`] and the
//! `startmap`/`trainmap` defaults are literal facts drawn from publicly
//! documented sources; see the per-item citations in `chapters.rs` and the
//! consolidated list in `docs/FORMAT_SOURCES.md` ("Campaign map
//! sequence"), reusing the research recorded in `.plan/m8-research.md`.
//!
//! # Open items (flagged "to verify" per `.plan/m8-research.md`)
//!
//! 1. **Interloper's starting map prefix.** Sources disagree (`c4a1a` vs
//!    `c4a2b`); [`chapters::CHAPTERS`] deliberately leaves that chapter's
//!    map list empty rather than commit an unverified literal. Needs a
//!    second independent citation (for example a TWHL map-vault entry or a
//!    legally owned installation's `maps/` listing) before a map name can
//!    be added here.
//! 2. **`env_global` / save-file global-state-variable semantics.** Not
//!    modeled by this crate at all yet; the public pages describing the
//!    exact on-disk/save format could not be fetched during the M8
//!    research pass (VDC and the Half-Life Wiki both returned
//!    bot-verification pages). Any future save/restore design needs that
//!    citation first.
//! 3. **Player-inventory persistence across `changelevel`.** Commonly
//!    understood by the GoldSrc modding community (the player edict,
//!    including weapons/ammo/health, persists automatically, independent
//!    of `globalname`), but not confirmed from a reachable public page in
//!    the M8 research pass. Treat as unconfirmed until a VDC/TWHL citation
//!    is found.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod chapters;
pub mod difficulty;
pub mod skill_table;

pub use chapters::{CHAPTERS, Chapter, STARTMAP, TRAINMAP, chapter_of, next_chapter};
pub use difficulty::Difficulty;
pub use skill_table::{Limits, SkillTable};
