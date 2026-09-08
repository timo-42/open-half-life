//! The entities lump: a NUL-terminated text block of `{ "key" "value" ... }`
//! records (Unofficial Quake Specs section 4, lump 0 "entities").

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bsp30::Limits;
use crate::error::{FormatError, Result};

/// One entity's key/value pairs.
pub type Entity = BTreeMap<String, String>;

/// What [`parse_with_report`] had to relax to read a lump, as aggregate
/// counts only (nothing here is derived from a map's own strings).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EntityLumpReport {
    /// How many quoted strings in this lump were not valid UTF-8 and were
    /// therefore decoded byte-per-byte (see [`decode_quoted`]). Zero for a
    /// lump whose every string is plain UTF-8 (which includes every
    /// pure-ASCII lump).
    pub relaxed_strings: usize,
}

/// Decodes one quoted string's raw bytes.
///
/// The published grammar for this lump (Unofficial Quake Specs section 4)
/// describes it as a block of text with quoted keys and values and defines
/// no character encoding beyond the ASCII the grammar's own delimiters live
/// in: a quoted value is a run of bytes ending at the next `"`. Requiring
/// the whole run to be valid UTF-8 is therefore a constraint this project
/// added (because it decodes into a Rust `String`), not one the format
/// imposes, and a map authored on a legacy single-byte Windows codepage can
/// legally carry a byte in `0x80..=0xFF` inside a quoted value — an
/// apostrophe or a dash in a human-readable value, typically.
///
/// This project's rule: decode the run as UTF-8 when it is valid UTF-8, and
/// otherwise map every byte to the Unicode scalar of the same value (the
/// Latin-1 / ISO-8859-1 range, i.e. `char::from(u8)`). That mapping is
/// total, deterministic, never fails, never loses or replaces a byte, and
/// leaves every ASCII byte — which is all the format's own structure and
/// all of the classname/targetname vocabulary the game logic matches on —
/// exactly where it was. Which of the two paths was taken is counted in
/// [`EntityLumpReport::relaxed_strings`] so a caller can report that the
/// map needed relaxing without inspecting (or logging) any of its text.
fn decode_quoted(raw: &[u8], report: &mut EntityLumpReport) -> String {
    if let Ok(text) = core::str::from_utf8(raw) {
        alloc::string::ToString::to_string(text)
    } else {
        report.relaxed_strings += 1;
        raw.iter().map(|&byte| char::from(byte)).collect()
    }
}

struct Cursor<'a> {
    text: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// Reads a `"..."` quoted string with no escape processing (the format
    /// has none), bounded by `max_len` bytes, and decodes it with
    /// [`decode_quoted`].
    fn read_quoted(&mut self, max_len: usize, report: &mut EntityLumpReport) -> Result<String> {
        if self.bump() != Some(b'"') {
            return Err(FormatError::InvalidText);
        }
        let start = self.pos;
        loop {
            match self.bump() {
                Some(b'"') => break,
                Some(_) => {
                    if self.pos - start > max_len {
                        return Err(FormatError::LimitExceeded);
                    }
                }
                None => return Err(FormatError::InvalidText),
            }
        }
        let end = self.pos - 1;
        Ok(decode_quoted(&self.text[start..end], report))
    }
}

/// Parses the entities lump into an ordered list of key/value maps,
/// discarding [`parse_with_report`]'s report.
///
/// # Errors
/// As [`parse_with_report`].
pub fn parse(lump: &[u8], limits: &Limits) -> Result<Vec<Entity>> {
    parse_with_report(lump, limits).map(|(entities, _)| entities)
}

/// Parses the entities lump into an ordered list of key/value maps, plus an
/// [`EntityLumpReport`] describing what had to be relaxed to read it.
///
/// Rejects a lump that does not end with a NUL terminator, is malformed
/// (unbalanced braces, missing quotes), or exceeds `limits`. A quoted
/// string that is not valid UTF-8 is *not* rejected: see [`decode_quoted`]
/// for the encoding rule and for why the format permits it. Never panics on
/// malformed input.
///
/// # Errors
/// [`FormatError::InvalidText`] for a missing terminator, an interior NUL,
/// or a structural violation of the `{ "key" "value" ... }` grammar;
/// [`FormatError::LimitExceeded`] when the lump, its entity count, or one of
/// its strings exceeds `limits`.
pub fn parse_with_report(lump: &[u8], limits: &Limits) -> Result<(Vec<Entity>, EntityLumpReport)> {
    let mut report = EntityLumpReport::default();
    if lump.is_empty() {
        return Ok((Vec::new(), report));
    }
    if lump.len() > limits.max_entities_bytes {
        return Err(FormatError::LimitExceeded);
    }
    if *lump.last().expect("checked non-empty above") != 0 {
        return Err(FormatError::InvalidText);
    }
    // Exactly one trailing NUL is expected; an interior NUL is malformed.
    let text = &lump[..lump.len() - 1];
    if text.contains(&0) {
        return Err(FormatError::InvalidText);
    }

    let mut cursor = Cursor { text, pos: 0 };
    let mut entities = Vec::new();

    loop {
        cursor.skip_whitespace();
        if cursor.peek().is_none() {
            break;
        }
        if cursor.bump() != Some(b'{') {
            return Err(FormatError::InvalidText);
        }
        if entities.len() >= limits.max_entities {
            return Err(FormatError::LimitExceeded);
        }
        let mut entity = Entity::new();
        loop {
            cursor.skip_whitespace();
            match cursor.peek() {
                Some(b'}') => {
                    cursor.pos += 1;
                    break;
                }
                Some(b'"') => {
                    let key = cursor.read_quoted(limits.max_entity_string_bytes, &mut report)?;
                    cursor.skip_whitespace();
                    let value = cursor.read_quoted(limits.max_entity_string_bytes, &mut report)?;
                    entity.insert(key, value);
                }
                _ => return Err(FormatError::InvalidText),
            }
        }
        entities.push(entity);
    }

    Ok((entities, report))
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_with_report};
    use crate::bsp30::Limits;
    use alloc::string::String;
    use alloc::vec::Vec;

    #[test]
    fn parses_two_entities() {
        let text = b"{\n\"classname\" \"worldspawn\"\n}\n{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 0 0\"\n}\n\0";
        let entities = parse(text, &Limits::default()).expect("valid entities lump");
        assert_eq!(entities.len(), 2);
        assert_eq!(
            entities[0].get("classname").map(String::as_str),
            Some("worldspawn")
        );
        assert_eq!(entities[1].get("origin").map(String::as_str), Some("0 0 0"));
    }

    #[test]
    fn rejects_missing_terminator() {
        let text = b"{\"classname\" \"worldspawn\"}";
        assert!(parse(text, &Limits::default()).is_err());
    }

    #[test]
    fn rejects_unbalanced_braces() {
        let text = b"{\"classname\" \"worldspawn\"\0";
        assert!(parse(text, &Limits::default()).is_err());
    }

    #[test]
    fn a_non_utf8_byte_inside_a_quoted_value_is_decoded_byte_per_byte() {
        // The failure class this test pins: a single byte in `0x80..=0xFF`
        // inside an otherwise well-formed quoted value, as a map authored
        // on a legacy single-byte Windows codepage carries. The lump's
        // grammar is intact; only the UTF-8 assumption was violated.
        let mut text = Vec::new();
        text.extend_from_slice(b"{\n\"classname\" \"worldspawn\"\n\"message\" \"it");
        text.push(0x92);
        text.extend_from_slice(b"s here\"\n}\n\0");
        let (entities, report) =
            parse_with_report(&text, &Limits::default()).expect("a non-UTF-8 value is legal");
        assert_eq!(entities.len(), 1);
        assert_eq!(report.relaxed_strings, 1);
        let value = entities[0].get("message").expect("the value is present");
        // Latin-1: the byte became the scalar of the same value, and every
        // surrounding ASCII byte is untouched.
        assert_eq!(value.as_str(), "it\u{92}s here");
    }

    #[test]
    fn a_non_utf8_byte_inside_a_quoted_key_is_decoded_the_same_way() {
        let mut text = Vec::new();
        text.extend_from_slice(b"{\n\"k");
        text.push(0xE9);
        text.extend_from_slice(b"y\" \"v\"\n}\n\0");
        let (entities, report) = parse_with_report(&text, &Limits::default()).expect("legal");
        assert_eq!(report.relaxed_strings, 1);
        assert_eq!(entities[0].len(), 1);
        assert_eq!(
            entities[0].keys().next().map(String::as_str),
            Some("k\u{e9}y")
        );
    }

    #[test]
    fn valid_utf8_is_kept_as_utf8_and_never_counted_as_relaxed() {
        let text = "{\n\"classname\" \"worldspawn\"\n\"message\" \"caf\u{e9} \u{2014} ok\"\n}\n\0";
        let (entities, report) =
            parse_with_report(text.as_bytes(), &Limits::default()).expect("valid UTF-8 parses");
        assert_eq!(report.relaxed_strings, 0);
        assert_eq!(
            entities[0].get("message").map(String::as_str),
            Some("caf\u{e9} \u{2014} ok")
        );
    }

    #[test]
    fn a_relaxed_string_does_not_stop_later_entities_from_parsing() {
        let mut text = Vec::new();
        text.extend_from_slice(b"{\n\"classname\" \"worldspawn\"\n\"message\" \"");
        text.push(0x92);
        text.extend_from_slice(
            b"\"\n}\n{\n\"classname\" \"info_player_start\"\n\"origin\" \"1 2 3\"\n}\n\0",
        );
        let (entities, report) = parse_with_report(&text, &Limits::default()).expect("legal");
        assert_eq!(entities.len(), 2);
        assert_eq!(report.relaxed_strings, 1);
        assert_eq!(
            entities[1].get("classname").map(String::as_str),
            Some("info_player_start")
        );
        assert_eq!(entities[1].get("origin").map(String::as_str), Some("1 2 3"));
    }

    #[test]
    fn a_structurally_broken_lump_is_still_rejected() {
        // Relaxing the encoding must not relax the grammar: an unterminated
        // quoted string at the end of the lump, and a block that never
        // closes, both still fail.
        assert!(parse(b"{\n\"classname\" \"worldspawn\n\0", &Limits::default()).is_err());
        assert!(parse(b"{\n\"classname\" \"worldspawn\"\n\0", &Limits::default()).is_err());
        assert!(parse(b"{\n\"key\" \"value\"\n}\n}\n\0", &Limits::default()).is_err());
    }

    #[test]
    fn an_over_long_value_still_exceeds_the_limit() {
        let limits = Limits {
            max_entity_string_bytes: 8,
            ..Limits::default()
        };
        let text = b"{\n\"classname\" \"a_very_long_value_indeed\"\n}\n\0";
        assert!(parse(text, &limits).is_err());
    }

    #[test]
    fn empty_lump_is_no_entities() {
        assert_eq!(parse(&[], &Limits::default()).expect("empty ok").len(), 0);
    }
}
