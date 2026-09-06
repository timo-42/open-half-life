//! The scripted-input milestone log lines (`docs/m79-design.md` §7),
//! observed purely from data [`ohl_engine::Game`] already exposes.
//!
//! Per that design's §10 logging policy, `ohl-engine` itself never logs;
//! every line below is emitted here, in `ohl-app`, from a counter the
//! engine returns as data. Each line fires at most once per run.
//!
//! All eight of the design's non-`--script-log`-loop lines are wired (the
//! other two, "Scripted input loaded." and "Scripted input finished.", are
//! emitted directly by `crate::game_run`'s scripted loop):
//!
//! - "The player fired a weapon." — [`ohl_engine::Game::weapon_fired_count`]
//!   increasing (M7.9 P4b, using P1's `Systems::weapons` phase).
//! - "A shot hit an entity." — [`ohl_engine::Game::shot_hit_count`]
//!   increasing.
//! - "A monster took damage." — [`ohl_engine::Game::monster_damage_event_count`]
//!   increasing.
//! - "A monster died." — [`ohl_engine::Game::monster_death_count`]
//!   increasing.
//! - "A pickup was collected." — [`ohl_engine::Game::pickup_count`]
//!   increasing (M7.9 P4b, using P1's `Systems::pickups` phase).
//! - "The player took damage." — [`ohl_engine::Game::player_damage_event_count`]
//!   increasing (M7.9 P4b, using P1's phase 9 damage resolution — a hit
//!   aimed at the player is now actually applied rather than discarded).
//! - "A scripted sequence started." / "A scripted sequence finished." —
//!   `ohl_engine::Game::active_script_count` rising above zero and, having
//!   done so, returning to it. Both are fixed strings: no script, monster
//!   or sequence name is ever interpolated (design §10).

use ohl_engine::Game;

/// Tracks which milestone lines have already fired this run.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one latch per milestone line, plus one edge detector"
)]
#[derive(Debug)]
pub struct ScriptLog {
    weapon_fired: bool,
    shot_hit: bool,
    monster_damaged: bool,
    monster_died: bool,
    pickup_collected: bool,
    player_damaged: bool,
    script_started: bool,
    script_finished: bool,
    script_was_active: bool,
    baseline_fired: u64,
    baseline_hit: u64,
    baseline_damage_events: u64,
    baseline_deaths: u64,
    baseline_pickups: u64,
    baseline_player_damage: u64,
}

impl ScriptLog {
    /// Starts tracking from `game`'s current counters, so a script that
    /// resumes mid-level (a `--load` combination) does not falsely report
    /// a milestone that happened before the script started.
    #[must_use]
    pub fn new(game: &Game) -> Self {
        Self {
            weapon_fired: false,
            shot_hit: false,
            monster_damaged: false,
            monster_died: false,
            pickup_collected: false,
            player_damaged: false,
            script_started: false,
            script_finished: false,
            script_was_active: game.active_script_count() > 0,
            baseline_fired: game.weapon_fired_count(),
            baseline_hit: game.shot_hit_count(),
            baseline_damage_events: game.monster_damage_event_count(),
            baseline_deaths: game.monster_death_count(),
            baseline_pickups: game.pickup_count(),
            baseline_player_damage: game.player_damage_event_count(),
        }
    }

    /// Call once per scripted tick, after [`Game::tick`]. Emits any
    /// milestone line that just became true; never emits the same line
    /// twice.
    pub fn observe(&mut self, game: &Game) {
        if !self.weapon_fired && game.weapon_fired_count() > self.baseline_fired {
            self.weapon_fired = true;
            tracing::info!("The player fired a weapon.");
        }
        if !self.shot_hit && game.shot_hit_count() > self.baseline_hit {
            self.shot_hit = true;
            tracing::info!("A shot hit an entity.");
        }
        if !self.monster_damaged && game.monster_damage_event_count() > self.baseline_damage_events
        {
            self.monster_damaged = true;
            tracing::info!("A monster took damage.");
        }
        if !self.monster_died && game.monster_death_count() > self.baseline_deaths {
            self.monster_died = true;
            tracing::info!("A monster died.");
        }
        if !self.pickup_collected && game.pickup_count() > self.baseline_pickups {
            self.pickup_collected = true;
            tracing::info!("A pickup was collected.");
        }
        if !self.player_damaged && game.player_damage_event_count() > self.baseline_player_damage {
            self.player_damaged = true;
            tracing::info!("The player took damage.");
        }

        let active = game.active_script_count() > 0;
        if active && !self.script_started {
            self.script_started = true;
            tracing::info!("A scripted sequence started.");
        }
        if !active && self.script_was_active && !self.script_finished {
            self.script_finished = true;
            tracing::info!("A scripted sequence finished.");
        }
        self.script_was_active = active;
    }
}
