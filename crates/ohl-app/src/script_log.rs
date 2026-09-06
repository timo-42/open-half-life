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
//! - "A camera sequence started." / "A camera sequence finished." — the
//!   same rising-then-falling edge, over
//!   `ohl_engine::Game::camera_sequence_active` (M7.12, `trigger_camera`).
//!   Also fixed strings: no camera or `target` name is ever interpolated.
//! - "The player moved from the spawn point." — the player's eye position
//!   ([`ohl_engine::Game::eye_position`]) has moved more than
//!   [`SPAWN_MOVE_THRESHOLD`] world units from where it stood when this
//!   [`ScriptLog`] was created. A data-derived boolean latch: the distance
//!   itself is never logged.
//! - "The player is inside solid geometry." — this run's own regression
//!   guard for the PR #91 class of bug (the player falling through a brush
//!   entity's floor): [`ohl_engine::Game::eye_is_in_solid`] has read `true`
//!   for a cumulative total of more than [`IN_SOLID_LOG_THRESHOLD_SECS`]
//!   seconds. Every combat-smoke scenario asserts this line absent.

use ohl_engine::Game;

/// How far the player's eye must move from its spawn position, in world
/// units, before [`ScriptLog::observe`] logs "The player moved from the
/// spawn point." Chosen generously above ordinary standing-still jitter
/// (turning in place, brief physics settling) so the line only fires for a
/// scenario that actually walks somewhere.
pub const SPAWN_MOVE_THRESHOLD: f32 = 64.0;

/// How many cumulative seconds [`ohl_engine::Game::eye_is_in_solid`] must
/// read `true` during one scripted run before [`ScriptLog::observe`] logs
/// "The player is inside solid geometry." A single-tick false positive
/// (e.g. a brief overlap while a door is still closing) should not trip
/// this regression guard; a player who has actually fallen through the
/// world stays in solid geometry far longer than one second.
pub const IN_SOLID_LOG_THRESHOLD_SECS: f32 = 1.0;

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
    camera_started: bool,
    camera_finished: bool,
    camera_was_active: bool,
    moved_from_spawn: bool,
    in_solid: bool,
    baseline_fired: u64,
    baseline_hit: u64,
    baseline_damage_events: u64,
    baseline_deaths: u64,
    baseline_pickups: u64,
    baseline_player_damage: u64,
    spawn_position: [f32; 3],
    in_solid_seconds: f32,
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
            camera_started: false,
            camera_finished: false,
            camera_was_active: game.camera_sequence_active(),
            moved_from_spawn: false,
            in_solid: false,
            baseline_fired: game.weapon_fired_count(),
            baseline_hit: game.shot_hit_count(),
            baseline_damage_events: game.monster_damage_event_count(),
            baseline_deaths: game.monster_death_count(),
            baseline_pickups: game.pickup_count(),
            baseline_player_damage: game.player_damage_event_count(),
            spawn_position: game.eye_position(),
            in_solid_seconds: 0.0,
        }
    }

    /// Call once per scripted tick, after [`Game::tick`], with the same
    /// `dt` the tick just advanced by. Emits any milestone line that just
    /// became true; never emits the same line twice.
    pub fn observe(&mut self, game: &Game, dt: f32) {
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

        let camera_active = game.camera_sequence_active();
        if camera_active && !self.camera_started {
            self.camera_started = true;
            tracing::info!("A camera sequence started.");
        }
        if !camera_active && self.camera_was_active && !self.camera_finished {
            self.camera_finished = true;
            tracing::info!("A camera sequence finished.");
        }
        self.camera_was_active = camera_active;

        if !self.moved_from_spawn {
            let eye = game.eye_position();
            let dx = eye[0] - self.spawn_position[0];
            let dy = eye[1] - self.spawn_position[1];
            let dz = eye[2] - self.spawn_position[2];
            let distance_squared = dx * dx + dy * dy + dz * dz;
            if distance_squared > SPAWN_MOVE_THRESHOLD * SPAWN_MOVE_THRESHOLD {
                self.moved_from_spawn = true;
                tracing::info!("The player moved from the spawn point.");
            }
        }

        if !self.in_solid {
            if game.eye_is_in_solid() {
                self.in_solid_seconds += dt;
                if self.in_solid_seconds > IN_SOLID_LOG_THRESHOLD_SECS {
                    self.in_solid = true;
                    tracing::info!("The player is inside solid geometry.");
                }
            } else {
                self.in_solid_seconds = 0.0;
            }
        }
    }
}
