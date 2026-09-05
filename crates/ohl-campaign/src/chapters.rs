//! The Half-Life single-player chapter/map sequence.
//!
//! Every literal below is a name/identifier fact drawn from publicly
//! documented sources (see `docs/CLEAN_ROOM.md` rule 7: chapter titles and
//! internal `.bsp` map names are reported on multiple independent wikis and
//! in the archived `liblist.gam` key/value pairs, so they qualify as
//! literals from a lawfully public source; no wiki article prose is
//! copied, only these name/identifier facts). This reproduces the table
//! recorded in `.plan/m8-research.md` section 1 and
//! `docs/FORMAT_SOURCES.md` ("Campaign map sequence").
//!
//! Cross-checked sources (repeated per row below by short name):
//! - `vdc-liblist`: developer.valvesoftware.com/wiki/Liblist.gam/Half-Life
//! - `steam-3261669377`: Steam Community guide "half-life 1 map names"
//!   (steamcommunity.com/sharedfiles/filedetails/?id=3261669377)
//! - `steam-2828763459`: Steam Community guide "Half-Life Chapter Maps +
//!   Weapon Codes" (steamcommunity.com/sharedfiles/filedetails/?id=2828763459)
//! - `twhl-changing-levels`: twhl.info/wiki/page/Tutorial:_Changing_Levels
//! - `combineoverwiki-storyline`: combineoverwiki.net/wiki/Half-Life_storyline
//! - `strategywiki-unforeseen`: strategywiki.org/wiki/Half-Life/Unforeseen_Consequences

use alloc::string::String;

/// `liblist.gam` `startmap`: the map loaded for "New Game" (per
/// `vdc-liblist`).
pub const STARTMAP: &str = "c0a0";

/// `liblist.gam` `trainmap`: the map loaded for "Training" (per
/// `vdc-liblist`).
pub const TRAINMAP: &str = "t0a0";

/// The Hazard Course training maps, selected via `trainmap` rather than
/// being part of the main chapter count (per `vdc-liblist`,
/// `steam-3261669377`).
pub const HAZARD_COURSE_MAPS: &[&str] = &[
    "t0a0", "t0a0a", "t0a0b", "t0a0b1", "t0a0b2", "t0a0c", "t0a0d",
];

/// One chapter: its display title and its ordered `.bsp` map name(s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chapter {
    /// The chapter's display title, exactly as shown on level load.
    pub title: &'static str,
    /// The chapter's internal map names, in level-transition order. Empty
    /// for a chapter whose exact starting map is not yet confirmed by a
    /// second independent source (see [`crate`]'s module documentation,
    /// open item 1).
    pub maps: &'static [&'static str],
}

/// The Half-Life single-player chapter sequence, in campaign order.
///
/// Per-row sources (see the module documentation above for the URLs behind
/// each short name):
///
/// | # | Chapter | Source |
/// |---|---|---|
/// | 1 | Black Mesa Inbound | `vdc-liblist`, `combineoverwiki-storyline` |
/// | 2 | Anomalous Materials | `steam-3261669377`, `combineoverwiki-storyline` |
/// | 3 | Unforeseen Consequences | `steam-3261669377`, `strategywiki-unforeseen` |
/// | 4 | Office Complex | `steam-3261669377`, `combineoverwiki-storyline` |
/// | 5 | "We've Got Hostiles!" | `steam-3261669377` |
/// | 6 | Blast Pit | `steam-3261669377` |
/// | 7 | Power Up | `steam-3261669377` |
/// | 8 | On A Rail | `steam-3261669377` |
/// | 9 | Apprehension | `steam-3261669377` |
/// | 10 | Residue Processing | `steam-3261669377` |
/// | 11 | Questionable Ethics | `steam-3261669377`, `combineoverwiki-storyline` |
/// | 12 | Surface Tension | `steam-3261669377` |
/// | 13 | "Forget About Freeman!" | `steam-3261669377` |
/// | 14 | Lambda Core | `steam-3261669377` |
/// | 15 | Xen | `steam-3261669377` |
/// | 16 | Gonarch's Lair | `steam-3261669377`, `combineoverwiki-storyline` |
/// | 17 | Interloper | UNVERIFIED (see open item 1); maps deliberately empty |
/// | 18 | Nihilanth | `steam-3261669377` |
/// | 19 | Endgame | `combineoverwiki-storyline` (internal name "End Game") |
pub const CHAPTERS: &[Chapter] = &[
    Chapter {
        title: "Black Mesa Inbound",
        maps: &["c0a0"],
    },
    Chapter {
        title: "Anomalous Materials",
        maps: &["c1a0"],
    },
    Chapter {
        title: "Unforeseen Consequences",
        maps: &["c1a1", "c1a1a", "c1a1b", "c1a1c", "c1a1d", "c1a1f"],
    },
    Chapter {
        title: "Office Complex",
        maps: &["c1a2", "c1a2a", "c1a2b", "c1a2c", "c1a2d"],
    },
    Chapter {
        title: "\"We've Got Hostiles!\"",
        maps: &["c1a3", "c1a3a", "c1a3b", "c1a3c", "c1a3d"],
    },
    Chapter {
        title: "Blast Pit",
        maps: &[
            "c1a4", "c1a4b", "c1a4d", "c1a4e", "c1a4f", "c1a4g", "c1a4i", "c1a4j", "c1a4k",
        ],
    },
    Chapter {
        title: "Power Up",
        maps: &["c2a1", "c2a1a", "c2a1b"],
    },
    Chapter {
        title: "On A Rail",
        maps: &[
            "c2a2", "c2a2a", "c2a2b1", "c2a2b2", "c2a2c", "c2a2d", "c2a2e", "c2a2f", "c2a2g",
            "c2a2h",
        ],
    },
    Chapter {
        title: "Apprehension",
        maps: &["c2a3", "c2a3a", "c2a3b", "c2a3c", "c2a3d", "c2a3e"],
    },
    Chapter {
        title: "Residue Processing",
        maps: &["c2a4", "c2a4a", "c2a4b", "c2a4c"],
    },
    Chapter {
        title: "Questionable Ethics",
        maps: &["c2a4d", "c2a4e", "c2a4f", "c2a4g"],
    },
    Chapter {
        title: "Surface Tension",
        maps: &[
            "c2a5", "c2a5w", "c2a5x", "c2a5a", "c2a5b", "c2a5c", "c2a5d", "c2a5e", "c2a5f", "c2a5g",
        ],
    },
    Chapter {
        title: "\"Forget About Freeman!\"",
        maps: &["c3a1", "c3a1a", "c3a1b"],
    },
    Chapter {
        title: "Lambda Core",
        maps: &["c3a2", "c3a2a", "c3a2b", "c3a2c", "c3a2d", "c3a2e", "c3a2f"],
    },
    Chapter {
        title: "Xen",
        maps: &["c4a1", "c4a1a", "c4a1b", "c4a1c", "c4a1d", "c4a1e", "c4a1f"],
    },
    Chapter {
        title: "Gonarch's Lair",
        maps: &["c4a2", "c4a2a", "c4a2b"],
    },
    Chapter {
        // UNVERIFIED (open item 1 in `crate` docs): independent sources
        // disagreed on whether Interloper begins at `c4a1a` or `c4a2b`.
        // Deliberately left with no map names until a second citation
        // resolves the disagreement.
        title: "Interloper",
        maps: &[],
    },
    Chapter {
        title: "Nihilanth",
        maps: &["c4a3"],
    },
    Chapter {
        title: "Endgame",
        maps: &["c5a1"],
    },
];

/// Finds the chapter that declares `map` (case-insensitive), if any.
#[must_use]
pub fn chapter_of(map: &str) -> Option<&'static Chapter> {
    CHAPTERS
        .iter()
        .find(|chapter| chapter.maps.iter().any(|m| m.eq_ignore_ascii_case(map)))
}

/// Finds the chapter immediately after the one that declares `map`, if
/// `map` is found and it is not the last chapter.
#[must_use]
pub fn next_chapter(map: &str) -> Option<&'static Chapter> {
    let index = CHAPTERS
        .iter()
        .position(|chapter| chapter.maps.iter().any(|m| m.eq_ignore_ascii_case(map)))?;
    CHAPTERS.get(index + 1)
}

/// Renders a chapter's title as an owned [`String`] (a small convenience
/// for callers building a HUD/menu string; everything else in this module
/// works directly with the `'static` data).
#[must_use]
pub fn title_owned(chapter: &Chapter) -> String {
    String::from(chapter.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startmap_is_first_chapter() {
        let first = chapter_of(STARTMAP).expect("startmap resolves to a chapter");
        assert_eq!(first.title, "Black Mesa Inbound");
    }

    #[test]
    fn trainmap_is_not_a_main_chapter() {
        assert!(chapter_of(TRAINMAP).is_none());
        assert!(HAZARD_COURSE_MAPS.contains(&TRAINMAP));
    }

    #[test]
    fn next_chapter_walks_the_sequence() {
        let anomalous = next_chapter("c0a0").expect("c0a0 has a following chapter");
        assert_eq!(anomalous.title, "Anomalous Materials");

        let unforeseen = next_chapter("c1a0").expect("c1a0 has a following chapter");
        assert_eq!(unforeseen.title, "Unforeseen Consequences");
    }

    #[test]
    fn last_chapter_has_no_next() {
        assert!(next_chapter("c5a1").is_none());
    }

    #[test]
    fn unknown_map_resolves_to_nothing() {
        assert!(chapter_of("not_a_real_map").is_none());
        assert!(next_chapter("not_a_real_map").is_none());
    }

    #[test]
    fn interloper_is_present_but_unverified() {
        let interloper = CHAPTERS
            .iter()
            .find(|chapter| chapter.title == "Interloper")
            .expect("Interloper chapter is listed");
        assert!(
            interloper.maps.is_empty(),
            "Interloper's map prefix is an open item pending a second citation"
        );
    }

    #[test]
    fn case_insensitive_lookup() {
        assert_eq!(
            chapter_of("C0A0").map(|c| c.title),
            chapter_of("c0a0").map(|c| c.title)
        );
    }
}
