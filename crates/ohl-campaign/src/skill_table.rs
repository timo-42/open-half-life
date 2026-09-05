//! A `skill.cfg`-backed difficulty lookup table.
//!
//! This crate deliberately does not depend on `ohl-formats` (see the
//! `xtask/src/graph.rs` dependency policy: `ohl-campaign -> ohl-core`
//! only), so [`SkillTable`] is built from already-parsed `(cvar, value)`
//! string pairs rather than from raw `skill.cfg` bytes directly. A caller
//! that has parsed a `skill.cfg` file with `ohl_formats::skill_cfg::parse`
//! adapts its `Entry { cvar, value }` records into `(&str, &str)` pairs
//! (for example `entries().iter().map(|e| (e.cvar, e.value))`) before
//! calling [`SkillTable::from_entries`].

use alloc::string::String;
use alloc::vec::Vec;

use ohl_core::SanitizedError;

use crate::difficulty::Difficulty;

/// Bounds enforced while building a [`SkillTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest number of `(cvar, value)` entries this table will hold.
    pub max_entries: usize,
}

impl Limits {
    /// Conservative default, generous enough for the shipping `skill.cfg`
    /// (on the order of a few hundred cvars).
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_entries: 16_384,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::conservative()
    }
}

/// A difficulty-aware view over a parsed `skill.cfg` file's cvars.
#[derive(Debug, Clone, Default)]
pub struct SkillTable {
    entries: Vec<(String, String)>,
}

impl SkillTable {
    /// Builds a table from `(cvar, value)` pairs, in the order a
    /// `skill.cfg` executes them (later duplicates of the same cvar name
    /// take precedence in [`SkillTable::lookup`], matching a file executed
    /// top to bottom).
    ///
    /// Returns [`SanitizedError::InvalidInput`] rather than panicking or
    /// silently truncating if `entries` yields more than
    /// `limits.max_entries` pairs.
    pub fn from_entries<'a, I>(entries: I, limits: &Limits) -> Result<Self, SanitizedError>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let mut collected = Vec::new();
        for (cvar, value) in entries {
            if collected.len() >= limits.max_entries {
                return Err(SanitizedError::InvalidInput);
            }
            collected.push((String::from(cvar), String::from(value)));
        }
        Ok(Self { entries: collected })
    }

    /// Every stored `(cvar, value)` pair, in the order they were provided.
    #[must_use]
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// Looks up `<subject_property><difficulty-suffix>` (for example
    /// `lookup("sk_headcrab_health", Difficulty::Hard)` reads the cvar
    /// `sk_headcrab_health3`), matching the cvar name case-insensitively.
    /// If the same cvar was provided more than once, the last occurrence
    /// wins.
    #[must_use]
    pub fn lookup(&self, subject_property: &str, difficulty: Difficulty) -> Option<&str> {
        let suffix = difficulty.skill_suffix();
        self.entries
            .iter()
            .rev()
            .find(|(cvar, _)| cvar_matches(cvar, subject_property, suffix))
            .map(|(_, value)| value.as_str())
    }
}

/// Whether `cvar` is exactly `subject_property` followed by the single
/// ASCII digit `suffix`, compared case-insensitively.
fn cvar_matches(cvar: &str, subject_property: &str, suffix: u8) -> bool {
    let cvar = cvar.as_bytes();
    let subject_property = subject_property.as_bytes();
    if cvar.len() != subject_property.len() + 1 {
        return false;
    }
    let (prefix, last) = cvar.split_at(subject_property.len());
    prefix.eq_ignore_ascii_case(subject_property) && last == [b'0' + suffix]
}

#[cfg(test)]
mod tests {
    use super::{Difficulty, Limits, SkillTable};

    // Invented, project-authored `skill.cfg` fixture (not derived from any
    // real game installation): a handful of `sk_<subject>_<property><N>`
    // rows following the documented naming convention.
    const FIXTURE: &[(&str, &str)] = &[
        ("sk_headcrab_health1", "10"),
        ("sk_headcrab_health2", "15"),
        ("sk_headcrab_health3", "20"),
        ("sk_headcrab_dmg_bite1", "2"),
        ("sk_headcrab_dmg_bite2", "4"),
        ("sk_headcrab_dmg_bite3", "6"),
        ("sk_plr_9mm_bullet1", "8"),
        ("sk_plr_9mm_bullet2", "8"),
        ("sk_plr_9mm_bullet3", "6"),
    ];

    fn fixture_table() -> SkillTable {
        SkillTable::from_entries(FIXTURE.iter().copied(), &Limits::default())
            .expect("fixture is well within limits")
    }

    #[test]
    fn looks_up_by_subject_and_difficulty() {
        let table = fixture_table();
        assert_eq!(
            table.lookup("sk_headcrab_health", Difficulty::Easy),
            Some("10")
        );
        assert_eq!(
            table.lookup("sk_headcrab_health", Difficulty::Medium),
            Some("15")
        );
        assert_eq!(
            table.lookup("sk_headcrab_health", Difficulty::Hard),
            Some("20")
        );
    }

    #[test]
    fn missing_cvar_is_none() {
        let table = fixture_table();
        assert_eq!(table.lookup("sk_zombie_health", Difficulty::Easy), None);
    }

    #[test]
    fn later_duplicate_wins() {
        let table = SkillTable::from_entries(
            [("sk_headcrab_health1", "10"), ("sk_headcrab_health1", "99")],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            table.lookup("sk_headcrab_health", Difficulty::Easy),
            Some("99")
        );
    }

    #[test]
    fn is_case_insensitive_on_the_cvar_name() {
        let table =
            SkillTable::from_entries([("SK_Headcrab_Health1", "10")], &Limits::default()).unwrap();
        assert_eq!(
            table.lookup("sk_headcrab_health", Difficulty::Easy),
            Some("10")
        );
    }

    #[test]
    fn rejects_too_many_entries() {
        let limits = Limits { max_entries: 1 };
        let result = SkillTable::from_entries([("sk_a_b1", "1"), ("sk_a_b2", "2")], &limits);
        assert!(result.is_err());
    }
}
