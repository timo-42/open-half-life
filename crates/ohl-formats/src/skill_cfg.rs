//! `skill.cfg` per-difficulty cvar definitions.
//!
//! See `docs/FORMAT_SOURCES.md` ("Game text formats") for the public
//! documentation this module was implemented from (TWHL "VERC: Adding New
//! skill.cfg Entries", "Vlatitude: Editing skill.cfg", vault entry "The
//! skill.cfg file").
//!
//! Grammar (bounded summary): one cvar-set per non-blank, non-comment line,
//! `cvar "value"` (a bare identifier followed by a double-quoted value);
//! by convention most entries are named `sk_<subject>_<property><1|2|3>`
//! where the trailing digit selects easy/medium/hard, but this parser
//! treats every line the same generic way and does not require the naming
//! convention.

use alloc::vec::Vec;

use crate::error::{FormatError, Result};
use crate::text_lines::{Lines, quoted_field, trim_ascii};

/// Bounds enforced while parsing a `skill.cfg` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest whole-file size this parser will look at.
    pub max_bytes: usize,
    /// The largest number of physical lines this parser will scan.
    pub max_lines: usize,
    /// The largest number of cvar entries this parser will collect.
    pub max_entries: usize,
}

impl Limits {
    /// Conservative defaults, generous enough for the shipping `skill.cfg`
    /// (on the order of a few hundred cvars) but far below what would let a
    /// malformed file force pathological work.
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_bytes: 1024 * 1024,
            max_lines: 65_536,
            max_entries: 16_384,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// One parsed `cvar "value"` line.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// The cvar name (for example `sk_headcrab_health1`).
    pub cvar: &'a str,
    /// The quoted value, unescaped exactly as written (no escape-sequence
    /// processing is documented for this format).
    pub value: &'a str,
}

/// A parsed `skill.cfg` file: zero or more cvar entries, in file order.
#[derive(Debug, Clone, Default)]
pub struct SkillCfg<'a> {
    entries: Vec<Entry<'a>>,
}

impl<'a> SkillCfg<'a> {
    /// Every parsed entry, in file order.
    #[must_use]
    pub fn entries(&self) -> &[Entry<'a>] {
        &self.entries
    }

    /// Looks up a cvar's value by exact (case-sensitive, matching GoldSrc
    /// cvar convention) name. If the same cvar is set more than once, the
    /// last occurrence wins (matching a file executed top to bottom).
    #[must_use]
    pub fn get(&self, cvar: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.cvar == cvar)
            .map(|entry| entry.value)
    }
}

/// Parses a `skill.cfg` file.
///
/// Never panics on malformed input: a line missing a bare cvar name, a
/// well-formed pair of double quotes, or valid UTF-8 in either field is
/// skipped rather than treated as an error. Returns
/// [`FormatError::LimitExceeded`] if `data`, the line count, or the entry
/// count exceeds `limits`.
pub fn parse<'a>(data: &'a [u8], limits: &Limits) -> Result<SkillCfg<'a>> {
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
        let cvar = trim_ascii(before);
        if cvar.is_empty() {
            continue;
        }
        let (Ok(cvar), Ok(value)) = (core::str::from_utf8(cvar), core::str::from_utf8(value))
        else {
            continue;
        };

        if entries.len() >= limits.max_entries {
            return Err(FormatError::LimitExceeded);
        }
        entries.push(Entry { cvar, value });
    }

    Ok(SkillCfg { entries })
}
