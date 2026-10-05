//! The host side of a local skirmish (`ohl_engine::skirmish`): finding the
//! payload's deathmatch maps, and turning the engine's match status and
//! frag events into what the HUD, the scoreboard and the log show.
//!
//! Map names come from the user's own payload at runtime. They are shown
//! in the user's own menu and passed back to the engine, and are never
//! written to a log line (`docs/CLEAN_ROOM.md`); every line logged here
//! is fixed text or an aggregate count.

use ohl_engine::{AssetSource, SkirmishStatus};
use ohl_ui::scoreboard::{ScoreRow, ScoreboardState};

/// The directory every map lives in, and the extension a map file has —
/// the same `maps/<name>.bsp` convention `ohl_engine::Level::load` reads
/// a map by.
const MAPS_DIR: &str = "maps";
const MAP_EXTENSION: &str = ".bsp";

/// Every map name `listing` (an asset listing of [`MAPS_DIR`]) publishes,
/// as the bare names `--map` takes, sorted and without duplicates.
#[must_use]
pub fn map_names(listing: &[String]) -> Vec<String> {
    let mut names: Vec<String> = listing
        .iter()
        .filter_map(|path| {
            let lower = path.to_ascii_lowercase();
            let rest = lower.strip_prefix(MAPS_DIR)?.strip_prefix('/')?;
            let name = rest.strip_suffix(MAP_EXTENSION)?;
            (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The bare names of every map an [`ohl_assets::AssetFs`] publishes.
#[must_use]
pub fn published_map_names(fs: &ohl_assets::AssetFs) -> Vec<String> {
    fs.list_dir(MAPS_DIR)
        .map(|listing| map_names(&listing))
        .unwrap_or_default()
}

/// Which of `candidates` declare at least one deathmatch spawn point,
/// read through `source`. A map that cannot be read is left out.
#[must_use]
pub fn deathmatch_maps(source: &dyn AssetSource, candidates: &[String]) -> Vec<String> {
    candidates
        .iter()
        .filter(|name| {
            source
                .read(&format!("{MAPS_DIR}/{name}{MAP_EXTENSION}"))
                .and_then(|bytes| ohl_engine::deathmatch_spawn_count(&bytes))
                .is_some_and(|count| count > 0)
        })
        .cloned()
        .collect()
}

/// The engine's status as the scoreboard draws it.
#[must_use]
pub fn scoreboard(status: &SkirmishStatus) -> ScoreboardState {
    ScoreboardState {
        rows: status
            .scoreboard
            .iter()
            .map(|row| ScoreRow {
                name: row.name.clone(),
                frags: row.frags,
                deaths: row.deaths,
                is_local: row.is_human,
                alive: row.alive,
            })
            .collect(),
        frag_limit: (status.frag_limit > 0).then_some(status.frag_limit),
        seconds_left: status.seconds_left,
        winner: status.winner.clone(),
    }
}

/// The centred notice the HUD shows for this status, if any.
#[must_use]
pub fn center_notice(status: &SkirmishStatus) -> Option<String> {
    if let Some(winner) = &status.winner {
        return Some(format!("{winner} wins the match"));
    }
    if status.respawn_ready {
        return Some("Press fire to respawn".to_string());
    }
    status.human_dead.then(|| "You were fragged".to_string())
}

/// One kill feed line for a [`GameEvent::Frag`].
#[must_use]
pub fn kill_feed_text(killer: Option<&str>, victim: &str, weapon: Option<&'static str>) -> String {
    match (killer, weapon) {
        (Some(killer), _) if killer == victim => format!("{victim} killed themself"),
        (Some(killer), Some(weapon)) => format!("{killer} [{weapon}] {victim}"),
        (Some(killer), None) => format!("{killer} killed {victim}"),
        (None, _) => format!("{victim} died"),
    }
}

/// One aggregate log line for a finished headless skirmish: how many
/// combatants played, how many deaths and frags they scored in all, how
/// many bots scored at all, the human's own tally, and whether a limit
/// ended the match. Counts only — no map name, no position.
pub fn log_summary(status: &SkirmishStatus) {
    let deaths: u32 = status.scoreboard.iter().map(|row| row.deaths).sum();
    let frags: i32 = status.scoreboard.iter().map(|row| row.frags).sum();
    let scoring_bots = status
        .scoreboard
        .iter()
        .filter(|row| !row.is_human && row.frags > 0)
        .count();
    let (human_frags, human_deaths) = status
        .scoreboard
        .iter()
        .find(|row| row.is_human)
        .map_or((0, 0), |row| (row.frags, row.deaths));
    tracing::info!(
        combatants = status.scoreboard.len(),
        deaths,
        frags,
        scoring_bots,
        human_frags,
        human_deaths,
        match_over = status.winner.is_some(),
        "Skirmish summary."
    );
}

#[cfg(test)]
mod tests {
    use super::{center_notice, kill_feed_text, map_names, scoreboard};
    use ohl_engine::{ScoreRow, SkirmishStatus};

    fn status() -> SkirmishStatus {
        SkirmishStatus {
            scoreboard: vec![
                ScoreRow {
                    name: "Alpha".into(),
                    frags: 3,
                    deaths: 1,
                    is_human: false,
                    alive: true,
                },
                ScoreRow {
                    name: "Player".into(),
                    frags: 1,
                    deaths: 2,
                    is_human: true,
                    alive: false,
                },
            ],
            frag_limit: 0,
            seconds_left: Some(90.0),
            winner: None,
            intermission_left: None,
            human_dead: true,
            respawn_ready: false,
            human_frags: 1,
        }
    }

    #[test]
    fn map_names_keep_only_top_level_maps_sorted_and_unique() {
        let listing = [
            "maps/ohltest_b.bsp",
            "maps/OHLTEST_A.BSP",
            "maps/ohltest_a.bsp",
            "maps/sub/ohltest_c.bsp",
            "maps/ohltest_d.res",
            "maps/.bsp",
            "models/ohltest_e.bsp",
        ]
        .map(String::from);
        assert_eq!(map_names(&listing), ["ohltest_a", "ohltest_b"]);
    }

    #[test]
    fn the_scoreboard_keeps_the_engines_order_and_marks_the_local_row() {
        let board = scoreboard(&status());
        assert_eq!(board.rows.len(), 2);
        assert_eq!(board.rows[0].name, "Alpha");
        assert!(board.rows[1].is_local);
        assert_eq!(board.frag_limit, None, "0 means no limit");
        assert_eq!(board.seconds_left, Some(90.0));
    }

    #[test]
    fn the_notice_says_what_the_player_can_do() {
        let mut current = status();
        assert_eq!(center_notice(&current).as_deref(), Some("You were fragged"));
        current.respawn_ready = true;
        assert_eq!(
            center_notice(&current).as_deref(),
            Some("Press fire to respawn")
        );
        current.winner = Some("Alpha".into());
        assert_eq!(
            center_notice(&current).as_deref(),
            Some("Alpha wins the match")
        );
        current.winner = None;
        current.human_dead = false;
        current.respawn_ready = false;
        assert_eq!(center_notice(&current), None);
    }

    #[test]
    fn kill_feed_lines_name_who_did_what() {
        assert_eq!(
            kill_feed_text(Some("Alpha"), "Player", Some("shotgun")),
            "Alpha [shotgun] Player"
        );
        assert_eq!(
            kill_feed_text(Some("Alpha"), "Bravo", None),
            "Alpha killed Bravo"
        );
        assert_eq!(
            kill_feed_text(Some("Bravo"), "Bravo", Some("RPG")),
            "Bravo killed themself"
        );
        assert_eq!(kill_feed_text(None, "Bravo", None), "Bravo died");
    }
}
