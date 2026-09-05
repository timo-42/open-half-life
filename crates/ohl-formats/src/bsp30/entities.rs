//! The entities lump: a NUL-terminated text block of `{ "key" "value" ... }`
//! records (Unofficial Quake Specs section 4, lump 0 "entities").

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::bsp30::Limits;
use crate::error::{FormatError, Result};

/// One entity's key/value pairs.
pub type Entity = BTreeMap<String, String>;

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
    /// has none), bounded by `max_len` bytes.
    fn read_quoted(&mut self, max_len: usize) -> Result<String> {
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
        core::str::from_utf8(&self.text[start..end])
            .map(alloc::string::ToString::to_string)
            .map_err(|_| FormatError::InvalidText)
    }
}

/// Parses the entities lump into an ordered list of key/value maps.
///
/// Rejects a lump that does not end with a NUL terminator, contains invalid
/// UTF-8, is malformed (unbalanced braces, missing quotes), or exceeds
/// `limits`. Never panics on malformed input.
pub fn parse(lump: &[u8], limits: &Limits) -> Result<Vec<Entity>> {
    if lump.is_empty() {
        return Ok(Vec::new());
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
                    let key = cursor.read_quoted(limits.max_entity_string_bytes)?;
                    cursor.skip_whitespace();
                    let value = cursor.read_quoted(limits.max_entity_string_bytes)?;
                    entity.insert(key, value);
                }
                _ => return Err(FormatError::InvalidText),
            }
        }
        entities.push(entity);
    }

    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::bsp30::Limits;

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
    fn empty_lump_is_no_entities() {
        assert_eq!(parse(&[], &Limits::default()).expect("empty ok").len(), 0);
    }
}
