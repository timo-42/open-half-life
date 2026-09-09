//! The Half-Life single-player chapter/map sequence.
//!
//! Every literal below is a name/identifier fact drawn from publicly
//! documented sources (see `docs/CLEAN_ROOM.md` rule 7: chapter titles and
//! internal `.bsp` map names are reported on multiple independent wikis and
//! in the archived `liblist.gam` key/value pairs, so they qualify as
//! literals from a lawfully public source; no wiki article prose is
//! copied, only these name/identifier facts). This reproduces the table
//! recorded in section 1 of the M8 research pass (local research notes,
//! not part of the repository) and `docs/FORMAT_SOURCES.md` ("Campaign
//! map sequence").
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

/// Additional public sources used only for [`CHAPTER_MAPS`] below (the
/// short-name sources reused unchanged from [`CHAPTERS`]'s own doc comment
/// are not repeated here):
///
/// - `sourceruns-wiki-maps`: SourceRuns Wiki, "Half-Life Maps"
///   (<https://wiki.sourceruns.org/Half-Life-Maps.html>), a read-only
///   archive page whose "Singleplayer Maps" section gives one `<h3>` per
///   chapter title, each followed by a `<ul>` of that chapter's `.bsp` map
///   names — fetched directly (HTTP 200) rather than as a search summary.
/// - `combineoverwiki-storyline` (already cited by [`CHAPTERS`]) is quoted
///   again below for one specific sentence, fetched directly: "It is
///   possible that _Gonarch's Lair_ was originally placed after
///   _Interloper_ as the _Gonarch's Lair_ map names begin from `c4a2`,
///   while _Interloper_ map names begin from `c4a1a`."
/// - `steam-3261669377-search`: a search-engine summary of the
///   already-cited Steam Community guide "half-life 1 map names"
///   (<https://steamcommunity.com/sharedfiles/filedetails/?id=3261669377>),
///   used only as cross-check corroboration below; the guide itself
///   returned HTTP 403/429 ("You've made too many requests recently") on
///   every direct fetch attempt during this pass, so it is labelled
///   honestly as a search summary, never quoted as if fetched directly.
///
/// One chapter's *interior* map list: every `.bsp` map name reachable by a
/// level change while still inside that chapter, keyed by a
/// [`Chapter::title`] from [`CHAPTERS`].
///
/// This table exists because [`Chapter::maps`] records only the chapter's
/// starting map for two chapters ("Black Mesa Inbound", "Anomalous
/// Materials") even though the real chapter has several interior maps
/// reached by level changes off that start; code that needs to name one of
/// those interior destinations (for example a chained scripted route named
/// after the map a level change lands in, rather than by its ordinal
/// position — see `xtask/chain-routes/` and PR #134) previously had no
/// cited literal to use for them. For every other chapter, this table
/// reuses [`CHAPTERS`]'s own list verbatim (same set, same citation as
/// that row in [`CHAPTERS`]'s per-row source table above); it is not a
/// second, independently-ordered source for those chapters, even though
/// `sourceruns-wiki-maps` independently corroborates the same *set* of
/// names for every chapter it lists (its per-chapter ordering sometimes
/// differs from [`CHAPTERS`]'s level-transition order, e.g. for "Blast
/// Pit" and "Lambda Core"; [`CHAPTERS`]'s existing order is kept as the
/// order of record since other code already depends on it).
///
/// "Interloper" is deliberately left empty here too, mirroring
/// [`CHAPTERS`] (see the module doc comment's open item 1) rather than
/// resolved from `sourceruns-wiki-maps`/`combineoverwiki-storyline`'s
/// agreement that it begins at `c4a1a`: [`CHAPTERS`]'s own "Xen" row
/// already assigns `c4a1a`..`c4a1f` to *Xen*, not Interloper, so adopting
/// those two sources' split here would give the same map names to two
/// different chapters in this table. Reconciling that disagreement (which
/// chapter title `c4a1a`..`c4a1f` actually belongs under) is left to a
/// follow-up that can also update [`CHAPTERS`] itself; this table only
/// reuses [`CHAPTERS`]'s own existing chapter/map assignment, never a
/// different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChapterMaps {
    /// Matches a [`Chapter::title`] in [`CHAPTERS`].
    pub title: &'static str,
    /// The chapter's interior map names, in level-transition order where
    /// that order is already established by [`CHAPTERS`] (see the doc
    /// comment above).
    pub maps: &'static [&'static str],
}

/// The Half-Life single-player chapters' interior map lists, indexed by
/// chapter title (see [`ChapterMaps`] and the doc comment above it for
/// sourcing).
pub const CHAPTER_MAPS: &[ChapterMaps] = &[
    ChapterMaps {
        title: "Black Mesa Inbound",
        // New literals beyond `CHAPTERS`' "Black Mesa Inbound" row (which
        // lists only the chapter's starting map, `c0a0`). Per
        // `sourceruns-wiki-maps`, the "Black Mesa Inbound" `<ul>` is
        // exactly: "c0a0 / c0a0a / c0a0b / c0a0c / c0a0d / c0a0e" (one name
        // per `<li>`, in that order). Cross-checked (search-summary only;
        // see `steam-3261669377-search` above) against the Steam Community
        // guide "half-life 1 map names", whose indexed content names the
        // same six maps for this chapter.
        maps: &["c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e"],
    },
    ChapterMaps {
        title: "Anomalous Materials",
        // New literals beyond `CHAPTERS`' "Anomalous Materials" row (which
        // lists only `c1a0`). Per `sourceruns-wiki-maps`, the "Anomalous
        // Materials" `<ul>` is exactly: "c1a0 / c1a0d / c1a0a / c1a0b /
        // c1a0e" (one name per `<li>`, in that order).
        maps: &["c1a0", "c1a0d", "c1a0a", "c1a0b", "c1a0e"],
    },
    ChapterMaps {
        title: "Unforeseen Consequences",
        // Reuses `CHAPTERS`' row verbatim (same citation: `steam-3261669377`,
        // `strategywiki-unforeseen`). `sourceruns-wiki-maps` lists the same
        // five suffixed names (`c1a1a`, `c1a1b`, `c1a1c`, `c1a1d`, `c1a1f`)
        // for this chapter, in a different order, but omits the bare
        // `c1a1` its own list starts from; `CHAPTERS`' order is kept.
        maps: &["c1a1", "c1a1a", "c1a1b", "c1a1c", "c1a1d", "c1a1f"],
    },
    ChapterMaps {
        title: "Office Complex",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical five names in the identical order for this chapter.
        maps: &["c1a2", "c1a2a", "c1a2b", "c1a2c", "c1a2d"],
    },
    ChapterMaps {
        title: "\"We've Got Hostiles!\"",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical five names in the identical order for this chapter.
        maps: &["c1a3", "c1a3a", "c1a3b", "c1a3c", "c1a3d"],
    },
    ChapterMaps {
        title: "Blast Pit",
        // Reuses `CHAPTERS`' row verbatim (same citation: `steam-3261669377`).
        // `sourceruns-wiki-maps` lists the same nine names for this chapter
        // in a different order; `CHAPTERS`' order is kept (see the doc
        // comment above [`CHAPTER_MAPS`]).
        maps: &[
            "c1a4", "c1a4b", "c1a4d", "c1a4e", "c1a4f", "c1a4g", "c1a4i", "c1a4j", "c1a4k",
        ],
    },
    ChapterMaps {
        title: "Power Up",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical three names in the identical order for this chapter.
        maps: &["c2a1", "c2a1a", "c2a1b"],
    },
    ChapterMaps {
        title: "On A Rail",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical ten names in the identical order for this chapter.
        maps: &[
            "c2a2", "c2a2a", "c2a2b1", "c2a2b2", "c2a2c", "c2a2d", "c2a2e", "c2a2f", "c2a2g",
            "c2a2h",
        ],
    },
    ChapterMaps {
        title: "Apprehension",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical six names in the identical order for this chapter.
        maps: &["c2a3", "c2a3a", "c2a3b", "c2a3c", "c2a3d", "c2a3e"],
    },
    ChapterMaps {
        title: "Residue Processing",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical four names in the identical order for this chapter.
        maps: &["c2a4", "c2a4a", "c2a4b", "c2a4c"],
    },
    ChapterMaps {
        title: "Questionable Ethics",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical four names in the identical order for this chapter.
        maps: &["c2a4d", "c2a4e", "c2a4f", "c2a4g"],
    },
    ChapterMaps {
        title: "Surface Tension",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical ten names in the identical order for this chapter.
        maps: &[
            "c2a5", "c2a5w", "c2a5x", "c2a5a", "c2a5b", "c2a5c", "c2a5d", "c2a5e", "c2a5f", "c2a5g",
        ],
    },
    ChapterMaps {
        title: "\"Forget About Freeman!\"",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical three names in the identical order for this chapter.
        maps: &["c3a1", "c3a1a", "c3a1b"],
    },
    ChapterMaps {
        title: "Lambda Core",
        // Reuses `CHAPTERS`' row verbatim (same citation: `steam-3261669377`).
        // `sourceruns-wiki-maps` lists the same seven names for this chapter
        // starting from `c3a2e`; `CHAPTERS`' order is kept (see the doc
        // comment above [`CHAPTER_MAPS`]).
        maps: &["c3a2", "c3a2a", "c3a2b", "c3a2c", "c3a2d", "c3a2e", "c3a2f"],
    },
    ChapterMaps {
        title: "Xen",
        // Reuses `CHAPTERS`' row verbatim (same citation: `steam-3261669377`).
        // Note: `sourceruns-wiki-maps` instead lists only `c4a1` under "Xen"
        // and puts `c4a1a`..`c4a1f` under "Interloper"; `CHAPTERS`' existing
        // assignment is kept as the order of record (see the doc comment
        // above [`CHAPTER_MAPS`]).
        maps: &["c4a1", "c4a1a", "c4a1b", "c4a1c", "c4a1d", "c4a1e", "c4a1f"],
    },
    ChapterMaps {
        title: "Gonarch's Lair",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists the
        // identical three names in the identical order for this chapter.
        maps: &["c4a2", "c4a2a", "c4a2b"],
    },
    ChapterMaps {
        // Deliberately empty, mirroring `CHAPTERS`' own "Interloper" row
        // (see the doc comment above [`CHAPTER_MAPS`] for why the new
        // `sourceruns-wiki-maps`/`combineoverwiki-storyline` evidence
        // agreeing on `c4a1a` is not adopted here: `CHAPTERS` already
        // assigns those same names to "Xen").
        title: "Interloper",
        maps: &[],
    },
    ChapterMaps {
        title: "Nihilanth",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists only
        // `c4a3` under "Nihilanth" too.
        maps: &["c4a3"],
    },
    ChapterMaps {
        title: "Endgame",
        // Reuses `CHAPTERS`' row verbatim; `sourceruns-wiki-maps` lists only
        // `c5a1` under its "Outro" heading (its title for this chapter
        // differs from `CHAPTERS`' "Endgame", but the map name matches).
        maps: &["c5a1"],
    },
];

/// Finds a chapter's interior map list by title (case-sensitive; titles
/// are exact `CHAPTERS`/`CHAPTER_MAPS` literals, not free text).
#[must_use]
pub fn chapter_maps(title: &str) -> Option<&'static [&'static str]> {
    CHAPTER_MAPS
        .iter()
        .find(|entry| entry.title == title)
        .map(|entry| entry.maps)
}

/// Whether `name` is a map-name literal that is present in one of this
/// module's cited tables ([`STARTMAP`], [`TRAINMAP`], [`HAZARD_COURSE_MAPS`],
/// [`CHAPTERS`], or [`CHAPTER_MAPS`]) — a case-insensitive check callers can
/// use before writing a map-name literal elsewhere (route fixtures, tests,
/// docs), so that literal stays traceable to this module's own citations
/// instead of being derived from a payload listing (`docs/CLEAN_ROOM.md`
/// rule 7).
#[must_use]
pub fn is_cited_map_name(name: &str) -> bool {
    STARTMAP.eq_ignore_ascii_case(name)
        || TRAINMAP.eq_ignore_ascii_case(name)
        || HAZARD_COURSE_MAPS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(name))
        || CHAPTERS
            .iter()
            .any(|chapter| chapter.maps.iter().any(|m| m.eq_ignore_ascii_case(name)))
        || CHAPTER_MAPS
            .iter()
            .any(|entry| entry.maps.iter().any(|m| m.eq_ignore_ascii_case(name)))
}

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

    #[test]
    fn chapter_maps_table_is_nonempty_per_chapter() {
        for chapter in CHAPTERS {
            let entry = CHAPTER_MAPS
                .iter()
                .find(|entry| entry.title == chapter.title)
                .unwrap_or_else(|| panic!("no CHAPTER_MAPS row for chapter {:?}", chapter.title));
            if chapter.title == "Interloper" {
                // Deliberately empty, mirroring `CHAPTERS`' own unresolved
                // "Interloper" row (see `chapters.rs`'s doc comment above
                // `CHAPTER_MAPS` and `interloper_is_present_but_unverified`
                // above for why).
                assert!(entry.maps.is_empty());
                continue;
            }
            assert!(
                !entry.maps.is_empty(),
                "CHAPTER_MAPS[{:?}] must not be empty",
                chapter.title
            );
        }
    }

    #[test]
    fn chapter_maps_table_covers_every_chapter_exactly_once() {
        assert_eq!(CHAPTER_MAPS.len(), CHAPTERS.len());
        for chapter in CHAPTERS {
            let count = CHAPTER_MAPS
                .iter()
                .filter(|entry| entry.title == chapter.title)
                .count();
            assert_eq!(
                count, 1,
                "chapter {:?} must appear exactly once",
                chapter.title
            );
        }
    }

    #[test]
    fn chapter_maps_names_are_lowercase_ascii() {
        for entry in CHAPTER_MAPS {
            for map in entry.maps {
                assert!(
                    map.chars().all(|c| c.is_ascii() && !c.is_ascii_uppercase()),
                    "map name {map:?} in chapter {:?} must be lowercase ASCII",
                    entry.title
                );
            }
        }
    }

    #[test]
    fn chapter_maps_names_have_no_duplicates_within_a_chapter() {
        for entry in CHAPTER_MAPS {
            for (index, map) in entry.maps.iter().enumerate() {
                assert!(
                    !entry.maps[..index].contains(map),
                    "duplicate map name {map:?} in chapter {:?}",
                    entry.title
                );
            }
        }
    }

    #[test]
    fn every_chapters_first_map_appears_in_its_chapter_maps_entry() {
        for chapter in CHAPTERS {
            let Some(first_map) = chapter.maps.first() else {
                // Chapters left deliberately empty in `CHAPTERS` (the
                // unverified "Interloper" row) have nothing to check here.
                continue;
            };
            let entry = CHAPTER_MAPS
                .iter()
                .find(|entry| entry.title == chapter.title)
                .unwrap_or_else(|| panic!("no CHAPTER_MAPS row for chapter {:?}", chapter.title));
            assert!(
                entry.maps.iter().any(|m| m == first_map),
                "chapter {:?}'s first CHAPTERS map {:?} is missing from its CHAPTER_MAPS entry",
                chapter.title,
                first_map
            );
        }
    }

    #[test]
    fn chapter_maps_lookup_by_title() {
        assert_eq!(
            chapter_maps("Black Mesa Inbound"),
            Some(&["c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e"][..])
        );
        assert_eq!(chapter_maps("not a real chapter"), None);
    }

    #[test]
    fn is_cited_map_name_covers_every_table() {
        assert!(is_cited_map_name(STARTMAP));
        assert!(is_cited_map_name(TRAINMAP));
        for map in HAZARD_COURSE_MAPS {
            assert!(is_cited_map_name(map), "hazard course map {map:?}");
        }
        for chapter in CHAPTERS {
            for map in chapter.maps {
                assert!(is_cited_map_name(map), "CHAPTERS map {map:?}");
            }
        }
        for entry in CHAPTER_MAPS {
            for map in entry.maps {
                assert!(is_cited_map_name(map), "CHAPTER_MAPS map {map:?}");
            }
        }
        assert!(!is_cited_map_name("not_a_real_map"));
        // Case-insensitive, matching the other lookups in this module.
        assert!(is_cited_map_name("C0A0"));
    }
}
