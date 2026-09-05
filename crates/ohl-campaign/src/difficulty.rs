//! The Half-Life difficulty/`skill` cvar convention.
//!
//! Per TWHL "VERC: Adding New skill.cfg Entries", "Vlatitude: Editing
//! skill.cfg", and vault entry "The skill.cfg file" (URLs recorded in
//! `docs/FORMAT_SOURCES.md` "Game text formats" and
//! `.plan/m8-research.md` section 5): `skill.cfg` cvars follow the
//! convention `sk_<subject>_<property><N>` where `N` in `{1, 2, 3}` selects
//! easy/medium/hard, chosen at runtime by the engine's `skill` cvar
//! (`1`/`2`/`3`).

/// The three documented difficulty levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Difficulty {
    /// `skill 1`, cvar suffix `1`.
    Easy,
    /// `skill 2`, cvar suffix `2`.
    Medium,
    /// `skill 3`, cvar suffix `3`.
    Hard,
}

impl Difficulty {
    /// The `sk_<subject>_<property><N>` suffix digit for this difficulty
    /// (`1`/`2`/`3`).
    #[must_use]
    pub const fn skill_suffix(self) -> u8 {
        match self {
            Self::Easy => 1,
            Self::Medium => 2,
            Self::Hard => 3,
        }
    }

    /// The engine `skill` cvar value for this difficulty (identical to
    /// [`Difficulty::skill_suffix`], exposed separately since the two
    /// numbers are documented as two different things that happen to share
    /// a value range: the `skill` cvar selects the difficulty, and the
    /// per-cvar suffix is what `skill.cfg` uses to encode it).
    #[must_use]
    pub const fn skill_cvar_value(self) -> u8 {
        self.skill_suffix()
    }

    /// Maps a raw `skill` cvar value (`1`/`2`/`3`) back to a
    /// [`Difficulty`], returning `None` for anything else instead of
    /// panicking.
    #[must_use]
    pub const fn from_skill_cvar_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Easy),
            2 => Some(Self::Medium),
            3 => Some(Self::Hard),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Difficulty;

    #[test]
    fn suffixes_match_documented_convention() {
        assert_eq!(Difficulty::Easy.skill_suffix(), 1);
        assert_eq!(Difficulty::Medium.skill_suffix(), 2);
        assert_eq!(Difficulty::Hard.skill_suffix(), 3);
    }

    #[test]
    fn round_trips_through_cvar_value() {
        for difficulty in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
            let value = difficulty.skill_cvar_value();
            assert_eq!(Difficulty::from_skill_cvar_value(value), Some(difficulty));
        }
    }

    #[test]
    fn rejects_out_of_range_cvar_values() {
        assert_eq!(Difficulty::from_skill_cvar_value(0), None);
        assert_eq!(Difficulty::from_skill_cvar_value(4), None);
        assert_eq!(Difficulty::from_skill_cvar_value(255), None);
    }
}
