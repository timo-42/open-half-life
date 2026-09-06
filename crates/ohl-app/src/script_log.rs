//! The scripted-input milestone log lines (`docs/m79-design.md` §7),
//! observed purely from data [`ohl_engine::Game`] already exposes.
//!
//! Per that design's §10 logging policy, `ohl-engine` itself never logs;
//! every line below is emitted here, in `ohl-app`, from a counter the
//! engine returns as data. Each line fires at most once per run.
//!
//! Of the eight documented lines, `--script-log`'s caller
//! (`crate::game_run`) emits "Scripted input loaded." and "Scripted input
//! finished." directly, and this module wires the two whose source exists
//! in this tree today:
//!
//! - "A monster took damage." — [`ohl_engine::Game::monster_damage_event_count`]
//!   increasing past its value when the script started.
//! - "A monster died." — [`ohl_engine::Game::monster_death_count`]
//!   increasing the same way.
//!
//! The remaining four are TODO hooks, not wired, because their sources do
//! not exist on this branch (M7.9 P1, PR #70, is not merged; see
//! `docs/m79-design.md` §0):
//!
//! - TODO(P1): "The player fired a weapon." — needs the weapon firing
//!   state machine (`ohl-engine`'s phase 6, `Systems::weapons`, currently
//!   an empty hook).
//! - TODO(P1): "A shot hit an entity." — needs hitscan resolution (the
//!   same phase).
//! - TODO(P1): "A pickup was collected." — needs the pickup touch system
//!   (phase 11, `Systems::pickups`, currently an empty hook).
//! - TODO(P1): "The player took damage." — damage aimed at a non-monster
//!   target, the player included, is currently discarded rather than
//!   applied (`ohl-engine`'s `ai::AiState::drain_engine_damage`, phase 9
//!   `Systems::resolve_damage` is still an empty hook), so it is not an
//!   observable engine event yet. Once P1 lands phase 9 and a player
//!   `Health` component is actually debited, this line wires the same way
//!   the two above do.

use ohl_engine::Game;

/// Tracks which milestone lines have already fired this run.
#[derive(Debug)]
pub struct ScriptLog {
    monster_damaged: bool,
    monster_died: bool,
    baseline_damage_events: u64,
    baseline_deaths: u64,
}

impl ScriptLog {
    /// Starts tracking from `game`'s current counters, so a script that
    /// resumes mid-level (a future `--load` combination) does not falsely
    /// report a milestone that happened before the script started.
    #[must_use]
    pub fn new(game: &Game) -> Self {
        Self {
            monster_damaged: false,
            monster_died: false,
            baseline_damage_events: game.monster_damage_event_count(),
            baseline_deaths: game.monster_death_count(),
        }
    }

    /// Call once per scripted tick, after [`Game::tick`]. Emits any
    /// milestone line that just became true; never emits the same line
    /// twice.
    pub fn observe(&mut self, game: &Game) {
        if !self.monster_damaged && game.monster_damage_event_count() > self.baseline_damage_events
        {
            self.monster_damaged = true;
            tracing::info!("A monster took damage.");
        }
        if !self.monster_died && game.monster_death_count() > self.baseline_deaths {
            self.monster_died = true;
            tracing::info!("A monster died.");
        }
    }
}
