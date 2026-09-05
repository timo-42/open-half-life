//! Turning a container's recorded name bytes into an offerable spelling.
//!
//! An `entry_batch` spelling is untrusted metadata, but it is not a free-for-all:
//!
//! 1. the wire type [`ohl_parser_protocol::ArchiveSpelling`] accepts printable
//!    ASCII only;
//! 2. the parent's catalog planner refuses an enumeration that contains one
//!    unusable spelling, a case-only alias, or a file that aliases another
//!    entry's directory prefix — the *whole* enumeration, not the entry;
//! 3. the parent's destination policy is [`ohl_payload::PayloadPath`].
//!
//! Rule 2 is why the filtering lives here rather than in the parent: a single
//! duplicate in a thousand-record script would otherwise fail the import. The
//! worker offers only spellings that satisfy all three rules, and simply does
//! not offer the rest; the parent still re-validates every one of them,
//! because nothing a worker says is trusted.
//!
//! The recorded bytes are used exactly as recorded, apart from the separator
//! folding [`ohl_payload::PayloadPath`] already performs: no variable is
//! expanded, no prefix is invented and no component is dropped.
//!
//! Nothing here logs, formats or returns a name outside the caller's own
//! buffer: [`SpellingSet`] holds case-folded keys for collision detection and
//! is `Debug`-redacted.

use alloc::string::String;
use alloc::vec::Vec;

use ohl_payload::PayloadPath;

/// The largest spelling this crate will offer, matching the wire ceiling.
pub const MAXIMUM_SPELLING_BYTES: usize = 4_096;

/// Why a recorded name was not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpellingRejection {
    /// The bytes are not a usable portable destination name.
    Unusable,
    /// Another accepted entry already claims this name, ignoring ASCII case.
    Duplicate,
    /// The name is a directory prefix of another accepted entry, or has one
    /// as a prefix, so the two cannot both exist on a filesystem.
    Alias,
}

/// The accepted spellings of one enumeration, keyed for collision detection.
#[derive(Default)]
pub struct SpellingSet {
    /// Case-folded keys, kept sorted so a collision is a binary search.
    keys: Vec<String>,
}

impl core::fmt::Debug for SpellingSet {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SpellingSet")
            .field("accepted", &self.keys.len())
            .finish()
    }
}

impl SpellingSet {
    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self { keys: Vec::new() }
    }

    /// How many spellings were accepted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether nothing was accepted yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Validates `raw` and, if it is offerable, records it and returns the
    /// normalised spelling.
    ///
    /// # Errors
    /// One [`SpellingRejection`] code; the caller counts it and moves on.
    pub fn accept(&mut self, raw: &[u8]) -> Result<String, SpellingRejection> {
        if raw.is_empty() || raw.len() > MAXIMUM_SPELLING_BYTES {
            return Err(SpellingRejection::Unusable);
        }
        let text = core::str::from_utf8(raw).map_err(|_| SpellingRejection::Unusable)?;
        let path = PayloadPath::parse(text).map_err(|_| SpellingRejection::Unusable)?;
        if path.as_str().len() > MAXIMUM_SPELLING_BYTES {
            return Err(SpellingRejection::Unusable);
        }
        let key = path.portability_key();
        match self.keys.binary_search_by(|held| held.as_str().cmp(key)) {
            Ok(_) => return Err(SpellingRejection::Duplicate),
            Err(at) => {
                let before = at.checked_sub(1).and_then(|index| self.keys.get(index));
                let after = self.keys.get(at);
                if before.is_some_and(|held| aliases(held, key))
                    || after.is_some_and(|held| aliases(key, held))
                {
                    return Err(SpellingRejection::Alias);
                }
                self.keys.insert(at, String::from(key));
            }
        }
        Ok(String::from(path.as_str()))
    }
}

/// Whether `shorter` is a directory prefix of `longer`, which is exactly the
/// aliasing the parent's catalog planner refuses.
fn aliases(shorter: &str, longer: &str) -> bool {
    longer.len() > shorter.len()
        && longer.starts_with(shorter)
        && longer.as_bytes()[shorter.len()] == b'/'
}

/// The reserved directory every unnamed stream is offered under.
pub const UNNAMED_DIRECTORY: &str = "unnamed";

/// The synthetic spelling of the unnamed stream at chain index `index`.
#[must_use]
pub fn unnamed_spelling(index: u32) -> String {
    let mut text = String::from(UNNAMED_DIRECTORY);
    text.push('/');
    let mut digits = [0u8; 10];
    let mut value = index;
    let mut written = 0;
    loop {
        digits[written] = b'0' + u8::try_from(value % 10).unwrap_or(0);
        written += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in digits[..written].iter().rev() {
        text.push(char::from(*digit));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{SpellingRejection, SpellingSet, unnamed_spelling};

    #[test]
    fn separators_fold_and_the_spelling_is_kept_as_recorded() {
        let mut set = SpellingSet::new();
        assert_eq!(set.accept(b"%MAINDIR%\\valve\\halflife.wad").unwrap(), "%MAINDIR%/valve/halflife.wad");
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn unusable_names_are_refused_one_by_one() {
        let mut set = SpellingSet::new();
        assert_eq!(set.accept(b""), Err(SpellingRejection::Unusable));
        assert_eq!(set.accept(b"..\\escape"), Err(SpellingRejection::Unusable));
        assert_eq!(set.accept(b"C:\\rooted"), Err(SpellingRejection::Unusable));
        assert_eq!(set.accept(b"\\rooted"), Err(SpellingRejection::Unusable));
        assert_eq!(set.accept(b"bad\x01byte"), Err(SpellingRejection::Unusable));
        assert_eq!(set.accept(b"nul.txt"), Err(SpellingRejection::Unusable));
        assert!(set.is_empty());
    }

    #[test]
    fn a_case_only_alias_is_a_duplicate() {
        let mut set = SpellingSet::new();
        set.accept(b"valve\\Sound\\ambience.wav").unwrap();
        assert_eq!(
            set.accept(b"VALVE\\sound\\AMBIENCE.WAV"),
            Err(SpellingRejection::Duplicate)
        );
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn a_file_that_aliases_a_directory_is_refused_either_way_round() {
        let mut set = SpellingSet::new();
        set.accept(b"valve\\models\\player.mdl").unwrap();
        assert_eq!(set.accept(b"valve\\models"), Err(SpellingRejection::Alias));

        let mut reversed = SpellingSet::new();
        reversed.accept(b"valve\\models").unwrap();
        assert_eq!(
            reversed.accept(b"valve\\models\\player.mdl"),
            Err(SpellingRejection::Alias)
        );
    }

    #[test]
    fn unnamed_streams_get_a_reserved_synthetic_spelling() {
        assert_eq!(unnamed_spelling(0), "unnamed/0");
        assert_eq!(unnamed_spelling(1_037), "unnamed/1037");
    }
}
