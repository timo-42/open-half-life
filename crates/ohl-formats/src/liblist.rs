//! `liblist.gam` game/mod declaration.
//!
//! See `docs/FORMAT_SOURCES.md` ("Game text formats") for the public
//! documentation this module was implemented from
//! (developer.valvesoftware.com/wiki/The_liblist.gam_File_Structure,
//! `Liblist.gam/Half-Life`).
//!
//! Grammar (bounded summary): flat, non-nested `key "value"` lines (Steam's
//! own parser requires a space between key and value; the engine itself
//! also tolerates a tab, so this parser accepts either); no lists, no
//! nesting. Well-known keys for Half-Life include `startmap`, `trainmap`,
//! `type`, `game`, `gamedll`/`gamedll_linux`/`gamedll_osx`, `mpentity`, and
//! `secure`.

use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::text_lines::{Lines, quoted_field, trim_ascii};

/// Bounds enforced while parsing a `liblist.gam` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest whole-file size this parser will look at.
    pub max_bytes: usize,
    /// The largest number of physical lines this parser will scan.
    pub max_lines: usize,
    /// The largest number of key/value entries this parser will collect.
    pub max_entries: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for the shipping
    /// `liblist.gam` (fewer than twenty keys) but far below what would let
    /// a malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bytes: 64 * 1024,
            max_lines: 4_096,
            max_entries: 512,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// A parsed `liblist.gam` file: zero or more `key "value"` entries, in file
/// order.
#[derive(Debug, Clone, Default)]
pub struct LibList<'a> {
    entries: Vec<(&'a str, &'a str)>,
}

impl<'a> LibList<'a> {
    /// Every parsed `(key, value)` pair, in file order.
    #[must_use]
    pub fn entries(&self) -> &[(&'a str, &'a str)] {
        &self.entries
    }

    /// Looks up a key's value by case-insensitive name. If the same key is
    /// set more than once, the last occurrence wins.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.entries
            .iter()
            .rev()
            .find(|(entry_key, _)| entry_key.eq_ignore_ascii_case(key))
            .map(|(_, value)| *value)
    }

    /// `startmap`: the map loaded for "New Game".
    #[must_use]
    pub fn startmap(&self) -> Option<&'a str> {
        self.get("startmap")
    }

    /// `trainmap`: the map loaded for "Training".
    #[must_use]
    pub fn trainmap(&self) -> Option<&'a str> {
        self.get("trainmap")
    }

    /// `game`: the display name of the game/mod.
    #[must_use]
    pub fn game(&self) -> Option<&'a str> {
        self.get("game")
    }

    /// `type`: for example `singleplayer_only`.
    #[must_use]
    pub fn game_type(&self) -> Option<&'a str> {
        self.get("type")
    }

    /// `mpentity`: the multiplayer spawn entity classname override.
    #[must_use]
    pub fn mpentity(&self) -> Option<&'a str> {
        self.get("mpentity")
    }
}

/// Parses a `liblist.gam` file.
///
/// Never panics on malformed input: a line without a well-formed pair of
/// double quotes, an empty key, or invalid UTF-8 in either field is skipped
/// rather than treated as an error. Returns [`FormatError::LimitExceeded`]
/// if `data`, the line count, or the entry count exceeds `limits`.
pub fn parse<'a>(data: &'a [u8], limits: &Limits) -> Result<LibList<'a>> {
    if data.len() > limits.max_bytes {
        return Err(FormatError::LimitExceeded);
    }

    let mut entries = Vec::new();
    let mut lines = Lines::new(data, limits.max_lines);

    while let Some(line) = lines.next_bounded()? {
        let trimmed = trim_ascii(line);
        if trimmed.is_empty() || trimmed.starts_with(b"//") {
            continue;
        }

        let Some((before, value)) = quoted_field(trimmed) else {
            continue;
        };
        let key = trim_ascii(before);
        if key.is_empty() {
            continue;
        }
        let (Ok(key), Ok(value)) = (core::str::from_utf8(key), core::str::from_utf8(value)) else {
            continue;
        };

        if entries.len() >= limits.max_entries {
            return Err(FormatError::LimitExceeded);
        }
        entries.push((key, value));
    }

    Ok(LibList { entries })
}
